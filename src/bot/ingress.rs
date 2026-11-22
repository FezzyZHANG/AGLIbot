use std::collections::VecDeque;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use serde_json::json;
use tokio::{sync::mpsc, time::Instant};
use tracing::{debug, error, info, warn};

use super::{Bot, ChatCtx, chat::ReplyBatch, report_error};
use crate::channels::{Envelope, Outlets};
use crate::config::{AclCfg, Config, SessionCfg};
use crate::control::ControlPlane;
use crate::error_report::{Fault, OwnerState, notify_state, report};
use crate::ns::{Audience, Namespace, Scope};
use crate::social::{floor::FloorDecision, identity::{Who, identify}, skill_lines};
use crate::store::{MessageRecord, Store};
use crate::work::research;

pub enum Event {
    Inbound(Envelope),
    Tick,
    JobDone(research::Outcome),
    JobWatchdog { task_id: String, reason: String },
    Control(ControlEvent),
}

pub enum ControlEvent {
    Frozen { stopped: Vec<String> },
    Stopped { stopped: Vec<String> },
}

pub enum WorkItem {
    Messages(Vec<Envelope>),
    Tick,
    JobDone(research::Outcome),
    JobWatchdog { task_id: String, reason: String },
    Control(ControlEvent),
}

impl From<Event> for WorkItem {
    fn from(event: Event) -> Self {
        match event {
            Event::Inbound(env) => Self::Messages(vec![env]),
            Event::Tick => Self::Tick,
            Event::JobDone(outcome) => Self::JobDone(outcome),
            Event::JobWatchdog { task_id, reason } => Self::JobWatchdog { task_id, reason },
            Event::Control(event) => Self::Control(event),
        }
    }
}

pub fn owner_command(acl: &AclCfg, env: &Envelope) -> bool {
    let task_caption = matches!(env.authored_text.split_whitespace().next(), Some("/task" | "/code"));
    !env.is_group && (task_caption || (env.authored_text.trim_start().starts_with('/') && env.text.trim_start().starts_with('/'))) && identify(acl, env).is_owner
}

pub(super) async fn set_freeze(control: &ControlPlane, store: &Store, platform: &str, freeze: bool) -> Result<OwnerState> {
    let state = if freeze { OwnerState::Frozen } else { OwnerState::Unfrozen };
    let before = control.set_frozen(store, freeze).await.inspect_err(|_| report(Fault::StateSave))?;
    if before != freeze && platform != "telegram" { notify_state(state); }
    Ok(state)
}

pub(super) fn stop_receipt(stopped: &[String]) -> String {
    if stopped.is_empty() { "现在没在做任务。".into() } else { format!("停了：{}", stopped.join("、")) }
}

pub async fn try_handle_fast_control(
    cfg: &Config, store: &Store, outlets: &Outlets, control: &ControlPlane,
    events: &mpsc::UnboundedSender<Event>, env: &Envelope,
) -> bool {
    let cmd = env.authored_text.trim();
    if env.notice || !matches!(cmd, "/freeze" | "/unfreeze" | "/stop") || !owner_command(&cfg.acl, env) {
        return false;
    }
    let (reply, event) = if cmd == "/stop" {
        let stopped = control.jobs().stop_all().await;
        (stop_receipt(&stopped), Some(ControlEvent::Stopped { stopped }))
    } else {
        let freeze = cmd == "/freeze";
        match set_freeze(control, store, env.platform, freeze).await {
            Ok(state) => {
                let event = if freeze {
                    Some(ControlEvent::Frozen { stopped: control.jobs().stop_all().await })
                } else {
                    None
                };
                (state.message().to_string(), event)
            }
            Err(e) => (format!("出错了：{e:#}"), None),
        }
    };
    if let Err(e) = store.kv_set("owner_last_chat", json!({"platform": env.platform, "chat_id": env.chat_id})).await {
        report(Fault::StateSave);
        warn!("saving owner route: {e:#}");
    }
    let mut shutdown = control.shutdown_token();
    tokio::select! {
        _ = shutdown.cancelled() => {}
        sent = outlets.send(env.platform, &env.chat_id, &reply) => {
            if let Err(e) = sent {
                report(Fault::Delivery);
                warn!("sending control reply: {e:#}");
            }
        }
    }
    if let Some(event) = event { let _ = events.send(Event::Control(event)); }
    true
}

