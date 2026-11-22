use super::*;
use crate::{
    control::CancelSource,
    hands::reader::FetchHttpError,
    mind::models::Models,
    store::Store,
    work::{self, Board, TaskKind, TaskStatus, ThreadRef},
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

fn plan_reply(questions: &[&str]) -> String {
    json!({
        "summary": "计划",
        "sub_questions": ["证据是什么？"],
        "search_queries": ["first", "first", "second"],
        "clarification_questions": questions,
    })
    .to_string()
}

async fn job(name: &str, replies: Vec<String>) -> (Job, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("agli-native-{name}-{}", std::process::id()));
    let store = Store::memory();
    let board = Board::open(&dir, store.clone()).await.unwrap();
    let task = board
        .add(
            "原任务",
            TaskKind::Research,
            "owner",
            TaskStatus::Active,
            None,
        )
        .await
        .unwrap();
    let hands =
        serde_yaml::from_str("research: native\ncode: off\ncodex_model: stub\ntimeout_min: 1")
            .unwrap();
    (
        Job {
            models: Arc::new(Models::stub(store.clone(), replies)),
            store,
            board,
            l1: String::new(),
            task,
            hands,
            tools: None,
        },
        dir,
    )
}

#[derive(Default)]
struct MockTools {
    fetch_count: AtomicUsize,
    search_count: AtomicUsize,
    late: Option<(Board, String)>,
    blocked: Option<Arc<Notify>>,
}

impl ResearchTools for MockTools {
    fn search<'a>(
        &'a self,
        _query: &'a str,
        _count: usize,
        _question: Option<&'a str>,
        _need_content: bool,
    ) -> ToolFuture<'a, Vec<SearchHit>> {
        Box::pin(async {
            self.search_count.fetch_add(1, Ordering::Relaxed);
            Ok(["https://example.org/ok", "https://example.org/missing"]
                .iter()
                .map(|url| SearchHit {
                    url: (*url).into(),
                    title: "candidate".into(),
                    snippet: String::new(),
                    summary: String::new(),
                    content: String::new(),
                    rerank_score: None,
                    authority: None,
                })
                .collect())
        })
    }

    fn fetch_page<'a>(&'a self, url: &'a str) -> ToolFuture<'a, FetchPage> {
        Box::pin(async move {
            self.fetch_count.fetch_add(1, Ordering::Relaxed);
            if let Some(blocked) = &self.blocked {
                blocked.notify_one();
                std::future::pending::<()>().await;
            }
            if url.ends_with("missing") {
                return Err(FetchHttpError {
                    status: 404,
                    message: "missing".into(),
                }
                .into());
            }
            if let Some((board, id)) = &self.late {
                board
                    .follow_up(
                        id,
                        "新增限制",
                        ThreadRef {
                            platform: "qq".into(),
                            chat_id: "private:1".into(),
                            message_id: None,
                        },
                    )
                    .await?;
            }
            Ok(FetchPage {
                url: url.into(),
                title: "retrieved".into(),
                content: "可核实正文".into(),
                http_status: 200,
            })
        })
    }
}

const PASS: &str = r#"{"verdict":"pass","reasons":"核实"}"#;

