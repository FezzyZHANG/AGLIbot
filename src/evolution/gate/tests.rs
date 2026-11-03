use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const ORIGINAL: &str = "fn main() { println!(\"old\"); }\n#[test] fn eval_positive() { assert_eq!(2 + 2, 4); }\n";
const UPDATED: &str = r#"fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("wait") => { std::thread::sleep(std::time::Duration::from_millis(400)); return; }
        Some("delayed") => { std::thread::sleep(std::time::Duration::from_secs(2)); std::fs::write("late-marker", "leaked").unwrap(); return; }
        Some("fork") => {
            std::process::Command::new(std::env::current_exe().unwrap()).arg("delayed").spawn().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(5)); return;
        }
        _ => {}
    }
    println!("new");
    if let Ok(root) = std::env::var("AGLI_ROOT") { std::fs::write(std::path::Path::new(&root).join("started-marker"), "new").unwrap(); }
}
#[test] fn eval_positive() { assert_eq!(2 + 2, 4); }
"#;

struct Fixture {
    gate: Gate,
    request: Request,
    patch: Vec<u8>,
    source: Arc<Mutex<Value>>,
    stop: Arc<AtomicBool>,
    server: Option<std::thread::JoinHandle<()>>,
}

impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "aglibot-evolution-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), ORIGINAL).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"aglibot\"\nversion=\"0.1.0\"\nedition=\"2024\"\n").unwrap();
        std::fs::write(root.join(".gitignore"), "/target\n/data\n/.moldloft\n").unwrap();
        assert!(run(&root, "cargo", &["generate-lockfile", "--offline"], &[], 30).await.unwrap().ok);
        git(&root, &["init", "-q"], &[]).await.unwrap();
        git(&root, &["add", "."], &[]).await.unwrap();
        git(&root, &["-c", "user.name=fixture", "-c", "user.email=fixture@localhost", "commit", "-qm", "base"], &[]).await.unwrap();
        let base = oid(&root, &["rev-parse", "HEAD"]).await.unwrap();
        std::fs::write(root.join("src/main.rs"), UPDATED).unwrap();
        let patch = git(&root, &["diff", "--binary", "--no-ext-diff", "--no-textconv", "--", "src/main.rs"], &[]).await.unwrap();
        git(&root, &["checkout", "--", "src/main.rs"], &[]).await.unwrap();
        // Production layout: the runtime root is a separate, non-git directory (ignored here).
        let runtime = root.join("data/runtime");
        let store = Arc::new(crate::store::jsonl::JsonlStore::open(&runtime.join("data")).unwrap());
        let gate = Gate { source: root.clone(), root: runtime.clone(), dir: runtime.join("data/evolution"), store };
        let request = Request {
            source: Source { drydock: "default".into(), ticket: Some(26), proposal: None },
            summary: "fixture code change".into(),
            base_commit: base,
            writer_model: "gpt-6-astra".into(),
            writer_host: "codex".into(),
            files: vec!["src/main.rs".into()],
            eval_tests: vec!["eval_positive".into()],
            director: None,
        };
        let source = Arc::new(Mutex::new(json!({"drydock_id":"default", "tickets":[{
            "id":26, "status":"open", "version":1, "blockers":[], "title":"fixture", "description":"change main", "tech_notes":"test"}]})));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        std::fs::create_dir_all(root.join(".moldloft")).unwrap();
        std::fs::write(root.join(".moldloft/project.json"), json!({"port":listener.local_addr().unwrap().port()}).to_string()).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let body = source.clone();
        let server = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                        let mut buf = [0; 8192];
                        let _ = stream.read(&mut buf);
                        let response = body.lock().unwrap().to_string();
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            response.len(),
                            response
                        );
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        Self { gate, request, patch, source, stop, server: Some(server) }
    }
    fn inbox(&self, id: &str, request: &Request, patch: &[u8]) {
        let dir = self.gate.dir.join("inbox").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("proposal.json"), serde_json::to_vec(request).unwrap()).unwrap();
        std::fs::write(dir.join("change.patch"), patch).unwrap();
    }
    fn models(&self, verdict: &str) -> Models {
        Models::stub(self.gate.store.clone(), vec![json!({"verdict":verdict,"reasons":"fixture review","fixes":""}).to_string()])
    }
    fn approve(&self, id: &str) {
        let r = self.gate.load(id).unwrap();
        let mut item = self.gate.store.reviews().unwrap().into_iter().find(|i| Some(i.id) == r.review_id).unwrap();
        self.gate.approve(&item, "owner").unwrap();
        item.status = ReviewStatus::Approved;
        self.gate.store.save_review(&item).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            server.join().unwrap();
        }
        assert!(self.gate.source.parent() == Some(std::env::temp_dir().as_path()));
        if std::thread::panicking() {
            eprintln!("preserved failing fixture: {}", self.gate.source.display());
            return;
        }
        let _ = std::fs::remove_dir_all(&self.gate.source);
    }
}

