//! QQ heuristics only nominate; the owner alone confirms agent identities.
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::channels::Envelope;
use crate::mind::judge::{self, Judgement, Verdict};
use crate::social::identity::Who;
use crate::store::{ReviewCategory, Store};

pub const CONFIRMED: &str = "confirmed_agents";
const REJECTED: &str = "rejected_agents";

pub fn list(store: &dyn Store, key: &str) -> Result<Vec<String>> {
    Ok(store.kv_get(key)?.map(serde_json::from_value).transpose()?.unwrap_or_default())
}

pub fn settle(store: &dyn Store, sender: &str, approve: bool) -> Result<()> {
    let key = if approve { CONFIRMED } else { REJECTED };
    let mut ids = list(store, key)?;
    if !ids.iter().any(|id| id == sender) { ids.push(sender.into()); }
    store.kv_set(key, json!(ids))
}

#[derive(Default, Serialize, Deserialize)]
struct Samples {
    // Only a bounded ten-minute window, with bounded text fingerprints (not message bodies).
    recent: Vec<(DateTime<Utc>, u64)>,
}

pub fn observe(store: &dyn Store, env: &Envelope, who: &Who, now: DateTime<Utc>) -> Result<()> {
    if env.platform != "qq" || !env.is_group || who.is_owner || who.is_agent { return Ok(()) }
    let sender = env.sender_key();
    if list(store, REJECTED)?.contains(&sender) { return Ok(()) }
    let proposed = format!("qq_bot_proposed:{sender}");
    if store.kv_get(&proposed)?.is_some() { return Ok(()) }
    let key = format!("qq_bot_samples:{sender}");
    let mut samples: Samples = store.kv_get(&key)?.map(serde_json::from_value).transpose()?.unwrap_or_default();
    samples.recent.retain(|(t, _)| (now - *t).num_seconds() < 600 && *t <= now);
    // Stable FNV-1a, ignoring whitespace and decimal digits to catch simple templates.
    let mut fingerprint = 0xcbf29ce484222325u64;
    let mut length = 0;
    for c in env.text.chars().filter(|c| !c.is_whitespace() && !c.is_ascii_digit()) {
        length += 1;
        fingerprint = (fingerprint ^ c as u64).wrapping_mul(0x100000001b3);
    }
    samples.recent.push((now, fingerprint));
    if samples.recent.len() > 64 { samples.recent.remove(0); }
    let repeated = if length >= 12 { samples.recent.iter().filter(|(_, f)| *f == fingerprint).count() } else { 0 };
    let rate = samples.recent.iter().filter(|(t, _)| (now - *t).num_seconds() < 60).count();
    let score = usize::from(repeated >= 6) + usize::from(rate >= 20);
    tracing::debug!(%sender, repeated, rate, score, "QQ agent candidate score");
    store.kv_set(&key, json!(samples))?;
    if score == 0 { return Ok(()) }
    // Only scan reviews on threshold crossing. This closes the save-review/save-marker crash gap.
    if store.reviews()?.iter().any(|r| r.payload.get("agent_candidate").and_then(|v| v.as_str()) == Some(&sender)) {
        return store.kv_set(&proposed, json!(true));
    }
    let evidence = format!("QQ {sender}：10 分钟内同模板 {repeated} 条，最近 60 秒 {rate} 条（最多保留 64 条样本）。这只是启发式线索，不是机器人身份的证明；请确认是否按 agent 限流。");
    let judgement = Judgement { verdict: Verdict::Owner, reasons: evidence.clone(), fixes: String::new(), urgent: false };
    judge::file(store, ReviewCategory::Act, &format!("确认 QQ agent：{sender}"), &evidence,
        &judgement, true, json!({"agent_candidate": sender}))?;
    store.kv_set(&proposed, json!(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::jsonl::JsonlStore;

    #[test]
    fn rolling_counters_survive_restart_expire_and_propose_only_once() {
        let dir = std::env::temp_dir().join(format!("agli-bot-flags-{}", std::process::id()));
        let store = JsonlStore::open(&dir).unwrap();
        let who = Who { person_id: "qq:7".into(), is_owner: false, is_agent: false };
        let mut env = Envelope { attachments: Vec::new(), platform: "qq", chat_id: "group:1".into(), message_id: None,
            sender_id: "7".into(), sender_name: "sender".into(), sender_is_bot: false, is_group: true,
            mentions_me: false, text: "模板自动播报当前状态正常序号123".into(), arrived: None };
        let now = Utc::now();
        for i in 0..5 { observe(&store, &env, &who, now + chrono::Duration::seconds(i * 10)).unwrap(); }
        assert!(store.reviews().unwrap().is_empty());
        drop(store);
        let store = JsonlStore::open(&dir).unwrap();
        for i in 5..9 { observe(&store, &env, &who, now + chrono::Duration::seconds(i * 10)).unwrap(); }
        assert_eq!(store.reviews().unwrap().len(), 1);
        // A separate sender's old samples cannot cross the threshold a day later.
        env.sender_id = "8".into();
        for i in 0..5 { observe(&store, &env, &who, now + chrono::Duration::seconds(i)).unwrap(); }
        observe(&store, &env, &who, now + chrono::Duration::days(1)).unwrap();
        assert_eq!(store.reviews().unwrap().len(), 1);
        // High rate alone is also a nomination; short messages do not count as templates.
        env.sender_id = "9".into();
        env.text = "hi".into();
        for i in 0..20 { observe(&store, &env, &who, now + chrono::Duration::seconds(i)).unwrap(); }
        assert_eq!(store.reviews().unwrap().len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
