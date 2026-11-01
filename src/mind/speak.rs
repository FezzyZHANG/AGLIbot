//! Conversation sessions, one per chat (private or group).
//!
//! A session pins one model and one frozen L2, and only ever appends turns, so the provider's
//! prompt cache keeps hitting. A new session starts after idling, after too many turns, when the
//! processing policy changes, or when the budget forces a cheaper model; its L2 is rebuilt with a
//! recap and privacy-filtered memories.

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use tracing::info;

use super::llm::{Prompt, Turn};
use super::models::Models;
use crate::config::SessionCfg;
use crate::ns::ProcessingPolicy;

const SCENARIO: &str = "chat";

struct Session {
    id: String,
    model_id: String,
    policy: ProcessingPolicy,
    l2: String,
    turns: Vec<Turn>,
    last_active: DateTime<Utc>,
}

pub struct Speaker {
    l1: String,
    limits: SessionCfg,
    sessions: HashMap<String, Session>,
}

impl Speaker {
    pub fn new(l1: String, limits: SessionCfg) -> Self {
        Speaker { l1, limits, sessions: HashMap::new() }
    }

    pub fn l1(&self) -> &str {
        &self.l1
    }

    /// End every session so the next turn rebuilds L2 with fresh memories (after nightly reflection).
    pub fn reset_all(&mut self) {
        self.sessions.clear();
    }

    /// Append `user_text` to the chat's session and get his reply. `frame` builds L2 and is only
    /// called when a new session starts.
    pub async fn say(
        &mut self,
        models: &Models,
        key: &str,
        policy: &ProcessingPolicy,
        now: DateTime<Utc>,
        user_text: String,
        frame: impl FnOnce() -> Result<String>,
    ) -> Result<String> {
        let exhausted = models.budget_exhausted();
        let desired = models.pick(SCENARIO, policy)?.id;
        let limits = &self.limits;
        let rotate = match self.sessions.get(key) {
            None => true,
            Some(s) => {
                now - s.last_active > chrono::Duration::minutes(limits.idle_minutes)
                    || s.turns.len() >= 2 * limits.max_turns
                    || &s.policy != policy
                    || models.router().model(&s.model_id).is_none()
                    || (exhausted && s.model_id != desired)
            }
        };
        if rotate {
            let l2 = frame()?;
            let id = format!("{key}-{}", now.timestamp_millis());
            info!("new session {id} on model `{desired}`");
            let session =
                Session { id, model_id: desired, policy: policy.clone(), l2, turns: Vec::new(), last_active: now };
            self.sessions.insert(key.to_string(), session);
        }
        let session = self.sessions.get_mut(key).ok_or_else(|| anyhow!("session missing"))?;
        session.turns.push(Turn::User(user_text));
        session.last_active = now;

        let effort = models.effort(SCENARIO);
        let prompt = Prompt { l1: &self.l1, l2: &session.l2, turns: &session.turns, effort: effort.as_deref() };
        match models.run(SCENARIO, policy, &session.model_id, &prompt, &session.id).await {
            Ok((reply, used)) => {
                session.model_id = used;
                session.turns.push(Turn::Assistant { text: reply.text.clone(), raw: reply.raw });
                Ok(reply.text)
            }
            Err(e) => {
                // Keep the transcript well-formed: the unanswered turn is dropped, not left dangling.
                session.turns.pop();
                Err(e)
            }
        }
    }
}
