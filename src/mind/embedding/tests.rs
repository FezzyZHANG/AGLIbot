use super::*;
use std::sync::Arc;
use chrono::Utc;
use serde_json::json;
use crate::ns::Scope;
use crate::store::{jsonl::JsonlStore, ModelCallRecord};

pub(crate) fn config() -> EmbeddingCfg {
    serde_yaml::from_str("provider: test\nmodel: embed-v1").unwrap()
}

pub(crate) fn reply(vector: &[f32]) -> String {
    json!({"data": [{"index": 0, "embedding": vector}], "usage": {"prompt_tokens": 10}}).to_string()
}

struct Fixture {
    dir: std::path::PathBuf,
    store: Arc<JsonlStore>,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("aglibot-embedding-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        Self { store: Arc::new(JsonlStore::open(&dir).unwrap()), dir }
    }

    fn calls(&self) -> Vec<ModelCallRecord> {
        std::fs::read_to_string(self.dir.join("model_calls.jsonl")).unwrap_or_default()
            .lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { std::fs::remove_dir_all(&self.dir).unwrap(); }
}

fn episode(id: &str, scope: Scope) -> Episode {
    Episode { id: id.into(), ts: Utc::now(), participants: vec![], summary: format!("summary {id}"),
        emotion: String::new(), salience: 0.5, ns: "person/a".parse().unwrap(), scope, sources: vec![] }
}

#[test]
fn defaults_and_bad_vectors() {
    let cfg = config();
    assert_eq!((cfg.max_cross_namespace, cfg.threshold, cfg.nightly_cap), (3, 0.75, 200));
    assert!(!cfg.local);
    cfg.validate().unwrap();
    let models: crate::config::ModelsCfg = serde_yaml::from_str(include_str!("../../../config/models.yaml")).unwrap();
    assert!(models.embedding.is_none());
    for (a, b) in [(vec![], vec![]), (vec![0.0], vec![1.0]), (vec![f32::NAN], vec![1.0]),
        (vec![f32::INFINITY], vec![1.0]), (vec![1.0], vec![1.0, 0.0])] {
        assert!(cosine(&a, &b).is_err());
    }
    assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]).unwrap(), 0.0);
    assert!((cosine(&[f32::MAX, f32::MAX], &[f32::MAX, f32::MAX]).unwrap() - 1.0).abs() < 1e-12);
    for value in [json!({}), json!({"data": []}), json!({"data": [{"index": 1, "embedding": [1.0]}]}),
        json!({"data": [{"index": 0, "embedding": [0.0]}]}),
        json!({"data": [{"index": 0, "embedding": [1e100]}]}),
        json!({"data": [{"index": 0, "embedding": "base64"}]})] {
        assert!(crate::mind::llm::parse_embedding(&value).is_err());
    }
}

#[tokio::test]
async fn off_and_processing_policies_never_send_disallowed_text() {
    let f = Fixture::new();
    let off = Models::stub(f.store.clone(), vec![]);
    assert!(off.embed(&ProcessingPolicy::Any, "off").await.unwrap().is_none());
    assert!(off.stub_prompts().is_empty());
    let remote = Models::stub(f.store.clone(), vec![reply(&[1.0, 0.0])]).with_embedding(config());
    let secret = episode("secret", Scope::SelfOnly);
    assert_eq!(fill(&remote, f.store.as_ref(), &[secret.clone(), episode("public", Scope::Public)], 10).await.unwrap(), 1);
    assert_eq!(remote.stub_prompts().len(), 1);
    assert_eq!(remote.stub_prompts()[0].2, "summary public");
    let denied = ProcessingPolicy::Providers { names: vec!["other".into()] };
    assert!(remote.embed(&denied, "provider secret").await.unwrap().is_none());
    assert!(remote.embed(&ProcessingPolicy::LocalOnly, "local secret").await.unwrap().is_none());
    assert_eq!(remote.stub_prompts().len(), 1);
    let mut local_cfg = config();
    local_cfg.local = true;
    let local = Models::stub(f.store.clone(), vec![reply(&[1.0, 0.0]), reply(&[0.0, 1.0])]).with_embedding(local_cfg);
    assert_eq!(fill(&local, f.store.as_ref(), &[secret], 10).await.unwrap(), 1);
    assert_eq!(local.stub_prompts()[0].1, ProcessingPolicy::LocalOnly);
    assert!(local.embed(&denied, "still denied locally").await.unwrap().is_none());
    assert!(local.embed(&ProcessingPolicy::Providers { names: vec!["test".into()] }, "permitted").await.unwrap().is_some());
    assert_eq!(f.calls().len(), 3);
}

