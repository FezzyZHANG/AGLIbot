use anyhow::Result;
use chrono::{Local, Utc};
use tracing::{error, info, warn};

use super::{Bot, ChatCtx, chat::{parse_intent, resolve_mentions}, commands::attach_followups_with_sources, reading};
use crate::channels::{ReplyPiece, reply_text};
use crate::error_report::{Fault, report};
use crate::mind::{context::{tail, user_turn}, speak::SayTurn};
use crate::ns::ProcessingPolicy;
use crate::store::{MessageRecord, Outcome};

impl Bot {
    pub(super) async fn converse(&mut self, ctx: &ChatCtx, body: String, current: usize, unsolicited: bool) -> Result<()> {
        // Another bot in private may be dropped, as L1 allows; people who talk to him are answered.
        let may_quit = unsolicited || ctx.partner.as_ref().is_some_and(|w| w.is_agent);
        self.converse_with_options(ctx, body, current, unsolicited, may_quit).await
    }

    /// `may_quit`: this turn's prompt let him decide to say nothing, so he may also take the
    /// reply back while formatting it.
    /// Append a chat turn, act on its markers, and send the visible reply.
    pub(super) async fn converse_with_options(&mut self, ctx: &ChatCtx, mut body: String, current: usize, unsolicited: bool, may_quit: bool) -> Result<()> {
        // Addressed turns only: chiming in never spends a look, and the images keep waiting.
        let mut look = match self.pending_media.remove(&ctx.key) {
            Some(pending) if !unsolicited => crate::mind::vision::look(&self.outlets, &self.media, pending).await,
            Some(pending) => {
                self.pending_media.insert(ctx.key.clone(), pending);
                Default::default()
            }
            None => Default::default(),
        };
        let mut without_parts = format!("{}{}", self.media.render(&body, &[]), look.note_without_parts);
        body = self.media.render(&body, &look.with_pixels);
        body.push_str(&look.note);
        let now = Utc::now();
        let busy = self.control.jobs().first_title();
        let mut notes: Vec<String> = self.dropped_tasks.remove(&ctx.key).unwrap_or_default().iter()
            .map(|t| format!("系统：你之前回复里的任务“{t}”没有建立（只有主人私聊里的任务才算），现在没有人在做它，也不会有结果。"))
            .collect();
        // Slang goes in the tail, never L1/L2, so the cached prefix is unchanged.
        notes.extend(self.slang_note(ctx, &body, now).await);
        let turn_tail = tail(now.with_timezone(&Local), &self.state.snapshot(), busy.as_deref(), &notes);
        // Recall runs on every turn (cheap at this volume) so the policy is known before routing.
        let tasks = ctx.task_cards(&self.board());
        let (l2, policy) = self.frame(ctx, current, &tasks).await?;
        let inputs = self.turn_inputs.take().filter(|(key, _)| key == &ctx.key && !unsolicited)
            .map(|(_, inputs)| inputs).unwrap_or_default();
        let mut task_images = inputs.images();
        let mut read = self.read_inputs(ctx, &policy, inputs, reading::Purpose::Chat).await;
        if !look.undecoded.is_empty() && policy.allows_executor("deepseek") {
            let unread = task_images.take_undecoded(&look.undecoded);
            let ocr = self.read_inputs(ctx, &policy, unread, reading::Purpose::OcrFallback).await;
            read.note.push_str(&ocr.note);
            read.evidence.extend(ocr.evidence);
            read.errors.extend(ocr.errors);
        }
        let mut readable_text = !read.evidence.is_empty();
        let mut read_errors = read.errors;
        let mut evidence = self.task_sources(ctx, &policy, &body, read.evidence);
        body.push_str(&read.note);
        without_parts.push_str(&read.note);
        without_parts.push_str(&read.fallback);
        look.parts.extend(read.parts);
        let mut readable_pixels = !look.parts.is_empty();
        let text = user_turn(&turn_tail, &body);
        self.speaker.refresh_tasks(&ctx.key, &tasks);
        #[cfg(test)]
        let response = if let Some(test) = &mut self.test_chat {
            test.turns.push((ctx.key.clone(), text.clone(), l2.clone()));
            test.images.push(look.parts.iter().filter(|p| matches!(p, crate::mind::llm::Part::Jpeg(_))).count());
            Some(test.replies.pop_front().expect("unexpected model call"))
        } else {
            None
        };
        #[cfg(not(test))]
        let response: Option<Result<String>> = None;
        let response = match response {
            Some(r) => r,
            None => {
                let had_images = !look.parts.is_empty();
                let first = self.speaker.say(&self.models, &ctx.key, &policy, SayTurn { now, text, images: look.parts, ephemeral: Vec::new() }, || Ok(l2.clone())).await;
                match first {
                    Err(e) if had_images => {
                        warn!("reply with images failed, retrying without them: {e:#}");
                        readable_pixels = false;
                        if !task_images.is_empty() && policy.allows_executor("deepseek") {
                            let transcription = self.read_inputs(ctx, &policy, std::mem::take(&mut task_images), reading::Purpose::OcrFallback).await;
                            without_parts.push_str(&transcription.note);
                            readable_text |= !transcription.evidence.is_empty();
                            read_errors.extend(transcription.errors);
                            evidence.extend(transcription.evidence);
                        } else if !read.fallback.trim().is_empty() {
                            read_errors.push(read.fallback.trim().into());
                        }
                        let text = user_turn(&turn_tail, &without_parts);
                        self.speaker.say(&self.models, &ctx.key, &policy, SayTurn { now, text, images: Vec::new(), ephemeral: Vec::new() }, || Ok(l2)).await
                    }
                    other => other,
                }
            }
        };
        if response.is_err() && !read_errors.is_empty() { report(Fault::ChatReply); }
        let raw = match response {
            _ if !read_errors.is_empty() && !readable_text && !readable_pixels => String::new(),
            Ok(r) => r,
            Err(_) if !read_errors.is_empty() => String::new(),
            Err(e) => {
                error!("reply failed: {e:#}");
                if !ctx.is_group && self.outlets.send(ctx.platform, &ctx.chat_id, "（刚才走神了，你再说一遍？）").await.is_err() {
                    report(Fault::Delivery);
                }
                return Err(e.context(Fault::ChatReply));
            }
        };
        let mut intent = parse_intent(&raw);
        if ctx.owner_private() && policy == ProcessingPolicy::Any && (!intent.tasks.is_empty() || !intent.followups.is_empty()) && !task_images.is_empty() {
            let transcription = self.read_inputs(ctx, &policy, task_images, reading::Purpose::Task).await;
            evidence.extend(transcription.evidence);
            if !transcription.errors.is_empty() {
                read_errors.push(format!("任务图片材料未完整保存：{}", transcription.errors.join(" ")));
            }
        }
        if let Some(id) = intent.sticker.take() {
            warn!("ignored sticker `{id}` in a draft in {}: candidates come only after a request", ctx.key);
        }
        // A sticker answers the people, chosen after his words exist, never before them.
        let may_sticker = !unsolicited && current > 0;
        let sticker = match intent.sticker_wish.take() {
            Some(wish) if may_sticker => self.pick_sticker(ctx, &policy, &wish).await,
            _ => None,
        };
        intent.silent = intent.silent || (intent.text.is_empty() && sticker.is_none() && intent.reaction.is_none());
        let from_owner = ctx.partner.as_ref().is_some_and(|w| w.is_owner);
        for (kind, title) in &intent.tasks {
            if from_owner {
                let id = self.add_chat_task(ctx, *kind, title, &evidence)?;
                info!("owner asked for a task: {title} ({id})");
            } else {
                warn!("ignored a task marker outside an owner chat in {}: {title}", ctx.key);
                // His transcript still holds the marker; tell him next turn so he does not act as if it runs.
                self.dropped_tasks.entry(ctx.key.clone()).or_default().push(title.clone());
            }
        }
        if let Some(req) = &intent.relay {
            let requester = ctx.partner.as_ref().map(|w| w.person_id.clone()).unwrap_or_else(|| ctx.key.clone());
            self.relay_request(&requester, req).await?;
            self.push_urgent().await?;
        }
        attach_followups_with_sources(&mut self.board(), ctx, &mut intent, &evidence);
        if intent.silent && read_errors.is_empty() {
            if ctx.is_group && !unsolicited {
                warn!("stayed silent though addressed in {}", ctx.key);
            } else {
                info!("chose not to speak in {}", ctx.key);
            }
            return Ok(());
        }
        if !intent.pieces.is_empty() {
            match self.format_reply(ctx, unsolicited, may_quit, &policy, intent.pieces).await {
                Some(pieces) => intent.pieces = pieces,
                None if !read_errors.is_empty() => intent.pieces = Vec::new(),
                None => return Ok(()),
            }
        }
        if !read_errors.is_empty() {
            let prefix = if intent.pieces.is_empty() { "" } else { "\n" };
            intent.pieces.push(ReplyPiece::Text(format!("{prefix}{}", read_errors.join("\n"))));
        }
        intent.text = reply_text(&intent.pieces);
        if intent.text.is_empty() && sticker.is_none() && intent.reaction.is_none() { return Ok(()) }
        if ctx.is_group {
            self.floor.spoke(&ctx.key, unsolicited, now);
        }
        let speakers = self.speakers.get(&ctx.key).map(Vec::as_slice).unwrap_or_default();
        let self_id = self.outlets.qq.as_ref().map_or(0, |qq| qq.self_id());
        resolve_mentions(&mut intent.pieces, ctx, speakers, &self.people, self_id);
        let mut stored = Vec::new();
        if let Some(emoji) = intent.reaction.take() {
            if self.react(ctx, &emoji).await {
                stored.push(format!("[贴表情 {emoji}]"));
            } else if intent.text.is_empty() && sticker.is_none() {
                intent.text = emoji.clone();
                intent.pieces = vec![ReplyPiece::Text(emoji)];
            }
        }
        let outcome = if intent.text.is_empty() { Outcome::Sent } else { self.send_bubbles(ctx, &intent.pieces).await };
        if unsolicited {
            self.audit("me", "group_chime_in", true, &ctx.key, outcome).await?;
        }
        if outcome != Outcome::Sent {
            return Err(anyhow::anyhow!("reply delivery in {}: {outcome:?}", ctx.key).context(Fault::Delivery));
        }
        if let Some(offer) = &sticker {
            // Text already went out, so its record below still matters; a lone sticker has nothing to keep.
            match self.send_sticker(ctx, offer).await {
                Ok(()) => stored.push(format!("[表情包：{}]", offer.record.description)),
                Err(e) if intent.text.is_empty() && stored.is_empty() => return Err(e.context(Fault::Delivery)),
                Err(e) => {
                    report(Fault::Delivery);
                    error!("sticker not sent in {}: {e:#}", ctx.key);
                }
            }
        }
        if !intent.text.is_empty() { stored.insert(0, intent.text.clone()); }
        if current > 0 && self.cfg.social.reflects_normally(&ctx.ns) {
            for message in self.store.recent_in_ns(&ctx.ns, current).await? {
                if !message.outgoing && message.platform == ctx.platform && message.chat_id == ctx.chat_id
                    && message.scope.permits(&ctx.audience) {
                    self.appraisals.record(message.text, policy.clone());
                }
            }
        }
        self.store.append_message(&MessageRecord {
            ts: Utc::now(),
            platform: ctx.platform.into(),
            chat_id: ctx.chat_id.clone(),
            sender: "me".into(),
            sender_name: String::new(),
            outgoing: true,
            text: stored.join(" "),
            ns: ctx.ns.clone(),
            scope: ctx.scope.clone(),
        }).await?;
        Ok(())
    }

