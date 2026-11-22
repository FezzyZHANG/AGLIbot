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

use super::llm::{Part, Prompt, Turn};
use super::models::Models;
use crate::config::SessionCfg;
use crate::ns::ProcessingPolicy;

const SCENARIO: &str = "chat";
/// Past this a session rotates rather than replaying more images on every turn.
const MAX_SESSION_IMAGES: usize = 16;

fn image_count(parts: &[Part]) -> usize {
    parts.iter().filter(|part| matches!(part, Part::Jpeg(_))).count()
}

struct Session {
    id: String,
    model_id: String,
    policy: ProcessingPolicy,
    l2: String,
    turns: Vec<Turn>,
    last_active: DateTime<Utc>,
}

impl Session {
    fn images(&self) -> usize {
        self.turns.iter().map(|turn| match turn {
            Turn::UserParts(parts) => image_count(parts),
            _ => 0,
        }).sum()
    }

    /// Keep only the first `lasting` parts of the turn just answered.
    fn forget_parts(&mut self, lasting: usize) {
        let Some(turn) = self.turns.last_mut() else { return };
        let Turn::UserParts(parts) = turn else { return };
        parts.truncate(lasting);
        if let [Part::Text(text)] = parts.as_mut_slice() {
            *turn = Turn::User(std::mem::take(text));
        }
    }
}

pub struct SayTurn {
    pub now: DateTime<Utc>,
    pub text: String,
    pub images: Vec<Part>,
    pub ephemeral: Vec<Part>,
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

    /// Append the user turn and reply to the session. Ephemeral parts are only sent on this call.
    /// `frame` builds L2 when a new session starts.
    pub async fn say(
        &mut self,
        models: &Models,
        key: &str,
        policy: &ProcessingPolicy,
        turn: SayTurn,
        frame: impl FnOnce() -> Result<String>,
    ) -> Result<String> {
        let SayTurn { now, text, images, ephemeral } = turn;
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
                    || s.images() + image_count(&images) > MAX_SESSION_IMAGES
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
        let lasting = 1 + images.len();
        session.turns.push(if images.is_empty() && ephemeral.is_empty() {
            Turn::User(text)
        } else {
            let mut parts = vec![Part::Text(text)];
            parts.extend(images);
            parts.extend(ephemeral);
            Turn::UserParts(parts)
        });
        session.last_active = now;

        let effort = models.effort(SCENARIO);
        let prompt = Prompt { l1: &self.l1, l2: &session.l2, turns: &session.turns, effort: effort.as_deref(), schema: None };
        match models.run(SCENARIO, policy, &session.model_id, &prompt, &session.id).await {
            Ok((reply, used)) => {
                session.model_id = used;
                session.forget_parts(lasting);
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

    /// Read an open session for a one-off follow-up while preserving its transcript.
    pub async fn follow_ephemeral(
        &self, models: &Models, key: &str, policy: &ProcessingPolicy, text: String, parts: Vec<Part>,
    ) -> Result<String> {
        let session = self.sessions.get(key).ok_or_else(|| anyhow!("no open session to follow up in {key}"))?;
        let mut turns = session.turns.clone();
        turns.push(if parts.is_empty() {
            Turn::User(text)
        } else {
            let mut all = vec![Part::Text(text)];
            all.extend(parts);
            Turn::UserParts(all)
        });
        let effort = models.effort(SCENARIO);
        let prompt = Prompt { l1: &self.l1, l2: &session.l2, turns: &turns, effort: effort.as_deref(), schema: None };
        let (reply, _) = models.run(SCENARIO, policy, &session.model_id, &prompt, &session.id).await?;
        Ok(reply.text)
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
    fn sessions_count_the_images_they_replay() {
        let mut s = session();
        assert_eq!(s.images(), 0);
        let parts = vec![Part::Text("看".into()), Part::Jpeg("QQ==".into()), Part::Jpeg("Qg==".into())];
        assert_eq!(image_count(&parts), 2);
        s.turns.push(Turn::UserParts(parts));
        s.turns.push(Turn::Assistant { text: "好".into(), raw: None });
        assert_eq!(s.images(), 2);
    }

    #[test]
    fn parts_for_ephemeral_are_not_replayed() {
        let parts = || vec![Part::Text("看".into()), Part::Jpeg("QQ==".into()), Part::Text("候选".into()), Part::Jpeg("Qg==".into())];
        let mut s = session();
        s.turns.push(Turn::UserParts(parts()));
        s.forget_parts(2);
        assert_eq!(s.images(), 1);
        let mut s = session();
        s.turns.push(Turn::UserParts(parts()));
        s.forget_parts(1);
        assert!(matches!(s.turns.last(), Some(Turn::User(text)) if text == "看"));
    }

    fn cached_speaker() -> (Speaker, serde_json::Value) {
        let mut speaker = Speaker::new("persona".into(), SessionCfg {
            max_turns: 20, idle_minutes: 30, recap_messages: 10, debounce: Default::default(),
        });
        let mut session = session();
        let raw = serde_json::json!([
            {"type": "thinking", "thinking": "cached", "signature": "provider-signature"},
            {"type": "text", "text": "draft"}
        ]);
        session.turns.push(Turn::Assistant { text: "draft".into(), raw: Some(raw.clone()) });
        speaker.sessions.insert("group".into(), session);
        (speaker, raw)
    }

    #[tokio::test]
    async fn later_turns_keep_the_cached_assistant_blocks() {
        let (mut speaker, raw) = cached_speaker();
        let store = crate::store::Store::memory();
        let cfg = crate::config::Config::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let mut model = cfg.models.models[0].clone();
        model.id = "test".into();
        model.q = 100.0;
        model.vision = true;
        let mut scenarios = cfg.scenarios;
        scenarios.get_mut(SCENARIO).unwrap().model = None;
        let router = crate::mind::router::Router::new(vec![model], scenarios);
        let models = Models::stub(store, vec!["next reply".into()]).with_stub_router(router);
        let turn = SayTurn { now: Utc::now(), text: "next turn".into(), images: vec![], ephemeral: vec![] };
        let reply = speaker.say(&models, "group", &ProcessingPolicy::Any, turn, || panic!("session rotated")).await.unwrap();
        assert_eq!(reply, "next reply");
        let turns = &speaker.sessions["group"].turns;
        assert_eq!(turns.len(), 4);
        assert!(matches!(&turns[1], Turn::Assistant { text, raw: Some(blocks) } if text == "draft" && *blocks == raw));
        drop(models);
    }

    #[tokio::test]
    async fn ephemeral_followup_leaves_cached_turns_intact() {
        let (speaker, raw) = cached_speaker();
        let store = crate::store::Store::memory();
        let models = Models::stub(store, vec!["picked".into()]);
        let reply = speaker.follow_ephemeral(&models, "group", &ProcessingPolicy::Any, "choose".into(),
            vec![Part::Jpeg("QQ==".into())]).await.unwrap();
        assert_eq!(reply, "picked");
        assert_eq!(models.stub_prompts()[0].2, "choose");
        let turns = &speaker.sessions["group"].turns;
        assert_eq!(turns.len(), 2);
        assert!(matches!(&turns[1], Turn::Assistant { raw: Some(blocks), .. } if *blocks == raw));
        drop(models);
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
