use crate::hands::process_tree::ProcessTree;
use anyhow::{Context, Result};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

pub struct Output {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Fixed executables/argument vectors only; no proposal-supplied shell commands. The lifetime
/// guard also kills descendants when the owner stops/freezes a running check.
pub async fn run(dir: &Path, program: &str, args: &[&str], input: &[u8], seconds: u64) -> Result<Output> {
    let mut cmd = Command::new(program);
    cmd.current_dir(dir).env_clear();
    let env = crate::ns::secrets::process_registry().assemble(
        "evolution",
        &[
            "PATH",
            "SystemRoot",
            "windir",
            "COMSPEC",
            "PATHEXT",
            "TEMP",
            "TMP",
            "TMPDIR",
            "HOME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "APPDATA",
            "LOCALAPPDATA",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "ProgramData",
            "ALLUSERSPROFILE",
            "SystemDrive",
            "PROCESSOR_ARCHITECTURE",
            "PROCESSOR_ARCHITEW6432",
            "INCLUDE",
            "LIB",
            "LIBPATH",
            "VCToolsInstallDir",
            "WindowsSdkDir",
        ],
        std::env::vars_os(),
        None,
    );
    cmd.envs(env.variables)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut tree = ProcessTree::prepare(&mut cmd)?;
    let mut child = cmd.spawn().with_context(|| format!("starting {program}"))?;
    tree.attach(&child)?;
    let mut stdin = child.stdin.take().context("stdin")?;
    let mut stdout = child.stdout.take().context("stdout")?;
    let mut stderr = child.stderr.take().context("stderr")?;
    async fn read(pipe: &mut (impl AsyncReadExt + Unpin)) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        pipe.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes).await?;
        anyhow::ensure!(bytes.len() <= 4 * 1024 * 1024, "process output exceeded 4 MiB");
        Ok(bytes)
    }
    let work = async {
        let write = async {
            stdin.write_all(input).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<_, anyhow::Error>(())
        };
        let wait = async { Ok::<_, anyhow::Error>(child.wait().await?) };
        let (_, out, err, status) = tokio::try_join!(write, read(&mut stdout), read(&mut stderr), wait)?;
        Ok(Output { ok: status.success(), stdout: out, stderr: err })
    };
    tokio::time::timeout(Duration::from_secs(seconds), work).await.context("evolution command timed out")?
}

pub async fn git(dir: &Path, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
    let mut fixed = vec!["-c", "core.hooksPath=", "-c", "core.autocrlf=false", "-c", "submodule.recurse=false"];
    fixed.extend_from_slice(args);
    let out = run(dir, "git", &fixed, input, 60).await?;
    anyhow::ensure!(out.ok, "git failed: {}", String::from_utf8_lossy(&out.stderr));
    Ok(out.stdout)
}

pub async fn oid(dir: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git(dir, args, &[]).await?)?.trim().to_string())
}
