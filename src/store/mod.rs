//! Async storage shared by the runtime and in-memory tests.

mod mem;
mod task;
pub(crate) use task::{RECOVERY_NOTE, task_status_line};
pub mod migrate;
mod mongo;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ns::{Namespace, Scope};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub ts: DateTime<Utc>,
    pub platform: String,
    pub chat_id: String,
    /// person_id of the sender, or "me" for his own messages.
    pub sender: String,
    #[serde(default)]
    pub sender_name: String,
    pub outgoing: bool,
    pub text: String,
    pub ns: Namespace,
    pub scope: Scope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCallRecord {
    pub ts: DateTime<Utc>,
    pub scenario: String,
    pub model_id: String,
    pub session_id: String,
    pub uncached_input: u64,
    pub cached_input: u64,
    pub cache_write: u64,
    pub output: u64,
    pub latency_ms: u64,
    pub cost_usd: f64,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub escalated_from: Option<String>,
}

/// Something that happened, distilled from messages. ns/scope come from the source chat, never the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub id: String,
    pub ts: DateTime<Utc>,
    pub participants: Vec<String>,
    pub summary: String,
    #[serde(default)]
    pub emotion: String,
    pub salience: f64,
    pub ns: Namespace,
    pub scope: Scope,
}

impl Episode {
    pub fn age_days(&self, now: DateTime<Utc>) -> f64 {
        (now - self.ts).num_seconds().max(0) as f64 / 86_400.0
    }

