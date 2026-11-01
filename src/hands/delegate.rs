//! His hands: delegate work to Claude Code (`claude -p`) or Codex (`codex exec`) as subprocesses,
//! confined to the task directory. Only owner-approved tasks ever reach here.
//!
//! The prompt goes through stdin, so no shell quoting is involved; on Windows the npm shims are
//! `.cmd` files, which `Command` runs through cmd.exe.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct Delegated {
    pub text: String,
    pub cost_usd: Option<f64>,
}

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
    let out = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .context("delegate timed out")??;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if !out.status.success() {
        // Some CLIs report errors on stdout (e.g. claude's JSON result), so show both.
        let err = String::from_utf8_lossy(&out.stderr);
        let detail: String = format!("{} {}", err.trim(), stdout.trim()).chars().take(800).collect();
        bail!("delegate exited with {}: {detail}", out.status);
    }
    Ok(stdout)
}

/// Research with Claude Code: web search/fetch and file edits inside `dir` only, no shell.
pub async fn claude(dir: &Path, prompt: &str, budget_usd: f64, timeout: Duration) -> Result<Delegated> {
    let mut cmd = Command::new(program("claude"));
    cmd.current_dir(dir).args([
        "-p",
        "--output-format",
        "json",
        "--permission-mode",
        "acceptEdits",
        "--allowedTools",
        "WebSearch,WebFetch,Read,Write,Edit,Glob,Grep",
        "--disallowedTools",
        "Bash",
        "--max-budget-usd",
        &format!("{budget_usd:.2}"),
    ]);
    let out = run(cmd, prompt, timeout).await?;
    let v: Value = serde_json::from_str(out.trim()).context("claude did not return JSON")?;
    if v.get("is_error").and_then(Value::as_bool) == Some(true) {
        bail!("claude reported an error: {}", v.get("result").unwrap_or(&Value::Null));
    }
    Ok(Delegated {
        text: v.get("result").and_then(Value::as_str).unwrap_or_default().to_string(),
        cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
    })
}

/// Codex in a workspace-write sandbox rooted at `dir`; `search` turns on its live web search.
pub async fn codex(dir: &Path, prompt: &str, search: bool, timeout: Duration) -> Result<Delegated> {
    let last = dir.join("codex-last-message.md");
    let mut cmd = Command::new(program("codex"));
    if search {
        cmd.arg("--search");
    }
    cmd.current_dir(dir).args(["exec", "--sandbox", "workspace-write", "--skip-git-repo-check", "-C"]).arg(dir).arg("-o").arg(&last).arg("-");
    run(cmd, prompt, timeout).await?;
    let text = std::fs::read_to_string(&last).unwrap_or_default();
    Ok(Delegated { text, cost_usd: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spends a few cents of the owner's Claude Code quota: opt-in only.
    #[tokio::test]
    #[ignore = "runs the local claude CLI"]
    async fn live_claude_round_trip() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("aglibot-test-claude-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let r = claude(&dir, "Reply with exactly: OK", 0.2, Duration::from_secs(180)).await?;
        assert!(r.text.contains("OK"), "{r:?}");
        println!("claude ok, cost {:?}", r.cost_usd);
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    /// Uses a little of the owner's Codex quota: opt-in only.
    #[tokio::test]
    #[ignore = "runs the local codex CLI"]
    async fn live_codex_round_trip() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("aglibot-test-codex-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let r = codex(&dir, "Create a file named ok.txt containing OK, then reply with exactly: DONE", true, Duration::from_secs(300)).await?;
        assert!(dir.join("ok.txt").exists(), "{r:?}");
        println!("codex: {}", r.text.trim());
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}
