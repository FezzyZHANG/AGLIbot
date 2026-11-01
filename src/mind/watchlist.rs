//! Store KV owns the private watchlist; YAML is a disposable view.

use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::store::Store;
use crate::work::board::{Board, Task, TaskKind, TaskStatus};

const KEY: &str = "life:watchlist";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status { Active, Dropped }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Topic {
    pub id: String,
    pub topic: String,
    pub why: String,
    pub added_from: String,
    pub interval_days: u32,
    pub last_checked: Option<DateTime<Utc>>,
    pub next_due: DateTime<Utc>,
    pub status: Status,
    #[serde(default)]
    pub owner_approved: bool,
    #[serde(default)]
    pub task_id: Option<String>,
}

pub fn load(store: &dyn Store) -> Result<Vec<Topic>> {
    store.kv_get(KEY)?.map(serde_json::from_value).transpose().map(|v| v.unwrap_or_default()).map_err(Into::into)
}

fn save(store: &dyn Store, topics: &[Topic]) -> Result<()> {
    store.kv_set(KEY, serde_json::to_value(topics)?)
}

pub fn render_view(store: &dyn Store, mind: &Path) -> Result<()> {
    let dir = mind.join("life");
    let text = serde_yaml::to_string(&load(store)?)?;
    let path = dir.join("watchlist.yaml");
    if std::fs::read_to_string(&path).ok().as_deref() == Some(&text) { return Ok(()) }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(path, text)?;
    Ok(())
}

/// Markers are stripped in every chat; only an owner private chat can grant approval.
pub fn accept_reply(store: &dyn Store, mind: &Path, raw: &str, owner_private: bool, chat: &str, now: DateTime<Utc>) -> Result<String> {
    if !raw.contains("<关注") { return Ok(raw.to_string()) }
    let mut text = raw.to_string();
    let mut topics = load(store)?;
    let mut changed = false;
    while let Some(start) = text.find("<关注") {
        let Some(head) = text[start..].find('>').map(|n| start + n) else { break };
        let Some(end) = text[head..].find("</关注>").map(|n| head + n) else { break };
        let attrs = text[start + "<关注".len()..head].trim();
        let interval = if attrs.is_empty() { Some(7) } else {
            attrs.strip_prefix("间隔=\"").and_then(|s| s.strip_suffix('"')).and_then(|s| s.parse::<u32>().ok())
        };
        let body = text[head + 1..end].trim().to_string();
        if owner_private && !body.is_empty() && body.chars().count() <= 500
            && let Some(days) = interval.filter(|d| (1..=3650).contains(d))
        {
            let id = format!("watch-{:x}", Sha256::digest(body.as_bytes()));
            if let Some(t) = topics.iter_mut().find(|t| t.id == id) {
                t.interval_days = days;
                t.status = Status::Active;
                t.owner_approved = true;
                t.added_from = format!("chat:{chat}");
                t.next_due = now + Duration::days(i64::from(days));
            } else {
                topics.push(Topic { id, topic: body, why: "主人要求持续关注".into(), added_from: format!("chat:{chat}"),
                    interval_days: days, last_checked: None, next_due: now + Duration::days(i64::from(days)),
                    status: Status::Active, owner_approved: true, task_id: None });
            }
            changed = true;
        }
        text.replace_range(start..end + "</关注>".len(), "");
    }
    if changed {
        save(store, &topics)?;
        render_view(store, mind)?;
    }
    Ok(text)
}

pub fn due(store: &dyn Store, board: &Board, now: DateTime<Utc>) -> Result<Vec<Topic>> {
    let mut topics: Vec<_> = load(store)?.into_iter().filter(|t| t.status == Status::Active && t.next_due <= now
        && !t.task_id.as_deref().and_then(|id| board.get(id)).is_some_and(|task| task.status.is_open())).collect();
    topics.sort_by(|a, b| a.next_due.cmp(&b.next_due).then(a.id.cmp(&b.id)));
    topics.truncate(3);
    Ok(topics)
}

