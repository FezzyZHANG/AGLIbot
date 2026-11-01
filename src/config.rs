//! Loads `config/*.yaml`. Secrets and person/machine-specific values never live here: the YAML
//! names the env var that holds them, and `Config::load` resolves those once.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::NaiveTime;
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
    /// Documents which scenarios go through judge.rs; the flows that need review call it explicitly.
    #[serde(default)]
    #[allow(dead_code)]
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
    /// Env var holding the owner's QQ number.
    #[serde(default)]
    pub qq_user_id_env: Option<String>,
    /// 0 until resolved, and stays 0 when the env var is unset: then nobody is the owner there.
    #[serde(skip)]
    pub telegram_user_id: i64,
    #[serde(skip)]
    pub qq_user_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AclCfg {
    pub owner: Owner,
    /// Platform ids (`qq:123`, `telegram:456`) known to be bots/agents.
    #[serde(default)]
    pub known_agents: Vec<String>,
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
pub struct QqCfg {
    pub enabled: bool,
    /// Env vars for the OneBot v11 forward WebSocket (NapCat) and its access token.
    pub url_env: String,
    pub token_env: String,
    #[serde(skip)]
    pub url: Option<String>,
    #[serde(skip)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelsCfg {
    pub telegram: TelegramCfg,
    pub qq: QqCfg,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FloorCfg {
    /// Ask the triage model whether to chime in after this many unanswered group messages.
    pub check_every: usize,
    /// Minimum minutes between two unsolicited messages in one group.
    pub unsolicited_cooldown_min: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TurnLimit {
    pub max_replies: usize,
    pub window_min: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SocialCfg {
    /// Group chats he takes part in, as `platform:chat_id`. Other groups are ignored entirely.
    #[serde(default)]
    pub groups: Vec<String>,
    /// Whether people other than the owner may chat with him in private.
    pub private_from_others: bool,
    pub floor: FloorCfg,
    /// Reply caps per (chat, sender) over a sliding window.
    pub human_limit: TurnLimit,
    pub agent_limit: TurnLimit,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionCfg {
    pub max_turns: usize,
    pub idle_minutes: i64,
    pub recap_messages: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RoutineCfg {
    pub sleep_at: NaiveTime,
    pub wake_at: NaiveTime,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HeartbeatCfg {
    pub tick_s: u64,
    /// Minimum hours between two messages he starts on his own.
    pub proactive_gap_hours: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryCfg {
    /// Extract episodes from a chat once this many messages are unprocessed (also nightly).
    pub extract_after: usize,
    pub episodes_in_context: usize,
    pub facts_in_context: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateTool {
    Claude,
    Codex,
    Off,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HandsCfg {
    pub research: DelegateTool,
    pub code: DelegateTool,
    pub claude_budget_usd: f64,
    pub timeout_min: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JudgeCfg {
    pub digest_at: NaiveTime,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeCfg {
    pub data_dir: PathBuf,
    pub mind_dir: PathBuf,
    pub work_dir: PathBuf,
    pub session: SessionCfg,
    pub routine: RoutineCfg,
    pub heartbeat: HeartbeatCfg,
    pub memory: MemoryCfg,
    pub hands: HandsCfg,
    pub judge: JudgeCfg,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub root: PathBuf,
    pub models: ModelsCfg,
    pub scenarios: ScenariosCfg,
    pub acl: AclCfg,
    pub channels: ChannelsCfg,
    pub social: SocialCfg,
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
            social: read_yaml(&dir.join("social.yaml"))?,
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

    pub fn work_dir(&self) -> PathBuf {
        self.root.join(&self.runtime.work_dir)
    }

    /// Machine- and person-specific values live in env vars (or `.env`), not in the repo.
    fn resolve_env(&mut self) -> Result<()> {
        let owner = &mut self.acl.owner;
        owner.telegram_user_id = env_id(&owner.telegram_user_id_env)?;
        if let Some(var) = &owner.qq_user_id_env {
            owner.qq_user_id = env_id(var)?;
        }
        let tg = &mut self.channels.telegram;
        tg.proxy = tg.proxy_env.as_deref().and_then(env_value);
        let qq = &mut self.channels.qq;
        qq.url = env_value(&qq.url_env);
        qq.token = env_value(&qq.token_env);
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        for m in &self.models.models {
            if !self.models.providers.contains_key(&m.provider) {
                bail!("model `{}` references unknown provider `{}`", m.id, m.provider);
            }
        }
        for name in ["chat", "triage", "extract", "consolidate", "research_plan", "verify", "synthesize"] {
            if !self.scenarios.contains_key(name) {
                bail!("config/scenarios.yaml must define a `{name}` scenario");
            }
        }
        Ok(())
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn env_id(name: &str) -> Result<i64> {
    match env_value(name) {
        None => Ok(0),
        Some(v) => v.parse().with_context(|| format!("{name} must be a numeric id")),
    }
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
        assert_ne!(cfg.runtime.routine.sleep_at, cfg.runtime.routine.wake_at);
    }
}
