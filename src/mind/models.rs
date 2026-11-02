//! One entry point for every model call: route by scenario, call, log to `model_calls`, charge
//! the budget, and on failure escalate once to the next stronger model on the Pareto front
//! (docs/DESIGN.md 5d). JSON answers get a self-check: unparsable output also escalates.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Local, NaiveTime, Utc};
use serde::de::DeserializeOwned;
use tracing::{info, warn};

use super::llm::{LlmClient, Prompt, Reply, Turn, Usage, cost_usd};
use super::router::{Budget, Router};
use crate::config::{Config, ModelEntry};
use crate::ns::ProcessingPolicy;
use crate::store::{ModelCallRecord, Store};

pub struct Models {
    pub focus_cache: super::flow_metrics::FocusCache,
    embedding: Option<super::embedding::EmbeddingCfg>,
    embedding_warned: AtomicBool,
    router: Router,
    llm: LlmClient,
    store: Arc<dyn Store>,
    budget: Mutex<Budget>,
    has_local: bool,
    #[cfg(test)]
    stub: Option<Mutex<Stub>>,
}

#[cfg(test)]
struct Stub {
    replies: std::collections::VecDeque<String>,
    prompts: Vec<(String, ProcessingPolicy, String)>,
    cost_usd: f64,
}

/// Admission limit from recorded spend. An in-flight call is charged when usage arrives.
#[derive(Clone, Copy)]
pub struct SpendLimit {
    pub since: DateTime<Utc>,
    pub max_usd: f64,
}

impl SpendLimit {
    pub fn exhausted(&self, store: &dyn Store) -> Result<bool> {
        Ok(!self.max_usd.is_finite() || self.max_usd <= 0.0 || store.spent_since(self.since)? >= self.max_usd)
    }
}

