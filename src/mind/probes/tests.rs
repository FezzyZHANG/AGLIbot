use super::*;
use std::sync::Arc;
use crate::store::jsonl::JsonlStore;

#[tokio::test]
async fn baseline_low_score_once_approval_latest_and_week_gate() {
    let dir = std::env::temp_dir().join(format!("agli-probes-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
    let mind = dir.join("mind");
    let data = dir.join("data");
    std::fs::create_dir_all(mind.join("self")).unwrap();
    let store = Arc::new(JsonlStore::open(&data).unwrap());
    let date: NaiveDate = "2026-11-01".parse().unwrap();
    let mut replies = vec!["baseline".into(); 5];
    replies.extend(vec!["changed".into(); 5]);
    replies.push(r#"{"scores":[0.1,0.2,0.3,0.4,0.5],"why":"changed"}"#.into());
    replies.extend(vec!["latest".into(); 5]);
    replies.push(r#"{"scores":[1,1,1,1,1],"why":"consistent"}"#.into());
    let models = Models::stub(store.clone(), replies);
    assert!(!weekly(&models, store.as_ref(), &mind, &data, date, Weekday::Sun, "effective L1").await.unwrap());
    assert!(models.stub_prompts().is_empty());
    let set_growth = |day| {
        std::fs::write(mind.join("self/drift.yaml"), serde_yaml::to_string(&growth::Overlay {
            last_week: Some(day), ..Default::default()
        }).unwrap()).unwrap();
    };
    set_growth(date);
    std::fs::write(mind.join("self/changelog.md"), "old changes\n").unwrap();
    assert!(weekly(&models, store.as_ref(), &mind, &data, date, Weekday::Sun, "effective L1").await.unwrap());
    assert!(data.join("probes/2026-11-01.json").exists());
    assert_eq!(store.kv_get(BASELINE).unwrap().unwrap()["answers"][0], "baseline");
    assert!(store.reviews().unwrap().is_empty());
    for offset in 0..7 {
        assert!(!weekly(&models, store.as_ref(), &mind, &data, date + chrono::Duration::days(offset), Weekday::Sun, "L1").await.unwrap());
    }
    assert_eq!(models.stub_prompts().len(), 5);
    let next = date + chrono::Duration::days(7);
    set_growth(next);
    std::fs::write(mind.join("self/changelog.md"), "old changes\nnew change\n").unwrap();
    assert!(weekly(&models, store.as_ref(), &mind, &data, next, Weekday::Sun, "L1").await.unwrap());
    // Retry even across the review/finished-marker gap must not rejudge or refile.
    store.kv_set(DONE, json!(date)).unwrap();
    assert!(weekly(&models, store.as_ref(), &mind, &data, next, Weekday::Sun, "L1").await.unwrap());
    let reviews = store.reviews().unwrap();
    assert_eq!(reviews.len(), 1);
    assert!(!reviews[0].urgent);
    assert_eq!(reviews[0].payload["changelog"], "new change\n");
    assert_eq!(reviews[0].payload["pairs"].as_array().unwrap().len(), 5);
    assert_eq!(models.stub_prompts().len(), 11);
    let third = next + chrono::Duration::days(8); // missed scheduled night
    set_growth(third - chrono::Duration::days(1));
    assert!(weekly(&models, store.as_ref(), &mind, &data, third, Weekday::Sun, "L1").await.unwrap());
    rebaseline(store.as_ref()).unwrap();
    assert_eq!(store.kv_get(BASELINE).unwrap().unwrap()["answers"][0], "latest");
    assert!(models.stub_prompts().iter().all(|p| p.1 == ProcessingPolicy::Any));
    assert_eq!(models.stub_prompts().iter().filter(|p| p.0 == "consolidate").count(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}
