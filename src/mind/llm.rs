//! Model clients over raw HTTP: Anthropic Messages API and OpenAI-compatible chat completions
//! (DeepSeek, OpenAI, local servers). Request bodies keep L1/L2 as a stable prefix so prompt
//! caching hits; transcripts are append-only.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::config::{ModelEntry, ProviderCfg, ProviderKind};

const ANTHROPIC_VERSION: &str = "2026-10-21";
const ANTHROPIC_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_TOKENS: u32 = 16_000;

#[derive(Debug, Clone)]
pub enum Turn {
    User(String),
    /// `raw` is the provider-native content (Anthropic content blocks, thinking included).
    /// It is replayed unchanged to Anthropic models; other providers get `text`.
    Assistant { text: String, raw: Option<Value> },
}

pub struct Prompt<'a> {
    pub l1: &'a str,
    pub l2: &'a str,
    pub turns: &'a [Turn],
    pub effort: Option<&'a str>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub uncached_input: u64,
    pub cached_input: u64,
    pub cache_write: u64,
    pub output: u64,
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub text: String,
    pub raw: Option<Value>,
    pub usage: Usage,
    pub latency: Duration,
}

pub fn cost_usd(m: &ModelEntry, u: &Usage) -> f64 {
    let write_price = m.c.cache_write.unwrap_or(m.c.input * 1.25);
    (u.uncached_input as f64 * m.c.input
        + u.cached_input as f64 * m.c.cached
        + u.cache_write as f64 * write_price
        + u.output as f64 * m.c.output)
        / 1_000_000.0
}

pub fn anthropic_body(m: &ModelEntry, p: &Prompt<'_>) -> Value {
    let ephemeral = json!({"type": "ephemeral"});
    let mut system = vec![json!({"type": "text", "text": p.l1, "cache_control": ephemeral})];
    if !p.l2.is_empty() {
        system.push(json!({"type": "text", "text": p.l2, "cache_control": ephemeral}));
    }
    let messages: Vec<Value> = p
        .turns
        .iter()
        .map(|t| match t {
            Turn::User(text) => json!({"role": "user", "content": text}),
            Turn::Assistant { raw: Some(raw), .. } => json!({"role": "assistant", "content": raw}),
            Turn::Assistant { text, raw: None } => json!({"role": "assistant", "content": text}),
        })
        .collect();
    let mut body = json!({
        "model": m.model,
        "max_tokens": MAX_TOKENS,
        // Automatic breakpoint for the growing transcript; explicit ones above guard L1/L2.
        "cache_control": ephemeral,
        "system": system,
        "messages": messages,
    });
    if let Some(effort) = p.effort {
        body["output_config"] = json!({"effort": effort});
    }
    if let Some(fb) = &m.fallbacks {
        body["fallbacks"] = json!(fb);
    }
    body
}

pub fn openai_body(m: &ModelEntry, p: &Prompt<'_>) -> Value {
    let system = if p.l2.is_empty() { p.l1.to_string() } else { format!("{}\n\n{}", p.l1, p.l2) };
    let mut messages = vec![json!({"role": "system", "content": system})];
    messages.extend(p.turns.iter().map(|t| match t {
        Turn::User(text) => json!({"role": "user", "content": text}),
        Turn::Assistant { text, .. } => json!({"role": "assistant", "content": text}),
    }));
    json!({"model": m.model, "messages": messages, "stream": false})
}

fn u64_at(v: &Value, ptr: &str) -> u64 {
    v.pointer(ptr).and_then(Value::as_u64).unwrap_or(0)
}

pub fn parse_anthropic(v: &Value) -> Result<(String, Value, Usage)> {
    if v.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        bail!("model refused: {}", v.get("stop_details").unwrap_or(&Value::Null));
    }
    let content = v.get("content").cloned().ok_or_else(|| anyhow!("response has no content"))?;
    let text = content
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let usage = Usage {
        uncached_input: u64_at(v, "/usage/input_tokens"),
        cached_input: u64_at(v, "/usage/cache_read_input_tokens"),
        cache_write: u64_at(v, "/usage/cache_creation_input_tokens"),
        output: u64_at(v, "/usage/output_tokens"),
    };
    Ok((text, content, usage))
}

pub fn parse_openai(v: &Value) -> Result<(String, Usage)> {
    let text = v
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("response has no choices[0].message.content"))?
        .to_string();
    let total = u64_at(v, "/usage/prompt_tokens");
    // OpenAI reports prompt_tokens_details.cached_tokens; DeepSeek reports prompt_cache_hit_tokens.
    let cached = u64_at(v, "/usage/prompt_tokens_details/cached_tokens").max(u64_at(v, "/usage/prompt_cache_hit_tokens"));
    let usage = Usage {
        uncached_input: total.saturating_sub(cached),
        cached_input: cached,
        cache_write: 0,
        output: u64_at(v, "/usage/completion_tokens"),
    };
    Ok((text, usage))
}

pub struct LlmClient {
    http: reqwest::Client,
    providers: BTreeMap<String, ProviderCfg>,
}

