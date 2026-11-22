mod artifacts;
pub mod board;
pub(crate) mod coding;
pub mod research;
mod resume;

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use serde_json::json;

use crate::config::HandsCfg;
use crate::control::CancelToken;
use crate::mind::{
    judge::{self, Verdict},
    models::Models,
};
use crate::store::{ReviewCategory, ReviewItem, ReviewStatus, Store};
use research::ResearchTools;

pub(crate) use artifacts::linked;
pub use board::{Board, Task, TaskFollowup, TaskKind, TaskStatus, TaskVerdict, ThreadRef};

const MAX_ROUNDS: u32 = 3;

pub struct Job {
    pub models: Arc<Models>,
    pub store: Store,
    pub board: Board,
    pub l1: String,
    pub hands: HandsCfg,
    pub task: Task,
    pub tools: Option<Arc<dyn ResearchTools>>,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub task_id: String,
    pub title: String,
    pub ok: bool,
    pub clarification: Option<String>,
    pub paused: Option<String>,
    pub summary: String,
    pub verdict: Option<String>,
    pub verdict_record: Option<TaskVerdict>,
}

impl Outcome {
    pub fn status(&self) -> TaskStatus {
        if self.ok && self.clarification.is_none() && self.paused.is_none() {
            TaskStatus::Done
        } else {
            TaskStatus::Waiting
        }
    }
}

impl Job {
    fn dir(&self) -> PathBuf {
        self.board.task_dir(&self.task.id)
    }

    fn log(&self, line: &str) {
        if let Err(e) = self.board.log(&self.task.id, line) {
            tracing::warn!("task log failed: {e:#}");
        }
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(self.hands.timeout_min.saturating_mul(60))
    }

    fn outcome(&self, ok: bool, summary: String) -> Outcome {
        Outcome {
            task_id: self.task.id.clone(),
            title: self.task.title.clone(),
            ok,
            summary,
            clarification: None,
            paused: None,
            verdict: None,
            verdict_record: None,
        }
    }

    async fn current_task(&self) -> Result<Task> {
        self.board.get(&self.task.id).await?.context("任务已不存在")
    }
}

pub async fn run(job: Job, cancel: CancelToken) -> Outcome {
    let mut stopping = cancel.clone();
    let work = async {
        let result = match job.task.kind {
            TaskKind::Research => research::run(&job, cancel.clone()).await,
            TaskKind::Code => coding::run(&job, cancel.clone()).await,
        };
        match result {
            Err(error) if error.is::<resume::Uncertain>() => {
                waiting_for_owner(&job, &error.to_string()).await
            }
            result => result,
        }
    };
    // The inner Codex runner also owns a cancel token. Dropping the work future
    // here drops its attached process tree before returning to the job registry.
    let result = tokio::select! {
        biased;
        () = stopping.cancelled() => None,
        result = work => Some(result),
    };
    match result {
        Some(Ok(outcome)) => outcome,
        Some(Err(e)) if !cancel.is_cancelled() => {
            crate::error_report::report(crate::error_report::Fault::TaskExecution);
            job.log(&format!("失败：{e:#}"));
            job.outcome(false, format!("任务没做完，卡在：{e:#}"))
        }
        _ => {
            let reason = "任务已停止，已保存断点。";
            job.log(reason);
            Outcome {
                paused: Some(reason.into()),
                ..job.outcome(true, reason.into())
            }
        }
    }
}

async fn waiting_for_owner(job: &Job, reason: &str) -> Result<Outcome> {
    job.log(reason);
    let j = judge::Judgement {
        verdict: Verdict::Owner,
        reasons: reason.into(),
        fixes: String::new(),
        urgent: true,
    };
    let item = judge::file(
        &job.store,
        ReviewCategory::Act,
        &job.task.title,
        reason,
        &j,
        false,
        json!({"task_id": job.task.id}),
    )
    .await?;
    Ok(Outcome {
        paused: Some(reason.into()),
        verdict: Some(format!("#{} {}", item.id, j.verdict.zh())),
        ..job.outcome(false, reason.into())
    })
}

