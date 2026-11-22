use anyhow::Result;
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::Instant;
use tracing::{error, info, warn};

use super::{Bot, ChatCtx, report_error};
use crate::channels::Envelope;
use crate::error_report::{Fault, report};
use crate::mind::{context::tail, decide::{self, Action, Situation}, judge, reflect, state::Change};
use crate::ns::{Namespace, ProcessingPolicy, Scope};
use crate::store::{MessageRecord, Outcome};
use crate::work::board::TaskStatus;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct OvernightChat {
    pub(super) platform: String,
    pub(super) chat_id: String,
    pub(super) sender_id: String,
    pub(super) is_agent: bool,
}

pub(super) const OVERNIGHT_CHATS: &str = "overnight_chats";
const NIGHTLY_ATTEMPTS: u8 = 3;
const NIGHTLY_BACKOFF: Duration = Duration::minutes(30);

impl Bot {
    pub(super) async fn overnight_chats(&self) -> Result<Vec<OvernightChat>> {
        Ok(self.store.kv_get(OVERNIGHT_CHATS).await?.map(serde_json::from_value).transpose()?.unwrap_or_default())
    }

    pub(super) async fn remember_overnight(&self, env: &Envelope, is_agent: bool) -> Result<()> {
        let mut chats = self.overnight_chats().await?;
        if let Some(chat) = chats.iter_mut().find(|c| c.platform == env.platform && c.chat_id == env.chat_id) {
            chat.is_agent |= is_agent;
        } else {
            // Insertion order is the order of each chat's first unanswered message.
            chats.push(OvernightChat {
                platform: env.platform.into(), chat_id: env.chat_id.clone(), sender_id: env.sender_id.clone(), is_agent,
            });
        }
        self.store.kv_set(OVERNIGHT_CHATS, json!(chats)).await
    }

    pub(super) async fn overnight_turn(&self, chat: &OvernightChat) -> Result<Option<(ChatCtx, String, usize)>> {
        let platform = match chat.platform.as_str() {
            "telegram" => "telegram",
            "qq" => "qq",
            _ => return Ok(None),
        };
        let env = Envelope {
            authored_text: String::new(), notice: false, attachments: Vec::new(),
            platform, chat_id: chat.chat_id.clone(), sender_id: chat.sender_id.clone(), sender_name: String::new(),
            sender_is_bot: chat.is_agent, is_group: false, mentions_me: false, text: String::new(), message_id: None, arrived: None,
        };
        let mut who = self.runtime_identity(&env);
        if who.is_owner {
            return Ok(None);
        }
        who.is_agent |= self.people.get(&who.person_id).is_some_and(|p| p.kind == crate::store::PersonKind::Agent);
        let ctx = ChatCtx {
            key: format!("{platform}:{}", chat.chat_id), platform, chat_id: chat.chat_id.clone(), message_id: None,
            ns: Namespace::Person(who.person_id.clone()), scope: Scope::Persons { ids: vec![who.person_id.clone()] },
            audience: self.people.private_audience(&who.person_id), is_group: false, partner: Some(who),
        };
        let messages = self.store.messages_after(&ctx.ns, DateTime::UNIX_EPOCH).await?;
        let in_chat = |m: &MessageRecord| m.platform == platform && m.chat_id == chat.chat_id;
        let start = messages.iter().rposition(|m| in_chat(m) && m.outgoing).map_or(0, |i| i + 1);
        let pending = &messages[start..];
        let lines: Vec<String> = pending.iter()
            .filter(|m| in_chat(m) && !m.outgoing && m.scope.permits(&ctx.audience))
            .map(|m| format!("{}：{}", self.people.name_of(&m.sender), m.text)).collect();
        Ok(Some((ctx, lines.join("\n"), pending.len())))
    }

