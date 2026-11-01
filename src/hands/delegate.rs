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
    let out = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .context("delegate timed out")??;
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
    let last = dir.join("codex-last-message.md");
    let mut cmd = Command::new(program("codex"));
    if search {
        cmd.arg("--search");
    }
    cmd.current_dir(dir)
        .args(["exec", "-m", model, "--sandbox", "workspace-write", "--skip-git-repo-check", "-C"])
        .arg(dir)
        .arg("-o")
        .arg(&last)
        .arg("-");
    run(cmd, prompt, timeout).await?;
    Ok(std::fs::read_to_string(&last).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

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
