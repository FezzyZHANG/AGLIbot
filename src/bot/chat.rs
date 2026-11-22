use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, error, info, warn};

use super::{Bot, OWNER_MARK, ingress::render_unseen, reading};
use crate::channels::{ReplyPiece, reply_prefix, reply_text};
use crate::error_report::Fault;
use crate::mind::llm::JsonSchema;
use crate::ns::{Audience, Namespace, ProcessingPolicy, Scope};
use crate::social::{identity::Who, people::People};
use crate::store::Outcome;
use crate::work::board::{Board, TaskKind, ThreadRef};

/// What a reply asked the runtime to do, parsed out of the text.
#[derive(Debug, Default, PartialEq)]
pub struct Intent {
    pub text: String,
    pub pieces: Vec<ReplyPiece>,
    pub silent: bool,
    pub tasks: Vec<(TaskKind, String)>,
    pub relay: Option<String>,
    pub followups: Vec<(String, String)>,
    /// An id from this turn's offered stickers.
    pub sticker: Option<String>,
    /// What sticker he would like to send; the runtime searches and shows him candidates.
    pub sticker_wish: Option<String>,
    /// One emoji to put on the message he is answering.
    pub reaction: Option<String>,
}

fn take_tag(text: &mut String, open_prefix: &str, close: &str) -> Option<(String, String)> {
    let start = text.find(open_prefix)?;
    let head_end = start + text[start..].find('>')?;
    let end = head_end + text[head_end..].find(close)?;
    let attrs = text[start + open_prefix.len()..head_end].to_string();
    let body = text[head_end + 1..end].trim().to_string();
    text.replace_range(start..end + close.len(), "");
    Some((attrs, body))
}

/// Strip runtime markers from a reply. See the "hands" section of L1.
pub fn parse_intent(raw: &str) -> Intent {
    let mut text = raw.to_string();
    // A merged batch may ask for several things; each gets its own marker and task.
    let mut tasks = Vec::new();
    while let Some((attrs, body)) = take_tag(&mut text, "<任务", "</任务>") {
        let kind = if attrs.contains("code") { TaskKind::Code } else { TaskKind::Research };
        tasks.push((kind, body));
    }
    let relay = take_tag(&mut text, "<转告主人", "</转告主人>").map(|(_, body)| body);
    let mut followups = Vec::new();
    while let Some((attrs, body)) = take_tag(&mut text, "<补充", "</补充>") {
        let id = attrs.trim().strip_prefix("任务=\"").and_then(|s| s.strip_suffix('"')).unwrap_or_default();
        followups.push((id.to_string(), body));
    }
    let mut sticker_wish = None;
    while let Some((_, body)) = take_tag(&mut text, "<找表情包", "</找表情包>") {
        if !body.is_empty() { sticker_wish.get_or_insert(body); }
    }
    let mut sticker = None;
    while let Some((_, body)) = take_tag(&mut text, "<表情包", "</表情包>") {
        sticker.get_or_insert(body);
    }
    let mut reaction = None;
    while let Some((_, body)) = take_tag(&mut text, "<贴表情", "</贴表情>") {
        if crate::mind::expression::reaction_id(&body).is_some() {
            reaction.get_or_insert(body);
        }
    }
    let mut pieces = parse_mentions(text.trim());
    let mut text = reply_text(&pieces);
    let quiet = text.is_empty() || text == "[沉默]" || text == "[不发]";
    let silent = quiet && sticker.is_none() && sticker_wish.is_none() && reaction.is_none();
    if quiet && !silent {
        pieces.clear();
        text.clear();
    }
    Intent { text, pieces, silent, tasks, relay, followups, sticker, sticker_wish, reaction }
}

