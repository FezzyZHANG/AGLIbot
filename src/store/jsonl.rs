//! Append-only JSONL files under `data/`. Reads scan whole files, which is fine at phase-1 volume.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{MessageRecord, ModelCallRecord, Store};

const MESSAGES: &str = "messages.jsonl";
const MODEL_CALLS: &str = "model_calls.jsonl";

pub struct JsonlStore {
    dir: PathBuf,
    write_lock: Mutex<()>,
}

impl JsonlStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(JsonlStore { dir: dir.to_path_buf(), write_lock: Mutex::new(()) })
    }

    fn append<T: Serialize>(&self, file: &str, rec: &T) -> Result<()> {
        let line = serde_json::to_string(rec)?;
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut f = OpenOptions::new().create(true).append(true).open(self.dir.join(file))?;
        writeln!(f, "{line}")?;
        Ok(())
    }

    fn scan<T: DeserializeOwned>(&self, file: &str) -> Result<Vec<T>> {
        let path = self.dir.join(file);
        let f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for (i, line) in BufReader::new(f).lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(&line) {
                Ok(rec) => out.push(rec),
                Err(e) => tracing::warn!("skipping bad line {} in {}: {e}", i + 1, path.display()),
            }
        }
        Ok(out)
    }
}

impl Store for JsonlStore {
    fn append_message(&self, m: &MessageRecord) -> Result<()> {
        self.append(MESSAGES, m)
    }

    fn recent_messages(&self, chat_id: &str, n: usize) -> Result<Vec<MessageRecord>> {
        let mut all: Vec<MessageRecord> = self.scan(MESSAGES)?;
        all.retain(|m| m.chat_id == chat_id);
        let skip = all.len().saturating_sub(n);
        Ok(all.split_off(skip))
    }

    fn append_model_call(&self, c: &ModelCallRecord) -> Result<()> {
        self.append(MODEL_CALLS, c)
    }

    fn spent_since(&self, since: DateTime<Utc>) -> Result<f64> {
        let calls: Vec<ModelCallRecord> = self.scan(MODEL_CALLS)?;
        Ok(calls.iter().filter(|c| c.ts >= since).map(|c| c.cost_usd).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ns::{Namespace, Scope};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aglibot-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn msg(chat: &str, text: &str) -> MessageRecord {
        MessageRecord {
            ts: Utc::now(),
            platform: "telegram".into(),
            chat_id: chat.into(),
            sender: "owner".into(),
            outgoing: false,
            text: text.into(),
            ns: Namespace::Person("owner".into()),
            scope: Scope::Persons { ids: vec!["owner".into()] },
        }
    }

    #[test]
    fn recent_messages_filters_and_keeps_order() {
        let dir = temp_dir("recent");
        let store = JsonlStore::open(&dir).unwrap();
        for i in 0..5 {
            store.append_message(&msg("a", &format!("a{i}"))).unwrap();
            store.append_message(&msg("b", &format!("b{i}"))).unwrap();
        }
        let got: Vec<String> = store.recent_messages("a", 3).unwrap().into_iter().map(|m| m.text).collect();
        assert_eq!(got, ["a2", "a3", "a4"]);
        assert!(store.recent_messages("none", 3).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn spend_sums_calls_since() {
        let dir = temp_dir("spend");
        let store = JsonlStore::open(&dir).unwrap();
        let call = |cost: f64, ts: DateTime<Utc>| ModelCallRecord {
            ts,
            scenario: "chat".into(),
            model_id: "haiku".into(),
            session_id: "s".into(),
            uncached_input: 0,
            cached_input: 0,
            cache_write: 0,
            output: 0,
            latency_ms: 0,
            cost_usd: cost,
            ok: true,
            error: None,
            escalated_from: None,
        };
        let now = Utc::now();
        store.append_model_call(&call(1.0, now - chrono::Duration::days(2))).unwrap();
        store.append_model_call(&call(0.25, now)).unwrap();
        store.append_model_call(&call(0.5, now)).unwrap();
        let spent = store.spent_since(now - chrono::Duration::hours(1)).unwrap();
        assert!((spent - 0.75).abs() < 1e-12);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
