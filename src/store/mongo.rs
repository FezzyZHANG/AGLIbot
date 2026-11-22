use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use mongodb::bson::{self, Bson, Document, doc, raw::RawDocumentBuf};
use mongodb::options::{ClientOptions, IndexOptions, ReturnDocument};
use mongodb::{Client, Collection, Cursor, Database, IndexModel};
use serde::de::DeserializeOwned;

use super::*;

mod tasks;
use crate::config::MongoCfg;

#[derive(Clone)]
pub(super) struct MongoStore {
    pub(super) db: Database,
}

pub(super) fn bson_dt(ts: DateTime<Utc>) -> bson::DateTime {
    bson::DateTime::from_millis(ts.timestamp_millis())
}

fn map_dates(doc: &mut Document, convert: fn(&Bson) -> Result<Option<Bson>>) -> Result<()> {
    for field in ["ts", "first_seen", "last_seen", "last_sent", "last_update", "created", "updated"] {
        if let Some(value) = doc.get_mut(field)
            && let Some(converted) = convert(value)?
        {
            *value = converted;
        }
    }
    for field in ["facts", "followups"] {
        if let Ok(entries) = doc.get_array_mut(field) {
            for entry in entries.iter_mut().filter_map(Bson::as_document_mut) {
                map_dates(entry, convert)?;
            }
        }
    }
    Ok(())
}

pub(super) fn to_bson_doc(value: &impl Serialize, id: Option<Bson>) -> Result<Document> {
    let mut doc = bson::to_document(value)?;
    map_dates(&mut doc, |value| {
        value
            .as_str()
            .map(|ts| Ok(Bson::DateTime(bson_dt(ts.parse()?))))
            .transpose()
    })?;
    if let Some(id) = id {
        doc.insert("_id", id);
    }
    Ok(doc)
}

pub(super) fn from_bson_doc<T: DeserializeOwned>(mut doc: Document) -> Result<T> {
    doc.remove("_id");
    map_dates(&mut doc, |value| {
        value
            .as_datetime()
            .map(|ts| {
                let ts = DateTime::<Utc>::from_timestamp_millis(ts.timestamp_millis())
                    .context("BSON datetime is outside chrono's range")?;
                Ok(Bson::String(ts.to_rfc3339()))
            })
            .transpose()
    })?;
    Ok(bson::from_document(doc)?)
}

// Treat JSON object keys literally, including objects that resemble Extended JSON.
// Decimal128 preserves unsigned JSON integers that exceed BSON's signed Int64 range.
pub(super) fn json_to_bson(value: &Value) -> Result<Bson> {
    Ok(match value {
        Value::Null => Bson::Null,
        Value::Bool(value) => Bson::Boolean(*value),
        Value::String(value) => Bson::String(value.clone()),
        Value::Number(value) => {
            if let Some(n) = value.as_i64() {
                Bson::Int64(n)
            } else if value.is_u64() {
                Bson::Decimal128(value.to_string().parse()?)
            } else {
                Bson::Double(value.as_f64().context("invalid JSON number")?)
            }
        }
        Value::Array(values) => {
            Bson::Array(values.iter().map(json_to_bson).collect::<Result<_>>()?)
        }
        Value::Object(fields) => Bson::Document(
            fields
                .iter()
                .map(|(key, value)| {
                    ensure!(!key.contains('\0'), "BSON document keys cannot contain NUL");
                    Ok((key.clone(), json_to_bson(value)?))
                })
                .collect::<Result<_>>()?,
        ),
    })
}

fn bson_to_json(value: Bson) -> Result<Value> {
    Ok(match value {
        Bson::Decimal128(n) => match n.to_string().parse::<u64>() {
            Ok(value) => Value::from(value),
            Err(_) => Bson::Decimal128(n).into_relaxed_extjson(),
        },
        Bson::Document(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, value)| Ok((key, bson_to_json(value)?)))
                .collect::<Result<_>>()?,
        ),
        Bson::Array(values) => Value::Array(
            values
                .into_iter()
                .map(bson_to_json)
                .collect::<Result<_>>()?,
        ),
        other => other.into_relaxed_extjson(),
    })
}