fn parse_mentions(text: &str) -> Vec<ReplyPiece> {
    let (open, close) = ("<提及>", "</提及>");
    let mut pieces = Vec::new();
    let (mut literal, mut cursor) = (0, 0);
    while let Some(start) = text[cursor..].find(open).map(|i| cursor + i) {
        let body = start + open.len();
        let Some(end) = text[body..].find(close).map(|i| body + i) else { break };
        cursor = end + close.len();
        let name = text[body..end].trim();
        // Empty, nested or multiline tags are malformed and stay entirely literal.
        if name.is_empty() || text[body..end].contains(['<', '>', '\n', '\r']) { continue; }
        if start > literal { pieces.push(ReplyPiece::Text(text[literal..start].into())); }
        pieces.push(ReplyPiece::Mention { name: name.into(), qq_id: None });
        literal = cursor;
    }
    if literal < text.len() { pieces.push(ReplyPiece::Text(text[literal..].into())); }
    pieces
}

/// Resolve only against this chat's recent speakers, never against arbitrary QQ ids in prose.
pub(super) fn resolve_mentions(pieces: &mut [ReplyPiece], ctx: &ChatCtx, speakers: &[String], people: &People, self_id: i64) {
    let mut resolved = 0;
    for piece in pieces {
        let ReplyPiece::Mention { name, qq_id } = piece else { continue };
        *qq_id = None;
        // The model may copy a whole `【主人】nickname(id)` label from the group lines.
        let bare = name.strip_prefix(OWNER_MARK).unwrap_or(name);
        let bare = bare.strip_suffix(')').and_then(|s| s.rsplit_once('('))
            .filter(|(_, id)| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).map_or(bare, |(n, _)| n);
        *name = bare.to_string();
        if ctx.platform != "qq" || !ctx.is_group || !ctx.chat_id.starts_with("group:") { continue; }
        let matches: Vec<_> = speakers.iter().filter_map(|id| people.get(id))
            .filter(|person| person.name == *name).collect();
        let target = if let [person] = matches.as_slice() {
            let ids: HashSet<i64> = person.platform_ids.iter().filter_map(|id| id.strip_prefix("qq:"))
                .filter_map(|id| id.parse().ok()).collect();
            if ids.len() == 1 { ids.into_iter().next() } else { None }
        } else { None };
        if resolved < 2 && name != "all" && name != "全体成员" && self_id > 0
            && let Some(id) = target.filter(|id| *id > 0 && *id != self_id) {
            *qq_id = Some(id);
            resolved += 1;
        } else {
            debug!(chat = %ctx.key, %name, matches = matches.len(), "mention left as plain text: no unique eligible target or reply limit reached");
        }
    }
}

/// What the chat shows today: neither QQ nor Telegram renders Markdown, so it arrives as raw
/// characters. Image-rendered replies will be a second surface beside this one.
const PLAIN_TEXT: &str = "现在是纯文字模式：对方看到的就是你打出来的字符，Markdown 不会被渲染。\
不要用 **粗体**、# 标题、- 或 1. 开头的列表、`反引号`、代码块、表格；要强调就换个说法，要分点就用短句分行，\
编号、命令、链接直接写出来。";

/// Pieces as he writes them, with mentions as `<提及>` markers.
pub(super) fn markup(pieces: &[ReplyPiece]) -> String {
    pieces.iter().map(|piece| match piece {
        ReplyPiece::Text(text) => text.clone(),
        ReplyPiece::Mention { name, .. } => format!("<提及>{name}</提及>"),
    }).collect()
}

/// Short multi-paragraph replies go out as separate bubbles, like texting.
pub fn bubbles(text: &str) -> Vec<String> {
    let parts: Vec<String> = text.split("\n\n").map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
    if parts.len() > 1 && parts.len() <= 3 && parts.iter().all(|p| p.chars().count() <= 120) {
        parts
    } else {
        vec![text.trim().to_string()]
    }
}

pub(super) fn reply_bubbles(pieces: &[ReplyPiece]) -> Vec<Vec<ReplyPiece>> {
    bubbles(&reply_text(pieces)).into_iter().enumerate().map(|(i, text)| {
        if i == 0 { reply_prefix(pieces, text.len()) } else { vec![ReplyPiece::Text(text)] }
    }).collect()
}

