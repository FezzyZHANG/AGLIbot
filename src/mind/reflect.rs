//! Reflection (docs/DESIGN.md 2c, 2d): distil messages into episodes, facts about people, and
//! de-identified lessons; at night, also write the diary.
//!
//! The model proposes content; the code assigns namespace and scope from the source chat. Raw
//! records keep their scope. Lessons are generalised and stored with scope `self` automatically
//! (usable only in solo thinking); anything that would widen visibility goes through judge and
//! then waits for the owner.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::json;
use tracing::info;

use super::models::Models;
use super::{commitments, judge};
use crate::ns::{Namespace, ProcessingPolicy, Scope};
use crate::social::groups;
use crate::social::people::People;
use crate::store::{Episode, Fact, Lesson, ReviewCategory, Store};

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
struct RawLesson {
    text: String,
    #[serde(default)]
    shareable: bool,
}

#[derive(Deserialize)]
struct Extracted {
    #[serde(flatten)]
    social: groups::Extracted,
    #[serde(flatten)]
    agenda: commitments::Extracted,
    #[serde(default)]
    episodes: Vec<RawEpisode>,
    #[serde(default)]
    facts: Vec<RawFact>,
    #[serde(default)]
    lessons: Vec<RawLesson>,
}

#[derive(Debug, Default)]
pub struct ExtractOutcome {
    pub messages: usize,
    pub episodes: usize,
    pub facts: usize,
    pub lessons: usize,
    pub widen_requests: usize,
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
    let Some(last) = msgs.last() else { return Ok(ExtractOutcome::default()) };
    let scope = last.scope.clone();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut transcript = String::new();
    for m in &msgs {
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
         3. lessons：你从中得到的一般性经验，必须去掉一切可识别信息（名字、时间、关系、独特事件组合都不行）。\
         shareable 表示你认为这条经验可以对其他人说；拿不准就写 false。\n\n\
         只输出一个 JSON 对象：{{\"episodes\": [{{\"summary\": \"\", \"participants\": [], \"emotion\": \"\", \"salience\": 0.5}}], \
         \"facts\": [{{\"person\": \"\", \"text\": \"\"}}], \"lessons\": [{{\"text\": \"\", \"shareable\": false}}]}}，没有就给空数组。\n\n{transcript}"
    );
    let (agenda, policy) = commitments::extraction_context(store, ns, &msgs)?;
    // Existing agenda items are also model input; a restricted one must not taint a group card.
    let social_allowed = matches!(ns, Namespace::Group(_)) && commitments::load(store)?.iter()
        .filter(|c| c.ns == *ns && c.status == commitments::Status::Open)
        .all(|c| c.scope == scope);
    let social = if social_allowed { groups::extraction_prompt(ns) } else { "" };
    let raw: Extracted = models.ask_json("extract", &policy, system, &format!("{user}\n{agenda}{social}")).await?;

    let now = Utc::now();
    commitments::apply(store, ns, &msgs, raw.agenda, now)?;
    if social_allowed {
        groups::apply(store, ns, &msgs, raw.social)?;
    }
    let mut out = ExtractOutcome { messages: msgs.len(), ..Default::default() };
    let mut episode_ids = Vec::new();
    for (i, e) in raw.episodes.into_iter().enumerate() {
        if e.summary.trim().is_empty() {
            continue;
        }
        let id = format!("{}-{}-{i}", ns, last.ts.timestamp_millis());
        let participants = e.participants.into_iter().filter(|p| ids.contains(p)).collect();
        store.append_episode(&Episode {
            id: id.clone(),
            ts: last.ts,
            participants,
            summary: e.summary,
            emotion: e.emotion,
            salience: e.salience.clamp(0.0, 1.0),
            ns: ns.clone(),
            scope: scope.clone(),
        })?;
        episode_ids.push(id);
        out.episodes += 1;
    }
    for f in raw.facts {
        if f.person == "me" || !ids.contains(&f.person) || f.text.trim().is_empty() {
            continue;
        }
        people.add_fact(store, &f.person, Fact { text: f.text, ns: ns.clone(), scope: scope.clone(), ts: now })?;
        out.facts += 1;
    }
    let mut widen = Vec::new();
    for (i, l) in raw.lessons.into_iter().enumerate() {
        if l.text.trim().is_empty() {
            continue;
        }
        let lesson = Lesson {
            id: format!("lesson-{}-{i}", now.timestamp_millis()),
            ts: now,
            text: l.text,
            scope: Scope::SelfOnly,
            sources: episode_ids.clone(),
        };
        store.append_lesson(&lesson)?;
        out.lessons += 1;
        if l.shareable {
            widen.push(lesson);
        }
    }
    // Mark the batch done before the slower reviews, so a failure there never re-extracts it.
    store.kv_set(&watermark_key(ns), json!(last.ts.to_rfc3339()))?;
    for lesson in widen {
        // Widening scope always goes through review (2d). A pass widens it now, as a separate
        // record pointing at its source; anything else stays self-only or waits for the owner.
        let evidence = format!("来源：{} 条经历（原可见范围 {:?}）。只看这条经验本身是否还能认出人。", episode_ids.len(), scope);
        let j = judge::review(models, ReviewCategory::Widen, &lesson.text, &evidence).await?;
        judge::file(
            store,
            ReviewCategory::Widen,
            "把一条经验改为可公开",
            &lesson.text,
            &j,
            false,
            json!({"lesson_id": lesson.id, "scope": "public"}),
        )?;
        if j.verdict == judge::Verdict::Pass {
            store.append_lesson(&widened(&lesson))?;
        }
        out.widen_requests += 1;
    }
    info!(
        "reflect: {ns}: {} messages → {} episodes, {} facts, {} lessons",
        out.messages, out.episodes, out.facts, out.lessons
    );
    Ok(out)
}

/// The public copy of a reviewed lesson; the self-scoped original stays as it was.
pub fn widened(l: &Lesson) -> Lesson {
    Lesson { id: format!("{}-public", l.id), ts: Utc::now(), text: l.text.clone(), scope: Scope::Public, sources: vec![l.id.clone()] }
}

/// The day a moment belongs to in his diary: a day runs from 06:00 to 06:00, so staying up
/// past midnight still counts as the same day.
pub fn diary_day(t: DateTime<Local>) -> NaiveDate {
    (t - chrono::Duration::hours(6)).date_naive()
}

/// Notes for today's diary that are not messages (flow sessions, finished tasks).
pub fn note_today(store: &dyn Store, line: &str) -> Result<()> {
    let key = format!("daylog:{}", diary_day(Local::now()));
    let mut lines: Vec<String> =
        store.kv_get(&key)?.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    lines.push(line.to_string());
    store.kv_set(&key, json!(lines))
}

/// Write `mind/life/journal/<date>.md` from the day's episodes and notes. Solo thinking: every
/// memory is available; self-scoped lessons only if a local model can process them.
pub async fn write_journal(models: &Models, store: &dyn Store, l1: &str, journal_dir: PathBuf, date: NaiveDate) -> Result<Option<PathBuf>> {
    let episodes: Vec<Episode> =
        store.episodes()?.into_iter().filter(|e| diary_day(e.ts.with_timezone(&Local)) == date).collect();
    let notes: Vec<String> =
        store.kv_get(&format!("daylog:{date}"))?.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    if episodes.is_empty() && notes.is_empty() {
        return Ok(None);
    }
    let mut material = String::new();
    for e in &episodes {
        material.push_str(&format!("- {}（{}）\n", e.summary, e.emotion));
    }
    for n in &notes {
        material.push_str(&format!("- {n}\n"));
    }
    let (policy, lessons) = if models.has_local() {
        let lessons: Vec<String> = store.lessons()?.into_iter().map(|l| l.text).collect();
        (ProcessingPolicy::LocalOnly, lessons)
    } else {
        (ProcessingPolicy::Any, Vec::new())
    };
    if !lessons.is_empty() {
        material.push_str("\n你以前总结过的经验：\n");
        for l in lessons.iter().rev().take(10) {
            material.push_str(&format!("- {l}\n"));
        }
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

    #[test]
    fn after_midnight_still_counts_as_the_same_day() {
        let late = Local.with_ymd_and_hms(2026, 11, 1, 0, 58, 0).unwrap();
        let morning = Local.with_ymd_and_hms(2026, 11, 1, 7, 0, 0).unwrap();
        assert_eq!(diary_day(late), NaiveDate::from_ymd_opt(2026, 10, 31).unwrap());
        assert_eq!(diary_day(morning), NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
    }
}