    pub(super) async fn catch_up_overnight(&mut self) -> Result<()> {
        if self.frozen() || !self.cfg.social.private_from_others {
            return Ok(());
        }
        let mut remaining = self.overnight_chats().await?;
        for chat in remaining.clone() {
            let result = async {
                let Some((ctx, mut body, current)) = self.overnight_turn(&chat).await? else { return Ok(false) };
                if body.is_empty() {
                    return Ok(false);
                }
                let who = ctx.partner.as_ref().unwrap();
                if !self.floor.allow_reply(&self.cfg.social, &ctx.key, &who.person_id, who.is_agent, Utc::now()) {
                    return Ok(false);
                }
                body.push_str("\n\n（你刚睡醒，这些是睡觉期间收到的消息。酌情回答，不需要回复就只输出 [沉默] 或 [不发]。）");
                self.converse_with_options(&ctx, body, current, false, true).await?;
                Ok::<_, anyhow::Error>(true)
            }.await;
            match result {
                Ok(true) => {
                    remaining.retain(|c| c.platform != chat.platform || c.chat_id != chat.chat_id);
                    self.store.kv_set(OVERNIGHT_CHATS, json!(remaining)).await?;
                }
                Ok(false) => {}
                Err(e) => {
                    report_error(&e, Fault::Background);
                    warn!("overnight catch-up in {}:{} failed: {e:#}", chat.platform, chat.chat_id);
                }
            }
        }
        Ok(())
    }

    pub(super) async fn on_tick(&mut self) -> Result<()> {
        self.on_tick_at(Utc::now()).await
    }

    pub(super) async fn on_tick_at(&mut self, now: DateTime<Utc>) -> Result<()> {
        let changes = self.state.tick(now, &self.cfg.runtime.routine);
        self.push_urgent().await?;
        if self.state.asleep {
            return self.maybe_nightly(now).await;
        }
        if changes.contains(&Change::WokeUp) {
            self.catch_up_overnight().await?;
            for o in std::mem::take(&mut self.deliveries) {
                self.deliver(o).await?;
            }
        }
        self.maybe_digest(now).await?;
        self.extract_due().await?;
        let revisit = self.state.revisit_at.is_some_and(|t| t <= now);
        if !self.frozen() && (revisit || self.proactive_due(now) || self.wake_decide) {
            self.wake_decide = false;
            self.state.revisit_at = None;
            self.think(now, revisit).await?;
        }
        Ok(())
    }

    pub(super) async fn maybe_digest(&mut self, now: DateTime<Utc>) -> Result<()> {
        let Some(ctx) = self.owner_ctx().await else { return Ok(()) };
        let local = now.with_timezone(&Local);
        if local.time() < self.cfg.runtime.judge.digest_at || self.state.last_digest == Some(local.date_naive()) {
            return Ok(());
        }
        if let Some((text, ids)) = judge::build_digest(&self.store).await? {
            let outcome = match self.send_plain(&ctx, &text).await {
                Ok(()) => {
                    judge::mark_notified(&self.store, &ids).await?;
                    self.state.last_digest = Some(local.date_naive());
                    Outcome::Sent
                }
                Err(e) => {
                    report(Fault::Delivery);
                    error!("digest send failed: {e:#}");
                    Outcome::Failed
                }
            };
            self.audit("me", "daily_digest", true, "", outcome).await?;
        }
        Ok(())
    }

    pub(super) async fn push_urgent(&self) -> Result<()> {
        let Some(ctx) = self.owner_ctx().await else { return Ok(()) };
        while let Some(mut item) = judge::claim_urgent(&self.store, self.state.asleep).await? {
            let outcome = match self.send_plain(&ctx, &judge::urgent_message(&item)).await {
                Ok(()) => {
                    item.pushed = true;
                    self.store.save_review(&item).await?;
                    Outcome::Sent
                }
                Err(e) => {
                    report(Fault::Delivery);
                    warn!(id = item.id, "urgent review send failed: {e:#}");
                    Outcome::Unknown
                }
            };
            self.audit("me", "urgent_review", true, &item.id.to_string(), outcome).await?;
        }
        Ok(())
    }

    /// Extract chats with enough unprocessed messages.
    pub(super) async fn extract_due(&mut self) -> Result<()> {
        let after = Utc::now() - chrono::Duration::days(2);
        for ns in self.store.active_namespaces(after).await? {
            if !self.cfg.social.reflects_normally(&ns) { continue }
            let since = reflect::watermark(&self.store, &ns).await?;
            if self.store.messages_after(&ns, since).await?.len() >= self.cfg.runtime.memory.extract_after {
                reflect::extract_ns(&self.models, &self.store, &mut self.people, &ns).await?;
            }
        }
        Ok(())
    }

    pub(super) async fn extract_all(&mut self, window: chrono::Duration) -> Result<usize> {
        let mut n = 0;
        for ns in self.store.active_namespaces(Utc::now() - window).await? {
            if !self.cfg.social.reflects_normally(&ns) { continue }
            let o = reflect::extract_ns(&self.models, &self.store, &mut self.people, &ns).await?;
            n += usize::from(o.messages > 0);
        }
        Ok(n)
    }