pub fn local_midnight_utc() -> DateTime<Utc> {
    Local::now()
        .date_naive()
        .and_time(NaiveTime::MIN)
        .and_local_timezone(Local)
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

/// Pull the first JSON object out of a model reply (tolerates prose and code fences around it).
pub fn extract_json<T: DeserializeOwned>(text: &str) -> Result<T> {
    let start = text.find('{').ok_or_else(|| anyhow!("no JSON object in reply"))?;
    let end = text.rfind('}').ok_or_else(|| anyhow!("no JSON object in reply"))?;
    if end < start {
        return Err(anyhow!("no JSON object in reply"));
    }
    serde_json::from_str(&text[start..=end]).context("reply JSON does not match the expected shape")
}

impl Models {
    /// Models without credentials, and local models whose server is down, are left out of routing.
    pub async fn new(cfg: &Config, store: Arc<dyn Store>) -> Result<Self> {
        let llm = LlmClient::new(cfg.models.providers.clone())?;
        let mut usable = Vec::new();
        for m in &cfg.models.models {
            if !llm.has_credentials(m) {
                warn!("model `{}` disabled: provider `{}` has no API key", m.id, m.provider);
            } else if m.local && !llm.reachable(m).await {
                warn!("model `{}` disabled: local server for `{}` is not reachable", m.id, m.provider);
            } else {
                usable.push(m.clone());
            }
        }
        let has_local = usable.iter().any(|m| m.local);
        let router = Router::new(usable, cfg.scenarios.clone());
        router
            .select("chat", &ProcessingPolicy::Any, false)
            .map_err(|e| anyhow!("没有可用的聊天模型，请在 .env 里配置至少一个提供商的密钥：{e}"))?;
        let budget = Budget::new(cfg.models.daily_budget_usd, store.spent_since(local_midnight_utc())?);
        let embedding = cfg.models.embedding.clone().filter(|embedding| {
            if let Err(e) = embedding.validate() {
                warn!("semantic recall disabled: {e}");
                false
            } else { true }
        });
        info!(enabled = embedding.is_some(), model = ?embedding.as_ref().map(|e| e.model_id()), "semantic recall startup");
        Ok(Models {
            focus_cache: super::flow_metrics::FocusCache::new(cfg)?,
            embedding, embedding_warned: AtomicBool::new(false),
            router, llm, store, budget: Mutex::new(budget), has_local,
            #[cfg(test)]
            stub: None,
        })
    }

    #[cfg(test)]
    pub fn stub(store: Arc<dyn Store>, replies: Vec<String>) -> Self {
        Self {
            focus_cache: Default::default(),
            embedding: None, embedding_warned: AtomicBool::new(false),
            router: Router::new(vec![], Default::default()),
            llm: LlmClient::new(Default::default()).unwrap(), store,
            budget: Mutex::new(Budget::new(1.0, 0.0)), has_local: false,
            stub: Some(Mutex::new(Stub { replies: replies.into(), prompts: vec![], cost_usd: 0.0 })),
        }
    }

    #[cfg(test)]
    pub fn stub_prompts(&self) -> Vec<(String, ProcessingPolicy, String)> {
        self.stub.as_ref().unwrap().lock().unwrap().prompts.clone()
    }

    #[cfg(test)]
    pub fn stub_cost(&self, cost: f64) {
        self.stub.as_ref().unwrap().lock().unwrap().cost_usd = cost;
    }

    #[cfg(test)]
    pub fn with_embedding(mut self, cfg: super::embedding::EmbeddingCfg) -> Self {
        self.embedding = Some(cfg);
        self
    }

    pub fn embedding(&self) -> Option<&super::embedding::EmbeddingCfg> { self.embedding.as_ref() }

    pub fn warn_embedding(&self, error: &anyhow::Error) {
        if !self.embedding_warned.swap(true, Ordering::Relaxed) {
            warn!("semantic recall unavailable; retaining ordinary recall: {error:#}");
        }
    }

    /// Check policy before passing any input to the client (including the test client).
    /// No escalation: another provider may not be allowed to process this text.
    pub async fn embed(&self, policy: &ProcessingPolicy, text: &str) -> Result<Option<Vec<f32>>> {
        let Some(cfg) = &self.embedding else { return Ok(None) };
        if !cfg.allows(policy) || self.budget_exhausted() || text.trim().is_empty() { return Ok(None) }
        let started = Instant::now();
        let outcome = self.embedding_call(cfg, policy, text).await;
        let usage = outcome.as_ref().map(|(_, usage)| usage.clone()).unwrap_or_default();
        let cost = usage.uncached_input as f64 * cfg.input_usd_per_million / 1_000_000.0;
        self.budget().add(cost);
        let record = ModelCallRecord {
            ts: Utc::now(), scenario: "embed".into(), model_id: cfg.model_id(), session_id: "embed".into(),
            uncached_input: usage.uncached_input, cached_input: 0, cache_write: 0, output: 0,
            latency_ms: started.elapsed().as_millis() as u64, cost_usd: cost,
            ok: outcome.is_ok(), error: outcome.as_ref().err().map(|_| "embedding request or response failed".into()),
            escalated_from: None,
        };
        if let Err(e) = self.store.append_model_call(&record) { self.warn_embedding(&e); }
        outcome.map(|(vector, _)| Some(vector))
    }

    async fn embedding_call(&self, cfg: &super::embedding::EmbeddingCfg, _policy: &ProcessingPolicy, text: &str) -> Result<(Vec<f32>, Usage)> {
        #[cfg(test)]
        if let Some(stub) = &self.stub {
            let mut stub = stub.lock().unwrap();
            stub.prompts.push(("embed".into(), _policy.clone(), text.into()));
            let reply = stub.replies.pop_front().ok_or_else(|| anyhow!("stub embedder unavailable"))?;
            return super::llm::parse_embedding(&serde_json::from_str(&reply)?);
        }
        self.llm.embed(cfg, text).await
    }

    #[cfg(test)]
    fn stub_reply(&self, scenario: &str, policy: &ProcessingPolicy, user: &str) -> Result<String> {
        let mut stub = self.stub.as_ref().unwrap().lock().unwrap();
        stub.prompts.push((scenario.into(), policy.clone(), user.into()));
        self.store.append_model_call(&ModelCallRecord {
            ts: Utc::now(), scenario: scenario.into(), model_id: "stub".into(), session_id: scenario.into(),
            uncached_input: 0, cached_input: 0, cache_write: 0, output: 0, latency_ms: 0,
            cost_usd: stub.cost_usd, ok: true, error: None, escalated_from: None,
        })?;
        Ok(stub.replies.pop_front().expect("unexpected model call"))
    }

    pub fn router(&self) -> &Router {
        &self.router
    }

    /// Whether `local_only` material (self-scoped lessons) can be processed at all.
    pub fn has_local(&self) -> bool {
        self.has_local
    }

    fn budget(&self) -> std::sync::MutexGuard<'_, Budget> {
        self.budget.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn budget_exhausted(&self) -> bool {
        self.budget().exhausted()
    }

    pub fn spent_today(&self) -> (f64, f64) {
        let b = self.budget();
        (b.spent_usd, b.limit_usd)
    }

    pub fn pick(&self, scenario: &str, policy: &ProcessingPolicy) -> Result<ModelEntry> {
        Ok(self.router.select(scenario, policy, self.budget_exhausted())?.clone())
    }

    pub fn effort(&self, scenario: &str) -> Option<String> {
        self.router.scenario(scenario).ok().and_then(|s| s.effort.clone())
    }

    async fn call_once(
        &self,
        scenario: &str,
        model: &ModelEntry,
        prompt: &Prompt<'_>,
        session_id: &str,
        escalated_from: Option<&str>,
    ) -> Result<Reply> {
        self.focus_cache.resumed(model, session_id, Utc::now());
        let outcome = self.llm.complete(model, prompt).await;
        self.focus_cache.completed(model, session_id, Utc::now());
        let (usage, latency, error) = match &outcome {
            Ok(r) if !r.text.trim().is_empty() => (r.usage.clone(), r.latency, None),
            Ok(r) => (r.usage.clone(), r.latency, Some("empty reply".to_string())),
            Err(e) => (Usage::default(), Duration::ZERO, Some(format!("{e:#}"))),
        };
        let cost = cost_usd(model, &usage);
        self.budget().add(cost);
        let call = ModelCallRecord {
            ts: Utc::now(),
            scenario: scenario.into(),
            model_id: model.id.clone(),
            session_id: session_id.into(),
            uncached_input: usage.uncached_input,
            cached_input: usage.cached_input,
            cache_write: usage.cache_write,
            output: usage.output,
            latency_ms: latency.as_millis() as u64,
            cost_usd: cost,
            ok: error.is_none(),
            error: error.clone(),
            escalated_from: escalated_from.map(String::from),
        };
        if let Err(e) = self.store.append_model_call(&call) {
            warn!("failed to log model call: {e:#}");
        }
        match error {
            None => outcome,
            Some(e) => Err(anyhow!("`{}` failed: {e}", model.id)),
        }
    }

    /// Call `model_id` (pinned by a session) and escalate once on failure.
    /// Returns the reply and the model that produced it, so sessions can re-pin.
    pub async fn run(
        &self,
        scenario: &str,
        policy: &ProcessingPolicy,
        model_id: &str,
        prompt: &Prompt<'_>,
        session_id: &str,
    ) -> Result<(Reply, String)> {
        let model = self.router.select_for_call(scenario, policy, model_id, self.budget_exhausted())?.clone();
        match self.call_once(scenario, &model, prompt, session_id, None).await {
            Ok(r) => Ok((r, model.id)),
            Err(first) => {
                let Some(next) = self.router.next_stronger(scenario, policy, &model.id).cloned() else {
                    return Err(first);
                };
                warn!("{first:#}; escalating to `{}`", next.id);
                let r = self.call_once(scenario, &next, prompt, session_id, Some(&model.id)).await?;
                Ok((r, next.id))
            }
        }
    }

    /// A cheap side call with no retry/escalation; the caller owns the fallback.
    pub async fn ask_once(&self, scenario: &str, policy: &ProcessingPolicy, system: &str, user: &str) -> Result<String> {
        #[cfg(test)]
        if self.stub.is_some() { return self.stub_reply(scenario, policy, user) }
        let model = self.pick(scenario, policy)?;
        let turns = [Turn::User(user.to_string())];
        let effort = self.effort(scenario);
        let prompt = Prompt { l1: system, l2: "", turns: &turns, effort: effort.as_deref() };
        Ok(self.call_once(scenario, &model, &prompt, scenario, None).await?.text)
    }

    /// One-shot call: `system` is the stable prefix (usually L1), `user` the task.
    pub async fn ask(&self, scenario: &str, policy: &ProcessingPolicy, system: &str, user: &str) -> Result<String> {
        let model = self.pick(scenario, policy)?;
        let turns = [Turn::User(user.to_string())];
        let effort = self.effort(scenario);
        let prompt = Prompt { l1: system, l2: "", turns: &turns, effort: effort.as_deref() };
        let (reply, _) = self.run(scenario, policy, &model.id, &prompt, scenario).await?;
        Ok(reply.text)
    }

    /// Like `ask`, but the reply must contain a JSON object of shape `T`. A reply that fails the
    /// check is redone once by the next stronger model.
    pub async fn ask_json<T: DeserializeOwned>(
        &self,
        scenario: &str,
        policy: &ProcessingPolicy,
        system: &str,
        user: &str,
    ) -> Result<T> {
        self.ask_json_with_limit(scenario, policy, system, user, None).await
    }

    /// Limited background calls never retry or escalate outside the caller's spend check.
    pub async fn ask_json_with_limit<T: DeserializeOwned>(
        &self,
        scenario: &str,
        policy: &ProcessingPolicy,
        system: &str,
        user: &str,
        limit: Option<SpendLimit>,
    ) -> Result<T> {
        if let Some(limit) = limit {
            anyhow::ensure!(!limit.exhausted(self.store.as_ref())?, "read-only reflection cost budget exhausted");
        }
        #[cfg(test)]
        if self.stub.is_some() {
            return extract_json(&self.stub_reply(scenario, policy, user)?);
        }
        if limit.is_some() {
            return extract_json(&self.ask_once(scenario, policy, system, user).await?);
        }
        let model = self.pick(scenario, policy)?;
        let turns = [Turn::User(user.to_string())];
        let effort = self.effort(scenario);
        let prompt = Prompt { l1: system, l2: "", turns: &turns, effort: effort.as_deref() };
        let (reply, used) = self.run(scenario, policy, &model.id, &prompt, scenario).await?;
        match extract_json(&reply.text) {
            Ok(v) => Ok(v),
            Err(e) => {
                let next = self
                    .router
                    .next_stronger(scenario, policy, &used)
                    .cloned()
                    .ok_or_else(|| e.context(format!("`{used}` returned unusable JSON and nothing stronger is available")))?;
                warn!("`{used}` returned unusable JSON; redoing with `{}`", next.id);
                let reply = self.call_once(scenario, &next, &prompt, scenario, Some(&used)).await?;
                extract_json(&reply.text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize, Debug, PartialEq)]
    struct D {
        action: String,
    }

    #[test]
    fn json_is_found_inside_prose_and_fences() {
        let d: D = extract_json("好的，结果如下：\n```json\n{\"action\": \"nothing\"}\n```").unwrap();
        assert_eq!(d, D { action: "nothing".into() });
        assert!(extract_json::<D>("没有 JSON").is_err());
        assert!(extract_json::<D>("{\"other\": 1}").is_err());
    }
}
