//! Focus-window accounting and advisory provider cache metadata. No routing/cache behavior.
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::config::{Config, ModelEntry, ProviderKind};
use crate::store::{ModelCallRecord, Store};

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct CachePolicy {
    /// None means provider-managed automatic caching, with no advertised expiry.
    pub ttl_seconds: Option<u64>,
    pub min_tokens: u64,
}

impl Default for CachePolicy {
    fn default() -> Self { Self { ttl_seconds: None, min_tokens: 64 } }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Override {
    ttl_seconds: Option<u64>,
    min_tokens: Option<u64>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Tools {
    provider_cache: BTreeMap<String, Override>,
}

#[derive(Default)]
pub struct ProviderCaches(BTreeMap<String, CachePolicy>);

impl ProviderCaches {
    /// Optional config/tools.yaml: provider_cache.<provider>.{ttl_seconds,min_tokens}.
    /// ttl_seconds: 0 explicitly selects automatic caching (no expiry warning).
    pub fn load(root: &Path) -> Result<Self> {
        let mut policies = BTreeMap::from([
            ("anthropic".into(), CachePolicy { ttl_seconds: Some(300), min_tokens: 1024 }),
            ("deepseek".into(), CachePolicy::default()),
            ("openai_compatible".into(), CachePolicy::default()),
        ]);
        let tools = match std::fs::read_to_string(root.join("config/tools.yaml")) {
            Ok(text) => serde_yaml::from_str::<Tools>(&text)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Tools::default(),
            Err(e) => return Err(e.into()),
        };
        for (name, patch) in tools.provider_cache {
            let policy = policies.entry(name).or_default();
            if let Some(ttl) = patch.ttl_seconds { policy.ttl_seconds = (ttl > 0).then_some(ttl); }
            if let Some(min) = patch.min_tokens { policy.min_tokens = min; }
        }
        Ok(Self(policies))
    }

    fn policy(&self, provider: &str, kind: ProviderKind) -> CachePolicy {
        self.0.get(provider).or_else(|| self.0.get(match kind {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenaiCompat => "openai_compatible",
        })).cloned().unwrap_or_default()
    }
}

type LastCalls = BTreeMap<(String, String), DateTime<Utc>>;

#[derive(Default)]
pub struct FocusCache {
    policies: ProviderCaches,
    kinds: BTreeMap<String, ProviderKind>,
    last: Mutex<Option<LastCalls>>,
}

impl FocusCache {
    pub fn new(cfg: &Config) -> Result<Self> {
        Ok(Self { policies: ProviderCaches::load(&cfg.root)?,
            kinds: cfg.models.providers.iter().map(|(k, v)| (k.clone(), v.kind)).collect(),
            last: Mutex::new(None) })
    }

    pub fn set_active(&self, active: bool) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = active.then(BTreeMap::new);
    }

    pub fn resumed(&self, model: &ModelEntry, session_id: &str, now: DateTime<Utc>) {
        let mut guard = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let Some(last) = guard.as_mut() else { return };
        let policy = self.policies.policy(&model.provider,
            self.kinds.get(&model.provider).copied().unwrap_or(ProviderKind::OpenaiCompat));
        if let (Some(previous), Some(ttl)) = (last.get(&(model.id.clone(), session_id.into())), policy.ttl_seconds)
            && (now - *previous).num_seconds() > i64::try_from(ttl).unwrap_or(i64::MAX) {
            tracing::debug!(provider = %model.provider, model = %model.id, ttl_seconds = ttl,
                min_tokens = policy.min_tokens, "focus resumed after provider cache TTL; cache may be cold");
        }
        last.insert((model.id.clone(), session_id.into()), now);
    }

    pub fn completed(&self, model: &ModelEntry, session_id: &str, now: DateTime<Utc>) {
        if let Some(last) = self.last.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            last.insert((model.id.clone(), session_id.into()), now);
        }
    }
}

#[derive(Default, Debug)]
pub struct Totals {
    pub calls: u64,
    pub cached: u64,
    pub uncached: u64,
    pub cost_usd: f64,
}