#[tokio::test]
async fn nightly_cap_model_change_archives_and_failure() {
    let f = Fixture::new();
    let original = episode("original", Scope::Public);
    f.store.append_episode(&original).unwrap();
    let mut summary = episode("archive", Scope::Public);
    summary.sources = vec![original.id];
    f.store.archive_episode(&summary).unwrap();
    for id in ["b", "c"] { f.store.append_episode(&episode(id, Scope::Public)).unwrap(); }
    let mut cfg = config();
    cfg.nightly_cap = 2;
    let models = Models::stub(f.store.clone(), vec![reply(&[1.0, 0.0]); 3]).with_embedding(cfg.clone());
    nightly(&models, f.store.as_ref()).await;
    let cached = f.store.load_embeddings(&cfg.model_id()).unwrap();
    assert_eq!(cached.len(), 2);
    assert!(cached.contains_key("archive") && !cached.contains_key("original"));
    assert_eq!(models.stub_prompts().len(), 2);
    nightly(&models, f.store.as_ref()).await;
    assert_eq!(models.stub_prompts().len(), 3);
    nightly(&models, f.store.as_ref()).await;
    assert_eq!(models.stub_prompts().len(), 3);

    cfg.model = "embed-v2".into();
    assert!(f.store.load_embeddings(&cfg.model_id()).unwrap().is_empty());
    let changed = Models::stub(f.store.clone(), vec![reply(&[1.0, 0.0]); 2]).with_embedding(cfg.clone());
    nightly(&changed, f.store.as_ref()).await;
    assert_eq!(changed.stub_prompts().len(), 2);
    assert_eq!(f.store.load_embeddings(&cfg.model_id()).unwrap().len(), 2);

    let failed = Models::stub(f.store.clone(), vec!["bad response".into()]).with_embedding(cfg.clone());
    nightly(&failed, f.store.as_ref()).await;
    assert_eq!(f.store.load_embeddings(&cfg.model_id()).unwrap().len(), 2);
    assert_eq!(failed.stub_prompts().len(), 1);
    let calls = f.calls();
    assert_eq!(calls.len(), 6);
    assert!(calls.iter().all(|call| call.scenario == "embed"));
    assert!(!calls.last().unwrap().ok);
    assert_eq!(calls.last().unwrap().model_id, cfg.model_id());
}

#[tokio::test]
async fn embedding_usage_charges_budget_and_stops_filling() {
    let f = Fixture::new();
    let mut cfg = config();
    cfg.input_usd_per_million = 100_000.0; // Ten tokens consume the stub's $1 budget.
    let models = Models::stub(f.store.clone(), vec![reply(&[1.0])]).with_embedding(cfg);
    let episodes = [episode("a", Scope::Public), episode("b", Scope::Public)];
    assert_eq!(fill(&models, f.store.as_ref(), &episodes, 200).await.unwrap(), 1);
    assert_eq!(models.stub_prompts().len(), 1);
    let call = &f.calls()[0];
    assert!(call.ok);
    assert_eq!((call.uncached_input, call.cost_usd), (10, 1.0));
    assert!(models.budget_exhausted());
}

#[tokio::test]
async fn newly_extracted_episodes_are_cached_without_repeating_extraction_on_failure() {
    let f = Fixture::new();
    let mut people = crate::social::people::People::load(f.store.as_ref()).unwrap();
    let msg = crate::store::MessageRecord { ts: Utc::now(), platform: "telegram".into(), chat_id: "a".into(),
        sender: "a".into(), sender_name: "a".into(), outgoing: false, text: "hello".into(),
        ns: "person/a".parse().unwrap(), scope: Scope::Persons { ids: vec!["a".into()] } };
    f.store.append_message(&msg).unwrap();
    let raw = json!({"episodes": [{"summary": "new memory"}, {"summary": "second memory"}]}).to_string();
    let models = Models::stub(f.store.clone(), vec![raw, reply(&[1.0]), "bad response".into()]).with_embedding(config());
    let extracted = crate::mind::reflect::extract_ns(&models, f.store.as_ref(), &mut people, &msg.ns).await.unwrap();
    assert_eq!(extracted.episodes, 2);
    assert_eq!(f.store.load_embeddings(&config().model_id()).unwrap().len(), 1);
    assert_eq!(models.stub_prompts()[1].2, "new memory");
    assert_eq!(crate::mind::reflect::extract_ns(&models, f.store.as_ref(), &mut people, &msg.ns).await.unwrap().messages, 0);
    assert_eq!(models.stub_prompts().len(), 3);
}
