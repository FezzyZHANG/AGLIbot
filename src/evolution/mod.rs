//! Evolution command placeholder and read-only release diagnostics.

use sha2::{Digest, Sha256};

pub const PLACEHOLDER_REPLY: &str = "自进化功能暂未开放，当前核心代码更新统一通过 Git 与 ops/deploy.ps1 发布。";
pub const RELEASE_FILE: &str = "release.json";

pub fn describe_release(data_dir: &std::path::Path) -> String {
    let Ok(bytes) = std::fs::read(data_dir.join(RELEASE_FILE)) else { return "no release pointer".into() };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return "unreadable release pointer".into() };
    let exe = std::env::current_exe().and_then(std::fs::read).map(|b| format!("{:x}", Sha256::digest(b)));
    let matches = exe.is_ok_and(|sha| v["exe_sha256"].as_str() == Some(sha.as_str()));
    format!("release {} ({}){}", v["commit"].as_str().unwrap_or("?"), v["origin"].as_str().unwrap_or("?"),
        if matches { "" } else { "; WARNING: running executable differs from the release pointer" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_pointer_reports_the_running_executable() -> anyhow::Result<()> {
        struct TestDir(std::path::PathBuf);

        impl Drop for TestDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let dir = std::env::temp_dir().join(format!("aglibot-test-release-{}", std::process::id()));
        std::fs::create_dir(&dir)?;
        let dir = TestDir(dir);
        let path = dir.0.join(RELEASE_FILE);
        assert_eq!(describe_release(&dir.0), "no release pointer");
        std::fs::write(&path, b"invalid JSON")?;
        assert_eq!(describe_release(&dir.0), "unreadable release pointer");

        let sha = format!("{:x}", Sha256::digest(std::fs::read(std::env::current_exe()?)?));
        let mut release = serde_json::json!({"commit": "abc123", "origin": "manual", "exe_sha256": sha});
        std::fs::write(&path, serde_json::to_vec(&release)?)?;
        assert_eq!(describe_release(&dir.0), "release abc123 (manual)");

        release["exe_sha256"] = serde_json::json!("different hash");
        std::fs::write(&path, serde_json::to_vec(&release)?)?;
        assert_eq!(
            describe_release(&dir.0),
            "release abc123 (manual); WARNING: running executable differs from the release pointer"
        );
        Ok(())
    }
}
