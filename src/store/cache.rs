//! Per-store, in-process source cache. Privacy and query-dependent ranking still run on
//! every recall. The cache lock spans reads and mutations, so a stale load cannot win
//! a race against invalidation. External database/file writers require reopening the store.
use std::collections::BTreeMap;
use std::sync::Mutex;

use anyhow::Result;
use super::{Episode, EpisodeEmbedding, Lesson, Person};

#[derive(Default)]
pub(super) struct Data {
    pub episodes: Option<Vec<Episode>>,
    pub active_episodes: Option<Vec<Episode>>,
    pub embeddings: Option<BTreeMap<String, EpisodeEmbedding>>,
    pub persons: Option<BTreeMap<String, Person>>,
    pub lessons: Option<Vec<Lesson>>,
}

#[derive(Default)]
struct State {
    generation: u64,
    data: Data,
    loads: u64,
}

#[derive(Default)]
pub(super) struct Cache(Mutex<State>);

impl Cache {
    pub fn read<T: Clone>(
        &self, name: &str, slot: impl FnOnce(&mut Data) -> &mut Option<T>,
        load: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let generation = state.generation;
        let entry = slot(&mut state.data);
        if let Some(value) = entry {
            tracing::debug!(name, generation, "recall source cache hit");
            return Ok(value.clone());
        }
        tracing::debug!(name, generation, "recall source cache miss");
        let value = load()?;
        *entry = Some(value.clone());
        state.loads += 1;
        Ok(value)
    }

    pub fn write(&self, write: impl FnOnce() -> Result<()>) -> Result<()> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // Also invalidate on failure: an I/O error may follow a partial write.
        state.generation = state.generation.wrapping_add(1);
        state.data = Data::default();
        write()
    }

    #[cfg(test)]
    pub fn counts(&self) -> (u64, u64) {
        let state = self.0.lock().unwrap();
        (state.generation, state.loads)
    }
}
