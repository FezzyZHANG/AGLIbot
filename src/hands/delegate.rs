//! His hands: delegate work to Codex (`codex exec`) as a subprocess, confined to the task
//! directory. Only owner-approved tasks ever reach here. Claude Code is the owner's tool and is
//! never invoked from here (see the root AGENTS.md).
//!
//! The prompt goes through stdin, so no shell quoting is involved; on Windows the npm shims are
//! `.cmd` files, which `Command` runs through cmd.exe.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

fn program(name: &str) -> String {
    if cfg!(windows) { format!("{name}.cmd") } else { name.to_string() }
}

async fn run(mut cmd: Command, stdin: &str, timeout: Duration) -> Result<String> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().context("starting delegate process")?;
    if let Some(mut input) = child.stdin.take() {
        input.write_all(stdin.as_bytes()).await?;
        input.shutdown().await?;
    }
    let out = tokio::time::timeout(timeout, child.wait_with_output()).await.context("delegate timed out")??;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        // CLIs do not agree on which stream carries the error, so show both.
        let err = String::from_utf8_lossy(&out.stderr);
        let detail: String = format!("{} {}", err.trim(), stdout.trim()).chars().take(800).collect();
        bail!("delegate exited with {}: {detail}", out.status);
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
    let events = run(codex_command(&dir, &last, model, search, resume), prompt, timeout).await?;
    let session_id = session_id(&events)?;
    if let Some(expected) = resume {
        anyhow::ensure!(session_id == expected, "Codex resumed a different session: {session_id}");
    }
    let text = std::fs::read_to_string(&last).context("reading Codex's last message")?;
    Ok(CodexOutput { text, session_id })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