    pub(super) async fn sticker_offers(&self, ctx: &ChatCtx, query: &str) -> Vec<crate::mind::expression::Offer> {
        #[cfg(test)]
        if let Some(test) = &self.test_chat { return test.offers.clone(); }
        if ctx.platform != "qq" { return Vec::new() }
        let dir = crate::mind::media::media_dir(&self.cfg);
        let sendable = |r: &crate::store::MediaRecord| crate::mind::media::file_path(&dir, &r.hash, &r.ext).exists();
        crate::mind::expression::offers(&self.store, &self.models, &ctx.key, query, sendable).await
            .unwrap_or_else(|e| { warn!("no stickers offered in {}: {e:#}", ctx.key); Vec::new() })
    }

    /// False when it did not land, or the chat has no reactions.
    pub(super) async fn react(&self, ctx: &ChatCtx, emoji: &str) -> bool {
        let Some(id) = crate::mind::expression::reaction_id(emoji) else { return false };
        #[cfg(test)]
        if let Some(test) = &self.test_chat {
            test.sent_texts.lock().unwrap().push(format!("react:{id}"));
            return test.sends.lock().unwrap().pop_front().expect("unexpected reaction") == Outcome::Sent;
        }
        let (true, Some(message_id)) = (ctx.is_group, ctx.message_id.as_deref()) else { return false };
        match self.outlets.react(ctx.platform, message_id, &id).await {
            Ok(()) => { info!("reacted {emoji} in {}", ctx.key); true }
            Err(e) => { warn!("reaction failed in {}: {e:#}", ctx.key); false }
        }
    }