#[tokio::test]
async fn revision_reuses_verified_source_and_flags_404_explicitly() {
    let (mut job, root) = job(
        "revision",
        vec![
            plan_reply(&[]),
            "初稿".into(),
            r#"{"verdict":"revise","reasons":"有缺漏","fixes":"补充限制"}"#.into(),
            "第二稿".into(),
            PASS.into(),
        ],
    )
    .await;
    let tools = Arc::new(MockTools::default());
    job.tools = Some(tools.clone());
    let dir = job.dir();
    let models = job.models.clone();
    let store = job.store.clone();
    let outcome = work::run(job, CancelSource::new().1).await;
    assert_eq!(outcome.status(), TaskStatus::Done, "{outcome:?}");
    assert_eq!(outcome.verdict_record.unwrap().round, 2);
    assert_eq!(tools.search_count.load(Ordering::Relaxed), 4);
    assert_eq!(tools.fetch_count.load(Ordering::Relaxed), 3);
    let missing = std::fs::read_to_string(dir.join("sources/02.md")).unwrap();
    assert!(missing.contains("HTTP_STATUS: 404\nFETCHED: false"));
    assert!(missing.contains("RETRIEVED_AT:") && missing.contains("TITLE: candidate"));
    let prompts = models.stub_prompts();
    assert!(prompts[3].2.contains("补充限制") && prompts[3].2.contains("HTTP_STATUS: 404"));
    let reviews = store.reviews().await.unwrap();
    let evidence = reviews[1].payload["evidence"].as_str().unwrap();
    assert!(evidence.contains("fetched_ok=1, fetched_failed=2"));
    assert!(evidence.contains("第二稿") && evidence.contains("可核实正文"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn clarification_saves_plan_without_tools_or_review() {
    let (job, root) = job("clarify", vec![plan_reply(&["研究 A 还是 B？"])]).await;
    let dir = job.dir();
    let store = job.store.clone();
    let outcome = work::run(job, CancelSource::new().1).await;
    assert!(outcome.ok);
    assert_eq!(outcome.clarification.as_deref(), Some("研究 A 还是 B？"));
    assert!(store.reviews().await.unwrap().is_empty());
    assert!(dir.join("plan.md").is_file());
    assert!(!dir.join("report.md").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn followup_during_fetch_is_planned_and_consumed_in_next_round() {
    let (mut job, root) = job(
        "late",
        vec![
            plan_reply(&[]),
            "初稿".into(),
            PASS.into(),
            plan_reply(&[]),
            "改稿".into(),
            PASS.into(),
        ],
    )
    .await;
    let board = job.board.clone();
    let id = job.task.id.clone();
    let models = job.models.clone();
    let store = job.store.clone();
    job.tools = Some(Arc::new(MockTools {
        late: Some((board.clone(), id.clone())),
        ..MockTools::default()
    }));
    let outcome = work::run(job, CancelSource::new().1).await;
    assert_eq!(outcome.status(), TaskStatus::Done);
    let saved = board.get(&id).await.unwrap().unwrap();
    assert!(saved.unconsumed_followups().is_empty());
    assert_eq!(saved.followups[0].consumed_round, Some(2));
    let prompts = models.stub_prompts();
    assert!(!prompts[1].2.contains("新增限制"));
    assert_eq!(prompts[3].0, "research_plan");
    assert!(prompts[3].2.contains("新增限制"));
    let reviews = store.reviews().await.unwrap();
    assert_eq!(reviews[0].payload["reviewed_followup_seq"], 0);
    assert_eq!(reviews[1].payload["reviewed_followup_seq"], 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn cancellation_stops_inflight_fetch_and_leaves_attempt_receipts() {
    let (mut job, root) = job("cancel", vec![plan_reply(&[])]).await;
    let blocked = Arc::new(Notify::new());
    job.tools = Some(Arc::new(MockTools {
        blocked: Some(blocked.clone()),
        ..MockTools::default()
    }));
    let dir = job.dir();
    let store = job.store.clone();
    let (source, token) = CancelSource::new();
    let handle = tokio::spawn(work::run(job, token));
    tokio::time::timeout(std::time::Duration::from_secs(2), blocked.notified())
        .await
        .unwrap();
    source.cancel();
    let outcome = tokio::time::timeout(std::time::Duration::from_millis(250), handle)
        .await
        .unwrap()
        .unwrap();
    assert!(outcome.ok && outcome.paused.is_some());
    assert!(store.reviews().await.unwrap().is_empty());
    let started = std::fs::read_to_string(dir.join("sources/01.md")).unwrap();
    assert!(started.contains("FETCHED: false") && started.contains("下载未完成"));
    std::fs::remove_dir_all(root).unwrap();
}
