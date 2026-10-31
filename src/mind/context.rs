//! Layered prompt assembly, ordered by how fast each layer changes (docs/DESIGN.md 4b).
//!
//! L1 constitution — boundaries, identity, voice; changes only when evolution commits.
//! L2 session frame — who he is talking to and a recap; frozen for one session.
//! L3 transcript — append-only turns, never rewritten.
//! L4 volatile tail — time, mood, energy; appended inside the newest user turn, never in system.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Local, Weekday};
use serde_json::Value;

/// Read a text file with line endings normalised, so L1 bytes are identical across platforms.
pub fn read_norm(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(raw.replace("\r\n", "\n").trim_end().to_string())
}

const L1_SECTIONS: [(&str, &str); 5] = [
    ("边界（不可违反）", "self/boundaries.md"),
    ("我是谁", "self/identity.md"),
    ("人格参数", "self/personality.yaml"),
    ("说话方式", "self/voice.md"),
    ("社交礼仪", "social/etiquette.md"),
];

pub fn build_l1(mind_dir: &Path) -> Result<String> {
    let mut out = String::from(
        "你不是助手，你是一个有自己生活的人。下面是你的身份与原则。\
         每条用户消息开头的 <当前状态> 是你此刻的时间和身心状态，不是对方说的话。",
    );
    for (title, rel) in L1_SECTIONS {
        out.push_str(&format!("\n\n## {title}\n\n{}", read_norm(&mind_dir.join(rel))?));
    }
    Ok(out)
}

/// One line of past conversation for the L2 recap.
pub struct RecapLine {
    pub ts: DateTime<Local>,
    pub speaker: String,
    pub text: String,
}

pub fn build_l2(partner_label: &str, partner_card: &str, recap: &[RecapLine]) -> String {
    let mut out = format!("# 会话框架\n\n## 正在和你聊天的人：{partner_label}\n\n{partner_card}");
    if !recap.is_empty() {
        out.push_str("\n\n## 本次会话之前的聊天记录\n");
        for line in recap {
            out.push_str(&format!("\n[{}] {}：{}", line.ts.format("%m-%d %H:%M"), line.speaker, line.text));
        }
    }
    out
}

pub struct StateSnapshot {
    pub mood: Value,
    pub energy: Value,
}

pub fn load_state(mind_dir: &Path) -> Result<StateSnapshot> {
    let read = |name: &str| -> Result<Value> {
        let text = read_norm(&mind_dir.join("state").join(name))?;
        serde_json::from_str(&text).with_context(|| format!("parsing state/{name}"))
    };
    Ok(StateSnapshot { mood: read("mood.json")?, energy: read("energy.json")? })
}

fn weekday_zh(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "周一",
        Weekday::Tue => "周二",
        Weekday::Wed => "周三",
        Weekday::Thu => "周四",
        Weekday::Fri => "周五",
        Weekday::Sat => "周六",
        Weekday::Sun => "周日",
    }
}

/// L4: the only place volatile state appears.
pub fn tail(now: DateTime<Local>, st: &StateSnapshot) -> String {
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let label = st.mood.get("label").and_then(Value::as_str).unwrap_or("平静");
    format!(
        "<当前状态>\n时间：{} {}\n心境：{label}（P {:.1} / A {:.1} / D {:.1}）\n精力：{:.1}\n</当前状态>",
        now.format("%Y-%m-%d %H:%M"),
        weekday_zh(now.weekday()),
        num(&st.mood, "pleasure"),
        num(&st.mood, "arousal"),
        num(&st.mood, "dominance"),
        num(&st.energy, "level"),
    )
}

pub fn user_turn(tail: &str, speaker: &str, text: &str) -> String {
    format!("{tail}\n\n{speaker}：{text}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn mind() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("mind")
    }

    #[test]
    fn l1_is_deterministic_and_lf_only() {
        let a = build_l1(&mind()).unwrap();
        let b = build_l1(&mind()).unwrap();
        assert_eq!(a, b);
        assert!(!a.contains('\r'));
        assert!(a.contains("## 边界（不可违反）"));
    }

    #[test]
    fn volatile_state_lives_only_in_the_tail() {
        let l1 = build_l1(&mind()).unwrap();
        let l2 = build_l2("主人", "card", &[]);
        let now = Local.with_ymd_and_hms(2026, 10, 31, 14, 5, 0).unwrap();
        let calm = StateSnapshot { mood: json!({"label": "平静"}), energy: json!({"level": 0.8}) };
        let upset = StateSnapshot { mood: json!({"label": "烦躁"}), energy: json!({"level": 0.2}) };
        let (t1, t2) = (tail(now, &calm), tail(now, &upset));
        assert_ne!(t1, t2);
        for layer in [&l1, &l2] {
            assert!(!layer.contains("烦躁") && !layer.contains("2026-10-31"));
        }
        assert!(t1.contains("周六"));
    }
}
