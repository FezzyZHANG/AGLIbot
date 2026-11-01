//! Private agenda. Store KV is authoritative; Markdown is only a human-readable view.

use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ns::{Namespace, ProcessingPolicy, Scope};
use crate::store::{MessageRecord, Store};

const KEY: &str = "agenda:commitments";

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Open,
    Done,
    Dropped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commitment {
    pub id: String,
    /// A person_id, or "me" for an intention with no recipient.
    pub who: String,
    pub what: String,
    pub due: Option<NaiveDate>,
    pub ns: Namespace,
    pub scope: Scope,
    pub created: DateTime<Utc>,
    pub status: Status,
    pub evidence: DateTime<Utc>,
    pub completed_evidence: Option<DateTime<Utc>>,
}

#[derive(Default, Deserialize)]
pub struct Extracted {
    #[serde(default)]
    commitments: Vec<RawCommitment>,
    #[serde(default)]
    completed_commitments: Vec<Completion>,
}

#[derive(Deserialize)]
struct RawCommitment {
    who: String,
    what: String,
    #[serde(default)]
    due: Option<NaiveDate>,
    evidence: DateTime<Utc>,
}

#[derive(Deserialize)]
struct Completion {
    id: String,
    evidence: DateTime<Utc>,
}

pub fn load(store: &dyn Store) -> Result<Vec<Commitment>> {
    match store.kv_get(KEY)? {
        Some(value) => Ok(serde_json::from_value(value)?),
        None => Ok(Vec::new()),
    }
}

/// Only this namespace's open items go back into extraction; they are never chat L2 material.
pub fn extraction_context(store: &dyn Store, ns: &Namespace, msgs: &[MessageRecord]) -> Result<(String, ProcessingPolicy)> {
    let open: Vec<_> = load(store)?.into_iter().filter(|c| c.ns == *ns && c.status == Status::Open).collect();
    let local = msgs.iter().any(|m| m.scope == Scope::SelfOnly) || open.iter().any(|c| c.scope == Scope::SelfOnly);
    let policy = if local { ProcessingPolicy::LocalOnly } else { ProcessingPolicy::Any };
    let prompt = format!(
        "\n另在同一个 JSON 对象里增加 commitments 和 completed_commitments 两个数组，没有就给空数组。\n\
         commitments: [{{\"who\":\"person_id 或 me\",\"what\":\"具体要做的事\",\"due\":\"YYYY-MM-DD 或 null\",\"evidence\":\"原消息的完整时间戳\"}}]。\n\
         只提取我（me）的发出消息中明确答应别人要做的事或明确意图，不把别人的承诺、请求、假设算成我的承诺。\
         who 用收件人的 person_id；没有对象的自身意图用 me。日期按原消息的当地日期理解，不明确就用 null。\
         同一件事不要重复提取；下面已有的条目不要重新创建。\n\
         completed_commitments: [{{\"id\":\"下列已有条目的 id\",\"evidence\":\"证明已履行的原消息完整时间戳\"}}]。\
         只有消息明确表明已履行才标完成，计划或再次保证不算完成。\n已有未完成条目：\n{}",
        serde_json::to_string(&open)?
    );
    Ok((prompt, policy))
}

pub fn apply(store: &dyn Store, ns: &Namespace, msgs: &[MessageRecord], raw: Extracted, now: DateTime<Utc>) -> Result<()> {
    let mut items = load(store)?;
    // Completion can only refer to an existing open item, never a newly invented id.
    for done in raw.completed_commitments {
        let Some(evidence) = msgs.iter().find(|m| m.ns == *ns && m.ts == done.evidence) else { continue };
        if let Some(c) = items.iter_mut().find(|c| c.id == done.id && c.ns == *ns && c.status == Status::Open && c.evidence <= evidence.ts) {
            c.status = Status::Done;
            c.completed_evidence = Some(evidence.ts);
        }
    }
    for c in raw.commitments {
        let Some(source) = msgs.iter().find(|m| m.ns == *ns && m.outgoing && m.ts == c.evidence) else { continue };
        let known = c.who == "me"
            || matches!(ns, Namespace::Person(id) if *id == c.who)
            || msgs.iter().any(|m| m.ns == *ns && !m.outgoing && m.sender == c.who);
        let what = c.what.trim();
        if !known || what.is_empty() {
            continue;
        }
        // Stable across retries; the model cannot select a storage key or overwrite another item.
        let key = serde_json::to_vec(&(ns, source.ts, &c.who, what))?;
        let id = format!("commitment-{:x}", Sha256::digest(key));
        if items.iter().any(|old| old.id == id || (old.ns == *ns && old.status == Status::Open && old.who == c.who && old.what == what)) {
            continue;
        }
        items.push(Commitment {
            id, who: c.who, what: what.into(), due: c.due, ns: source.ns.clone(), scope: source.scope.clone(),
            created: now, status: Status::Open, evidence: source.ts, completed_evidence: None,
        });
    }
    store.kv_set(KEY, serde_json::to_value(items)?)
}

/// Date-only deadlines use the caller's local day. Undated intentions stay in the agenda view.
pub fn due_soon(store: &dyn Store, today: NaiveDate, has_local: bool) -> Result<Vec<Commitment>> {
    let mut items: Vec<_> = load(store)?.into_iter().filter(|c| {
        c.status == Status::Open && c.due.is_some_and(|d| d <= today + Duration::days(3))
            && (has_local || c.scope != Scope::SelfOnly)
    }).collect();
    items.sort_by(|a, b| (a.due, a.created, &a.id).cmp(&(b.due, b.created, &b.id)));
    items.truncate(5);
    Ok(items)
}

fn one_line(text: &str, limit: usize) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(limit).collect()
}

pub fn summary(items: &[Commitment]) -> String {
    items.iter().take(5).map(|c| format!(
        "- 对 {}：{}（期限 {}；来源 {}）",
        one_line(&c.who, 80), one_line(&c.what, 160),
        c.due.map(|d| d.to_string()).unwrap_or_else(|| "未定".into()), one_line(&c.ns.to_string(), 100),
    )).collect::<Vec<_>>().join("\n")
}

pub fn render_view(store: &dyn Store, mind_dir: &Path) -> Result<()> {
    let mut text = "# 承诺与意图\n\n私人视图，由 Store 自动生成；修改此文件不会改变记录。\n".to_string();
    for c in load(store)? {
        text.push_str(&format!(
            "\n## {}\n- 状态：{}\n- 对谁：{}\n- 事项：{}\n- 期限：{}\n- 来源：{}\n- 可见范围：{}\n- 创建：{}\n- 消息证据：{}\n",
            c.id, serde_json::to_value(c.status)?.as_str().unwrap_or_default(), one_line(&c.who, usize::MAX),
            one_line(&c.what, usize::MAX), c.due.map(|d| d.to_string()).unwrap_or_else(|| "未定".into()),
            c.ns, serde_json::to_string(&c.scope)?, c.created.to_rfc3339(), c.evidence.to_rfc3339(),
        ));
        if let Some(ts) = c.completed_evidence {
            text.push_str(&format!("- 完成证据：{}\n", ts.to_rfc3339()));
        }
    }
    let dir = mind_dir.join("agenda");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("commitments.md"), text)?;
    Ok(())
}
