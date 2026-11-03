//! A small handoff process waits for the old bot to exit before starting the approved binary.
//! No overlapping listeners, shell strings, automatic retries or edits to the owner's checkout.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Restart {
    pub id: String,
    pub commit: String,
    pub executable: PathBuf,
    pub executable_sha256: String,
    pub root: PathBuf,
    pub receipt: PathBuf,
}

struct PendingHelper(Option<std::process::Child>);

impl Drop for PendingHelper {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Restart {
    pub fn receipt(&self, state: &str, error: Option<&str>) -> Result<()> {
        std::fs::write(
            &self.receipt,
            serde_json::to_vec_pretty(&json!({
                "id": self.id, "commit": self.commit, "state": state, "error": error,
                "executable": self.executable, "at": chrono::Utc::now(),
            }))?,
        )?;
        Ok(())
    }
    pub async fn launch(&self) -> Result<()> {
        let request = self.receipt.with_file_name("restart-request.json");
        std::fs::write(&request, serde_json::to_vec_pretty(self)?)?;
        self.receipt("launching_helper", None)?;
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.arg("evolution-restart")
            .arg(std::process::id().to_string())
            .arg(request)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        let mut child = PendingHelper(Some(cmd.spawn().context("starting restart handoff")?));
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let receipt: serde_json::Value = serde_json::from_slice(&std::fs::read(&self.receipt)?).unwrap_or_default();
            if receipt["state"] == "waiting_for_exit" {
                child.0.take(); // explicitly detach only after the helper has captured our process
                return Ok(());
            }
            if let Some(status) = child.0.as_mut().context("missing helper")?.try_wait()? {
                anyhow::bail!("restart helper exited: {status}; inspect {}", self.receipt.display());
            }
        }
        anyhow::bail!("restart helper did not acknowledge readiness; old bot remains running")
    }
}

/// `data/release.json`: the one record of what the bot should run. Manual deploys
/// (`ops/deploy.ps1`) and evolution both write it; `ops/start-bot.ps1` starts only what it names.
pub const RELEASE_FILE: &str = "release.json";

pub fn record_release(data_dir: &std::path::Path, commit: &str, exe: &std::path::Path, origin: &str) -> Result<()> {
    let exe = exe.canonicalize()?;
    let release = json!({
        "commit": commit, "exe": exe, "exe_sha256": format!("{:x}", Sha256::digest(std::fs::read(&exe)?)),
        "origin": origin, "at": chrono::Utc::now(),
    });
    std::fs::write(data_dir.join(RELEASE_FILE), serde_json::to_vec_pretty(&release)?)?;
    Ok(())
}

/// Logged at startup; a mismatch is reported, not enforced (the start script enforces it).
pub fn describe_release(data_dir: &std::path::Path) -> String {
    let Ok(bytes) = std::fs::read(data_dir.join(RELEASE_FILE)) else { return "no release pointer".into() };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return "unreadable release pointer".into() };
    let exe = std::env::current_exe().and_then(std::fs::read).map(|b| format!("{:x}", Sha256::digest(b)));
    let matches = exe.is_ok_and(|sha| v["exe_sha256"].as_str() == Some(sha.as_str()));
    format!("release {} ({}){}", v["commit"].as_str().unwrap_or("?"), v["origin"].as_str().unwrap_or("?"),
        if matches { "" } else { "; WARNING: running executable differs from the release pointer" })
}

/// Called before loading configuration or opening a database in the helper process.
pub fn helper(parent: u32, request: PathBuf) -> Result<()> {
    let restart: Restart = serde_json::from_slice(&std::fs::read(request)?)?;
    let result = handoff(parent, &restart);
    if let Err(e) = &result {
        restart.receipt("failed", Some(&format!("{e:#}")))?;
    }
    result
}

fn handoff(parent: u32, restart: &Restart) -> Result<()> {
    anyhow::ensure!(parent != 0 && parent != std::process::id(), "invalid parent process");
    #[cfg(windows)]
    let parent_handle = {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
        let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parent) };
        anyhow::ensure!(!h.is_null(), "opening parent process: {}", std::io::Error::last_os_error());
        // OpenProcess created this handle; retain it so PID reuse cannot target a new process.
        unsafe { OwnedHandle::from_raw_handle(h) }
    };
    restart.receipt("waiting_for_exit", None)?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        #[cfg(windows)]
        let alive = {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::{
                Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
                System::Threading::WaitForSingleObject,
            };
            match unsafe { WaitForSingleObject(parent_handle.as_raw_handle(), 0) } {
                WAIT_OBJECT_0 => false,
                WAIT_TIMEOUT => true,
                _ => anyhow::bail!("waiting for parent failed"),
            }
        };
        #[cfg(unix)]
        let alive = if unsafe { libc::kill(parent as i32, 0) } == 0 {
            true
        } else {
            let error = std::io::Error::last_os_error();
            anyhow::ensure!(error.raw_os_error() == Some(libc::ESRCH), "checking parent process: {error}");
            false
        };
        if !alive {
            break;
        }
        anyhow::ensure!(Instant::now() < deadline, "old bot did not exit within 60 seconds");
        std::thread::sleep(Duration::from_millis(100));
    }
    let digest = format!("{:x}", Sha256::digest(std::fs::read(&restart.executable)?));
    anyhow::ensure!(digest == restart.executable_sha256, "approved executable changed before restart");
    // Write before spawn: a lost acknowledgement must not cause a second launch.
    restart.receipt("starting", None)?;
    let mut cmd = Command::new(&restart.executable);
    cmd.current_dir(&restart.root).env("AGLI_ROOT", &restart.root).env("AGLI_EVOLUTION_STARTED", &restart.id).stdin(Stdio::null());
    let log = std::fs::OpenOptions::new().create(true).append(true).open(restart.receipt.with_file_name("runtime.log"))?;
    cmd.stdout(log.try_clone()?).stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    cmd.spawn().context("starting approved executable")?;
    // The new bot acknowledges Running after initialization, just before entering its main loop.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_pointer_names_commit_and_flags_another_executable() {
        let dir = std::env::temp_dir().join(format!("aglibot-test-release-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(describe_release(&dir), "no release pointer");
        let exe = std::env::current_exe().unwrap();
        record_release(&dir, "abc123", &exe, "manual").unwrap();
        assert_eq!(describe_release(&dir), "release abc123 (manual)");
        let other = dir.join("other.exe");
        std::fs::write(&other, b"not the running binary").unwrap();
        record_release(&dir, "def456", &other, "evolution:x").unwrap();
        assert!(describe_release(&dir).starts_with("release def456 (evolution:x); WARNING"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
