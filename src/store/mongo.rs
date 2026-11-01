//! MongoDB storage with indexed BSON fields and a lossless JSON record. Keeping the record as
//! JSON preserves nanosecond timestamps, u64s and arbitrary KV/review payloads (including `$` keys).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use mongodb::IndexModel;
use mongodb::bson::{self, Document, doc};
use mongodb::options::{ClientOptions, IndexOptions, ReturnDocument};
use mongodb::sync::{Client, Collection, Database};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{AuditRecord, Episode, Group, Lesson, MessageRecord, ModelCallRecord, Person, Relation, ReviewItem, Store};
use crate::ns::Namespace;

const COLLECTIONS: &[&str] =
    &["messages", "model_calls", "episodes", "persons", "lessons", "audit", "reviews", "kv", "relations", "groups"];

pub struct MongoStore {
    db: Database,
    writes: Mutex<()>,
}

// The driver's sync API runs its own Tokio runtime. Leave the caller's runtime before using it.
fn blocking<T: Send>(f: impl FnOnce() -> Result<T> + Send) -> Result<T> {
    match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(f),
        Ok(_) => std::thread::scope(|s| s.spawn(f).join().map_err(|_| anyhow!("MongoDB worker panicked"))?),
        Err(_) => f(),
    }
}

fn timestamp(ts: DateTime<Utc>) -> bson::DateTime {
    bson::DateTime::from_millis(ts.timestamp_millis())
}

// BSON dates have millisecond precision. Fetch the boundary millisecond too, then compare the
// original timestamps in Rust, for both strictly-after and inclusive-since queries.
fn time_filter(ts: DateTime<Utc>) -> Document {
    doc! { "ts": { "$gte": timestamp(ts) } }
}

fn record<T: Serialize>(value: &T, mut fields: Document) -> Result<Document> {
    fields.insert("record", serde_json::to_string(value)?);
    Ok(fields)
}

fn decode<T: DeserializeOwned>(doc: Document) -> Result<T> {
    Ok(serde_json::from_str(doc.get_str("record").context("MongoDB record is missing JSON data")?)?)
}

fn message_fields(m: &MessageRecord) -> Document {
    doc! { "chat_id": &m.chat_id, "ns": m.ns.to_string(), "ts": timestamp(m.ts) }
}

fn model_call_fields(c: &ModelCallRecord) -> Document {
    doc! { "scenario": &c.scenario, "model": &c.model_id, "ts": timestamp(c.ts) }
}

