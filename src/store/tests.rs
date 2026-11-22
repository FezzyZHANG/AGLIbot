use chrono::TimeDelta;
use serde_json::json;

use super::*;

pub(super) fn time() -> DateTime<Utc> {
    "2026-11-01T10:00:00.123456789Z".parse().unwrap()
}

pub(super) fn message(ns: &str, text: &str, ts: DateTime<Utc>) -> MessageRecord {
    MessageRecord {
        ts,
        platform: "qq".into(),
        chat_id: "7".into(),
        sender: "qq:8".into(),
        sender_name: "name".into(),
        outgoing: false,
        text: text.into(),
        ns: ns.parse().unwrap(),
        scope: Scope::Public,
    }
}

pub(super) fn call(ts: DateTime<Utc>, cost_usd: f64) -> ModelCallRecord {
    ModelCallRecord {
        ts,
        scenario: "chat".into(),
        model_id: "model".into(),
        session_id: "session".into(),
        uncached_input: i64::MAX as u64,
        cached_input: 2,
        cache_write: 3,
        output: 4,
        latency_ms: 5,
        cost_usd,
        ok: false,
        error: Some("request failed".into()),
        escalated_from: None,
    }
}

pub(super) fn episode() -> Episode {
    Episode {
        id: "e1".into(),
        ts: time(),
        participants: vec!["qq:8".into()],
        summary: "memory".into(),
        emotion: "happy".into(),
        salience: 0.5,
        ns: Namespace::SelfNs,
        scope: Scope::SelfOnly,
    }
}

pub(super) fn person() -> Person {
    Person {
        person_id: "qq:8".into(),
        platform_ids: vec!["qq:8".into()],
        name: "name".into(),
        kind: PersonKind::Human,
        tier: Tier::Owner,
        trust: 0.5,
        intimacy: 0.5,
        last_seen: time(),
        last_update: Some(time()),
        last_update_day: Some(time().date_naive()),
        facts: vec![Fact {
            text: "fact".into(),
            ns: Namespace::SelfNs,
            scope: Scope::SelfOnly,
            ts: time(),
        }],
        groups: vec!["qq:7".into()],
    }
}

pub(super) fn audit() -> AuditRecord {
    AuditRecord {
        ts: time(),
        requester: "owner".into(),
        tool: "send".into(),
        allowed: true,
        reason: "test".into(),
        outcome: Outcome::Sent,
    }
}

pub(super) fn json_payload() -> Value {
    json!({"$date": {"$numberLong": "123"}, "ts": "literal", "a.b": [null, 1, u64::MAX, {"$oid": "literal"}]})
}

pub(super) fn review() -> NewReview {
    NewReview {
        ts: time(),
        category: ReviewCategory::Act,
        subject: "subject".into(),
        detail: "detail".into(),
        verdict: "review".into(),
        status: ReviewStatus::Open,
        payload: json_payload(),
        notified: false,
        urgent: true,
        pushed: false,
    }
}

pub(super) fn media() -> MediaRecord {
    MediaRecord {
        hash: "hash".into(),
        ext: "png".into(),
        status: MediaStatus::Seen,
        keep: true,
        description: "original".into(),
        text: "image text".into(),
        use_when: "often".into(),
        avoid: "rarely".into(),
        tags: vec!["original".into()],
        scope: StickerScope::Origin,
        scope_reason: "private".into(),
        origin: "qq:8".into(),
        first_seen: time(),
        last_seen: time(),
        sightings: 1,
        sent_count: 0,
        last_sent: None,
    }
}

pub(super) fn vector() -> StickerVector {
    StickerVector {
        hash: "hash".into(),
        model_id: "model".into(),
        text_hash: "text".into(),
        vector: vec![0.5, -0.25],
    }
}

