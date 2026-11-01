//! One entry point for every model call: route by scenario, call, log to `model_calls`, charge
//! the budget, and on failure escalate once to the next stronger model on the Pareto front
//! (docs/DESIGN.md 5d). JSON answers get a self-check: unparsable output also escalates.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Local, NaiveTime, Utc};
use serde::de::DeserializeOwned;
use tracing::warn;

use super::llm::{LlmClient, Prompt, Reply, Turn, Usage, cost_usd};
use super::router::{Budget, Router};
use crate::config::{Config, ModelEntry};
use crate::ns::ProcessingPolicy;
use crate::store::{ModelCallRecord, Store};

pub struct Models {
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
        Ok(Models {
            router, llm, store, budget: Mutex::new(budget), has_local,
            #[cfg(test)]
            stub: None,
        })
    }

    #[cfg(test)]
    pub fn stub(store: Arc<dyn Store>, replies: Vec<String>) -> Self {
        Self {
            router: Router::new(vec![], Default::default()),
            llm: LlmClient::new(Default::default()).unwrap(), store,
            budget: Mutex::new(Budget::new(1.0, 0.0)), has_local: false,
            stub: Some(Mutex::new(Stub { replies: replies.into(), prompts: vec![] })),
        }
    }

    #[cfg(test)]
    pub fn stub_prompts(&self) -> Vec<(String, ProcessingPolicy, String)> {
        self.stub.as_ref().unwrap().lock().unwrap().prompts.clone()
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
        let outcome = self.llm.complete(model, prompt).await;
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
        let model = self.router.model(model_id).ok_or_else(|| anyhow!("model `{model_id}` is not available"))?.clone();
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
        #[cfg(test)]
        if let Some(stub) = &self.stub {
            let mut stub = stub.lock().unwrap();
            stub.prompts.push((scenario.into(), policy.clone(), user.into()));
            return extract_json(&stub.replies.pop_front().expect("unexpected model call"));
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
