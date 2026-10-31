//! Storage behind a trait: JSONL now, MongoDB later (docs/DESIGN.md 2b).

pub mod jsonl;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ns::{Namespace, Scope};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub ts: DateTime<Utc>,
    pub platform: String,
    pub chat_id: String,
    /// person_id of the sender, or "me" for his own messages.
    pub sender: String,
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

pub trait Store: Send + Sync {
    fn append_message(&self, m: &MessageRecord) -> Result<()>;
    /// The last `n` messages of a chat, oldest first.
    fn recent_messages(&self, chat_id: &str, n: usize) -> Result<Vec<MessageRecord>>;
    fn append_model_call(&self, c: &ModelCallRecord) -> Result<()>;
    fn spent_since(&self, since: DateTime<Utc>) -> Result<f64>;
}