async fn task_contract(store: &Store) -> Result<()> {
    let first: TaskRecord = serde_json::from_value(json!({
        "id": "first", "title": "first", "kind": "research", "status": "active",
        "requester": "owner", "created": truncate_ms(time()), "updated": truncate_ms(time()),
    }))?;
    let next = truncate_ms(time()) + TimeDelta::seconds(1);
    let second = TaskRecord { id: "second".into(), created: next, updated: next, status: TaskStatus::Inbox, ..first.clone() };
    store.save_task(&second).await?;
    store.save_task(&first).await?;
    assert_eq!(store.tasks().await?.into_iter().map(|t| t.id).collect::<Vec<_>>(), ["first", "second"]);
    let thread = ThreadRef { platform: "qq".into(), chat_id: "1".into(), message_id: Some("2".into()) };
    let updates = (0..12).map(|n| {
        let store = store.clone();
        let thread = thread.clone();
        tokio::spawn(async move {
            store.append_task_followup("first", &format!("update {n}"), thread, next).await
        })
    }).collect::<Vec<_>>();
    for update in updates { update.await??; }
    store.mark_task_followups_consumed("first", 4, 2).await?;
    store.mark_task_followups_consumed("first", 6, 1).await?;
    assert_eq!(store.recover_active_tasks(next).await?, ["first"]);
    let task = store.task("first").await?.unwrap();
    assert_eq!(task.followups.iter().map(|f| f.seq).collect::<Vec<_>>(), (1..=12).collect::<Vec<_>>());
    assert_eq!(task.followups[3].consumed_round, Some(2));
    assert_eq!(task.followups[4].consumed_round, Some(1));
    assert_eq!(task.current_round, 2);
    assert_eq!(task.unconsumed_followups().len(), 6);
    assert_eq!(task.status, TaskStatus::Inbox);
    assert_eq!(task.note, RECOVERY_NOTE);
    assert_eq!(task.threads, [thread]);
    assert!(store.recover_active_tasks(next).await?.is_empty());
    Ok(())
}