/// Return the total units and the UTF-8 end of the longest prefix within the cap.
pub(super) fn measure_reply(text: &str, cap: usize) -> (usize, usize) {
    let (mut units, mut end, mut in_word) = (0, 0, false);
    for (i, c) in text.char_indices() {
        let cjk = matches!(c, '〇' | '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' |
            '\u{f900}'..='\u{faff}' | '\u{20000}'..='\u{2fa1f}' | '\u{30000}'..='\u{323af}');
        let word = !cjk && c.is_alphanumeric();
        if cjk || (word && !in_word) { units += 1; }
        in_word = word;
        if units <= cap { end = i + c.len_utf8(); }
    }
    (units, end)
}

pub(super) fn truncate_reply(text: &str, cap: usize) -> String {
    let (units, end) = measure_reply(text, cap);
    if units <= cap { return text.to_string() }
    if cap == 0 { return String::new() }
    let prefix = &text[..end];
    if let Some((i, c)) = prefix.char_indices().rev().find(|(_, c)| matches!(c, '。' | '！' | '？' | '!' | '?' | '.' | '\n')) {
        let sentence = prefix[..i + c.len_utf8()].trim();
        if !sentence.is_empty() { return sentence.to_string() }
    }
    format!("{}…", prefix.trim_end())
}

pub(super) struct ChatCtx {
    pub(super) key: String,
    pub(super) platform: &'static str,
    pub(super) chat_id: String,
    pub(super) message_id: Option<String>,
    pub(super) ns: Namespace,
    pub(super) scope: Scope,
    pub(super) audience: Audience,
    pub(super) is_group: bool,
    /// The person addressed in a private chat.
    pub(super) partner: Option<Who>,
}

#[derive(Default)]
pub(super) struct ReplyBatch {
    pub(super) inputs: reading::Inputs,
    pub(super) ctx: Option<ChatCtx>,
    pub(super) lines: Vec<String>,
    pub(super) current: usize,
    pub(super) speak: bool,
    pub(super) consider: bool,
}

impl ChatCtx {
    pub(super) fn owner_private(&self) -> bool {
        !self.is_group && self.partner.as_ref().is_some_and(|w| w.is_owner)
    }

    pub(super) fn thread_ref(&self) -> ThreadRef {
        ThreadRef { platform: self.platform.into(), chat_id: self.chat_id.clone(), message_id: self.message_id.clone() }
    }

    pub(super) fn task_cards(&self, board: &Board) -> String {
        if self.owner_private() { board.cards() } else { String::new() }
    }
}

#[derive(Deserialize)]
struct Chime {
    speak: bool,
    #[serde(default)]
    reason: String,
}

fn chime_schema() -> JsonSchema {
    JsonSchema {
        name: "chime",
        schema: json!({
            "type": "object",
            "properties": { "speak": { "type": "boolean" }, "reason": { "type": "string" } },
            "required": ["speak", "reason"],
            "additionalProperties": false,
        }),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum FormatAction { Keep, Replace, Drop }

#[derive(Deserialize)]
struct FormatResult {
    action: FormatAction,
    text: String,
}

fn format_schema() -> JsonSchema {
    JsonSchema {
        name: "format_reply",
        schema: json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["keep", "replace", "drop"] },
                "text": { "type": "string" },
            },
            "required": ["action", "text"],
            "additionalProperties": false,
        }),
    }
}

pub(super) fn needs_format(text: &str, cap: Option<usize>) -> bool {
    if cap.is_some_and(|cap| measure_reply(text, cap).0 > cap)
        || ["**", "__", "`", "](http://", "](https://"].iter().any(|marker| text.contains(marker))
    {
        return true;
    }
    let mut list_items = 0;
    for line in text.lines().map(str::trim_start) {
        let hashes = line.bytes().take_while(|&byte| byte == b'#').count();
        if ((1..=6).contains(&hashes) && line.as_bytes().get(hashes) == Some(&b' '))
            || line.contains("|---") || line.contains("| ---")
        {
            return true;
        }
        if line.starts_with("- ") || line.starts_with("* ")
            || matches!(line.as_bytes(), [b'0'..=b'9', b'.', b' ', ..])
        {
            list_items += 1;
            if list_items >= 2 { return true }
        }
    }
    false
}

