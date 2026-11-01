use super::*;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use crate::mind::{models::Models, reflect};
use crate::social::people::People;
use crate::store::jsonl::JsonlStore;

struct Fixture {
    dir: PathBuf,
    store: Arc<JsonlStore>,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("aglibot-groups-{name}-{}", std::process::id()));
        let store = Arc::new(JsonlStore::open(&dir).unwrap());
        Self { dir, store }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}

fn message(group: &str, sender: &str, offset: i64) -> MessageRecord {
    MessageRecord {
        ts: "2026-11-02T12:00:00.123456789Z".parse::<DateTime<Utc>>().unwrap() + chrono::Duration::seconds(offset),
        platform: "telegram".into(), chat_id: group.into(), sender: sender.into(), sender_name: sender.into(),
        outgoing: sender == "me", text: format!("message from {sender}"),
        ns: Namespace::Group(group.into()), scope: Scope::Group { id: group.into() },
    }
}

#[tokio::test]
async fn reflection_learns_groups_and_validated_edges_in_the_existing_call() {
    let f = Fixture::new("extract");
    let mut people = People::load(f.store.as_ref()).unwrap();
    let a = message("telegram:-7", "a", 0);
    let b = message("telegram:-7", "b", 1);
    for m in [&a, &b] {
        f.store.append_message(m).unwrap();
    }
    let reply = json!({
        "episodes": [{"summary":"a helped b", "participants":["a","b"]}],
        "group": {"norms":["cite sources"], "topics":["Rust"], "my_role":"observer",
            "chat_id":"other", "members":["invented"], "ns":"group/other", "scope":{"kind":"public"},
            "activity_stats":{"messages":9999}},
        "relations": [
            {"a":"a", "b":"b", "type":"helps", "evidence":[a.ts, b.ts, a.ts], "strength":1.5,
                "ns":"group/other", "scope":{"kind":"public"}},
            {"a":"a", "b":"stranger", "type":"invented person", "evidence":[a.ts]},
            {"a":"a", "b":"a", "type":"self edge", "evidence":[a.ts]},
            {"a":"a", "b":"b", "type":"invented evidence", "evidence":[message("g", "a", 99).ts]},
            {"a":"a", "b":"b", "type":"no evidence", "evidence":[]},
            {"a":"a", "b":"b", "type":"  ", "evidence":[a.ts]}
        ]
    });
    let models = Models::stub(f.store.clone(), vec![format!("```json\n{reply}\n```")]);
    let outcome = reflect::extract_ns(&models, f.store.as_ref(), &mut people, &a.ns).await.unwrap();
    assert_eq!((outcome.messages, outcome.episodes), (2, 1));
    let prompts = models.stub_prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].2.contains("relations:") && prompts[0].2.contains("commitments:"));
    let group = f.store.group("telegram:-7").unwrap().unwrap();
    assert_eq!((group.ns, group.scope), (a.ns.clone(), a.scope.clone()));
    assert_eq!(group.members, ["a", "b"]);
    assert_eq!(group.norms, ["cite sources"]);
    assert_eq!(group.topics, ["Rust"]);
    assert_eq!(group.my_role, "observer");
    assert_eq!(group.activity_stats.messages, 2);
    assert_eq!(group.activity_stats.last_seen, b.ts);
    assert!(f.store.group("other").unwrap().is_none());
    let edges = f.store.relations(&a.ns).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].evidence, [a.ts, b.ts]);
    assert_eq!(edges[0].strength, 1.0);
    assert_eq!((&edges[0].ns, &edges[0].scope), (&a.ns, &a.scope));
    assert_eq!(reflect::watermark(f.store.as_ref(), &a.ns).unwrap(), b.ts);
    assert_eq!(reflect::extract_ns(&models, f.store.as_ref(), &mut people, &a.ns).await.unwrap().messages, 0);
    assert_eq!(models.stub_prompts().len(), 1);

    let a2 = message("telegram:-7", "a", 2);
    let b2 = message("telegram:-7", "b", 3);
    for m in [&a2, &b2] {
        f.store.append_message(m).unwrap();
    }
    let reply = json!({"group":{"norms":["cite sources","short posts"], "topics":["tests"]},
        "relations":[{"a":"a","b":"b","type":"helps","evidence":[a2.ts],"strength":0.8}]}).to_string();
    let models = Models::stub(f.store.clone(), vec![reply]);
    reflect::extract_ns(&models, f.store.as_ref(), &mut people, &a.ns).await.unwrap();
    let group = f.store.group("telegram:-7").unwrap().unwrap();
    assert_eq!(group.norms, ["cite sources", "short posts"]);
    assert_eq!(group.topics, ["tests", "Rust"]);
    assert_eq!(group.my_role, "observer");
    assert_eq!(group.activity_stats.messages, 4);
    assert_eq!(group.activity_stats.first_seen, a.ts);
    assert_eq!(f.store.relations(&a.ns).unwrap()[0].evidence, [a.ts, b.ts, a2.ts]);
}

