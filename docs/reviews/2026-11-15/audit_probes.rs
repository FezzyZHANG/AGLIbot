#![allow(dead_code)]
// Isolated audit probes. Production files are imported unchanged; only synthetic data is used.
include!("../../../src/main.rs");

#[cfg(test)]
mod audit_probes {
    use super::*;
    use chrono::{NaiveDate, Utc, Weekday};
    use serde_json::json;
    use std::{path::PathBuf, sync::Arc};
    use store::{MediaRecord, MediaStatus, ReviewCategory, ReviewItem, ReviewStatus, StickerScope, Store};

    fn fixture(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("audit-{label}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[tokio::test]
    async fn confirms_private_diary_text_can_enter_global_l1_without_review() {
        let root = fixture("growth");
        let mind_dir = root.join("mind");
        for folder in ["self", "social", "life/journal"] {
            std::fs::create_dir_all(mind_dir.join(folder)).unwrap();
        }
        for (path, content) in [
            ("self/boundaries.md", "Keep private information private."),
            ("self/identity.md", "Synthetic test persona."),
            ("self/personality.yaml", "ocean: {}\nstyle: {}\n"),
            ("self/voice.md", "Plain speech."),
            ("social/etiquette.md", "Respect context boundaries."),
            ("life/journal/2026-11-14.md", "Private diary: PRIVATE_GROWTH_SENTINEL."),
        ] { std::fs::write(mind_dir.join(path), content).unwrap(); }
        let db = Arc::new(store::jsonl::JsonlStore::open(&root.join("data")).unwrap());
        let models = mind::models::Models::stub(db.clone(), vec![
            "Private narrative.".into(),
            json!({"ocean":{},"style":{},"voice_note":"PRIVATE_GROWTH_SENTINEL","interests_add":[],"interests_drop":[],"why":"Diary evidence."}).to_string(),
        ]);
        assert!(mind::growth::weekly(&models, &mind_dir, NaiveDate::from_ymd_opt(2026, 11, 15).unwrap(), Weekday::Sun).await.unwrap());
        let l1 = mind::context::build_l1(&mind_dir).unwrap();
        assert!(l1.contains("PRIVATE_GROWTH_SENTINEL"));
        assert!(db.reviews().unwrap().is_empty());
        println!("CONFIRMED: private diary-derived free text enters audience-independent L1; no Widen review exists.");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn confirms_review_id_interleaving_overwrites_a_pending_review() {
        let root = fixture("reviews");
        let db = store::jsonl::JsonlStore::open(&root).unwrap();
        // The allocation/save interleaving permitted to two concurrent judge::file callers.
        let first = db.next_review_id().unwrap();
        let second = db.next_review_id().unwrap();
        assert_eq!(first, second);
        let item = |id, subject: &str| ReviewItem {
            id, ts: Utc::now(), category: ReviewCategory::Act,
            subject: subject.into(), detail: "synthetic".into(), verdict: "owner".into(),
            status: ReviewStatus::Open, payload: json!({"subject":subject}),
            notified: false, urgent: false, pushed: false,
        };
        db.save_review(&item(first, "first pending request")).unwrap();
        db.save_review(&item(second, "second pending request")).unwrap();
        let remaining = db.reviews().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].subject, "second pending request");
        println!("CONFIRMED: two unreserved allocations return {first}; second save overwrites first review.");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn confirms_stale_sighting_write_restores_owner_disabled_scope() {
        let root = fixture("media");
        let db = store::jsonl::JsonlStore::open(&root).unwrap();
        let now = Utc::now();
        let initial = MediaRecord {
            hash: "a".repeat(64), ext: "png".into(), status: MediaStatus::Active,
            keep: true, description: "synthetic".into(), text: String::new(), use_when: String::new(),
            avoid: String::new(), tags: vec![], scope: StickerScope::Any, scope_reason: String::new(),
            origin: "qq:1".into(), first_seen: now, last_seen: now, sightings: 1, sent_count: 0, last_sent: None,
        };
        db.save_media(&initial).unwrap();
        let mut worker = db.media(&initial.hash).unwrap().unwrap();
        let mut owner = db.media(&initial.hash).unwrap().unwrap();
        owner.status = MediaStatus::Disabled;
        owner.scope = StickerScope::Origin;
        db.save_media(&owner).unwrap();
        worker.sightings += 1;
        db.save_media(&worker).unwrap();
        let actual = db.media(&initial.hash).unwrap().unwrap();
        assert_eq!(actual.status, MediaStatus::Active);
        assert_eq!(actual.scope, StickerScope::Any);
        println!("CONFIRMED: stale sighting save restores Active/Any after owner saved Disabled/Origin.");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn confirms_default_local_model_cannot_serve_reflection_or_persona_probes() {
        let models: config::ModelsCfg = serde_yaml::from_str(include_str!("../../../config/models.yaml")).unwrap();
        let scenarios: config::ScenariosCfg = serde_yaml::from_str(include_str!("../../../config/scenarios.yaml")).unwrap();
        assert!(models.models.iter().any(|m| m.local));
        let router = mind::router::Router::new(models.models, scenarios);
        assert!(router.select("consolidate", &ns::ProcessingPolicy::Any, false).is_ok());
        assert!(router.select("consolidate", &ns::ProcessingPolicy::LocalOnly, false).is_err());
        assert!(router.select("chat", &ns::ProcessingPolicy::Any, false).is_ok());
        assert!(router.select("chat", &ns::ProcessingPolicy::LocalOnly, false).is_err());
        println!("CONFIRMED: default local entry exists but neither consolidate nor pinned chat can satisfy LocalOnly.");
    }
}
