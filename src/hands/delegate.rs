//! His hands: delegate work to Codex or DSH as a subprocess, rooted at the task
//! directory. Only owner-approved tasks ever reach here. Claude Code is the owner's tool and is
//! never invoked from here (see the root AGENTS.md).
//!
//! The prompt goes through stdin, so no shell quoting is involved; on Windows the npm shims are
//! `.cmd` files, which `Command` runs through cmd.exe.

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::process_tree::ProcessTree;
use crate::ns::secrets::{SecretRegistry, process_registry};
use crate::store::{ModelCallRecord, Store};

fn program(name: &str) -> String {
    if cfg!(windows) { format!("{name}.cmd") } else { name.to_string() }
}

// Runtime discovery, npm shims, CLI homes/login, and network proxies only. New parent
// variables are private by default; do not add credential or code-injection variables.
const DELEGATE_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "SystemRoot",
    "windir",
    "COMSPEC",
    "PATHEXT",
    "TEMP",
    "TMP",
    "TMPDIR",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "HOME",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CODEX_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

#[derive(Clone, Copy)]
enum Delegate<'a> {
    Codex,
    Dsh(&'a Path),
}

fn delegate_environment(
    cmd: &mut Command,
    delegate: Delegate<'_>,
    registry: &SecretRegistry,
    parent: impl IntoIterator<Item = (OsString, OsString)>,
) {
    cmd.env_clear();
    let (executor, dsh_home) = match delegate {
        Delegate::Codex => ("codex", None),
        Delegate::Dsh(home) => ("dsh", Some(home)),
    };
    let environment = registry.assemble(executor, DELEGATE_ENV_ALLOWLIST, parent, dsh_home);
    cmd.envs(environment.variables);
    for source in environment.missing {
        tracing::warn!(
            "{executor} dedicated key {source} is missing or empty; configure it in AGLI's environment if required by the profile"
        );
    }
}

async fn run(
    mut cmd: Command,
    delegate: Delegate<'_>,
    stdin: &str,
    timeout: Duration,
    mut on_line: impl FnMut(&[u8]) -> Result<()>,
) -> Result<String> {
    delegate_environment(&mut cmd, delegate, &process_registry(), std::env::vars_os());
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut tree = ProcessTree::prepare(&mut cmd)?;
    let mut child = cmd.spawn().context("starting delegate process")?;
    tree.attach(&child)?;
    let mut input = child.stdin.take().context("delegate stdin")?;
    let mut output = BufReader::new(child.stdout.take().context("delegate stdout")?);
    let mut errors = child.stderr.take().context("delegate stderr")?;
    // Input, both output pipes, and wait share one deadline. No detached reader tasks survive
    // cancellation; the tree guard closes even when /stop or /freeze aborts this future.
    let work = async {
        let write = async {
            input.write_all(stdin.as_bytes()).await?;
            input.shutdown().await?;
            drop(input);
            Ok::<_, anyhow::Error>(())
        };
        let read = async {
            let mut stdout = Vec::new();
            let mut line = Vec::new();
            while output.read_until(b'\n', &mut line).await? != 0 {
                on_line(&line)?;
                stdout.extend_from_slice(&line);
                line.clear();
            }
            Ok::<_, anyhow::Error>(stdout)
        };
        let stderr = async {
            let mut bytes = Vec::new();
            errors.read_to_end(&mut bytes).await?;
            Ok::<_, anyhow::Error>(bytes)
        };
        let wait = async { Ok::<_, anyhow::Error>(child.wait().await?) };
        let (_, stdout, stderr, status) = tokio::try_join!(write, read, stderr, wait)?;
        Ok::<_, anyhow::Error>((stdout, stderr, status))
    };
    let (stdout, stderr, status) = tokio::time::timeout(timeout, work).await.context("delegate timed out")??;
    let stdout = String::from_utf8_lossy(&stdout).to_string();
    if !status.success() {
        // CLIs do not agree on which stream carries the error, so show both.
        let err = String::from_utf8_lossy(&stderr);
        let detail: String = format!("{} {}", err.trim(), stdout.trim()).chars().take(800).collect();
        bail!("delegate exited with {status}: {detail}");
    }
    Ok(stdout)
}

/// Codex in a workspace-write sandbox rooted at `dir`, pinned to `model` rather than whatever
/// default the owner's ~/.codex/config.toml names; `search` turns on its live web search.
/// Returns Codex's last message.
pub async fn codex(dir: &Path, prompt: &str, model: &str, search: bool, timeout: Duration) -> Result<String> {
    Ok(codex_session(dir, prompt, model, search, timeout, None).await?.text)
}

pub struct CodexOutput {
    pub text: String,
    pub session_id: String,
}

fn session_id(events: &str) -> Result<String> {
    events
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|event| {
            if event["type"] != "thread.started" {
                return None;
            }
            let id = event["thread_id"].as_str()?;
            let valid = id.len() == 36
                && id.bytes().enumerate().all(|(i, b)| if matches!(i, 8 | 13 | 18 | 23) { b == b'-' } else { b.is_ascii_hexdigit() });
            valid.then(|| id.to_string())
        })
        .context("Codex did not emit a thread.started event with a session UUID")
}

fn codex_command(dir: &Path, last: &Path, model: &str, search: bool, resume: Option<&str>) -> Command {
    let mut cmd = Command::new(program("codex"));
    if search {
        cmd.arg("--search");
    }
    // Sandbox and cwd belong to exec, not its resume subcommand.
    cmd.current_dir(dir).args(["exec", "--sandbox", "workspace-write", "-C"]).arg(dir);
    if let Some(id) = resume {
        cmd.args(["resume", id]);
    }
    cmd.args(["-m", model, "--skip-git-repo-check", "--json", "-o"]).arg(last).arg("-");
    cmd
}

