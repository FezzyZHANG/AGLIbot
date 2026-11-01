use super::*;
use crate::mind::embedding::EmbeddingCfg;
use crate::store::{EpisodeEmbedding, jsonl::JsonlStore};

fn config() -> EmbeddingCfg {
    serde_yaml::from_str("provider: test\nmodel: v1\nmax_cross_namespace: 2").unwrap()
}

fn now() -> DateTime<Utc> { "2026-11-02T00:00:00Z".parse().unwrap() }

fn episode(id: &str, ns: &str, scope: Scope, salience: f64) -> Episode {
    Episode { id: id.into(), ts: now(), participants: vec![], summary: id.into(), emotion: String::new(),
        salience, ns: ns.parse().unwrap(), scope, sources: vec![] }
}

struct Fixture { store: JsonlStore, dir: std::path::PathBuf }
impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("aglibot-semantic-{name}-{}", std::process::id()));
        Self { store: JsonlStore::open(&dir).unwrap(), dir }
    }
    fn add(&self, e: Episode, vector: &[f32]) {
        self.store.append_episode(&e).unwrap();
        self.store.save_embedding(&EpisodeEmbedding { episode_id: e.id, model_id: config().model_id(), vector: vector.into() }).unwrap();
    }
}
impl Drop for Fixture { fn drop(&mut self) { std::fs::remove_dir_all(&self.dir).unwrap(); } }

fn ids(recall: &Recall) -> Vec<&str> { recall.episodes.iter().map(|e| e.id.as_str()).collect() }

#[test]
fn warm_source_cache_still_recomputes_semantics_and_scope() {
    let f = Fixture::new("source-cache");
    f.add(episode("x", "project/x", Scope::Public, 1.0), &[1.0, 0.0]);
    f.add(episode("y", "project/y", Scope::Public, 1.0), &[0.0, 1.0]);
    f.add(episode("secret", "person/a", Scope::Persons { ids: vec!["a".into()] }, 1.0), &[1.0, 0.0]);
    let ns = "person/a".parse().unwrap();
    let audience = Audience::private("a");
    let mut q = Query { ns: &ns, audience: &audience, partners: &[], max_episodes: 3, max_facts: 0,
        include_lessons: false, half_life_days: 30.0 };
    let first = recall_semantic_at(&f.store, &q, &config(), &[1.0, 0.0], now()).unwrap();
    assert_eq!(ids(&first), ["secret", "x"]);
    let counts = f.store.recall_cache_counts();
    let second = recall_semantic_at(&f.store, &q, &config(), &[0.0, 1.0], now()).unwrap();
    assert_eq!(ids(&second), ["secret", "y"]);
    assert_eq!(f.store.recall_cache_counts(), counts);
    let group = Audience::group("g", ["a".into(), "b".into()]);
    q.audience = &group;
    let shared = recall_semantic_at(&f.store, &q, &config(), &[1.0, 0.0], now()).unwrap();
    assert_eq!(ids(&shared), ["x"]);
    let fresh = JsonlStore::open(&f.dir).unwrap();
    assert_eq!(render(&shared), render(&recall_semantic_at(&fresh, &q, &config(), &[1.0, 0.0], now()).unwrap()));
    assert_eq!(f.store.recall_cache_counts(), counts);
    f.store.save_embedding(&EpisodeEmbedding { episode_id: "x".into(), model_id: config().model_id(), vector: vec![0.0, 1.0] }).unwrap();
    assert!(recall_semantic_at(&f.store, &q, &config(), &[1.0, 0.0], now()).unwrap().episodes.is_empty());
    assert!(f.store.recall_cache_counts().1 > counts.1);
}