impl LlmClient {
    pub fn new(providers: BTreeMap<String, ProviderCfg>) -> Result<Self> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(600)).build()?;
        Ok(LlmClient { http, providers })
    }

    /// A model is usable when its provider needs no key or the key is set.
    pub fn has_credentials(&self, m: &ModelEntry) -> bool {
        match self.providers.get(&m.provider).and_then(|p| p.api_key_env.as_deref()) {
            None => self.providers.contains_key(&m.provider),
            Some(var) => std::env::var(var).is_ok_and(|k| !k.is_empty()),
        }
    }

    fn api_key(provider: &ProviderCfg) -> Result<Option<String>> {
        match &provider.api_key_env {
            None => Ok(None),
            Some(var) => std::env::var(var).map(Some).with_context(|| format!("env var {var} is not set")),
        }
    }

    pub async fn complete(&self, m: &ModelEntry, p: &Prompt<'_>) -> Result<Reply> {
        let provider = self.providers.get(&m.provider).ok_or_else(|| anyhow!("unknown provider {}", m.provider))?;
        let key = Self::api_key(provider)?;
        let started = Instant::now();
        let reply = match provider.kind {
            ProviderKind::Anthropic => {
                let base = provider.base_url.as_deref().unwrap_or("https://api.anthropic.com");
                let mut req = self
                    .http
                    .post(format!("{}/v1/messages", base.trim_end_matches('/')))
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .json(&anthropic_body(m, p));
                if let Some(key) = &key {
                    req = req.header("x-api-key", key);
                }
                if m.fallbacks.is_some() {
                    req = req.header("anthropic-beta", ANTHROPIC_FALLBACK_BETA);
                }
                let v = send_json(req).await?;
                let (text, raw, usage) = parse_anthropic(&v)?;
                Reply { text, raw: Some(raw), usage, latency: started.elapsed() }
            }
            ProviderKind::OpenaiCompat => {
                let base = provider.base_url.as_deref().ok_or_else(|| anyhow!("provider {} needs base_url", m.provider))?;
                let mut req = self
                    .http
                    .post(format!("{}/chat/completions", base.trim_end_matches('/')))
                    .json(&openai_body(m, p));
                if let Some(key) = &key {
                    req = req.bearer_auth(key);
                }
                let v = send_json(req).await?;
                let (text, usage) = parse_openai(&v)?;
                Reply { text, raw: None, usage, latency: started.elapsed() }
            }
        };
        Ok(reply)
    }
}

async fn send_json(req: reqwest::RequestBuilder) -> Result<Value> {
    let resp = req.send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        let snippet: String = body.chars().take(500).collect();
        bail!("HTTP {status}: {snippet}");
    }
    serde_json::from_str(&body).context("response is not JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Price;

    fn model(fallbacks: Option<&str>) -> ModelEntry {
        ModelEntry {
            id: "opus".into(),
            provider: "anthropic".into(),
            model: "claude-opus-5-5".into(),
            q: 9.5,
            c: Price { input: 4.0, cached: 0.2, output: 20.0, cache_write: None },
            l: 3.0,
            local: false,
            fallbacks: fallbacks.map(String::from),
        }
    }

    /// Turn n's request must be a byte prefix of turn n+1's: same system, same earlier messages.
    #[test]
    fn anthropic_requests_are_append_only() {
        let m = model(Some("default"));
        let raw = json!([{"type": "thinking", "thinking": "", "signature": "sig"}, {"type": "text", "text": "嗯"}]);
        let mut turns = vec![Turn::User("t1".into())];
        let first = anthropic_body(&m, &Prompt { l1: "L1", l2: "L2", turns: &turns, effort: Some("low") });
        turns.push(Turn::Assistant { text: "嗯".into(), raw: Some(raw.clone()) });
        turns.push(Turn::User("t2".into()));
        let second = anthropic_body(&m, &Prompt { l1: "L1", l2: "L2", turns: &turns, effort: Some("low") });

        assert_eq!(first["system"], second["system"]);
        assert_eq!(first["messages"][0], second["messages"][0]);
        // Thinking blocks are replayed unchanged, never stripped.
        assert_eq!(second["messages"][1]["content"], raw);
        assert_eq!(second["fallbacks"], "default");
        assert_eq!(second["output_config"]["effort"], "low");
        assert_eq!(second["system"][1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn openai_gets_text_only_history() {
        let m = model(None);
        let turns = vec![
            Turn::User("hi".into()),
            Turn::Assistant { text: "嗯".into(), raw: Some(json!([{"type": "text", "text": "嗯"}])) },
        ];
        let body = openai_body(&m, &Prompt { l1: "L1", l2: "L2", turns: &turns, effort: None });
        assert_eq!(body["messages"][0]["content"], "L1\n\nL2");
        assert_eq!(body["messages"][2]["content"], "嗯");
    }

    #[test]
    fn parses_usage_and_cost() {
        let v = json!({
            "content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": "好"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 100, "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 0, "output_tokens": 10}
        });
        let (text, _, usage) = parse_anthropic(&v).unwrap();
        assert_eq!(text, "好");
        assert_eq!(usage.cached_input, 1000);
        let cost = cost_usd(&model(None), &usage);
        assert!((cost - (100.0 * 4.0 + 1000.0 * 0.2 + 10.0 * 20.0) / 1e6).abs() < 1e-12);

        let ds = json!({
            "choices": [{"message": {"content": "ok"}}],
            "usage": {"prompt_tokens": 500, "prompt_cache_hit_tokens": 400, "completion_tokens": 7}
        });
        let (_, u) = parse_openai(&ds).unwrap();
        assert_eq!((u.uncached_input, u.cached_input, u.output), (100, 400, 7));
    }

    #[test]
    fn refusal_is_an_error() {
        let v = json!({"content": [], "stop_reason": "refusal", "stop_details": {"category": "cyber"}});
        assert!(parse_anthropic(&v).is_err());
    }
}
