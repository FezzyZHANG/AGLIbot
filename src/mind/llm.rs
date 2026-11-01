//! Model clients over raw HTTP: Anthropic Messages API and OpenAI-compatible chat completions
//! (Bailian, DeepSeek, OpenAI, local servers). Request bodies keep L1/L2 as a stable prefix so prompt
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
    let mut body = json!({"model": m.model, "messages": messages, "stream": m.stream});
    if m.stream {
        body["stream_options"] = json!({"include_usage": true});
    }
    // A low-effort scenario (chat, triage) turns thinking off even on models configured with it:
    // the same model serves research too, so thinking follows the scenario, not the model.
    if let Some(thinking) = m.enable_thinking {
        body["enable_thinking"] = json!(thinking && p.effort != Some("low"));
    }
    body
}

/// Collect OpenAI SSE output into the same reply shape as a non-streaming request.
/// Thinking deltas stay private; the final usage-only event supplies token accounting.
fn parse_openai_stream(body: &str) -> Result<(String, Usage)> {
    let mut text = String::new();
    let mut usage = Value::Null;
    let mut done = false;
    let normalized = body.replace("\r\n", "\n");
    for event in normalized.split("\n\n") {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:").map(|s| s.strip_prefix(' ').unwrap_or(s)))
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        if data.trim() == "[DONE]" {
            done = true;
            break;
        }
        let chunk: Value = serde_json::from_str(&data).context("invalid chat completion stream event")?;
        if let Some(error) = chunk.get("error") {
            bail!("chat completion stream error: {error}");
        }
        if let Some(delta) = chunk.pointer("/choices/0/delta/content").and_then(Value::as_str) {
            text.push_str(delta);
        }
        if chunk.get("usage").is_some_and(Value::is_object) {
            usage = chunk["usage"].clone();
        }
    }
    if !done {
        bail!("chat completion stream ended before [DONE]");
    }
    if text.is_empty() {
        bail!("chat completion stream returned no text");
    }
    parse_openai(&json!({"choices": [{"message": {"content": text}}], "usage": usage}))
}

fn provider_base_url(provider: &ProviderCfg) -> Option<String> {
    provider
        .base_url_env
        .as_deref()
        .and_then(|var| std::env::var(var).ok())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| provider.base_url.clone())
        .map(|value| value.trim().trim_end_matches('/').to_string())
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
    pub async fn embed(&self, cfg: &super::embedding::EmbeddingCfg, text: &str) -> Result<(Vec<f32>, Usage)> {
        let provider = self.providers.get(&cfg.provider).ok_or_else(|| anyhow!("unknown embedding provider"))?;
        anyhow::ensure!(matches!(provider.kind, ProviderKind::OpenaiCompat), "embedding provider must be openai_compat");
        let base = provider_base_url(provider).ok_or_else(|| anyhow!("embedding provider needs base_url"))?;
        let mut request = self.http.post(format!("{base}/embeddings"))
            .timeout(Duration::from_secs(10))
            .json(&json!({"model": cfg.model, "input": text, "encoding_format": "float"}));
        if let Some(key) = Self::api_key(provider)? { request = request.bearer_auth(key); }
        // Do not log provider response bodies: they may echo the private input.
        let value: Value = request.send().await?.error_for_status()?.json().await?;
        parse_embedding(&value)
    }

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

    /// For keyless (local) providers: is the server actually up? Checked once at startup.
    pub async fn reachable(&self, m: &ModelEntry) -> bool {
        let Some(base) = self.providers.get(&m.provider).and_then(provider_base_url) else { return false };
        let probe = self.http.get(format!("{base}/models")).timeout(Duration::from_secs(2)).send().await;
        probe.is_ok_and(|r| r.status().is_success())
    }

    pub async fn complete(&self, m: &ModelEntry, p: &Prompt<'_>) -> Result<Reply> {
        let provider = self.providers.get(&m.provider).ok_or_else(|| anyhow!("unknown provider {}", m.provider))?;
        let key = Self::api_key(provider)?;
        let started = Instant::now();
        let reply = match provider.kind {
            ProviderKind::Anthropic => {
                let base = provider_base_url(provider).unwrap_or_else(|| "https://api.anthropic.com".into());
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
                let base = provider_base_url(provider).ok_or_else(|| anyhow!("provider {} needs base_url", m.provider))?;
                let mut req = self
                    .http
                    .post(format!("{}/chat/completions", base.trim_end_matches('/')))
                    .json(&openai_body(m, p));
                if let Some(key) = &key {
                    req = req.bearer_auth(key);
                }
                let body = send_text(req).await?;
                let (text, usage) = if m.stream {
                    parse_openai_stream(&body)?
                } else {
                    parse_openai(&serde_json::from_str(&body).context("response is not JSON")?)?
                };
                Reply { text, raw: None, usage, latency: started.elapsed() }
            }
        };
        Ok(reply)
    }
}

