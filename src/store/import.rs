//! Offline, one-off migration. Stop source and target writers before taking the snapshot.
//! Nonempty collections must match exactly; interrupted partial collections require manual review.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::jsonl::JsonlStore;
use super::mongo::MongoStore;
use super::*;

#[derive(Default)]
pub(super) struct Snapshot {
    pub messages: Vec<MessageRecord>,
    pub model_calls: Vec<ModelCallRecord>,
    pub episodes: Vec<Episode>,
    pub persons: BTreeMap<String, Person>,
    pub lessons: Vec<Lesson>,
    pub audit: Vec<AuditRecord>,
    pub reviews: Vec<ReviewItem>,
    pub kv: BTreeMap<String, Value>,
}

fn records<T: Serialize>(items: impl IntoIterator<Item = T>) -> Result<Vec<String>> {
    items.into_iter().map(|item| Ok(serde_json::to_string(&item)?)).collect()
}

impl Snapshot {
    fn collections(&self) -> Result<BTreeMap<&'static str, Vec<String>>> {
        Ok(BTreeMap::from([
            ("messages", records(&self.messages)?),
            ("model_calls", records(&self.model_calls)?),
            ("episodes", records(&self.episodes)?),
            ("persons", records(&self.persons)?),
            ("lessons", records(&self.lessons)?),
            ("audit", records(&self.audit)?),
            ("reviews", records(&self.reviews)?),
            ("kv", records(&self.kv)?),
        ]))
    }

    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.persons.iter().all(|(key, person)| key == &person.person_id),
            "persons: key differs from person_id"
        );
        let ids: std::collections::BTreeSet<_> = self.reviews.iter().map(|r| r.id).collect();
        ensure!(ids.len() == self.reviews.len(), "reviews: duplicate ids");
        ensure!(!ids.contains(&u64::MAX), "reviews: next_review_id would overflow");
        Ok(())
    }

    fn write(&self, target: &MongoStore, name: &str) -> Result<()> {
        match name {
            "messages" => {
                for item in &self.messages {
                    target.append_message(item)?;
                }
            }
            "model_calls" => {
                for item in &self.model_calls {
                    target.append_model_call(item)?;
                }
            }
            "episodes" => {
                for item in &self.episodes {
                    target.append_episode(item)?;
                }
            }
            "persons" => {
                for item in self.persons.values() {
                    target.save_person(item)?;
                }
            }
            "lessons" => {
                for item in &self.lessons {
                    target.append_lesson(item)?;
                }
            }
            "audit" => {
                for item in &self.audit {
                    target.append_audit(item)?;
                }
            }
            "reviews" => {
                for item in &self.reviews {
                    target.save_review(item)?;
                }
            }
            "kv" => {
                for (key, value) in &self.kv {
                    target.kv_set(key, value.clone())?;
                }
            }
            _ => bail!("unknown Store collection"),
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
struct CollectionPlan {
    name: &'static str,
    written: usize,
    skipped: usize,
}

fn plan(source: &Snapshot, target: &Snapshot) -> Result<Vec<CollectionPlan>> {
    let target = target.collections()?;
    source
        .collections()?
        .into_iter()
        .map(|(name, records)| {
            let existing = &target[name];
            ensure!(
                existing.is_empty() || existing == &records,
                "{name}: nonempty target differs from source; nothing written"
            );
            let skipped = if existing.is_empty() { 0 } else { records.len() };
            Ok(CollectionPlan { name, written: records.len() - skipped, skipped })
        })
        .collect()
}

fn verify(source: &Snapshot, target: &Snapshot) -> Result<()> {
    let target = target.collections()?;
    for (name, records) in source.collections()? {
        ensure!(target[name] == records, "{name}: verification failed (count, order or content mismatch)");
        let hash = Sha256::digest(serde_json::to_vec(&records)?);
        println!("{name}: verified={} sha256={hash:x}", records.len());
    }
    Ok(())
}

fn import(source: &Snapshot, target: &MongoStore, dry_run: bool) -> Result<Vec<CollectionPlan>> {
    let plan = plan(source, &target.snapshot()?)?;
    for item in &plan {
        if !dry_run && item.written != 0 {
            source.write(target, item.name)?;
        }
        println!(
            "{}: source={} {}written={} skipped={}",
            item.name,
            item.written + item.skipped,
            if dry_run { "would_be_" } else { "" },
            item.written,
            item.skipped
        );
    }
    if !dry_run {
        verify(source, &target.snapshot()?)?;
    }
    Ok(plan)
}

fn arguments(args: impl Iterator<Item = OsString>) -> Result<(PathBuf, bool)> {
    let mut args = args;
    let mut from = None;
    let mut dry_run = false;
    while let Some(arg) = args.next() {
        if arg == "--from" && from.is_none() {
            from = Some(PathBuf::from(args.next().context("--from requires a directory")?));
        } else if arg == "--dry-run" && !dry_run {
            dry_run = true;
        } else {
            bail!("usage: aglibot import-jsonl --from <data dir> [--dry-run]");
        }
    }
    Ok((from.context("--from <data dir> is required")?, dry_run))
}

pub fn run(args: impl Iterator<Item = OsString>) -> Result<()> {
    let (from, dry_run) = arguments(args)?;
    let source = JsonlStore::import_snapshot(&from)?;
    let url = std::env::var("MONGO_URL").ok().filter(|s| !s.trim().is_empty()).context("MONGO_URL is required")?;
    let database =
        std::env::var("MONGO_DATABASE").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "aglibot".into());
    // Driver errors may contain connection details. Keep them out of all command output.
    let result = (|| {
        let target = MongoStore::open_for_import(&url, &database, dry_run)?;
        import(&source, &target, dry_run)?;
        Ok::<_, anyhow::Error>(())
    })();
    result.map_err(|e| {
        if e.chain().any(|cause| cause.is::<mongodb::error::Error>()) {
            anyhow::anyhow!("MongoDB import failed (connection details redacted); inspect target before retrying")
        } else {
            e
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::tests::{contract, message, time};
    use super::*;
    use mongodb::bson::oid::ObjectId;
    use serde_json::json;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("aglibot-import-{}", ObjectId::new().to_hex())))
        }
        fn populate(&self) -> Snapshot {
            let store = JsonlStore::open(&self.0).unwrap();
            contract(&store);
            // Identical appended records must stay distinct, and review ids need not be contiguous.
            let m = message("person/a", "duplicate", time());
            store.append_message(&m).unwrap();
            store.append_message(&m).unwrap();
            let mut review = store.reviews().unwrap()[0].clone();
            review.id = 99;
            store.save_review(&review).unwrap();
            store.kv_set("special", json!({"$date": "literal", "a.b": u64::MAX, "zero": -0.0})).unwrap();
            JsonlStore::import_snapshot(&self.0).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reading_plan_and_verification_are_lossless() -> Result<()> {
        let fixture = Fixture::new();
        let source = fixture.populate();
        let before: BTreeMap<_, _> = std::fs::read_dir(&fixture.0)?
            .map(|e| {
                let path = e.unwrap().path();
                (path.clone(), std::fs::read(path).unwrap())
            })
            .collect();
        let empty_plan = plan(&source, &Snapshot::default())?;
        assert_eq!(empty_plan.len(), 8);
        assert!(empty_plan.iter().all(|p| p.written > 0 && p.skipped == 0));
        let reread = JsonlStore::import_snapshot(&fixture.0)?;
        assert!(plan(&source, &reread)?.iter().all(|p| p.written == 0 && p.skipped > 0));
        verify(&source, &reread)?;
        assert_eq!(source.messages.len(), 10);
        assert_eq!(source.model_calls[0].uncached_input, u64::MAX);
        assert_eq!(source.messages[0].ts.timestamp_subsec_nanos(), 123456789);
        let mut different = reread;
        different.messages.swap(0, 1);
        assert!(plan(&source, &different).is_err());
        assert!(verify(&source, &different).is_err());
        different.messages = source.messages.clone();
        different.kv.insert("special".into(), json!(0));
        assert!(plan(&source, &different).is_err());
        for (path, bytes) in before {
            assert_eq!(std::fs::read(path)?, bytes);
        }
        Ok(())
    }

    #[test]
    fn strict_source_validation_and_missing_optional_files() -> Result<()> {
        let fixture = Fixture::new();
        assert!(JsonlStore::import_snapshot(&fixture.0).is_err());
        assert!(!fixture.0.exists());
        std::fs::create_dir(&fixture.0)?;
        assert!(JsonlStore::import_snapshot(&fixture.0)?.collections()?.values().all(Vec::is_empty));
        std::fs::write(fixture.0.join("audit.jsonl"), "\n{broken}\n")?;
        assert!(JsonlStore::import_snapshot(&fixture.0).is_err());
        std::fs::write(fixture.0.join("audit.jsonl"), "\n")?;
        let mut source = fixture.populate();
        source.reviews.push(source.reviews[0].clone());
        std::fs::write(fixture.0.join("reviews.json"), serde_json::to_vec(&source.reviews)?)?;
        assert!(JsonlStore::import_snapshot(&fixture.0).is_err());
        Ok(())
    }

    #[test]
    fn command_arguments() {
        let parse = |args: &[&str]| arguments(args.iter().map(OsString::from));
        assert_eq!(parse(&["--dry-run", "--from", "a b"]).unwrap(), (PathBuf::from("a b"), true));
        for args in [vec![], vec!["--from"], vec!["--unknown"], vec!["--from", "a", "--from", "b"]] {
            assert!(parse(&args).is_err());
        }
    }

    #[test]
    #[ignore = "requires MONGO_URL; creates and drops a unique aglibot_test_* database"]
    fn mongo_import_twice_and_verify() -> Result<()> {
        let url = std::env::var("MONGO_URL").context("MONGO_URL required")?;
        let database = format!("aglibot_test_{}", ObjectId::new().to_hex());
        let target = MongoStore::open_for_import(&url, &database, true)?;
        let fixture = Fixture::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
            let source = fixture.populate();
            assert!(import(&source, &target, true)?.iter().all(|p| p.written > 0));
            verify(&Snapshot::default(), &target.snapshot()?)?;
            let target = MongoStore::open(&url, &database)?;
            assert!(import(&source, &target, false)?.iter().all(|p| p.written > 0));
            let target = MongoStore::open(&url, &database)?;
            assert!(import(&source, &target, false)?.iter().all(|p| p.written == 0));
            assert_eq!(target.next_review_id()?, 100);
            let mut conflict = JsonlStore::import_snapshot(&fixture.0)?;
            conflict.messages[0].text = "conflict".into();
            assert!(import(&conflict, &target, false).is_err());
            verify(&source, &target.snapshot()?)?;
            Ok(())
        }));
        let cleanup = target.drop_import_test_database();
        match result {
            Ok(result) => {
                result?;
                cleanup?;
            }
            Err(panic) => {
                let _ = cleanup;
                std::panic::resume_unwind(panic);
            }
        }
        Ok(())
    }
}