    async fn maybe_nightly(&mut self, now: DateTime<Utc>) -> Result<()> {
        let date = reflect::diary_day(now.with_timezone(&Local));
        if self.state.last_reflect == Some(date) {
            return Ok(());
        }
        let mut attempts = 0;
        if let Some((day, count, retry_at)) = self.nightly_retry && day == date {
            if count >= NIGHTLY_ATTEMPTS || now < retry_at {
                return Ok(());
            }
            attempts = count;
        }
        let start = Instant::now();
        let result = self.nightly(date, now).await;
        self.nightly_retry = result.is_err().then(|| (date, attempts + 1, now + start.elapsed() + NIGHTLY_BACKOFF));
        result
    }

    async fn nightly(&mut self, date: NaiveDate, now: DateTime<Utc>) -> Result<()> {
        info!("going to sleep: reflecting on {date}");
        self.extract_all(Duration::days(2)).await?;
        reflect::read_only_nightly(&self.models, &self.store, &mut self.people, &self.cfg.social, date,
            now - Duration::days(2)).await?;
        let dir = self.cfg.mind_dir().join("life").join("journal");
        if let Some(path) = reflect::write_journal(&self.models, &self.store, self.speaker.l1(), dir, date).await? {
            info!("journal written: {}", path.display());
        }
        self.state.last_reflect = Some(date);
        self.speaker.reset_all();
        Ok(())
    }

    fn proactive_due(&self, now: DateTime<Utc>) -> bool {
        let gap = Duration::hours(self.cfg.runtime.heartbeat.proactive_gap_hours);
        [self.state.last_owner_contact, self.state.last_proactive, self.last_proactive_check]
            .into_iter().flatten().all(|t| now - t >= gap)
    }

    async fn think(&mut self, now: DateTime<Utc>, revisit: bool) -> Result<()> {
        let may_message_owner = if self.proactive_due(now) {
            self.last_proactive_check = Some(now);
            self.owner_ctx().await.is_some()
        } else {
            false
        };
        let startable: Vec<String> = if self.control.jobs().len() >= self.cfg.runtime.hands.max_jobs {
            Vec::new()
        } else {
            self.board.tasks().await?.into_iter().filter(|t| t.status == TaskStatus::Inbox).map(|t| t.id).collect()
        };
        if !may_message_owner && startable.is_empty() && !revisit {
            return Ok(());
        }
        let s = &self.state;
        let (spent, limit) = self.models.spent_today();
        let since_owner = s
            .last_owner_contact
            .map(|t| format!("{} 小时前", (now - t).num_hours()))
            .unwrap_or_else(|| "还没聊过".into());
        let text = format!(
            "{}\n\n上次和主人说话：{since_owner}\n\
             今天花费 ${spent:.3} / ${limit:.2}\n正在做：{}\n\n## 看板\n{}",
            tail(now.with_timezone(&Local), s.asleep, None, &[]),
            self.running_jobs(),
            self.board.summary().await?,
        );
        let situation = Situation { text, startable, may_message_owner };
        let d = decide::decide(&self.models, &ProcessingPolicy::Any, self.speaker.l1(), &situation).await?;
        if self.frozen() { return Ok(()) }
        info!("decide: {} {:?} — {}", d.action, d.target, d.reason);
        self.state.revisit_at = decide::parse_revisit(d.revisit_when.as_deref(), now);
        match decide::validate(&d, &situation) {
            Action::Nothing => {}
            Action::Work { task_id } => {
                let msg = self.start_job(&task_id).await?;
                info!("{msg}");
            }
            Action::MessageOwner => self.reach_out(now, &d.reason).await?,
        }
        Ok(())
    }

    async fn reach_out(&mut self, now: DateTime<Utc>, reason: &str) -> Result<()> {
        let Some(ctx) = self.owner_ctx().await else { return Ok(()) };
        self.state.last_proactive = Some(now);
        let body = "（没有新消息。现在有空，你想主动找主人聊几句。根据你们最近的聊天和记忆自然地开个头；改主意了就只回复 [不发]。）";
        self.converse_with_options(&ctx, body.into(), 0, false, true).await?;
        self.audit("me", "reach_out_owner", true, reason, Outcome::Sent).await?;
        Ok(())
    }
}