    pub fn effective_salience(&self, now: DateTime<Utc>, half_life_days: f64) -> f64 {
        self.salience * 0.5_f64.powf(self.age_days(now) / half_life_days)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub text: String,
    pub ns: Namespace,
    pub scope: Scope,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonKind {
    Human,
    Agent,
}

/// Dunbar-style closeness tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Owner,
    Acquaintance,
    Groupmate,
    Stranger,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    pub person_id: String,
    /// `platform:native_id`.
    pub platform_ids: Vec<String>,
    pub name: String,
    pub kind: PersonKind,
    pub tier: Tier,
    #[serde(default)]
    pub trust: f64,
    #[serde(default)]
    pub intimacy: f64,
    #[serde(default)]
    pub last_update: Option<DateTime<Utc>>, // newest episode considered by reciprocity
    #[serde(default)]
    pub last_update_day: Option<chrono::NaiveDate>,
    #[serde(default)]
    pub facts: Vec<Fact>,
    pub last_seen: DateTime<Utc>,
    /// Groups (`platform:group_id`) they were seen in; append-only, see `Scope::permits`.
    #[serde(default)]
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Sent,
    Unknown,
    Failed,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub ts: DateTime<Utc>,
    pub requester: String,
    pub tool: String,
    pub allowed: bool,
    pub reason: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewCategory {
    Verify,
    Act,
    Widen,
    Evolve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// Waiting for the owner (forwarded by judge).
    Open,
    Approved,
    Rejected,
    /// Settled without the owner (judge passed or deferred it).
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewItem {
    pub id: u64,
    pub ts: DateTime<Utc>,
    pub category: ReviewCategory,
    pub subject: String,
    pub detail: String,
    /// Judge's verdict (`通过` / `修改后再审` / `暂缓` / `转交主人`) and reasons.
    pub verdict: String,
    pub status: ReviewStatus,
    /// What to do on approval, interpreted by the flow that created the item.
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub notified: bool,
    #[serde(default)]
    pub urgent: bool,
    #[serde(default)]
    pub pushed: bool,
}

/// What became of an image, one record per file content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaStatus {
    /// A sticker he may use.
    Active,
    /// The filter turned it down; the owner can still keep it.
    Rejected,
    /// Described for its own private chat; never shown or collected.
    Seen,
    /// The owner stopped it. The record stays, so it is not collected again.
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StickerScope {
    Any,
    Origin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRecord {
    /// sha256 of the file.
    pub hash: String,
    pub ext: String,
    pub status: MediaStatus,
    /// The model's judgement, kept apart from `status`, which also depends on where it was seen.
    pub keep: bool,
    pub description: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub use_when: String,
    #[serde(default)]
    pub avoid: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub scope: StickerScope,
    #[serde(default)]
    pub scope_reason: String,
    /// `platform:chat_id` where it was first collectable.
    pub origin: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub sightings: u64,
    #[serde(default)]
    pub sent_count: u64,
    #[serde(default)]
    pub last_sent: Option<DateTime<Utc>>,
}

/// Rebuildable from `MediaRecord`: a changed `text_hash` or model means it is rebuilt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StickerVector {
    pub hash: String,
    pub model_id: String,
    pub text_hash: String,
    pub vector: Vec<f32>,
}

pub struct MediaCaptionPatch {
    pub description: String,
    pub text: String,
    pub use_when: String,
    pub avoid: String,
    pub tags: Vec<String>,
}

#[derive(Default)]
pub struct MediaEditPatch {
    pub description: Option<String>,
    pub use_when: Option<String>,
    pub avoid: Option<String>,
    pub tags: Option<Vec<String>>,
    pub scope: Option<StickerScope>,
    pub status: Option<MediaStatus>,
}

pub struct NewReview {
    pub ts: DateTime<Utc>,
    pub category: ReviewCategory,
    pub subject: String,
    pub detail: String,
    pub verdict: String,
    pub status: ReviewStatus,
    pub payload: Value,
    pub notified: bool,
    pub urgent: bool,
    pub pushed: bool,
}

impl NewReview {
    fn with_id(self, id: u64) -> ReviewItem {
        ReviewItem {
            id,
            ts: truncate_ms(self.ts),
            category: self.category,
            subject: self.subject,
            detail: self.detail,
            verdict: self.verdict,
            status: self.status,
            payload: self.payload,
            notified: self.notified,
            urgent: self.urgent,
            pushed: self.pushed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Research,
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Approved and waiting to be started.
    Inbox,
    Active,
    /// Waiting for the owner (requests from others, or a stopped task).
    Waiting,
    Done,
    Dropped,
}

impl TaskStatus {
    pub fn is_open(self) -> bool {
        matches!(self, Self::Inbox | Self::Active | Self::Waiting)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRef {
    pub platform: String,
    pub chat_id: String,
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub title: String,
    pub kind: TaskKind,
    pub status: TaskStatus,
    /// person_id of whoever asked.
    pub requester: String,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub origin: Option<ThreadRef>,
    #[serde(default)]
    pub threads: Vec<ThreadRef>,
    /// Frozen at creation / status change; ordinary log appends must not change L2.
    #[serde(default)]
    pub status_log: String,
    #[serde(default)]
    pub followups: Vec<TaskFollowup>,
    #[serde(default)]
    pub current_round: u32,
    #[serde(default)]
    pub last_verdict: Option<TaskVerdict>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFollowup {
    pub seq: u64,
    pub ts: DateTime<Utc>,
    pub body: String,
    pub thread: ThreadRef,
    #[serde(default)]
    pub consumed_round: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskVerdict {
    pub review_id: u64,
    pub round: u32,
    pub verdict: String,
    #[serde(default)]
    pub reasons: String,
    #[serde(default)]
    pub fixes: String,
}

#[derive(Clone)]
pub struct Store(Arc<StoreBackend>);

enum StoreBackend {
    Mongo(mongo::MongoStore),
    Mem(mem::MemStore),
}

pub async fn open(cfg: &crate::config::MongoCfg) -> Result<Store> {
    Store::open(cfg).await
}

fn truncate_ms(dt: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(dt.timestamp_millis())
        .expect("timestamp originated from chrono")
}

macro_rules! store_methods {
    ($($(#[$attr:meta])* fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
        $($(#[$attr])*
        pub async fn $name(&self, $($arg: $ty),*) -> Result<$ret> {
            match self.0.as_ref() {
                StoreBackend::Mongo(store) => store.$name($($arg),*).await,
                StoreBackend::Mem(store) => store.$name($($arg),*),
            }
        })*
    };
}

impl Store {
    pub async fn open(cfg: &crate::config::MongoCfg) -> Result<Self> {
        let store = mongo::MongoStore::connect(cfg).await?;
        let legacy = migrate::has_legacy_schema(&store)
            .await
            .map_err(|_| anyhow::anyhow!("MongoDB startup: could not inspect database schema"))?;
        anyhow::ensure!(
            !legacy,
            "MongoDB startup: legacy storage detected; run migrate-bson --apply while AGLIbot is stopped"
        );
        store.init().await.map_err(|_| anyhow::anyhow!(
            "MongoDB startup: index setup failed; check permissions and run migrate-bson for legacy data"
        ))?;
        Ok(Self(Arc::new(StoreBackend::Mongo(store))))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn memory() -> Self {
        Self(Arc::new(StoreBackend::Mem(mem::MemStore::default())))
    }

    store_methods! {
        fn tasks() -> Vec<TaskRecord>;
        fn task(id: &str) -> Option<TaskRecord>;
        fn save_task(task: &TaskRecord) -> ();
        fn append_task_followup(id: &str, body: &str, thread: ThreadRef, now: DateTime<Utc>) -> TaskRecord;
        fn mark_task_followups_consumed(id: &str, up_to_seq: u64, round: u32) -> ();
        fn set_task_status(id: &str, status: TaskStatus, note: &str, now: DateTime<Utc>, verdict: Option<TaskVerdict>) -> TaskRecord;
        fn recover_active_tasks(now: DateTime<Utc>) -> Vec<String>;
        fn append_message(m: &MessageRecord) -> ();
        fn recent_in_ns(ns: &Namespace, n: usize) -> Vec<MessageRecord>;
        fn messages_after(ns: &Namespace, after: DateTime<Utc>) -> Vec<MessageRecord>;
        fn active_namespaces(after: DateTime<Utc>) -> Vec<Namespace>;
        fn append_model_call(c: &ModelCallRecord) -> ();
        fn spent_since(since: DateTime<Utc>) -> f64;
        #[cfg(test)]
        fn model_calls() -> Vec<ModelCallRecord>;
        fn append_episode(e: &Episode) -> ();
        fn episodes() -> Vec<Episode>;
        fn persons() -> BTreeMap<String, Person>;
        fn save_person(p: &Person) -> ();
        fn append_audit(a: &AuditRecord) -> ();
        fn audits() -> Vec<AuditRecord>;
        fn reviews() -> Vec<ReviewItem>;
        fn create_review(new: NewReview) -> ReviewItem;
        fn save_review(r: &ReviewItem) -> ();
        fn mark_reviews_notified(ids: &[u64]) -> ();
        fn kv_get(key: &str) -> Option<Value>;
        fn kv_set(key: &str, value: Value) -> ();
        fn kv_keys_with_prefix(prefix: &str) -> Vec<String>;
        fn lexicon() -> Vec<Value>;
        fn insert_lexicon(docs: &[Value]) -> ();
        fn media(hash: &str) -> Option<MediaRecord>;
        fn save_media(record: &MediaRecord) -> ();
        fn touch_media_sighting(hash: &str, now: DateTime<Utc>, promote: Option<(MediaStatus, String)>) -> Option<MediaRecord>;
        fn record_media_sent(hash: &str, now: DateTime<Utc>) -> ();
        fn update_media_caption(hash: &str, patch: &MediaCaptionPatch) -> bool;
        fn patch_media(hash: &str, patch: &MediaEditPatch) -> Option<MediaRecord>;
        fn all_media() -> Vec<MediaRecord>;
        fn delete_media(hash: &str) -> ();
        fn delete_unused_media(hash: &str, cutoff: DateTime<Utc>) -> bool;
        fn sticker_vectors(model_id: &str) -> BTreeMap<String, StickerVector>;
        fn save_sticker_vector(vector: &StickerVector) -> ();
    }
}

fn lexicon_key(doc: &Value) -> Result<(&str, Option<&str>)> {
    let term = doc
        .get("term")
        .and_then(Value::as_str)
        .context("lexicon term must be a string")?;
    let scope = match doc.get("scope") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .context("lexicon scope must be a string or null")?,
        ),
    };
    Ok((term, scope))
}
