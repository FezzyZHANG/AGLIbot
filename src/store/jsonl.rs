//! File-backed store under `data/`. Append-only collections are JSONL; mutable ones (persons,
//! reviews, kv) are whole JSON documents rewritten atomically. Reads scan whole files, which is
//! fine at single-person volume; MongoDB replaces this when volume demands it.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{AuditRecord, Episode, Group, Lesson, MessageRecord, ModelCallRecord, Person, Relation, ReviewItem, Store};
use crate::ns::Namespace;

const MESSAGES: &str = "messages.jsonl";
const MODEL_CALLS: &str = "model_calls.jsonl";
const EPISODES: &str = "episodes.jsonl";
const LESSONS: &str = "lessons.jsonl";
const AUDIT: &str = "audit.jsonl";
const PERSONS: &str = "persons.json";
const GROUPS: &str = "groups.json";
const RELATIONS: &str = "relations.json";
const REVIEWS: &str = "reviews.json";
const KV: &str = "kv.json";

pub struct JsonlStore {
    dir: PathBuf,
    lock: Mutex<()>,
}

impl JsonlStore {
    pub(super) fn import_snapshot(dir: &Path) -> Result<super::import::Snapshot> {
        anyhow::ensure!(dir.is_dir(), "source must be an existing data directory");
        let store = Self { dir: dir.to_path_buf(), lock: Mutex::new(()) };
        let snapshot = super::import::Snapshot {
            messages: store.scan_mode(MESSAGES, true)?,
            model_calls: store.scan_mode(MODEL_CALLS, true)?,
            episodes: store.scan_mode(EPISODES, true)?,
            persons: store.persons()?,
            groups: store.read_doc(GROUPS)?,
            relations: store.read_doc(RELATIONS)?,
            lessons: store.scan_mode(LESSONS, true)?,
            audit: store.scan_mode(AUDIT, true)?,
            reviews: store.reviews()?,
            kv: store.read_doc(KV)?,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(JsonlStore { dir: dir.to_path_buf(), lock: Mutex::new(()) })
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn append<T: Serialize>(&self, file: &str, rec: &T) -> Result<()> {
        let line = serde_json::to_string(rec)?;
        let _g = self.guard();
        let mut f = OpenOptions::new().create(true).append(true).open(self.dir.join(file))?;
        writeln!(f, "{line}")?;
        Ok(())
    }

    fn scan<T: DeserializeOwned>(&self, file: &str) -> Result<Vec<T>> {
        self.scan_mode(file, false)
    }

    fn scan_mode<T: DeserializeOwned>(&self, file: &str, strict: bool) -> Result<Vec<T>> {
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
                Err(e) if strict => {
                    return Err(e).with_context(|| format!("parsing {} line {}", path.display(), i + 1));
                }
                Err(e) => tracing::warn!("skipping bad line {} in {}: {e}", i + 1, path.display()),
            }
        }
        Ok(out)
    }

