//! Run Codex in an isolated task directory with a bounded process lifetime.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::process_tree::ProcessTree;
use crate::control::CancelToken;
use crate::ns::secrets::{SecretRegistry, process_registry};

pub const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

fn program(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.cmd")
    } else {
        name.to_string()
    }
}

// Runtime discovery, npm shims, CLI homes/login, and network proxies only. New parent
// variables are private by default; do not add credential or code-injection variables.
const CODEX_ENV_ALLOWLIST: &[&str] = &[
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

fn codex_environment(
    cmd: &mut Command,
    registry: &SecretRegistry,
    parent: impl IntoIterator<Item = (OsString, OsString)>,
) {
    cmd.env_clear();
    let environment = registry.assemble("codex", CODEX_ENV_ALLOWLIST, parent);
    cmd.envs(environment.variables);
    for source in environment.missing {
        tracing::warn!("Codex credential {source} is missing or empty");
    }
}

async fn drain(mut pipe: impl AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut kept = Vec::new();
    let mut buf = [0; 8192];
    loop {
        let read = pipe.read(&mut buf).await?;
        if read == 0 {
            return Ok(kept);
        }
        let keep = read.min(MAX_OUTPUT_BYTES - kept.len());
        kept.extend_from_slice(&buf[..keep]);
    }
}

async fn run(
    mut cmd: Command,
    stdin: &str,
    timeout: Duration,
    mut cancel: CancelToken,
) -> Result<String> {
    anyhow::ensure!(!cancel.is_cancelled(), "Codex 已取消");
    codex_environment(&mut cmd, &process_registry(), std::env::vars_os());
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut tree = ProcessTree::prepare(&mut cmd)?;
    let mut child = cmd.spawn().context("starting Codex")?;
    tree.attach(&child)?;
    let mut input = child.stdin.take().context("Codex stdin")?;
    let stdout = child.stdout.take().context("Codex stdout")?;
    let stderr = child.stderr.take().context("Codex stderr")?;
    let work = async {
        let write = async move {
            input.write_all(stdin.as_bytes()).await?;
            input.shutdown().await
        };
        tokio::try_join!(write, drain(stdout), drain(stderr), child.wait())
    };
    let result = tokio::select! {
        biased;
        () = cancel.cancelled() => Err(anyhow::anyhow!("Codex 已取消")),
        result = tokio::time::timeout(timeout, work) => match result {
            Ok(result) => result.map_err(Into::into),
            Err(error) => Err(anyhow::Error::new(error).context("Codex timed out")),
        },
    };
    drop(tree);
    drop(child);
    let (_, stdout, stderr, status) = result?;
    if !status.success() {
        let detail: String = String::from_utf8_lossy(&stderr)
            .trim()
            .chars()
            .chain(String::from_utf8_lossy(&stdout).trim().chars())
            .take(800)
            .collect();
        bail!("Codex exited with {status}: {detail}");
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

/// Codex in a workspace-write sandbox rooted at `dir`, pinned to `model` rather than whatever
/// default the owner's ~/.codex/config.toml names; `search` turns on its live web search.
/// Returns Codex's last message.
pub async fn codex(
    dir: &Path,
    prompt: &str,
    model: &str,
    search: bool,
    timeout: Duration,
    cancel: CancelToken,
) -> Result<String> {
    Ok(
        codex_session(dir, prompt, model, search, timeout, None, cancel)
            .await?
            .text,
    )
}

pub struct CodexOutput {
    pub text: String,
    pub session_id: String,
}

fn valid_session_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
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
            valid_session_id(id).then(|| id.to_string())
        })
        .context("Codex did not emit a thread.started event with a session UUID")
}

