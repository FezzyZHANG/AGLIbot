//! Convert legacy MongoDB records while AGLIbot is stopped.

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use mongodb::bson::{Bson, Document, doc};
use serde::de::DeserializeOwned;

use super::mongo::{MongoStore, kv_doc, review_doc, to_bson_doc};
use super::{
    AuditRecord, Episode, MediaRecord, MessageRecord, ModelCallRecord, Person, ReviewItem,
    StickerVector,
};
use crate::config::Config;
use crate::mind::state::{MIND_STATE_KEY, MindState};

const COLLECTIONS: &[&str] = &[
    "messages",
    "model_calls",
    "episodes",
    "persons",
    "audit",
    "reviews",
    "kv",
    "media",
    "sticker_vectors",
];
const LEGACY_ORDERED: &[&str] = &[
    "messages",
    "model_calls",
    "episodes",
    "lessons",
    "audit",
    "reviews",
];

pub(super) async fn has_legacy_schema(store: &MongoStore) -> Result<bool> {
    let names = store.db.list_collection_names().await?;
    if names.iter().any(|name| name == "_store_counters") {
        return Ok(true);
    }
    if names.iter().any(|name| name == "messages") {
        return has_legacy_order_index(store, "messages").await;
    }
    Ok(false)
}

async fn has_legacy_order_index(store: &MongoStore, name: &str) -> Result<bool> {
    Ok(store
        .collection(name)
        .list_index_names()
        .await?
        .iter()
        .any(|name| name == "_order_1"))
}

pub async fn run(args: impl IntoIterator<Item = OsString>) -> Result<()> {
    let mut apply = false;
    let mut dry_run = false;
    for arg in args {
        match arg.to_str() {
            Some("--apply") => apply = true,
            Some("--dry-run") => dry_run = true,
            _ => bail!("usage: aglibot migrate-bson [--dry-run | --apply]"),
        }
    }
    ensure!(!(apply && dry_run), "choose either --dry-run or --apply");
    let root = std::env::var_os("AGLI_ROOT")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let cfg = Config::load(&root)?;
    let store = MongoStore::connect(&cfg.runtime.mongo).await?;

    let mind_state = if store
        .raw("kv")
        .find_one(doc! { "_id": MIND_STATE_KEY })
        .await?
        .is_none()
    {
        let path = cfg.data_dir().join("state").join("state.json");
        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let state: serde_json::Value = serde_json::from_slice(&bytes)
                    .with_context(|| format!("invalid mind state in {}", path.display()))?;
                serde_json::from_value::<MindState>(state.clone())
                    .with_context(|| format!("invalid mind state in {}", path.display()))?;
                Some(kv_doc(MIND_STATE_KEY, &state)?)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
        }
    } else {
        None
    };

    println!(
        "{}",
        if apply {
            "Preflight for BSON migration"
        } else {
            "BSON migration dry run"
        }
    );
    for &name in COLLECTIONS {
        let count = convert_collection(&store, name, false).await?;
        println!("{name}: {count} legacy document(s)");
    }
    let max_id = max_review_id(&store).await?;
    println!("Highest existing review ID: {max_id}");
    println!(
        "Import mind_state from state.json: {}",
        mind_state.is_some()
    );

    if apply {
        // Legacy unique indexes reject multiple replacement documents without an _order field.
        let names = store.db.list_collection_names().await?;
        for &name in LEGACY_ORDERED {
            if names.iter().any(|existing| existing == name)
                && has_legacy_order_index(&store, name).await?
            {
                store.collection(name).drop_index("_order_1").await?;
            }
        }
        for &name in COLLECTIONS {
            convert_collection(&store, name, true).await?;
        }
        store
            .collection("_counters")
            .update_one(
                doc! { "_id": "review_id" },
                doc! { "$max": { "seq": max_id } },
            )
            .upsert(true)
            .await?;
        if let Some(mut fields) = mind_state {
            fields.remove("_id");
            store
                .collection("kv")
                .update_one(
                    doc! { "_id": MIND_STATE_KEY },
                    doc! { "$setOnInsert": fields },
                )
                .upsert(true)
                .await?;
        }
        store.init().await?;
        store.collection("_store_counters").drop().await?;
        println!("BSON migration applied");
    }
    Ok(())
}

async fn convert_collection(store: &MongoStore, name: &str, apply: bool) -> Result<u64> {
    let collection = store.collection(name);
    let raw = store.raw(name);
    let mut cursor = raw
        .find(doc! { "record": { "$type": "string" } })
        .sort(doc! { "_id": 1 })
        .await?;
    let mut count = 0;
    while cursor.advance().await? {
        let legacy = cursor.deserialize_current()?.to_document()?;
        let old_id = legacy
            .get("_id")
            .context("legacy document is missing _id")?;
        let converted = convert(name, &legacy)
            .with_context(|| format!("converting legacy {name} document {old_id}"))?;
        let new_id = converted
            .get("_id")
            .context("converted document is missing _id")?;
        let old_key = doc! { "_id": old_id };
        if old_id == new_id {
            if apply {
                collection.replace_one(old_key, converted).await?;
            }
        } else {
            // Write the normalized key first. An interrupted run can find and verify that copy.
            let existing = raw.find_one(doc! { "_id": new_id }).await?;
            if let Some(existing) = existing {
                ensure!(
                    existing.to_document()? == converted,
                    "conflicting {name} document at {new_id}"
                );
            } else if apply {
                collection.insert_one(converted).await?;
            }
            if apply {
                collection.delete_one(old_key).await?;
            }
        }
        count += 1;
    }
    Ok(count)
}

