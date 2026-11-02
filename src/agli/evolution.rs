use super::*;
use crate::evolution::{
    gate::{Completion, Gate},
    restart::Restart,
};

pub(super) struct Running {
    id: String,
    platform: &'static str,
    chat_id: String,
    handle: JoinHandle<Result<Completion>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl Agli {
    pub(super) fn evolution_gate(&self) -> Gate {
        Gate::new(&self.cfg, self.store.clone())
    }

    fn evolution_available(&self, ctx: &ChatCtx) -> Result<()> {
        anyhow::ensure!(ctx.owner_private(), "只有主人私聊能选择实现者和批准进化");
        anyhow::ensure!(!self.frozen(), "冻结期间不能检查或应用进化提案");
        anyhow::ensure!(self.evolution_job.is_none() && self.evolution_restart.is_none(), "已有进化任务正在运行");
        Ok(())
    }

    pub(super) fn start_evolution(&mut self, id: &str, ctx: &ChatCtx) -> Result<String> {
        self.evolution_available(ctx)?;
        anyhow::ensure!(crate::evolution::gate::valid_id(id), "提案 ID 只能用小写字母、数字和连字符（1–64 字符）");
        let gate = self.evolution_gate();
        let models = self.models.clone();
        let owner = self.cfg.acl.owner.person_id.clone();
        let key = id.to_string();
        let handle = tokio::spawn(async move { gate.prepare(&key, &owner, &models).await });
        self.evolution_job = Some(Running { id: id.into(), platform: ctx.platform, chat_id: ctx.chat_id.clone(), handle });
        Ok(format!("开始检查进化提案 {id}；/stop 或 /freeze 可停止，结果会回到这里。"))
    }

    pub(super) fn approve_evolution(&mut self, id: u64, reason: &str, ctx: &ChatCtx) -> Result<String> {
        self.evolution_available(ctx)?;
        let mut item = self.store.reviews()?.into_iter().find(|r| r.id == id).context("没有这条审查")?;
        let gate = self.evolution_gate();
        let key = gate.approve(&item, &self.cfg.acl.owner.person_id)?;
        item.status = ReviewStatus::Approved;
        if !reason.is_empty() {
            item.verdict.push_str(&format!("；主人：{reason}"));
        }
        self.store.save_review(&item)?;
        let task_id = key.clone();
        let handle = tokio::spawn(async move { gate.finish(&task_id).await });
        self.evolution_job = Some(Running { id: key, platform: ctx.platform, chat_id: ctx.chat_id.clone(), handle });
        Ok(format!("已记录 #{id} 的主人审批；复核来源和改动后提交、构建并重启。"))
    }

    pub(super) async fn stop_evolution(&mut self) -> Result<bool> {
        if let Some(mut job) = self.evolution_job.take() {
            job.handle.abort();
            let _ = (&mut job.handle).await;
            self.evolution_gate().interrupted(&job.id, "owner stopped/froze evolution; inspect before any new proposal")?;
            return Ok(true);
        }
        Ok(false)
    }

    pub async fn poll_evolution(&mut self) {
        if !self.evolution_job.as_ref().is_some_and(|job| job.handle.is_finished()) {
            return;
        }
        let mut job = self.evolution_job.take().unwrap();
        let result = (&mut job.handle).await;
        let message = match result {
            Ok(Ok(done)) => {
                self.evolution_restart = done.restart;
                done.message
            }
            other => {
                let why = match other {
                    Ok(Err(e)) => format!("{e:#}"),
                    Err(e) => e.to_string(),
                    _ => unreachable!(),
                };
                let _ = self.evolution_gate().interrupted(&job.id, &why);
                format!("进化 {} 已停下：{why}", job.id)
            }
        };
        if let Err(e) = self.outlets.send(job.platform, &job.chat_id, &message).await {
            warn!("evolution result delivery failed: {e:#}");
        }
    }

    pub fn take_evolution_restart(&mut self) -> Option<Restart> {
        self.evolution_restart.take()
    }
    pub fn acknowledge_evolution_start(&self) -> Result<()> {
        self.evolution_gate().acknowledge_start()
    }
}

use anyhow::Context;
