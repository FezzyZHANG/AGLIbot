use super::*;
use crate::store::jsonl::JsonlStore;
use std::sync::Arc;

fn fixture(name: &str) -> (std::path::PathBuf, Arc<JsonlStore>) {
    let dir = std::env::temp_dir().join(format!("agli-growth-{name}-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
    std::fs::create_dir_all(dir.join("self")).unwrap();
    std::fs::create_dir_all(dir.join("life/journal")).unwrap();
    std::fs::write(dir.join("self/personality.yaml"), "ocean:\n  openness: 0.8\n  neuroticism: 0.01\nstyle:\n  verbosity: 0.35\n").unwrap();
    std::fs::write(dir.join("self/voice.md"), "seed voice").unwrap();
    for day in 20..=28 {
        std::fs::write(dir.join(format!("life/journal/2026-10-{day}.md")), format!("journal-{day}")).unwrap();
    }
    let store = Arc::new(JsonlStore::open(&dir.join("data")).unwrap());
    (dir, store)
}

fn proposal() -> String {
    r#"{"ocean":{"o":9,"n":-9},"style":{"verbosity":-9,"unknown":1},"voice_note":"quieter","interests_add":["art"],"interests_drop":["racing"],"why":"weekly feedback"}"#.into()
}

#[tokio::test]
async fn weekly_persisted_gate_caps_seed_changelog_and_effective_personality() {
    let (dir, store) = fixture("weekly");
    let original = std::fs::read(dir.join("self/personality.yaml")).unwrap();
    let original_voice = std::fs::read(dir.join("self/voice.md")).unwrap();
    let date: NaiveDate = "2026-11-01".parse().unwrap(); // Sunday
    let models = Models::stub(store, vec!["叙".repeat(900), proposal(), "next narrative".into(), proposal()]);
    assert!(weekly(&models, &dir, date, Weekday::Sun).await.unwrap());
    let overlay = load(&dir).unwrap();
    assert!((overlay.ocean["openness"] - 0.02).abs() < 1e-12);
    assert!((overlay.ocean["neuroticism"] + 0.01).abs() < 1e-12);
    assert!((overlay.style["verbosity"] + 0.02).abs() < 1e-12);
    assert!(!overlay.style.contains_key("unknown"));
    assert_eq!(overlay.voice_note, "quieter");
    assert_eq!(overlay.interests_add.len() + overlay.interests_drop.len(), 2);
    let merged: serde_json::Value = serde_yaml::from_str(&personality(&dir).unwrap()).unwrap();
    assert!((merged["ocean"]["openness"].as_f64().unwrap() - 0.82).abs() < 1e-12);
    let voice = voice(&dir).unwrap();
    assert!(voice.contains("seed voice") && voice.contains("quieter") && voice.contains("art") && voice.contains("racing"));
    let log = std::fs::read_to_string(dir.join("self/changelog.md")).unwrap();
    assert!(log.contains("2026-11-01 ocean.openness") && log.contains("weekly feedback"));
    assert_eq!(std::fs::read_to_string(dir.join("memory/narrative.md")).unwrap().trim().chars().count(), 800);
    for offset in 0..7 {
        assert!(!weekly(&models, &dir, date + Duration::days(offset), Weekday::Sun).await.unwrap());
    }
    assert_eq!(models.stub_prompts().len(), 2);
    let prompt = &models.stub_prompts()[0];
    assert_eq!(prompt.1, ProcessingPolicy::Any); // existing journal's no-local exception
    assert!(prompt.2.contains("journal-22") && !prompt.2.contains("journal-21"));
    // Miss Sunday: the first Monday pass still runs for the new Sunday boundary.
    assert!(weekly(&models, &dir, date + Duration::days(8), Weekday::Sun).await.unwrap());
    assert!(models.stub_prompts()[2].2.contains(&"叙".repeat(800)));
    assert_eq!(std::fs::read(dir.join("self/personality.yaml")).unwrap(), original);
    assert_eq!(std::fs::read(dir.join("self/voice.md")).unwrap(), original_voice);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn invalid_json_means_no_change_and_empty_journals_make_no_calls() {
    let (dir, store) = fixture("invalid");
    let date = "2026-11-01".parse().unwrap();
    let models = Models::stub(store, vec!["narrative".into(), "not json".into()]);
    assert!(weekly(&models, &dir, date, Weekday::Sun).await.is_err());
    for path in ["self/drift.yaml", "self/changelog.md", "memory/narrative.md"] { assert!(!dir.join(path).exists()); }
    std::fs::remove_dir_all(dir.join("life/journal")).unwrap();
    assert!(!weekly(&models, &dir, date, Weekday::Sun).await.unwrap());
    assert_eq!(models.stub_prompts().len(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cumulative_caps_invalid_interests_voice_and_custom_week_boundary() {
    let (dir, _) = fixture("caps");
    let date: NaiveDate = "2026-11-01".parse().unwrap();
    let mut overlay = Overlay::default();
    for _ in 0..20 { apply(&mut overlay, serde_json::from_str(&proposal()).unwrap(), &seed(&dir).unwrap(), date); }
    assert!((overlay.ocean["openness"] - 0.15).abs() < 1e-12);
    assert!((overlay.style["verbosity"] + 0.15).abs() < 1e-12);
    let mut invalid: Proposal = serde_json::from_str(&proposal()).unwrap();
    invalid.ocean.clear(); invalid.style.clear();
    invalid.voice_note = "line1\nline2".into();
    invalid.interests_add = vec!["a".into(), "b".into(), "c".into()];
    let before = serde_json::to_value(&overlay).unwrap();
    assert!(apply(&mut overlay, invalid, &seed(&dir).unwrap(), date).is_empty());
    assert_eq!(serde_json::to_value(&overlay).unwrap(), before);
    assert_eq!(week_start(date, Weekday::Wed), "2026-10-28".parse::<NaiveDate>().unwrap());
    let cfg: crate::config::MemoryCfg = serde_yaml::from_str("extract_after: 10\nepisodes_in_context: 5\nfacts_in_context: 5").unwrap();
    assert_eq!(cfg.weekly_weekday, Weekday::Sun);
    let cfg: crate::config::MemoryCfg = serde_yaml::from_str("extract_after: 10\nepisodes_in_context: 5\nfacts_in_context: 5\nweekly_weekday: Wed").unwrap();
    assert_eq!(cfg.weekly_weekday, Weekday::Wed);
    std::fs::remove_dir_all(dir).unwrap();
}