#[test]
fn path_policy_rejects_l4_private_aliases_and_traversal() {
    for path in [
        "config/runtime.yaml",
        "mind/self/boundaries.md",
        "src/evolution/gate.rs",
        "src/Evolution/process.rs",
        "src/evolution.rs",
        "src/agli/evolution.rs",
        "AGENTS.md",
        ".env",
        ".git/config",
        "src/../config/a",
        "src/./a",
        "src\\main.rs",
        "C:/src/a.rs",
        "/src/a.rs",
        "src/a:secret",
        "src/nul.rs",
        "src/a. /b.rs",
        "src/.hidden",
        "src/com1.rs",
    ] {
        assert!(!allowed_path(path), "{path}");
    }
    for path in ["src/main.rs", "src/social/privacy.rs", "tests/eval.rs", "docs/CHANGE.md", "Cargo.toml", "Cargo.lock"] {
        assert!(allowed_path(path), "{path}");
    }
    for id in ["../a", "CON", "nul", "", "a/b", "a\\b"] {
        assert!(!valid_id(id));
    }
}

#[tokio::test]
async fn writer_and_director_validation_precedes_worktree_and_build() {
    let f = Fixture::new().await;
    let mut r = f.request.clone();
    for (model, host) in [("gpt-6-sol", "codex"), ("gpt-6-astra", "dsh"), ("opus", "codex"), ("deepseek-v4.1-flash", "dsh")] {
        r.writer_model = model.into();
        r.writer_host = host.into();
        f.inbox("denied", &r, &f.patch);
        assert!(f.gate.prepare("denied", "owner", &f.models("pass")).await.is_err());
        assert!(!f.gate.worktree("denied").exists());
    }
    r.writer_model = "deepseek-v4.1-flash-1017".into();
    r.writer_host = "dsh".into();
    assert!(validate(&r, &f.patch).is_err());
    r.director = Some(Direction {
        model: "opus".into(),
        plan: "Change only the main greeting".into(),
        interfaces: vec!["main()".into()],
        reviewed_patch_sha256: hash(&f.patch),
        review: "Reviewed exact diff; scope and eval are adequate".into(),
    });
    validate(&r, &f.patch).unwrap();
    r.director.as_mut().unwrap().reviewed_patch_sha256 = "0".repeat(64);
    assert!(validate(&r, &f.patch).is_err());
    r = f.request.clone();
    r.writer_model = "opus".into();
    r.writer_host = "claude-code".into();
    validate(&r, &f.patch).unwrap();
}

