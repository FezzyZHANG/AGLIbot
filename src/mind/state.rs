//! Circadian state and persisted scheduler timers.

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::RoutineCfg;
use crate::store::Store;

pub(crate) const MIND_STATE_KEY: &str = "mind_state";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MindState {
    #[serde(default)]
    pub asleep: bool,
    #[serde(default)]
    pub last_tick: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_owner_contact: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_proactive: Option<DateTime<Utc>>,
    #[serde(default)]
    pub revisit_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_reflect: Option<NaiveDate>,
    #[serde(default)]
    pub last_digest: Option<NaiveDate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    FellAsleep,
    WokeUp,
}

/// Whether `t` falls in the sleep window, which may wrap past midnight.
pub fn in_sleep_window(t: NaiveTime, routine: &RoutineCfg) -> bool {
    if routine.sleep_at < routine.wake_at {
        t >= routine.sleep_at && t < routine.wake_at
    } else {
        t >= routine.sleep_at || t < routine.wake_at
    }
}

impl MindState {
    pub async fn load(store: &Store) -> Result<Self> {
        match store.kv_get(MIND_STATE_KEY).await? {
            Some(mut value) => {
                if value.get("asleep").is_none()
                    && let Some(asleep) = value.pointer("/energy/asleep").and_then(serde_json::Value::as_bool)
                {
                    value["asleep"] = asleep.into();
                }
                serde_json::from_value(value).context("decoding mind_state from storage")
            }
            None => Ok(Self::default()),
        }
    }

    pub async fn save(&self, store: &Store) -> Result<()> {
        store.kv_set(MIND_STATE_KEY, serde_json::to_value(self)?).await
    }

    pub fn tick(&mut self, now: DateTime<Utc>, routine: &RoutineCfg) -> Vec<Change> {
        self.last_tick = Some(now);
        let asleep = in_sleep_window(now.with_timezone(&Local).time(), routine);
        match (std::mem::replace(&mut self.asleep, asleep), asleep) {
            (false, true) => vec![Change::FellAsleep],
            (true, false) => vec![Change::WokeUp],
            _ => Vec::new(),
        }
    }

    pub fn talked_with_owner(&mut self, now: DateTime<Utc>) {
        self.last_owner_contact = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn sleep_window_wraps_midnight() {
        let t = |h| NaiveTime::from_hms_opt(h, 0, 0).unwrap();
        let routine = RoutineCfg { sleep_at: t(23), wake_at: t(7) };
        assert!(in_sleep_window(t(23), &routine));
        assert!(in_sleep_window(t(3), &routine));
        assert!(!in_sleep_window(t(7), &routine));
        assert!(!in_sleep_window(t(12), &routine));
    }

    #[test]
    fn day_and_night_cycle() {
        let at = |h| Local.with_ymd_and_hms(2026, 11, 2, h, 0, 0).unwrap().with_timezone(&Utc);
        let routine = RoutineCfg { sleep_at: at(1).with_timezone(&Local).time(), wake_at: at(8).with_timezone(&Local).time() };
        let mut state = MindState::default();
        assert_eq!(state.tick(at(2), &routine), [Change::FellAsleep]);
        assert!(state.asleep && state.tick(at(3), &routine).is_empty());
        assert_eq!(state.tick(at(8), &routine), [Change::WokeUp]);
        assert!(!state.asleep);
        assert_eq!(state.last_tick, Some(at(8)));
    }

    #[tokio::test]
    async fn persisted_state_loads_defaults_and_preserves_legacy_sleep() {
        let store = Store::memory();
        assert!(!MindState::load(&store).await.unwrap().asleep);
        let now = Utc::now();
        store.kv_set(MIND_STATE_KEY, json!({
            "energy": {"asleep": true, "level": 0.2},
            "mood": {},
            "drives": {},
            "last_tick": now,
        })).await.unwrap();
        let mut state = MindState::load(&store).await.unwrap();
        assert!(state.asleep);
        assert_eq!(state.last_tick, Some(now));
        state.talked_with_owner(now);
        state.save(&store).await.unwrap();
        let saved = store.kv_get(MIND_STATE_KEY).await.unwrap().unwrap();
        assert!(saved.get("energy").is_none());
        let reloaded = MindState::load(&store).await.unwrap();
        assert!(reloaded.asleep);
        assert_eq!(reloaded.last_owner_contact, Some(now));
    }
}
