//! Multi-model routing over the hand-filled QCL table (docs/DESIGN.md 5c).
//!
//! 1. Drop models below the scenario's Q floor or not allowed by the processing policy.
//! 2. Drop dominated models (Pareto filter over Q↑, C↓, L↓).
//! 3. Pick by the scenario's priority; once the daily budget is gone, non-exempt scenarios go cheap.

use std::cmp::Ordering;

use anyhow::{Result, anyhow, bail};
use chrono::{Local, NaiveDate};

use crate::config::{ModelEntry, Priority, Scenario, ScenariosCfg};
use crate::ns::ProcessingPolicy;

pub struct Router {
    models: Vec<ModelEntry>,
    scenarios: ScenariosCfg,
}

/// Single cost number for comparisons: input + output price per M tokens.
pub fn blended_cost(m: &ModelEntry) -> f64 {
    m.c.input + m.c.output
}

fn dominates(a: &ModelEntry, b: &ModelEntry) -> bool {
    let (ca, cb) = (blended_cost(a), blended_cost(b));
    let no_worse = a.q >= b.q && ca <= cb && a.l <= b.l;
    let better = a.q > b.q || ca < cb || a.l < b.l;
    no_worse && better
}

fn by_priority(priority: Priority, a: &ModelEntry, b: &ModelEntry) -> Ordering {
    let cost = blended_cost(a).total_cmp(&blended_cost(b));
    let quality = b.q.total_cmp(&a.q);
    match priority {
        Priority::Cheap => cost.then(quality),
        Priority::Fast => a.l.total_cmp(&b.l).then(quality),
        Priority::Quality => quality.then(cost),
    }
}

impl Router {
    pub fn new(models: Vec<ModelEntry>, scenarios: ScenariosCfg) -> Self {
        Router { models, scenarios }
    }

    pub fn model(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn scenario(&self, name: &str) -> Result<&Scenario> {
        self.scenarios.get(name).ok_or_else(|| anyhow!("unknown scenario `{name}`"))
    }

    /// Allowed models for the scenario with dominated ones removed.
    fn front(&self, sc: &Scenario, policy: &ProcessingPolicy) -> Vec<&ModelEntry> {
        let cands: Vec<&ModelEntry> =
            self.models.iter().filter(|m| m.q >= sc.q_min && policy.allows(m)).collect();
        cands.iter().copied().filter(|b| !cands.iter().any(|a| dominates(a, b))).collect()
    }

    pub fn select(&self, scenario: &str, policy: &ProcessingPolicy, budget_exhausted: bool) -> Result<&ModelEntry> {
        let sc = self.scenario(scenario)?;
        let front = self.front(sc, policy);
        if front.is_empty() {
            bail!("no model satisfies scenario `{scenario}` (q_min {}) under {policy:?}", sc.q_min);
        }
        let priority = if budget_exhausted && !sc.budget_exempt { Priority::Cheap } else { sc.priority };
        let pick = front.into_iter().min_by(|a, b| by_priority(priority, a, b));
        pick.ok_or_else(|| anyhow!("empty Pareto front for `{scenario}`"))
    }

    /// The next model up in Q on the Pareto front, for escalation after a failure.
    pub fn next_stronger(&self, scenario: &str, policy: &ProcessingPolicy, current_id: &str) -> Option<&ModelEntry> {
        let sc = self.scenario(scenario).ok()?;
        let current_q = self.model(current_id)?.q;
        self.front(sc, policy)
            .into_iter()
            .filter(|m| m.q > current_q)
            .min_by(|a, b| a.q.total_cmp(&b.q).then(blended_cost(a).total_cmp(&blended_cost(b))))
    }
}

/// Daily spend tracker; rolls over at local midnight.
pub struct Budget {
    limit_usd: f64,
    day: NaiveDate,
    spent_usd: f64,
}

impl Budget {
    pub fn new(limit_usd: f64, spent_today_usd: f64) -> Self {
        Budget { limit_usd, day: Local::now().date_naive(), spent_usd: spent_today_usd }
    }