#[derive(Default)]
pub struct WorkQueue {
    events: VecDeque<QueuedEvent>,
    #[cfg(test)]
    pub(super) last_batch: Option<String>,
}

struct QueuedEvent {
    event: Event,
    received: Instant,
}

struct Batch {
    end: usize,
    deadline: Option<Instant>,
    cap_ended: bool,
}

impl WorkQueue {
    pub fn new() -> Self { Self::default() }

    pub fn is_empty(&self) -> bool { self.events.is_empty() }

    pub fn push(&mut self, event: Event) {
        let now = Instant::now();
        let received = match &event {
            Event::Inbound(env) => env.arrived.map_or(now, |t| t.min(now)),
            _ => now,
        };
        self.events.push_back(QueuedEvent { event, received });
    }

    pub fn deadline(&self, acl: &AclCfg, session: &SessionCfg) -> Option<Instant> {
        self.batch(acl, session).deadline
    }

    fn batch(&self, acl: &AclCfg, session: &SessionCfg) -> Batch {
        let mut batch = Batch { end: 1, deadline: None, cap_ended: false };
        let Some(QueuedEvent { event: Event::Inbound(first), received }) = self.events.front() else { return batch };
        if owner_command(acl, first) { return batch }
        let cfg = &session.debounce;
        let enabled = cfg.quiet_ms != 0;
        let quiet = Duration::from_millis(cfg.quiet_ms.min(cfg.cap_ms));
        let cap = *received + Duration::from_millis(cfg.cap_ms);
        let mut deadline = *received + quiet;
        for queued in self.events.iter().skip(1) {
            if enabled && queued.received >= deadline { break }
            match &queued.event {
                Event::Inbound(next) if same_chat(first, next) => {
                    if owner_command(acl, next) { return batch }
                    deadline = (queued.received + quiet).min(cap);
                }
                Event::Control(_) | Event::JobWatchdog { .. } => return batch,
                _ if !enabled => break,
                _ => {}
            }
            batch.end += 1;
        }
        batch.deadline = enabled.then_some(deadline);
        batch.cap_ended = enabled && deadline == cap && Instant::now() >= cap;
        batch
    }
}

#[cfg(test)]
impl<const N: usize> From<[Event; N]> for WorkQueue {
    fn from(events: [Event; N]) -> Self {
        let mut queue = Self::new();
        for event in events { queue.push(event); }
        queue
    }
}

fn same_chat(first: &Envelope, next: &Envelope) -> bool {
    next.platform == first.platform && next.chat_id == first.chat_id && next.is_group == first.is_group
}

pub async fn recv_work(events: &mut mpsc::Receiver<Event>, queued: &mut WorkQueue, acl: &AclCfg, session: &SessionCfg) -> Option<WorkItem> {
    if queued.is_empty() {
        queued.push(events.recv().await?);
    }
    // Drain only the current backlog, so a busy producer cannot postpone Ctrl+C or the cap.
    for _ in 0..events.len() {
        if let Ok(ev) = events.try_recv() {
            queued.push(ev);
        }
    }
    while let Some(deadline) = queued.deadline(acl, session) {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => break,
            ev = events.recv() => match ev {
                Some(ev) => queued.push(ev),
                None => break,
            },
        }
    }
    next_work(queued, acl, session)
}