fn codex_command(
    dir: &Path,
    last: &Path,
    model: &str,
    search: bool,
    resume: Option<&str>,
) -> Command {
    let mut cmd = Command::new(program("codex"));
    if search {
        cmd.arg("--search");
    }
    // Sandbox and cwd belong to exec, not its resume subcommand.
    cmd.current_dir(dir)
        .args(["exec", "--sandbox", "workspace-write", "-C"])
        .arg(dir);
    if let Some(id) = resume {
        cmd.args(["resume", id]);
    }
    cmd.args(["-m", model, "--skip-git-repo-check", "--json", "-o"])
        .arg(last)
        .arg("-");
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
    cancel: CancelToken,
) -> Result<CodexOutput> {
    anyhow::ensure!(
        resume.is_none_or(valid_session_id),
        "invalid Codex session ID"
    );
    let dir = std::path::absolute(dir)?;
    let last = dir.join("codex-last-message.md");
    match std::fs::remove_file(&last) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let events = run(
        codex_command(&dir, &last, model, search, resume),
        prompt,
        timeout,
        cancel,
    )
    .await?;
    let session_id = session_id(&events)?;
    if let Some(expected) = resume {
        anyhow::ensure!(
            session_id == expected,
            "Codex resumed a different session: {session_id}"
        );
    }
    let meta = tokio::fs::symlink_metadata(&last)
        .await
        .context("inspecting Codex's last message")?;
    anyhow::ensure!(meta.is_file(), "Codex's last message is not a regular file");
    let mut bytes = Vec::new();
    tokio::fs::File::open(&last)
        .await
        .context("opening Codex's last message")?
        .take(MAX_OUTPUT_BYTES as u64)
        .read_to_end(&mut bytes)
        .await?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(CodexOutput { text, session_id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::CancelSource;

    #[test]
    fn environment_preserves_runtime_and_omits_private_variables() {
        let mut command = Command::new("unused");
        command.env("EXISTING_SECRET", "must disappear");
        codex_environment(
            &mut command,
            &SecretRegistry::default(),
            [
                ("PATH", "runtime"),
                ("CODEX_HOME", "codex"),
                ("TERM", "unlisted"),
                ("OPENAI_API_KEY", "internal"),
                ("DEEPSEEK_API_KEY", "internal"),
                ("MONGO_URL", "internal"),
                ("HTTPS_PROXY", "http://name:secret@localhost:7890"),
            ]
            .map(|(n, v)| (n.into(), v.into())),
        );
        let names: Vec<_> = command
            .as_std()
            .get_envs()
            .filter_map(|(name, value)| value.map(|_| name.to_string_lossy().to_string()))
            .collect();
        assert_eq!(names, ["CODEX_HOME", "PATH"]);
    }

    #[test]
    fn session_resume_is_explicit_and_workspace_scoped() {
        let id = "0199a213-81c0-7800-8aa1-bbab2a035a53";
        let events = format!("noise\n{{\"type\":\"thread.started\",\"thread_id\":\"{id}\"}}\n");
        assert_eq!(session_id(&events).unwrap(), id);
        assert!(session_id(r#"{"type":"thread.started","thread_id":"--last"}"#).is_err());
        let dir = Path::new("task with spaces");
        let command = codex_command(dir, &dir.join("result.md"), "pinned", true, Some(id));
        let args: Vec<_> = command.as_std().get_args().collect();
        assert_eq!(
            &args[..7],
            [
                "--search",
                "exec",
                "--sandbox",
                "workspace-write",
                "-C",
                "task with spaces",
                "resume"
            ]
        );
        assert_eq!(args[7], id);
        assert_eq!(&args[8..10], ["-m", "pinned"]);
    }

    #[tokio::test]
    async fn output_cap_keeps_draining_until_eof() {
        let (input, mut writer) = tokio::io::duplex(32 * 1024);
        let write = async move {
            for _ in 0..750 {
                writer.write_all(&[b'x'; 8192]).await.unwrap();
            }
            writer.shutdown().await.unwrap();
        };
        let (output, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(drain(input), write)
        })
        .await
        .unwrap();
        assert_eq!(output.unwrap().len(), MAX_OUTPUT_BYTES);
    }

    fn fixture_command(program: impl AsRef<std::ffi::OsStr>, role: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new(program);
        cmd.args([
            "--exact",
            "hands::codex::tests::process_fixture",
            "--nocapture",
            "--skip",
        ])
        .arg(format!("agli-fixture-role={role}"));
        cmd
    }

    #[test]
    fn process_fixture() {
        let role = std::env::args()
            .find_map(|arg| arg.strip_prefix("agli-fixture-role=").map(String::from));
        let Some(role) = role else { return };
        std::fs::write(format!("{role}.pid"), std::process::id().to_string()).unwrap();
        let next = match role.as_str() {
            "root" => "child",
            "child" => "grandchild",
            _ => {
                std::thread::sleep(Duration::from_secs(60));
                return;
            }
        };
        let mut cmd = fixture_command(std::env::current_exe().unwrap(), next);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn().unwrap();
        child.wait().unwrap();
    }

    #[cfg(windows)]
    fn process_handle(pid: u32) -> std::os::windows::io::OwnedHandle {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, SYNCHRONIZATION_SYNCHRONIZE};
        let raw = unsafe { OpenProcess(SYNCHRONIZATION_SYNCHRONIZE, 0, pid) };
        assert!(!raw.is_null(), "process {pid} should be alive");
        unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw) }
    }

    #[cfg(windows)]
    fn assert_exit(process: std::os::windows::io::OwnedHandle) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject,
        };
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle(), 5000) },
            WAIT_OBJECT_0
        );
    }

    #[cfg(target_os = "linux")]
    fn process_handle(pid: u32) -> u32 {
        assert!(Path::new(&format!("/proc/{pid}")).exists());
        pid
    }

    #[cfg(target_os = "linux")]
    fn assert_exit(pid: u32) {
        let start = std::time::Instant::now();
        let stat = format!("/proc/{pid}/stat");
        loop {
            let Ok(state) = std::fs::read_to_string(&stat) else {
                return;
            };
            if state
                .rsplit_once(") ")
                .is_some_and(|(_, fields)| fields.starts_with('Z'))
            {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "process {pid} survived"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(any(windows, target_os = "linux"))]
    async fn check_process_cleanup(cancel: bool) {
        let dir =
            std::env::temp_dir().join(format!("agli-codex-tree-{cancel}-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let mut command = fixture_command(std::env::current_exe().unwrap(), "root");
        command.current_dir(&dir);
        let (source, token) = CancelSource::new();
        let deadline = if cancel {
            Duration::from_secs(20)
        } else {
            Duration::from_secs(4)
        };
        let runner = tokio::spawn(run(command.into(), "", deadline, token));
        let processes = tokio::time::timeout(Duration::from_secs(3), async {
            let mut handles = Vec::new();
            for role in ["root", "child", "grandchild"] {
                let path = dir.join(format!("{role}.pid"));
                loop {
                    if let Some(pid) = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|p| p.parse().ok())
                    {
                        handles.push(process_handle(pid));
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            handles
        })
        .await
        .unwrap();
        if cancel {
            source.cancel();
        }
        let wait = if cancel {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(5)
        };
        let error = tokio::time::timeout(wait, runner)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(if cancel { "取消" } else { "timed out" })
        );
        for process in processes {
            assert_exit(process);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[tokio::test]
    async fn timeout_kills_child_and_grandchild() {
        check_process_cleanup(false).await;
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[tokio::test]
    async fn cancel_token_kills_child_and_grandchild_promptly() {
        check_process_cleanup(true).await;
    }
}
