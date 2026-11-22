use anyhow::Result;
use chrono::Local;
use serde_json::{Value, json};
use tracing::warn;

use super::{Bot, ChatCtx, chat::Intent, ingress::{self, ControlEvent}, reading, report_error};
use crate::channels::{Envelope, ReplyPiece, reply_text};
use crate::error_report::Fault;
use crate::mind::judge;
use crate::ns::ProcessingPolicy;
use crate::social::bot_flags;
use crate::store::{Outcome, ReviewCategory, ReviewStatus};
use crate::work::board::{Board, TaskKind, TaskStatus};

/// Keep the acknowledgement (including failures and the original supplement) in the sent reply.
pub(super) fn attach_followups_with_sources(board: &mut Board, ctx: &ChatCtx, intent: &mut Intent, evidence: &[reading::Evidence]) {
    for (id, body) in &intent.followups {
        let reply = if !ctx.owner_private() {
            "任务补充只接受主人私聊。".to_string()
        } else {
            let save = || -> Result<()> {
                if evidence.is_empty() { return Ok(()); }
                anyhow::ensure!(board.get(id).is_some_and(|t| t.status.is_open()), "任务已关闭或不存在");
                reading::save(board, id, ctx, evidence)
            };
            match save().and_then(|()| board.follow_up(id, body, ctx.thread_ref())) {
                Ok(()) => format!("已补充到任务 {id}（{}）。", board.get(id).unwrap().title),
                Err(e) => format!("任务 {id} 的补充未完整保存：{e:#}。补充原文：{body}"),
            }
        };
        if intent.silent {
            intent.pieces.clear();
        } else {
            intent.pieces.push(ReplyPiece::Text("\n".into()));
        }
        intent.pieces.push(ReplyPiece::Text(reply));
        intent.text = reply_text(&intent.pieces);
        intent.silent = false;
    }
}

#[cfg(test)]
pub(super) fn attach_followups(board: &mut Board, ctx: &ChatCtx, intent: &mut Intent) {
    attach_followups_with_sources(board, ctx, intent, &[]);
}

impl Bot {
    /// File a request that needs the owner's decision.
    pub(super) async fn relay_request(&self, requester: &str, request: &str) -> Result<()> {
        self.audit(requester, "relay_to_owner", false, "requester is not the owner", Outcome::Denied).await?;
        let j = judge::Judgement { verdict: judge::Verdict::Owner, reasons: "请求者不是主人，按权限规则转交".into(), fixes: String::new(),
            urgent: judge::deadline_today(request, Local::now().date_naive()) };
        judge::file(
            &self.store,
            ReviewCategory::Act,
            &format!("{} 的请求", self.people.name_of(requester)),
            request,
            &j,
            true,
            json!({"request_from": requester, "text": request}),
        ).await?;
        Ok(())
    }

    pub(super) async fn command(&mut self, line: &str, ctx: &ChatCtx, env: &Envelope) -> String {
        let (cmd, arg) = line.split_once(char::is_whitespace).map(|(c, a)| (c, a.trim())).unwrap_or((line, ""));
        if matches!(cmd, "/task" | "/code") && !arg.is_empty() {
            let mut inputs = reading::Inputs::default();
            inputs.include(env);
            let has_inputs = !inputs.is_empty();
            let result: Result<String> = async {
                let cards = ctx.task_cards(&self.board());
                let (_, policy) = self.frame(ctx, 1, &cards).await?;
                let read = self.read_inputs(ctx, &policy, inputs, reading::Purpose::Task).await;
                if !read.errors.is_empty() { return Ok(format!("任务未建立：{}", read.errors.join(" "))); }
                anyhow::ensure!(!has_inputs || policy == ProcessingPolicy::Any, "当前处理策略不允许把材料交给任务执行器");
                let evidence = self.task_sources(ctx, &policy, arg, read.evidence);
                let kind = if cmd == "/code" { TaskKind::Code } else { TaskKind::Research };
                let id = self.add_chat_task(ctx, kind, arg, &evidence)?;
                Ok(if evidence.is_empty() { format!("记下了：{id}") } else { format!("记下了：{id}（附 {} 份材料）", evidence.len()) })
            }.await;
            return result.unwrap_or_else(|e| format!("出错了：{e:#}"));
        }
        match self.run_command(cmd, arg, ctx).await {
            Ok(s) => s,
            Err(e) => format!("出错了：{e:#}"),
        }
    }

    pub(super) fn add_chat_task(&mut self, ctx: &ChatCtx, kind: TaskKind, title: &str, evidence: &[reading::Evidence]) -> Result<String> {
        let mut board = self.board();
        let task = board.add(title, kind, &self.cfg.acl.owner.person_id, TaskStatus::Inbox, Some(ctx.thread_ref()))?;
        if let Err(e) = reading::save(&board, &task.id, ctx, evidence) {
            board.set_status(&task.id, TaskStatus::Waiting, "任务材料未保存完整")?;
            return Err(e);
        }
        drop(board);
        self.wake_decide = true;
        Ok(task.id)
    }