/// Resume only an explicitly captured session, never the owner's most recent CLI session.
pub async fn codex_session(
    dir: &Path,
    prompt: &str,
    model: &str,
    search: bool,
    timeout: Duration,
    resume: Option<&str>,
) -> Result<CodexOutput> {
    let dir = std::path::absolute(dir)?;
    let last = dir.join("codex-last-message.md");
    match std::fs::remove_file(&last) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let events = run(codex_command(&dir, &last, model, search, resume), Delegate::Codex, prompt, timeout, |_| Ok(())).await?;
    let session_id = session_id(&events)?;
    if let Some(expected) = resume {
        anyhow::ensure!(session_id == expected, "Codex resumed a different session: {session_id}");
    }
    let text = std::fs::read_to_string(&last).context("reading Codex's last message")?;
    Ok(CodexOutput { text, session_id })
}

#[derive(Debug)]
pub struct DshOutput {
    pub text: String,
    pub session_id: Option<String>,
}

#[derive(Default, Debug, PartialEq)]
struct DshUsage {
    input: u64,
    cached: u64,
    cache_write: u64,
    output: u64,
}

#[derive(Debug, serde::Serialize)]
struct DshToolCall {
    call_id: Option<String>,
    tool: Option<String>,
    input: serde_json::Value,
    targets: Vec<String>,
    truncated: bool,
}

#[derive(Default)]
struct DshEvents {
    model_id: Option<String>,
    text: Option<String>,
    session_id: Option<String>,
    usage: Option<DshUsage>,
    missing_usage_steps: usize,
    calls: Vec<DshToolCall>,
    error: Option<String>,
}

fn targets(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(key.as_str(), "url" | "urls" | "path" | "paths" | "file_path" | "filePath" | "cwd") {
                    match value {
                        serde_json::Value::String(s) => out.push(s.clone()),
                        serde_json::Value::Array(items) => out.extend(items.iter().filter_map(|v| v.as_str().map(String::from))),
                        _ => {}
                    }
                }
                targets(value, out);
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| targets(v, out)),
        _ => {}
    }
}

impl DshEvents {
    // CLI 0.2.1-rc.5: C:/dev/DSH/resources/app.asar/dsh/node_modules/@deepseek-ai/
    // dsh-headless/lib/json-stream-BA-F3lfb.js, projectJsonRun/addUsage; errors in
    // dsh-headless/lib/index.js, fail(). step_end usage already includes retried attempts.
    // Tool inputs can be text or absent when DSH truncates a bounded event; final is lossless.
    fn line(&mut self, line: &[u8]) {
        let Ok(event) = serde_json::from_slice::<serde_json::Value>(line) else {
            return;
        };
        // Future DSH versions may expose either a model string or a model object.
        let model = &event["model"];
        let name = model.as_str().or_else(|| model["model"].as_str()).or_else(|| model["id"].as_str());
        if let Some(name) = name.map(str::trim).filter(|s| !s.is_empty()) {
            let provider = model["provider"].as_str().or_else(|| event["provider"].as_str())
                .map(str::trim).filter(|s| !s.is_empty());
            self.model_id = Some(match provider {
                Some(provider) if !name.contains('/') => format!("dsh:{provider}/{name}"),
                _ => format!("dsh:{}", name.strip_prefix("dsh:").unwrap_or(name)),
            });
        }
        match event["type"].as_str() {
            Some("session") => {
                if let Some(id) = event["sessionId"].as_str().filter(|s| valid_dsh_id(s)) {
                    self.session_id = Some(id.into());
                }
            }
            Some("final") => {
                if let Some(text) = event["text"].as_str() {
                    self.text = Some(text.into());
                }
            }
            Some("error") => self.error = Some(event["message"].as_str().unwrap_or("DSH reported an error").into()),
            Some("status") if event["phase"] == "step_end" => {
                let u = &event["usage"];
                if let (Some(input), Some(output)) = (u["inputTokens"].as_u64(), u["outputTokens"].as_u64()) {
                    let usage = self.usage.get_or_insert_with(DshUsage::default);
                    usage.input = usage.input.saturating_add(input);
                    usage.output = usage.output.saturating_add(output);
                    usage.cached = usage.cached.saturating_add(u["cacheReadTokens"].as_u64().unwrap_or(0).min(input));
                    usage.cache_write = usage.cache_write.saturating_add(u["cacheWriteTokens"].as_u64().unwrap_or(0));
                } else {
                    self.missing_usage_steps += 1;
                }
            }
            Some("tool_call") => {
                let input = event["input"].as_str().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_else(|| event["input"].clone());
                let mut found = Vec::new();
                targets(&input, &mut found);
                self.calls.push(DshToolCall {
                    call_id: event["callId"].as_str().map(String::from),
                    tool: event["tool"].as_str().map(String::from),
                    input,
                    targets: found,
                    truncated: event["truncated"] == true,
                });
            }
            _ => {}
        }
    }
}

/// Audit files append invocations, including resumes of the same session. A resumed
/// session still holds its earlier fetches in its transcript, so they keep vouching for the
/// answer; a different (fresh) session, or a run that never reported one, starts over.
pub(crate) fn dsh_research_targets(raw: &str) -> (Vec<String>, Vec<String>) {
    #[derive(Default, Clone)]
    struct Run {
        session: Option<String>,
        fetched: Vec<String>,
        searched: Vec<String>,
    }
    let mut events = DshEvents::default();
    let mut previous = Run::default();
    let mut run = Run::default();
    for line in raw.lines() {
        let event = serde_json::from_str::<serde_json::Value>(line).unwrap_or_default();
        if line.trim().is_empty() || event["type"] == "agli_run_start" {
            if run.session.is_some() {
                previous = std::mem::take(&mut run);
            }
            run = Run::default();
            events = DshEvents::default();
            continue;
        }
        if event["type"] == "session" {
            let id = event["sessionId"].as_str().map(String::from);
            run = if id.is_some() && id == previous.session { Run { session: id, ..previous.clone() } } else { Run { session: id, ..Run::default() } };
            continue;
        }
        let count = events.calls.len();
        events.line(line.as_bytes());
        if let Some(call) = events.calls.get(count) {
            match call.tool.as_deref().unwrap_or_default().to_ascii_lowercase().as_str() {
                "web_fetch" | "webfetch" => run.fetched.extend(call.targets.iter().cloned()),
                "web_search" | "websearch" => run.searched.extend(call.targets.iter().cloned()),
                _ => {}
            }
        }
        if event["type"] == "tool_result"
            && event["callId"].as_str().is_some_and(|id| {
                events.calls.iter().any(|call| {
                    call.call_id.as_deref() == Some(id)
                        && matches!(call.tool.as_deref().unwrap_or_default().to_ascii_lowercase().as_str(), "web_search" | "websearch")
                })
            })
        {
            search_result_urls(&event["result"], &mut run.searched);
        }
    }
    (run.fetched, run.searched)
}

