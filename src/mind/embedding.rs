//! Optional derived episode vectors. No startup backfill and no vector-server dependency.

use anyhow::{Result, ensure};
use serde::Deserialize;
use tracing::info;

use super::models::Models;
use crate::ns::ProcessingPolicy;
use crate::store::{Episode, EpisodeEmbedding, Store};

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingCfg {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub local: bool,
    #[serde(default)]
    pub input_usd_per_million: f64,
    #[serde(default = "default_hits")]
    pub max_cross_namespace: usize,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    #[serde(default = "default_cap")]
    pub nightly_cap: usize,
}

fn default_hits() -> usize { 3 }
fn default_threshold() -> f64 { 0.75 }
fn default_cap() -> usize { 200 }

impl EmbeddingCfg {
    pub fn model_id(&self) -> String { format!("{}/{}", self.provider, self.model) }

    pub fn allows(&self, policy: &ProcessingPolicy) -> bool {
        match policy {
            ProcessingPolicy::Any => true,
            ProcessingPolicy::LocalOnly => self.local,
            ProcessingPolicy::Providers { names } => names.contains(&self.provider),
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(!self.provider.trim().is_empty() && !self.model.trim().is_empty(), "embedding needs provider and model");
        ensure!(self.threshold.is_finite() && (0.0..=1.0).contains(&self.threshold), "embedding threshold must be in [0, 1]");
        ensure!(self.input_usd_per_million.is_finite() && self.input_usd_per_million >= 0.0, "embedding price must be nonnegative and finite");
        Ok(())
    }
}

pub fn valid_vector(vector: &[f32]) -> bool {
    !vector.is_empty() && vector.iter().all(|x| x.is_finite()) && vector.iter().any(|x| *x != 0.0)
}

pub fn cosine(a: &[f32], b: &[f32]) -> Result<f64> {
    ensure!(a.len() == b.len() && valid_vector(a) && valid_vector(b), "invalid or incompatible embedding vectors");
    let dot: f64 = a.iter().zip(b).map(|(&x, &y)| f64::from(x) * f64::from(y)).sum();
    let norm = |v: &[f32]| v.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>().sqrt();
    Ok((dot / (norm(a) * norm(b))).clamp(-1.0, 1.0))
}

/// Called after extraction's watermark is durable. A failed cache write never repeats extraction.
pub async fn fill(models: &Models, store: &dyn Store, episodes: &[Episode], cap: usize) -> Result<usize> {
    let Some(cfg) = models.embedding() else { return Ok(0) };
    if cap == 0 { return Ok(0) }
    let model_id = cfg.model_id();
    let mut existing = store.load_embeddings(&model_id)?;
    let mut filled = 0;
    for episode in episodes {
        if filled == cap { break }
        let policy = ProcessingPolicy::default_for(&episode.scope);
        if existing.contains_key(&episode.id) || !cfg.allows(&policy) { continue }
        let Some(vector) = models.embed(&policy, &episode.summary).await? else { continue };
        store.save_embedding(&EpisodeEmbedding { episode_id: episode.id.clone(), model_id: model_id.clone(), vector: vector.clone() })?;
        existing.insert(episode.id.clone(), vector);
        filled += 1;
    }
    Ok(filled)
}

pub async fn nightly(models: &Models, store: &dyn Store) {
    let Some(cfg) = models.embedding() else { return };
    let result = async { fill(models, store, &store.episodes()?, cfg.nightly_cap).await }.await;
    match result {
        Ok(filled) => info!(filled, cap = cfg.nightly_cap, "nightly episode embeddings"),
        Err(e) => models.warn_embedding(&e),
    }
}

#[cfg(test)]
mod tests;
