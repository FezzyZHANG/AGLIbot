use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::{Result, anyhow, ensure};

use super::*;

#[derive(Default)]
struct MemData {
    messages: Vec<MessageRecord>,
    model_calls: Vec<ModelCallRecord>,
    episodes: Vec<Episode>,
    persons: BTreeMap<String, Person>,
    audits: Vec<AuditRecord>,
    reviews: BTreeMap<u64, ReviewItem>,
    kv: BTreeMap<String, Value>,
    lexicon: Vec<Value>,
    media: BTreeMap<String, MediaRecord>,
    sticker_vectors: BTreeMap<String, StickerVector>,
}

pub(super) struct MemStore {
    data: Arc<RwLock<MemData>>,
    review_id: AtomicU64,
}

impl Default for MemStore {
    fn default() -> Self {
        Self {
            data: Arc::default(),
            review_id: AtomicU64::new(1),
        }
    }
}

impl MemStore {
    fn read(&self) -> Result<std::sync::RwLockReadGuard<'_, MemData>> {
        self.data
            .read()
            .map_err(|_| anyhow!("in-memory store lock poisoned"))
    }

    fn write(&self) -> Result<std::sync::RwLockWriteGuard<'_, MemData>> {
        self.data
            .write()
            .map_err(|_| anyhow!("in-memory store lock poisoned"))
    }

    pub fn append_message(&self, m: &MessageRecord) -> Result<()> {
        self.write()?.messages.push(MessageRecord {
            ts: truncate_ms(m.ts),
            ..m.clone()
        });
        Ok(())
    }

    pub fn recent_in_ns(&self, ns: &Namespace, n: usize) -> Result<Vec<MessageRecord>> {
        let mut records = self.messages(ns, |_| true)?;
        Ok(records.split_off(records.len().saturating_sub(n)))
    }

    pub fn messages_after(
        &self,
        ns: &Namespace,
        after: DateTime<Utc>,
    ) -> Result<Vec<MessageRecord>> {
        let after = truncate_ms(after);
        self.messages(ns, |m| m.ts > after)
    }

    fn messages(
        &self,
        ns: &Namespace,
        matches: impl Fn(&MessageRecord) -> bool,
    ) -> Result<Vec<MessageRecord>> {
        let mut records: Vec<_> = self
            .read()?
            .messages
            .iter()
            .filter(|m| &m.ns == ns && matches(m))
            .cloned()
            .collect();
        records.sort_by_key(|m| m.ts);
        Ok(records)
    }

    pub fn active_namespaces(&self, after: DateTime<Utc>) -> Result<Vec<Namespace>> {
        let after = truncate_ms(after);
        let mut namespaces: Vec<_> = self
            .read()?
            .messages
            .iter()
            .filter(|m| m.ts > after)
            .map(|m| m.ns.clone())
            .collect();
        namespaces.sort();
        namespaces.dedup();
        Ok(namespaces)
    }

    pub fn append_model_call(&self, c: &ModelCallRecord) -> Result<()> {
        for value in [
            c.uncached_input,
            c.cached_input,
            c.cache_write,
            c.output,
            c.latency_ms,
        ] {
            i64::try_from(value).context("model call counter exceeds BSON Int64")?;
        }
        self.write()?.model_calls.push(ModelCallRecord {
            ts: truncate_ms(c.ts),
            ..c.clone()
        });
        Ok(())
    }

    pub fn spent_since(&self, since: DateTime<Utc>) -> Result<f64> {
        let since = truncate_ms(since);
        Ok(self
            .read()?
            .model_calls
            .iter()
            .filter(|c| c.ts >= since)
            .map(|c| c.cost_usd)
            .sum())
    }

    #[cfg(test)]
    pub fn model_calls(&self) -> Result<Vec<ModelCallRecord>> {
        Ok(self.read()?.model_calls.clone())
    }

    pub fn append_episode(&self, e: &Episode) -> Result<()> {
        self.write()?.episodes.push(Episode {
            ts: truncate_ms(e.ts),
            ..e.clone()
        });
        Ok(())
    }

    pub fn episodes(&self) -> Result<Vec<Episode>> {
        Ok(self.read()?.episodes.clone())
    }

    pub fn persons(&self) -> Result<BTreeMap<String, Person>> {
        Ok(self.read()?.persons.clone())
    }

    pub fn save_person(&self, p: &Person) -> Result<()> {
        let mut person = p.clone();
        person.last_seen = truncate_ms(person.last_seen);
        person.last_update = person.last_update.map(truncate_ms);
        for fact in &mut person.facts {
            fact.ts = truncate_ms(fact.ts);
        }
        self.write()?
            .persons
            .insert(person.person_id.clone(), person);
        Ok(())
    }

    pub fn append_audit(&self, a: &AuditRecord) -> Result<()> {
        self.write()?.audits.push(AuditRecord {
            ts: truncate_ms(a.ts),
            ..a.clone()
        });
        Ok(())
    }

    pub fn audits(&self) -> Result<Vec<AuditRecord>> {
        Ok(self.read()?.audits.clone())
    }

    pub fn reviews(&self) -> Result<Vec<ReviewItem>> {
        Ok(self.read()?.reviews.values().cloned().collect())
    }

    pub fn create_review(&self, new: NewReview) -> Result<ReviewItem> {
        let id = self
            .review_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
                i64::try_from(id).ok().map(|_| id + 1)
            })
            .map_err(|_| anyhow!("review ID exceeds BSON Int64"))?;
        let item = new.with_id(id);
        self.write()?.reviews.insert(id, item.clone());
        Ok(item)
    }

    pub fn save_review(&self, r: &ReviewItem) -> Result<()> {
        let mut data = self.write()?;
        let slot = data
            .reviews
            .get_mut(&r.id)
            .context("review does not exist")?;
        *slot = ReviewItem {
            ts: truncate_ms(r.ts),
            ..r.clone()
        };
        Ok(())
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<Value>> {
        Ok(self.read()?.kv.get(key).cloned())
    }

    pub fn kv_set(&self, key: &str, value: Value) -> Result<()> {
        self.write()?.kv.insert(key.into(), value);
        Ok(())
    }

    pub fn lexicon(&self) -> Result<Vec<Value>> {
        Ok(self.read()?.lexicon.clone())
    }

    pub fn insert_lexicon(&self, docs: &[Value]) -> Result<()> {
        let keys = docs.iter().map(lexicon_key).collect::<Result<Vec<_>>>()?;
        let mut data = self.write()?;
        for (doc, key) in docs.iter().zip(keys) {
            ensure!(
                !data
                    .lexicon
                    .iter()
                    .any(|entry| lexicon_key(entry).ok() == Some(key)),
                "duplicate lexicon entry {key:?}"
            );
            let mut doc = doc.clone();
            if let Some(fields) = doc.as_object_mut() {
                fields.remove("_id");
            }
            data.lexicon.push(doc);
        }
        Ok(())
    }

    pub fn media(&self, hash: &str) -> Result<Option<MediaRecord>> {
        Ok(self.read()?.media.get(hash).cloned())
    }

    pub fn save_media(&self, record: &MediaRecord) -> Result<()> {
        i64::try_from(record.sightings).context("media sightings exceeds BSON Int64")?;
        i64::try_from(record.sent_count).context("media sent count exceeds BSON Int64")?;
        let record = MediaRecord {
            first_seen: truncate_ms(record.first_seen),
            last_seen: truncate_ms(record.last_seen),
            last_sent: record.last_sent.map(truncate_ms),
            ..record.clone()
        };
        self.write()?.media.insert(record.hash.clone(), record);
        Ok(())
    }

    pub fn touch_media_sighting(
        &self,
        hash: &str,
        now: DateTime<Utc>,
        promote: Option<(MediaStatus, String)>,
    ) -> Result<Option<MediaRecord>> {
        let mut data = self.write()?;
        let Some(record) = data.media.get_mut(hash) else {
            return Ok(None);
        };
        ensure!(
            record.sightings < i64::MAX as u64,
            "media sightings exceeds BSON Int64"
        );
        record.sightings += 1;
        record.last_seen = truncate_ms(now);
        if let Some((status, origin)) = promote.filter(|_| record.status == MediaStatus::Seen) {
            record.status = status;
            record.origin = origin;
        }
        Ok(Some(record.clone()))
    }

    pub fn record_media_sent(&self, hash: &str, now: DateTime<Utc>) -> Result<()> {
        if let Some(record) = self.write()?.media.get_mut(hash) {
            ensure!(
                record.sent_count < i64::MAX as u64,
                "media sent count exceeds BSON Int64"
            );
            record.sent_count += 1;
            record.last_sent = Some(truncate_ms(now));
        }
        Ok(())
    }

    pub fn update_media_caption(&self, hash: &str, patch: &MediaCaptionPatch) -> Result<bool> {
        let mut data = self.write()?;
        let Some(record) = data
            .media
            .get_mut(hash)
            .filter(|r| r.status == MediaStatus::Active)
        else {
            return Ok(false);
        };
        record.description.clone_from(&patch.description);
        record.text.clone_from(&patch.text);
        record.use_when.clone_from(&patch.use_when);
        record.avoid.clone_from(&patch.avoid);
        record.tags.clone_from(&patch.tags);
        Ok(true)
    }

    pub fn patch_media(&self, hash: &str, patch: &MediaEditPatch) -> Result<Option<MediaRecord>> {
        let mut data = self.write()?;
        let Some(record) = data.media.get_mut(hash) else {
            return Ok(None);
        };
        macro_rules! apply_fields {
            ($($field:ident),*) => {
                $(if let Some(value) = &patch.$field { record.$field.clone_from(value); })*
            };
        }
        apply_fields!(description, use_when, avoid, tags, scope, status);
        Ok(Some(record.clone()))
    }

    pub fn all_media(&self) -> Result<Vec<MediaRecord>> {
        Ok(self.read()?.media.values().cloned().collect())
    }

    pub fn delete_media(&self, hash: &str) -> Result<()> {
        let mut data = self.write()?;
        data.media.remove(hash);
        data.sticker_vectors.remove(hash);
        Ok(())
    }

    pub fn sticker_vectors(&self, model_id: &str) -> Result<BTreeMap<String, StickerVector>> {
        Ok(self
            .read()?
            .sticker_vectors
            .iter()
            .filter(|(_, v)| v.model_id == model_id)
            .map(|(hash, vector)| (hash.clone(), vector.clone()))
            .collect())
    }

    pub fn save_sticker_vector(&self, vector: &StickerVector) -> Result<()> {
        self.write()?
            .sticker_vectors
            .insert(vector.hash.clone(), vector.clone());
        Ok(())
    }
}
