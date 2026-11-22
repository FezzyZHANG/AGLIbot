use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use super::{TaskFollowup, TaskRecord, TaskStatus, TaskVerdict, ThreadRef};

pub(crate) const RECOVERY_NOTE: &str = "上次运行中断，回到待办";

impl TaskRecord {
    pub fn unconsumed_followups(&self) -> Vec<&TaskFollowup> {
        self.followups
            .iter()
            .filter(|f| f.consumed_round.is_none())
            .collect()
    }

    pub fn max_followup_seq(&self) -> u64 {
        self.followups.iter().map(|f| f.seq).max().unwrap_or(0)
    }

    pub(super) fn append_followup(
        &mut self,
        body: &str,
        thread: &ThreadRef,
        now: DateTime<Utc>,
    ) -> Result<()> {
        anyhow::ensure!(self.status.is_open(), "任务已关闭（{:?}）", self.status);
        anyhow::ensure!(!body.trim().is_empty(), "补充内容为空");
        let seq = self
            .max_followup_seq()
            .checked_add(1)
            .context("followup sequence exhausted")?;
        self.followups.push(TaskFollowup {
            seq,
            ts: now,
            body: body.trim().into(),
            thread: thread.clone(),
            consumed_round: None,
        });
        if !self.threads.contains(thread) {
            self.threads.push(thread.clone());
        }
        self.updated = now;
        Ok(())
    }

    pub(super) fn consume_followups(&mut self, up_to_seq: u64, round: u32) {
        for f in &mut self.followups {
            if f.seq <= up_to_seq && f.consumed_round.is_none() {
                f.consumed_round = Some(round);
            }
        }
        self.current_round = self.current_round.max(round);
    }

    pub(super) fn update_status(
        &mut self,
        status: TaskStatus,
        note: &str,
        now: DateTime<Utc>,
        verdict: Option<TaskVerdict>,
    ) {
        if self.status != status {
            self.status_log = task_status_line(status, note);
        }
        self.status = status;
        if !note.is_empty() {
            self.note = note.into();
        }
        if verdict.is_some() {
            self.last_verdict = verdict;
        }
        self.updated = now;
    }
}

pub(crate) fn task_status_line(status: TaskStatus, note: &str) -> String {
    if note.is_empty() {
        format!("状态 → {status:?}")
    } else {
        format!("状态 → {status:?}：{note}")
    }
}