async fn send_json(req: reqwest::RequestBuilder) -> Result<Value> {
    serde_json::from_str(&send_text(req).await?).context("response is not JSON")
}

pub(super) fn parse_embedding(value: &Value) -> Result<(Vec<f32>, Usage)> {
    let data = value["data"].as_array().ok_or_else(|| anyhow!("embedding response has no data"))?;
    anyhow::ensure!(data.len() == 1 && data[0]["index"].as_u64() == Some(0), "embedding response must contain exactly index 0");
    let vector: Vec<f32> = serde_json::from_value(data[0]["embedding"].clone()).context("invalid embedding array")?;
    anyhow::ensure!(super::embedding::valid_vector(&vector), "embedding must be finite and nonzero");
    let usage = Usage { uncached_input: u64_at(value, "/usage/prompt_tokens"), ..Usage::default() };
    Ok((vector, usage))
}

async fn send_text(req: reqwest::RequestBuilder) -> Result<String> {
    let resp = req.send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        let snippet: String = body.chars().take(500).collect();
        bail!("HTTP {status}: {snippet}");
    }
    Ok(body)
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
            stream: false,
            enable_thinking: None,
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

    #[test]
    fn streaming_thinking_request_keeps_provider_options_separate() {
        let mut m = model(None);
        let prompt = Prompt { l1: "system", l2: "", turns: &[], effort: Some("low") };
        let plain = openai_body(&m, &prompt);
        assert_eq!(plain["stream"], false);
        assert!(plain.get("enable_thinking").is_none());
        assert!(plain.get("stream_options").is_none());
        m.stream = true;
        m.enable_thinking = Some(true);
        let body = openai_body(&m, &prompt);
        assert_eq!(body["stream"], true);
        assert_eq!(body["enable_thinking"], false, "low effort turns thinking off");
        let deep = openai_body(&m, &Prompt { effort: None, ..prompt });
        assert_eq!(deep["enable_thinking"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn streaming_reply_collects_text_and_final_usage_without_reasoning() {
        let body = concat!(
            ": keepalive\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"private\",\"content\":null}}]}\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"！\"},\"finish_reason\":\"stop\"}]}\r\n\r\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":500,\"completion_tokens\":30,\"prompt_tokens_details\":{\"cached_tokens\":400}}}\r\n\r\n",
            "data: [DONE]\r\n\r\n",
        );
        let (text, usage) = parse_openai_stream(body).unwrap();
        assert_eq!(text, "你好！");
        assert_eq!(usage, Usage { uncached_input: 100, cached_input: 400, output: 30, cache_write: 0 });
    }

    #[test]
    fn streaming_errors_and_incomplete_replies_are_rejected() {
        for body in [
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
            "data: {\"error\":{\"message\":\"quota exceeded\"}}\n\ndata: [DONE]\n\n",
            "data: not json\n\ndata: [DONE]\n\n",
            "data: [DONE]\n\n",
        ] {
            assert!(parse_openai_stream(body).is_err());
        }
    }

    #[test]
    fn provider_endpoint_uses_environment_override_then_fallback() {
        let mut provider: ProviderCfg = serde_yaml::from_str(
            "kind: openai_compat\nbase_url: https://fallback.example/v1/\nbase_url_env: AGLIBOT_UNSET_TEST_BASE_URL",
        ).unwrap();
        assert_eq!(provider_base_url(&provider).as_deref(), Some("https://fallback.example/v1"));
        // Use an existing variable without mutating the process environment in parallel tests.
        let value = std::env::var("PATH").unwrap();
        provider.base_url_env = Some("PATH".into());
        assert_eq!(provider_base_url(&provider).unwrap(), value.trim().trim_end_matches('/'));
    }

    /// Explicit opt-in only: sends a tiny prompt to each configured Bailian model.
    #[tokio::test]
    #[ignore = "requires BAILIAN_API_KEY and network; consumes Token Plan quota"]
    async fn live_bailian_models() -> Result<()> {
        let cfg = crate::config::Config::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))?;
        let client = LlmClient::new(cfg.models.providers.clone())?;
        let turns = [Turn::User("Reply with exactly OK.".into())];
        let prompt = Prompt { l1: "This is a connection test. Keep the answer minimal.", l2: "", turns: &turns, effort: None };
        for m in cfg.models.models.iter().filter(|m| m.provider == "bailian") {
            let reply = client.complete(m, &prompt).await.with_context(|| format!("testing {}", m.model))?;
            assert!(!reply.text.trim().is_empty(), "{} returned no text", m.model);
            assert!(reply.usage.output > 0, "{} returned no usage", m.model);
            println!("{}: reply received, output_tokens={}", m.model, reply.usage.output);
        }
        Ok(())
    }
}
