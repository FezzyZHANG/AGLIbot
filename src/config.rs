//! Loads `config/*.yaml`. Secrets never live here: providers name the env var that holds the key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    OpenaiCompat,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderCfg {
    pub kind: ProviderKind,
    #[serde(default)]
    pub base_url: Option<String>,
    /// Optional environment override, useful for plan/region-specific endpoints.
    #[serde(default)]
    pub base_url_env: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
}

/// $/M tokens.
#[derive(Debug, Clone, Deserialize)]
pub struct Price {
    pub input: f64,
    pub cached: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

/// One row of the QCL table.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub q: f64,
    pub c: Price,
    pub l: f64,
    #[serde(default)]
    pub local: bool,
    #[serde(default)]
    pub fallbacks: Option<String>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub enable_thinking: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelsCfg {
    pub daily_budget_usd: f64,
    pub providers: BTreeMap<String, ProviderCfg>,
    pub models: Vec<ModelEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Cheap,
    Fast,
    Quality,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    #[serde(default)]
    pub q_min: f64,
    pub priority: Priority,
    #[serde(default)]
    #[allow(dead_code)] // read by judge.rs (phase 8)
    pub review: bool,
    #[serde(default)]
    pub budget_exempt: bool,
    #[serde(default)]
    pub effort: Option<String>,
}

pub type ScenariosCfg = BTreeMap<String, Scenario>;

#[derive(Debug, Clone, Deserialize)]
pub struct Owner {
    pub person_id: String,
    /// Env var holding the owner's numeric Telegram id; read once by `Config::load`.
    pub telegram_user_id_env: String,
    /// 0 until resolved, and stays 0 when the env var is unset: then nobody is the owner.
    #[serde(skip)]
    pub telegram_user_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AclCfg {
    pub owner: Owner,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramCfg {
    pub poll_timeout_s: u64,
    /// Env var holding the proxy URL; unset or empty means no explicit proxy.
    #[serde(default)]
    pub proxy_env: Option<String>,
    #[serde(skip)]
    pub proxy: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelsCfg {
    pub telegram: TelegramCfg,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionCfg {
    pub max_turns: usize,
    pub idle_minutes: i64,
    pub recap_messages: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeCfg {
    pub data_dir: PathBuf,
    pub mind_dir: PathBuf,
    pub session: SessionCfg,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub root: PathBuf,
    pub models: ModelsCfg,
    pub scenarios: ScenariosCfg,
    pub acl: AclCfg,
    pub channels: ChannelsCfg,
    pub runtime: RuntimeCfg,
}

impl Config {
    pub fn load(root: &Path) -> Result<Self> {
        let dir = root.join("config");
        let mut cfg = Config {
            root: root.to_path_buf(),
            models: read_yaml(&dir.join("models.yaml"))?,
            scenarios: read_yaml(&dir.join("scenarios.yaml"))?,
            acl: read_yaml(&dir.join("acl.yaml"))?,
            channels: read_yaml(&dir.join("channels.yaml"))?,
            runtime: read_yaml(&dir.join("runtime.yaml"))?,
        };
        cfg.resolve_env()?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn mind_dir(&self) -> PathBuf {
        self.root.join(&self.runtime.mind_dir)
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join(&self.runtime.data_dir)
    }

    /// Machine- and person-specific values live in env vars (or `.env`), not in the repo.
    fn resolve_env(&mut self) -> Result<()> {
        let owner = &mut self.acl.owner;
        if let Some(id) = env_value(&owner.telegram_user_id_env) {
            owner.telegram_user_id =
                id.parse().with_context(|| format!("{} must be a numeric Telegram id", owner.telegram_user_id_env))?;
        }
        let tg = &mut self.channels.telegram;
        tg.proxy = tg.proxy_env.as_deref().and_then(env_value);
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        for m in &self.models.models {
            if !self.models.providers.contains_key(&m.provider) {
                bail!("model `{}` references unknown provider `{}`", m.id, m.provider);
            }
        }
        if !self.scenarios.contains_key("chat") {
            bail!("config/scenarios.yaml must define a `chat` scenario");
        }
        Ok(())
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn read_yaml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_yaml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_config_loads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let cfg = Config::load(root).expect("config/ should parse");
        assert!(!cfg.models.models.is_empty());
        assert!(cfg.scenarios.contains_key("verify"));
    }
}
