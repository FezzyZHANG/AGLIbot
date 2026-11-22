use anyhow::{Context, Result};
use serde_json::json;
#[cfg(test)]
use tokio::time::Instant;
use tracing::{info, warn};

use super::{Bot, ingress::ControlEvent};
use crate::error_report::Fault;
use crate::mind::{judge, reflect};
use crate::store::{Outcome, ReviewCategory};
use crate::work::{board::{TaskKind, TaskStatus}, research::{self, Job}};

pub(super) fn delivery_preview(text: &str) -> String {
    const LIMIT: usize = 600;
    if text.chars().count() <= LIMIT { return text.to_string() }
    let mut preview: String = text.chars().take(LIMIT - 1).collect();
    preview.push('…');
    preview
}

impl Bot {
    pub(super) async fn start_job(&mut self, task_id: &str) -> Result<String> {
        self.start_job_with_owner_slot(task_id, false).await
    }

    pub(super) async fn start_job_with_owner_slot(&mut self, task_id: &str, owner_run: bool) -> Result<String> {
        let control = self.control.clone();
        control.admit_job(async || self.start_admitted_job(task_id, owner_run).await).await
            .unwrap_or_else(|| Ok("冻结中，不开始任务。".into()))
    }

    async fn start_admitted_job(&mut self, task_id: &str, owner_run: bool) -> Result<String> {
        let jobs = self.control.jobs();
        let max = self.cfg.runtime.hands.max_jobs;
        // Keep the default single-worker command behavior exactly as before.
        if max == 1 && let Some(running) = jobs.ids().first() {
            return Ok(format!("正在做 {}，一次只做一件。", running));
        }
        if jobs.contains(task_id) {
            return Ok(format!("正在做 {task_id}，不能重复开始。"));
        }
        let limit = max.saturating_add(usize::from(owner_run && max > 1));
        if jobs.len() >= limit {
            return Ok(format!("任务槽位已满，正在做：{}。", self.running_jobs()));
        }
        let task = self.board().get(task_id).cloned().ok_or_else(|| anyhow::anyhow!("没有任务 {task_id}"))?;
        if task.status != TaskStatus::Inbox && task.status != TaskStatus::Waiting {
            return Ok(format!("{task_id} 的状态是 {:?}，不能开始。", task.status));
        }
        self.board().set_status(&task.id, TaskStatus::Active, "")?;
        self.audit(&task.requester, &format!("delegate:{:?}", task.kind), true, &task.title, Outcome::Unknown).await?;
        let job = Job {
            models: self.models.clone(),
            store: self.store.clone(),
            board: self.board.clone(),
            l1: self.speaker.l1().to_string(),
            hands: self.cfg.runtime.hands.clone(),
            task: task.clone(),
        };
        let log_path = self.board().task_dir(&task.id).join("log.md");
        jobs.spawn(task.id.clone(), task.title.clone(), log_path, self.events.clone(), |_| research::run(job));
        info!("started task {}", task.id);
        Ok(format!("开始做：{}", task.title))
    }

    pub(super) async fn on_job_done(&mut self, o: research::Outcome) -> Result<()> {
        let Some(started) = self.control.jobs().complete(&o.task_id) else { return Ok(()) };
        self.finish_job(o, started.elapsed().as_secs() / 60).await
    }