impl Totals {
    pub fn cached_share(&self) -> f64 {
        let input = self.cached as f64 + self.uncached as f64;
        if input == 0.0 { 0.0 } else { self.cached as f64 / input }
    }
}

pub fn totals(calls: &[ModelCallRecord], models: &[ModelEntry]) -> BTreeMap<(String, String), Totals> {
    let mut out = BTreeMap::<_, Totals>::new();
    for call in calls {
        // Delegate model ids need not be in the router catalog; do not guess their provider.
        let provider = models.iter().find(|m| m.id == call.model_id).map(|m| m.provider.as_str()).unwrap_or("unmapped");
        let t = out.entry((provider.to_string(), call.model_id.clone())).or_default();
        t.calls += 1;
        t.cached = t.cached.saturating_add(call.cached_input);
        t.uncached = t.uncached.saturating_add(call.uncached_input);
        t.cost_usd += call.cost_usd;
    }
    out
}

pub fn record(store: &dyn Store, models: &[ModelEntry], flow: &super::flow::Flow, until: DateTime<Utc>) -> Result<()> {
    let Some(since) = flow.since.filter(|_| flow.is_focused()) else { return Ok(()) };
    let calls = store.model_calls_between(since, until)?;
    let mut session = Totals::default();
    let parts: Vec<String> = totals(&calls, models).into_iter().map(|((provider, model), t)| {
        session.calls += t.calls;
        session.cached = session.cached.saturating_add(t.cached);
        session.uncached = session.uncached.saturating_add(t.uncached);
        session.cost_usd += t.cost_usd;
        format!("{provider}/{model}: calls={}, cached_share={:.4}, cached_input={}, uncached_input={}, cost_usd={:.6}",
            t.calls, t.cached_share(), t.cached, t.uncached, t.cost_usd)
    }).collect();
    let line = format!("flow cache metrics task={} window=[{since},{until}) calls={} cached_share={:.4} cost_usd={:.6} [{}] (time-window attribution)",
        flow.task_id.as_deref().unwrap_or("unknown"), session.calls, session.cached_share(), session.cost_usd, parts.join("; "));
    tracing::info!("{line}");
    super::reflect::note_today(store, &line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::jsonl::JsonlStore;

    #[test]
    fn ttl_warning_is_scoped_to_focus_model_and_session() {
        #[derive(Clone, Default)]
        struct Log(std::sync::Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Log {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let log = Log::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG)
            .without_time().with_ansi(false).with_writer(move || writer.clone()).finish();
        let tracker = FocusCache { policies: ProviderCaches::load(Path::new("nonexistent-test-root")).unwrap(),
            ..Default::default() };
        let model: ModelEntry = serde_yaml::from_str("{id: a, provider: anthropic, model: claude, q: 1, c: {input: 1, cached: 1, output: 1}, l: 1}").unwrap();
        let now = Utc::now();
        tracing::subscriber::with_default(subscriber, || {
            tracker.resumed(&model, "one", now); // Not focused.
            tracker.set_active(true);
            tracker.resumed(&model, "one", now);
            tracker.completed(&model, "one", now + chrono::Duration::seconds(10));
            tracker.resumed(&model, "two", now + chrono::Duration::seconds(400)); // Different prefix.
            tracker.resumed(&model, "one", now + chrono::Duration::seconds(311)); // Expired.
            tracker.resumed(&model, "one", now + chrono::Duration::seconds(320)); // Warm.
            tracker.set_active(false);
            tracker.set_active(true);
            tracker.resumed(&model, "one", now + chrono::Duration::seconds(1000)); // New focus.
        });
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        assert_eq!(text.matches("focus resumed after provider cache TTL").count(), 1, "{text}");
        assert!(text.contains("min_tokens=1024"));
    }

    #[test]
    fn optional_tools_defaults_and_partial_overrides() {
        let root = std::env::temp_dir().join(format!("agli-cache-settings-{}", Utc::now().timestamp_nanos_opt().unwrap()));
        let policies = ProviderCaches::load(&root).unwrap();
        assert_eq!(policies.policy("anthropic", ProviderKind::Anthropic), CachePolicy { ttl_seconds: Some(300), min_tokens: 1024 });
        for name in ["deepseek", "openai", "bailian"] {
            assert_eq!(policies.policy(name, ProviderKind::OpenaiCompat), CachePolicy::default());
        }
        std::fs::create_dir_all(root.join("config")).unwrap();
        let path = root.join("config/tools.yaml");
        std::fs::write(&path, "unrelated: true\n").unwrap();
        assert_eq!(ProviderCaches::load(&root).unwrap().policy("alias", ProviderKind::Anthropic).min_tokens, 1024);
        std::fs::write(&path, "provider_cache:\n  anthropic:\n    ttl_seconds: 600\n  deepseek:\n    min_tokens: 128\n  alias:\n    ttl_seconds: 60\n    min_tokens: 256\n").unwrap();
        let policies = ProviderCaches::load(&root).unwrap();
        assert_eq!(policies.policy("anthropic", ProviderKind::Anthropic), CachePolicy { ttl_seconds: Some(600), min_tokens: 1024 });
        assert_eq!(policies.policy("deepseek", ProviderKind::OpenaiCompat), CachePolicy { ttl_seconds: None, min_tokens: 128 });
        assert_eq!(policies.policy("alias", ProviderKind::OpenaiCompat), CachePolicy { ttl_seconds: Some(60), min_tokens: 256 });
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn window_totals_and_journal_material() {
        let root = std::env::temp_dir().join(format!("agli-flow-metrics-{}", Utc::now().timestamp_nanos_opt().unwrap()));
        let store = JsonlStore::open(&root).unwrap();
        let start = Utc::now() - chrono::Duration::minutes(5);
        let end = Utc::now();
        let call = ModelCallRecord { ts: start, scenario: "research".into(), model_id: "a".into(),
            session_id: "delegate".into(), uncached_input: 20, cached_input: 80, cache_write: 0,
            output: 5, latency_ms: 10, cost_usd: 0.1, ok: true, error: None, escalated_from: None };
        for c in [ModelCallRecord { ts: start - chrono::Duration::nanoseconds(1), ..call.clone() },
            call.clone(), ModelCallRecord { ts: start + chrono::Duration::seconds(1), cached_input: 0, ..call.clone() },
            ModelCallRecord { model_id: "b".into(), cached_input: 0, uncached_input: 0, ..call.clone() },
            ModelCallRecord { ts: end, ..call.clone() }] {
            store.append_model_call(&c).unwrap();
        }
        let models: Vec<ModelEntry> = serde_yaml::from_str("- {id: a, provider: anthropic, model: claude, q: 1, c: {input: 1, cached: 1, output: 1}, l: 1}").unwrap();
        let calls = store.model_calls_between(start, end).unwrap();
        assert_eq!(calls.len(), 3);
        let grouped = totals(&calls, &models);
        let a = &grouped[&("anthropic".into(), "a".into())];
        assert_eq!((a.calls, a.cached, a.uncached), (2, 80, 40));
        assert!((a.cached_share() - 2.0 / 3.0).abs() < 1e-10);
        assert!((a.cost_usd - 0.2).abs() < 1e-10);
        assert_eq!(grouped[&("unmapped".into(), "b".into())].cached_share(), 0.0);
        let mut flow = super::super::flow::Flow::default();
        flow.enter("project/test".parse().unwrap(), "test", "test", start);
        record(&store, &models, &flow, end).unwrap();
        let key = format!("daylog:{}", super::super::reflect::diary_day(chrono::Local::now()));
        let notes: Vec<String> = serde_json::from_value(store.kv_get(&key).unwrap().unwrap()).unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("calls=3") && notes[0].contains("anthropic/a: calls=2") && notes[0].contains("cached_share=0.6667"));
        assert!(store.model_calls_between(end, end + chrono::Duration::seconds(1)).unwrap().len() == 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