pub(super) fn review_doc(review: &ReviewItem) -> Result<Document> {
    let mut fields = serde_json::to_value(review)?;
    fields
        .as_object_mut()
        .context("expected review object")?
        .remove("payload");
    let mut doc = to_bson_doc(&fields, Some(Bson::Int64(i64::try_from(review.id)?)))?;
    doc.insert("payload", json_to_bson(&review.payload)?);
    Ok(doc)
}

fn decode_review(mut doc: Document) -> Result<ReviewItem> {
    let payload = doc.remove("payload").map(bson_to_json).transpose()?;
    let mut review: ReviewItem = from_bson_doc(doc)?;
    review.payload = payload.unwrap_or_default();
    Ok(review)
}

pub(super) fn kv_doc(key: &str, value: &Value) -> Result<Document> {
    Ok(doc! { "_id": key, "value": json_to_bson(value)? })
}

fn decode_kv(mut doc: Document) -> Result<Value> {
    bson_to_json(
        doc.remove("value")
            .context("KV document is missing value")?,
    )
}

impl MongoStore {
    pub async fn connect(cfg: &MongoCfg) -> Result<Self> {
        let url = cfg
            .url
            .as_deref()
            .filter(|url| !url.trim().is_empty())
            .context("MongoDB startup: MONGO_URL is required")?;
        // Driver errors can quote connection-string input, so omit their sources here.
        let mut options = ClientOptions::parse(url).await.map_err(|_| {
            anyhow!("MongoDB startup: invalid or unresolvable MONGO_URL (value redacted)")
        })?;
        options
            .server_selection_timeout
            .get_or_insert(Duration::from_secs(5));
        options
            .connect_timeout
            .get_or_insert(Duration::from_secs(5));
        let client = Client::with_options(options)
            .map_err(|_| anyhow!("MongoDB startup: invalid client options (URL redacted)"))?;
        let store = Self {
            db: client.database(&cfg.database),
        };
        store.db.run_command(doc! { "ping": 1 }).await.map_err(|_| {
            anyhow!("MongoDB startup: connection/authentication failed; check MONGO_URL, MONGO_DATABASE and server availability")
        })?;
        Ok(store)
    }