fn indexes() -> Vec<(&'static str, IndexModel)> {
    let mut indexes = vec![
        ("messages", IndexModel::builder().keys(doc! { "chat_id": 1, "ts": 1 }).build()),
        ("messages", IndexModel::builder().keys(doc! { "ns": 1, "ts": 1 }).build()),
        ("episodes", IndexModel::builder().keys(doc! { "ns": 1, "ts": 1 }).build()),
        ("model_calls", IndexModel::builder().keys(doc! { "scenario": 1, "model": 1, "ts": 1 }).build()),
        ("model_calls", IndexModel::builder().keys(doc! { "ts": 1 }).build()),
    ];
    for collection in ["messages", "model_calls", "episodes", "lessons", "audit", "reviews"] {
        indexes.push((
            collection,
            IndexModel::builder()
                .keys(doc! { "_order": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        ));
    }
    indexes
}

impl MongoStore {
    pub fn open(url: &str, database: &str) -> Result<Self> {
        Self::open_for_import(url, database, false)
    }

    pub(super) fn open_for_import(url: &str, database: &str, read_only: bool) -> Result<Self> {
        blocking(|| {
            // Driver errors can quote connection-string input. Do not attach their source here.
            let mut options = ClientOptions::parse(url).run().map_err(|_| {
                anyhow!("MongoDB startup: invalid or unresolvable MONGO_URL (value redacted); no JSONL fallback")
            })?;
            options.server_selection_timeout.get_or_insert(Duration::from_secs(5));
            options.connect_timeout.get_or_insert(Duration::from_secs(5));
            let client = Client::with_options(options)
                .map_err(|_| anyhow!("MongoDB startup: invalid client options (URL redacted); no JSONL fallback"))?;
            let store = Self { db: client.database(database), writes: Mutex::new(()) };
            store.db.run_command(doc! { "ping": 1 }).run()
                .map_err(|_| anyhow!("MongoDB startup: connection/authentication failed; check MONGO_URL, MONGO_DATABASE and server availability; no JSONL fallback"))?;
            if !read_only {
                store.init().map_err(|_| anyhow!("MongoDB startup: creating collections/indexes failed; check database permissions and existing indexes; no JSONL fallback"))?;
            }
            Ok(store)
        })
    }

    fn collection(&self, name: &str) -> Collection<Document> {
        self.db.collection(name)
    }

    fn keyed_snapshot<T: DeserializeOwned + Send>(&self, name: &str) -> Result<BTreeMap<String, T>> {
        blocking(|| {
            self.collection(name)
                .find(doc! {})
                .run()?
                .map(|doc| {
                    let doc = doc?;
                    Ok((doc.get_str("_id")?.to_owned(), decode(doc)?))
                })
                .collect()
        })
    }

    pub(super) fn snapshot(&self) -> Result<super::import::Snapshot> {
        let snapshot = super::import::Snapshot {
            messages: self.scan("messages", doc! {})?,
            model_calls: self.scan("model_calls", doc! {})?,
            episodes: self.episodes()?,
            persons: self.keyed_snapshot("persons")?,
            groups: self.keyed_snapshot("groups")?,
            relations: self.keyed_snapshot("relations")?,
            lessons: self.lessons()?,
            audit: self.scan("audit", doc! {})?,
            reviews: self.reviews()?,
            kv: self.keyed_snapshot("kv")?,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    #[cfg(test)]
    pub(super) fn drop_import_test_database(&self) -> Result<()> {
        anyhow::ensure!(self.db.name().starts_with("aglibot_test_"), "test database required");
        blocking(|| Ok(self.db.drop().run()?))
    }

    fn init(&self) -> Result<()> {
        for name in COLLECTIONS {
            match self.db.create_collection(*name).run() {
                Ok(()) => {}
                Err(e) if matches!(*e.kind, mongodb::error::ErrorKind::Command(ref c) if c.code == 48) => {}
                Err(e) => return Err(e.into()),
            }
        }
        for (name, index) in indexes() {
            self.collection(name).create_index(index).run()?;
        }
        // persons, reviews and kv use their logical key as _id (MongoDB's unique primary key).
        Ok(())
    }

    fn next_order(&self) -> Result<i64> {
        let counter = self
            .collection("_store_counters")
            .find_one_and_update(doc! { "_id": "order" }, doc! { "$inc": { "value": 1_i64 } })
            .upsert(true)
            .return_document(ReturnDocument::After)
            .run()?
            .context("MongoDB order counter is missing")?;
        Ok(counter.get_i64("value")?)
    }

    fn append<T: Serialize>(&self, name: &str, value: &T, fields: Document) -> Result<()> {
        let mut doc = record(value, fields)?;
        blocking(|| {
            let _guard = self.writes.lock().unwrap_or_else(|e| e.into_inner());
            doc.insert("_order", self.next_order()?);
            self.collection(name).insert_one(doc).run()?;
            Ok(())
        })
    }

    fn scan<T: DeserializeOwned + Send>(&self, name: &str, filter: Document) -> Result<Vec<T>> {
        blocking(|| {
            self.collection(name).find(filter).sort(doc! { "_order": 1 }).run()?.map(|doc| decode(doc?)).collect()
        })
    }

    fn save<T: Serialize>(&self, name: &str, key: &str, value: &T, ordered: bool) -> Result<()> {
        let doc = record(value, doc! {})?;
        blocking(|| {
            let _guard = self.writes.lock().unwrap_or_else(|e| e.into_inner());
            let mut update = doc! { "$set": doc };
            if ordered {
                update.insert("$setOnInsert", doc! { "_order": self.next_order()? });
            }
            self.collection(name).update_one(doc! { "_id": key }, update).upsert(true).run()?;
            Ok(())
        })
    }
}

impl Store for MongoStore {
    fn append_message(&self, m: &MessageRecord) -> Result<()> {
        self.append("messages", m, message_fields(m))
    }

    fn recent_in_ns(&self, ns: &Namespace, n: usize) -> Result<Vec<MessageRecord>> {
        if n == 0 {
            return Ok(Vec::new());
        }
        blocking(|| {
            let mut records: Vec<MessageRecord> = self
                .collection("messages")
                .find(doc! { "ns": ns.to_string() })
                .sort(doc! { "_order": -1 })
                .limit(i64::try_from(n).unwrap_or(i64::MAX))
                .run()?
                .map(|doc| decode(doc?))
                .collect::<Result<_>>()?;
            records.reverse();
            Ok(records)
        })
    }

    fn messages_after(&self, ns: &Namespace, after: DateTime<Utc>) -> Result<Vec<MessageRecord>> {
        let mut filter = time_filter(after);
        filter.insert("ns", ns.to_string());
        let mut records: Vec<MessageRecord> = self.scan("messages", filter)?;
        records.retain(|m| m.ts > after);
        Ok(records)
    }

    fn active_namespaces(&self, after: DateTime<Utc>) -> Result<Vec<Namespace>> {
        let records: Vec<MessageRecord> = self.scan("messages", time_filter(after))?;
        let mut namespaces: Vec<_> = records.into_iter().filter(|m| m.ts > after).map(|m| m.ns).collect();
        namespaces.sort();
        namespaces.dedup();
        Ok(namespaces)
    }

    fn append_model_call(&self, c: &ModelCallRecord) -> Result<()> {
        self.append("model_calls", c, model_call_fields(c))
    }

    fn spent_since(&self, since: DateTime<Utc>) -> Result<f64> {
        let calls: Vec<ModelCallRecord> = self.scan("model_calls", time_filter(since))?;
        Ok(calls.iter().filter(|c| c.ts >= since).map(|c| c.cost_usd).sum())
    }

    fn append_episode(&self, e: &Episode) -> Result<()> {
        self.append("episodes", e, doc! { "ns": e.ns.to_string(), "ts": timestamp(e.ts) })
    }

    fn episodes(&self) -> Result<Vec<Episode>> {
        self.scan("episodes", doc! {})
    }

    fn persons(&self) -> Result<BTreeMap<String, Person>> {
        Ok(self.scan::<Person>("persons", doc! {})?.into_iter().map(|p| (p.person_id.clone(), p)).collect())
    }

    fn save_person(&self, p: &Person) -> Result<()> {
        self.save("persons", &p.person_id, p, false)
    }

    fn append_lesson(&self, l: &Lesson) -> Result<()> {
        self.append("lessons", l, doc! { "ts": timestamp(l.ts) })
    }

    fn group(&self, chat_id: &str) -> Result<Option<Group>> {
        blocking(|| self.collection("groups").find_one(doc! { "_id": chat_id }).run()?.map(decode).transpose())
    }

    fn save_group(&self, g: &Group) -> Result<()> {
        self.save("groups", &g.chat_id, g, false)
    }

    fn relations(&self, ns: &Namespace) -> Result<Vec<Relation>> {
        let relations: BTreeMap<String, Relation> = self.keyed_snapshot("relations")?;
        Ok(relations.into_values().filter(|r| &r.ns == ns).collect())
    }

    fn save_relation(&self, r: &Relation) -> Result<()> {
        self.save("relations", &r.key(), r, false)
    }

    fn lessons(&self) -> Result<Vec<Lesson>> {
        self.scan("lessons", doc! {})
    }

    fn append_audit(&self, a: &AuditRecord) -> Result<()> {
        self.append("audit", a, doc! { "ts": timestamp(a.ts) })
    }

    fn audits(&self) -> Result<Vec<AuditRecord>> {
        self.scan("audit", doc! {})
    }

    fn reviews(&self) -> Result<Vec<ReviewItem>> {
        self.scan("reviews", doc! {})
    }

    fn save_review(&self, r: &ReviewItem) -> Result<()> {
        self.save("reviews", &r.id.to_string(), r, true)
    }

    fn next_review_id(&self) -> Result<u64> {
        Ok(self.reviews()?.iter().map(|r| r.id).max().unwrap_or(0) + 1)
    }

    fn kv_get(&self, key: &str) -> Result<Option<Value>> {
        blocking(|| self.collection("kv").find_one(doc! { "_id": key }).run()?.map(decode).transpose())
    }

    fn kv_set(&self, key: &str, value: Value) -> Result<()> {
        self.save("kv", key, &value, false)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{call, contract, message, persisted, time};
    use super::*;
    use mongodb::bson::oid::ObjectId;
    use serde_json::json;

    fn round_trip<T: Serialize + DeserializeOwned>(value: &T, fields: Document) -> T {
        let doc = record(value, fields).unwrap();
        let bytes = bson::to_vec(&doc).unwrap();
        decode(bson::from_slice(&bytes).unwrap()).unwrap()
    }

    #[test]
    fn bson_mapping_preserves_record_precision_and_json_values() {
        let m = message("person/a", "消息", time());
        let fields = message_fields(&m);
        assert_eq!(fields.get_str("ns").unwrap(), "person/a");
        assert_eq!(fields.get_str("chat_id").unwrap(), "1");
        assert_eq!(fields.get_datetime("ts").unwrap().timestamp_millis(), m.ts.timestamp_millis());
        let back: MessageRecord = round_trip(&m, fields);
        assert_eq!(serde_json::to_value(back).unwrap(), serde_json::to_value(m).unwrap());
        let c = call(time(), 0.25);
        let fields = model_call_fields(&c);
        assert_eq!(fields.get_str("scenario").unwrap(), "chat");
        assert_eq!(fields.get_str("model").unwrap(), "model-a");
        let back: ModelCallRecord = round_trip(&c, fields);
        assert_eq!(back.uncached_input, u64::MAX);
        assert_eq!(serde_json::to_value(back).unwrap(), serde_json::to_value(c).unwrap());
        for value in [
            Value::Null,
            json!(u64::MAX),
            json!(-0.0),
            json!({"$date": "literal", "a.b": [true, null], "nul\0key": "ok"}),
        ] {
            assert_eq!(round_trip(&value, doc! {}), value);
        }
        assert!(decode::<MessageRecord>(doc! {}).is_err());
    }

    #[test]
    fn time_filter_includes_boundary_millisecond() {
        for ts in [time(), "1960-01-01T00:00:00.999999999Z".parse().unwrap()] {
            let filter = time_filter(ts);
            let boundary = *filter.get_document("ts").unwrap().get_datetime("$gte").unwrap();
            assert_eq!(boundary.timestamp_millis(), ts.timestamp_millis());
        }
        assert_eq!(time_filter(time()), time_filter(time() + chrono::Duration::nanoseconds(1)));
    }

    #[test]
    fn index_definitions_cover_design_and_ordering() {
        let indexes = indexes();
        for (collection, keys) in [
            ("messages", doc! { "chat_id": 1, "ts": 1 }),
            ("messages", doc! { "ns": 1, "ts": 1 }),
            ("episodes", doc! { "ns": 1, "ts": 1 }),
            ("model_calls", doc! { "scenario": 1, "model": 1, "ts": 1 }),
        ] {
            assert!(indexes.iter().any(|(name, index)| *name == collection && index.keys == keys));
        }
        for collection in ["messages", "model_calls", "episodes", "lessons", "audit", "reviews"] {
            assert!(indexes.iter().any(|(name, index)| *name == collection
                && index.keys == doc! { "_order": 1 }
                && index.options.as_ref().unwrap().unique == Some(true)));
        }
    }

    fn nested_runtime() -> Result<u8> {
        Ok(tokio::runtime::Runtime::new()?.block_on(async { 7 }))
    }

    #[test]
    fn sync_bridge_outside_runtime() {
        assert_eq!(blocking(nested_runtime).unwrap(), 7);
    }

    #[tokio::test]
    async fn sync_bridge_in_current_thread_runtime() {
        assert_eq!(blocking(nested_runtime).unwrap(), 7);
        assert!(MongoStore::open("invalid://secret", "test").is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_bridge_in_multi_thread_runtime() {
        assert_eq!(blocking(nested_runtime).unwrap(), 7);
        assert!(MongoStore::open("invalid://secret", "test").is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires MONGO_URL pointing at a test server; creates and drops a unique test database"]
    async fn mongo_store_contract_and_reopen() -> Result<()> {
        let url = std::env::var("MONGO_URL").context("set MONGO_URL to run MongoDB integration tests")?;
        let database = format!("aglibot_test_{}", ObjectId::new().to_hex());
        let store = MongoStore::open(&url, &database)?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
            contract(&store);
            let reopened = MongoStore::open(&url, &database)?; // Index creation must be idempotent.
            persisted(&reopened);
            blocking(|| {
                let names = reopened.db.list_collection_names().run()?;
                for collection in COLLECTIONS {
                    assert!(names.iter().any(|name| name == collection));
                }
                for (collection, expected) in indexes() {
                    let actual: Vec<_> =
                        reopened.collection(collection).list_indexes().run()?.collect::<mongodb::error::Result<_>>()?;
                    assert!(actual.iter().any(|index| index.keys == expected.keys));
                }
                for collection in ["persons", "reviews", "kv"] {
                    let doc = reopened.collection(collection).find_one(doc! {}).run()?.unwrap();
                    assert!(reopened.collection(collection).insert_one(doc).run().is_err());
                }
                let audits: Vec<AuditRecord> = reopened.scan("audit", doc! {})?;
                assert_eq!(audits.len(), 1);
                assert_eq!(audits[0].outcome, super::super::Outcome::Denied);
                Ok(())
            })
        }));
        let cleanup = blocking(|| Ok(store.db.drop().run()?));
        match result {
            Ok(result) => {
                result?;
                cleanup?;
            }
            Err(panic) => {
                let _ = cleanup;
                std::panic::resume_unwind(panic);
            }
        }
        Ok(())
    }
}
