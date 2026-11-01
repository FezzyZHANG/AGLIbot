//! The same contract runs against JSONL and (explicitly opt-in) MongoDB.

use super::*;
use chrono::Duration;
use serde_json::json;

pub(super) fn time() -> DateTime<Utc> {
    "2026-10-31T12:00:00.123456789Z".parse().unwrap()
}

pub(super) fn message(ns: &str, text: &str, ts: DateTime<Utc>) -> MessageRecord {
    MessageRecord {
        ts,
        platform: "telegram".into(),
        chat_id: "1".into(),
        sender: "owner".into(),
        sender_name: "主人".into(),
        outgoing: false,
        text: text.into(),
        ns: ns.parse().unwrap(),
        scope: Scope::Persons { ids: vec!["owner".into()] },
    }
}

pub(super) fn call(ts: DateTime<Utc>, cost_usd: f64) -> ModelCallRecord {
    ModelCallRecord {
        ts,
        scenario: "chat".into(),
        model_id: "model-a".into(),
        session_id: "session".into(),
        uncached_input: u64::MAX,
        cached_input: 2,
        cache_write: 3,
        output: 4,
        latency_ms: 5,
        cost_usd,
        ok: false,
        error: Some("failed".into()),
        escalated_from: Some("model-b".into()),
    }
}

fn same<T: Serialize>(got: &T, expected: &T) {
    assert_eq!(serde_json::to_value(got).unwrap(), serde_json::to_value(expected).unwrap());
}

pub(super) fn contract(store: &dyn Store) {
    let ts = time();
    let ns: Namespace = "person/a".parse().unwrap();
    assert!(store.recent_in_ns(&ns, 10).unwrap().is_empty());
    assert!(store.messages_after(&ns, ts).unwrap().is_empty());
    assert!(store.active_namespaces(ts).unwrap().is_empty());
    assert_eq!(store.spent_since(ts).unwrap(), 0.0);
    assert!(store.persons().unwrap().is_empty());
    assert!(store.episodes().unwrap().is_empty());
    assert!(store.lessons().unwrap().is_empty());
    assert!(store.reviews().unwrap().is_empty());
    assert_eq!(store.next_review_id().unwrap(), 1);
    assert_eq!(store.next_review_id().unwrap(), 1); // Reading does not reserve an id.
    assert_eq!(store.kv_get("missing").unwrap(), None);

    // Deliberately out of timestamp order, with equal timestamps and sub-millisecond boundaries.
    let messages = [
        message("person/a", "later first", ts + Duration::days(1)),
        message("person/a", "before", ts - Duration::nanoseconds(1)),
        message("group/g", "other namespace", ts + Duration::nanoseconds(1)),
        message("person/a", "at", ts),
        message("person/a", "after", ts + Duration::nanoseconds(1)),
        message("person/a", "equal", ts),
        message("self", "self", ts + Duration::nanoseconds(1)),
        message("project/p", "project", ts + Duration::nanoseconds(1)),
    ];
    for m in &messages {
        store.append_message(m).unwrap();
    }
    same(&store.recent_in_ns(&ns, 3).unwrap(), &vec![messages[3].clone(), messages[4].clone(), messages[5].clone()]);
    assert!(store.recent_in_ns(&ns, 0).unwrap().is_empty());
    same(
        &store.recent_in_ns(&ns, usize::MAX).unwrap(),
        &vec![messages[0].clone(), messages[1].clone(), messages[3].clone(), messages[4].clone(), messages[5].clone()],
    );
    same(&store.messages_after(&ns, ts).unwrap(), &vec![messages[0].clone(), messages[4].clone()]);
    assert_eq!(
        store.active_namespaces(ts).unwrap(),
        vec![Namespace::SelfNs, ns, Namespace::Group("g".into()), Namespace::Project("p".into())]
    );
    assert!(store.messages_after(&"person/absent".parse().unwrap(), ts).unwrap().is_empty());
    assert!(store.active_namespaces(ts + Duration::days(1)).unwrap().is_empty());

    for c in [call(ts - Duration::nanoseconds(1), 10.0), call(ts, 0.25), call(ts + Duration::nanoseconds(1), 0.5)] {
        store.append_model_call(&c).unwrap();
    }
    assert_eq!(store.spent_since(ts).unwrap(), 0.75);
    assert_eq!(store.spent_since(ts + Duration::nanoseconds(1)).unwrap(), 0.5);
    assert_eq!(store.spent_since(ts + Duration::days(1)).unwrap(), 0.0);

    let e = Episode {
        id: "same-id".into(),
        ts,
        participants: vec!["owner".into()],
        summary: "event".into(),
        emotion: "curious".into(),
        salience: 0.5,
        ns: Namespace::SelfNs,
        scope: Scope::SelfOnly,
    };
    let mut earlier = e.clone();
    earlier.ts -= Duration::days(1);
    store.append_episode(&e).unwrap();
    store.append_episode(&earlier).unwrap();
    same(&store.episodes().unwrap(), &vec![e, earlier]); // Append, even with duplicate ids.

    let l = Lesson {
        id: "lesson".into(),
        ts,
        text: "lesson".into(),
        scope: Scope::SelfOnly,
        sources: vec!["same-id".into()],
    };
    let mut l2 = l.clone();
    l2.text = "second".into();
    l2.ts -= Duration::days(1);
    store.append_lesson(&l).unwrap();
    store.append_lesson(&l2).unwrap();
    same(&store.lessons().unwrap(), &vec![l, l2]);

    let mut person = Person {
        person_id: "p.z".into(),
        platform_ids: vec!["qq:1".into()],
        name: "old".into(),
        kind: PersonKind::Human,
        tier: Tier::Owner,
        trust: 0.5,
        intimacy: 0.25,
        facts: vec![Fact { text: "fact".into(), ns: Namespace::SelfNs, scope: Scope::SelfOnly, ts }],
        last_seen: ts,
    };
    store.save_person(&person).unwrap();
    let mut other = person.clone();
    other.person_id = "$other".into();
    store.save_person(&other).unwrap();
    person.name = "new".into();
    person.facts.clear();
    store.save_person(&person).unwrap();
    same(
        &store.persons().unwrap(),
        &BTreeMap::from([(other.person_id.clone(), other), (person.person_id.clone(), person)]),
    );

    let mut review = ReviewItem {
        id: 9,
        ts,
        category: ReviewCategory::Act,
        subject: "subject".into(),
        detail: "detail".into(),
        verdict: "转交主人".into(),
        status: ReviewStatus::Open,
        payload: json!({"$date": "literal", "a.b": u64::MAX}),
        notified: false,
    };
    store.save_review(&review).unwrap();
    let mut lower = review.clone();
    lower.id = 2;
    store.save_review(&lower).unwrap();
    review.status = ReviewStatus::Approved;
    review.payload = Value::Null;
    review.notified = true;
    store.save_review(&review).unwrap();
    same(&store.reviews().unwrap(), &vec![review, lower]);
    assert_eq!(store.next_review_id().unwrap(), 10);
    assert_eq!(store.next_review_id().unwrap(), 10);

    store
        .kv_set("$key.with.dots", json!({"$date": "literal", "a.b": [u64::MAX, true, null], "nul\0key": "值"}))
        .unwrap();
    assert_eq!(
        store.kv_get("$key.with.dots").unwrap(),
        Some(json!({"$date": "literal", "a.b": [u64::MAX, true, null], "nul\0key": "值"}))
    );
    store.kv_set("$key.with.dots", Value::Null).unwrap();
    assert_eq!(store.kv_get("$key.with.dots").unwrap(), Some(Value::Null));
    store.kv_set("", json!([1, "two"])).unwrap();
    assert_eq!(store.kv_get("").unwrap(), Some(json!([1, "two"])));
    store.kv_set("persist", json!({"ok": true})).unwrap();

    store
        .append_audit(&AuditRecord {
            ts,
            requester: "owner".into(),
            tool: "test".into(),
            allowed: false,
            reason: "denied".into(),
            outcome: Outcome::Denied,
        })
        .unwrap();
}