fn followup_subject(task: &Task) -> String {
    let mut subject = task.title.clone();
    if !task.followups.is_empty() {
        subject.push_str(
            "

主人补充（按顺序，后面的要求更新前面的要求）：",
        );
        for f in &task.followups {
            let state = if f.consumed_round.is_none() {
                "新补充"
            } else {
                "已审查"
            };
            subject.push_str(&format!("\n- #{} {state}：{}", f.seq, f.body));
        }
    }
    subject
}

async fn review_round(
    job: &Job,
    category: ReviewCategory,
    subject: &str,
    evidence: &str,
    round: u32,
    max_seq: u64,
) -> Result<(judge::Judgement, ReviewItem)> {
    let j = judge::review_work(&job.models, category, subject, evidence).await?;
    let detail = format!(
        "任务 {} 第 {round} 轮，产物目录 {}。\n{}\n{}",
        job.task.id,
        job.dir().display(),
        j.reasons,
        j.fixes
    );
    let item = judge::file(&job.store, category, &job.task.title, &detail, &j, false,
        json!({"task_id": job.task.id, "work_round": round, "fixes": j.fixes, "evidence": evidence, "reviewed_followup_seq": max_seq})).await?;
    job.board
        .mark_followups_consumed(&job.task.id, max_seq, round)
        .await?;
    job.log(&format!(
        "第 {round} 轮审查 #{}：{}，{}；修改意见：{}",
        item.id,
        j.verdict.zh(),
        j.reasons,
        j.fixes
    ));
    Ok((j, item))
}

async fn previous_revision(job: &Job) -> Result<(u32, String)> {
    let previous = job
        .store
        .reviews()
        .await?
        .into_iter()
        .filter(|item| {
            item.payload["task_id"].as_str() == Some(&job.task.id)
                && item.payload["work_round"].is_number()
        })
        .max_by_key(|item| item.id);
    Ok(match previous {
        Some(item) => {
            let round = item.payload["work_round"].as_u64().unwrap_or_default();
            let fixes = if item.verdict.starts_with(Verdict::Pass.zh()) {
                String::new()
            } else {
                format!(
                    "上次审查 #{}：{}\n{}",
                    item.id,
                    item.verdict,
                    item.payload["fixes"].as_str().unwrap_or_default()
                )
            };
            (u32::try_from(round)?, fixes)
        }
        None => (0, String::new()),
    })
}

async fn reviewed_outcome(
    job: &Job,
    summary: String,
    j: judge::Judgement,
    mut item: ReviewItem,
    round: u32,
) -> Result<Outcome> {
    if matches!(j.verdict, Verdict::Revise | Verdict::Defer) {
        item.status = ReviewStatus::Open;
        job.store.save_review(&item).await?;
    }
    let paused = (j.verdict != Verdict::Pass).then(|| {
        format!(
            "{}：{}。{} 可补充材料后用 /run {} 继续。",
            j.verdict.zh(),
            j.reasons,
            j.fixes,
            job.task.id,
        )
    });
    let verdict = j.verdict.zh().to_owned();
    Ok(Outcome {
        paused,
        verdict: Some(format!("#{} {verdict}", item.id)),
        verdict_record: Some(TaskVerdict {
            review_id: item.id,
            round,
            verdict,
            reasons: j.reasons,
            fixes: j.fixes,
        }),
        ..job.outcome(true, summary)
    })
}

async fn another_round(job: &Job, verdict: Verdict, attempt: u32) -> Result<bool> {
    if attempt >= MAX_ROUNDS {
        return Ok(false);
    }
    Ok(match verdict {
        Verdict::Revise => true,
        Verdict::Pass => !job.current_task().await?.unconsumed_followups().is_empty(),
        Verdict::Defer | Verdict::Owner => false,
    })
}
