//! Storage behind a synchronous trait: MongoDB when configured, JSONL otherwise (DESIGN 2b).

pub mod jsonl;
pub mod mongo;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ns::{Namespace, Scope};

pub fn open(cfg: &crate::config::MongoCfg, data_dir: &std::path::Path) -> Result<std::sync::Arc<dyn Store>> {
    match &cfg.url {
        Some(url) => Ok(std::sync::Arc::new(mongo::MongoStore::open(url, &cfg.database)?)),
        None => Ok(std::sync::Arc::new(jsonl::JsonlStore::open(data_dir)?)),
    }
}

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
    pub facts: Vec<Fact>,
    pub last_seen: DateTime<Utc>,
}

/// A generalised, de-identified lesson from reflection. Always created with scope `self`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub id: String,
    pub ts: DateTime<Utc>,
    pub text: String,
    pub scope: Scope,
    /// Episode ids it was derived from: references only, never the restricted text.
    pub sources: Vec<String>,
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
}

pub trait Store: Send + Sync {
    fn append_message(&self, m: &MessageRecord) -> Result<()>;
    /// The last `n` messages in a namespace, oldest first.
    fn recent_in_ns(&self, ns: &Namespace, n: usize) -> Result<Vec<MessageRecord>>;
    /// Messages in a namespace strictly after `after`, oldest first.
    fn messages_after(&self, ns: &Namespace, after: DateTime<Utc>) -> Result<Vec<MessageRecord>>;
    /// Every namespace that has messages after `after`.
    fn active_namespaces(&self, after: DateTime<Utc>) -> Result<Vec<Namespace>>;

    fn append_model_call(&self, c: &ModelCallRecord) -> Result<()>;
    fn spent_since(&self, since: DateTime<Utc>) -> Result<f64>;

    fn append_episode(&self, e: &Episode) -> Result<()>;
    fn episodes(&self) -> Result<Vec<Episode>>;

    fn persons(&self) -> Result<BTreeMap<String, Person>>;
    fn save_person(&self, p: &Person) -> Result<()>;

    fn append_lesson(&self, l: &Lesson) -> Result<()>;
    fn lessons(&self) -> Result<Vec<Lesson>>;

    fn append_audit(&self, a: &AuditRecord) -> Result<()>;

    fn reviews(&self) -> Result<Vec<ReviewItem>>;
    /// Insert or replace by id. Callers allocate ids via `next_review_id`.
    fn save_review(&self, r: &ReviewItem) -> Result<()>;
    fn next_review_id(&self) -> Result<u64>;

    /// Small key/value facts: extraction watermarks, owner's last chat, freeze flag.
    fn kv_get(&self, key: &str) -> Result<Option<Value>>;
    fn kv_set(&self, key: &str, value: Value) -> Result<()>;
}