async fn max_review_id(store: &MongoStore) -> Result<i64> {
    let mut cursor = store.raw("reviews").find(doc! {}).await?;
    let mut max_id = 0;
    while cursor.advance().await? {
        let doc = cursor.deserialize_current()?.to_document()?;
        let id = match doc.get_str("record") {
            Ok(json) => i64::try_from(serde_json::from_str::<ReviewItem>(json)?.id)?,
            Err(_) => match doc.get("id") {
                Some(Bson::Int64(id)) => *id,
                Some(Bson::Int32(id)) => i64::from(*id),
                _ => bail!("invalid native review ID"),
            },
        };
        max_id = max_id.max(id);
    }
    Ok(max_id)
}

fn convert(name: &str, legacy: &Document) -> Result<Document> {
    let id = legacy
        .get("_id")
        .context("legacy document is missing _id")?;
    let record = legacy.get_str("record")?;
    fn append<T: DeserializeOwned + serde::Serialize>(record: &str, id: &Bson) -> Result<Document> {
        to_bson_doc(&serde_json::from_str::<T>(record)?, Some(id.clone()))
    }
    match name {
        "messages" => append::<MessageRecord>(record, id),
        "model_calls" => append::<ModelCallRecord>(record, id),
        "episodes" => append::<Episode>(record, id),
        "audit" => append::<AuditRecord>(record, id),
        "persons" => {
            let person: Person = serde_json::from_str(record)?;
            to_bson_doc(&person, Some(person.person_id.as_str().into()))
        }
        "reviews" => review_doc(&serde_json::from_str(record)?),
        "kv" => kv_doc(
            id.as_str().context("legacy KV key must be a string")?,
            &serde_json::from_str(record)?,
        ),
        "media" => {
            let media: MediaRecord = serde_json::from_str(record)?;
            to_bson_doc(&media, Some(media.hash.as_str().into()))
        }
        "sticker_vectors" => {
            let vector: StickerVector = serde_json::from_str(record)?;
            to_bson_doc(&vector, Some(vector.hash.as_str().into()))
        }
        _ => bail!("unsupported legacy collection {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::mongo::bson_dt;
    use crate::store::tests::{
        audit, call, episode, json_payload, media, message, person, review, time, vector,
    };
    use mongodb::bson::oid::ObjectId;

    fn wrap(id: impl Into<Bson>, record: &impl serde::Serialize) -> Document {
        doc! { "_id": id.into(), "_order": 123_i64, "record": serde_json::to_string(record).unwrap() }
    }

    fn native(collection: &str, legacy: Document) -> Result<Document> {
        let converted = convert(collection, &legacy)?;
        assert!(!converted.contains_key("record"));
        assert!(!converted.contains_key("_order"));
        Ok(converted)
    }

    #[test]
    fn migrates_append_only_records_with_original_ids() -> Result<()> {
        let id = ObjectId::new();
        let doc = native("messages", wrap(id, &message("group/qq:7", "text", time())))?;
        assert_eq!(doc.get_object_id("_id")?, id);
        assert_eq!(doc.get_datetime("ts")?, &bson_dt(time()));
        assert_eq!(doc.get_str("ns")?, "group/qq:7");
        assert_eq!(doc.get_str("text")?, "text");

        let mut legacy = serde_json::to_value(episode())?;
        legacy["sources"] = serde_json::json!(["old"]);
        let doc = native("episodes", wrap(id, &legacy))?;
        assert_eq!(doc.get_object_id("_id")?, id);
        assert_eq!(doc.get_datetime("ts")?, &bson_dt(time()));
        assert!(!doc.contains_key("sources"));

        let doc = native("model_calls", wrap(id, &call(time(), 0.5)))?;
        assert_eq!(doc.get_datetime("ts")?, &bson_dt(time()));
        assert_eq!(doc.get_f64("cost_usd")?, 0.5);
        assert_eq!(doc.get_str("model_id")?, "model");

        let doc = native("audit", wrap(id, &audit()))?;
        assert_eq!(doc.get_datetime("ts")?, &bson_dt(time()));
        assert_eq!(doc.get_str("outcome")?, "sent");
        Ok(())
    }

    #[test]
    fn migrates_keyed_records_and_preserves_json_values() -> Result<()> {
        let doc = native("persons", wrap("legacy", &person()))?;
        assert_eq!(doc.get_str("_id")?, "qq:8");
        assert_eq!(doc.get_datetime("last_update")?, &bson_dt(time()));
        assert_eq!(
            doc.get_array("facts")?[0]
                .as_document()
                .unwrap()
                .get_datetime("ts")?,
            &bson_dt(time())
        );

        let doc = native("reviews", wrap("42", &review().with_id(42)))?;
        assert_eq!(doc.get_i64("_id")?, 42);
        assert_eq!(doc.get_i64("id")?, 42);
        assert!(doc.get_document("payload")?.contains_key("$date"));

        let doc = native("kv", wrap("key", &json_payload()))?;
        assert_eq!(doc.len(), 2);
        assert_eq!(doc, kv_doc("key", &json_payload())?);

        let doc = native("media", wrap("legacy", &media()))?;
        assert_eq!(doc.get_str("_id")?, "hash");
        assert_eq!(doc.get_datetime("first_seen")?, &bson_dt(time()));
        assert_eq!(doc.get_datetime("last_seen")?, &bson_dt(time()));
        assert!(matches!(doc.get("last_sent"), Some(Bson::Null)));

        let doc = native("sticker_vectors", wrap("legacy", &vector()))?;
        assert_eq!(doc.get_str("_id")?, "hash");
        assert_eq!(doc.get_str("model_id")?, "model");
        Ok(())
    }
}