    async fn finish_job(&mut self, mut o: research::Outcome, elapsed_minutes: u64) -> Result<()> {
        let mut status = o.status();
        if status == TaskStatus::Done
            && let Some(review_id) = o.verdict.as_deref().and_then(|s| s.split_whitespace().next()).and_then(|s| s.strip_prefix('#')).and_then(|s| s.parse::<u64>().ok())
            && let Some(review) = self.store.reviews().await?.into_iter().find(|r| r.id == review_id && r.payload["task_id"].as_str() == Some(o.task_id.as_str()))
            && let Some(reviewed) = review.payload["reviewed_followups_bytes"].as_u64()
        {
            let path = self.board().task_dir(&o.task_id).join("followups.md");
            let length = match std::fs::metadata(path) {
                Ok(meta) => meta.len(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
                Err(e) => return Err(e.into()),
            };
            if length != reviewed {
                status = TaskStatus::Inbox;
                o.paused = Some("审查后收到了新的补充，已排入下一轮。".into());
                o.summary = o.paused.clone().unwrap_or_default();
            } else {
                judge::settle_work_revisions(&self.store, &o.task_id, &review).await?;
            }
        }
        let note = if let Some(reason) = &o.paused {
            reason.as_str()
        } else if o.clarification.is_some() { "等待主人澄清" } else if o.ok { "" } else { "出错，等主人看" };
        self.board().set_status(&o.task_id, status, note)?;
        self.audit("owner", "delegate_result", true, &o.task_id, if o.ok { Outcome::Sent } else { Outcome::Failed }).await?;
        self.state.finished_work();
        reflect::note_today(&self.store, &format!("做了 {elapsed_minutes} 分钟：{}（{}）", o.title, if o.status() == TaskStatus::Done { "完成" } else { "没做完" })).await?;
        self.push_urgent().await?;
        // A watchdog pause has its own persisted review alert, including the reason and id.
        if o.ok || o.paused.is_none() || o.verdict.is_none() {
            if self.state.energy.asleep {
                self.deliveries.push(o);
            } else {
                self.deliver(o).await?;
            }
        }
        self.wake_decide = true;
        Ok(())
    }

    pub(super) fn running_jobs(&self) -> String { self.control.jobs().summary() }

    #[cfg(test)]
    async fn stop_job_by_id(&self, id: &str) -> Option<String> {
        self.control.jobs().stop_one(id).await
    }

    pub(super) async fn stop_for_freeze(&mut self) -> Result<()> {
        let stopped = self.stop_jobs().await;
        self.on_control_event(ControlEvent::Frozen { stopped })
    }

    pub(super) async fn stop_jobs(&self) -> Vec<String> { self.control.jobs().stop_all().await }

    #[cfg(test)]
    pub(super) async fn stop_job(&self) -> Option<String> {
        let id = self.control.jobs().ids().into_iter().next()?;
        self.stop_job_by_id(&id).await
    }

    pub(super) fn on_control_event(&mut self, event: ControlEvent) -> Result<()> {
        let (stopped, reason) = match event {
            ControlEvent::Frozen { stopped } => (stopped, "冻结"),
            ControlEvent::Stopped { stopped } => (stopped, "主人让我停下"),
        };
        self.deliveries.retain(|outcome| !stopped.contains(&outcome.task_id));
        let mut result = Ok(());
        for id in stopped {
            if let Err(e) = self.board().set_status(&id, TaskStatus::Waiting, reason) {
                warn!(task_id = id, "saving stopped task: {e:#}");
                result = Err(e.context(Fault::StateSave));
            }
        }
        result
    }

    pub(super) async fn on_job_watchdog(&mut self, id: String, reason: String) -> Result<()> {
        let task = self.board().get(&id).cloned().ok_or_else(|| anyhow::anyhow!("没有任务 {id}"))?;
        let j = judge::Judgement { verdict: judge::Verdict::Owner, reasons: reason.clone(), fixes: String::new(), urgent: true };
        let detail = format!("{reason}。已保存断点，任务尚未完成；可用 /run {id} 继续。");
        let item = match judge::file(&self.store, ReviewCategory::Act, &task.title, &detail, &j, false,
            json!({"task_id": id, "watchdog": true})).await {
            Ok(item) => item,
            Err(e) => {
                self.board().set_status(&id, TaskStatus::Waiting, &reason).context(Fault::StateSave)?;
                return Err(e);
            }
        };
        self.finish_job(research::Outcome {
            task_id: id, title: task.title, ok: false, clarification: None,
            paused: Some(reason.clone()), summary: reason, verdict: Some(format!("#{} {}", item.id, j.verdict.zh())),
        }, 0).await
    }

    #[cfg(test)]
    pub(crate) async fn watchdog(&mut self) -> Result<()> {
        for (id, reason) in self.control.jobs().take_overdue(Instant::now(), &self.cfg.runtime.hands).await {
            self.on_job_watchdog(id, reason).await?;
        }
        Ok(())
    }

    pub(super) async fn deliver(&mut self, o: research::Outcome) -> Result<()> {
        let Some(ctx) = self.owner_ctx().await else {
            warn!("task {} finished but the owner has never messaged; nowhere to report", o.task_id);
            return Ok(());
        };
        let body = if o.paused.is_some() {
            format!("（这不是主人的消息。任务 {}：{}，已保存断点并暂挂，尚未完成。\n{}\n用自己的语气简短告诉主人暂停原因，可用 /run {} 继续。）",
                o.task_id, o.title, delivery_preview(&o.summary), o.task_id)
        } else if let Some(questions) = &o.clarification {
            let questions = delivery_preview(questions);
            format!(
                "（这不是主人的消息。任务 {}：{}，已暂停并等待主人澄清，还没开始本轮取证。\n\
                 用你自己的语气向主人问清以下问题，保留任务编号，告诉他回复时注明编号即可：\n{questions}\n\
                 告诉主人补充后可用 /run {} 继续。主人的后续回答应通过 <补充 任务=\"{}\">回答内容</补充> 挂回任务；本轮只提问，不生成补充标记，也不宣称任务完成。）",
                o.task_id, o.title, o.task_id, o.task_id
            )
        } else {
            let board = self.board();
            let dir = board.task_dir(&o.task_id);
            let report = if board.get(&o.task_id).is_some_and(|t| t.kind == TaskKind::Code) { dir.join("scratch") } else { dir.join("report.md") };
            format!(
                "（这不是主人的消息。任务 {}：{}。\n结果预览：\n{}\n报告/产出位置（若已生成）：{}\n审查：{}\n用你自己的话简短告诉主人结论，附上报告位置；没做完就直说卡在哪。）",
                o.task_id, o.title,
                delivery_preview(&o.summary), report.display(),
                o.verdict.clone().unwrap_or_else(|| "无".into())
            )
        };
        self.converse(&ctx, body, 0, false).await
    }

    // ---------------------------------------------------------------- inner life
}