#[test]
fn cross_namespace_threshold_score_decay_and_total_cap() {
    let f = Fixture::new("rank");
    f.add(episode("own", "person/a", Scope::Public, 0.01), &[0.0, 1.0]);
    f.add(episode("related-high", "project/rust", Scope::Public, 0.9), &[0.8, 0.6]);
    f.add(episode("related-low", "project/other", Scope::Public, 0.5), &[1.0, 0.0]);
    f.add(episode("unrelated", "project/unrelated", Scope::Public, 1.0), &[0.0, 1.0]);
    let mut old = episode("old", "project/old", Scope::Public, 1.0);
    old.ts = now() - chrono::Duration::days(60);
    f.add(old, &[1.0, 0.0]);
    let ns = "person/a".parse().unwrap();
    let audience = Audience::private("a");
    let mut q = Query { ns: &ns, audience: &audience, partners: &[], max_episodes: 3, max_facts: 0,
        include_lessons: false, half_life_days: 30.0 };
    let baseline = recall_at(&f.store, &q, now()).unwrap();
    assert_eq!(ids(&baseline), ["own"]);
    let r = recall_semantic_at(&f.store, &q, &config(), &[1.0, 0.0], now()).unwrap();
    assert_eq!(ids(&r), ["own", "related-high", "related-low"]);
    q.max_episodes = 2;
    assert_eq!(ids(&recall_semantic_at(&f.store, &q, &config(), &[1.0, 0.0], now()).unwrap()), ["own", "related-high"]);
    q.max_episodes = 1;
    assert_eq!(ids(&recall_semantic_at(&f.store, &q, &config(), &[1.0, 0.0], now()).unwrap()), ["own"]);
    q.max_episodes = 0;
    assert!(recall_semantic_at(&f.store, &q, &config(), &[1.0, 0.0], now()).unwrap().episodes.is_empty());
}

#[test]
fn scope_precedes_similarity_even_for_perfect_or_malformed_private_vectors() {
    let f = Fixture::new("scope");
    f.add(episode("secret", "person/alice", Scope::Persons { ids: vec!["alice".into()] }, 1.0), &[1.0, 0.0]);
    f.add(episode("self", "self", Scope::SelfOnly, 1.0), &[]);
    f.add(episode("public", "project/shared", Scope::Public, 0.1), &[1.0, 0.0]);
    for (ns, audience) in [("group/g", Audience::group("g", ["alice".into(), "bob".into()])),
        ("person/bob", Audience::private("bob"))] {
        let ns = ns.parse().unwrap();
        let q = Query { ns: &ns, audience: &audience, partners: &[], max_episodes: 5, max_facts: 0,
            include_lessons: false, half_life_days: 30.0 };
        let r = recall_semantic_at(&f.store, &q, &config(), &[1.0, 0.0], now()).unwrap();
        assert_eq!(ids(&r), ["public"]);
    }
}

#[test]
fn model_change_and_no_hits_preserve_baseline_and_archived_sources_stay_out() {
    let f = Fixture::new("archive");
    f.add(episode("own", "person/a", Scope::Public, 0.1), &[1.0, 0.0]);
    f.add(episode("original", "project/rust", Scope::Public, 1.0), &[1.0, 0.0]);
    let mut archive = episode("archive", "project/rust", Scope::Public, 0.5);
    archive.sources = vec!["original".into()];
    f.store.archive_episode(&archive).unwrap();
    f.store.save_embedding(&EpisodeEmbedding { episode_id: archive.id, model_id: config().model_id(), vector: vec![1.0, 0.0] }).unwrap();
    let ns = "person/a".parse().unwrap();
    let audience = Audience::private("a");
    let q = Query { ns: &ns, audience: &audience, partners: &[], max_episodes: 5, max_facts: 0,
        include_lessons: false, half_life_days: 30.0 };
    assert_eq!(ids(&recall_semantic_at(&f.store, &q, &config(), &[1.0, 0.0], now()).unwrap()), ["own", "archive"]);
    let baseline = recall_at(&f.store, &q, now()).unwrap();
    let mut cfg = config();
    cfg.model = "v2".into();
    assert_eq!(render(&recall_semantic_at(&f.store, &q, &cfg, &[1.0, 0.0], now()).unwrap()), render(&baseline));
    assert_eq!(render(&recall_semantic_at(&f.store, &q, &config(), &[0.0, 1.0], now()).unwrap()), render(&baseline));
}