impl Bot {
    pub(super) async fn consider_group(&mut self, ctx: &ChatCtx) -> Result<()> {
        self.floor.considered(&ctx.key);
        let lines = self.unseen.get(&ctx.key).cloned().unwrap_or_default();
        if lines.is_empty() {
            return Ok(());
        }
        let joined = self.media.render(&render_unseen(&lines, false), &[]);
        let slang = self.slang_note(ctx, &joined, Utc::now()).map(|n| format!("{n}\n\n")).unwrap_or_default();
        let user = format!(
            "（这不是聊天回复，是你在想要不要在群里插话。）{joined}\n\n{slang}\
             大多数时候你只是看看。只有话题你真的有东西可说、气氛合适、不会打扰别人时才说。\
             只输出一个 JSON 对象：{{\"speak\": true 或 false, \"reason\": \"一句话\"}}"
        );
        let chime: Chime = self.models.ask_structured("triage", &ProcessingPolicy::Any, self.speaker.l1(), &user, &chime_schema()).await?;
        info!("floor in {}: speak={} ({})", ctx.key, chime.speak, chime.reason);
        if chime.speak {
            self.converse_group(ctx, true).await?;
        }
        Ok(())
    }

    /// The lexicon's slang line for a group chat's text; private chats get none.
    pub(super) fn slang_note(&mut self, ctx: &ChatCtx, text: &str, now: DateTime<Utc>) -> Option<String> {
        let Namespace::Group(group) = &ctx.ns else { return None };
        self.lexicon.refresh(self.store.as_ref(), now);
        self.lexicon.slang_note(group, text)
    }

    pub(super) async fn converse_group(&mut self, ctx: &ChatCtx, unsolicited: bool) -> Result<()> {
        let lines = self.unseen.remove(&ctx.key).unwrap_or_default();
        let n = lines.len();
        let mut body = render_unseen(&lines, !unsolicited);
        if unsolicited {
            body.push_str("\n\n（没人叫你，你觉得可以插一句。接最近的话题，别翻旧话；说的时候要自然；改主意就只回复 [沉默]。）");
        } else {
            body.push_str("\n\n（有人点名叫你，这次一定要回应带 → 的消息，不能用 [沉默]；之前的消息只是背景，别逐条回应。\
                           没什么要说的话，只回一个表情也行。）");
        }
        self.converse(ctx, body, n, unsolicited).await
    }

    async fn format_call(&self, policy: &ProcessingPolicy, prompt: String) -> Result<FormatResult> {
        #[cfg(test)]
        if let Some(test) = &self.test_chat {
            test.step_prompts.lock().unwrap().push((policy.clone(), prompt));
            test.step_images.lock().unwrap().push(0);
            let raw = test.steps.lock().unwrap().pop_front().unwrap_or_else(|| Ok("[原样]".into()))?;
            if let Ok(result) = serde_json::from_str(&raw) { return Ok(result) }
            let action = match raw.trim() {
                "[不发]" | "[沉默]" => FormatAction::Drop,
                "" | "[原样]" | "[不用]" => FormatAction::Keep,
                _ => FormatAction::Replace,
            };
            return Ok(FormatResult { action, text: raw });
        }
        self.models.ask_structured("format", policy, "", &prompt, &format_schema()).await
    }

