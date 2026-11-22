use std::{future::Future, path::PathBuf};

use anyhow::Result;

use super::{
    Job, MAX_ROUNDS, Outcome, another_round,
    artifacts::{artifact_evidence, supplied_material},
    followup_subject, linked, previous_revision, resume, review_round, reviewed_outcome,
};
use crate::{config::DelegateTool, control::CancelToken, hands::codex, store::ReviewCategory};

pub(super) async fn run(job: &Job, cancel: CancelToken) -> Result<Outcome> {
    anyhow::ensure!(
        job.hands.code == DelegateTool::Codex,
        "编码委托需要将 config/runtime.yaml 的 hands.code 设为 codex"
    );
    code_rounds(job, |dir, prompt| {
        let cancel = cancel.clone();
        async move {
            codex::codex(
                &dir,
                &prompt,
                &job.hands.codex_model,
                false,
                job.timeout(),
                cancel,
            )
            .await
        }
    })
    .await
}

pub(crate) async fn code_rounds<F: Future<Output = Result<String>>>(
    job: &Job,
    mut execute: impl FnMut(PathBuf, String) -> F,
) -> Result<Outcome> {
    let task_dir = std::path::absolute(job.dir())?;
    std::fs::create_dir_all(&task_dir)?;
    anyhow::ensure!(!linked(&task_dir.symlink_metadata()?), "任务目录包含链接");
    let dir = task_dir.join("scratch");
    std::fs::create_dir_all(&dir)?;
    anyhow::ensure!(!linked(&dir.symlink_metadata()?), "scratch 目录包含链接");
    let (last_reviewed, mut fixes) = previous_revision(job).await?;
    let start = last_reviewed.max(job.current_task().await?.current_round);
    for attempt in 1..=MAX_ROUNDS {
        let round = start
            .checked_add(attempt)
            .ok_or_else(|| anyhow::anyhow!("任务轮次超出限制"))?;
        let task = job.current_task().await?;
        let subject = followup_subject(&task);
        let snapshot = resume::inspect(&task_dir, &task, &job.store).await?;
        let materials = supplied_material(&task_dir)?;
        let prompt = format!(
            "{}任务：{subject}\n任务资料目录（只读）：{}\n{materials}\n只在当前 scratch 目录里工作。在已有产物上继续，验证成果并说明检查结果与未解决项。\n审稿意见：{fixes}",
            snapshot.brief(),
            task_dir.display(),
        );
        job.log(&format!("第 {round} 轮调用 Codex"));
        let text = execute(dir.clone(), prompt).await?;
        resume::check_reply(&text)?;
        let evidence = format!(
            "{materials}\n任务产出的文件与摘录：{}\n\n执行者的说明：\n{text}",
            artifact_evidence(&dir)?
        );
        let (j, review) = review_round(
            job,
            ReviewCategory::Act,
            &subject,
            &evidence,
            round,
            task.max_followup_seq(),
        )
        .await?;
        if !another_round(job, j.verdict, attempt).await? {
            let summary = format!("{}\n产出在：{}", text.trim(), dir.display());
            return reviewed_outcome(job, summary, j, review, round).await;
        }
        fixes = j.fixes;
    }
    unreachable!("the last coding round always returns")
}