    pub(super) async fn send_sticker(&self, ctx: &ChatCtx, offer: &crate::mind::expression::Offer) -> Result<()> {
        let reply_to = if ctx.is_group { ctx.message_id.as_deref() } else { None };
        let record = &offer.record;
        #[cfg(test)]
        let sent = if let Some(test) = &self.test_chat {
            test.sent_texts.lock().unwrap().push(format!("sticker:{}", offer.id));
            let outcome = test.sends.lock().unwrap().pop_front().expect("unexpected sticker");
            anyhow::ensure!(outcome == Outcome::Sent, "sticker delivery: {outcome:?}");
            true
        } else { false };
        #[cfg(not(test))]
        let sent = false;
        if !sent {
            let path = crate::mind::media::file_path(&crate::mind::media::media_dir(&self.cfg), &record.hash, &record.ext);
            let bytes = tokio::fs::read(&path).await.map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
            self.outlets.send_sticker(ctx.platform, &ctx.chat_id, &bytes, reply_to).await?;
        }
        info!("sent sticker {} in {}", offer.id, ctx.key);
        self.store.record_media_sent(&record.hash, Utc::now()).await?;
        Ok(())
    }

    /// One step on the chat's open session after the draft. Tests answer from `steps`, and an
    /// unscripted step gets `echo`, so the draft passes through unchanged.
    async fn step(&self, ctx: &ChatCtx, policy: &ProcessingPolicy, prompt: String, parts: Vec<crate::mind::llm::Part>, echo: &str) -> Result<String> {
        #[cfg(test)]
        if let Some(test) = &self.test_chat {
            test.step_prompts.lock().unwrap().push((policy.clone(), prompt));
            test.step_images.lock().unwrap().push(parts.iter().filter(|p| matches!(p, crate::mind::llm::Part::Jpeg(_))).count());
            test.labels.lock().unwrap().extend(parts.iter().filter_map(|p| match p {
                crate::mind::llm::Part::Text(text) => Some(text.clone()),
                _ => None,
            }));
            return test.steps.lock().unwrap().pop_front().unwrap_or_else(|| Ok(echo.to_string()));
        }
        let _ = echo;
        self.speaker.follow_ephemeral(&self.models, &ctx.key, policy, prompt, parts).await
    }

    /// He asked for a sticker: search by his own words and let him pick one, or none.
    async fn pick_sticker(&self, ctx: &ChatCtx, policy: &ProcessingPolicy, wish: &str) -> Option<crate::mind::expression::Offer> {
        let offers = self.sticker_offers(ctx, wish).await;
        let (offers, parts) = crate::mind::expression::show(&crate::mind::media::media_dir(&self.cfg), offers).await;
        if offers.is_empty() {
            info!("no sticker found for `{wish}` in {}", ctx.key);
            return None;
        }
        let prompt = format!("（系统：按你想要的“{wish}”找到了下面这些表情包。合适就只回复 <表情包>编号</表情包>，都不合适就只回复 [不用]。）");
        let reply = match self.step(ctx, policy, prompt, parts, "[不用]").await {
            Ok(reply) => reply,
            Err(e) => {
                warn!("sticker pick failed in {}: {e:#}", ctx.key);
                return None;
            }
        };
        let id = parse_intent(&reply).sticker?;
        let found = offers.into_iter().find(|o| o.id == id);
        if found.is_none() { warn!("ignored sticker `{id}`, not offered in {}", ctx.key); }
        found
    }
}
