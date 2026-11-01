//! Action selection is left to the model (docs/DESIGN.md 2c). This module only assembles the
//! situation, asks a cheap model, and validates the answer; permissions, budget, quiet hours and
//! rate limits are enforced by the runtime, not left to the model's judgement.

use anyhow::Result;
use chrono::{DateTime, Duration, Local, NaiveTime, TimeZone, Utc};
use serde::Deserialize;

use super::models::Models;
use crate::ns::ProcessingPolicy;

#[derive(Debug, Clone, Deserialize)]
pub struct MoodShift {
    #[serde(default)]
    pub pleasure: f64,
    #[serde(default)]
    pub arousal: f64,
    #[serde(default)]
    pub dominance: f64,
}

/// The few checkable fields `decide` may return.
#[derive(Debug, Clone, Deserialize)]
pub struct Decision {
    pub action: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub deferred: Vec<String>,
    /// Minutes from now (`"90"`) or a local clock time (`"21:30"`).
    #[serde(default)]
    pub revisit_when: Option<String>,
    /// Optional appraisal of how the situation makes him feel.
    #[serde(default)]
    pub mood: Option<MoodShift>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Nothing,
    MessageOwner { reason: String },
    Work { task_id: String },
}

pub struct Situation {
    pub text: String,
    /// Task ids he may start now (approved, not running, nothing else running).
    pub startable: Vec<String>,
    /// Whether starting a conversation with the owner is allowed right now.
    pub may_message_owner: bool,
}

pub async fn decide(models: &Models, policy: &ProcessingPolicy, l1: &str, s: &Situation) -> Result<Decision> {
    let mut options = vec!["\"nothing\"：什么都不做，继续过自己的日子".to_string()];
    if s.may_message_owner {
        options.push("\"message\"：主动找主人聊几句（target 写 owner）".into());
    }
    if !s.startable.is_empty() {
        options.push(format!("\"work\"：开始做一件任务（target 写任务编号，可选：{}）", s.startable.join("、")));
    }
    let user = format!(
        "（这不是聊天消息，是你自己在想接下来做什么。）\n\n{}\n\n## 可选的行动\n{}\n\n\
         像一个真实的人那样选：不必每次都做点什么，也可以推迟。\
         只输出一个 JSON 对象：{{\"action\": \"...\", \"target\": \"...或 null\", \"reason\": \"一句话\", \
         \"deferred\": [\"暂时放下的事\"], \"revisit_when\": \"多少分钟后再想，或 HH:MM，或 null\", \
         \"mood\": {{\"pleasure\": 0, \"arousal\": 0, \"dominance\": 0}}}}。mood 是这些情况让你心情变化的幅度（-0.3 到 0.3）。",
        s.text,
        options.iter().map(|o| format!("- {o}")).collect::<Vec<_>>().join("\n")
    );
    models.ask_json("triage", policy, l1, &user).await
}

/// Turn the model's choice into something the runtime is willing to do.
pub fn validate(d: &Decision, s: &Situation) -> Action {
    match d.action.trim() {
        "message" if s.may_message_owner => Action::MessageOwner { reason: d.reason.clone() },
        "work" => match d.target.as_deref().map(str::trim) {
            Some(t) if s.startable.iter().any(|id| id == t) => Action::Work { task_id: t.to_string() },
            _ => Action::Nothing,
        },
        _ => Action::Nothing,
    }
}

pub fn parse_revisit(when: Option<&str>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let w = when?.trim();
    if let Ok(min) = w.parse::<i64>() {
        return (min > 0).then(|| now + Duration::minutes(min.min(24 * 60)));
    }
    let t = NaiveTime::parse_from_str(w, "%H:%M").ok()?;
    let local_now = now.with_timezone(&Local);
    let mut at = Local.from_local_datetime(&local_now.date_naive().and_time(t)).earliest()?;
    if at <= local_now {
        at += Duration::days(1);
    }
    Some(at.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(action: &str, target: Option<&str>) -> Decision {
        Decision {
            action: action.into(),
            target: target.map(String::from),
            reason: "r".into(),
            deferred: vec![],
            revisit_when: None,
            mood: None,
        }
    }

    #[test]
    fn runtime_refuses_what_is_not_allowed() {
        let s = Situation { text: String::new(), startable: vec!["t1".into()], may_message_owner: false };
        assert_eq!(validate(&decision("message", Some("owner")), &s), Action::Nothing);
        assert_eq!(validate(&decision("work", Some("t2")), &s), Action::Nothing);
        assert_eq!(validate(&decision("work", Some("t1")), &s), Action::Work { task_id: "t1".into() });
        assert_eq!(validate(&decision("launch rockets", None), &s), Action::Nothing);
        let s = Situation { may_message_owner: true, ..s };
        assert!(matches!(validate(&decision("message", Some("owner")), &s), Action::MessageOwner { .. }));
    }

    #[test]
    fn revisit_accepts_minutes_and_clock_times() {
        let now = Utc::now();
        assert_eq!(parse_revisit(Some("90"), now), Some(now + Duration::minutes(90)));
        let at = parse_revisit(Some("21:30"), now).unwrap();
        assert!(at > now && at - now <= Duration::days(1));
        assert_eq!(parse_revisit(Some("soon"), now), None);
        assert_eq!(parse_revisit(None, now), None);
    }
}
