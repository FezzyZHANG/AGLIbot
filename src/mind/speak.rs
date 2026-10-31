//! Conversation sessions and replies.
//!
//! A session pins one model and one frozen L2, and only ever appends turns, so the provider's
//! prompt cache keeps hitting. A new session starts after idling, after too many turns, or when
//! the budget forces a cheaper model; its L2 carries a text recap of the recent chat.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use chrono::{DateTime, Local, NaiveTime, Utc};
use tracing::{info, warn};

use super::context::{RecapLine, build_l1, build_l2, load_state, read_norm, tail, user_turn};
use super::llm::{LlmClient, Prompt, Turn, Usage, cost_usd};
use super::router::{Budget, Router};
use crate::channels::Envelope;
use crate::config::Config;
use crate::ns::{Audience, Namespace, ProcessingPolicy, Scope};
use crate::store::{MessageRecord, ModelCallRecord, Store};

const SCENARIO: &str = "chat";
const OWNER_LABEL: &str = "主人";

struct Session {
    id: String,
    model_id: String,
    l2: String,
    turns: Vec<Turn>,
    last_active: DateTime<Utc>,
}

pub struct Mind {
    cfg: Arc<Config>,
    router: Router,
    llm: LlmClient,
    store: Arc<dyn Store>,
    budget: Budget,
    l1: String,
    sessions: HashMap<String, Session>,
}

fn local_midnight_utc() -> DateTime<Utc> {
    Local::now()
        .date_naive()
        .and_time(NaiveTime::MIN)
        .and_local_timezone(Local)
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

impl Mind {
    pub fn new(cfg: Arc<Config>, store: Arc<dyn Store>) -> Result<Self> {
        let llm = LlmClient::new(cfg.models.providers.clone())?;
        let usable = cfg
            .models
            .models
            .iter()
            .filter(|m| {
                let ok = llm.has_credentials(m);
                if !ok {
                    warn!("model `{}` disabled: provider `{}` has no API key", m.id, m.provider);
                }
                ok
            })
            .cloned()
            .collect();
        let router = Router::new(usable, cfg.scenarios.clone());
        router
            .select(SCENARIO, &ProcessingPolicy::Any, false)
            .map_err(|e| anyhow!("没有可用的聊天模型，请在 .env 里配置至少一个提供商的密钥：{e}"))?;
        let budget = Budget::new(cfg.models.daily_budget_usd, store.spent_since(local_midnight_utc())?);
        let l1 = build_l1(&cfg.mind_dir())?;
        Ok(Mind { cfg, router, llm, store, budget, l1, sessions: HashMap::new() })
    }

    fn recap(&self, chat_id: &str, audience: &Audience) -> Result<Vec<RecapLine>> {
        let n = self.cfg.runtime.session.recap_messages;
        let mut recent = self.store.recent_messages(chat_id, n + 1)?;
        recent.pop(); // the message being answered now; it goes into L3, not the recap
        Ok(recent
            .into_iter()
            .filter(|m| m.scope.permits(audience))
            .map(|m| RecapLine {
                ts: m.ts.with_timezone(&Local),
                speaker: if m.outgoing { "我".into() } else { OWNER_LABEL.into() },
                text: m.text,
            })
            .collect())
    }

    pub async fn reply_owner(&mut self, env: &Envelope) -> Result<String> {
        let owner = self.cfg.acl.owner.person_id.clone();
        let ns = Namespace::Person(owner.clone());
        let scope = Scope::Persons { ids: vec![owner.clone()] };
        let policy = ProcessingPolicy::default_for(&scope);
        let now = Utc::now();
        let record = |outgoing: bool, sender: &str, text: &str| MessageRecord {
            ts: Utc::now(),
            platform: env.platform.into(),
            chat_id: env.chat_id.clone(),
            sender: sender.into(),
            outgoing,
            text: text.into(),
            ns: ns.clone(),
            scope: scope.clone(),
        };
        self.store.append_message(&record(false, &owner, &env.text))?;

        let exhausted = self.budget.exhausted();
        let desired = self.router.select(SCENARIO, &policy, exhausted)?.id.clone();
        let limits = &self.cfg.runtime.session;
        let rotate = match self.sessions.get(&env.chat_id) {
            None => true,
            Some(s) => {
                now - s.last_active > chrono::Duration::minutes(limits.idle_minutes)
                    || s.turns.len() >= 2 * limits.max_turns
                    || self.router.model(&s.model_id).is_none_or(|m| !policy.allows(m))
                    || (exhausted && s.model_id != desired)
            }
        };
        if rotate {
            let card = read_norm(&self.cfg.mind_dir().join("social/people/owner.md"))?;
            let l2 = build_l2(OWNER_LABEL, &card, &self.recap(&env.chat_id, &Audience::private(&owner))?);
            let id = format!("{}-{}", env.chat_id, now.timestamp_millis());
            info!("new session {id} on model `{desired}`");
            let session = Session { id, model_id: desired, l2, turns: Vec::new(), last_active: now };
            self.sessions.insert(env.chat_id.clone(), session);
        }

        let state = load_state(&self.cfg.mind_dir())?;
        let text = user_turn(&tail(now.with_timezone(&Local), &state), OWNER_LABEL, &env.text);
        let effort = self.router.scenario(SCENARIO)?.effort.clone();
        let session = self.sessions.get_mut(&env.chat_id).ok_or_else(|| anyhow!("session missing"))?;
        session.turns.push(Turn::User(text));
        session.last_active = now;

        let mut escalated_from: Option<String> = None;
        loop {
            let model = self.router.model(&session.model_id).ok_or_else(|| anyhow!("model vanished"))?.clone();
            let prompt = Prompt { l1: &self.l1, l2: &session.l2, turns: &session.turns, effort: effort.as_deref() };
            let outcome = self.llm.complete(&model, &prompt).await;
            let (usage, latency, error) = match &outcome {
                Ok(r) if !r.text.trim().is_empty() => (r.usage.clone(), r.latency, None),
                Ok(r) => (r.usage.clone(), r.latency, Some("empty reply".to_string())),
                Err(e) => (Usage::default(), Duration::ZERO, Some(format!("{e:#}"))),
            };
            let cost = cost_usd(&model, &usage);
            self.budget.add(cost);
            let call = ModelCallRecord {
                ts: Utc::now(),
                scenario: SCENARIO.into(),
                model_id: model.id.clone(),
                session_id: session.id.clone(),
                uncached_input: usage.uncached_input,
                cached_input: usage.cached_input,
                cache_write: usage.cache_write,
                output: usage.output,
                latency_ms: latency.as_millis() as u64,
                cost_usd: cost,
                ok: error.is_none(),
                error: error.clone(),
                escalated_from: escalated_from.clone(),
            };
            if let Err(e) = self.store.append_model_call(&call) {
                warn!("failed to log model call: {e:#}");
            }

            let Some(err) = error else {
                let reply = outcome?;
                session.turns.push(Turn::Assistant { text: reply.text.clone(), raw: reply.raw });
                self.store.append_message(&record(true, "me", &reply.text))?;
                return Ok(reply.text);
            };
            match self.router.next_stronger(SCENARIO, &policy, &model.id) {
                Some(next) if escalated_from.is_none() => {
                    warn!("`{}` failed ({err}); escalating to `{}`", model.id, next.id);
                    escalated_from = Some(model.id.clone());
                    session.model_id = next.id.clone();
                }
                _ => {
                    // Keep the transcript well-formed: the unanswered turn is dropped, not left dangling.
                    session.turns.pop();
                    return Err(anyhow!("chat failed on `{}`: {err}", model.id));
                }
            }
        }
    }
}
