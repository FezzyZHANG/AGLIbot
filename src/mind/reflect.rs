//! Distil messages into episodes and facts, then use the day's episodes to write the diary.
//! Namespace and scope come from the source messages.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

use super::models::{Models, SpendLimit};
use crate::config::SocialCfg;
use crate::ns::{Namespace, ProcessingPolicy, Scope};
use crate::social::people::People;
use crate::store::{Episode, Fact, MessageRecord, Store};

#[derive(Deserialize)]
struct RawEpisode {
    summary: String,
    #[serde(default)]
    participants: Vec<String>,
    #[serde(default)]
    emotion: String,
    #[serde(default)]
    salience: f64,
}

#[derive(Deserialize)]
struct RawFact {
    person: String,
    text: String,
}

#[derive(Deserialize)]
struct Extracted {
    #[serde(default)]
    episodes: Vec<RawEpisode>,
    #[serde(default)]
    facts: Vec<RawFact>,
}

#[derive(Debug, Default)]
pub struct ExtractOutcome {
    pub messages: usize,
    pub episodes: usize,
    pub facts: usize,
}

fn watermark_key(ns: &Namespace) -> String {
    format!("extract:{ns}")
}

pub fn watermark(store: &dyn Store, ns: &Namespace) -> Result<DateTime<Utc>> {
    Ok(store
        .kv_get(&watermark_key(ns))?
        .and_then(|v| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(DateTime::<Utc>::MIN_UTC))
}

/// Distil everything in `ns` since the last extraction.
pub async fn extract_ns(models: &Models, store: &dyn Store, people: &mut People, ns: &Namespace) -> Result<ExtractOutcome> {
    let since = watermark(store, ns)?;
    let msgs = store.messages_after(ns, since)?;
    extract_messages(models, store, people, ns, &msgs, None, None).await
}

async fn extract_messages(
    models: &Models, store: &dyn Store, people: &mut People, ns: &Namespace,
    msgs: &[MessageRecord], through: Option<DateTime<Utc>>, limit: Option<SpendLimit>,
) -> Result<ExtractOutcome> {
    let Some(last) = msgs.last() else { return Ok(ExtractOutcome::default()) };
    let scope = if msgs.iter().any(|m| m.scope == Scope::SelfOnly) {
        Scope::SelfOnly
    } else {
        last.scope.clone()
    };
    let policy = ProcessingPolicy::default_for(&scope);
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut transcript = String::new();
    for m in msgs {
        let (id, name) = if m.outgoing { ("me".to_string(), "我".to_string()) } else { (m.sender.clone(), people.name_of(&m.sender)) };
        transcript.push_str(&format!(
            "[{}] {name}（{id}）：{}\n",
            m.ts.with_timezone(&Local).to_rfc3339(),
            m.text
        ));
        ids.insert(id);
    }
    let system = "你在整理自己的记忆。只写聊天记录里确实出现的内容，不推测，不编造。";
    let user = format!(
        "下面是一段聊天记录（{ns}）。请提炼：\n\
         1. episodes：发生了什么，一件事一条，写成你自己的回忆口吻；participants 用括号里的 id；salience 是重要程度 0-1。\n\
         2. facts：关于某个人的稳定事实（偏好、近况、经历），person 用括号里的 id，不要写关于“我”的。\n\
         只输出一个 JSON 对象：{{\"episodes\": [{{\"summary\": \"\", \"participants\": [], \"emotion\": \"\", \"salience\": 0.5}}], \
         \"facts\": [{{\"person\": \"\", \"text\": \"\"}}]}}，没有就给空数组。\n\n{transcript}"
    );
    let raw: Extracted = models.ask_json_with_limit("extract", &policy, system, &user, limit).await?;

    let now = Utc::now();
    let mut out = ExtractOutcome { messages: msgs.len(), ..Default::default() };
    for (i, e) in raw.episodes.into_iter().enumerate() {
        if e.summary.trim().is_empty() {
            continue;
        }
        let id = format!("{}-{}-{i}", ns, last.ts.timestamp_millis());
        let participants = e.participants.into_iter().filter(|p| ids.contains(p)).collect();
        let episode = Episode {
            id,
            ts: last.ts,
            participants,
            summary: e.summary,
            emotion: e.emotion,
            salience: e.salience.clamp(0.0, 1.0),
            ns: ns.clone(),
            scope: scope.clone(),
            sources: vec![],
        };
        store.append_episode(&episode)?;
        out.episodes += 1;
    }
    for f in raw.facts {
        if f.person == "me" || !ids.contains(&f.person) || f.text.trim().is_empty() {
            continue;
        }
        people.add_fact(store, &f.person, Fact { text: f.text, ns: ns.clone(), scope: scope.clone(), ts: now })?;
        out.facts += 1;
    }
    store.kv_set(&watermark_key(ns), json!(through.unwrap_or(last.ts).to_rfc3339()))?;
    info!(
        "reflect: {ns}: {} messages → {} episodes, {} facts",
        out.messages, out.episodes, out.facts
    );
    Ok(out)
}

#[derive(Default, Deserialize, Serialize)]
struct ReadOnlyUsage {
    messages: usize,
    cost_usd: f64,
    pending_since: Option<DateTime<Utc>>,
}

impl ReadOnlyUsage {
    fn settle(&mut self, store: &dyn Store) -> Result<()> {
        if let Some(since) = self.pending_since {
            self.cost_usd += store.spent_since(since)?;
            self.pending_since = None;
        }
        Ok(())
    }
}

/// Evenly spaced messages over the whole window, including both ends when possible.
fn sample(msgs: &[MessageRecord], count: usize) -> Vec<MessageRecord> {
    let count = count.min(msgs.len());
    match count {
        0 => vec![],
        1 => vec![msgs[msgs.len() / 2].clone()],
        _ => (0..count).map(|i| msgs[i * (msgs.len() - 1) / (count - 1)].clone()).collect(),
    }
}

/// Run only after priority chats. Budget state survives retries/restarts in the same diary night.
pub async fn read_only_nightly(
    models: &Models, store: &dyn Store, people: &mut People, social: &SocialCfg,
    date: NaiveDate, after: DateTime<Utc>,
) -> Result<()> {
    let key = format!("read_only_reflect:{date}");
    let mut used: ReadOnlyUsage = store.kv_get(&key)?.map(serde_json::from_value).transpose()?.unwrap_or_default();
    // After an interrupted call, conservatively charge all recorded spend since its start.
    used.settle(store)?;
    store.kv_set(&key, json!(used))?;
    let mut groups = Vec::new();
    for ns in store.active_namespaces(after)? {
        if let Namespace::Group(id) = &ns && social.read_only_group(id) {
            let msgs = store.messages_after(&ns, watermark(store, &ns)?.max(after))?;
            if !msgs.is_empty() { groups.push((ns, msgs)); }
        }
    }
    groups.sort_by(|(a, am), (b, bm)| bm.len().cmp(&am.len()).then_with(|| a.to_string().cmp(&b.to_string())));
    for (ns, msgs) in groups {
        let remaining = social.read_only_reflect.max_messages_per_night.saturating_sub(used.messages);
        let limit = SpendLimit { since: Utc::now(), max_usd: social.read_only_reflect.max_cost_usd - used.cost_usd };
        let count = if limit.exhausted(store)? { 0 } else { remaining.min(msgs.len()) };
        if count < msgs.len() {
            info!(%ns, sampled = count, total = msgs.len(), ratio = count as f64 / msgs.len() as f64,
                cost_usd = used.cost_usd, "read-only reflect: budget sampling");
        }
        if count == 0 { continue }
        let selected = sample(&msgs, count);
        used.messages += count;
        used.pending_since = Some(limit.since);
        store.kv_set(&key, json!(used))?;
        let result = extract_messages(models, store, people, &ns, &selected, msgs.last().map(|m| m.ts), Some(limit)).await;
        used.settle(store)?;
        store.kv_set(&key, json!(used))?;
        if let Err(e) = result {
            warn!(%ns, "read-only reflect failed: {e:#}");
        }
    }
    Ok(())
}

/// The day a moment belongs to in his diary: a day runs from 06:00 to 06:00, so staying up
/// past midnight still counts as the same day.
pub fn diary_day(t: DateTime<Local>) -> NaiveDate {
    (t - chrono::Duration::hours(6)).date_naive()
}

/// Record a note for today's diary, such as a finished task.
pub fn note_today(store: &dyn Store, line: &str) -> Result<()> {
    let key = format!("daylog:{}", diary_day(Local::now()));
    let mut lines: Vec<String> =
        store.kv_get(&key)?.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    lines.push(line.to_string());
    store.kv_set(&key, json!(lines))
}

/// Write the day's journal, keeping self-only episodes on eligible local models.
pub async fn write_journal(models: &Models, store: &dyn Store, l1: &str, journal_dir: PathBuf, date: NaiveDate) -> Result<Option<PathBuf>> {
    let mut episodes: Vec<Episode> =
        store.episodes()?.into_iter().filter(|e| diary_day(e.ts.with_timezone(&Local)) == date).collect();
    let notes: Vec<String> =
        store.kv_get(&format!("daylog:{date}"))?.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    if episodes.is_empty() && notes.is_empty() {
        return Ok(None);
    }
    let mut policy = ProcessingPolicy::Any;
    if episodes.iter().any(|e| e.scope == Scope::SelfOnly) {
        if models.pick("consolidate", &ProcessingPolicy::LocalOnly).is_ok() {
            policy = ProcessingPolicy::LocalOnly;
        } else {
            episodes.retain(|e| e.scope != Scope::SelfOnly);
            if episodes.is_empty() {
                info!(%date, "journal skipped: no local model qualifies to consolidate self-only episodes");
                return Ok(None);
            }
        }
    }
    let mut material = String::new();
    for e in &episodes {
        material.push_str(&format!("- {}（{}）\n", e.summary, e.emotion));
    }
    for n in &notes {
        material.push_str(&format!("- {n}\n"));
    }
    let user = format!(
        "（这不是聊天，是你睡前写日记。）今天是 {date}。根据下面的经历写一篇简短的日记：发生了什么、你怎么想、\
         有没有想修正的看法、明天想做什么。用第一人称，像写给自己看的，不超过 400 字。\n\n{material}"
    );
    let text = models.ask("consolidate", &policy, l1, &user).await?;
    std::fs::create_dir_all(&journal_dir)?;
    let path = journal_dir.join(format!("{date}.md"));
    std::fs::write(&path, format!("# {date}\n\n{}\n", text.trim()))?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> { "2026-11-01T00:00:00Z".parse().unwrap() }

    fn old_episode(id: &str, ns: &str, scope: Scope) -> Episode {
        Episode { id: id.into(), ts: now() - chrono::Duration::days(60), participants: vec![id.into()],
            summary: format!("event {id}"), emotion: String::new(), salience: 0.1,
            ns: ns.parse().unwrap(), scope, sources: vec![] }
    }

    #[test]
    fn decay_is_computed_and_handles_fractional_days_and_future_timestamps() {
        let e = old_episode("a", "self", Scope::SelfOnly);
        assert_eq!(e.effective_salience(e.ts, 30.0), 0.1);
        assert_eq!(e.effective_salience(e.ts + chrono::Duration::days(30), 30.0), 0.05);
        assert_eq!(e.effective_salience(e.ts + chrono::Duration::days(60), 30.0), 0.025);
        assert!((e.effective_salience(e.ts + chrono::Duration::hours(360), 30.0) - 0.1 * 0.5_f64.sqrt()).abs() < 1e-12);
        assert_eq!(e.effective_salience(e.ts - chrono::Duration::days(1), 30.0), 0.1);
        assert_eq!(e.salience, 0.1);
        let mut old_json = serde_json::to_value(&e).unwrap();
        old_json.as_object_mut().unwrap().remove("sources");
        assert!(serde_json::from_value::<Episode>(old_json).unwrap().sources.is_empty());
    }

    #[test]
    fn sampling_is_even_deterministic_and_handles_zero_or_one() {
        let msgs: Vec<_> = (0..10).map(|i| MessageRecord {
            ts: Utc.timestamp_opt(i, 0).unwrap(), platform: "telegram".into(), chat_id: "g".into(),
            sender: "p".into(), sender_name: "p".into(), outgoing: false, text: i.to_string(),
            ns: Namespace::Group("g".into()), scope: Scope::Group { id: "g".into() },
        }).collect();
        let indices = |n| sample(&msgs, n).into_iter().map(|m| m.ts.timestamp()).collect::<Vec<_>>();
        assert!(indices(0).is_empty());
        assert_eq!(indices(1), [5]);
        assert_eq!(indices(4), [0, 3, 6, 9]);
        assert_eq!(indices(4), indices(4));
        assert_eq!(indices(20), (0..10).collect::<Vec<_>>());
        assert!(sample(&[], 1).is_empty());
    }

    #[test]
    fn read_only_social_defaults_and_ignore_precedence() {
        let mut social: SocialCfg = serde_yaml::from_str(include_str!("../../config/social.yaml")).unwrap();
        assert!(social.ignored_groups.is_empty());
        assert_eq!(social.read_only_reflect.max_messages_per_night, 400);
        assert_eq!(social.read_only_reflect.max_cost_usd, 0.10);
        let partial: crate::config::ReadOnlyReflectCfg = serde_yaml::from_str("max_messages_per_night: 10").unwrap();
        assert_eq!(partial.max_cost_usd, 0.10);
        social.groups.push("g".into());
        social.ignored_groups.push("g".into());
        assert!(!social.read_only_group("g"));
        assert!(!social.reflects_normally(&Namespace::Group("g".into())));
    }

    #[tokio::test]
    async fn journal_omits_self_only_episodes_when_no_local_model_qualifies() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("aglibot-journal-test-{}", std::process::id()));
        let store = std::sync::Arc::new(crate::store::jsonl::JsonlStore::open(&dir)?);
        let private = old_episode("private", "self", Scope::SelfOnly);
        let date = diary_day(private.ts.with_timezone(&Local));
        let journal = dir.join("journal");
        store.append_episode(&private)?;
        let models = Models::stub(store.clone(), vec!["journal entry".into()]);

        assert_eq!(write_journal(&models, store.as_ref(), "", journal.clone(), date).await?, None);
        assert!(models.stub_prompts().is_empty());

        store.append_episode(&old_episode("shared", "self", Scope::Public))?;
        let path = write_journal(&models, store.as_ref(), "", journal, date).await?.unwrap();
        let prompts = models.stub_prompts();
        assert_eq!(prompts[0].0, "consolidate");
        assert_eq!(prompts[0].1, ProcessingPolicy::Any);
        assert!(prompts[0].2.contains("event shared"));
        assert!(!prompts[0].2.contains("event private"));
        assert!(std::fs::read_to_string(path)?.contains("journal entry"));
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn after_midnight_still_counts_as_the_same_day() {
        let late = Local.with_ymd_and_hms(2026, 11, 1, 0, 58, 0).unwrap();
        let morning = Local.with_ymd_and_hms(2026, 11, 1, 7, 0, 0).unwrap();
        assert_eq!(diary_day(late), NaiveDate::from_ymd_opt(2026, 10, 31).unwrap());
        assert_eq!(diary_day(morning), NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
    }
}