pub(super) fn persisted(store: &dyn Store) {
    let audit = store.audits().unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].outcome, Outcome::Denied);
    assert_eq!(audit[0].reason, "denied");
    assert_eq!(store.recent_in_ns(&"person/a".parse().unwrap(), 100).unwrap().len(), 5);
    assert_eq!(store.episodes().unwrap().len(), 2);
    assert_eq!(store.lessons().unwrap().len(), 2);
    assert_eq!(store.persons().unwrap()["p.z"].name, "new");
    let reviews = store.reviews().unwrap();
    assert_eq!(reviews.iter().map(|r| r.id).collect::<Vec<_>>(), [9, 2]);
    assert_eq!(reviews[0].status, ReviewStatus::Approved);
    assert_eq!(store.next_review_id().unwrap(), 10);
    assert_eq!(store.spent_since(time()).unwrap(), 0.75);
    assert_eq!(store.kv_get("persist").unwrap(), Some(json!({"ok": true})));
    store.append_message(&message("person/a", "after reopen", time())).unwrap();
    assert_eq!(store.recent_in_ns(&"person/a".parse().unwrap(), 1).unwrap()[0].text, "after reopen");
}

#[test]
fn jsonl_store_contract_and_reopen() {
    let dir = std::env::temp_dir().join(format!("aglibot-store-contract-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = jsonl::JsonlStore::open(&dir).unwrap();
    contract(&store);
    drop(store);
    persisted(&jsonl::JsonlStore::open(&dir).unwrap());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn no_mongo_url_selects_jsonl() {
    let dir = std::env::temp_dir().join(format!("aglibot-store-selection-{}", std::process::id()));
    let store = open(&crate::config::MongoCfg::default(), &dir).unwrap();
    store.kv_set("backend", json!("jsonl")).unwrap();
    assert!(dir.join("kv.json").exists());
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_mongo_url_fails_without_fallback_or_secret_in_error() {
    let dir = std::env::temp_dir().join(format!("aglibot-no-fallback-{}", std::process::id()));
    let cfg = crate::config::MongoCfg { url: Some("invalid://username:secret@host".into()), ..Default::default() };
    let error = match open(&cfg, &dir) {
        Err(e) => format!("{e:#}"),
        Ok(_) => panic!("invalid Mongo URL must fail"),
    };
    assert!(error.contains("MongoDB startup"));
    assert!(error.contains("no JSONL fallback"));
    assert!(!error.contains("secret"));
    assert!(!error.contains("username"));
    assert!(!dir.exists());
}