    pub(super) async fn run_command(&mut self, cmd: &str, arg: &str, ctx: &ChatCtx) -> Result<String> {
        Ok(match cmd {
            "/help" => "/status 状态  /board 看板  /task 描述 新研究任务  /code 描述 新编码任务  /run 任务编号 立即开始\n\
                        /stop 停下当前任务  /freeze 冻结主动行为与工具  /unfreeze 解冻\n\
                        /digest 现在发待决事项  /approve 编号  /reject 编号 理由  /reflect 现在整理记忆\n\
                        /evolve /evolve-status 自进化功能说明"
                .into(),
            "/status" => {
                let (spent, limit) = self.models.spent_today();
                let s = &self.state;
                format!(
                    "心境 {}，精力 {:.2}{}，社交需求 {:.2}，好奇 {:.2}\n今日花费 ${spent:.4} / ${limit:.2}\n运行任务：{}\n冻结：{}\n聊天模型：{}",
                    s.mood.label,
                    s.energy.level,
                    if s.energy.asleep { "（睡觉中）" } else { "" },
                    s.drives.social,
                    s.drives.curiosity,
                    self.running_jobs(),
                    self.frozen(),
                    self.models.pick("chat", &ProcessingPolicy::Any).map(|m| m.id).unwrap_or_else(|e| e.to_string()),
                )
            }
            "/board" => self.board().summary(),
            "/task" | "/code" if !arg.is_empty() => {
                let kind = if cmd == "/code" { TaskKind::Code } else { TaskKind::Research };
                format!("记下了：{}", self.add_chat_task(ctx, kind, arg, &[])?)
            }
            "/run" if !arg.is_empty() => self.start_job_with_owner_slot(arg, ctx.owner_private()).await?,
            "/evolve" | "/evolve-status" => {
                if ctx.owner_private() {
                    crate::evolution::PLACEHOLDER_REPLY.into()
                } else {
                    "只有主人私聊能使用此命令。".into()
                }
            }
            "/stop" => {
                anyhow::ensure!(ctx.owner_private(), "仅主人私聊可以停止任务");
                let stopped = self.stop_jobs().await;
                let reply = ingress::stop_receipt(&stopped);
                self.on_control_event(ControlEvent::Stopped { stopped })?;
                reply
            }
            "/freeze" | "/unfreeze" => {
                anyhow::ensure!(ctx.owner_private(), "仅主人私聊可以更改冻结状态");
                let freeze = cmd == "/freeze";
                let state = ingress::set_freeze(&self.control, &self.store, ctx.platform, freeze).await?;
                if freeze && let Err(e) = self.stop_for_freeze().await {
                    report_error(&e, Fault::Background);
                    warn!("freeze cleanup failed: {e:#}");
                    return Ok(format!("{}\n停止任务或保存收尾记录时出错。", state.message()));
                }
                state.message().into()
            }
            "/digest" => judge::digest(&self.store).await?.unwrap_or_else(|| "没有待你决定的事。".into()),
            "/approve" | "/reject" => {
                let (id, reason) = arg.split_once(char::is_whitespace).map(|(i, r)| (i, r.trim())).unwrap_or((arg, ""));
                let id = id.trim_start_matches('#').parse()?;
                self.settle_review(id, cmd == "/approve", reason, ctx).await?
            }
            "/reflect" => {
                let n = self.extract_all(chrono::Duration::days(2)).await?;
                format!("整理了 {n} 个对话的记忆。")
            }
            _ => "不认识这个命令，/help 看看。".into(),
        })
    }

    pub(super) async fn settle_review(&mut self, id: u64, approve: bool, reason: &str, ctx: &ChatCtx) -> Result<String> {
        let mut item = self
            .store
            .reviews().await?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| anyhow::anyhow!("没有 #{id}"))?;
        if item.status != ReviewStatus::Open {
            return Ok(format!("#{id} 已经处理过了。"));
        }
        if let Some(sender) = item.payload.get("agent_candidate").and_then(Value::as_str) {
            bot_flags::settle(&self.store, sender, approve).await?;
        }
        item.status = if approve { ReviewStatus::Approved } else { ReviewStatus::Rejected };
        if !reason.is_empty() {
            item.verdict.push_str(&format!("；主人：{reason}"));
        }
        self.store.save_review(&item).await?;
        if !approve {
            return Ok(format!("#{id} 驳回了。"));
        }
        let p = &item.payload;
        let done = match item.category {
            ReviewCategory::Act if p.get("agent_candidate").is_some() => {
                format!("已将 {} 加入运行时 agent 名单并应用 agent 回复限制。写入 config/acl.yaml 的 known_agents 可在数据重置后保留。",
                    p["agent_candidate"].as_str().unwrap_or_default())
            }
            ReviewCategory::Act if p.get("request_from").is_some() => {
                let text = p.get("text").and_then(Value::as_str).unwrap_or_default();
                let from = p.get("request_from").and_then(Value::as_str).unwrap_or_default();
                let t = self.board().add(text, TaskKind::Research, from, TaskStatus::Inbox, Some(ctx.thread_ref()))?;
                self.wake_decide = true;
                format!("已加入看板：{}", t.id)
            }
            _ => "记下了。".to_string(),
        };
        Ok(format!("#{id} 通过。{done}"))
    }
}