    pub async fn init(&self) -> Result<()> {
        let indexes = [
            ("messages", doc! { "ns": 1, "ts": 1, "_id": 1 }),
            ("messages", doc! { "chat_id": 1, "ts": 1 }),
            ("messages", doc! { "ts": 1 }),
            ("model_calls", doc! { "ts": 1 }),
            (
                "model_calls",
                doc! { "scenario": 1, "model_id": 1, "ts": 1 },
            ),
            ("episodes", doc! { "ns": 1, "ts": 1 }),
            ("audit", doc! { "ts": 1 }),
            ("tasks", doc! { "status": 1, "created": 1 }),
            ("reviews", doc! { "status": 1, "id": 1 }),
            ("media", doc! { "status": 1, "first_seen": -1 }),
            ("sticker_vectors", doc! { "model_id": 1 }),
        ];
        for (name, keys) in indexes {
            self.collection(name)
                .create_index(IndexModel::builder().keys(keys).build())
                .await?;
        }
        self.collection("lexicon")
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "term": 1, "scope": 1 })
                    .options(IndexOptions::builder().unique(true).build())
                    .build(),
            )
            .await?;
        Ok(())
    }

    pub(super) fn collection(&self, name: &str) -> Collection<Document> {
        self.db.collection(name)
    }

    pub(super) fn raw(&self, name: &str) -> Collection<RawDocumentBuf> {
        self.db.collection(name)
    }

    async fn scan<T: DeserializeOwned>(&self, name: &str) -> Result<Vec<T>> {
        collect(
            self.raw(name).find(doc! {}).sort(doc! { "_id": 1 }).await?,
            from_bson_doc,
        )
        .await
    }

    async fn append(&self, name: &str, value: &impl Serialize) -> Result<()> {
        self.collection(name)
            .insert_one(to_bson_doc(value, None)?)
            .await?;
        Ok(())
    }

    async fn save(&self, name: &str, id: &str, value: &impl Serialize) -> Result<()> {
        self.collection(name)
            .replace_one(doc! { "_id": id }, to_bson_doc(value, Some(id.into()))?)
            .upsert(true)
            .await?;
        Ok(())
    }

    async fn find<T: DeserializeOwned>(&self, name: &str, id: &str) -> Result<Option<T>> {
        self.raw(name)
            .find_one(doc! { "_id": id })
            .await?
            .map(|doc| from_bson_doc(doc.to_document()?))
            .transpose()
    }

    pub async fn append_message(&self, m: &MessageRecord) -> Result<()> {
        self.append("messages", m).await
    }

    pub async fn recent_in_ns(&self, ns: &Namespace, n: usize) -> Result<Vec<MessageRecord>> {
        if n == 0 {
            return Ok(Vec::new());
        }
        let cursor = self
            .raw("messages")
            .find(doc! { "ns": ns.to_string() })
            .sort(doc! { "ts": -1, "_id": -1 })
            .limit(i64::try_from(n).unwrap_or(i64::MAX))
            .await?;
        let mut records = collect(cursor, from_bson_doc).await?;
        records.reverse();
        Ok(records)
    }

    pub async fn messages_after(
        &self,
        ns: &Namespace,
        after: DateTime<Utc>,
    ) -> Result<Vec<MessageRecord>> {
        let cursor = self
            .raw("messages")
            .find(doc! { "ns": ns.to_string(), "ts": { "$gt": bson_dt(after) } })
            .sort(doc! { "ts": 1, "_id": 1 })
            .await?;
        collect(cursor, from_bson_doc).await
    }

    pub async fn active_namespaces(&self, after: DateTime<Utc>) -> Result<Vec<Namespace>> {
        let values = self
            .collection("messages")
            .distinct("ns", doc! { "ts": { "$gt": bson_dt(after) } })
            .await?;
        let mut namespaces = values
            .iter()
            .map(|value| value.as_str().context("invalid BSON namespace")?.parse())
            .collect::<Result<Vec<Namespace>>>()?;
        namespaces.sort();
        namespaces.dedup();
        Ok(namespaces)
    }

    pub async fn append_model_call(&self, c: &ModelCallRecord) -> Result<()> {
        self.append("model_calls", c).await
    }

    pub async fn spent_since(&self, since: DateTime<Utc>) -> Result<f64> {
        let mut cursor = self
            .collection("model_calls")
            .aggregate([
                doc! { "$match": { "ts": { "$gte": bson_dt(since) } } },
                doc! { "$group": { "_id": Bson::Null, "total": { "$sum": "$cost_usd" } } },
            ])
            .await?;
        if cursor.advance().await? {
            return Ok(cursor.deserialize_current()?.get_f64("total")?);
        }
        Ok(0.0)
    }

    #[cfg(test)]
    pub async fn model_calls(&self) -> Result<Vec<ModelCallRecord>> {
        self.scan("model_calls").await
    }

    pub async fn append_episode(&self, e: &Episode) -> Result<()> {
        self.append("episodes", e).await
    }

    pub async fn episodes(&self) -> Result<Vec<Episode>> {
        self.scan("episodes").await
    }

    pub async fn persons(&self) -> Result<BTreeMap<String, Person>> {
        Ok(self
            .scan::<Person>("persons")
            .await?
            .into_iter()
            .map(|p| (p.person_id.clone(), p))
            .collect())
    }

    pub async fn save_person(&self, p: &Person) -> Result<()> {
        self.save("persons", &p.person_id, p).await
    }

    pub async fn append_audit(&self, a: &AuditRecord) -> Result<()> {
        self.append("audit", a).await
    }

    pub async fn audits(&self) -> Result<Vec<AuditRecord>> {
        self.scan("audit").await
    }

    pub async fn reviews(&self) -> Result<Vec<ReviewItem>> {
        let cursor = self
            .raw("reviews")
            .find(doc! {})
            .sort(doc! { "_id": 1 })
            .await?;
        collect(cursor, decode_review).await
    }

    pub async fn create_review(&self, new: NewReview) -> Result<ReviewItem> {
        let counter = self
            .collection("_counters")
            .find_one_and_update(
                doc! { "_id": "review_id" },
                doc! { "$inc": { "seq": 1_i64 } },
            )
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await?
            .context("MongoDB review counter is missing")?;
        let item = new.with_id(u64::try_from(counter.get_i64("seq")?)?);
        self.collection("reviews")
            .insert_one(review_doc(&item)?)
            .await?;
        Ok(item)
    }

    pub async fn save_review(&self, r: &ReviewItem) -> Result<()> {
        let document = review_doc(r)?;
        let result = self
            .collection("reviews")
            .replace_one(doc! { "_id": document.get_i64("_id")? }, document)
            .await?;
        ensure!(result.matched_count == 1, "review does not exist");
        Ok(())
    }

    pub async fn mark_reviews_notified(&self, ids: &[u64]) -> Result<()> {
        let ids = ids.iter().copied().map(i64::try_from).collect::<Result<Vec<_>, _>>()?;
        self.collection("reviews")
            .update_many(doc! { "_id": { "$in": ids } }, doc! { "$set": { "notified": true } })
            .await?;
        Ok(())
    }

    pub async fn kv_get(&self, key: &str) -> Result<Option<Value>> {
        self.raw("kv")
            .find_one(doc! { "_id": key })
            .await?
            .map(|doc| decode_kv(doc.to_document()?))
            .transpose()
    }

    pub async fn kv_set(&self, key: &str, value: Value) -> Result<()> {
        self.collection("kv")
            .replace_one(doc! { "_id": key }, kv_doc(key, &value)?)
            .upsert(true)
            .await?;
        Ok(())
    }

    pub async fn kv_keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        let mut pattern = String::from("^");
        for c in prefix.chars() {
            if r"\.^$|?*+()[]{}".contains(c) {
                pattern.push('\\');
            }
            pattern.push(c);
        }
        collect(
            self.raw("kv").find(doc! { "_id": { "$regex": pattern } })
                .projection(doc! { "_id": 1 }).await?,
            |doc| Ok(doc.get_str("_id")?.to_owned()),
        ).await
    }

    pub async fn lexicon(&self) -> Result<Vec<Value>> {
        let cursor = self
            .raw("lexicon")
            .find(doc! {})
            .sort(doc! { "_id": 1 })
            .await?;
        collect(cursor, |mut doc| {
            doc.remove("_id");
            bson_to_json(Bson::Document(doc))
        })
        .await
    }

    pub async fn insert_lexicon(&self, docs: &[Value]) -> Result<()> {
        if docs.is_empty() {
            return Ok(());
        }
        let docs = docs
            .iter()
            .map(|value| {
                lexicon_key(value)?;
                let Bson::Document(mut doc) = json_to_bson(value)? else {
                    bail!("expected lexicon object")
                };
                doc.remove("_id");
                Ok(doc)
            })
            .collect::<Result<Vec<_>>>()?;
        self.collection("lexicon").insert_many(docs).await?;
        Ok(())
    }

    pub async fn media(&self, hash: &str) -> Result<Option<MediaRecord>> {
        self.find("media", hash).await
    }

    pub async fn save_media(&self, record: &MediaRecord) -> Result<()> {
        self.save("media", &record.hash, record).await
    }

    pub async fn touch_media_sighting(
        &self,
        hash: &str,
        now: DateTime<Utc>,
        promote: Option<(MediaStatus, String)>,
    ) -> Result<Option<MediaRecord>> {
        let collection = self.raw("media");
        let filter = doc! { "_id": hash };
        let updated = if let Some((status, origin)) = promote {
            let seen = doc! { "$eq": ["$status", bson::to_bson(&MediaStatus::Seen)?] };
            // All expressions in this stage read the same pre-update document.
            let update = vec![doc! { "$set": {
                "sightings": { "$toLong": { "$add": ["$sightings", 1_i64] } },
                "last_seen": bson_dt(now),
                "status": { "$cond": [seen.clone(), bson::to_bson(&status)?, "$status"] },
                "origin": { "$cond": [seen, doc! { "$literal": origin }, "$origin"] },
            } }];
            collection
                .find_one_and_update(filter, update)
                .return_document(ReturnDocument::After)
                .await?
        } else {
            let update =
                doc! { "$inc": { "sightings": 1_i64 }, "$set": { "last_seen": bson_dt(now) } };
            collection
                .find_one_and_update(filter, update)
                .return_document(ReturnDocument::After)
                .await?
        };
        updated
            .map(|doc| from_bson_doc(doc.to_document()?))
            .transpose()
    }

    pub async fn record_media_sent(&self, hash: &str, now: DateTime<Utc>) -> Result<()> {
        self.collection("media")
            .update_one(
                doc! { "_id": hash },
                doc! { "$inc": { "sent_count": 1_i64 }, "$set": { "last_sent": bson_dt(now) } },
            )
            .await?;
        Ok(())
    }

    pub async fn update_media_caption(
        &self,
        hash: &str,
        patch: &MediaCaptionPatch,
    ) -> Result<bool> {
        let fields = doc! {
            "description": &patch.description, "text": &patch.text, "use_when": &patch.use_when,
            "avoid": &patch.avoid, "tags": &patch.tags,
        };
        let result = self
            .collection("media")
            .update_one(
                doc! { "_id": hash, "status": bson::to_bson(&MediaStatus::Active)? },
                doc! { "$set": fields },
            )
            .await?;
        Ok(result.matched_count == 1)
    }

    pub async fn patch_media(
        &self,
        hash: &str,
        patch: &MediaEditPatch,
    ) -> Result<Option<MediaRecord>> {
        let mut fields = Document::new();
        macro_rules! set_fields {
            ($($field:ident),*) => {
                $(if let Some(value) = &patch.$field {
                    fields.insert(stringify!($field), bson::to_bson(value)?);
                })*
            };
        }
        set_fields!(description, use_when, avoid, tags, scope, status);
        if fields.is_empty() {
            return self.media(hash).await;
        }
        self.raw("media")
            .find_one_and_update(doc! { "_id": hash }, doc! { "$set": fields })
            .return_document(ReturnDocument::After)
            .await?
            .map(|doc| from_bson_doc(doc.to_document()?))
            .transpose()
    }

    pub async fn all_media(&self) -> Result<Vec<MediaRecord>> {
        self.scan("media").await
    }

    pub async fn delete_media(&self, hash: &str) -> Result<()> {
        self.collection("media")
            .delete_one(doc! { "_id": hash })
            .await?;
        self.delete_sticker_vector(hash).await
    }

    pub async fn delete_unused_media(&self, hash: &str, cutoff: DateTime<Utc>) -> Result<bool> {
        let result = self.collection("media")
            .delete_one(doc! {
                "_id": hash,
                "status": { "$in": [bson::to_bson(&MediaStatus::Seen)?, bson::to_bson(&MediaStatus::Rejected)?] },
                "last_seen": { "$lt": bson_dt(cutoff) },
            })
            .await?;
        if result.deleted_count == 0 {
            return Ok(false);
        }
        self.delete_sticker_vector(hash).await?;
        Ok(true)
    }

    async fn delete_sticker_vector(&self, hash: &str) -> Result<()> {
        self.collection("sticker_vectors")
            .delete_one(doc! { "_id": hash })
            .await?;
        Ok(())
    }

    pub async fn sticker_vectors(&self, model_id: &str) -> Result<BTreeMap<String, StickerVector>> {
        let cursor = self
            .raw("sticker_vectors")
            .find(doc! { "model_id": model_id })
            .await?;
        let vectors: Vec<StickerVector> = collect(cursor, from_bson_doc).await?;
        Ok(vectors.into_iter().map(|v| (v.hash.clone(), v)).collect())
    }

    pub async fn save_sticker_vector(&self, vector: &StickerVector) -> Result<()> {
        self.save("sticker_vectors", &vector.hash, vector).await
    }
}