    /// Clean up Markdown and enforce the group cap before delivery.
    pub(super) async fn format_reply(&mut self, ctx: &ChatCtx, unsolicited: bool, may_quit: bool, policy: &ProcessingPolicy, pieces: Vec<ReplyPiece>) -> Option<Vec<ReplyPiece>> {
        let cap = ctx.is_group.then(|| if unsolicited { self.cfg.social.interjection_max } else { self.cfg.social.group_reply_max });
        if !needs_format(&reply_text(&pieces), cap) { return Some(pieces) }
        let length = match cap {
            Some(cap) => format!("不超过 {cap} 字（汉字每字计 1，其他连续字母或数字计 1，标点不计），超了就只留最要紧的话，不加新内容"),
            None => "长短保持原意，不加新内容".to_string(),
        };
        let quit = if may_quit { "觉得这条其实不该发，就用 drop。" } else { "这条必须发出，不能用 drop。" };
        let prompt = format!(
            "下面 <草稿> 里是一个聊天机器人马上要在{where_}发出的消息。把它整理成最终要发的消息。\
             {PLAIN_TEXT}\
             要求：保持原来的语气、人称和意思，像聊天打字那样自然；{length}；可以用空行分成几条消息；\
             原样保留 <提及>名字</提及>，别写其他标记。\
             只输出 JSON 对象，包含 action 和 text。原文合适就用 keep，修改就用 replace 并在 text 中写正文。\
             keep 和 drop 的 text 留空。{quit}\n\n<草稿>\n{draft}\n</草稿>",
            where_ = if ctx.is_group { "群聊里" } else { "私聊里" },
            draft = markup(&pieces),
        );
        let mut pieces = match self.format_call(policy, prompt).await {
            Ok(FormatResult { action: FormatAction::Drop, .. }) if may_quit => {
                info!("took the reply back while formatting in {}", ctx.key);
                return None;
            }
            Ok(FormatResult { action: FormatAction::Replace, text }) => {
                let formatted = parse_intent(text.trim());
                if formatted.silent || formatted.pieces.is_empty() { pieces } else { formatted.pieces }
            }
            Ok(_) => pieces,
            Err(e) => {
                warn!(chat = %ctx.key, "formatting failed, sending the draft: {e:#}");
                pieces
            }
        };
        let Some(cap) = cap else { return Some(pieces) };
        let text = reply_text(&pieces);
        let (units, _) = measure_reply(&text, cap);
        if units > cap {
            warn!(chat = %ctx.key, units, cap, "group reply still over cap after formatting; truncating");
            let truncated = truncate_reply(&text, cap);
            if text.starts_with(&truncated) {
                pieces = reply_prefix(&pieces, truncated.len());
            } else {
                // truncate_reply adds an ellipsis only after a literal prefix.
                pieces = reply_prefix(&pieces, truncated.len() - '…'.len_utf8());
                pieces.push(ReplyPiece::Text("…".into()));
            }
        }
        Some(pieces)
    }

    pub(super) async fn send_bubbles(&self, ctx: &ChatCtx, pieces: &[ReplyPiece]) -> Outcome {
        #[cfg(test)]
        let text = reply_text(pieces);
        let reply_to = if ctx.is_group { ctx.message_id.as_deref() } else { None };
        #[cfg(test)]
        if let Some(test) = &self.test_chat {
            test.sent_texts.lock().unwrap().push(text.to_string());
            test.reply_targets.lock().unwrap().extend(bubbles(&text).iter().enumerate()
                .map(|(i, _)| if i == 0 { reply_to.map(str::to_owned) } else { None }));
            return test.sends.lock().unwrap().pop_front().expect("unexpected send");
        }
        for (i, body) in reply_bubbles(pieces).iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(Duration::from_millis(700)).await;
            }
            if let Err(e) = self.outlets.send_reply(ctx.platform, &ctx.chat_id, body, if i == 0 { reply_to } else { None }).await {
                error!("send failed: {e:#}");
                return if i == 0 { Outcome::Failed } else { Outcome::Unknown };
            }
        }
        Outcome::Sent
    }

    pub(super) async fn send_plain(&self, ctx: &ChatCtx, text: &str) -> Result<()> {
        #[cfg(test)]
        if let Some(test) = &self.test_chat {
            test.sent_texts.lock().unwrap().push(text.to_string());
            let outcome = test.sends.lock().unwrap().pop_front().expect("unexpected plain send");
            if outcome != Outcome::Sent {
                return Err(anyhow::anyhow!("plain delivery: {outcome:?}").context(Fault::Delivery));
            }
            return Ok(());
        }
        self.outlets.send(ctx.platform, &ctx.chat_id, text).await.context(Fault::Delivery)
    }
}