/// Take the ready chat batch, preserving the arrival order of all deferred events.
pub fn next_work(queue: &mut WorkQueue, acl: &AclCfg, session: &SessionCfg) -> Option<WorkItem> {
    let batch = queue.batch(acl, session);
    let first = match queue.events.pop_front()?.event {
        Event::Inbound(first) => first,
        event => return Some(event.into()),
    };
    let mut messages = Vec::new();
    let mut deferred = VecDeque::new();
    for queued in queue.events.drain(..batch.end - 1) {
        match queued.event {
            Event::Inbound(next) if same_chat(&first, &next) => messages.push(next),
            _ => deferred.push_back(queued),
        }
    }
    deferred.append(&mut queue.events);
    queue.events = deferred;
    let cfg = &session.debounce;
    let (basis, window) = if owner_command(acl, &first) { ("command", 0) }
        else if cfg.quiet_ms == 0 { ("disabled", 0) }
        else { ("fixed", cfg.quiet_ms.min(cfg.cap_ms)) };
    let line = format!(
        "reply batch chat_key={}:{} messages={} window_ms={window} basis={basis} cap_ended={}",
        first.platform, first.chat_id, messages.len() + 1, batch.cap_ended
    );
    info!("{line}");
    #[cfg(test)]
    { queue.last_batch = Some(line); }
    messages.insert(0, first);
    Some(WorkItem::Messages(messages))
}

/// One group line waiting to be read into his session.
#[derive(Debug, Clone)]
pub(super) struct UnseenLine {
    pub(super) at: DateTime<Local>,
    /// `昵称(账号)：内容`, as the speaker label renders it.
    pub(super) text: String,
    /// This line @-ed him, replied to him or called him by name.
    pub(super) addressed: bool,
}

pub(super) const MAX_UNSEEN_PER_CHAT: usize = 80;

pub(super) fn push_unseen(lines: &mut VecDeque<UnseenLine>, line: UnseenLine) {
    if lines.len() == MAX_UNSEEN_PER_CHAT { lines.pop_front(); }
    lines.push_back(line);
}

/// Mark addressed lines and display the lines preceding the first mention as background.
pub(super) fn render_unseen(lines: &VecDeque<UnseenLine>, addressed_turn: bool) -> String {
    let row = |l: &UnseenLine| {
        let mark = if addressed_turn && l.addressed { "→ " } else { "" };
        format!("{mark}[{}] {}", l.at.format("%H:%M"), l.text)
    };
    let rows = |range: std::ops::Range<usize>| lines.range(range).map(row).collect::<Vec<_>>().join("\n");
    let first = if addressed_turn { lines.iter().position(|l| l.addressed) } else { None };
    let len = lines.len();
    match first {
        Some(i) if i > 0 => format!(
            "群里之前的消息（你没接过话，只是背景）：\n{}\n\n现在的消息（→ 是叫你的那条）：\n{}",
            rows(0..i), rows(i..len),
        ),
        Some(_) => format!("群里的新消息（→ 是叫你的那条）：\n{}", rows(0..len)),
        None => format!("群里最近的消息（按时间排列，最后一条最新）：\n{}", rows(0..len)),
    }
}

impl Bot {
    pub(super) async fn prepare_messages(&mut self, messages: Vec<Envelope>) -> ReplyBatch {
        let mut batch = ReplyBatch::default();
        for env in messages {
            if let Err(e) = self.on_message(env, &mut batch).await {
                report_error(&e, Fault::EventHandling);
                error!("{e:#}");
            }
        }
        batch
    }

    pub(super) async fn on_messages(&mut self, messages: Vec<Envelope>) -> Result<()> {
        let batch = self.prepare_messages(messages).await;
        let Some(ctx) = batch.ctx else { return Ok(()) };
        if batch.speak {
            self.turn_inputs = Some((ctx.key.clone(), batch.inputs));
            if ctx.is_group {
                self.converse_group(&ctx, false).await?;
            } else {
                self.converse(&ctx, batch.lines.join("\n"), batch.current, false).await?;
            }
        } else if batch.consider {
            self.consider_group(&ctx).await?;
        }
        Ok(())
    }

    /// Queue this message's images for the worker and return its transcript markers.
    pub(super) fn queue_media(&self, env: &mut Envelope, who: &Who, ctx: &ChatCtx) -> String {
        use crate::channels::AttachmentKind;
        let collect = env.is_group || who.is_owner;
        let mut markers = String::new();
        for attachment in &mut env.attachments {
            if !matches!(attachment.kind, AttachmentKind::Image | AttachmentKind::Sticker) { continue; }
            let Some(id) = self.media.next_id() else { continue };
            attachment.media_id = Some(id);
            let queued = self.media.enqueue(crate::mind::media::MediaJob {
                id, platform: env.platform, attachment: attachment.clone(),
                origin: ctx.key.clone(), chat_id: env.chat_id.clone(),
                sender: who.person_id.clone(), sender_name: env.sender_name.clone(),
                ns: ctx.ns.clone(), scope: ctx.scope.clone(), collect,
            });
            if queued { markers.push_str(&crate::mind::media::marker(id)); }
        }
        markers
    }