async fn collect<T>(
    mut cursor: Cursor<RawDocumentBuf>,
    decode: impl Fn(Document) -> Result<T>,
) -> Result<Vec<T>> {
    let mut records = Vec::new();
    while cursor.advance().await? {
        records.push(decode(cursor.deserialize_current()?.to_document()?)?);
    }
    Ok(records)
}

#[cfg(test)]
mod test {
    use super::super::tests::{
        audit, call, contract, episode, json_payload, media, message, person, review, time, vector,
    };
    use super::*;


    fn wire(doc: &Document) -> Result<Document> {
        Ok(RawDocumentBuf::from_document(doc)?.to_document()?)
    }

    fn round_trip<T: Serialize + DeserializeOwned>(value: &T) -> Result<(Document, T)> {
        let doc = to_bson_doc(value, None)?;
        let decoded = from_bson_doc(wire(&doc)?)?;
        Ok((doc, decoded))
    }

    #[test]
    fn native_records_keep_millisecond_dates_and_signed_counters() -> Result<()> {
        let ms = truncate_ms(time());
        let (doc, record) = round_trip(&message("self", "text", time()))?;
        assert_eq!(
            doc.get_datetime("ts")?.timestamp_millis(),
            ms.timestamp_millis()
        );
        assert_eq!(doc.get_str("ns")?, "self");
        assert_eq!(record.ts, ms);
        assert_eq!(record.text, "text");

        let (doc, record) = round_trip(&call(time(), 0.5))?;
        assert_eq!(doc.get_i64("uncached_input")?, i64::MAX);
        assert_eq!(record.uncached_input, i64::MAX as u64);
        assert_eq!(record.ts, ms);
        assert_eq!(record.cost_usd, 0.5);

        let (doc, record) = round_trip(&episode())?;
        assert!(doc.get_datetime("ts").is_ok());
        assert_eq!(record.ts, ms);
        assert_eq!(record.summary, "memory");
        let (doc, record) = round_trip(&audit())?;
        assert!(doc.get_datetime("ts").is_ok());
        assert_eq!(record.ts, ms);
        assert_eq!(record.outcome, Outcome::Sent);

        let (doc, record) = round_trip(&person())?;
        assert!(doc.get_datetime("last_seen").is_ok());
        assert!(doc.get_datetime("last_update").is_ok());
        assert!(
            doc.get_array("facts")?[0]
                .as_document()
                .unwrap()
                .get_datetime("ts")
                .is_ok()
        );
        assert_eq!(record.last_update_day, Some(ms.date_naive()));
        assert_eq!(record.last_update, Some(ms));
        assert_eq!(record.facts[0].ts, ms);

        let media = MediaRecord {
            sent_count: i64::MAX as u64,
            last_sent: Some(time()),
            ..media()
        };
        let (doc, record) = round_trip(&media)?;
        assert_eq!(doc.get_i64("sent_count")?, i64::MAX);
        assert_eq!(doc.get_str("status")?, "seen");
        assert!(doc.get_datetime("first_seen").is_ok());
        assert!(doc.get_datetime("last_seen").is_ok());
        assert!(doc.get_datetime("last_sent").is_ok());
        assert_eq!(record.last_sent, Some(ms));
        assert_eq!(record.last_seen, ms);
        assert!(
            round_trip(&MediaRecord {
                last_sent: None,
                ..media
            })?
            .1
            .last_sent
            .is_none()
        );

        let (_, record) = round_trip(&vector())?;
        assert_eq!(record.vector, vector().vector);
        assert_eq!(record.text_hash, vector().text_hash);
        Ok(())
    }