    fn roll(&mut self) {
        let today = Local::now().date_naive();
        if today != self.day {
            self.day = today;
            self.spent_usd = 0.0;
        }
    }

    pub fn add(&mut self, usd: f64) {
        self.roll();
        self.spent_usd += usd;
    }

    pub fn exhausted(&mut self) -> bool {
        self.roll();
        self.spent_usd >= self.limit_usd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Price;

    fn model(id: &str, provider: &str, q: f64, input: f64, output: f64, l: f64, local: bool) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            provider: provider.into(),
            model: id.into(),
            q,
            c: Price { input, cached: input / 10.0, output, cache_write: None },
            l,
            local,
            fallbacks: None,
        }
    }

    fn scenario(q_min: f64, priority: Priority, budget_exempt: bool) -> Scenario {
        Scenario { q_min, priority, review: false, budget_exempt, effort: None }
    }

    /// The 4-row sample table from docs/DESIGN.md 5b, plus one deliberately dominated row.
    fn router() -> Router {
        let models = vec![
            model("top", "a", 9.5, 4.0, 20.0, 3.0, false),
            model("mid", "a", 8.0, 2.0, 10.0, 1.5, false),
            model("cheap", "b", 6.0, 0.3, 1.0, 0.6, false),
            model("local", "local", 5.0, 0.0, 0.0, 1.0, true),
            // Worse than `mid` on every axis.
            model("dominated", "b", 7.5, 3.0, 12.0, 2.0, false),
        ];
        let scenarios = ScenariosCfg::from([
            ("chat".to_string(), scenario(6.0, Priority::Fast, false)),
            ("extract".to_string(), scenario(5.0, Priority::Cheap, false)),
            ("verify".to_string(), scenario(8.0, Priority::Quality, true)),
            ("research".to_string(), scenario(7.0, Priority::Quality, false)),
        ]);
        Router::new(models, scenarios)
    }

    #[test]
    fn picks_by_priority() {
        let r = router();
        let any = ProcessingPolicy::Any;
        assert_eq!(r.select("chat", &any, false).unwrap().id, "cheap");
        assert_eq!(r.select("extract", &any, false).unwrap().id, "local");
        assert_eq!(r.select("verify", &any, false).unwrap().id, "top");
    }

    #[test]
    fn dominated_model_is_never_chosen() {
        let r = router();
        let any = ProcessingPolicy::Any;
        for sc in ["chat", "extract", "verify", "research"] {
            for exhausted in [false, true] {
                assert_ne!(r.select(sc, &any, exhausted).unwrap().id, "dominated");
            }
        }
    }

    #[test]
    fn exhausted_budget_goes_cheap_except_exempt() {
        let r = router();
        let any = ProcessingPolicy::Any;
        assert_eq!(r.select("research", &any, false).unwrap().id, "top");
        assert_eq!(r.select("research", &any, true).unwrap().id, "mid");
        assert_eq!(r.select("verify", &any, true).unwrap().id, "top");
    }

    #[test]
    fn policy_filters_models() {
        let r = router();
        assert_eq!(r.select("extract", &ProcessingPolicy::LocalOnly, false).unwrap().id, "local");
        assert!(r.select("chat", &ProcessingPolicy::LocalOnly, false).is_err());
        let only_a = ProcessingPolicy::Providers { names: vec!["a".into()] };
        assert_eq!(r.select("chat", &only_a, false).unwrap().id, "mid");
    }

    #[test]
    fn escalation_climbs_q() {
        let r = router();
        let any = ProcessingPolicy::Any;
        assert_eq!(r.next_stronger("chat", &any, "cheap").unwrap().id, "mid");
        assert_eq!(r.next_stronger("chat", &any, "mid").unwrap().id, "top");
        assert!(r.next_stronger("chat", &any, "top").is_none());
    }
}