fn search_result_urls(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => {
            for word in text.split(|c: char| c.is_whitespace() || "\"<>`([".contains(c)) {
                if word.to_ascii_lowercase().starts_with("https://") || word.to_ascii_lowercase().starts_with("http://") {
                    out.push(word.trim_end_matches([')', ']', ',', '.', ';']).to_string());
                }
            }
        }
        serde_json::Value::Object(fields) => fields.values().for_each(|value| search_result_urls(value, out)),
        serde_json::Value::Array(items) => items.iter().for_each(|value| search_result_urls(value, out)),
        _ => {}
    }
}

fn valid_dsh_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.starts_with('-') && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn dsh_command(executable: &std::ffi::OsStr, dir: &Path, profile: &str, resume: Option<&str>) -> Result<Command> {
    anyhow::ensure!(valid_dsh_id(profile), "invalid DSH profile name");
    let mut cmd = Command::new(executable);
    cmd.current_dir(dir).args(["--profile", profile, "--json"]);
    if let Some(id) = resume {
        anyhow::ensure!(valid_dsh_id(id), "invalid DSH session id");
        cmd.args(["--session-id", id]);
    }
    cmd.arg("-");
    Ok(cmd)
}

/// Resolve per invocation: profile edits take effect without restarting AGLI.
fn dsh_profile_model(home: &Path, profile: &str) -> String {
    let read = || -> Result<String> {
        let bytes = std::fs::read(home.join("profiles").join(profile).join("cordis.patch.yml"))?;
        let patch: serde_yaml::Value = serde_yaml::from_slice(&bytes)?;
        let entry = patch.as_sequence().context("profile patch must be a sequence")?
            .iter().find(|entry| entry["id"].as_str() == Some("agent-default-model"))
            .context("agent-default-model missing")?;
        let field = |key: &str| entry["config"][key].as_str().map(str::trim)
            .filter(|s| !s.is_empty()).with_context(|| format!("agent-default-model {key} missing"));
        Ok(format!("dsh:{}/{}", field("provider")?, field("model")?))
    };
    read().unwrap_or_else(|error| {
        tracing::debug!("DSH profile {profile} model unavailable: {error:#}");
        format!("dsh:profile:{profile}")
    })
}

struct DshAudit<'a> {
    events: DshEvents,
    raw: std::fs::File,
    calls: std::fs::File,
    log: std::fs::File,
    store: &'a dyn Store,
    model_id: String,
    kind: &'a str,
    started: std::time::Instant,
    error: Option<String>,
}

impl<'a> DshAudit<'a> {
    fn open(dir: &Path, store: &'a dyn Store, profile: &str, home: &Path, kind: &'a str) -> Result<Self> {
        let append = |name| std::fs::OpenOptions::new().create(true).append(true).open(dir.join(name));
        let mut raw = append("dsh-events.jsonl")?;
        // A cancelled invocation may have ended on a partial line. Keep the next run separate.
        writeln!(raw)?;
        writeln!(raw, "{{\"type\":\"agli_run_start\"}}")?;
        Ok(Self {
            events: DshEvents::default(),
            raw,
            calls: append("dsh-tool-calls.jsonl")?,
            log: append("log.md")?,
            store,
            model_id: dsh_profile_model(home, profile),
            kind,
            started: std::time::Instant::now(),
            error: Some("delegate cancelled".into()),
        })
    }

    fn line(&mut self, line: &[u8]) -> Result<()> {
        self.raw.write_all(line)?;
        let count = self.events.calls.len();
        self.events.line(line);
        if let Some(call) = self.events.calls.get(count) {
            serde_json::to_writer(&mut self.calls, call)?;
            writeln!(self.calls)?;
        }
        Ok(())
    }

    fn record(&self) -> Option<ModelCallRecord> {
        let u = self.events.usage.as_ref()?;
        Some(ModelCallRecord {
            ts: chrono::Utc::now(),
            scenario: format!("dsh:{}", self.kind),
            model_id: self.events.model_id.clone().unwrap_or_else(|| self.model_id.clone()),
            session_id: self.events.session_id.clone().unwrap_or_default(),
            uncached_input: u.input.saturating_sub(u.cached),
            cached_input: u.cached,
            cache_write: u.cache_write,
            output: u.output,
            latency_ms: self.started.elapsed().as_millis() as u64,
            cost_usd: 0.0,
            ok: self.error.is_none(),
            error: self.error.clone(),
            escalated_from: None,
        })
    }
}

impl Drop for DshAudit<'_> {
    fn drop(&mut self) {
        // Store is synchronous, so cancellation also records any usage received before /stop.
        let usage = match self.record() {
            Some(record) => {
                if let Err(e) = self.store.append_model_call(&record) {
                    tracing::warn!("DSH usage log failed: {e:#}");
                    let _ = writeln!(self.log, "- DSH usage storage failed: {e:#}");
                }
                format!(
                    "input={}, output={}, cost unknown (recorded as 0)",
                    record.uncached_input.saturating_add(record.cached_input),
                    record.output
                )
            }
            None => "usage not reported".into(),
        };
        let status = if self.error.is_none() { "completed" } else { "failed/cancelled" };
        if let Err(e) = writeln!(
            self.log,
            "- {} DSH {status}; session={}; tool calls={}; {usage}; steps without usage={}; audit=dsh-events.jsonl",
            chrono::Utc::now().to_rfc3339(),
            self.events.session_id.as_deref().unwrap_or("not reported"),
            self.events.calls.len(),
            self.events.missing_usage_steps
        ) {
            tracing::warn!("DSH task log failed: {e:#}");
        }
    }
}