    #[test]
    fn reviews_and_kv_preserve_literal_json_and_unsigned_numbers() -> Result<()> {
        let mut review = review().with_id(i64::MAX as u64);
        review.ts = time();
        let doc = review_doc(&review)?;
        assert_eq!(doc.get_i64("_id")?, i64::MAX);
        assert_eq!(doc.get_i64("id")?, i64::MAX);
        assert!(doc.get_datetime("ts").is_ok());
        let record = decode_review(wire(&doc)?)?;
        assert_eq!(record.ts, truncate_ms(time()));
        assert_eq!(record.payload, json_payload());

        for value in [
            json_payload(),
            serde_json::json!({"$numberLong": "42"}),
            Value::Null,
        ] {
            let doc = kv_doc("key", &value)?;
            assert_eq!(doc.get_str("_id")?, "key");
            assert_eq!(decode_kv(wire(&doc)?)?, value);
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires AGLI_TEST_MONGO_URL and permission to create/drop a test database"]
    async fn live_mongo_contract() -> Result<()> {
        let url = std::env::var("AGLI_TEST_MONGO_URL").context("set AGLI_TEST_MONGO_URL")?;
        let cfg = MongoCfg {
            url: Some(url),
            database: format!("aglibot_test_{}", bson::oid::ObjectId::new()),
            ..Default::default()
        };
        let store = Store::open(&cfg).await?;
        let StoreBackend::Mongo(mongo) = store.0.as_ref() else {
            unreachable!()
        };
        let cleanup = mongo.clone();
        let task = tokio::spawn(async move { contract(&store).await });
        // Run cleanup even if a contract assertion panics.
        let result = task.await;
        cleanup.db.drop().await?;
        result?
    }
}