pub(super) async fn contract(store: &Store) -> Result<()> {
    task_contract(store).await?;
    let t = time();
    let ms = truncate_ms(t);
    let group = "group/qq:7".parse()?;
    let person_ns = "person/qq:8".parse()?;
    let later = ms + TimeDelta::milliseconds(1);
    for (ns, text, ts) in [
        ("group/qq:7", "later", later),
        ("group/qq:7", "first", t),
        ("person/qq:8", "other", t),
        ("group/qq:7", "second", ms),
    ] {
        store.append_message(&message(ns, text, ts)).await?;
    }
    let texts =
        |messages: Vec<MessageRecord>| messages.into_iter().map(|m| m.text).collect::<Vec<_>>();
    assert!(store.recent_in_ns(&group, 0).await?.is_empty());
    assert_eq!(
        texts(store.recent_in_ns(&group, 2).await?),
        ["second", "later"]
    );
    assert_eq!(texts(store.messages_after(&group, t).await?), ["later"]);
    assert_eq!(
        texts(
            store
                .messages_after(&group, ms - TimeDelta::nanoseconds(1))
                .await?
        ),
        ["first", "second", "later"]
    );
    assert_eq!(store.recent_in_ns(&person_ns, 1).await?[0].ts, ms);
    assert_eq!(
        store.active_namespaces(t).await?,
        std::slice::from_ref(&group)
    );
    assert_eq!(
        store
            .active_namespaces(ms - TimeDelta::milliseconds(1))
            .await?,
        [person_ns, group]
    );

    store.append_model_call(&call(t, 0.25)).await?;
    store.append_model_call(&call(later, 0.5)).await?;
    assert_eq!(store.spent_since(t).await?, 0.75);
    assert_eq!(store.spent_since(later).await?, 0.5);
    assert_eq!(
        store
            .spent_since(later + TimeDelta::milliseconds(1))
            .await?,
        0.0
    );
    assert_eq!(store.model_calls().await?[0].ts, ms);
    assert!(
        store
            .append_model_call(&ModelCallRecord {
                output: u64::MAX,
                ..call(t, 0.0)
            })
            .await
            .is_err()
    );

    store.append_episode(&episode()).await?;
    assert_eq!(store.episodes().await?[0].ts, ms);
    let mut person = person();
    store.save_person(&person).await?;
    person.name = "edited".into();
    store.save_person(&person).await?;
    let persons = store.persons().await?;
    let stored = &persons[&person.person_id];
    assert_eq!(persons.len(), 1);
    assert_eq!(stored.name, "edited");
    assert_eq!(stored.last_seen, ms);
    assert_eq!(stored.last_update, Some(ms));
    assert_eq!(stored.facts[0].ts, ms);
    store.append_audit(&audit()).await?;
    assert_eq!(store.audits().await?[0].ts, ms);

    let mut writes = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let store = store.clone();
        writes.spawn(async move { store.create_review(review()).await });
    }
    let mut ids = Vec::new();
    while let Some(result) = writes.join_next().await {
        ids.push(result??.id);
    }
    ids.sort_unstable();
    assert_eq!(ids, (1..=16).collect::<Vec<_>>());
    let mut reviews = store.reviews().await?;
    assert_eq!(reviews.len(), 16);
    let item = &mut reviews[0];
    assert_eq!(item.ts, ms);
    assert_eq!(item.payload, json_payload());
    item.status = ReviewStatus::Approved;
    store.save_review(item).await?;
    assert_eq!(store.reviews().await?[0].status, ReviewStatus::Approved);
    item.id = 99;
    assert!(store.save_review(item).await.is_err());

    assert!(store.kv_get("key").await?.is_none());
    store.kv_set("key", Value::Null).await?;
    assert_eq!(store.kv_get("key").await?, Some(Value::Null));
    store.kv_set("key", json_payload()).await?;
    assert_eq!(store.kv_get("key").await?, Some(json_payload()));

    let entries = [json!({"term": "a"}), json!({"term": "a", "scope": "qq:7"})];
    store.insert_lexicon(&[]).await?;
    store.insert_lexicon(&entries).await?;
    assert!(
        store
            .insert_lexicon(&[json!({"term": "a", "scope": null})])
            .await
            .is_err()
    );
    assert!(store.insert_lexicon(&entries[1..]).await.is_err());
    assert_eq!(store.lexicon().await?, entries);

    media_contract(store).await?;
    Ok(())
}