/// A profile is owner-provisioned; no profile creation, credential changes, or budget gate here.
pub async fn dsh(
    dir: &Path,
    prompt: &str,
    hands: &crate::config::HandsCfg,
    timeout: Duration,
    resume: Option<&str>,
    store: &dyn Store,
    kind: &str,
) -> Result<DshOutput> {
    let executable = std::env::var_os("AGLI_DSH_BIN").filter(|s| !s.is_empty()).unwrap_or_else(|| program("dsh").into());
    let dir = std::path::absolute(dir)?;
    let cmd = dsh_command(&executable, &dir, &hands.dsh_profile, resume)?;
    dsh_run(cmd, &dir, prompt, &hands.dsh_profile, &hands.dsh_home, timeout, resume, store, kind).await
}

fn prepare_dsh_home(home: &Path) -> Result<()> {
    anyhow::ensure!(home.is_absolute(), "AGLI DSH home must be resolved against the AGLI root");
    std::fs::create_dir_all(home).with_context(|| format!("creating AGLI DSH home {}", home.display()))
}

#[allow(clippy::too_many_arguments)]
async fn dsh_run(
    cmd: Command,
    dir: &Path,
    prompt: &str,
    profile: &str,
    home: &Path,
    timeout: Duration,
    resume: Option<&str>,
    store: &dyn Store,
    kind: &str,
) -> Result<DshOutput> {
    prepare_dsh_home(home)?;
    let mut audit = DshAudit::open(dir, store, profile, home, kind)?;
    let result = async {
        run(cmd, Delegate::Dsh(home), prompt, timeout, |line| audit.line(line)).await?;
        if let Some(error) = &audit.events.error {
            bail!("DSH: {error}");
        }
        if let (Some(expected), Some(actual)) = (resume, audit.events.session_id.as_deref()) {
            anyhow::ensure!(actual == expected, "DSH resumed a different session: {actual}");
        }
        let text = audit.events.text.as_ref().filter(|t| !t.trim().is_empty()).context("DSH did not emit a nonempty final answer")?.clone();
        Ok(DshOutput { text, session_id: audit.events.session_id.clone().or_else(|| resume.map(String::from)) })
    }
    .await;
    audit.error = result.as_ref().err().map(|e| format!("{e:#}"));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ns::secrets::env_name_matches;
    use std::ffi::OsStr;

    const DSH_EVENTS: &str = include_str!("fixtures/dsh-events.jsonl");
    const DSH_ENV_KEYS: &[(&str, &str)] =
        &[("AGLI_DSH_DEEPSEEK_API_KEY", "DEEPSEEK_API_KEY"), ("AGLI_DSH_BAILIAN_API_KEY", "DSH_BAILIAN_API_KEY")];

    #[test]
    fn dsh_profile_model_is_read_per_run_with_safe_fallbacks() {
        let home = fixture_dir("profile-model");
        assert_eq!(dsh_profile_model(&home, "agli"), "dsh:profile:agli");
        let profile = home.join("profiles/agli");
        std::fs::create_dir_all(&profile).unwrap();
        let path = profile.join("cordis.patch.yml");
        for invalid in ["[", "- id: unrelated", "- id: agent-default-model\n  config: {provider: bailian}"] {
            std::fs::write(&path, invalid).unwrap();
            assert_eq!(dsh_profile_model(&home, "agli"), "dsh:profile:agli");
        }
        std::fs::write(&path, "- id: agent-default-model\n  config: {provider: bailian, model: deepseek-v4.1-flash}").unwrap();
        assert_eq!(dsh_profile_model(&home, "agli"), "dsh:bailian/deepseek-v4.1-flash");
        std::fs::write(&path, "- id: agent-default-model\n  config: {provider: other, model: edited}").unwrap();
        assert_eq!(dsh_profile_model(&home, "agli"), "dsh:other/edited");
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn dsh_model_event_overrides_profile_in_record() {
        let dir = fixture_dir("model-event");
        let profile = dir.join("profiles/agli");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("cordis.patch.yml"),
            "- id: agent-default-model\n  config: {provider: bailian, model: deepseek-v4.1-flash}").unwrap();
        let store = crate::store::jsonl::JsonlStore::open(&dir.join("store")).unwrap();
        {
            let mut audit = DshAudit::open(&dir, &store, "agli", &dir, "test").unwrap();
            audit.line(b"{\"type\":\"status\",\"phase\":\"step_end\",\"usage\":{\"inputTokens\":5,\"outputTokens\":2}}\n").unwrap();
            assert_eq!(audit.record().unwrap().model_id, "dsh:bailian/deepseek-v4.1-flash");
            audit.line(include_bytes!("fixtures/dsh-model-event.jsonl")).unwrap();
            assert_eq!(audit.record().unwrap().model_id, "dsh:future/event-model");
            audit.line(b"{\"type\":\"status\",\"model\":{\"provider\":\"other\",\"id\":\"object-model\"}}\n").unwrap();
            assert_eq!(audit.record().unwrap().model_id, "dsh:other/object-model");
            audit.line(b"{\"type\":\"text\",\"model\":42}\n").unwrap();
            assert_eq!(audit.record().unwrap().model_id, "dsh:other/object-model");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn environment_names_follow_platform_case_rules() {
        assert!(env_name_matches(OsStr::new("PATH"), "PATH"));
        assert_eq!(env_name_matches(OsStr::new("pAtH"), "PATH"), cfg!(windows));
        assert_eq!(env_name_matches(OsStr::new("agli_dsh_bailian_api_key"), "AGLI_DSH_BAILIAN_API_KEY"), cfg!(windows));
        assert!(!env_name_matches(OsStr::new("PATH_EXTRA"), "PATH"));
    }

    #[tokio::test]
    async fn delegate_child_environment_is_allowlisted_and_dsh_keys_are_private() -> Result<()> {
        // Inject a parent map, never mutate the test process environment. Only Windows' OS
        // directory comes from the host; all credentials and other values are synthetic.
        let mut parent: Vec<(OsString, OsString)> = DELEGATE_ENV_ALLOWLIST
            .iter()
            .map(|name| {
                let value = if cfg!(windows) && matches!(*name, "SystemRoot" | "windir") {
                    std::env::var_os("SystemRoot").unwrap()
                } else {
                    "allowlisted-value".into()
                };
                let name = if cfg!(windows) { name.to_ascii_lowercase() } else { name.to_string() };
                (name.into(), value)
            })
            .collect();
        let blocked = [
            "TELEGRAM_BOT_TOKEN",
            "BAILIAN_API_KEY",
            "ONEBOT_TOKEN",
            "DEEPSEEK_API_KEY",
            "DSH_BAILIAN_API_KEY",
            "FUTURE_API_KEY",
            "NODE_OPTIONS",
        ];
        parent.extend(blocked.iter().map(|name| ((*name).into(), "parent-secret".into())));
        let home = std::path::absolute("data/dsh-home")?;
        for (delegate, key_value) in [
            (Delegate::Codex, Some("dedicated-secret")),
            (Delegate::Codex, None),
            (Delegate::Dsh(&home), Some("dedicated-secret")),
            (Delegate::Dsh(&home), None),
            (Delegate::Dsh(&home), Some("")),
        ] {
            let mut source = parent.clone();
            // Exercise both inherited and absent homes without changing process env.
            if key_value.is_some() {
                source.push((if cfg!(windows) { "dSh_HoMe" } else { "DSH_HOME" }.into(), "owner-home".into()));
            }
            if let Some(value) = key_value {
                source.extend(DSH_ENV_KEYS.iter().map(|(name, target)| {
                    let name = if cfg!(windows) { name.to_ascii_lowercase() } else { name.to_string() };
                    let value = if value.is_empty() { String::new() } else { format!("{value}-{target}") };
                    (name.into(), value.into())
                }));
            }
            let mut cmd = fixture_command(&std::env::current_dir()?, "environment");
            cmd.env("COMMAND_OVERRIDE_SECRET", "must-be-cleared");
            delegate_environment(&mut cmd, delegate, &SecretRegistry::default(), source);
            let output = cmd.output().await?;
            assert!(output.status.success(), "{}: {}", output.status, String::from_utf8_lossy(&output.stderr));
            let stdout = String::from_utf8(output.stdout)?;
            let actual: Vec<_> = stdout.lines().filter_map(|line| line.split_once('=')).collect();
            let value = |name: &str| actual.iter().find(|(key, _)| env_name_matches(OsStr::new(key), name)).map(|(_, value)| *value);
            assert_eq!(value("DSH_HOME"), if matches!(delegate, Delegate::Dsh(_)) { home.to_str() } else { None });
            for (name, expected) in parent.iter().take(DELEGATE_ENV_ALLOWLIST.len()) {
                assert_eq!(value(name.to_str().unwrap()), expected.to_str(), "allowlisted variable {name:?}");
            }
            for name in blocked.iter().chain(["COMMAND_OVERRIDE_SECRET"].iter()) {
                if !matches!(*name, "DEEPSEEK_API_KEY" | "DSH_BAILIAN_API_KEY") {
                    assert!(value(name).is_none(), "unexpected variable {name}");
                }
            }
            for (source, target) in DSH_ENV_KEYS {
                assert!(value(source).is_none(), "prefixed variable {source} leaked");
                let expected = if matches!(delegate, Delegate::Dsh(_)) {
                    key_value.filter(|v| !v.is_empty()).map(|v| format!("{v}-{target}"))
                } else {
                    None
                };
                assert_eq!(value(target), expected.as_deref(), "DSH-only variable {target}");
            }
        }
        Ok(())
    }

    #[test]
    fn dsh_schema_extracts_final_session_step_usage_and_every_tool_call() {
        let mut events = DshEvents::default();
        for line in DSH_EVENTS.lines() {
            events.line(line.as_bytes());
        }
        assert_eq!(events.text.as_deref(), Some("Final answer.\nWith evidence."));
        assert_eq!(events.session_id.as_deref(), Some("session-research-25"));
        assert_eq!(events.usage, Some(DshUsage { input: 160, output: 30, cached: 30, cache_write: 5 }));
        assert_eq!(events.calls.len(), 5);
        assert!(events.calls[0].targets.is_empty()); // Search has a query, not a fetched URL.
        assert_eq!(events.calls[1].targets, ["https://example.org/source"]);
        assert_eq!(events.calls[2].targets, ["C:/tasks/research/plan.md"]);
        assert!(events.calls[3].truncated);
        assert_eq!(events.calls[3].input, "{broken");
        assert!(events.calls[4].input.is_null());
    }

    #[test]
    fn dsh_research_targets_only_include_current_fetches_and_search_results() {
        let (fetched, searched) = dsh_research_targets(include_str!("fixtures/dsh-source-check-events.jsonl"));
        assert_eq!(fetched, ["HTTPS://EXAMPLE.ORG:443/fetched/?a=1&b=2#section", "http://example.org:80/alias"]);
        assert_eq!(searched, ["https://example.org/search-only"]);
        let (fetched, searched) = dsh_research_targets(DSH_EVENTS);
        assert_eq!(fetched, ["https://example.org/source"]);
        assert_eq!(searched, ["https://example.org/source"]);
    }

    #[test]
    fn dsh_research_run_boundaries_clear_old_targets_even_without_a_session_id() {
        for boundary in ["\n", "{\"type\":\"agli_run_start\"}\n", "{\"type\":\"session\",\"sessionId\":\"session-research-25\"}\n"] {
            let raw = format!("{DSH_EVENTS}{boundary}{{\"type\":\"final\",\"text\":\"no tools this run\"}}\n");
            let (fetched, searched) = dsh_research_targets(&raw);
            assert!(fetched.is_empty() && searched.is_empty(), "{boundary:?}");
        }
        // A cancelled run can leave a partial line; the explicit marker still clears it.
        let raw = format!("{DSH_EVENTS}{{broken\n{{\"type\":\"agli_run_start\"}}\n");
        assert_eq!(dsh_research_targets(&raw), (vec![], vec![]));
    }

    #[test]
    fn dsh_research_targets_carry_over_only_within_the_same_resumed_session() {
        let resume = |id: &str| format!("{DSH_EVENTS}
{{\"type\":\"agli_run_start\"}}
{{\"type\":\"session\",\"sessionId\":\"{id}\"}}
{{\"type\":\"final\",\"text\":\"revised\"}}
");
        let (fetched, searched) = dsh_research_targets(&resume("session-research-25"));
        assert_eq!((fetched, searched), (vec!["https://example.org/source".to_string()], vec!["https://example.org/source".to_string()]));
        assert_eq!(dsh_research_targets(&resume("session-fresh")), (vec![], vec![]));
        // A run that never reported a session (failed early) does not break the chain.
        let raw = format!("{DSH_EVENTS}
{{\"type\":\"agli_run_start\"}}
{{broken
{}", resume("session-research-25").trim_start_matches(DSH_EVENTS));
        assert_eq!(dsh_research_targets(&raw).0, ["https://example.org/source"]);
    }

    #[test]
    fn dsh_missing_malformed_and_future_events_are_safe() {
        let mut events = DshEvents::default();
        for line in [
            "",
            "null",
            "[]",
            "{",
            r#"{"type":"session","sessionId":"--bad"}"#,
            r#"{"type":"final","text":17}"#,
            r#"{"type":"status","phase":"step_end"}"#,
            r#"{"type":"status","phase":"step_end","usage":{"inputTokens":-1,"outputTokens":"oops"}}"#,
        ] {
            events.line(line.as_bytes());
        }
        assert!(events.text.is_none() && events.session_id.is_none() && events.usage.is_none());
        assert_eq!(events.missing_usage_steps, 2);
        events.line(br#"{"type":"status","phase":"step_end","usage":{"inputTokens":0,"outputTokens":0}}"#);
        assert_eq!(events.usage, Some(DshUsage::default()));
        events.line(br#"{"type":"error","message":"unknown session"}"#);
        assert_eq!(events.error.as_deref(), Some("unknown session"));
        events.line(br#"{"type":"tool_call","input":"{\"paths\":[\"a\",\"b\"]}"}"#);
        assert_eq!(events.calls[0].targets, ["a", "b"]);
    }

    #[test]
    fn dsh_fresh_and_resume_commands_use_profile_stdin_and_json() {
        let dir = Path::new("task with spaces");
        for resume in [None, Some("session-research-25")] {
            let cmd = dsh_command("C:/DSH/dsh.cmd".as_ref(), dir, "agli", resume).unwrap();
            assert_eq!(cmd.as_std().get_current_dir(), Some(dir));
            assert_eq!(cmd.as_std().get_program(), "C:/DSH/dsh.cmd");
            let args: Vec<_> = cmd.as_std().get_args().map(|s| s.to_str().unwrap()).collect();
            let mut expected = vec!["--profile", "agli", "--json"];
            if let Some(id) = resume {
                expected.extend(["--session-id", id]);
            }
            expected.push("-");
            assert_eq!(args, expected);
        }
        assert!(dsh_command("dsh".as_ref(), dir, "agli", Some("--last")).is_err());
        assert!(dsh_command("dsh".as_ref(), dir, "a&b", None).is_err());
    }

    #[test]
    fn dsh_home_is_created_empty_and_existing_state_is_preserved() -> Result<()> {
        let dir = fixture_dir("home");
        let home = dir.join("data/dsh-home");
        assert!(!home.exists());
        prepare_dsh_home(&home)?;
        assert!(home.is_dir());
        assert_eq!(std::fs::read_dir(&home)?.count(), 0);
        std::fs::write(home.join("profile-marker"), "keep")?;
        prepare_dsh_home(&home)?;
        assert_eq!(std::fs::read_to_string(home.join("profile-marker"))?, "keep");
        assert!(!home.join(".credentials.yaml").exists());
        assert!(prepare_dsh_home(&home.join("profile-marker")).is_err());
        assert!(prepare_dsh_home(Path::new("relative-home")).is_err());
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    fn fixture_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("agli-dsh-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture_command(dir: &Path, role: &str) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        // libtest accepts no custom arguments. A nonmatching skip filter carries the
        // fixture role without depending on an environment variable that run() scrubs.
        cmd.args(["--exact", "hands::delegate::tests::delegate_child_fixture", "--nocapture"])
            .current_dir(dir)
            .args(["--skip", &format!("agli-fixture-role={role}")]);
        cmd
    }

    /// Runs only inside this test executable, never a real hand or a model.
    #[test]
    fn delegate_child_fixture() {
        let role = std::env::args().find_map(|arg| arg.strip_prefix("agli-fixture-role=").map(String::from));
        let Some(role) = role else {
            return;
        };
        if role == "environment" {
            for (name, value) in std::env::vars() {
                println!("{name}={value}");
            }
            return;
        }
        if matches!(role.as_str(), "events" | "failure" | "no-usage" | "no-final" | "error") {
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).unwrap();
            assert_eq!(input, "prompt through stdin");
            if role == "no-usage" {
                println!(r#"{{"type":"final","text":"No tokens"}}"#);
            } else if role == "no-final" {
                println!(r#"{{"type":"text","text":"Not a final answer"}}"#);
            } else if role == "error" {
                println!(r#"{{"type":"error","message":"Unknown session"}}"#);
            } else {
                print!("{DSH_EVENTS}");
            }
            if role == "failure" {
                std::process::exit(7);
            }
            return;
        }
        if role == "tree" || role == "branch" {
            let next = if role == "tree" { "branch" } else { "leaf" };
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child.args([
                "--exact",
                "hands::delegate::tests::delegate_child_fixture",
                "--nocapture",
                "--skip",
                &format!("agli-fixture-role={next}"),
            ]);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                child.creation_flags(0x08000000);
            }
            let mut child = child.spawn().unwrap();
            std::fs::write(format!("{role}.pid"), std::process::id().to_string()).unwrap();
            child.wait().unwrap();
        } else {
            println!(r#"{{"type":"status","phase":"step_end","usage":{{"inputTokens":3,"outputTokens":2}}}}"#);
            std::io::stdout().flush().unwrap();
            std::fs::write("leaf.pid", std::process::id().to_string()).unwrap();
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    #[tokio::test]
    async fn dsh_audits_success_failure_and_missing_usage_without_models() -> Result<()> {
        let dir = fixture_dir("audit");
        let store = crate::store::jsonl::JsonlStore::open(&dir.join("store"))?;
        for role in ["events", "failure", "no-usage"] {
            let result = dsh_run(
                fixture_command(&dir, role),
                &dir,
                "prompt through stdin",
                "agli",
                &dir.join("dsh-home"),
                Duration::from_secs(10),
                None,
                &store,
                "research",
            )
            .await;
            assert_eq!(result.is_ok(), role != "failure");
        }
        let calls = std::fs::read_to_string(dir.join("store/model_calls.jsonl"))?;
        let calls: Vec<ModelCallRecord> = calls.lines().map(serde_json::from_str).collect::<std::result::Result<_, _>>()?;
        assert_eq!(calls.len(), 2);
        assert!(calls[0].ok && !calls[1].ok);
        assert_eq!(calls[0].scenario, "dsh:research");
        assert_eq!((calls[0].uncached_input, calls[0].cached_input, calls[0].output), (130, 30, 30));
        assert_eq!(calls[0].cost_usd, 0.0);
        assert_eq!(std::fs::read_to_string(dir.join("dsh-tool-calls.jsonl"))?.lines().count(), 10);
        assert_eq!(std::fs::read_to_string(dir.join("dsh-events.jsonl"))?.matches(DSH_EVENTS).count(), 2);
        assert_eq!(std::fs::read_to_string(dir.join("dsh-events.jsonl"))?.matches("agli_run_start").count(), 3);
        assert_eq!(dsh_research_targets(&std::fs::read_to_string(dir.join("dsh-events.jsonl"))?), (vec![], vec![]));
        assert!(std::fs::read_to_string(dir.join("log.md"))?.contains("usage not reported"));
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn dsh_validates_resume_identity_and_terminal_answer() -> Result<()> {
        let dir = fixture_dir("resume");
        let store = crate::store::jsonl::JsonlStore::open(&dir.join("store"))?;
        for (role, session, ok) in [
            ("events", Some("session-research-25"), true),
            ("events", Some("different-session"), false),
            ("no-usage", None, true),
            ("no-final", None, false),
            ("error", Some("missing-session"), false),
        ] {
            let result = dsh_run(
                fixture_command(&dir, role),
                &dir,
                "prompt through stdin",
                "agli",
                &dir.join("dsh-home"),
                Duration::from_secs(10),
                session,
                &store,
                "research",
            )
            .await;
            assert_eq!(result.is_ok(), ok, "{role}: {result:?}");
            if role == "no-usage" {
                assert!(result?.session_id.is_none());
            }
        }
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[cfg(windows)]
    async fn check_process_tree_cleanup(cancel: bool) -> Result<()> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::{OpenProcess, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject};
        let dir = fixture_dir(if cancel { "cancel tree with spaces" } else { "timeout tree with spaces" });
        let store = std::sync::Arc::new(crate::store::jsonl::JsonlStore::open(&dir.join("store"))?);
        let script = dir.join("tree.cmd");
        std::fs::write(
            &script,
            format!(
                "@echo off\r\n\"{}\" --exact hands::delegate::tests::delegate_child_fixture --nocapture --skip agli-fixture-role=tree\r\n",
                std::env::current_exe()?.display()
            ),
        )?;
        let mut cmd = Command::new(&script);
        cmd.current_dir(&dir);
        let work_dir = dir.clone();
        let handle = tokio::spawn(async move {
            // A full pipe must not prevent the timeout from firing (the fixture never reads stdin).
            let prompt = if cancel { String::new() } else { "x".repeat(1024 * 1024) };
            dsh_run(cmd, &work_dir, &prompt, "agli", &work_dir.join("dsh-home"), Duration::from_secs(8), None, store.as_ref(), "research").await
        });
        let ready = tokio::time::timeout(Duration::from_secs(6), async {
            loop {
                if dir.join("leaf.pid").exists()
                    && std::fs::read_to_string(dir.join("dsh-events.jsonl")).unwrap_or_default().contains("inputTokens")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        if ready.is_err() {
            handle.abort();
            let _ = handle.await;
            bail!("test child tree did not become ready");
        }
        let mut processes = Vec::new();
        for role in ["tree", "branch", "leaf"] {
            let pid = std::fs::read_to_string(dir.join(format!("{role}.pid")))?.parse::<u32>()?;
            let raw = unsafe { OpenProcess(SYNCHRONIZATION_SYNCHRONIZE, 0, pid) };
            anyhow::ensure!(!raw.is_null(), "fixture process should be alive");
            processes.push(unsafe { OwnedHandle::from_raw_handle(raw) });
        }
        if cancel {
            // This is exactly the JoinHandle::abort path used by /stop and /freeze.
            handle.abort();
            assert!(handle.await.unwrap_err().is_cancelled());
        } else {
            assert!(handle.await?.err().unwrap().to_string().contains("timed out"));
        }
        for process in processes {
            assert_eq!(
                unsafe { WaitForSingleObject(process.as_raw_handle(), 5000) },
                WAIT_OBJECT_0,
                "descendant survived delegate cleanup"
            );
        }
        let calls = std::fs::read_to_string(dir.join("store/model_calls.jsonl"))?;
        assert_eq!(calls.lines().count(), 1);
        let call: ModelCallRecord = serde_json::from_str(calls.lines().next().unwrap())?;
        assert!(!call.ok);
        assert_eq!((call.uncached_input, call.output), (3, 2));
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cancellation_kills_cmd_child_and_grandchild_and_records_usage() -> Result<()> {
        check_process_tree_cleanup(true).await
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn timeout_kills_cmd_child_and_grandchild_and_records_usage() -> Result<()> {
        check_process_tree_cleanup(false).await
    }

    #[test]
    fn captures_session_uuid_from_jsonl_not_other_events() {
        let id = "0199a213-81c0-7800-8aa1-bbab2a035a53";
        let events = format!(
            "not json\n{{\"type\":\"item.completed\",\"thread_id\":\"wrong\"}}\n{{\"type\":\"thread.started\",\"thread_id\":\"{id}\"}}\n{{\"type\":\"turn.completed\"}}\n"
        );
        assert_eq!(session_id(&events).unwrap(), id);
        for invalid in ["", "{}", r#"{"type":"thread.started"}"#, r#"{"type":"thread.started","thread_id":"--last"}"#] {
            assert!(session_id(invalid).is_err());
        }
    }

    #[test]
    fn fresh_and_resume_commands_keep_the_same_constraints() {
        let dir = std::env::temp_dir().join("task with spaces");
        let last = dir.join("codex-last-message.md");
        let id = "0199a213-81c0-7800-8aa1-bbab2a035a53";
        for resume in [None, Some(id)] {
            let cmd = codex_command(&dir, &last, "pinned-model", true, resume);
            assert_eq!(cmd.as_std().get_current_dir(), Some(dir.as_path()));
            let args: Vec<_> = cmd.as_std().get_args().map(|s| s.to_string_lossy().into_owned()).collect();
            let mut expected =
                vec!["--search", "exec", "--sandbox", "workspace-write", "-C"].into_iter().map(String::from).collect::<Vec<_>>();
            expected.push(dir.to_string_lossy().into_owned());
            if resume.is_some() {
                expected.extend(["resume".into(), id.into()]);
            }
            expected.extend(["-m", "pinned-model", "--skip-git-repo-check", "--json", "-o"].into_iter().map(String::from));
            expected.extend([last.to_string_lossy().into_owned(), "-".into()]);
            assert_eq!(args, expected);
            assert!(!args.iter().any(|a| a == "--last"));
        }
        let cmd = codex_command(&dir, &last, "pinned-model", false, None);
        assert!(!cmd.as_std().get_args().any(|a| a == "--search"));
    }

    /// Uses the owner's Codex quota: opt-in only, with an explicit id across both calls.
    #[tokio::test]
    #[ignore = "runs the local codex CLI twice"]
    async fn live_codex_resume_round_trip() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("aglibot-test-codex-resume-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let first = codex_session(
            &dir,
            "Remember the token amber-otter-73 for the next turn. Reply READY. Do not write files.",
            "gpt-6-astra",
            true,
            Duration::from_secs(300),
            None,
        )
        .await?;
        let next = codex_session(
            &dir,
            "What token did I ask you to remember? Reply with just that token. Do not use tools.",
            "gpt-6-astra",
            true,
            Duration::from_secs(300),
            Some(&first.session_id),
        )
        .await?;
        assert_eq!(next.session_id, first.session_id);
        assert!(next.text.contains("amber-otter-73"), "{}", next.text);
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    /// Uses a little of the owner's Codex quota: opt-in only.
    #[tokio::test]
    #[ignore = "runs the local codex CLI"]
    async fn live_codex_round_trip() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("aglibot-test-codex-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let text = codex(
            &dir,
            "Create a file named ok.txt containing OK, then reply with exactly: DONE",
            "gpt-6-astra",
            true,
            Duration::from_secs(300),
        )
        .await?;
        assert!(dir.join("ok.txt").exists(), "{text:?}");
        println!("codex: {}", text.trim());
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    /// Verifies note n-agli-asm-dsh-apikeyenv end to end through AGLI's real env assembly.
    /// Spends a little Bailian and DeepSeek search quota: opt-in only, run alone
    /// (`--ignored --test-threads=1`, it edits the process environment). Needs the agli profile
    /// in data/dsh-home and both AGLI_DSH_* keys in .env (docs/DSH.md).
    #[tokio::test]
    #[ignore = "runs the local dsh CLI against live providers"]
    async fn live_dsh_profile_uses_dedicated_keys() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let _ = dotenvy::from_path(root.join(".env"));
        if std::env::var_os("AGLI_DSH_BIN").is_none() {
            // SAFETY: this test runs alone; nothing else reads the environment concurrently.
            unsafe { std::env::set_var("AGLI_DSH_BIN", "C:/dev/DSH/resources/runtime/cli/bin/dsh.cmd") };
        }
        let cfg = crate::config::Config::load(root)?;
        let base = std::env::temp_dir().join(format!("aglibot-test-dsh-{}", std::process::id()));
        let (ok_dir, nokey_dir) = (base.join("ok"), base.join("nokey"));
        std::fs::create_dir_all(&ok_dir)?;
        std::fs::create_dir_all(&nokey_dir)?;
        let store = crate::store::jsonl::JsonlStore::open(&base.join("data"))?;

        let prompt = "Use web_search once to find the official homepage of the Rust programming language, \
                      then web_fetch that page. Reply with only the text of its <title> element.";
        let out = dsh(&ok_dir, prompt, &cfg.runtime.hands, Duration::from_secs(300), None, &store, "live-test").await?;
        println!("dsh answer: {}", out.text.trim());
        let events = std::fs::read_to_string(ok_dir.join("dsh-events.jsonl"))?;
        assert!(events.contains("web_search"), "no web_search call recorded");
        let calls = std::fs::read_to_string(base.join("data").join("model_calls.jsonl")).unwrap_or_default();
        println!("model calls: {}", calls.trim());
        assert!(calls.contains("dsh:live-test"), "usage was not recorded");

        // Without the dedicated model key the run must fail, not fall back to another key or provider.
        // SAFETY: as above.
        unsafe { std::env::remove_var("AGLI_DSH_BAILIAN_API_KEY") };
        let missing = dsh(&nokey_dir, "Reply with exactly: OK", &cfg.runtime.hands, Duration::from_secs(120), None, &store, "live-test").await;
        println!("without key: {missing:?}");
        assert!(missing.is_err(), "dsh answered without DSH_BAILIAN_API_KEY: {:?}", missing.map(|o| o.text));
        std::fs::remove_dir_all(&base)?;
        Ok(())
    }
}