#[tokio::test]
async fn full_pipeline_requires_owner_then_commits_exact_tree_and_prepares_restart() {
    let f = Fixture::new().await;
    f.inbox("happy", &f.request, &f.patch);
    let models = f.models("pass");
    let result = f.gate.prepare("happy", "owner", &models).await.unwrap();
    assert!(result.restart.is_none());
    let mut r = f.gate.load("happy").unwrap();
    assert_eq!(r.state, State::AwaitingOwner);
    assert_eq!(models.stub_prompts()[0].0, "evolve");
    assert_eq!(r.checks.len(), 4);
    assert!(f.gate.commit(&mut r).await.is_err());
    let item = f.gate.store.reviews().unwrap().pop().unwrap();
    assert!(f.gate.approve(&item, "stranger").is_err());
    f.approve("happy");
    let result = f.gate.finish("happy").await.unwrap();
    let restart = result.restart.unwrap();
    let r = f.gate.load("happy").unwrap();
    assert_eq!(r.state, State::RestartRequested);
    assert_eq!(restart.executable_sha256, hash(&std::fs::read(&restart.executable).unwrap()));
    assert_eq!(oid(&f.gate.source, &["rev-parse", "HEAD"]).await.unwrap(), f.request.base_commit);
    assert_eq!(std::fs::read_to_string(f.gate.source.join("src/main.rs")).unwrap(), ORIGINAL);
    assert_eq!(oid(&f.gate.source, &["rev-parse", &format!("{}^{{tree}}", restart.commit)]).await.unwrap(), r.tree);
    assert_eq!(oid(&f.gate.source, &["rev-parse", "refs/heads/codex/evolution-happy"]).await.unwrap(), restart.commit);
    // Commits land in the source repo; the release and the restart belong to the runtime root.
    assert!(restart.executable.starts_with(f.gate.root.canonicalize().unwrap()));
    assert_eq!(restart.root, f.gate.root.canonicalize().unwrap());
    crate::evolution::restart::record_release(f.gate.dir.parent().unwrap(), &restart.commit, &restart.executable, "evolution:happy").unwrap();
    let release: Value = serde_json::from_slice(&std::fs::read(f.gate.root.join("data/release.json")).unwrap()).unwrap();
    assert_eq!((release["commit"].as_str(), release["origin"].as_str()), (Some(restart.commit.as_str()), Some("evolution:happy")));
    assert_eq!(release["exe_sha256"], restart.executable_sha256);
    assert!(f.gate.finish("happy").await.is_err()); // no duplicate commit/restart
    assert!(f.gate.prepare("happy", "owner", &f.models("pass")).await.is_err());

    // The handoff launches only this harmless fixture binary, never the real bot.
    let start_parent = || {
        let mut cmd = std::process::Command::new(&restart.executable);
        cmd.arg("wait");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        cmd.spawn().unwrap()
    };
    let mut parent = start_parent();
    let request = restart.receipt.with_file_name("fixture-restart.json");
    std::fs::write(&request, serde_json::to_vec(&restart).unwrap()).unwrap();
    crate::evolution::restart::helper(parent.id(), request.clone()).unwrap();
    assert!(parent.try_wait().unwrap().is_some());
    for _ in 0..80 {
        if f.gate.root.join("started-marker").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(std::fs::read_to_string(f.gate.root.join("started-marker")).unwrap(), "new");
    std::fs::remove_file(f.gate.root.join("started-marker")).unwrap();
    let mut tampered = restart.clone();
    tampered.executable_sha256 = "0".repeat(64);
    std::fs::write(&request, serde_json::to_vec(&tampered).unwrap()).unwrap();
    let mut parent = start_parent();
    assert!(crate::evolution::restart::helper(parent.id(), request).is_err());
    parent.wait().unwrap();
    assert!(!f.gate.root.join("started-marker").exists());
    // Timeout terminates both the checking process and its delayed descendant.
    assert!(run(&f.gate.source, restart.executable.to_str().unwrap(), &["fork"], &[], 1).await.is_err());
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert!(!f.gate.source.join("late-marker").exists());
}

#[tokio::test]
async fn falsified_scope_cannot_write_l4_and_symlinks_are_rejected_before_build() {
    let f = Fixture::new().await;
    let mut patch = f.patch.clone();
    patch.extend_from_slice(b"diff --git a/config/forbidden.yaml b/config/forbidden.yaml\nnew file mode 100644\n--- /dev/null\n+++ b/config/forbidden.yaml\n@@ -0,0 +1 @@\n+forbidden: true\n");
    f.inbox("scope", &f.request, &patch);
    assert!(f.gate.prepare("scope", "owner", &f.models("pass")).await.is_err());
    assert!(!f.gate.worktree("scope").join("config/forbidden.yaml").exists());
    assert!(!f.gate.dir.join("logs/scope").exists());
    let mut r = f.request.clone();
    r.files = vec!["src/link.rs".into()];
    let symlink = b"diff --git a/src/link.rs b/src/link.rs\nnew file mode 120000\n--- /dev/null\n+++ b/src/link.rs\n@@ -0,0 +1 @@\n+../../config/runtime.yaml\n";
    f.inbox("symlink", &r, symlink);
    assert!(f.gate.prepare("symlink", "owner", &f.models("pass")).await.is_err());
    assert!(!f.gate.worktree("symlink").join("src/link.rs").exists());
}

#[tokio::test]
async fn changed_candidate_or_moldloft_source_invalidates_approval() {
    let f = Fixture::new().await;
    f.inbox("modified", &f.request, &f.patch);
    f.gate.prepare("modified", "owner", &f.models("pass")).await.unwrap();
    f.approve("modified");
    std::fs::write(f.gate.worktree("modified").join("src/main.rs"), ORIGINAL).unwrap();
    assert!(f.gate.finish("modified").await.is_err());
    assert_eq!(f.gate.load("modified").unwrap().state, State::Failed);
    assert!(f.gate.load("modified").unwrap().commit.is_none());
    f.inbox("stale", &f.request, &f.patch);
    f.gate.prepare("stale", "owner", &f.models("pass")).await.unwrap();
    f.approve("stale");
    f.source.lock().unwrap()["tickets"][0]["version"] = json!(2);
    assert!(f.gate.finish("stale").await.is_err());
    assert!(f.gate.load("stale").unwrap().commit.is_none());
}

#[tokio::test]
async fn absent_eval_failed_build_and_judge_rejection_never_become_approvable() {
    let f = Fixture::new().await;
    let mut r = f.request.clone();
    r.eval_tests = vec!["missing_test".into()];
    f.inbox("missing", &r, &f.patch);
    assert!(f.gate.prepare("missing", "owner", &f.models("pass")).await.unwrap_err().to_string().contains("eval test not found"));
    assert!(f.gate.store.reviews().unwrap().is_empty());
    let broken = String::from_utf8(f.patch.clone()).unwrap().replace("println!(\"new\");", "not valid rust;");
    f.inbox("broken", &f.request, broken.as_bytes());
    assert!(f.gate.prepare("broken", "owner", &f.models("pass")).await.is_err());
    assert!(f.gate.store.reviews().unwrap().is_empty());
    f.inbox("revise", &f.request, &f.patch);
    f.gate.prepare("revise", "owner", &f.models("revise")).await.unwrap();
    let item = f.gate.store.reviews().unwrap().pop().unwrap();
    assert!(f.gate.approve(&item, "owner").is_err());
    f.gate.reject(&item).unwrap();
    assert_eq!(f.gate.load("revise").unwrap().state, State::Rejected);
}

#[test]
fn sources_require_real_unblocked_ticket_or_applied_proposal() {
    let ticket = Source { drydock: "default".into(), ticket: Some(26), proposal: None };
    let mut graph = json!({"drydock_id":"default","tickets":[{"id":26,"blockers":[25],"status":"open"}]});
    assert!(source_snapshot(&ticket, graph.clone()).is_err());
    graph["tickets"][0]["blockers"] = json!([]);
    assert!(source_snapshot(&ticket, graph.clone()).is_ok());
    graph["drydock_id"] = json!("other");
    assert!(source_snapshot(&ticket, graph).is_err());
    let proposal = Source { drydock: "default".into(), ticket: None, proposal: Some("abc-123".into()) };
    assert!(source_snapshot(&proposal, json!({"proposal":{"id":"abc-123","status":"pending"}})).is_err());
    assert!(source_snapshot(&proposal, json!({"proposal":{"id":"abc-123","status":"applied"}})).is_ok());
}
