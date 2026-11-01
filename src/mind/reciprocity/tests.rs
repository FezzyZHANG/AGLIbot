use super::*;
use crate::ns::Namespace;
use crate::store::{Episode, jsonl::JsonlStore};
use std::sync::Arc;

#[tokio::test]
async fn deltas_clamp_owner_local_empty_and_persisted_nightly_gate() {
    let dir = std::env::temp_dir().join(format!("agli-reciprocity-{}-{}", std::process::id(), Utc::now().timestamp_nanos_opt().unwrap()));
    let store = Arc::new(JsonlStore::open(&dir).unwrap());
    let mut people = People::load(store.as_ref()).unwrap();
    for (id, tier) in [("owner", Tier::Owner), ("alice", Tier::Acquaintance), ("empty", Tier::Groupmate), ("secret", Tier::Stranger)] {
        people.ensure(store.as_ref(), id, id, tier).unwrap();
    }
    let mut alice = people.get("alice").unwrap().clone();
    alice.trust = 0.98;
    alice.intimacy = 0.02;
    people.save(store.as_ref(), alice).unwrap();
    let now: DateTime<Utc> = "2026-11-01T20:00:00Z".parse().unwrap();
    let date = now.date_naive();
    for id in ["owner", "alice", "secret"] {
        store.append_episode(&Episode { id: id.into(), ts: now, participants: vec![id.into()],
            summary: format!("{id} helped me"), emotion: "unexposed tone metadata".into(), salience: 0.5,
            ns: Namespace::Person(id.into()), scope: if id == "secret" { Scope::SelfOnly } else { Scope::Public }, sources: vec![] }).unwrap();
    }
    let models = Models::stub(store.clone(), vec![r#"{"dtrust":0.8,"dintimacy":-0.9,"why":"help and tone"}"#.into(),
        r#"{"dtrust":-0.8,"dintimacy":0.9,"why":"later evidence"}"#.into()]);
    nightly(&models, store.as_ref(), &mut people, date, now).await.unwrap();
    let alice = people.get("alice").unwrap();
    assert_eq!((alice.trust, alice.intimacy), (1.0, 0.0));
    assert_eq!(alice.last_update, Some(now));
    for id in ["owner", "empty", "secret"] { assert!(people.get(id).unwrap().last_update.is_none()); }
    assert_eq!(models.stub_prompts().len(), 1);
    assert!(!models.stub_prompts()[0].2.contains("unexposed tone metadata"));
    // Restart, then even NEW same-night evidence cannot spend another delta.
    let reopened = JsonlStore::open(&dir).unwrap();
    let mut people = People::load(&reopened).unwrap();
    let mut later = reopened.all_episodes().unwrap().into_iter().find(|e| e.id == "alice").unwrap();
    later.id = "later".into();
    later.ts = now + chrono::Duration::minutes(1);
    reopened.append_episode(&later).unwrap();
    nightly(&models, &reopened, &mut people, date, later.ts).await.unwrap();
    assert_eq!(models.stub_prompts().len(), 1);
    nightly(&models, &reopened, &mut people, date.succ_opt().unwrap(), later.ts).await.unwrap();
    let alice = people.get("alice").unwrap();
    assert!((alice.trust - 0.95).abs() < 1e-12 && (alice.intimacy - 0.05).abs() < 1e-12);
    nightly(&models, &reopened, &mut people, date + chrono::Duration::days(2), later.ts).await.unwrap();
    assert_eq!(models.stub_prompts().len(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn caps_summaries_and_rejects_bad_json_without_advancing_watermark() {
    let dir = std::env::temp_dir().join(format!("agli-reciprocity-cap-{}-{}", std::process::id(), Utc::now().timestamp_nanos_opt().unwrap()));
    let store = Arc::new(JsonlStore::open(&dir).unwrap());
    let mut people = People::load(store.as_ref()).unwrap();
    people.ensure(store.as_ref(), "a", "a", Tier::Acquaintance).unwrap();
    let now = Utc::now();
    for i in 0..25 {
        store.append_episode(&Episode { id: i.to_string(), ts: now - chrono::Duration::minutes(i), participants: vec!["a".into()],
            summary: format!("evidence-{i:02}"), emotion: String::new(), salience: 0.5,
            ns: Namespace::Person("a".into()), scope: Scope::Public, sources: vec![] }).unwrap();
    }
    let models = Models::stub(store.clone(), vec!["invalid".into()]);
    nightly(&models, store.as_ref(), &mut people, now.date_naive(), now).await.unwrap();
    assert!(people.get("a").unwrap().last_update.is_none());
    let prompts = models.stub_prompts();
    assert_eq!(prompts[0].2.matches("evidence-").count(), 20);
    assert!(prompts[0].2.contains("evidence-00") && !prompts[0].2.contains("evidence-20"));
    std::fs::remove_dir_all(dir).unwrap();
}