/// Recheck the authoritative entry before creating an ordinary board task.
pub fn create_task(store: &dyn Store, board: &mut Board, id: &str, owner: &str, now: DateTime<Utc>) -> Result<Option<Task>> {
    if !due(store, board, now)?.iter().any(|t| t.id == id) { return Ok(None) }
    let mut topics = load(store)?;
    let Some(t) = topics.iter_mut().find(|t| t.id == id) else { return Ok(None) };
    let status = if t.owner_approved { TaskStatus::Inbox } else { TaskStatus::Waiting };
    let requester = if t.owner_approved { owner } else { "me" };
    let task = board.add(&format!("跟进：{}", t.topic), TaskKind::Research, requester, status, None)?;
    t.last_checked = Some(now);
    t.next_due = now + Duration::days(i64::from(t.interval_days.clamp(1, 3650)));
    t.task_id = Some(task.id.clone());
    save(store, &topics)?;
    Ok(Some(task))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mind::decide::{self, Action, Decision, Situation};
    use crate::store::jsonl::JsonlStore;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("agli-watch-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn store(&self) -> JsonlStore { JsonlStore::open(&self.0.join("data")).unwrap() }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { std::fs::remove_dir_all(&self.0).unwrap(); }
    }

    #[test]
    fn markers_require_owner_private_and_valid_interval() {
        let f = Fixture::new("markers");
        let store = f.store();
        let now = DateTime::UNIX_EPOCH;
        let raw = "收到<关注 间隔=\"7\">主题</关注>";
        assert_eq!(accept_reply(&store, &f.0, raw, false, "group:1", now).unwrap(), "收到");
        assert!(load(&store).unwrap().is_empty());
        for raw in ["<关注 间隔=\"0\">x</关注>", "<关注 间隔=\"-1\">x</关注>", "<关注 间隔=\"999999999999\">x</关注>", "<关注> </关注>", "<关注 间隔=\"abc\">x</关注>"] {
            assert!(accept_reply(&store, &f.0, raw, true, "owner", now).unwrap().is_empty());
        }
        assert!(load(&store).unwrap().is_empty());
        accept_reply(&store, &f.0, raw, true, "owner", now).unwrap();
        accept_reply(&store, &f.0, "<关注>主题</关注>", true, "owner", now).unwrap();
        let topics = load(&store).unwrap();
        assert_eq!(topics.len(), 1);
        assert!(topics[0].owner_approved);
        assert_eq!(topics[0].interval_days, 7);
        assert_eq!(topics[0].next_due, now + Duration::days(7));
        assert_eq!(topics[0].added_from, "chat:owner");
    }

    #[test]
    fn kv_survives_reopen_and_repairs_view() {
        let f = Fixture::new("persistence");
        {
            let store = f.store();
            accept_reply(&store, &f.0, "<关注>私密主题</关注>", true, "owner", DateTime::UNIX_EPOCH).unwrap();
        }
        std::fs::write(f.0.join("life/watchlist.yaml"), "tampered").unwrap();
        let store = f.store();
        render_view(&store, &f.0).unwrap();
        let view: Vec<Topic> = serde_yaml::from_str(&std::fs::read_to_string(f.0.join("life/watchlist.yaml")).unwrap()).unwrap();
        assert_eq!(view[0].topic, "私密主题");
        assert_eq!(view[0].id, load(&store).unwrap()[0].id);
    }

    #[test]
    fn due_is_ordered_capped_and_excludes_dropped_and_open_tasks() {
        let f = Fixture::new("due");
        let store = f.store();
        let board = Board::load(&f.0.join("work")).unwrap();
        let now = DateTime::UNIX_EPOCH;
        for i in 1..=5 {
            accept_reply(&store, &f.0, &format!("<关注 间隔=\"{i}\">topic{i}</关注>"), true, "owner", now).unwrap();
        }
        assert!(due(&store, &board, now).unwrap().is_empty());
        let mut topics = load(&store).unwrap();
        topics[0].status = Status::Dropped;
        save(&store, &topics).unwrap();
        let selected = due(&store, &board, now + Duration::days(10)).unwrap();
        assert_eq!(selected.iter().map(|t| t.topic.as_str()).collect::<Vec<_>>(), ["topic2", "topic3", "topic4"]);
    }

    #[test]
    fn stubbed_decision_creates_board_task_and_preserves_approval() {
        let f = Fixture::new("decision");
        let store = f.store();
        let mut board = Board::load(&f.0.join("work")).unwrap();
        let now = DateTime::UNIX_EPOCH + Duration::days(7);
        accept_reply(&store, &f.0, "<关注>主题</关注>", true, "owner", DateTime::UNIX_EPOCH).unwrap();
        let s = Situation { text: String::new(), commitments: vec![], startable: vec![],
            watchlist: due(&store, &board, now).unwrap(), may_message_owner: false };
        let stub_decide = |id: &str| Decision { action: "follow_up".into(), target: Some(id.into()),
            reason: "curious".into(), deferred: vec![], revisit_when: None, mood: None };
        assert_eq!(decide::validate(&stub_decide("unknown"), &s), Action::Nothing);
        let Action::FollowUp { topic_id } = decide::validate(&stub_decide(&s.watchlist[0].id), &s) else { panic!("expected follow-up") };
        let task = create_task(&store, &mut board, &topic_id, "owner", now).unwrap().unwrap();
        assert_eq!(task.kind, TaskKind::Research);
        assert_eq!(task.status, TaskStatus::Inbox);
        assert_eq!(task.requester, "owner");
        let saved = load(&store).unwrap();
        assert_eq!(saved[0].last_checked, Some(now));
        assert_eq!(saved[0].next_due, now + Duration::days(7));
        assert!(create_task(&store, &mut board, &topic_id, "owner", now + Duration::days(8)).unwrap().is_none());
        board.set_status(&task.id, TaskStatus::Done, "done").unwrap();
        let mut topics = load(&store).unwrap();
        topics[0].owner_approved = false;
        topics[0].added_from = "task:research".into();
        save(&store, &topics).unwrap();
        let task = create_task(&store, &mut board, &topic_id, "owner", now + Duration::days(8)).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Waiting);
        assert_eq!(task.requester, "me");
        assert_eq!(Board::load(&f.0.join("work")).unwrap().tasks().len(), 2);
    }
}