#[tokio::test]
async fn private_or_mixed_scope_batches_cannot_produce_group_observations() {
    let f = Fixture::new("scope");
    let mut people = People::load(f.store.as_ref()).unwrap();
    let reply = json!({"group":{"norms":["SECRET"]},
        "relations":[{"a":"a","b":"b","type":"SECRET","evidence":[message("g","a",0).ts]}]}).to_string();
    for (ns, restricted) in [(Namespace::Person("a".into()), Scope::Persons { ids: vec!["a".into()] }),
        (Namespace::Group("g".into()), Scope::SelfOnly)] {
        let a = MessageRecord { ns: ns.clone(), scope: restricted, ..message("g", "a", 0) };
        let b = MessageRecord { ns: ns.clone(), ..message("g", "b", 1) };
        for m in [&a, &b] {
            f.store.append_message(m).unwrap();
        }
        let models = Models::stub(f.store.clone(), vec![reply.clone()]);
        reflect::extract_ns(&models, f.store.as_ref(), &mut people, &ns).await.unwrap();
        assert!(f.store.group("g").unwrap().is_none());
        assert!(f.store.relations(&ns).unwrap().is_empty());
        if matches!(ns, Namespace::Person(_)) {
            assert!(!models.stub_prompts()[0].2.contains("relations:"));
        } else {
            assert_eq!(models.stub_prompts()[0].1, crate::ns::ProcessingPolicy::LocalOnly);
        }
    }
}

#[test]
fn cards_are_capped_and_isolated_even_for_shared_members() {
    let f = Fixture::new("cards");
    for id in ["telegram:-7", "telegram:-8"] {
        let msgs = [message(id, "a", 0), message(id, "b", 1)];
        let raw = json!({
            "group":{"norms": (0..20).map(|i| format!("norm-{id}-{i}\n{}", "x".repeat(500))).collect::<Vec<_>>(),
                "topics":(0..20).map(|i| format!("topic-{i}")).collect::<Vec<_>>(), "my_role":"observer\nnot authority"},
            "relations": (0..10).map(|i| json!({"a":"a","b":"b","type":format!("edge-{id}-{i}"),"evidence":[msgs[0].ts]})).collect::<Vec<_>>()
        });
        apply(f.store.as_ref(), &msgs[0].ns, &msgs, serde_json::from_value(raw).unwrap()).unwrap();
        assert_eq!(f.store.group(id).unwrap().unwrap().norms.len(), 8);
    }
    let ns = Namespace::Group("telegram:-7".into());
    let audience = Audience::group("telegram:-7", ["a".into(), "b".into()]);
    let mut card = "HAND-WRITTEN\nkeep exactly".to_string();
    append_card(&mut card, f.store.as_ref(), &ns, &audience).unwrap();
    assert!(card.starts_with("HAND-WRITTEN\nkeep exactly\n\n## 群聊观察"));
    let learned = card.split("## 群聊观察").nth(1).unwrap();
    assert_eq!(learned.lines().count(), 13);
    assert!(learned.lines().all(|line| line.chars().count() <= 190));
    assert!(learned.contains("norm-telegram:-7") && learned.contains("edge-telegram:-7"));
    assert!(!learned.contains("telegram:-8") && !learned.contains("topic-3"));
    assert!(learned.contains("以手写内容为准"));
    for wrong in [Audience::group("telegram:-8", ["a".into()]), Audience::private("a"), Audience::solo()] {
        let mut empty = String::new();
        append_card(&mut empty, f.store.as_ref(), &ns, &wrong).unwrap();
        assert!(empty.is_empty());
    }
    let mut group = f.store.group("telegram:-7").unwrap().unwrap();
    group.scope = Scope::SelfOnly;
    f.store.save_group(&group).unwrap();
    let mut empty = String::new();
    append_card(&mut empty, f.store.as_ref(), &ns, &audience).unwrap();
    assert!(empty.is_empty());
}

#[tokio::test]
async fn restricted_agenda_context_cannot_taint_group_observations() {
    use crate::mind::commitments;
    let f = Fixture::new("agenda-scope");
    let mut people = People::load(f.store.as_ref()).unwrap();
    let a = message("telegram:-7", "a", 1);
    let private = MessageRecord { scope: Scope::Persons { ids: vec!["a".into()] }, ..message("telegram:-7", "me", 0) };
    let raw = serde_json::from_value(json!({"commitments":[{"who":"me","what":"PRIVATE_AGENDA", "evidence":private.ts}]})).unwrap();
    commitments::apply(f.store.as_ref(), &a.ns, &[private], raw, a.ts).unwrap();
    f.store.append_message(&a).unwrap();
    let models = Models::stub(f.store.clone(), vec![json!({"group":{"norms":["PRIVATE_AGENDA"]}}).to_string()]);
    reflect::extract_ns(&models, f.store.as_ref(), &mut people, &a.ns).await.unwrap();
    assert!(models.stub_prompts()[0].2.contains("PRIVATE_AGENDA"));
    assert!(!models.stub_prompts()[0].2.contains("relations:"));
    assert!(f.store.group("telegram:-7").unwrap().is_none());
}
