//! QQ heuristics only nominate; the owner alone confirms agent identities.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use crate::channels::Envelope;
use crate::mind::judge::{self, Judgement, Verdict};
use crate::social::identity::Who;
use crate::store::{ReviewCategory, Store};

const CONFIRMED: &str = "confirmed_agents";
const REJECTED: &str = "rejected_agents";
const PROPOSED: &str = "qq_bot_proposed:";
const MAX_SENDERS: usize = 512;
const MAX_SAMPLES: usize = 64;
const WINDOW: Duration = Duration::minutes(10);
const RATE_WINDOW: Duration = Duration::minutes(1);

pub struct BotFlags {
    confirmed: HashSet<String>,
    rejected: HashSet<String>,
    proposed: HashSet<String>,
    samples: HashMap<String, VecDeque<(DateTime<Utc>, u64)>>,
}

impl BotFlags {
    pub async fn load(store: &Store) -> Result<Self> {
        let confirmed = store.kv_get(CONFIRMED).await?.map(serde_json::from_value).transpose()?.unwrap_or_default();
        let rejected = store.kv_get(REJECTED).await?.map(serde_json::from_value).transpose()?.unwrap_or_default();
        let proposed = store.kv_keys_with_prefix(PROPOSED).await?.into_iter()
            .filter_map(|key| key.strip_prefix(PROPOSED).map(str::to_owned))
            .collect();
        Ok(Self { confirmed, rejected, proposed, samples: HashMap::new() })
    }

    pub fn confirmed(&self) -> &HashSet<String> {
        &self.confirmed
    }

    pub async fn settle(&mut self, store: &Store, sender: &str, approve: bool) -> Result<()> {
        let (key, ids) = if approve { (CONFIRMED, &mut self.confirmed) } else { (REJECTED, &mut self.rejected) };
        if !ids.contains(sender) {
            let mut updated: Vec<&str> = ids.iter().map(String::as_str).chain([sender]).collect();
            updated.sort_unstable();
            store.kv_set(key, json!(updated)).await?;
            ids.insert(sender.into());
        }
        self.samples.remove(sender);
        Ok(())
    }

    pub async fn observe(&mut self, store: &Store, env: &Envelope, who: &Who, now: DateTime<Utc>) -> Result<()> {
        if env.platform != "qq" || !env.is_group || who.is_owner || who.is_agent || env.notice {
            return Ok(());
        }
        let sender = env.sender_key();
        if self.confirmed.contains(&sender) || self.rejected.contains(&sender) || self.proposed.contains(&sender) {
            return Ok(());
        }
        if self.samples.len() == MAX_SENDERS && !self.samples.contains_key(&sender)
            && let Some(oldest) = self.samples.iter().min_by_key(|(_, samples)| samples.back().map(|(t, _)| *t))
                .map(|(key, _)| key.clone())
        {
            self.samples.remove(&oldest);
        }
        // Stable FNV-1a, ignoring whitespace and decimal digits to catch simple templates.
        let mut fingerprint = 0xcbf29ce484222325u64;
        let mut length = 0;
        for c in env.text.chars().filter(|c| !c.is_whitespace() && !c.is_ascii_digit()) {
            length += 1;
            fingerprint = (fingerprint ^ c as u64).wrapping_mul(0x100000001b3);
        }
        let samples = self.samples.entry(sender.clone()).or_default();
        samples.retain(|(t, _)| *t <= now && now - *t < WINDOW);
        if samples.len() == MAX_SAMPLES {
            samples.pop_front();
        }
        samples.push_back((now, fingerprint));
        let repeated = if length >= 12 { samples.iter().filter(|(_, f)| *f == fingerprint).count() } else { 0 };
        let rate = samples.iter().filter(|(t, _)| now - *t < RATE_WINDOW).count();
        let score = usize::from(repeated >= 6) + usize::from(rate >= 20);
        tracing::debug!(%sender, repeated, rate, score, "QQ agent candidate score");
        if score == 0 {
            return Ok(());
        }

        // Only scan reviews on threshold crossing. This closes the save-review/save-marker crash gap.
        if !store.reviews().await?.iter().any(|r| r.payload["agent_candidate"].as_str() == Some(&sender)) {
            let evidence = format!("QQ {sender}：10 分钟内同模板 {repeated} 条，最近 60 秒 {rate} 条（最多保留 {MAX_SAMPLES} 条样本）。这只是启发式线索，不是机器人身份的证明；请确认是否按 agent 限流。");
            let judgement = Judgement { verdict: Verdict::Owner, reasons: evidence.clone(), fixes: String::new(), urgent: false };
            judge::file(store, ReviewCategory::Act, &format!("确认 QQ agent：{sender}"), &evidence,
                &judgement, true, json!({"agent_candidate": sender})).await?;
        }
        store.kv_set(&format!("{PROPOSED}{sender}"), json!(true)).await?;
        self.samples.remove(&sender);
        self.proposed.insert(sender);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn sample_count(&self, sender: &str) -> usize {
        self.samples.get(sender).map_or(0, VecDeque::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rolling_samples_expire_and_propose_only_once_without_ordinary_kv_writes() {
        let store = Store::memory();
        let mut flags = BotFlags::load(&store).await.unwrap();
        let who = Who { person_id: "qq:7".into(), is_owner: false, is_agent: false };
        let mut env = Envelope { authored_text: String::new(), notice: false, attachments: Vec::new(), platform: "qq", chat_id: "group:1".into(), message_id: None,
            sender_id: "7".into(), sender_name: "sender".into(), sender_is_bot: false, is_group: true,
            mentions_me: false, text: "模板自动播报当前状态正常序号123".into(), arrived: None };
        let now = Utc::now();
        for i in 0..5 {
            flags.observe(&store, &env, &who, now + Duration::seconds(i)).await.unwrap();
        }
        assert!(store.reviews().await.unwrap().is_empty());
        assert!(store.kv_keys_with_prefix("").await.unwrap().is_empty());
        assert!(BotFlags::load(&store).await.unwrap().samples.is_empty());

        let later = now + Duration::seconds(4) + WINDOW;
        flags.observe(&store, &env, &who, later).await.unwrap();
        assert_eq!(flags.sample_count("qq:7"), 1);
        assert!(store.reviews().await.unwrap().is_empty());
        assert!(store.kv_keys_with_prefix("").await.unwrap().is_empty());
        for i in 1..=5 {
            flags.observe(&store, &env, &who, later + Duration::seconds(i)).await.unwrap();
        }
        assert_eq!(store.reviews().await.unwrap().len(), 1);
        assert_eq!(store.kv_get("qq_bot_proposed:qq:7").await.unwrap(), Some(json!(true)));
        flags.observe(&store, &env, &who, later).await.unwrap();
        assert_eq!(flags.sample_count("qq:7"), 0);

        let mut restarted = BotFlags::load(&store).await.unwrap();
        for i in 0..6 {
            restarted.observe(&store, &env, &who, later + Duration::seconds(i)).await.unwrap();
        }
        assert!(restarted.samples.is_empty());
        assert_eq!(store.reviews().await.unwrap().len(), 1);

        // High rate alone also nominates; short messages cannot count as templates.
        env.sender_id = "9".into();
        env.text = "hi".into();
        for i in 0..20 {
            restarted.observe(&store, &env, &who, now + Duration::seconds(i)).await.unwrap();
        }
        assert_eq!(store.reviews().await.unwrap().len(), 2);
    }
}