    pub(super) async fn on_message(&mut self, mut env: Envelope, batch: &mut ReplyBatch) -> Result<()> {
        let now = Utc::now();
        let who = self.runtime_identity(&env);
        let group_key = env.group_key();
        if let Some(g) = &group_key {
            if self.cfg.social.ignored_groups.contains(g) {
                if self.ignored_logged.insert(g.clone()) {
                    debug!("ignoring group {g} (social.ignored_groups)");
                }
                return Ok(());
            }
        } else if !who.is_owner && !self.cfg.social.private_from_others {
            info!("ignored private message from {} ({})", env.sender_key(), env.sender_name);
            return Ok(());
        }
        if !who.is_owner && env.platform == "telegram" && self.cfg.acl.owner.telegram_user_id == 0 {
            info!("message from telegram user {} — set OWNER_TELEGRAM_ID if that is you", env.sender_id);
        }
        if !who.is_owner && env.platform == "qq" && self.cfg.acl.owner.qq_user_id == 0 && !env.is_group {
            info!("message from qq user {} — set OWNER_QQ_ID if that is you", env.sender_id);
        }
        self.people.observe(&self.store, &env, &who, now).await?;
        self.check_qq_groups(&env, &who).await?;
        let observed_at = env.arrived.and_then(|t| chrono::Duration::from_std(t.elapsed()).ok()).map_or(now, |age| now - age);
        if let Err(e) = self.bot_flags.observe(&self.store, &env, &who, observed_at).await {
            warn!("QQ agent heuristic failed: {e:#}");
        }

        let ctx = match &group_key {
            Some(g) => {
                let speakers = self.speakers.entry(format!("{}:{}", env.platform, env.chat_id)).or_default();
                speakers.retain(|p| p != &who.person_id);
                speakers.push(who.person_id.clone());
                if speakers.len() > 8 {
                    speakers.remove(0);
                }
                ChatCtx {
                    key: format!("{}:{}", env.platform, env.chat_id),
                    platform: env.platform,
                    chat_id: env.chat_id.clone(),
                    message_id: env.message_id.clone(),
                    ns: Namespace::Group(g.clone()),
                    scope: Scope::Group { id: g.clone() },
                    audience: Audience::group(g, speakers.iter().cloned()),
                    is_group: true,
                    partner: None,
                }
            }
            None => ChatCtx {
                key: format!("{}:{}", env.platform, env.chat_id),
                platform: env.platform,
                chat_id: env.chat_id.clone(),
                message_id: env.message_id.clone(),
                ns: Namespace::Person(who.person_id.clone()),
                scope: Scope::Persons { ids: vec![who.person_id.clone()] },
                audience: self.people.private_audience(&who.person_id),
                is_group: false,
                partner: Some(who.clone()),
            },
        };
        self.store.append_message(&MessageRecord {
            ts: now,
            platform: env.platform.into(),
            chat_id: env.chat_id.clone(),
            sender: who.person_id.clone(),
            sender_name: env.sender_name.clone(),
            outgoing: false,
            text: env.text.clone(),
            ns: ctx.ns.clone(),
            scope: ctx.scope.clone(),
        }).await?;

        let markers = self.queue_media(&mut env, &who, &ctx);
        let spoken = format!("{}{markers}", env.text.replace(crate::mind::media::MARKER_OPEN, ""));

        // Read-only batches stop before floor, unseen lines or any reply/command path.
        if group_key.as_ref().is_some_and(|g| self.cfg.social.read_only_group(g)) {
            return Ok(());
        }

        if who.is_owner && !env.is_group {
            self.store.kv_set("owner_last_chat", json!({"platform": env.platform, "chat_id": env.chat_id})).await?;
            self.state.talked_with_owner(now);
        }

        // A skill request gets its canned voice line, for anyone, before owner commands or the model.
        // In groups only when he is @'d (or the request replies to him).
        if (env.mentions_me || !env.is_group) && (who.is_owner || !self.state.asleep)
            && let Some(name) = skill_lines::requested(&env.text) {
            if !self.floor.allow_reply(&self.cfg.social, &ctx.key, &who.person_id, who.is_agent, now) {
                info!("reply cap reached for {} in {}", who.person_id, ctx.key);
                return Ok(());
            }
            let reply = self.skill_lines.reply(&name, now.timestamp_subsec_nanos() as usize);
            if ctx.is_group {
                self.floor.spoke(&ctx.key, false, now);
            }
            self.send_plain(&ctx, &reply).await?;
            return self.store.append_message(&MessageRecord {
                ts: Utc::now(),
                platform: ctx.platform.into(),
                chat_id: ctx.chat_id.clone(),
                sender: "me".into(),
                sender_name: String::new(),
                outgoing: true,
                text: reply,
                ns: ctx.ns.clone(),
                scope: ctx.scope.clone(),
            }).await;
        }

        if !env.attachments.is_empty() {
            let sender = self.speaker_label(env.platform, &who.person_id, &env.sender_name);
            let waiting = self.pending_media.entry(ctx.key.clone()).or_default();
            crate::mind::vision::remember(waiting, env.platform, &sender, &env.attachments, tokio::time::Instant::now());
        }

        if who.is_owner && owner_command(&self.cfg.acl, &env) {
            let line = if env.authored_text.trim_start().starts_with('/') { env.authored_text.trim() } else { env.text.trim() };
            let reply = self.command(line, &ctx, &env).await;
            return self.send_plain(&ctx, &reply).await;
        }

        let asleep = self.state.asleep;
        let line = format!("{}：{spoken}", self.speaker_label(env.platform, &who.person_id, &env.sender_name));

        // Store and account for each message first; reply once using the last message's thread.
        batch.current += 1;
        batch.ctx = Some(ctx);
        let ctx = batch.ctx.as_ref().unwrap();

        if !ctx.is_group {
            let label = if who.is_owner { "主人".to_string() } else { self.people.name_of(&who.person_id) };
            batch.lines.push(format!("{label}：{spoken}"));
            if asleep && !who.is_owner {
                self.remember_overnight(&env, who.is_agent).await?;
                info!("asleep: not replying to {} now", who.person_id);
                return Ok(());
            }
            if !self.floor.allow_reply(&self.cfg.social, &ctx.key, &who.person_id, who.is_agent, now) {
                info!("reply cap reached for {} in {}", who.person_id, ctx.key);
                return Ok(());
            }
            batch.speak = true;
            if !who.is_agent { batch.inputs.include(&env); }
            return Ok(());
        }

        let called = match (&group_key, env.mentions_me) {
            (Some(g), false) => {
                self.lexicon.refresh(&self.store, now).await;
                self.lexicon.called_by(g, &env.text).map(String::from)
            }
            _ => None,
        };
        if let Some(alias) = &called {
            info!("called by name `{alias}` in {}", ctx.key);
        }
        let addressed = (env.mentions_me || called.is_some()) && (!asleep || who.is_owner);
        push_unseen(self.unseen.entry(ctx.key.clone()).or_default(),
            UnseenLine { at: now.with_timezone(&Local), text: line, addressed });
        let decision = if env.notice {
            if addressed { FloorDecision::Speak } else { FloorDecision::Silent("notice") }
        } else {
            self.floor.on_group_message(&self.cfg.social, &ctx.key, addressed, asleep, now)
        };
        match decision {
            FloorDecision::Speak => {
                if self.floor.allow_reply(&self.cfg.social, &ctx.key, &who.person_id, who.is_agent, now) {
                    batch.speak = true;
                    if addressed && !who.is_agent && (env.mentions_me || group_key.as_ref().is_some_and(|g| self.lexicon.called_by(g, &env.authored_text).is_some())) {
                        batch.inputs.include(&env);
                    }
                } else {
                    info!("reply cap reached for {} in {}", who.person_id, ctx.key);
                }
            }
            FloorDecision::Consider => batch.consider = true,
            FloorDecision::Silent(_) => {}
        }
        Ok(())
    }
}
