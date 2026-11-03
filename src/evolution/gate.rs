//! L4: the owner bootstraps this gate; candidates cannot change it or other L4 paths.
//! V1 uses a detached worktree, not an OS security boundary. Authorship is attested by the
//! owner's explicit selection, never inferred from prose or accepted from an AGLI tool call.
use super::{
    process::{git, oid, run},
    restart::Restart,
};
use crate::{
    config::Config,
    mind::{
        judge::{self, Verdict},
        models::Models,
    },
    store::{ReviewCategory, ReviewItem, ReviewStatus, Store},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_PATCH: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub drydock: String,
    #[serde(default)]
    pub ticket: Option<u64>,
    #[serde(default)]
    pub proposal: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Direction {
    pub model: String,
    pub plan: String,
    pub interfaces: Vec<String>,
    /// Review of the exact change.patch bytes, after implementation and before owner review.
    pub reviewed_patch_sha256: String,
    pub review: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub source: Source,
    pub summary: String,
    pub base_commit: String,
    pub writer_model: String,
    pub writer_host: String,
    pub files: Vec<String>,
    /// Exact Rust test names. A zero-test run is never an eval pass.
    pub eval_tests: Vec<String>,
    #[serde(default)]
    pub director: Option<Direction>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Checking,
    AwaitingOwner,
    NeedsRevision,
    Approved,
    Rejected,
    Failed,
    Committing,
    Committed,
    RestartRequested,
    Running,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Record {
    pub id: String,
    pub request: Request,
    pub owner: String,
    pub source_snapshot: Value,
    pub patch_sha256: String,
    pub tree: String,
    pub state: State,
    pub checks: Vec<String>,
    pub review_id: Option<u64>,
    pub commit: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct Gate {
    /// The git checkout with `.moldloft/`: candidates are based, committed and built from here.
    source: PathBuf,
    /// The runtime root (`AGLI_ROOT`): evolution data lives here and the new bot starts here.
    root: PathBuf,
    dir: PathBuf,
    store: Arc<dyn Store>,
}

#[derive(Debug)]
pub struct Completion {
    pub message: String,
    pub restart: Option<Restart>,
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !matches!(id, "con" | "prn" | "aux" | "nul")
        && !((id.starts_with("com") || id.starts_with("lpt")) && id.len() == 4 && id.as_bytes()[3].is_ascii_digit())
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn is_oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|c| c.is_ascii_hexdigit())
}
fn advanced(model: &str) -> bool {
    matches!(model, "gpt-6-astra" | "opus")
}

fn allowed_path(path: &str) -> bool {
    if path.is_empty() || !path.is_ascii() || path.contains(['\\', ':', '\0']) {
        return false;
    }
    if path.split('/').any(|p| {
        p.is_empty() || p.starts_with('.') || p.ends_with(['.', ' ']) || p.bytes().any(|c| c.is_ascii_control()) || {
            let name = p.split('.').next().unwrap_or_default().to_ascii_lowercase();
            matches!(name.as_str(), "con" | "prn" | "aux" | "nul")
                || name.starts_with("com") && name.len() == 4 && name.as_bytes()[3].is_ascii_digit()
                || name.starts_with("lpt") && name.len() == 4 && name.as_bytes()[3].is_ascii_digit()
        }
    }) {
        return false;
    }
    let path = path.to_ascii_lowercase();
    // The entire gate implementation is protected, including its process and restart helpers.
    if path.starts_with("src/evolution/") || matches!(path.as_str(), "src/evolution.rs" | "src/agli/evolution.rs") {
        return false;
    }
    path.starts_with("src/")
        || path.starts_with("tests/")
        || path.starts_with("evals/")
        || path.starts_with("docs/")
        || matches!(path.as_str(), "cargo.toml" | "cargo.lock" | "readme.md")
}

fn validate(request: &Request, patch: &[u8]) -> Result<()> {
    let direct =
        matches!((request.writer_model.as_str(), request.writer_host.as_str()), ("gpt-6-astra", "codex") | ("opus", "claude-code"));
    let directed = request.writer_model == "deepseek-v4.1-flash-1017" && request.writer_host == "dsh";
    anyhow::ensure!(direct || directed, "writer is not on the core-code allowlist");
    anyhow::ensure!(!request.summary.trim().is_empty() && request.summary.len() <= 4000, "invalid summary");
    anyhow::ensure!(is_oid(&request.base_commit), "base_commit must be a full immutable commit id");
    anyhow::ensure!(valid_id(&request.source.drydock), "invalid Drydock id");
    anyhow::ensure!(request.source.ticket.is_some() ^ request.source.proposal.is_some(), "select one Moldloft ticket or proposal");
    anyhow::ensure!(request.source.ticket != Some(0), "invalid ticket");
    if let Some(id) = &request.source.proposal {
        anyhow::ensure!(valid_id(id), "invalid proposal id");
    }
    anyhow::ensure!(!patch.is_empty() && patch.len() <= MAX_PATCH, "patch must contain 1..256 KiB");
    anyhow::ensure!(
        !request.files.is_empty() && request.files.len() <= 100 && request.files.iter().all(|f| allowed_path(f)),
        "scope contains an L4, private or unsupported path"
    );
    let files: BTreeSet<_> = request.files.iter().map(|f| f.to_ascii_lowercase()).collect();
    anyhow::ensure!(files.len() == request.files.len(), "duplicate or case-aliased scope path");
    anyhow::ensure!(
        !request.eval_tests.is_empty()
            && request.eval_tests.len() <= 20
            && request
                .eval_tests
                .iter()
                .all(|t| !t.is_empty() && t.len() <= 200 && t.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b':')),
        "eval_tests must name 1..20 exact Rust tests"
    );
    if directed || request.director.is_some() {
        let d = request.director.as_ref().context("directed writer requires an Astra/Opus director")?;
        anyhow::ensure!(
            advanced(&d.model)
                && !d.plan.trim().is_empty()
                && !d.review.trim().is_empty()
                && !d.interfaces.is_empty()
                && d.interfaces.iter().all(|s| !s.trim().is_empty()),
            "incomplete direction record"
        );
        anyhow::ensure!(d.reviewed_patch_sha256 == hash(patch), "director reviewed a different patch");
    }
    Ok(())
}

impl Gate {
    pub fn new(cfg: &Config, store: Arc<dyn Store>) -> Self {
        let root = cfg.root.canonicalize().unwrap_or_else(|_| cfg.root.clone());
        let source = cfg.source.canonicalize().unwrap_or_else(|_| cfg.source.clone());
        Self { dir: root.join(&cfg.runtime.data_dir).join("evolution"), source, root, store }
    }
    fn worktree(&self, id: &str) -> PathBuf {
        self.dir.join("runs").join(id)
    }
    fn key(id: &str) -> String {
        format!("evolution:{id}")
    }
    fn save(&self, record: &Record) -> Result<()> {
        self.store.kv_set(&Self::key(&record.id), json!(record))
    }
    pub fn load(&self, id: &str) -> Result<Record> {
        anyhow::ensure!(valid_id(id), "invalid evolution id");
        serde_json::from_value(self.store.kv_get(&Self::key(id))?.context("unknown evolution id")?).map_err(Into::into)
    }
    pub fn status(&self, id: &str) -> Result<String> {
        let r = self.load(id)?;
        let receipt =
            std::fs::read(self.dir.join("logs").join(id).join("restart.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        Ok(format!(
            "{}：{:?}\n{}\n作者：{} / {}\n提交：{}\n{}{}",
            r.id,
            r.state,
            r.request.summary,
            r.request.writer_model,
            r.request.writer_host,
            r.commit.as_deref().unwrap_or("尚未提交"),
            r.error.as_deref().unwrap_or(""),
            receipt
                .map(|v| format!("\n重启：{} {}", v["state"].as_str().unwrap_or("unknown"), v["error"].as_str().unwrap_or("")))
                .unwrap_or_default()
        ))
    }
    pub fn interrupted(&self, id: &str, why: &str) -> Result<()> {
        if let Ok(mut r) = self.load(id) {
            // Never turn a possibly committed/deployed change back into a retryable proposal.
            r.error = Some(why.into());
            if matches!(r.state, State::Checking | State::Approved) {
                r.state = State::Failed;
            }
            self.save(&r)?;
        }
        Ok(())
    }

    async fn source(&self, source: &Source) -> Result<Value> {
        let manifest: Value = serde_json::from_slice(&std::fs::read(self.source.join(".moldloft/project.json"))?)?;
        let port = manifest["port"].as_u64().filter(|p| (1..=65535).contains(p)).context("Moldloft port")?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        let endpoint = if let Some(id) = &source.proposal { format!("proposals/{id}") } else { "graph".into() };
        let mut response = client
            .get(format!("http://127.0.0.1:{port}/api/{endpoint}"))
            .query(&[("drydock", &source.drydock)])
            .send()
            .await?
            .error_for_status()?;
        anyhow::ensure!(response.content_length().is_none_or(|n| n <= 8 * 1024 * 1024), "Moldloft response too large");
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(body.len() + chunk.len() <= 8 * 1024 * 1024, "Moldloft response too large");
            body.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&body)?;
        let snapshot = source_snapshot(source, value)?;
        anyhow::ensure!(serde_json::to_vec(&snapshot)?.len() <= 64 * 1024, "Moldloft brief exceeds 64 KiB review limit");
        Ok(snapshot)
    }

    pub async fn prepare(&self, id: &str, owner: &str, models: &Models) -> Result<Completion> {
        anyhow::ensure!(valid_id(id), "invalid evolution id");
        anyhow::ensure!(
            self.store.kv_get(&Self::key(id))?.is_none(),
            "id already used; inspect /evolve-status, use a new id for a revised patch"
        );
        let inbox = self.dir.join("inbox").join(id);
        let request_bytes = std::fs::read(inbox.join("proposal.json"))?;
        anyhow::ensure!(request_bytes.len() <= 64 * 1024, "proposal too large");
        let request: Request = serde_json::from_slice(&request_bytes)?;
        let patch = std::fs::read(inbox.join("change.patch"))?;
        validate(&request, &patch)?; // writer/scope check happens before any build or patch application
        let source = self.source(&request.source).await?;
        let mut record = Record {
            id: id.into(),
            request,
            owner: owner.into(),
            source_snapshot: source,
            patch_sha256: hash(&patch),
            tree: String::new(),
            state: State::Checking,
            checks: Vec::new(),
            review_id: None,
            commit: None,
            error: None,
        };
        self.save(&record)?;
        let result = self.check(&mut record, &patch, models).await;
        if let Err(e) = &result {
            self.interrupted(id, &format!("{e:#}"))?;
        }
        result
    }

    async fn check(&self, r: &mut Record, patch: &[u8], models: &Models) -> Result<Completion> {
        let worktree = self.worktree(&r.id);
        anyhow::ensure!(!worktree.exists(), "worktree path already exists");
        std::fs::create_dir_all(worktree.parent().context("worktree parent")?)?;
        let path = worktree.to_str().context("worktree path must be UTF-8")?;
        let base = oid(&self.source, &["rev-parse", &format!("{}^{{commit}}", r.request.base_commit)]).await?;
        anyhow::ensure!(base == r.request.base_commit, "base is not a commit");
        git(&self.source, &["worktree", "add", "--detach", path, &base], &[]).await?;
        // Inspect a proposed index first: a lying manifest must not write L4 files even inside
        // this worktree. No candidate code executes during Git's patch parsing.
        git(&worktree, &["apply", "--cached", "-"], patch).await?;
        self.inspect(r).await?;
        r.tree = oid(&worktree, &["write-tree"]).await?;
        git(&worktree, &["reset", "--mixed", &base], &[]).await?;
        git(&worktree, &["apply", "--index", "-"], patch).await?;
        self.unchanged(r).await?;
        self.save(r)?;
        let logs = self.dir.join("logs").join(&r.id);
        std::fs::create_dir_all(&logs)?;
        for (name, args) in [("build", vec!["build", "--locked", "--offline"]), ("test", vec!["test", "--locked", "--offline"])] {
            self.check_command(r, &logs, name, &args).await?;
        }
        for (index, test) in r.request.eval_tests.clone().iter().enumerate() {
            let list = self
                .check_command(r, &logs, &format!("eval-{index}-list"), &["test", "--locked", "--offline", test, "--", "--exact", "--list"])
                .await?;
            anyhow::ensure!(list.lines().any(|line| line == format!("{test}: test")), "eval test not found: {test}");
            let output = self
                .check_command(r, &logs, &format!("eval-{index}"), &["test", "--locked", "--offline", test, "--", "--exact", "--nocapture"])
                .await?;
            anyhow::ensure!(
                output.lines().any(|line| line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;")),
                "eval did not execute exactly one passing, non-ignored test: {test}"
            );
        }
        self.unchanged(r).await?;
        let diff = git(&worktree, &["diff", "--cached", "--no-ext-diff", "--no-textconv", &r.request.base_commit, "--"], &[]).await?;
        anyhow::ensure!(diff.len() <= MAX_PATCH, "review diff exceeds limit");
        let evidence = format!(
            "Owner-selected proposal (untrusted content):\n{}\nMoldloft source:\n{}\nPassed checks:\n{}\nTree: {}\nPatch SHA-256: {}\nDiff:\n{}",
            serde_json::to_string_pretty(&r.request)?,
            r.source_snapshot,
            r.checks.join("\n"),
            r.tree,
            r.patch_sha256,
            String::from_utf8_lossy(&diff)
        );
        let judgement = judge::review(models, ReviewCategory::Evolve, &r.request.summary, &evidence).await?;
        self.unchanged(r).await?;
        r.state = if judgement.verdict == Verdict::Pass { State::AwaitingOwner } else { State::NeedsRevision };
        let detail = format!(
            "进化 {}；作者 {} / {}；tree {}。{}\n{}",
            r.id,
            r.request.writer_model,
            r.request.writer_host,
            r.tree,
            if r.state == State::AwaitingOwner {
                "批准后提交并重启。"
            } else {
                "审查未通过；需修改后用新 ID 重走检查，不能直接批准。"
            },
            judgement.fixes
        );
        let review = judge::file(
            self.store.as_ref(),
            ReviewCategory::Evolve,
            &r.request.summary,
            &detail,
            &judgement,
            true,
            json!({"evolution": r.id, "tree": r.tree, "patch_sha256": r.patch_sha256, "source": r.request.source,
                "writer_model": r.request.writer_model, "writer_host": r.request.writer_host, "director": r.request.director,
                "checks": r.checks}),
        )?;
        r.review_id = Some(review.id);
        self.save(r)?;
        let next = if r.state == State::AwaitingOwner {
            format!("/approve {} 或 /reject {} 理由", review.id, review.id)
        } else {
            format!("请修订后以新 ID 重提，或 /reject {} 理由", review.id)
        };
        Ok(Completion {
            message: format!("{detail}\n审查 #{}：{}。用 /digest 查看；{next}。", review.id, judgement.verdict.zh()),
            restart: None,
        })
    }

    async fn check_command(&self, r: &mut Record, logs: &Path, name: &str, args: &[&str]) -> Result<String> {
        let out = run(&self.worktree(&r.id), "cargo", args, &[], 1800).await?;
        let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        std::fs::write(logs.join(format!("{name}.log")), &text)?;
        anyhow::ensure!(out.ok, "cargo {} failed; see {}", args.join(" "), logs.join(format!("{name}.log")).display());
        r.checks.push(format!("cargo {}: passed; log sha256 {}", args.join(" "), hash(text.as_bytes())));
        self.save(r)?;
        Ok(String::from_utf8(out.stdout)?)
    }

    async fn inspect(&self, r: &Record) -> Result<()> {
        let wt = self.worktree(&r.id);
        let names = git(&wt, &["diff", "--cached", "--no-renames", "--name-only", "-z", &r.request.base_commit, "--"], &[]).await?;
        let actual: BTreeSet<_> = std::str::from_utf8(&names)?.split('\0').filter(|s| !s.is_empty()).map(str::to_owned).collect();
        let scope: BTreeSet<_> = r.request.files.iter().cloned().collect();
        anyhow::ensure!(actual == scope && actual.iter().all(|p| allowed_path(p)), "patch paths differ from declared scope or touch L4");
        let raw = git(&wt, &["diff", "--cached", "--no-renames", "--raw", &r.request.base_commit, "--"], &[]).await?;
        for line in std::str::from_utf8(&raw)?.lines() {
            let modes: Vec<_> = line.trim_start_matches(':').split_whitespace().take(2).collect();
            anyhow::ensure!(
                modes.len() == 2 && modes.iter().all(|m| matches!(*m, "000000" | "100644" | "100755")),
                "symlink/submodule changes are forbidden"
            );
        }
        Ok(())
    }

    async fn unchanged(&self, r: &Record) -> Result<()> {
        let wt = self.worktree(&r.id);
        self.inspect(r).await?;
        anyhow::ensure!(oid(&wt, &["rev-parse", "HEAD"]).await? == r.request.base_commit, "candidate HEAD changed");
        anyhow::ensure!(oid(&wt, &["write-tree"]).await? == r.tree, "candidate index changed; rerun review with a new id");
        git(&wt, &["diff", "--exit-code", "--no-ext-diff", "--no-textconv", "--"], &[]).await?;
        let extra = git(&wt, &["ls-files", "--others", "--exclude-standard", "-z"], &[]).await?;
        anyhow::ensure!(extra.is_empty(), "candidate has untracked files");
        Ok(())
    }

    pub fn approve(&self, item: &ReviewItem, owner: &str) -> Result<String> {
        let id = item.payload["evolution"].as_str().context("missing evolution id")?;
        let mut r = self.load(id)?;
        anyhow::ensure!(
            item.category == ReviewCategory::Evolve
                && item.status == ReviewStatus::Open
                && r.review_id == Some(item.id)
                && r.state == State::AwaitingOwner
                && item.payload["tree"] == r.tree
                && item.payload["patch_sha256"] == r.patch_sha256,
            "evolution is not eligible for approval"
        );
        anyhow::ensure!(r.owner == owner, "only the selecting owner may approve");
        r.state = State::Approved;
        self.save(&r)?;
        Ok(id.into())
    }

    pub fn reject(&self, item: &ReviewItem) -> Result<()> {
        let id = item.payload["evolution"].as_str().context("missing evolution id")?;
        let mut r = self.load(id)?;
        anyhow::ensure!(
            r.review_id == Some(item.id) && matches!(r.state, State::AwaitingOwner | State::NeedsRevision),
            "evolution cannot be rejected in this state"
        );
        r.state = State::Rejected;
        self.save(&r)
    }

    pub async fn finish(&self, id: &str) -> Result<Completion> {
        let result = self.finish_inner(id).await;
        if let Err(e) = &result {
            self.interrupted(id, &format!("{e:#}"))?;
        }
        result
    }
    async fn finish_inner(&self, id: &str) -> Result<Completion> {
        let mut r = self.load(id)?;
        anyhow::ensure!(r.state == State::Approved, "evolution is not approved");
        let review = self.store.reviews()?.into_iter().find(|v| Some(v.id) == r.review_id).context("approval record missing")?;
        anyhow::ensure!(review.status == ReviewStatus::Approved, "owner approval was not recorded");
        anyhow::ensure!(self.source(&r.request.source).await? == r.source_snapshot, "Moldloft source changed; stage a new proposal");
        self.unchanged(&r).await?;
        let commit = self.commit(&mut r).await?;
        // Rebuild from the approved committed tree, not an executable left by earlier tests.
        let wt = self.worktree(id);
        let artifacts = self.dir.join("releases").join(id);
        anyhow::ensure!(!artifacts.exists(), "release directory already exists; inspect before retrying");
        let out = run(
            &wt,
            "cargo",
            &[
                "build",
                "--locked",
                "--offline",
                "--release",
                "--bin",
                "aglibot",
                "--target-dir",
                artifacts.to_str().context("release path must be UTF-8")?,
            ],
            &[],
            1800,
        )
        .await?;
        let logs = self.dir.join("logs").join(id);
        std::fs::write(logs.join("release.log"), [out.stdout, out.stderr].concat())?;
        anyhow::ensure!(out.ok, "approved commit {commit}; release build failed, no restart");
        anyhow::ensure!(
            oid(&wt, &["rev-parse", "HEAD"]).await? == commit && oid(&wt, &["write-tree"]).await? == r.tree,
            "committed candidate changed during release build"
        );
        git(&wt, &["diff", "--exit-code", "--no-ext-diff", "--no-textconv", "HEAD", "--"], &[]).await?;
        let executable = artifacts.join("release").join(if cfg!(windows) { "aglibot.exe" } else { "aglibot" });
        anyhow::ensure!(executable.is_file(), "release executable missing");
        let restart = Restart {
            id: id.into(),
            commit: commit.clone(),
            executable_sha256: hash(&std::fs::read(&executable)?),
            executable: executable.canonicalize()?,
            root: self.root.canonicalize()?,
            receipt: self.dir.join("logs").join(id).join("restart.json"),
        };
        // Persist intent before an external effect. Interrupted restarts are inspected, never replayed.
        r.state = State::RestartRequested;
        self.save(&r)?;
        Ok(Completion {
            message: format!("进化 {id} 已提交 {commit}；准备切换到已批准版本并重启。"), restart: Some(restart)
        })
    }

    pub fn acknowledge_start(&self) -> Result<()> {
        let Ok(id) = std::env::var("AGLI_EVOLUTION_STARTED") else { return Ok(()) };
        let mut r = self.load(&id)?;
        if r.state != State::RestartRequested {
            return Ok(());
        }
        let request = self.dir.join("logs").join(&id).join("restart-request.json");
        let restart: Restart = serde_json::from_slice(&std::fs::read(request)?)?;
        anyhow::ensure!(
            std::env::current_exe()?.canonicalize()? == restart.executable.canonicalize()? && r.commit.as_deref() == Some(&restart.commit),
            "restart acknowledgement does not match approved executable"
        );
        anyhow::ensure!(
            hash(&std::fs::read(&restart.executable)?) == restart.executable_sha256,
            "running executable differs from approved release"
        );
        r.state = State::Running;
        self.save(&r)?;
        restart.receipt("running", None)?;
        crate::evolution::restart::record_release(self.dir.parent().context("evolution dir has no data dir")?, &restart.commit, &restart.executable, &format!("evolution:{id}"))
    }

    async fn commit(&self, r: &mut Record) -> Result<String> {
        anyhow::ensure!(
            r.state == State::Approved
                && self.store.reviews()?.iter().any(|item| Some(item.id) == r.review_id && item.status == ReviewStatus::Approved),
            "commit requires recorded owner approval"
        );
        self.unchanged(r).await?;
        r.state = State::Committing;
        self.save(r)?;
        let wt = self.worktree(&r.id);
        let message = format!(
            "Evolution {}: {}\n\nWriter: {} ({})\nMoldloft: {}\nApproved review: {}\nPatch-SHA256: {}\n",
            r.id,
            r.request.summary,
            r.request.writer_model,
            r.request.writer_host,
            serde_json::to_string(&r.request.source)?,
            r.review_id.unwrap_or_default(),
            r.patch_sha256
        );
        // commit-tree commits precisely the reviewed tree without executing repository hooks.
        let commit = String::from_utf8(
            git(
                &wt,
                &[
                    "-c",
                    "user.name=AGLI evolution",
                    "-c",
                    "user.email=agli@localhost",
                    "commit-tree",
                    &r.tree,
                    "-p",
                    &r.request.base_commit,
                ],
                message.as_bytes(),
            )
            .await?,
        )?
        .trim()
        .to_string();
        r.commit = Some(commit.clone());
        self.save(r)?; // an uncertain ref update retains the exact commit for owner inspection
        git(&wt, &["update-ref", &format!("refs/heads/codex/evolution-{}", r.id), &commit, &"0".repeat(commit.len())], &[]).await?;
        git(&wt, &["update-ref", "HEAD", &commit, &r.request.base_commit], &[]).await?;
        r.state = State::Committed;
        self.save(r)?;
        Ok(commit)
    }
}

fn source_snapshot(source: &Source, value: Value) -> Result<Value> {
    if let Some(id) = source.ticket {
        anyhow::ensure!(value["drydock_id"] == source.drydock, "wrong Moldloft Drydock");
        let ticket = value["tickets"]
            .as_array()
            .and_then(|items| items.iter().find(|t| t["id"].as_u64() == Some(id)))
            .context("Moldloft ticket missing")?;
        anyhow::ensure!(
            ticket["status"] != "closed" && ticket["blockers"].as_array().is_some_and(Vec::is_empty),
            "Moldloft ticket is closed or blocked"
        );
        // Ignore translation/cache churn, but include the version and exact implementation brief.
        Ok(json!({"drydock": source.drydock, "ticket": id, "version": ticket["version"], "title": ticket["title"],
            "description": ticket["description"], "tech_notes": ticket["tech_notes"], "status": ticket["status"]}))
    } else {
        let proposal = value.get("proposal").unwrap_or(&value);
        anyhow::ensure!(
            proposal["id"].as_str() == source.proposal.as_deref() && proposal["status"] == "applied",
            "Moldloft proposal must exist and be accepted/applied"
        );
        Ok(proposal.clone())
    }
}

#[cfg(test)]
mod tests;
