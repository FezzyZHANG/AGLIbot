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

#[derive(Debug, Clone, PartialEq, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Price {
    pub input: f64,
    pub cached: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

/// One row of the QCL table.
#[derive(Debug, Clone, PartialEq, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelsCfg {
    #[serde(default)]
    pub embedding: Option<crate::mind::embedding::EmbeddingCfg>,
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AclCfg {
    pub owner: Owner,
    /// Platform ids (`qq:123`, `telegram:456`) known to be bots/agents.
    #[serde(default)]
    pub known_agents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TelegramCfg {
    pub poll_timeout_s: u64,
    /// Env var holding the proxy URL; unset or empty means no explicit proxy.
    #[serde(default)]
    pub proxy_env: Option<String>,
    #[serde(skip)]
    pub proxy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ChannelsCfg {
    pub telegram: TelegramCfg,
    pub qq: QqCfg,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FloorCfg {
    /// Ask the triage model whether to chime in after this many unanswered group messages.
    pub check_every: usize,
    /// Minimum minutes between two unsolicited messages in one group.
    pub unsolicited_cooldown_min: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TurnLimit {
    pub max_replies: usize,
    pub window_min: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SocialCfg {
    /// Reply allowlist, as `platform:chat_id`. Other joined groups are read-only.
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub ignored_groups: Vec<String>,
    #[serde(default)]
    pub read_only_reflect: ReadOnlyReflectCfg,
    /// Whether people other than the owner may chat with him in private.
    pub private_from_others: bool,
    /// CJK characters plus runs of other letters/digits, per complete group reply.
    #[serde(default = "default_group_reply_max")]
    pub group_reply_max: usize,
    #[serde(default = "default_interjection_max")]
    pub interjection_max: usize,
    pub floor: FloorCfg,
    /// Reply caps per (chat, sender) over a sliding window.
    pub human_limit: TurnLimit,
    pub agent_limit: TurnLimit,
}

fn default_group_reply_max() -> usize { 150 }
fn default_interjection_max() -> usize { 80 }

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ReadOnlyReflectCfg {
    pub max_messages_per_night: usize,
    pub max_cost_usd: f64,
}

impl Default for ReadOnlyReflectCfg {
    fn default() -> Self { Self { max_messages_per_night: 400, max_cost_usd: 0.10 } }
}

impl SocialCfg {
    pub fn read_only_group(&self, id: &str) -> bool {
        !self.groups.iter().any(|g| g == id) && !self.ignored_groups.iter().any(|g| g == id)
    }

    pub fn reflects_normally(&self, ns: &crate::ns::Namespace) -> bool {
        match ns {
            crate::ns::Namespace::Group(id) => self.groups.contains(id) && !self.ignored_groups.contains(id),
            _ => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SessionCfg {
    pub max_turns: usize,
    pub idle_minutes: i64,
    pub recap_messages: usize,
    #[serde(default)]
    pub debounce: DebounceCfg,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct DebounceCfg {
    /// 0 disables both waiting and adaptive learning.
    pub quiet_ms: u64,
    pub cap_ms: u64,
    pub adaptive: bool,
    pub alpha: f64,
    pub factor: f64,
    pub min_ms: u64,
    pub max_ms: u64,
}

impl Default for DebounceCfg {
    fn default() -> Self {
        Self { quiet_ms: 2500, cap_ms: 10000, adaptive: true, alpha: 0.3, factor: 2.5, min_ms: 1200, max_ms: 6000 }
    }
}

impl DebounceCfg {
    fn validate(&self) -> Result<()> {
        if self.quiet_ms == 0 { return Ok(()) }
        anyhow::ensure!(self.cap_ms > 0, "session.debounce.cap_ms must be positive (quiet_ms: 0 disables debouncing)");
        if self.adaptive {
            anyhow::ensure!(self.alpha.is_finite() && self.alpha > 0.0 && self.alpha <= 1.0,
                "session.debounce.alpha must be in (0, 1]");
            anyhow::ensure!(self.factor.is_finite() && self.factor > 0.0, "session.debounce.factor must be positive and finite");
            anyhow::ensure!(self.min_ms > 0 && self.min_ms <= self.max_ms && self.max_ms <= self.cap_ms,
                "session.debounce requires 0 < min_ms <= max_ms <= cap_ms");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RoutineCfg {
    pub sleep_at: NaiveTime,
    pub wake_at: NaiveTime,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HeartbeatCfg {
    pub tick_s: u64,
    /// Minimum hours between two messages he starts on his own.
    pub proactive_gap_hours: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MemoryCfg {
    /// Extract episodes from a chat once this many messages are unprocessed (also nightly).
    pub extract_after: usize,
    /// Weekly reflection boundary; defaults to Sunday without editing owner config.
    #[serde(default = "default_weekly_weekday")]
    pub weekly_weekday: chrono::Weekday,
    pub episodes_in_context: usize,
    pub facts_in_context: usize,
    #[serde(default = "default_half_life_days")]
    pub half_life_days: f64,
    #[serde(default = "default_archive_threshold")]
    pub archive_threshold: f64,
    #[serde(default = "default_archive_min_age_days")]
    pub archive_min_age_days: f64,
    #[serde(default = "default_archive_max_groups")]
    pub archive_max_groups: usize,
}

fn default_weekly_weekday() -> chrono::Weekday { chrono::Weekday::Sun }

fn default_half_life_days() -> f64 { 30.0 }
fn default_archive_threshold() -> f64 { 0.05 }
fn default_archive_min_age_days() -> f64 { 30.0 }
fn default_archive_max_groups() -> usize { 20 }

impl MemoryCfg {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.half_life_days.is_finite() && self.half_life_days > 0.0,
            "memory.half_life_days must be positive and finite");
        anyhow::ensure!(self.archive_threshold.is_finite() && (0.0..=1.0).contains(&self.archive_threshold),
            "memory.archive_threshold must be in [0, 1]");
        anyhow::ensure!(self.archive_min_age_days.is_finite() && self.archive_min_age_days >= 0.0,
            "memory.archive_min_age_days must be nonnegative and finite");
        Ok(())
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateTool {
    #[default]
    Codex,
    Dsh,
    Off,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HandsCfg {
    #[serde(default = "default_max_jobs", deserialize_with = "positive_max_jobs")]
    pub max_jobs: usize,
    #[serde(default)]
    pub research: DelegateTool,
    pub code: DelegateTool,
    pub codex_model: String,
    #[serde(default = "default_dsh_profile")]
    pub dsh_profile: String,
    #[serde(default = "default_dsh_home")]
    pub dsh_home: PathBuf,
    pub timeout_min: u64,
    #[serde(default = "default_stall_minutes")]
    pub stall_minutes: u64,
    #[serde(default = "default_total_budget_minutes")]
    pub total_budget_minutes: u64,
}

fn default_max_jobs() -> usize { 1 }

fn positive_max_jobs<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<usize, D::Error> {
    let value = usize::deserialize(deserializer)?;
    if value == 0 { return Err(serde::de::Error::custom("hands.max_jobs must be >= 1")) }
    Ok(value)
}

fn default_stall_minutes() -> u64 { 20 }
fn default_total_budget_minutes() -> u64 { 90 }

fn default_dsh_profile() -> String {
    "agli".into()
}

fn default_dsh_home() -> PathBuf {
    "data/dsh-home".into()
}

impl HandsCfg {
    fn resolve_dsh_home(&mut self, root: &Path) -> Result<()> {
        self.dsh_home = std::path::absolute(root.join(&self.dsh_home)).context("resolving AGLI DSH home")?;
        Ok(())
    }

    /// Fail at load time rather than when the first task starts.
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.code != DelegateTool::Dsh, "hands.code: dsh is not supported; DSH is research-only (use codex or off)");
        for (name, minutes) in [("timeout_min", self.timeout_min), ("stall_minutes", self.stall_minutes),
            ("total_budget_minutes", self.total_budget_minutes)] {
            anyhow::ensure!(minutes > 0, "hands.{name} must be > 0");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct JudgeCfg {
    pub digest_at: NaiveTime,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
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
    #[serde(default)]
    pub mongo: MongoCfg,
}

#[derive(Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct MongoCfg {
    pub url_env: String,
    pub database_env: String,
    #[serde(skip)]
    pub url: Option<String>,
    #[serde(skip)]
    pub database: String,
}

impl Default for MongoCfg {
    fn default() -> Self {
        Self {
            url_env: "MONGO_URL".into(),
            database_env: "MONGO_DATABASE".into(),
            url: None,
            database: "aglibot".into(),
        }
    }
}

impl std::fmt::Debug for MongoCfg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MongoCfg")
            .field("url_env", &self.url_env)
            .field("database_env", &self.database_env)
            .field("url", &self.url.as_ref().map(|_| "[redacted]"))
            .field("database", &self.database)
            .finish()
    }
}

impl MongoCfg {
    fn resolve_env(&mut self, value: impl Fn(&str) -> Option<String>) {
        self.url = value(&self.url_env);
        self.database = value(&self.database_env).unwrap_or_else(|| "aglibot".into());
    }
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
        cfg.runtime.hands.resolve_dsh_home(root)?;
        cfg.resolve_env()?;
        cfg.validate()?;
        crate::ns::secrets::register_config(&cfg);
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
        self.runtime.mongo.resolve_env(env_value);
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        self.runtime.session.debounce.validate()?;
        self.runtime.memory.validate()?;
        self.runtime.hands.validate()?;
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
    fn hands_validation_rejects_dsh_code_and_zero_minutes() {
        let base = "research: dsh
code: codex
codex_model: m
timeout_min: 30
";
        let hands: HandsCfg = serde_yaml::from_str(base).unwrap();
        hands.validate().unwrap();
        let hands: HandsCfg = serde_yaml::from_str(&base.replace("code: codex", "code: dsh")).unwrap();
        assert!(hands.validate().unwrap_err().to_string().contains("research-only"));
        for key in ["timeout_min: 0", "stall_minutes: 0", "total_budget_minutes: 0"] {
            let yaml = if key.starts_with("timeout") { base.replace("timeout_min: 30", key) } else { format!("{base}{key}
") };
            let hands: HandsCfg = serde_yaml::from_str(&yaml).unwrap();
            assert!(hands.validate().is_err(), "{key}");
        }
    }

    #[test]
    fn memory_decay_defaults_overrides_and_validation() {
        let yaml = "extract_after: 10\nepisodes_in_context: 5\nfacts_in_context: 5\n";
        let memory: MemoryCfg = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(memory.half_life_days, 30.0);
        assert_eq!(memory.archive_threshold, 0.05);
        assert_eq!(memory.archive_min_age_days, 30.0);
        assert_eq!(memory.archive_max_groups, 20);
        memory.validate().unwrap();
        let mut memory: MemoryCfg = serde_yaml::from_str(&format!("{yaml}half_life_days: 10\narchive_threshold: 0.1\narchive_min_age_days: 7\narchive_max_groups: 0\n")).unwrap();
        assert_eq!(memory.half_life_days, 10.0);
        assert_eq!(memory.archive_threshold, 0.1);
        assert_eq!(memory.archive_min_age_days, 7.0);
        assert_eq!(memory.archive_max_groups, 0);
        memory.validate().unwrap();
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            memory.half_life_days = bad;
            assert!(memory.validate().is_err());
        }
        memory.half_life_days = 30.0;
        memory.archive_threshold = 1.1;
        assert!(memory.validate().is_err());
        memory.archive_threshold = 0.05;
        memory.archive_min_age_days = -1.0;
        assert!(memory.validate().is_err());
    }

    #[test]
    fn social_reply_limits_default_and_override() {
        let yaml = "private_from_others: true\nfloor: {check_every: 3, unsolicited_cooldown_min: 30}\nhuman_limit: {max_replies: 5, window_min: 10}\nagent_limit: {max_replies: 2, window_min: 10}\n";
        let cfg: SocialCfg = serde_yaml::from_str(yaml).unwrap();
        assert_eq!((cfg.group_reply_max, cfg.interjection_max), (150, 80));
        let cfg: SocialCfg = serde_yaml::from_str(&format!("{yaml}group_reply_max: 12\ninterjection_max: 4\n")).unwrap();
        assert_eq!((cfg.group_reply_max, cfg.interjection_max), (12, 4));
    }

    #[test]
    fn session_debounce_defaults_and_overrides() {
        let yaml = "max_turns: 20\nidle_minutes: 30\nrecap_messages: 10\n";
        let session: SessionCfg = serde_yaml::from_str(yaml).unwrap();
        let d = session.debounce;
        assert_eq!((d.quiet_ms, d.cap_ms, d.adaptive, d.min_ms, d.max_ms), (2500, 10000, true, 1200, 6000));
        assert_eq!((d.alpha, d.factor), (0.3, 2.5));
        d.validate().unwrap();
        let session: SessionCfg = serde_yaml::from_str(&format!("{yaml}debounce:\n  adaptive: false\n")).unwrap();
        assert!(!session.debounce.adaptive);
        assert_eq!(session.debounce.quiet_ms, 2500);
        let d: DebounceCfg = serde_yaml::from_str("quiet_ms: 0\ncap_ms: 4000\nadaptive: false\nalpha: 0.5\nfactor: 3\nmin_ms: 500\nmax_ms: 3000").unwrap();
        assert_eq!((d.quiet_ms, d.cap_ms, d.adaptive, d.min_ms, d.max_ms), (0, 4000, false, 500, 3000));
        assert_eq!((d.alpha, d.factor), (0.5, 3.0));
        d.validate().unwrap();
    }

    #[test]
    fn debounce_rejects_invalid_adaptive_parameters() {
        for yaml in ["alpha: 0", "alpha: 1.1", "alpha: .nan", "factor: -1", "factor: .inf",
            "min_ms: 0", "min_ms: 7000", "max_ms: 10001", "cap_ms: 0"] {
            let cfg: DebounceCfg = serde_yaml::from_str(yaml).unwrap();
            assert!(cfg.validate().is_err(), "{yaml}");
        }
    }

    #[test]
    fn hands_slots_default_and_positive_validation() {
        let yaml = "code: off\ncodex_model: test\ntimeout_min: 1\n";
        assert_eq!(serde_yaml::from_str::<HandsCfg>(yaml).unwrap().max_jobs, 1);
        assert_eq!(serde_yaml::from_str::<HandsCfg>(&format!("{yaml}max_jobs: 3")).unwrap().max_jobs, 3);
        for invalid in ["0", "-1", "1.5"] {
            assert!(serde_yaml::from_str::<HandsCfg>(&format!("{yaml}max_jobs: {invalid}")).is_err());
        }
    }

    #[test]
    fn hands_defaults_preserve_codex_and_dsh_is_opt_in() {
        let yaml = "code: codex\ncodex_model: pinned\ntimeout_min: 10\n";
        let hands: HandsCfg = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(hands.research, DelegateTool::Codex);
        assert_eq!(hands.dsh_profile, "agli");
        assert_eq!(hands.dsh_home, Path::new("data/dsh-home"));
        assert_eq!((hands.stall_minutes, hands.total_budget_minutes), (20, 90));
        let hands: HandsCfg = serde_yaml::from_str(&format!("{yaml}research: dsh\ndsh_profile: agli-research")).unwrap();
        assert_eq!(hands.research, DelegateTool::Dsh);
        assert_eq!(hands.code, DelegateTool::Codex);
        assert_eq!(hands.dsh_profile, "agli-research");
        let hands: HandsCfg = serde_yaml::from_str(&format!("{yaml}stall_minutes: 3\ntotal_budget_minutes: 15")).unwrap();
        assert_eq!((hands.stall_minutes, hands.total_budget_minutes), (3, 15));
    }

    #[test]
    fn dsh_home_resolves_against_root_not_task_directory() {
        let yaml = "code: codex\ncodex_model: pinned\ntimeout_min: 10\n";
        let root = Path::new("agli root");
        for configured in [None, Some("data/custom home")] {
            let setting = configured.map(|p| format!("dsh_home: {p}")).unwrap_or_default();
            let mut hands: HandsCfg = serde_yaml::from_str(&format!("{yaml}{setting}")).unwrap();
            hands.resolve_dsh_home(root).unwrap();
            assert_eq!(hands.dsh_home, std::path::absolute(root.join(configured.unwrap_or("data/dsh-home"))).unwrap());
            assert!(hands.dsh_home.is_absolute());
            let resolved = hands.dsh_home.clone();
            hands.resolve_dsh_home(Path::new("another root")).unwrap();
            assert_eq!(hands.dsh_home, resolved);
        }
    }

    #[test]
    fn mongo_defaults_and_env_overrides() {
        let mut mongo: MongoCfg = serde_yaml::from_str("{}").unwrap();
        assert_eq!(mongo.url_env, "MONGO_URL");
        assert_eq!(mongo.database_env, "MONGO_DATABASE");
        assert_eq!(mongo.database, "aglibot");
        mongo.resolve_env(|_| None);
        assert!(mongo.url.is_none());
        assert_eq!(mongo.database, "aglibot");
        mongo.resolve_env(|key| match key {
            "MONGO_URL" => Some("mongodb://user:secret@localhost".into()),
            "MONGO_DATABASE" => Some("test_db".into()),
            _ => None,
        });
        assert_eq!(mongo.database, "test_db");
        assert!(mongo.url.is_some());
        assert!(!format!("{mongo:?}").contains("secret"));
        assert!(!format!("{mongo:?}").contains("mongodb://"));

        let mut custom: MongoCfg = serde_yaml::from_str("url_env: CUSTOM_URL\ndatabase_env: CUSTOM_DB").unwrap();
        custom.resolve_env(|key| (key == "CUSTOM_URL").then(|| "mongodb://localhost".into()));
        assert_eq!(custom.url.as_deref(), Some("mongodb://localhost"));
        assert_eq!(custom.database, "aglibot");
    }

    #[test]
    fn repo_config_loads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let cfg = Config::load(root).expect("config/ should parse");
        assert!(!cfg.models.models.is_empty());
        assert_eq!(cfg.runtime.hands.dsh_home, root.join("data/dsh-home"));
        assert!(cfg.scenarios.contains_key("verify"));
        assert_ne!(cfg.runtime.routine.sleep_at, cfg.runtime.routine.wake_at);
    }
}