async fn media_contract(store: &Store) -> Result<()> {
    let record = media();
    let hash = &record.hash;
    let ms = truncate_ms(record.first_seen);
    let later = ms + TimeDelta::milliseconds(1);
    assert!(store.media(hash).await?.is_none());
    assert!(
        store
            .touch_media_sighting(hash, later, None)
            .await?
            .is_none()
    );
    store.record_media_sent(hash, later).await?;
    assert!(
        store
            .patch_media(hash, &MediaEditPatch::default())
            .await?
            .is_none()
    );
    store.save_media(&record).await?;

    let caption = MediaCaptionPatch {
        description: "caption".into(),
        text: "ocr".into(),
        use_when: "use".into(),
        avoid: "avoid".into(),
        tags: vec!["caption".into()],
    };
    assert!(!store.update_media_caption(hash, &caption).await?);
    let edit = MediaEditPatch {
        description: Some("owner edit".into()),
        scope: Some(StickerScope::Any),
        ..Default::default()
    };
    let (sighting, sent, edited) = tokio::join!(
        store.touch_media_sighting(hash, later, Some((MediaStatus::Active, "$group".into()))),
        store.record_media_sent(hash, later),
        store.patch_media(hash, &edit),
    );
    assert!(sighting?.is_some());
    sent?;
    assert!(edited?.is_some());
    let promoted = store.media(hash).await?.unwrap();
    assert_eq!(promoted.status, MediaStatus::Active);
    assert_eq!(promoted.origin, "$group");
    assert_eq!(promoted.description, "owner edit");
    assert_eq!(promoted.scope, StickerScope::Any);
    assert_eq!(promoted.first_seen, ms);
    assert_eq!(promoted.last_seen, later);
    assert_eq!(promoted.last_sent, Some(later));
    assert_eq!(promoted.sightings, 2);
    assert_eq!(promoted.sent_count, 1);

    let mut writes = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let store = store.clone();
        writes.spawn(async move {
            store
                .touch_media_sighting(
                    "hash",
                    later,
                    Some((MediaStatus::Rejected, "another".into())),
                )
                .await?;
            store.record_media_sent("hash", later).await
        });
    }
    assert!(store.update_media_caption(hash, &caption).await?);
    while let Some(result) = writes.join_next().await {
        result??;
    }
    let records = store.all_media().await?;
    assert_eq!(records.len(), 1);
    let edited = &records[0];
    assert_eq!(edited.sightings, 18);
    assert_eq!(edited.sent_count, 17);
    assert_eq!(edited.origin, "$group");
    assert_eq!(edited.status, MediaStatus::Active);
    assert_eq!(edited.scope, StickerScope::Any);
    assert_eq!(edited.description, caption.description);
    assert_eq!(edited.text, caption.text);
    assert_eq!(edited.use_when, caption.use_when);
    assert_eq!(edited.avoid, caption.avoid);
    assert_eq!(edited.tags, caption.tags);

    let disable = MediaEditPatch {
        status: Some(MediaStatus::Disabled),
        ..Default::default()
    };
    assert_eq!(
        store.patch_media(hash, &disable).await?.unwrap().status,
        MediaStatus::Disabled
    );
    assert!(!store.update_media_caption(hash, &caption).await?);
    let vector = vector();
    store.save_sticker_vector(&vector).await?;
    assert_eq!(
        store.sticker_vectors("model").await?[hash].vector,
        vector.vector
    );
    assert!(store.sticker_vectors("another").await?.is_empty());
    store.delete_media(hash).await?;
    assert!(store.all_media().await?.is_empty());
    assert!(store.sticker_vectors("model").await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn mem_store_contract() -> Result<()> {
    contract(&Store::memory()).await
}

#[tokio::test]
async fn missing_mongo_url_fails_without_jsonl_fallback() {
    let cfg = crate::config::MongoCfg {
        url: None,
        ..Default::default()
    };
    let Err(error) = Store::open(&cfg).await else {
        panic!("expected missing URL error")
    };
    assert!(error.to_string().contains("MONGO_URL is required"));
}

#[tokio::test]
async fn invalid_mongo_url_redacts_credentials() {
    let cfg = crate::config::MongoCfg {
        url: Some("invalid://username:secret@example.com".into()),
        ..Default::default()
    };
    let Err(error) = Store::open(&cfg).await else {
        panic!("expected invalid URL error")
    };
    assert!(!format!("{error:#}").contains("secret"));
}

#[test]
fn task_document_keeps_exact_id_and_nested_bson_dates() {
    use mongodb::bson::Bson;
    let task: TaskRecord = serde_json::from_value(json!({
        "id": "exact-id", "title": "task", "kind": "research", "status": "inbox",
        "requester": "owner", "created": time(), "updated": time(),
        "followups": [{
            "seq": 1, "ts": time(), "body": "context",
            "thread": { "platform": "qq", "chat_id": "1", "message_id": null }
        }],
    })).unwrap();
    let doc = mongo::to_bson_doc(&task, Some(Bson::String(task.id.clone()))).unwrap();
    assert_eq!(doc.get_str("_id").unwrap(), task.id);
    for field in ["created", "updated"] {
        assert_eq!(doc.get_datetime(field).unwrap().timestamp_millis(), time().timestamp_millis());
    }
    let followup = doc.get_array("followups").unwrap()[0].as_document().unwrap();
    assert_eq!(followup.get_datetime("ts").unwrap().timestamp_millis(), time().timestamp_millis());
    let decoded: TaskRecord = mongo::from_bson_doc(doc).unwrap();
    assert_eq!(decoded.id, task.id);
    assert_eq!(decoded.followups[0].ts, truncate_ms(time()));
    assert!(decoded.followups[0].consumed_round.is_none());
}