    fn read_doc<T: DeserializeOwned + Default>(&self, file: &str) -> Result<T> {
        let path = self.dir.join(file);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Write to a temp file then rename, so a crash never leaves half a document.
    fn write_doc<T: Serialize>(&self, file: &str, doc: &T) -> Result<()> {
        let path = self.dir.join(file);
        let tmp = self.dir.join(format!(".{file}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(doc)?)?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))
    }

    fn update_doc<T, F>(&self, file: &str, f: F) -> Result<()>
    where
        T: Serialize + DeserializeOwned + Default,
        F: FnOnce(&mut T),
    {
        let _g = self.guard();
        let mut doc: T = self.read_doc(file)?;
        f(&mut doc);
        self.write_doc(file, &doc)
    }
}

impl Store for JsonlStore {
    fn append_message(&self, m: &MessageRecord) -> Result<()> {
        self.append(MESSAGES, m)
    }

    fn recent_in_ns(&self, ns: &Namespace, n: usize) -> Result<Vec<MessageRecord>> {
        let mut all: Vec<MessageRecord> = self.scan(MESSAGES)?;
        all.retain(|m| &m.ns == ns);
        let skip = all.len().saturating_sub(n);
        Ok(all.split_off(skip))
    }

    fn messages_after(&self, ns: &Namespace, after: DateTime<Utc>) -> Result<Vec<MessageRecord>> {
        let mut all: Vec<MessageRecord> = self.scan(MESSAGES)?;
        all.retain(|m| &m.ns == ns && m.ts > after);
        Ok(all)
    }

    fn active_namespaces(&self, after: DateTime<Utc>) -> Result<Vec<Namespace>> {
        let all: Vec<MessageRecord> = self.scan(MESSAGES)?;
        let mut out: Vec<Namespace> = all.into_iter().filter(|m| m.ts > after).map(|m| m.ns).collect();
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn append_model_call(&self, c: &ModelCallRecord) -> Result<()> {
        self.append(MODEL_CALLS, c)
    }

    fn spent_since(&self, since: DateTime<Utc>) -> Result<f64> {
        let calls: Vec<ModelCallRecord> = self.scan(MODEL_CALLS)?;
        Ok(calls.iter().filter(|c| c.ts >= since).map(|c| c.cost_usd).sum())
    }

    fn append_episode(&self, e: &Episode) -> Result<()> {
        self.append(EPISODES, e)
    }

    fn episodes(&self) -> Result<Vec<Episode>> {
        self.scan(EPISODES)
    }

    fn persons(&self) -> Result<BTreeMap<String, Person>> {
        let _g = self.guard();
        self.read_doc(PERSONS)
    }

    fn save_person(&self, p: &Person) -> Result<()> {
        self.update_doc(PERSONS, |doc: &mut BTreeMap<String, Person>| {
            doc.insert(p.person_id.clone(), p.clone());
        })
    }

    fn append_lesson(&self, l: &Lesson) -> Result<()> {
        self.append(LESSONS, l)
    }

    fn group(&self, chat_id: &str) -> Result<Option<Group>> {
        let _g = self.guard();
        let groups: BTreeMap<String, Group> = self.read_doc(GROUPS)?;
        Ok(groups.get(chat_id).cloned())
    }

    fn save_group(&self, g: &Group) -> Result<()> {
        self.update_doc(GROUPS, |doc: &mut BTreeMap<String, Group>| {
            doc.insert(g.chat_id.clone(), g.clone());
        })
    }

    fn relations(&self, ns: &Namespace) -> Result<Vec<Relation>> {
        let _g = self.guard();
        let relations: BTreeMap<String, Relation> = self.read_doc(RELATIONS)?;
        Ok(relations.into_values().filter(|r| &r.ns == ns).collect())
    }

    fn save_relation(&self, r: &Relation) -> Result<()> {
        self.update_doc(RELATIONS, |doc: &mut BTreeMap<String, Relation>| {
            doc.insert(r.key(), r.clone());
        })
    }

    fn lessons(&self) -> Result<Vec<Lesson>> {
        self.scan(LESSONS)
    }

    fn append_audit(&self, a: &AuditRecord) -> Result<()> {
        self.append(AUDIT, a)
    }

    fn audits(&self) -> Result<Vec<AuditRecord>> {
        let _g = self.guard();
        // A corrupt outcome must not disappear during a resume inspection.
        self.scan_mode(AUDIT, true)
    }

    fn reviews(&self) -> Result<Vec<ReviewItem>> {
        let _g = self.guard();
        self.read_doc(REVIEWS)
    }

    fn save_review(&self, r: &ReviewItem) -> Result<()> {
        self.update_doc(REVIEWS, |doc: &mut Vec<ReviewItem>| match doc.iter_mut().find(|x| x.id == r.id) {
            Some(slot) => *slot = r.clone(),
            None => doc.push(r.clone()),
        })
    }

    fn next_review_id(&self) -> Result<u64> {
        Ok(self.reviews()?.iter().map(|r| r.id).max().unwrap_or(0) + 1)
    }

    fn kv_get(&self, key: &str) -> Result<Option<Value>> {
        let _g = self.guard();
        let doc: BTreeMap<String, Value> = self.read_doc(KV)?;
        Ok(doc.get(key).cloned())
    }

    fn kv_set(&self, key: &str, value: Value) -> Result<()> {
        self.update_doc(KV, |doc: &mut BTreeMap<String, Value>| {
            doc.insert(key.to_string(), value);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ns::Scope;
    use crate::store::{PersonKind, ReviewCategory, ReviewStatus, Tier};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aglibot-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn msg(ns: &str, text: &str) -> MessageRecord {
        MessageRecord {
            ts: Utc::now(),
            platform: "telegram".into(),
            chat_id: "1".into(),
            sender: "owner".into(),
            sender_name: String::new(),
            outgoing: false,
            text: text.into(),
            ns: ns.parse().unwrap(),
            scope: Scope::Persons { ids: vec!["owner".into()] },
        }
    }

    #[test]
    fn recent_in_ns_filters_and_keeps_order() {
        let dir = temp_dir("recent");
        let store = JsonlStore::open(&dir).unwrap();
        for i in 0..5 {
            store.append_message(&msg("person/a", &format!("a{i}"))).unwrap();
            store.append_message(&msg("person/b", &format!("b{i}"))).unwrap();
        }
        let a: Namespace = "person/a".parse().unwrap();
        let got: Vec<String> = store.recent_in_ns(&a, 3).unwrap().into_iter().map(|m| m.text).collect();
        assert_eq!(got, ["a2", "a3", "a4"]);
        assert_eq!(store.active_namespaces(Utc::now() - chrono::Duration::hours(1)).unwrap().len(), 2);
        assert!(store.recent_in_ns(&"person/none".parse().unwrap(), 3).unwrap().is_empty());
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

    #[test]
    fn documents_round_trip() {
        let dir = temp_dir("docs");
        let store = JsonlStore::open(&dir).unwrap();
        let mut p = Person {
            person_id: "qq:1".into(),
            platform_ids: vec!["qq:1".into()],
            name: "A".into(),
            kind: PersonKind::Human,
            tier: Tier::Groupmate,
            trust: 0.0,
            intimacy: 0.0,
            facts: vec![],
            last_seen: Utc::now(),
        };
        store.save_person(&p).unwrap();
        p.name = "B".into();
        store.save_person(&p).unwrap();
        assert_eq!(store.persons().unwrap()["qq:1"].name, "B");

        let id = store.next_review_id().unwrap();
        let mut r = ReviewItem {
            id,
            ts: Utc::now(),
            category: ReviewCategory::Act,
            subject: "s".into(),
            detail: "d".into(),
            verdict: "转交主人".into(),
            status: ReviewStatus::Open,
            payload: Value::Null,
            notified: false,
        };
        store.save_review(&r).unwrap();
        r.status = ReviewStatus::Approved;
        store.save_review(&r).unwrap();
        assert_eq!(store.reviews().unwrap().len(), 1);
        assert_eq!(store.next_review_id().unwrap(), id + 1);

        store.kv_set("k", serde_json::json!(3)).unwrap();
        assert_eq!(store.kv_get("k").unwrap(), Some(serde_json::json!(3)));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
