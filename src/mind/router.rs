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
        if let Some(id) = &sc.model {
            anyhow::ensure!(!budget_exhausted || sc.budget_exempt,
                "scenario `{scenario}` is pinned to `{id}` but its daily budget is exhausted");
            let model = self.model(id).ok_or_else(|| anyhow!("pinned model `{id}` is not available for scenario `{scenario}`"))?;
            anyhow::ensure!(policy.allows(model), "pinned model `{id}` is not allowed by {policy:?}");
            anyhow::ensure!(model.q >= sc.q_min, "pinned model `{id}` does not meet scenario `{scenario}` Q floor {}", sc.q_min);
            return Ok(model);
        }
        let front = self.front(sc, policy);
        if front.is_empty() {
            bail!("no model satisfies scenario `{scenario}` (q_min {}) under {policy:?}", sc.q_min);
        }
        let priority = if budget_exhausted && !sc.budget_exempt { Priority::Cheap } else { sc.priority };
        let pick = front.into_iter().min_by(|a, b| by_priority(priority, a, b));
        pick.ok_or_else(|| anyhow!("empty Pareto front for `{scenario}`"))
    }

    /// Recheck an explicit scenario pin at the call boundary, including cached sessions.
    /// Unpinned sessions retain their existing model until normal session rotation.
    pub fn select_for_call(&self, scenario: &str, policy: &ProcessingPolicy, session_model: &str, budget_exhausted: bool) -> Result<&ModelEntry> {
        if self.scenario(scenario)?.model.is_some() {
            self.select(scenario, policy, budget_exhausted)
        } else {
            self.model(session_model).ok_or_else(|| anyhow!("model `{session_model}` is not available"))
        }
    }

    /// The next model up in Q on the Pareto front, for escalation after a failure.
    pub fn next_stronger(&self, scenario: &str, policy: &ProcessingPolicy, current_id: &str) -> Option<&ModelEntry> {
        let sc = self.scenario(scenario).ok()?;
        if sc.model.is_some() { return None; }
        let current_q = self.model(current_id)?.q;
        self.front(sc, policy)
            .into_iter()
            .filter(|m| m.q > current_q)
            .min_by(|a, b| a.q.total_cmp(&b.q).then(blended_cost(a).total_cmp(&blended_cost(b))))
    }
}

/// Daily spend tracker; rolls over at local midnight.
pub struct Budget {
    pub limit_usd: f64,
    day: NaiveDate,
    pub spent_usd: f64,
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
            stream: false,
            enable_thinking: None,
        }
    }

    fn scenario(q_min: f64, priority: Priority, budget_exempt: bool) -> Scenario {
        Scenario { model: None, q_min, priority, review: false, budget_exempt, effort: None }
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
    fn scenario_pin_selects_exact_alias_even_when_dominated_and_does_not_change_other_scenarios() {
        let mut r = router();
        r.models.extend([
            model("argon", "a", 9.5, 0.1, 0.5, 3.0, false),
            model("argon-low", "a", 9.5, 0.1, 0.5, 3.0, false),
            model("argon-medium", "a", 9.5, 0.1, 0.5, 3.0, false),
            model("argon-high", "a", 9.5, 0.1, 0.5, 3.0, false),
        ]);
        let any = ProcessingPolicy::Any;
        let research = r.select("research", &any, false).unwrap().id.clone();
        r.scenarios.get_mut("chat").unwrap().model = Some("argon-medium".into());
        assert_eq!(r.select("chat", &any, false).unwrap().id, "argon-medium");
        assert_eq!(r.select("research", &any, false).unwrap().id, research);
        r.scenarios.get_mut("chat").unwrap().model = Some("dominated".into());
        assert_eq!(r.select("chat", &any, false).unwrap().id, "dominated");
        r.scenarios.get_mut("chat").unwrap().model = None;
        assert_eq!(r.select("chat", &any, false).unwrap().id, "cheap");
    }

    #[test]
    fn pin_rejects_missing_unavailable_policy_and_q_failures_without_escalation() {
        let mut r = router();
        let any = ProcessingPolicy::Any;
        r.scenarios.get_mut("chat").unwrap().model = Some("missing-or-disabled".into());
        assert!(r.select("chat", &any, false).is_err());
        r.scenarios.get_mut("chat").unwrap().model = Some("local".into());
        assert!(r.select("chat", &any, false).is_err());
        r.scenarios.get_mut("chat").unwrap().model = Some("mid".into());
        assert!(r.select("chat", &ProcessingPolicy::LocalOnly, false).is_err());
        assert!(r.select("chat", &ProcessingPolicy::Providers { names: vec!["b".into()] }, false).is_err());
        assert!(r.next_stronger("chat", &any, "mid").is_none());
        r.scenarios.get_mut("chat").unwrap().model = None;
        assert_eq!(r.next_stronger("chat", &any, "mid").unwrap().id, "top");
    }

    #[test]
    fn pin_is_rechecked_for_cached_session_and_budget() {
        let mut r = router();
        let any = ProcessingPolicy::Any;
        r.scenarios.get_mut("chat").unwrap().model = Some("mid".into());
        assert_eq!(r.select_for_call("chat", &any, "cheap", false).unwrap().id, "mid");
        assert!(r.select_for_call("chat", &any, "cheap", true).is_err());
        assert!(r.select_for_call("chat", &ProcessingPolicy::LocalOnly, "mid", false).is_err());
        r.scenarios.get_mut("chat").unwrap().budget_exempt = true;
        assert_eq!(r.select_for_call("chat", &any, "cheap", true).unwrap().id, "mid");
        r.scenarios.get_mut("chat").unwrap().model = None;
        assert_eq!(r.select_for_call("chat", &any, "mid", false).unwrap().id, "mid");
    }

    #[test]
    fn scenario_pin_yaml_is_optional_and_exact() {
        let old: Scenario = serde_yaml::from_str("q_min: 6\npriority: fast\neffort: low\n").unwrap();
        assert!(old.model.is_none());
        let pin: Scenario = serde_yaml::from_str("q_min: 6\npriority: fast\nmodel: gemini-4-argon-medium\n").unwrap();
        assert_eq!(pin.model.as_deref(), Some("gemini-4-argon-medium"));
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

    #[test]
    fn bailian_alone_covers_repo_scenarios() {
        let cfg = crate::config::Config::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let full = Router::new(cfg.models.models.clone(), cfg.scenarios.clone());
        let models = cfg.models.models.into_iter().filter(|m| m.provider == "bailian").collect();
        let r = Router::new(models, cfg.scenarios);
        // `evolve` is owner-triggered and falls back to owner review, so it may need a non-bailian model.
        assert!(full.select("evolve", &ProcessingPolicy::Any, false).is_ok());
        for name in r.scenarios.keys().filter(|n| *n != "evolve") {
            for exhausted in [false, true] {
                assert_eq!(r.select(name, &ProcessingPolicy::Any, exhausted).unwrap().provider, "bailian");
            }
        }
        assert_eq!(r.select("chat", &ProcessingPolicy::Any, false).unwrap().id, "qwen-flash");
        assert!(r.select("chat", &ProcessingPolicy::LocalOnly, false).is_err());
    }
}
