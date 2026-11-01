//! Conversation sessions, one per chat (private or group).
//!
//! A session pins one model and one frozen L2, and only ever appends turns, so the provider's
//! prompt cache keeps hitting. A new session starts after idling, after too many turns, when the
//! processing policy or open task cards change, or when the budget forces a cheaper model; its L2
//! is rebuilt with a recap and privacy-filtered memories.

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
    task_cards: HashMap<String, String>,
}

impl Speaker {
    pub fn new(l1: String, limits: SessionCfg) -> Self {
        Speaker { l1, limits, sessions: HashMap::new(), task_cards: HashMap::new() }
    }

    pub fn l1(&self) -> &str {
        &self.l1
    }

    /// End every session so the next turn rebuilds L2 with fresh memories (after nightly reflection).
    pub fn reset_all(&mut self) {
        self.sessions.clear();
    }

    /// Rebuild a chat's frozen L2 only when its open task cards change.
    pub fn refresh_tasks(&mut self, key: &str, cards: &str) {
        if self.task_cards.get(key).map(String::as_str).unwrap_or_default() != cards {
            self.sessions.remove(key);
            self.task_cards.insert(key.into(), cards.into());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            id: "test".into(), model_id: "test".into(), policy: ProcessingPolicy::Any,
            l2: "frozen frame".into(), turns: vec![Turn::User("previous turn".into())], last_active: Utc::now(),
        }
    }

    #[test]
    fn changed_cards_refresh_only_the_affected_session() {
        let mut s = Speaker::new(String::new(), SessionCfg {
            max_turns: 20, idle_minutes: 30, recap_messages: 10, debounce: Default::default(),
        });
        s.sessions.insert("owner".into(), session());
        s.sessions.insert("group".into(), session());
        s.refresh_tasks("group", "");
        s.refresh_tasks("owner", "- task [Inbox] 研究 | 任务创建");
        assert!(!s.sessions.contains_key("owner"));
        s.sessions.insert("owner".into(), session());
        s.refresh_tasks("owner", "- task [Inbox] 研究 | 任务创建");
        assert_eq!(s.sessions["owner"].l2, "frozen frame");
        assert_eq!(s.sessions["owner"].turns.len(), 1);
        s.refresh_tasks("owner", "- task [Active] 研究 | 状态 → Active");
        assert!(!s.sessions.contains_key("owner"));
        s.sessions.insert("owner".into(), session());
        s.refresh_tasks("owner", "");
        assert!(!s.sessions.contains_key("owner"));
        assert_eq!(s.sessions["group"].turns.len(), 1);
    }
}
