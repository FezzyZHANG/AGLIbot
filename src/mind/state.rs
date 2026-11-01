//! Fast variables: mood (PAD), energy and circadian sleep, drives (SDT needs). The heartbeat
//! advances them; `tail()` shows them to the model in L4 only.
//!
//! Runtime state lives in `data/state/state.json` (gitignored), seeded from `mind/state/*.json`,
//! so the tracked seed files never churn.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};

use super::context::StateSnapshot;
use crate::config::RoutineCfg;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mood {
    pub label: String,
    pub pleasure: f64,
    pub arousal: f64,
    pub dominance: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Energy {
    pub level: f64,
    pub asleep: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drives {
    pub curiosity: f64,
    pub social: f64,
    pub competence: f64,
    pub rest: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MindState {
    pub mood: Mood,
    pub energy: Energy,
    pub drives: Drives,
    #[serde(default)]
    pub last_tick: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_owner_contact: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_proactive: Option<DateTime<Utc>>,
    /// `decide` asked to be woken again at this time.
    #[serde(default)]
    pub revisit_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_reflect: Option<NaiveDate>,
    #[serde(default)]
    pub last_digest: Option<NaiveDate>,
}

/// Something worth waking `decide` (or another routine) for.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    FellAsleep,
    WokeUp,
    /// A drive crossed the threshold upward on this tick.
    DriveHigh(&'static str),
}

const BASELINE: (f64, f64, f64) = (0.2, 0.0, 0.1);
pub const DRIVE_THRESHOLD: f64 = 0.7;

fn mood_label(p: f64, a: f64) -> &'static str {
    match (p >= 0.0, a >= 0.3, a <= -0.3) {
        (true, true, _) => "兴奋",
        (true, _, true) => "放松",
        (true, _, _) => "平静",
        (false, true, _) => "烦躁",
        (false, _, true) => "低落",
        (false, _, _) => "有点闷",
    }
}

/// Whether `t` falls in the sleep window, which may wrap past midnight.
pub fn in_sleep_window(t: NaiveTime, r: &RoutineCfg) -> bool {
    if r.sleep_at < r.wake_at {
        t >= r.sleep_at && t < r.wake_at
    } else {
        t >= r.sleep_at || t < r.wake_at
    }
}

impl MindState {
    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("state").join("state.json")
    }

    pub fn load(data_dir: &Path, mind_dir: &Path) -> Result<Self> {
        let path = Self::path(data_dir);
        if let Ok(text) = std::fs::read_to_string(&path) {
            return serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()));
        }
        let seed = |name: &str| -> Result<serde_json::Value> {
            let p = mind_dir.join("state").join(name);
            let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
            Ok(serde_json::from_str(&text)?)
        };
        Ok(MindState {
            mood: serde_json::from_value(seed("mood.json")?)?,
            energy: serde_json::from_value(seed("energy.json")?)?,
            drives: serde_json::from_value(seed("drives.json")?)?,
            last_tick: None,
            last_owner_contact: None,
            last_proactive: None,
            revisit_at: None,
            last_reflect: None,
            last_digest: None,
        })
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let path = Self::path(data_dir);
        std::fs::create_dir_all(path.parent().unwrap_or(data_dir))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            mood: serde_json::to_value(&self.mood).unwrap_or_default(),
            energy: serde_json::to_value(&self.energy).unwrap_or_default(),
        }
    }

    /// Advance time. Needs grow, mood drifts back to baseline, energy follows the routine.
    pub fn tick(&mut self, now: DateTime<Utc>, routine: &RoutineCfg) -> Vec<Change> {
        let hours = self.last_tick.map(|t| (now - t).num_seconds().max(0) as f64 / 3600.0).unwrap_or(0.0).min(12.0);
        self.last_tick = Some(now);
        let mut changes = Vec::new();

        // The elapsed interval is accounted in the state he was in during it; then the routine flips.
        let was_asleep = self.energy.asleep;
        let e = &mut self.energy.level;
        *e = if was_asleep { *e + 0.12 * hours } else { *e - 0.04 * hours }.clamp(0.0, 1.0);
        let before = self.drives.clone();
        let d = &mut self.drives;
        if !was_asleep {
            d.social = (d.social + 0.05 * hours).min(1.0);
            d.curiosity = (d.curiosity + 0.03 * hours).min(1.0);
            d.competence = (d.competence + 0.02 * hours).min(1.0);
        }
        d.rest = 1.0 - self.energy.level;

        let should_sleep = in_sleep_window(now.with_timezone(&Local).time(), routine);
        if should_sleep && !was_asleep {
            self.energy.asleep = true;
            changes.push(Change::FellAsleep);
        } else if !should_sleep && was_asleep {
            self.energy.asleep = false;
            changes.push(Change::WokeUp);
        }
        for (name, was, now) in [
            ("social", before.social, d.social),
            ("curiosity", before.curiosity, d.curiosity),
            ("competence", before.competence, d.competence),
        ] {
            if was < DRIVE_THRESHOLD && now >= DRIVE_THRESHOLD {
                changes.push(Change::DriveHigh(name));
            }
        }

        // Exponential drift toward baseline, about half-way every two hours.
        let k = 1.0 - 0.5f64.powf(hours / 2.0);
        let m = &mut self.mood;
        m.pleasure += (BASELINE.0 - m.pleasure) * k;
        m.arousal += (BASELINE.1 - m.arousal) * k;
        m.dominance += (BASELINE.2 - m.dominance) * k;
        m.label = mood_label(m.pleasure, m.arousal).to_string();
        changes
    }

    /// An appraisal from `decide` or a conversation: nudge mood, keep it in range.
    pub fn appraise(&mut self, dp: f64, da: f64, dd: f64) {
        let m = &mut self.mood;
        m.pleasure = (m.pleasure + dp.clamp(-0.5, 0.5)).clamp(-1.0, 1.0);
        m.arousal = (m.arousal + da.clamp(-0.5, 0.5)).clamp(-1.0, 1.0);
        m.dominance = (m.dominance + dd.clamp(-0.5, 0.5)).clamp(-1.0, 1.0);
        m.label = mood_label(m.pleasure, m.arousal).to_string();
    }

    pub fn talked_with_owner(&mut self, now: DateTime<Utc>) {
        self.last_owner_contact = Some(now);
        self.drives.social = (self.drives.social - 0.4).max(0.05);
    }

    pub fn finished_work(&mut self) {
        self.drives.curiosity = (self.drives.curiosity - 0.4).max(0.05);
        self.drives.competence = (self.drives.competence - 0.4).max(0.05);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn routine() -> RoutineCfg {
        RoutineCfg { sleep_at: NaiveTime::from_hms_opt(1, 30, 0).unwrap(), wake_at: NaiveTime::from_hms_opt(8, 30, 0).unwrap() }
    }

    fn state() -> MindState {
        let mind = Path::new(env!("CARGO_MANIFEST_DIR")).join("mind");
        MindState::load(Path::new("/nonexistent-aglibot"), &mind).unwrap()
    }

    #[test]
    fn sleep_window_wraps_midnight() {
        let r = RoutineCfg { sleep_at: NaiveTime::from_hms_opt(23, 0, 0).unwrap(), wake_at: NaiveTime::from_hms_opt(7, 0, 0).unwrap() };
        assert!(in_sleep_window(NaiveTime::from_hms_opt(23, 30, 0).unwrap(), &r));
        assert!(in_sleep_window(NaiveTime::from_hms_opt(3, 0, 0).unwrap(), &r));
        assert!(!in_sleep_window(NaiveTime::from_hms_opt(12, 0, 0).unwrap(), &r));
        assert!(in_sleep_window(NaiveTime::from_hms_opt(2, 0, 0).unwrap(), &routine()));
    }

    #[test]
    fn day_and_night_cycle() {
        let mut s = state();
        let noon = Local.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap().with_timezone(&Utc);
        assert!(s.tick(noon, &routine()).is_empty());
        // Ten waking hours: social need crosses the threshold once; energy drops.
        let evening = noon + Duration::hours(10);
        let changes = s.tick(evening, &routine());
        assert!(changes.contains(&Change::DriveHigh("social")), "{changes:?}");
        assert!(s.energy.level < 0.8);
        let night = Local.with_ymd_and_hms(2026, 11, 2, 2, 0, 0).unwrap().with_timezone(&Utc);
        assert!(s.tick(night, &routine()).contains(&Change::FellAsleep));
        let morning = Local.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap().with_timezone(&Utc);
        assert!(s.tick(morning, &routine()).contains(&Change::WokeUp));
        assert!(s.energy.level > 0.5);
    }

    #[test]
    fn appraisal_moves_mood_and_it_drifts_back() {
        let mut s = state();
        let t0 = Utc::now();
        s.tick(t0, &routine());
        s.appraise(-0.5, 0.5, 0.0);
        assert_eq!(s.mood.label, "烦躁");
        s.tick(t0 + Duration::hours(8), &routine());
        assert!(s.mood.pleasure > 0.0);
    }
}
