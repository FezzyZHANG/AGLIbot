//! QQ via a OneBot v11 forward WebSocket (NapCat). One task owns the socket and reconnects with
//! backoff; senders talk to it through a channel and get the action's response back by `echo`.
//!
//! Personal-account QQ bots go through an unofficial protocol: use a secondary account (DESIGN 7).
//!
//! Inbound events pass through one ordered enrichment step before reaching the main loop: merged
//! forwards are fetched there with `get_forward_msg` (NapCat may not inline them; its
//! `parseMultMsg` is off), through the same request channel, so the socket task never waits on
//! its own reply and messages keep their order.
//!
//! Segments become model-facing text like this (array post format, NapCat 4.x):
//!
//! | type | text |
//! |---|---|
//! | text | as is |
//! | at | `@名字 `; @him sets `mentions_me` instead; `all` is `@全体成员 ` |
//! | reply | `[回复 名字: 前40字…]`, else `[回复]`; replies to him set `mentions_me`; hidden in forwards; images in the quoted message, or the nearest earlier one it quotes, become attachments |
//! | face | `[表情:微笑]` from `raw.faceText`, else `[表情]` |
//! | mface | `[表情:summary]` (market sticker) |
//! | image | `[图片]`; stickers (`sub_type` 1) `[动画表情]` |
//! | record / video | `[语音]` / `[视频]` |
//! | file | `[文件:name size]` |
//! | forward | `[合并转发 N条]` then `名字: 内容` lines, nested two levels, capped |
//! | json | `[卡片:prompt title desc url]` from the app card |
//! | xml | `[卡片]` |
//! | markdown | `[markdown:content]` |
//! | poke / dice / rps | `[戳一戳]` / `[骰子:n]` / `[猜拳:n]` |
//! | contact | `[推荐好友:id]` / `[推荐群:id]` |
//! | location | `[位置:title content]` |
//! | music / share | `[音乐:title]` / `[链接:title url]` |
//! | anything else | `[type]`, logged once per type |
//!
//! Notices use the same inbound gates and transcript as messages:
//!
//! | notice | text |
//! |---|---|
//! | group_recall / friend_recall | `[撤回了一条消息]`, with cached first 40 characters when available |
//! | notify/poke at self | `[戳了戳你]`, addressed; once per sender/chat per 10 minutes |
//! | group_increase / group_decrease | `[入群]` / `[退群]`, or `[被移出群]` for kick |
//! | group_card / other notices | ignored |
//!
//! CQ codes are never interpreted, in or out.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tracing::{info, warn};

use super::{Attachment, AttachmentKind, Envelope, ReplyPiece, reply_prefix, reply_text};

const CHUNK_CHARS: usize = 1500;
const ACTION_TIMEOUT: Duration = Duration::from_secs(20);
/// How many of his own message ids to remember, to recognise replies to him.
const OWN_MESSAGES: usize = 300;
const RECENT_MESSAGES: usize = 500;
/// Forwards nested deeper than this show only their header.
const FORWARD_DEPTH: usize = 2;
/// Per forward: nodes and characters shown before `…(省略 K 条)`.
const FORWARD_NODES: usize = 40;
const FORWARD_CHARS: usize = 2000;
/// Per node, after rendering.
const NODE_CHARS: usize = 500;
/// `get_forward_msg` calls per inbound message, and how long each may take: the inbound line
/// waits on them.
const FORWARD_FETCHES: usize = 6;
const FORWARD_TIMEOUT: Duration = Duration::from_secs(8);
/// `get_msg` calls per quote chain: the inbound line waits on them too.
const QUOTE_FETCHES: usize = 3;
/// Card fields beyond this are cut.
const CARD_CHARS: usize = 200;

struct Request {
    action: &'static str,
    params: Value,
    reply: oneshot::Sender<Result<Value>>,
}

#[derive(Default)]
struct Shared {
    self_id: AtomicI64,
    last_inbound: Mutex<HashMap<String, Option<String>>>,
    /// His QQ nickname, for self-authored forward nodes.
    nickname: Mutex<Option<String>>,
    pokes: Mutex<HashMap<(String, i64), tokio::time::Instant>>,
    own_messages: Mutex<VecDeque<String>>,
    recent: Mutex<VecDeque<(String, Quoted)>>,
    /// Unknown segment types already logged.
    unknown_types: Mutex<HashSet<String>>,
}

#[derive(Clone)]
struct Quoted {
    name: String,
    text: String,
    images: Vec<Attachment>,
}

impl Shared {
    fn cache(&self, id: String, message: Quoted) {
        let mut q = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        q.retain(|(key, _)| key != &id);
        q.push_back((id, message));
        if q.len() > RECENT_MESSAGES {
            q.pop_front();
        }
    }

    fn cached(&self, id: &str) -> Option<Quoted> {
        let mut q = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        let i = q.iter().position(|(key, _)| key == id)?;
        let entry = q.remove(i)?;
        let message = entry.1.clone();
        q.push_back(entry);
        Some(message)
    }

    fn note_unknown(&self, kind: &str) {
        if self.unknown_types.lock().unwrap_or_else(|e| e.into_inner()).insert(kind.to_string()) {
            info!("qq: unhandled segment type `{kind}`, shown as [{kind}]");
        }
    }

    fn remember(&self, id: String) {
        let mut q = self.own_messages.lock().unwrap_or_else(|e| e.into_inner());
        q.push_back(id);
        while q.len() > OWN_MESSAGES {
            q.pop_front();
        }
    }

    fn is_mine(&self, id: &str) -> bool {
        self.own_messages.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|m| m == id)
    }
}

#[derive(Clone)]
pub struct QqHandle {
    tx: mpsc::Sender<Request>,
    shared: Arc<Shared>,
}

/// Start the connection task. Inbound messages go to `inbound`.
pub fn spawn(url: String, token: Option<String>, inbound: mpsc::Sender<Envelope>) -> QqHandle {
    let (tx, rx) = mpsc::channel(64);
    // If this fills while a forward is being fetched, the socket task waits too; the fetch ends
    // at FORWARD_TIMEOUT, so it never waits on its own reply forever.
    let (raw_tx, raw_rx) = mpsc::channel(256);
    let shared = Arc::new(Shared::default());
    let qq = QqHandle { tx, shared: shared.clone() };
    tokio::spawn(run(url, token, rx, raw_tx, shared));
    tokio::spawn(enrich(raw_rx, qq.clone(), inbound));
    qq
}

/// Resolve what an event needs from NapCat, in arrival order, then hand it on.
async fn enrich(mut raw: mpsc::Receiver<Pending>, qq: QqHandle, inbound: mpsc::Sender<Envelope>) {
    while let Some(p) = raw.recv().await {
        // Record arrival before potentially slow reply/forward lookups; notices never replace it.
        if !p.env.notice {
            qq.shared.last_inbound.lock().unwrap_or_else(|e| e.into_inner())
                .insert(p.env.chat_id.clone(), p.env.message_id.clone());
        }
        let forwards = if has_forward(&p.message) {
            collect_forwards(&p.message, |id| {
                let qq = qq.clone();
                async move { qq.forward_nodes(&id).await }
            })
            .await
        } else {
            Forwards::new()
        };
        let (reply, quoted_images) = resolve_reply(&p.message, &qq.shared, |id| {
            let qq = qq.clone();
            async move { qq.get_message(&id).await }
        })
        .await;
        if let Some(mut env) = finish(p, &forwards, &qq.shared, &reply) {
            if let Some(id) = &env.message_id {
                // Cache the body only, so quoting a reply does not nest prefixes.
                let body = env.text.strip_prefix(reply.as_str()).unwrap_or(&env.text).trim().to_string();
                // Its own images, else the ones it quoted, so a later quote of it finds them.
                let own: Vec<_> = env.attachments.iter().filter(|a| is_image(a)).cloned().collect();
                let images = if own.is_empty() { quoted_images.clone() } else { own };
                qq.shared.cache(id.clone(), Quoted { name: env.sender_name.clone(), text: body, images });
            }
            env.attachments.extend(quoted_images);
            if inbound.send(env).await.is_err() {
                return;
            }
        }
    }
}

impl QqHandle {
    pub fn self_id(&self) -> i64 {
        self.shared.self_id.load(Ordering::Relaxed)
    }

    async fn call(&self, action: &'static str, params: Value) -> Result<Value> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Request { action, params, reply }).await.map_err(|_| anyhow!("qq task stopped"))?;
        tokio::time::timeout(ACTION_TIMEOUT, rx)
            .await
            .map_err(|_| anyhow!("qq action {action} timed out"))?
            .map_err(|_| anyhow!("qq connection dropped during {action}"))?
    }

    /// `chat_id` is `private:<user>` or `group:<group>`. Text goes as a text segment, so CQ codes
    /// in model output are never interpreted.
    pub async fn send(&self, chat_id: &str, text: &str) -> Result<()> {
        self.send_reply(chat_id, &[ReplyPiece::Text(text.into())], None).await
    }

    pub async fn send_reply(&self, chat_id: &str, pieces: &[ReplyPiece], reply_to: Option<&str>) -> Result<()> {
        let (action, forward_action, key, id) = match chat_id.split_once(':') {
            Some(("private", id)) => ("send_private_msg", "send_private_forward_msg", "user_id", id),
            Some(("group", id)) => ("send_group_msg", "send_group_forward_msg", "group_id", id),
            _ => bail!("bad qq chat id `{chat_id}`"),
        };
        let id: i64 = id.parse().with_context(|| format!("bad qq chat id `{chat_id}`"))?;
        let text = reply_text(pieces);
        let chunks = super::telegram::chunks(&text, CHUNK_CHARS);
        if chunks.len() > 2 {
            let nickname = self.nickname().await;
            let params = forward_params(key, id, self.shared.self_id.load(Ordering::Relaxed), &nickname, &chunks);
            match self.call(forward_action, params).await {
                Ok(data) => {
                    self.cache_sent(&data, &chunks[0]);
                    return Ok(());
                }
                Err(e) => warn!("qq forward failed, falling back to chunks: {e:#}"),
            }
        }
        for (i, chunk) in chunks.iter().enumerate() {
            let quote = if i == 0 && key == "group_id" && let Some(target) = reply_to {
                let last = self.shared.last_inbound.lock().unwrap_or_else(|e| e.into_inner());
                (last.get(chat_id).and_then(|id| id.as_deref()) != Some(target)).then_some(target)
            } else { None };
            // Forward messages (including failure fallback) and later chunks stay plain.
            let body = if i == 0 && chunks.len() <= 2 {
                reply_prefix(pieces, chunk.len())
            } else { vec![ReplyPiece::Text(chunk.clone())] };
            let data = self.call(action, reply_params(key, id, self.self_id(), &body, quote)).await?;
            self.cache_sent(&data, chunk);
        }
        Ok(())
    }

    /// Asked once; an empty name lets NapCat fall back to the account's own.
    async fn nickname(&self) -> String {
        if let Some(n) = self.shared.nickname.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return n;
        }
        let Ok(info) = self.call("get_login_info", json!({})).await else { return String::new() };
        let n = info.get("nickname").and_then(Value::as_str).unwrap_or_default().to_string();
        *self.shared.nickname.lock().unwrap_or_else(|e| e.into_inner()) = Some(n.clone());
        n
    }

    fn cache_sent(&self, data: &Value, text: &str) {
        if let Some(mid) = data.get("message_id") {
            self.shared.remember(id_string(mid));
            self.shared.cache(id_string(mid), Quoted { name: "我".into(), text: text.to_string(), images: Vec::new() });
        }
    }

    async fn get_message(&self, id: &str) -> Result<Value> {
        tokio::time::timeout(FORWARD_TIMEOUT, self.call("get_msg", json!({ "message_id": id }))).await.map_err(|_| anyhow!("get_msg timed out"))?
    }

    /// A fresh download URL for a received image.
    pub async fn image_url(&self, file: &str) -> Result<String> {
        let data = self.call("get_image", json!({ "file": file })).await?;
        data.get("url").and_then(Value::as_str).filter(|url| !url.is_empty()).map(String::from)
            .ok_or_else(|| anyhow!("get_image returned no url"))
    }

    /// Groups the bot account is in.
    pub async fn group_ids(&self) -> Result<Vec<i64>> {
        let data = self.call("get_group_list", json!({})).await?;
        Ok(data.as_array().into_iter().flatten().filter_map(|g| g.get("group_id")).filter_map(|id| id_string(id).parse().ok()).collect())
    }

    /// NapCat fails the call for a non-member; any failure counts as not (yet) known.
    pub async fn in_group(&self, group_id: i64, user_id: i64) -> bool {
        self.call("get_group_member_info", json!({ "group_id": group_id, "user_id": user_id })).await.is_ok()
    }

    /// The nodes of a merged forward.
    async fn forward_nodes(&self, id: &str) -> Result<Vec<Value>> {
        let data = tokio::time::timeout(FORWARD_TIMEOUT, self.call("get_forward_msg", json!({ "message_id": id })))
            .await
            .map_err(|_| anyhow!("get_forward_msg timed out"))??;
        match data.get("messages").or_else(|| data.get("message")) {
            Some(Value::Array(nodes)) => Ok(nodes.clone()),
            _ => bail!("get_forward_msg returned no messages: {data}"),
        }
    }
}

fn reply_params(key: &str, id: i64, self_id: i64, pieces: &[ReplyPiece], quote: Option<&str>) -> Value {
    let mut message = Vec::new();
    if key == "group_id" && let Some(target) = quote {
        message.push(json!({"type": "reply", "data": {"id": target}}));
    }
    let (mut text, mut mentions) = (String::new(), 0);
    for piece in pieces {
        if key == "group_id" && mentions < 2 && self_id > 0
            && let ReplyPiece::Mention { qq_id: Some(qq), .. } = piece
            && *qq > 0 && *qq != self_id {
            if !text.is_empty() {
                message.push(json!({"type": "text", "data": {"text": std::mem::take(&mut text)}}));
            }
            message.push(json!({"type": "at", "data": {"qq": qq.to_string()}}));
            mentions += 1;
        } else {
            text.push_str(&piece.plain());
        }
    }
    if !text.is_empty() {
        message.push(json!({"type": "text", "data": {"text": text}}));
    }
    json!({key: id, "message": message})
}

/// Only explicit text segments: model output cannot create CQ segments.
fn forward_params(key: &str, id: i64, self_id: i64, nickname: &str, chunks: &[String]) -> Value {
    let nodes: Vec<_> = chunks.iter().map(|chunk| json!({
        "type": "node", "data": {"user_id": self_id, "nickname": nickname,
            "content": [{"type": "text", "data": {"text": chunk}}]}
    })).collect();
    json!({key: id, "messages": nodes})
}

fn id_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

async fn run(
    url: String,
    token: Option<String>,
    mut rx: mpsc::Receiver<Request>,
    inbound: mpsc::Sender<Pending>,
    shared: Arc<Shared>,
) {
    let mut backoff = Duration::from_secs(2);
    loop {
        match connect(&url, token.as_deref()).await {
            Ok(ws) => {
                info!("qq connected to {url}");
                backoff = Duration::from_secs(2);
                match session(ws, &mut rx, &inbound, &shared).await {
                    Ok(()) => return, // all senders and the inbound side are gone
                    Err(e) => warn!("qq connection lost: {e:#}"),
                }
            }
            Err(e) => warn!("qq connect failed: {e:#}"),
        }
        // Fail requests queued while disconnected instead of letting them hang.
        while let Ok(req) = rx.try_recv() {
            let _ = req.reply.send(Err(anyhow!("qq is disconnected")));
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(url: &str, token: Option<&str>) -> Result<Ws> {
    let mut req = url.into_client_request()?;
    if let Some(t) = token {
        req.headers_mut().insert("Authorization", format!("Bearer {t}").parse()?);
    }
    let (ws, _) = tokio_tungstenite::connect_async(req).await?;
    Ok(ws)
}

async fn session(
    ws: Ws,
    rx: &mut mpsc::Receiver<Request>,
    inbound: &mpsc::Sender<Pending>,
    shared: &Shared,
) -> Result<()> {
    let (mut sink, mut stream) = ws.split();
    let mut pending: HashMap<String, oneshot::Sender<Result<Value>>> = HashMap::new();
    let seq = AtomicU64::new(0);
    let result = loop {
        tokio::select! {
            req = rx.recv() => {
                let Some(req) = req else { break Ok(()) };
                let echo = format!("aglibot-{}", seq.fetch_add(1, Ordering::Relaxed));
                let frame = json!({"action": req.action, "params": req.params, "echo": echo});
                if let Err(e) = sink.send(Message::text(frame.to_string())).await {
                    let _ = req.reply.send(Err(anyhow!("qq send failed: {e}")));
                    break Err(e.into());
                }
                pending.insert(echo, req.reply);
            }
            msg = stream.next() => {
                let text = match msg {
                    None => break Err(anyhow!("socket closed")),
                    Some(Err(e)) => break Err(e.into()),
                    Some(Ok(Message::Text(t))) => t.to_string(),
                    Some(Ok(Message::Ping(p))) => { let _ = sink.send(Message::Pong(p)).await; continue; }
                    Some(Ok(Message::Close(_))) => break Err(anyhow!("server closed the socket")),
                    Some(Ok(_)) => continue,
                };
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                if let Some(echo) = v.get("echo").and_then(Value::as_str) {
                    if let Some(reply) = pending.remove(echo) {
                        let ok = v.get("status").and_then(Value::as_str) == Some("ok");
                        let _ = reply.send(if ok { Ok(v["data"].clone()) } else { Err(anyhow!("qq action failed: {v}")) });
                    }
                    continue;
                }
                if let Some(id) = v.get("self_id").and_then(Value::as_i64) {
                    shared.self_id.store(id, Ordering::Relaxed);
                }
                if let Some(p) = parse_event(&v, shared)
                    && inbound.send(p).await.is_err()
                {
                    break Ok(());
                }
            }
        }
    };
    for (_, reply) in pending.drain() {
        let _ = reply.send(Err(anyhow!("qq connection dropped")));
    }
    result
}

/// A message event whose text is not rendered yet: forwards may need fetching first.
struct Pending {
    env: Envelope,
    message: Vec<Value>,
    recalled_id: Option<String>,
}

/// Merged-forward nodes by forward id; `None` when fetching failed.
type Forwards = HashMap<String, Option<Vec<Value>>>;

fn parse_event(v: &Value, shared: &Shared) -> Option<Pending> {
    match v.get("post_type")?.as_str()? {
        "message" => {}
        "notice" => return parse_notice(v, shared),
        _ => return None,
    }
    let self_id = shared.self_id.load(Ordering::Relaxed);
    let user_id = v.get("user_id")?.as_i64()?;
    if user_id == self_id {
        return None;
    }
    let is_group = v.get("message_type")?.as_str()? == "group";
    let chat_id = if is_group {
        format!("group:{}", v.get("group_id")?.as_i64()?)
    } else {
        format!("private:{user_id}")
    };
    let message = v.get("message")?.as_array()?.clone();
    let name = sender_name(v.get("sender").unwrap_or(&Value::Null)).unwrap_or_default();
    Some(Pending {
        env: Envelope {
            notice: false, arrived: Some(tokio::time::Instant::now()),
            platform: "qq",
            chat_id,
            message_id: v.get("message_id").filter(|id| !id.is_null()).map(id_string),
            sender_id: user_id.to_string(),
            sender_name: name,
            sender_is_bot: false,
            is_group,
            mentions_me: !is_group,
            text: String::new(),
            attachments: attachments(&message),
        },
        message,
        recalled_id: None,
    })
}

/// Notice bodies are finished in the ordered enrichment task, after preceding messages enter the LRU.
fn parse_notice(v: &Value, shared: &Shared) -> Option<Pending> {
    let self_id = shared.self_id.load(Ordering::Relaxed);
    let member = v.get("user_id")?.as_i64()?;
    if member == self_id { return None; }
    let kind = v.get("notice_type")?.as_str()?;
    let is_group = v.get("group_id").and_then(Value::as_i64).is_some_and(|id| id != 0);
    let chat_id = if is_group { format!("group:{}", v["group_id"].as_i64()?) } else { format!("private:{member}") };
    let (sender, text, mentions_me) = match kind {
        "group_recall" | "friend_recall" => (
            v.get("operator_id").and_then(Value::as_i64).unwrap_or(member), "[撤回了一条消息]", false),
        "notify" if v["sub_type"] == "poke" && v["target_id"].as_i64() == Some(self_id) => {
            let now = tokio::time::Instant::now();
            let mut pokes = shared.pokes.lock().unwrap_or_else(|e| e.into_inner());
            pokes.retain(|_, t| now.duration_since(*t) < Duration::from_secs(600));
            if pokes.contains_key(&(chat_id.clone(), member)) { return None; }
            pokes.insert((chat_id.clone(), member), now);
            (member, "[戳了戳你]", true)
        }
        "group_increase" => (member, "[入群]", false),
        "group_decrease" => (member, if v["sub_type"] == "kick" { "[被移出群]" } else { "[退群]" }, false),
        "group_card" | "group_upload" | "group_admin" | "group_ban" | "friend_add" | "notify" => return None,
        _ => return None,
    };
    Some(Pending {
        env: Envelope {
            notice: true, platform: "qq", chat_id, message_id: None,
            // No name: people.observe would rename the person to their id.
            sender_id: sender.to_string(), sender_name: String::new(), sender_is_bot: false,
            is_group, mentions_me, text: text.into(), attachments: Vec::new(),
            arrived: Some(tokio::time::Instant::now()),
        },
        message: Vec::new(),
        recalled_id: matches!(kind, "group_recall" | "friend_recall")
            .then(|| v.get("message_id").filter(|id| !id.is_null()).map(id_string)).flatten(),
    })
}

/// Collect only this message's segments, never forward nodes or quoted messages.
fn attachments(message: &[Value]) -> Vec<Attachment> {
    message.iter().filter_map(|seg| {
        let data = &seg["data"];
        let field = |key: &str| match data.get(key)? {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        };
        let kind = match seg.get("type")?.as_str()? {
            "image" if field("sub_type").as_deref() == Some("1") => AttachmentKind::Sticker,
            "image" => AttachmentKind::Image,
            "mface" => AttachmentKind::Sticker,
            "record" => AttachmentKind::Voice,
            "video" => AttachmentKind::Video,
            "file" => AttachmentKind::File,
            _ => return None,
        };
        let file_id = if seg["type"] == "mface" {
            None
        } else {
            field("file_id").or_else(|| field("file_unique")).or_else(|| field("file"))
        };
        Some(Attachment {
            kind,
            url: field("url"),
            file_id,
            name: field("name").or_else(|| (seg["type"] == "file").then(|| field("file")).flatten()),
            size: field("file_size").and_then(|s| s.parse().ok()),
            quoted_from: None,
        })
    }).collect()
}

fn is_image(attachment: &Attachment) -> bool {
    matches!(attachment.kind, AttachmentKind::Image | AttachmentKind::Sticker)
}

fn images_in(segments: &[Value]) -> Vec<Attachment> {
    attachments(segments).into_iter().filter(is_image).collect()
}

/// Render the text; `None` when nothing readable is left.
fn finish(mut p: Pending, forwards: &Forwards, shared: &Shared, reply: &str) -> Option<Envelope> {
    if p.env.notice {
        if let Some(id) = p.recalled_id {
            let mut recent = shared.recent.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = recent.iter().position(|(key, _)| key == &id) {
                let (_, quoted) = recent.remove(i)?;
                p.env.text = format!("[撤回了一条消息: {}]", cut(&quoted.text.replace("\r\n", " ").replace(['\r', '\n'], " "), 40));
            }
        }
        return Some(p.env);
    }
    let r = Render { self_id: shared.self_id.load(Ordering::Relaxed).to_string(), shared, forwards };
    let mut mentions_me = false;
    let body = r.segments(&p.message, 0, Some(&mut mentions_me));
    let text = format!("{reply}{}", body.trim());
    if text.is_empty() {
        return None;
    }
    Some(Envelope { text, mentions_me: p.env.mentions_me || mentions_me, ..p.env })
}

fn reply_segment(message: &[Value]) -> Option<&Value> {
    message.iter().find(|s| s.get("type").and_then(Value::as_str) == Some("reply"))
}

fn reply_id(message: &[Value]) -> Option<String> {
    reply_segment(message)?.pointer("/data/id").filter(|id| !id.is_null()).map(id_string).filter(|id| !id.is_empty())
}

fn quoted_name(data: &Value, id: &str, shared: &Shared) -> String {
    let me = shared.self_id.load(Ordering::Relaxed).to_string();
    if shared.is_mine(id) || data.pointer("/sender/user_id").map(id_string).as_deref() == Some(&me) {
        "我".into()
    } else {
        node_name(data)
    }
}

/// Walk back from a quoted message with no images to the nearest earlier one that has some, keeping
/// who first sent them. `fetched` calls are already spent.
async fn earlier_images<F, Fut>(segments: &[Value], shared: &Shared, fetch: &mut F, mut fetched: usize) -> Vec<Attachment>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    let mut next = reply_id(segments);
    while let Some(id) = next {
        if let Some(cached) = shared.cached(&id) {
            let from = Some(cached.name).filter(|name| !name.is_empty());
            return cached.images.into_iter().map(|a| Attachment { quoted_from: a.quoted_from.or_else(|| from.clone()), ..a }).collect();
        }
        if fetched >= QUOTE_FETCHES { break; }
        fetched += 1;
        let data = match fetch(id.clone()).await {
            Ok(data) => data,
            Err(e) => {
                warn!("qq: quoted message {id} unreadable: {e:#}");
                break;
            }
        };
        let segments = node_segments(&data);
        let images = images_in(&segments);
        if !images.is_empty() {
            let from = Some(quoted_name(&data, &id, shared));
            return images.into_iter().map(|a| Attachment { quoted_from: from.clone(), ..a }).collect();
        }
        next = reply_id(&segments);
    }
    Vec::new()
}

/// The quote's text prefix, and the images in the quoted message.
async fn resolve_reply<F, Fut>(message: &[Value], shared: &Shared, mut fetch: F) -> (String, Vec<Attachment>)
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    if reply_segment(message).is_none() {
        return (String::new(), Vec::new());
    }
    let Some(id) = reply_id(message) else {
        return ("[回复]".into(), Vec::new());
    };
    let quoted = match shared.cached(&id) {
        Some(message) => Some(message),
        None => match fetch(id.clone()).await {
            Ok(data) if data.get("message").is_some_and(|m| m.is_array() || m.is_string()) => {
                let forwards = Forwards::new();
                let r = Render { self_id: shared.self_id.load(Ordering::Relaxed).to_string(), shared, forwards: &forwards };
                let name = quoted_name(&data, &id, shared);
                let segments = node_segments(&data);
                let text = r.segments(&segments, FORWARD_DEPTH, None).trim().to_string();
                let mut images = images_in(&segments);
                if images.is_empty() {
                    images = earlier_images(&segments, shared, &mut fetch, 1).await;
                }
                let message = Quoted { name, text, images };
                shared.cache(id, message.clone());
                Some(message)
            }
            Ok(_) => None,
            Err(e) => {
                warn!("qq: reply {id} unreadable: {e:#}");
                None
            }
        },
    };
    match quoted {
        Some(q) => {
            let from = Some(q.name.clone()).filter(|name| !name.is_empty());
            let images = q.images.iter().map(|a| Attachment { quoted_from: a.quoted_from.clone().or_else(|| from.clone()), ..a.clone() }).collect();
            (format!("[回复 {}: {}]", q.name, cut(&q.text.replace("\r\n", " ").replace(['\r', '\n'], " "), 40)), images)
        }
        None => ("[回复]".into(), Vec::new()),
    }
}

fn sender_name(sender: &Value) -> Option<String> {
    [sender.get("card"), sender.get("nickname")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

fn has_forward(segs: &[Value]) -> bool {
    segs.iter().any(|s| s.get("type").and_then(Value::as_str) == Some("forward"))
}

/// Inline forward content, when NapCat put it in the segment.
fn inline_nodes(data: &Value) -> Option<&Vec<Value>> {
    data.get("content").and_then(Value::as_array)
}

/// The segments of one forward node: `get_forward_msg` gives whole messages, other shapes use
/// `content` or a `node` segment's `data.content`; string messages become a text segment.
fn node_segments(node: &Value) -> Vec<Value> {
    let body = node.get("message").or_else(|| node.get("content")).or_else(|| node.pointer("/data/content"));
    match body {
        Some(Value::Array(segs)) => segs.clone(),
        Some(Value::String(s)) => vec![json!({"type": "text", "data": {"text": s}})],
        _ => Vec::new(),
    }
}

fn node_name(node: &Value) -> String {
    node.get("sender")
        .and_then(sender_name)
        .or_else(|| ["nickname", "name"].iter().find_map(|k| node.pointer(&format!("/data/{k}"))?.as_str().map(str::to_string)))
        .or_else(|| node.get("user_id").map(id_string))
        .unwrap_or_else(|| "?".into())
}

/// Fetch the forwards a message refers to, two levels deep, inline content first.
async fn collect_forwards<F, Fut>(message: &[Value], mut fetch: F) -> Forwards
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Vec<Value>>>,
{
    let mut out = Forwards::new();
    let mut fetches = 0;
    let mut work = vec![(message.to_vec(), 1)];
    while let Some((segs, level)) = work.pop() {
        for seg in segs.iter().filter(|s| s.get("type").and_then(Value::as_str) == Some("forward")) {
            let data = &seg["data"];
            let id = id_string(&data["id"]);
            if out.contains_key(&id) {
                continue;
            }
            let nodes = match inline_nodes(data) {
                Some(nodes) => Some(nodes.clone()),
                None if fetches < FORWARD_FETCHES => {
                    fetches += 1;
                    match fetch(id.clone()).await {
                        Ok(nodes) => Some(nodes),
                        Err(e) => {
                            warn!("qq: forward {id} unreadable: {e:#}");
                            None
                        }
                    }
                }
                None => continue,
            };
            if level < FORWARD_DEPTH {
                work.extend(nodes.iter().flatten().map(|n| (node_segments(n), level + 1)));
            }
            out.insert(id, nodes);
        }
    }
    out
}

struct Render<'a> {
    self_id: String,
    shared: &'a Shared,
    forwards: &'a Forwards,
}

impl Render<'_> {
    /// `level` counts enclosing forwards. Only top-level @ and replies can mention him.
    fn segments(&self, segs: &[Value], level: usize, mut mentions_me: Option<&mut bool>) -> String {
        let mut text = String::new();
        for seg in segs {
            let data = &seg["data"];
            let field = |k: &str| data.get(k).map(id_string).filter(|s| !s.is_empty());
            match seg.get("type").and_then(Value::as_str).unwrap_or_default() {
                "text" => text.push_str(data.get("text").and_then(Value::as_str).unwrap_or_default()),
                "at" => {
                    let target = id_string(&data["qq"]);
                    if target == self.self_id {
                        match mentions_me.as_deref_mut() {
                            Some(m) => *m = true,
                            None => text.push_str("@我 "),
                        }
                    } else if target == "all" {
                        text.push_str("@全体成员 ");
                    } else {
                        let name = data.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(&target);
                        text.push_str(&format!("@{name} "));
                    }
                }
                "reply" => {
                    if let Some(m) = mentions_me.as_deref_mut() {
                        *m |= self.shared.is_mine(&id_string(&data["id"]));
                    }
                }
                "face" => match data.pointer("/raw/faceText").and_then(Value::as_str) {
                    Some(t) if !t.is_empty() => text.push_str(&format!("[表情:{}]", t.trim_start_matches('/'))),
                    _ => text.push_str("[表情]"),
                },
                "mface" => match field("summary") {
                    Some(s) => text.push_str(&format!("[表情:{}]", s.trim_matches(['[', ']']))),
                    None => text.push_str("[表情]"),
                },
                "image" => {
                    let sticker = data.get("sub_type").map(id_string).as_deref() == Some("1");
                    text.push_str(if sticker { "[动画表情]" } else { "[图片]" });
                }
                "record" => text.push_str("[语音]"),
                "video" => text.push_str("[视频]"),
                "file" => {
                    let name = field("name").or_else(|| field("file"));
                    let size = data.get("file_size").map(id_string).and_then(|s| s.parse::<u64>().ok()).map(human_size);
                    text.push_str(&bracket("文件", [name, size]));
                }
                "forward" => {
                    text.push_str(&self.forward(data, level + 1));
                    text.push('\n');
                }
                "json" => text.push_str(&json_card(data)),
                "xml" => text.push_str("[卡片]"),
                "markdown" => text.push_str(&bracket("markdown", [field("content").map(|c| cut(&c, CARD_CHARS))])),
                "poke" => text.push_str("[戳一戳]"),
                "dice" => text.push_str(&bracket("骰子", [field("result")])),
                "rps" => text.push_str(&bracket("猜拳", [field("result")])),
                "contact" => {
                    let what = if field("type").as_deref() == Some("group") { "推荐群" } else { "推荐好友" };
                    text.push_str(&bracket(what, [field("id")]));
                }
                "location" => text.push_str(&bracket("位置", [field("title"), field("content")])),
                "music" => text.push_str(&bracket("音乐", [field("title")])),
                "share" => text.push_str(&bracket("链接", [field("title"), field("url")])),
                "" => {}
                other => {
                    self.shared.note_unknown(other);
                    text.push_str(&format!("[{other}]"));
                }
            }
        }
        text
    }

    fn forward(&self, data: &Value, level: usize) -> String {
        let id = id_string(&data["id"]);
        let nodes = match (inline_nodes(data), self.forwards.get(&id)) {
            (Some(nodes), _) | (None, Some(Some(nodes))) => nodes,
            (None, Some(None)) => return "[合并转发，读取失败]".into(),
            (None, None) => return "[合并转发]".into(),
        };
        let mut out = format!("[合并转发 {}条]", nodes.len());
        if level > FORWARD_DEPTH {
            return out;
        }
        let mut chars = 0;
        for (i, node) in nodes.iter().enumerate() {
            if i >= FORWARD_NODES || chars >= FORWARD_CHARS {
                out.push_str(&format!("\n…(省略 {} 条)", nodes.len() - i));
                break;
            }
            let body = cut(self.segments(&node_segments(node), level, None).trim(), NODE_CHARS);
            let line = format!("\n{}: {}", node_name(node), body.replace('\n', "\n  "));
            chars += line.chars().count();
            out.push_str(&line);
        }
        out
    }
}

/// `[label:a b]`, or `[label]` when every part is missing.
fn bracket<const N: usize>(label: &str, parts: [Option<String>; N]) -> String {
    let parts: Vec<String> = parts.into_iter().flatten().map(|p| cut(p.trim(), CARD_CHARS)).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() { format!("[{label}]") } else { format!("[{label}:{}]", parts.join(" ")) }
}

/// App cards carry a JSON string: a `prompt` like `[QQ小程序]哔哩哔哩` plus app-specific `meta`.
fn json_card(data: &Value) -> String {
    let card: Value = match data.get("data") {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_default(),
        Some(v) => v.clone(),
        None => Value::Null,
    };
    let str_at = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty());
    let prompt = str_at(&card, "prompt");
    let meta = card.get("meta").and_then(Value::as_object).and_then(|m| m.values().next()).cloned().unwrap_or_default();
    let title = str_at(&meta, "title");
    let desc = str_at(&meta, "desc").filter(|d| Some(d) != title.as_ref());
    let url = ["qqdocurl", "jumpUrl", "url"].iter().find_map(|k| str_at(&meta, k));
    // Prompts often repeat the title: `[QQ小程序]标题`.
    let title = title.filter(|t| prompt.as_ref().is_none_or(|p| !p.contains(t.as_str())));
    bracket("卡片", [prompt, title, desc, url])
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1}GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1}MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{}KB", b >> 10),
        b => format!("{b}B"),
    }
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { format!("{}…", s.chars().take(max).collect::<String>()) }
}

#[cfg(test)]
mod tests {
    use super::*;


    fn notice(kind: &str) -> Value {
        json!({"post_type":"notice", "notice_type":kind, "user_id":12, "group_id":34,
            "operator_id":56, "message_id":7})
    }

    #[test]
    fn recalls_render_operator_and_consume_cache_in_order() {
        for kind in ["group_recall", "friend_recall"] {
            let s = shared(99);
            let mut v = notice(kind);
            if kind == "friend_recall" { v.as_object_mut().unwrap().remove("group_id"); }
            let env = parse_event(&v, &s).unwrap();
            assert_eq!(env.text, "[撤回了一条消息]");
            assert_eq!(env.sender_id, "56");
            assert!(!env.mentions_me && env.notice && env.message_id.is_none());
            assert_eq!(env.is_group, kind == "group_recall");
            // Parsing can precede enrichment of the message being recalled.
            let pending = super::parse_event(&v, &s).unwrap();
            s.cache("7".into(), Quoted { name:"member".into(), text:"字".repeat(41), images: Vec::new() });
            let env = finish(pending, &Forwards::new(), &s, "").unwrap();
            assert_eq!(env.text, format!("[撤回了一条消息: {}…]", "字".repeat(40)));
            assert!(s.cached("7").is_none());
            v["user_id"] = json!(99);
            assert!(parse_event(&v, &s).is_none());
        }
        let mut v = notice("friend_recall");
        v.as_object_mut().unwrap().remove("operator_id");
        v.as_object_mut().unwrap().remove("group_id");
        let env = parse_event(&v, &shared(99)).unwrap();
        assert_eq!(env.sender_id, "12");
        assert_eq!(env.chat_id, "private:12");
    }

    #[test]
    fn membership_notices_and_ignored_types() {
        for (kind, subtype, expected) in [("group_increase", "approve", "[入群]"),
            ("group_decrease", "leave", "[退群]"), ("group_decrease", "kick", "[被移出群]")] {
            let mut v = notice(kind);
            v["sub_type"] = json!(subtype);
            let s = shared(99);
            let env = parse_event(&v, &s).unwrap();
            assert_eq!(env.text, expected);
            assert_eq!(env.sender_id, "12");
            assert!(env.sender_name.is_empty());
            assert_eq!(env.chat_id, "group:34");
            assert!(env.notice && env.is_group && !env.mentions_me);
            v["user_id"] = json!(99);
            assert!(parse_event(&v, &s).is_none());
        }
        for kind in ["group_card", "group_upload", "group_admin", "group_ban", "friend_add", "notify", "unknown"] {
            assert!(parse_event(&notice(kind), &shared(99)).is_none());
        }
        assert!(!parse_event(&group(json!([text("hello")])), &shared(99)).unwrap().notice);
    }

    #[test]
    fn poke_target_and_sender_chat_rate_limit() {
        let s = shared(99);
        let mut v = notice("notify");
        v["sub_type"] = json!("poke");
        v["target_id"] = json!(98);
        assert!(parse_event(&v, &s).is_none());
        v["target_id"] = json!(99);
        let env = parse_event(&v, &s).unwrap();
        assert_eq!(env.text, "[戳了戳你]");
        assert_eq!(env.sender_id, "12");
        assert!(env.notice && env.mentions_me);
        assert!(parse_event(&v, &s).is_none());
        s.pokes.lock().unwrap().insert(("group:34".into(), 12), tokio::time::Instant::now() - Duration::from_secs(600));
        assert!(parse_event(&v, &s).is_some());
        v["user_id"] = json!(13);
        assert!(parse_event(&v, &s).is_some());
        v["group_id"] = json!(35);
        assert!(parse_event(&v, &s).is_some());
        v.as_object_mut().unwrap().remove("group_id");
        assert_eq!(parse_event(&v, &s).unwrap().chat_id, "private:13");
        assert!(parse_event(&v, &s).is_none());
        v["user_id"] = json!(99);
        assert!(parse_event(&v, &s).is_none());
    }

    fn shared(me: i64) -> Shared {
        let s = Shared::default();
        s.self_id.store(me, Ordering::Relaxed);
        s
    }

    fn parse_event(v: &Value, s: &Shared) -> Option<Envelope> {
        parse_with(v, s, &Forwards::new())
    }

    fn parse_with(v: &Value, s: &Shared, forwards: &Forwards) -> Option<Envelope> {
        finish(super::parse_event(v, s)?, forwards, s, if v["message"].as_array().is_some_and(|m| reply_segment(m).is_some()) { "[回复]" } else { "" })
    }

    fn group(message: Value) -> Value {
        json!({"post_type": "message", "message_type": "group", "user_id": 12, "group_id": 34, "message_id": 1,
               "sender": {"nickname": "nick"}, "message": message})
    }

    fn text(t: &str) -> Value {
        json!({"type": "text", "data": {"text": t}})
    }

    fn at(id: &str) -> Value {
        json!({"type": "at", "data": {"qq": id}})
    }

    /// A node as `get_forward_msg` returns it: a whole message.
    fn node(name: &str, message: Value) -> Value {
        json!({"user_id": 7, "sender": {"user_id": 7, "nickname": name, "card": ""}, "message_format": "array", "message": message})
    }

    fn text_of(s: &Shared, message: Value) -> String {
        parse_event(&group(message), s).unwrap().text
    }

    #[test]
    fn top_level_media_references_preserve_metadata() {
        let env = parse_event(&group(json!([
            {"type": "image", "data": {"file": "ABC.jpg", "sub_type": 0, "url": "https://multimedia.nt.qq.com.cn/download?appid=1407&rkey=image", "file_size": "12345"}},
            {"type": "image", "data": {"file": "DEF.gif", "sub_type": "1", "url": "https://example.test/sticker", "file_size": 42}},
            {"type": "mface", "data": {"summary": "[sticker]", "url": "https://gxh.vip.qq.com/club/item/parcel/item.gif"}},
            {"type": "record", "data": {"file": "voice.amr", "url": "https://grouptalk.c2c.qq.com/voice", "file_size": 128}},
            {"type": "video", "data": {"file": "video.mp4", "url": "https://example.test/video", "file_size": "2048"}},
            {"type": "file", "data": {"file": "report.pdf", "file_id": "/group/file-id", "file_unique": "unique-id", "name": "Report.pdf", "url": "https://example.test/file", "file_size": "2621440"}}
        ])), &shared(99)).unwrap();
        let a = &env.attachments;
        assert_eq!(a.len(), 6);
        assert!(matches!(a[0].kind, AttachmentKind::Image));
        assert!(matches!(a[1].kind, AttachmentKind::Sticker));
        assert!(matches!(a[2].kind, AttachmentKind::Sticker));
        assert!(matches!(a[3].kind, AttachmentKind::Voice));
        assert!(matches!(a[4].kind, AttachmentKind::Video));
        assert!(matches!(a[5].kind, AttachmentKind::File));
        for (i, id, size) in [(0, "ABC.jpg", 12345), (1, "DEF.gif", 42), (3, "voice.amr", 128), (4, "video.mp4", 2048), (5, "/group/file-id", 2621440)] {
            assert_eq!(a[i].file_id.as_deref(), Some(id));
            assert_eq!(a[i].size, Some(size));
            assert!(a[i].url.as_deref().unwrap().starts_with("https://"));
        }
        assert_eq!(a[2].url.as_deref(), Some("https://gxh.vip.qq.com/club/item/parcel/item.gif"));
        assert!(a[2].file_id.is_none());
        assert_eq!(a[5].name.as_deref(), Some("Report.pdf"));
    }

    #[test]
    fn media_missing_or_invalid_fields_are_tolerated() {
        for kind in ["image", "mface", "record", "video", "file"] {
            for data in [json!({}), Value::Null, json!({"url": null, "file": {}, "file_id": [], "name": false, "file_size": "invalid"})] {
                let env = parse_event(&group(json!([{"type": kind, "data": data}])), &shared(99)).unwrap();
                let a = &env.attachments[0];
                assert_eq!(env.attachments.len(), 1);
                assert!(a.url.is_none() && a.file_id.is_none() && a.name.is_none() && a.size.is_none());
            }
        }
        for (data, id, name) in [
            (json!({"file": "report.pdf"}), "report.pdf", Some("report.pdf")),
            (json!({"file_unique": "unique-id"}), "unique-id", None),
            (json!({"file_id": 123, "file_size": -1}), "123", None),
        ] {
            let env = parse_event(&group(json!([{"type": "file", "data": data}])), &shared(99)).unwrap();
            assert_eq!(env.attachments[0].file_id.as_deref(), Some(id));
            assert_eq!(env.attachments[0].name.as_deref(), name);
            assert!(env.attachments[0].size.is_none());
        }
        assert!(parse_event(&group(json!([text("hello")])), &shared(99)).unwrap().attachments.is_empty());
    }

    #[tokio::test]
    async fn quoted_images_come_along_but_forwards_and_other_quoted_media_do_not() {
        let s = shared(99);
        let media = json!([{"type": "image", "data": {"file": "nested.jpg", "url": "https://example.test/nested"}}]);
        let msg = json!([
            reply("123"),
            {"type": "forward", "data": {"content": [node("inline", media.clone())]}},
            {"type": "forward", "data": {"id": "fetched"}}
        ]);
        let fetched = json!([media[0], {"type": "record", "data": {"file": "quoted.amr"}}]);
        let (prefix, images) = resolve_reply(msg.as_array().unwrap(), &s, |_| async { Ok(node("quoted", fetched.clone())) }).await;
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].file_id.as_deref(), Some("nested.jpg"));
        assert_eq!(images[0].quoted_from.as_deref(), Some("quoted"));
        // The cached copy answers the next quote without another fetch.
        let (_, images) = resolve_reply(msg.as_array().unwrap(), &s, |_| async { panic!("cache hit must not fetch") }).await;
        assert_eq!(images.len(), 1);
        let forwards = Forwards::from([("fetched".into(), Some(vec![node("fetched", media)]))]);
        let env = finish(super::parse_event(&group(msg.clone()), &s).unwrap(), &forwards, &s, &prefix).unwrap();
        assert!(env.attachments.is_empty());
        let mut mixed = msg.as_array().unwrap().clone();
        mixed.push(json!({"type": "record", "data": {"file": "own.amr"}}));
        let env = finish(super::parse_event(&group(json!(mixed)), &s).unwrap(), &forwards, &s, &prefix).unwrap();
        assert_eq!(env.attachments.len(), 1);
        assert_eq!(env.attachments[0].file_id.as_deref(), Some("own.amr"));
        assert!(matches!(env.attachments[0].kind, AttachmentKind::Voice));
    }

    fn image(file: &str) -> Value {
        json!({"type": "image", "data": {"file": file, "url": format!("https://example.test/{file}")}})
    }

    #[tokio::test]
    async fn a_quote_of_a_quote_reaches_the_image_through_get_msg() {
        let s = shared(99);
        let msg = json!([reply("c"), text("这是啥")]);
        let mut asked = Vec::new();
        let (_, images) = resolve_reply(msg.as_array().unwrap(), &s, |id| {
            asked.push(id.clone());
            async move {
                Ok(match id.as_str() {
                    "c" => node("丙", json!([reply("b"), text("看这个")])),
                    "b" => node("乙", json!([reply("a"), text("转")])),
                    _ => node("甲", json!([image("a.jpg")])),
                })
            }
        }).await;
        assert_eq!(asked, ["c", "b", "a"]);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].file_id.as_deref(), Some("a.jpg"));
        assert_eq!(images[0].quoted_from.as_deref(), Some("甲"));
        // Quoting the same message again needs no fetch and keeps the first sender.
        let (_, images) = resolve_reply(msg.as_array().unwrap(), &s, |_| async { panic!("cache hit must not fetch") }).await;
        assert_eq!(images[0].quoted_from.as_deref(), Some("甲"));
    }

    #[tokio::test]
    async fn a_quote_chain_stops_at_the_nearest_image_and_at_the_fetch_limit() {
        let s = shared(99);
        let msg = json!([reply("c")]);
        let mut asked = Vec::new();
        let (_, images) = resolve_reply(msg.as_array().unwrap(), &s, |id| {
            asked.push(id.clone());
            async move {
                Ok(match id.as_str() {
                    "c" => node("丙", json!([reply("b"), text("看")])),
                    _ => node("乙", json!([reply("a"), image("b.jpg")])),
                })
            }
        }).await;
        assert_eq!(asked, ["c", "b"]);
        assert_eq!(images[0].file_id.as_deref(), Some("b.jpg"));

        let s = shared(99);
        let mut asked = Vec::new();
        let (_, images) = resolve_reply(msg.as_array().unwrap(), &s, |id| {
            asked.push(id.clone());
            let next = format!("{id}x");
            async move { Ok(node("丙", json!([reply(&next), text("再转")]))) }
        }).await;
        assert_eq!(asked.len(), QUOTE_FETCHES);
        assert!(images.is_empty());
    }

    #[tokio::test]
    async fn recent_quote_chains_resolve_from_the_cache_with_the_first_sender() {
        let (raw_tx, raw_rx) = mpsc::channel(8);
        let (inbound_tx, mut inbound) = mpsc::channel(8);
        // Holding the receiver without answering: a fetch would time out and fail the assertions.
        let (tx, _requests) = mpsc::channel(8);
        let s = Arc::new(shared(99));
        tokio::spawn(enrich(raw_rx, QqHandle { tx, shared: s.clone() }, inbound_tx));
        for (id, name, message) in [
            (1, "甲", json!([image("a.jpg")])),
            (2, "乙", json!([reply("1"), text("这是啥")])),
            (3, "丙", json!([reply("2"), text("我也想问")])),
            (4, "丁", json!([reply("3"), text("加一")])),
        ] {
            let mut event = group(message);
            event["message_id"] = json!(id);
            event["sender"]["nickname"] = json!(name);
            raw_tx.send(super::parse_event(&event, &s).unwrap()).await.unwrap();
        }
        let mut last = None;
        for _ in 0..4 { last = inbound.recv().await; }
        let last = last.unwrap();
        assert_eq!(last.attachments.len(), 1);
        assert_eq!(last.attachments[0].file_id.as_deref(), Some("a.jpg"));
        assert_eq!(last.attachments[0].quoted_from.as_deref(), Some("甲"));
    }

    #[test]
    fn parses_group_message_with_at_and_reply() {
        let s = shared(99);
        s.remember("555".into());
        let ev = json!({
            "post_type": "message", "message_type": "group", "self_id": 99,
            "user_id": 12, "group_id": 34, "message_id": 777,
            "sender": {"nickname": "nick", "card": "群名片"},
            "message": [{"type": "at", "data": {"qq": "99"}}, {"type": "text", "data": {"text": " 你怎么看"}}, {"type": "image", "data": {}}]
        });
        let env = parse_event(&ev, &s).unwrap();
        assert_eq!(env.chat_id, "group:34");
        assert_eq!(env.message_id.as_deref(), Some("777"));
        assert_eq!(env.group_key().as_deref(), Some("qq:34"));
        assert_eq!(env.sender_name, "群名片");
        assert_eq!(env.text, "你怎么看[图片]");
        assert!(env.mentions_me && env.is_group);

        let reply = json!({
            "post_type": "message", "message_type": "group", "user_id": 12, "group_id": 34, "message_id": 778,
            "sender": {"nickname": "nick"},
            "message": [{"type": "reply", "data": {"id": "555"}}, {"type": "at", "data": {"qq": "13", "name": "老王"}}, {"type": "text", "data": {"text": "对"}}]
        });
        let env = parse_event(&reply, &s).unwrap();
        assert!(env.mentions_me);
        assert_eq!(env.text, "[回复]@老王 对");
    }

    #[test]
    fn ignores_own_and_non_message_events() {
        let s = shared(99);
        let own = json!({"post_type": "message", "message_type": "private", "user_id": 99, "message": [{"type": "text", "data": {"text": "x"}}]});
        assert!(parse_event(&own, &s).is_none());
        assert!(parse_event(&json!({"post_type": "meta_event"}), &s).is_none());
        let private = json!({"post_type": "message", "message_type": "private", "user_id": 5, "message_id": "1", "message": [{"type": "text", "data": {"text": "在吗"}}]});
        let env = parse_event(&private, &s).unwrap();
        assert_eq!((env.chat_id.as_str(), env.mentions_me), ("private:5", true));
        assert_eq!(env.message_id.as_deref(), Some("1"));
    }

    #[test]
    fn forward_only_message_is_kept_and_rendered_from_inline_content() {
        let s = shared(99);
        let fwd = json!([{"type": "forward", "data": {"id": "7703", "content": [
            node("甲", json!([text("第一句")])),
            node("乙", json!([text("看图"), {"type": "image", "data": {"url": "http://x"}}])),
        ]}}]);
        let env = parse_event(&group(fwd), &s).unwrap();
        assert_eq!(env.text, "[合并转发 2条]\n甲: 第一句\n乙: 看图[图片]");
        assert!(!env.mentions_me);
    }

    #[tokio::test]
    async fn forward_fetched_when_not_inline_and_nested_to_two_levels() {
        let s = shared(99);
        let msg = json!([text("你们看"), {"type": "forward", "data": {"id": "a"}}]);
        let mut asked = Vec::new();
        let forwards = collect_forwards(msg.as_array().unwrap(), |id| {
            asked.push(id.clone());
            async move {
                Ok(match id.as_str() {
                    "a" => vec![node("甲", json!([text("外层")])), node("乙", json!([{"type": "forward", "data": {"id": "b"}}]))],
                    "b" => vec![node("丙", json!([text("内层"), {"type": "forward", "data": {"id": "c"}}]))],
                    _ => vec![node("丁", json!([text("太深")]))],
                })
            }
        })
        .await;
        assert_eq!(asked, ["a", "b"], "the third level is never fetched");
        let env = parse_with(&group(msg), &s, &forwards).unwrap();
        assert_eq!(env.text, "你们看[合并转发 2条]\n甲: 外层\n乙: [合并转发 1条]\n  丙: 内层[合并转发]");
    }

    #[tokio::test]
    async fn unreadable_forward_keeps_the_message() {
        let s = shared(99);
        let msg = json!([{"type": "forward", "data": {"id": "a"}}]);
        let forwards = collect_forwards(msg.as_array().unwrap(), |_| async { Err(anyhow!("boom")) }).await;
        assert_eq!(parse_with(&group(msg), &s, &forwards).unwrap().text, "[合并转发，读取失败]");
    }

    #[test]
    fn forward_caps_nodes_and_characters() {
        let s = shared(99);
        let many: Vec<Value> = (0..50).map(|i| node("甲", json!([text(&format!("{i}"))]))).collect();
        let t = text_of(&s, json!([{"type": "forward", "data": {"id": "a", "content": many}}]));
        assert!(t.starts_with("[合并转发 50条]\n甲: 0\n"));
        assert!(t.ends_with(&format!("甲: {}\n…(省略 10 条)", FORWARD_NODES - 1)), "{t}");

        let long: Vec<Value> = (0..10).map(|_| node("甲", json!([text(&"字".repeat(NODE_CHARS + 50))]))).collect();
        let t = text_of(&s, json!([{"type": "forward", "data": {"id": "a", "content": long}}]));
        assert!(t.contains(&format!("{}…", "字".repeat(NODE_CHARS))));
        assert!(t.ends_with("…(省略 6 条)"), "{t}");
        assert!(t.chars().count() < FORWARD_CHARS + NODE_CHARS + 50);
    }

    #[test]
    fn mentions_inside_a_forward_do_not_mention_him() {
        let s = shared(99);
        s.remember("555".into());
        let inner = json!([{"type": "reply", "data": {"id": "555"}}, {"type": "at", "data": {"qq": "99"}}, text("你好")]);
        let env = parse_event(&group(json!([{"type": "forward", "data": {"id": "a", "content": [node("甲", inner)]}}])), &s).unwrap();
        assert!(!env.mentions_me);
        assert_eq!(env.text, "[合并转发 1条]\n甲: @我 你好");
    }

    #[test]
    fn renders_every_segment_type() {
        let s = shared(99);
        let card = json!({"prompt": "[QQ小程序]哔哩哔哩", "meta": {"detail_1": {"title": "哔哩哔哩", "desc": "猫猫视频", "qqdocurl": "https://b23.tv/x"}}});
        let cases = [
            (json!({"type": "at", "data": {"qq": "all"}}), "@全体成员"),
            (json!({"type": "face", "data": {"id": "14", "raw": {"faceText": "/微笑"}}}), "[表情:微笑]"),
            (json!({"type": "face", "data": {"id": "14"}}), "[表情]"),
            (json!({"type": "mface", "data": {"summary": "[狗头]", "url": "http://x"}}), "[表情:狗头]"),
            (json!({"type": "image", "data": {"sub_type": 1, "summary": "[动画表情]"}}), "[动画表情]"),
            (json!({"type": "image", "data": {"sub_type": 0}}), "[图片]"),
            (json!({"type": "record", "data": {}}), "[语音]"),
            (json!({"type": "video", "data": {}}), "[视频]"),
            (json!({"type": "file", "data": {"file": "报告.pdf", "file_size": "2621440"}}), "[文件:报告.pdf 2.5MB]"),
            (json!({"type": "file", "data": {}}), "[文件]"),
            (json!({"type": "json", "data": {"data": card.to_string()}}), "[卡片:[QQ小程序]哔哩哔哩 猫猫视频 https://b23.tv/x]"),
            (json!({"type": "json", "data": {"data": "not json"}}), "[卡片]"),
            (json!({"type": "xml", "data": {"data": "<msg/>"}}), "[卡片]"),
            (json!({"type": "markdown", "data": {"content": "# 标题"}}), "[markdown:# 标题]"),
            (json!({"type": "poke", "data": {"type": "1", "id": "1"}}), "[戳一戳]"),
            (json!({"type": "dice", "data": {"result": "6"}}), "[骰子:6]"),
            (json!({"type": "rps", "data": {"result": "2"}}), "[猜拳:2]"),
            (json!({"type": "contact", "data": {"type": "group", "id": "123"}}), "[推荐群:123]"),
            (json!({"type": "contact", "data": {"type": "qq", "id": "456"}}), "[推荐好友:456]"),
            (json!({"type": "location", "data": {"lat": "1", "lon": "2", "title": "公园", "content": "某路1号"}}), "[位置:公园 某路1号]"),
            (json!({"type": "music", "data": {"type": "custom", "title": "歌"}}), "[音乐:歌]"),
            (json!({"type": "music", "data": {"type": "qq", "id": "1"}}), "[音乐]"),
            (json!({"type": "share", "data": {"url": "https://a.b", "title": "文章"}}), "[链接:文章 https://a.b]"),
            (json!({"type": "shake", "data": {}}), "[shake]"),
        ];
        for (seg, want) in cases {
            assert_eq!(text_of(&s, json!([seg.clone()])), want, "{seg}");
        }
        assert!(s.unknown_types.lock().unwrap().contains("shake"));
    }

    #[test]
    fn message_without_readable_segments_is_dropped() {
        let s = shared(99);
        assert_eq!(text_of(&s, json!([{"type": "reply", "data": {"id": "1"}}])), "[回复]");
        assert!(parse_event(&group(json!([text("  ")])), &s).is_none());
    }

    fn reply(id: &str) -> Value {
        json!({"type": "reply", "data": {"id": id}})
    }

    #[tokio::test]
    async fn reply_uses_cache_and_preserves_own_mention() {
        let s = shared(99);
        s.remember("555".into());
        s.cache("555".into(), Quoted { name: "我".into(), text: "你好\r\n世界".into(), images: Vec::new() });
        let msg = json!([reply("555"), text("对")]);
        let (prefix, _) = resolve_reply(msg.as_array().unwrap(), &s, |_| async { panic!("cache hit must not fetch") }).await;
        let env = finish(super::parse_event(&group(msg), &s).unwrap(), &Forwards::new(), &s, &prefix).unwrap();
        assert_eq!(env.text, "[回复 我: 你好 世界]对");
        assert!(env.mentions_me);
    }

    #[tokio::test]
    async fn reply_fetches_and_cuts_unicode_text() {
        let s = shared(99);
        for n in [39, 40, 41] {
            let msg = json!([reply(&n.to_string())]);
            let (prefix, _) = resolve_reply(msg.as_array().unwrap(), &s, |id| async move {
                assert_eq!(id, n.to_string());
                Ok(node("甲", json!([text(&"字".repeat(n))])))
            })
            .await;
            assert_eq!(prefix, format!("[回复 甲: {}{}]", "字".repeat(n.min(40)), if n > 40 { "…" } else { "" }));
            let env = finish(super::parse_event(&group(msg), &s).unwrap(), &Forwards::new(), &s, &prefix).unwrap();
            assert_eq!(env.text, prefix);
            assert!(!env.mentions_me);
        }
    }

    #[tokio::test]
    async fn missing_reply_keeps_body_and_reply_only_message() {
        let s = shared(99);
        for data in [None, Some(Value::Null), Some(json!({}))] {
            let msg = json!([reply("123"), text("原文")]);
            let (prefix, _) = resolve_reply(msg.as_array().unwrap(), &s, |_| {
                let data = data.clone();
                async move { data.ok_or_else(|| anyhow!("boom")) }
            })
            .await;
            let env = finish(super::parse_event(&group(msg), &s).unwrap(), &Forwards::new(), &s, &prefix).unwrap();
            assert_eq!(env.text, "[回复]原文");
        }
        let msg = json!([reply("123")]);
        let (prefix, _) = resolve_reply(msg.as_array().unwrap(), &s, |_| async { Err(anyhow!("missing")) }).await;
        assert_eq!(finish(super::parse_event(&group(msg), &s).unwrap(), &Forwards::new(), &s, &prefix).unwrap().text, "[回复]");
    }

    #[tokio::test]
    async fn quoted_forwards_show_only_headers() {
        let s = shared(99);
        let msg = json!([reply("123")]);
        let (prefix, _) = resolve_reply(msg.as_array().unwrap(), &s, |_| async {
            Ok(node(
                "甲",
                json!([
                    reply("555"),
                    {"type": "forward", "data": {"id": "a", "content": [node("甲", json!([text("原文")]))]}},
                    {"type": "forward", "data": {"id": "b"}}
                ]),
            ))
        })
        .await;
        assert_eq!(prefix, "[回复 甲: [合并转发 1条] [合并转发]]");
    }

    #[test]
    fn recent_messages_evict_least_recently_used_and_replace() {
        let s = shared(99);
        for i in 0..RECENT_MESSAGES {
            s.cache(i.to_string(), Quoted { name: "我".into(), text: i.to_string(), images: Vec::new() });
        }
        assert_eq!(s.cached("0").unwrap().text, "0");
        s.cache("500".into(), Quoted { name: "我".into(), text: "new".into(), images: Vec::new() });
        assert!(s.cached("1").is_none());
        s.cache("0".into(), Quoted { name: "我".into(), text: "updated".into(), images: Vec::new() });
        assert_eq!(s.cached("0").unwrap().text, "updated");
        assert_eq!(s.recent.lock().unwrap().len(), RECENT_MESSAGES);
    }

    #[tokio::test]
    async fn send_caches_each_chunk_as_me() {
        let (tx, mut rx) = mpsc::channel::<Request>(8);
        let qq = QqHandle { tx, shared: Arc::new(shared(99)) };
        let respond = async {
            for i in 0..2 {
                let req = rx.recv().await.unwrap();
                assert_eq!(req.action, "send_group_msg");
                req.reply.send(Ok(json!({"message_id": i}))).unwrap();
            }
        };
        let body = "字".repeat(CHUNK_CHARS + 1);
        let (sent, ()) = tokio::join!(qq.send("group:34", &body), respond);
        sent.unwrap();
        for (i, len) in [(0, CHUNK_CHARS), (1, 1)] {
            let q = qq.shared.cached(&i.to_string()).unwrap();
            assert_eq!(q.name, "我");
            assert_eq!(q.text, "字".repeat(len));
            assert!(qq.shared.is_mine(&i.to_string()));
        }
    }

    #[tokio::test]
    async fn reply_only_quotes_ambiguous_groups_on_first_chunk() {
        for (chat, last, target, quoted) in [
            ("group:34", "2", Some("1"), true),
            ("group:34", "1", Some("1"), false),
            ("private:34", "2", Some("1"), false),
            ("group:34", "2", None, false),
        ] {
            let (tx, mut rx) = mpsc::channel::<Request>(8);
            let qq = QqHandle { tx, shared: Arc::new(shared(99)) };
            qq.shared.last_inbound.lock().unwrap().insert(chat.into(), Some(last.into()));
            let body = format!("[CQ:at,qq=42]{}", "字".repeat(CHUNK_CHARS));
            let chunks = super::super::telegram::chunks(&body, CHUNK_CHARS);
            let respond = async {
                for (i, chunk) in chunks.iter().enumerate() {
                    let req = rx.recv().await.unwrap();
                    let mut expected = vec![text(chunk)];
                    if quoted && i == 0 { expected.insert(0, reply("1")); }
                    assert_eq!(req.params["message"], json!(expected));
                    req.reply.send(Ok(json!({"message_id": i}))).unwrap();
                }
            };
            let pieces = [ReplyPiece::Text(body.clone())];
            let (sent, ()) = tokio::join!(qq.send_reply(chat, &pieces, target), respond);
            sent.unwrap();
            assert!(rx.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn mention_segments_keep_order_quotes_and_first_chunk_only() {
        for (chat, last, quoted) in [("group:34", "2", true), ("group:34", "1", false), ("private:34", "2", false)] {
            let (tx, mut rx) = mpsc::channel::<Request>(8);
            let qq = QqHandle { tx, shared: Arc::new(shared(99)) };
            qq.shared.last_inbound.lock().unwrap().insert(chat.into(), Some(last.into()));
            let pieces = vec![
                ReplyPiece::Text("先说：".into()),
                ReplyPiece::Mention { name: "小明".into(), qq_id: Some(42) },
                ReplyPiece::Text("，再问".into()),
                ReplyPiece::Mention { name: "小红".into(), qq_id: Some(43) },
                ReplyPiece::Text(format!("[CQ:at,qq=44] @原样{}", "字".repeat(CHUNK_CHARS))),
                ReplyPiece::Mention { name: "后面".into(), qq_id: Some(44) },
            ];
            let plain = reply_text(&pieces);
            let chunks = super::super::telegram::chunks(&plain, CHUNK_CHARS);
            let respond = async {
                for (i, chunk) in chunks.iter().enumerate() {
                    let req = rx.recv().await.unwrap();
                    let mut expected = if i == 0 && chat.starts_with("group:") {
                        vec![text("先说："), at("42"), text("，再问"), at("43"), text(&chunk["先说：@小明，再问@小红".len()..])]
                    } else { vec![text(chunk)] };
                    if quoted && i == 0 { expected.insert(0, reply("1")); }
                    assert_eq!(req.params["message"], json!(expected));
                    req.reply.send(Ok(json!({"message_id": i}))).unwrap();
                }
            };
            let (sent, ()) = tokio::join!(qq.send_reply(chat, &pieces, Some("1")), respond);
            sent.unwrap();
            assert!(rx.try_recv().is_err());
            assert_eq!(qq.shared.cached("0").unwrap().text, chunks[0]);
        }
    }

    #[test]
    fn mention_params_enforce_private_self_invalid_and_two_mention_limits() {
        let pieces = vec![
            ReplyPiece::Mention { name: "自己".into(), qq_id: Some(99) },
            ReplyPiece::Mention { name: "all".into(), qq_id: None },
            ReplyPiece::Mention { name: "坏id".into(), qq_id: Some(-1) },
            ReplyPiece::Mention { name: "零".into(), qq_id: Some(0) },
            ReplyPiece::Mention { name: "未知".into(), qq_id: None },
            ReplyPiece::Mention { name: "小明".into(), qq_id: Some(42) },
            ReplyPiece::Mention { name: "小红".into(), qq_id: Some(43) },
            ReplyPiece::Mention { name: "小蓝".into(), qq_id: Some(44) },
        ];
        assert_eq!(reply_params("group_id", 34, 99, &pieces, None), json!({"group_id":34, "message":[
            text("@自己@all@坏id@零@未知"), at("42"), at("43"), text("@小蓝")
        ]}));
        assert_eq!(reply_params("user_id", 34, 99, &pieces, Some("1")),
            json!({"user_id":34, "message":[text(&reply_text(&pieces))]}));
        assert_eq!(reply_params("group_id", 34, 0, &pieces, None),
            json!({"group_id":34, "message":[text(&reply_text(&pieces))]}));
        let cut = reply_prefix(&[ReplyPiece::Mention { name: "小明".into(), qq_id: Some(42) }], "@小".len());
        assert_eq!(reply_params("group_id", 34, 99, &cut, None), json!({"group_id":34, "message":[text("@小")]}));
    }

    /// Manual live-shaped payload preview; builds parameters only, with no socket or credentials.
    #[test]
    #[ignore = "manual mention params preview, sends nothing"]
    fn live_build_mention_params() {
        let pieces = vec![
            ReplyPiece::Mention { name: "小明".into(), qq_id: Some(42) },
            ReplyPiece::Text("，你怎么看？[CQ:at,qq=all]".into()),
        ];
        for quote in [None, Some("123456")] {
            println!("{}", serde_json::to_string_pretty(&reply_params("group_id", 34, 99, &pieces, quote)).unwrap());
        }
        println!("{}", serde_json::to_string_pretty(&forward_params("group_id", 34, 99, "小庄", &[reply_text(&pieces)])).unwrap());
    }

    #[tokio::test]
    async fn long_forward_actions_nodes_cache_and_fallback() {
        for (chat, key, forward, plain) in [
            ("group:34", "group_id", "send_group_forward_msg", "send_group_msg"),
            ("private:34", "user_id", "send_private_forward_msg", "send_private_msg"),
        ] {
            for fail in [false, true] {
                let (tx, mut rx) = mpsc::channel::<Request>(8);
                let qq = QqHandle { tx, shared: Arc::new(shared(99)) };
                *qq.shared.nickname.lock().unwrap() = Some("小庄".into());
                let pieces = vec![ReplyPiece::Mention { name: "小明".into(), qq_id: Some(42) },
                    ReplyPiece::Text(format!("[CQ:at,qq=42]{}", "字".repeat(CHUNK_CHARS * 2)))];
                let body = reply_text(&pieces);
                let chunks = super::super::telegram::chunks(&body, CHUNK_CHARS);
                assert_eq!(chunks.len(), 3);
                let respond = async {
                    let req = rx.recv().await.unwrap();
                    assert_eq!(req.action, forward);
                    assert_eq!(req.params[key], 34);
                    let nodes = req.params["messages"].as_array().unwrap();
                    assert_eq!(nodes.len(), chunks.len());
                    for (node, chunk) in nodes.iter().zip(&chunks) {
                        assert_eq!(*node, json!({"type":"node", "data":{
                            "user_id":99, "nickname":"小庄", "content":[text(chunk)]}}));
                    }
                    if fail {
                        req.reply.send(Err(anyhow!("forward rejected"))).unwrap();
                        for (i, chunk) in chunks.iter().enumerate() {
                            let req = rx.recv().await.unwrap();
                            assert_eq!(req.action, plain);
                            assert_eq!(req.params, json!({key:34, "message":[text(chunk)]}));
                            req.reply.send(Ok(json!({"message_id": i}))).unwrap();
                        }
                    } else {
                        req.reply.send(Ok(json!({"message_id":"forward-id"}))).unwrap();
                    }
                };
                let (sent, ()) = tokio::join!(qq.send_reply(chat, &pieces, None), respond);
                sent.unwrap();
                assert!(rx.try_recv().is_err());
                let mid = if fail { "0" } else { "forward-id" };
                assert!(qq.shared.is_mine(mid));
                let cached = qq.shared.cached(mid).unwrap();
                assert_eq!(cached.name, "我");
                assert_eq!(cached.text, chunks[0]);
            }
        }
    }

    /// Manual live-payload preview; never connects or sends anything.
    #[test]
    #[ignore = "manual forward params preview, sends nothing"]
    fn live_build_forward_params() {
        let chunks = super::super::telegram::chunks(&"sample text ".repeat(300), CHUNK_CHARS);
        println!("{}", serde_json::to_string_pretty(&forward_params("group_id", 34, 99, "小庄", &chunks)).unwrap());
    }

    #[tokio::test]
    async fn enrich_caches_envelopes_for_later_replies() {
        let (tx, mut requests) = mpsc::channel(8);
        let qq = QqHandle { tx, shared: Arc::new(shared(99)) };
        let (raw_tx, raw_rx) = mpsc::channel(8);
        let (in_tx, mut in_rx) = mpsc::channel(8);
        raw_tx.send(super::parse_event(&group(json!([text("原文")])), &qq.shared).unwrap()).await.unwrap();
        let mut next = group(json!([reply("1"), text("原文")]));
        next["message_id"] = json!(2);
        raw_tx.send(super::parse_event(&next, &qq.shared).unwrap()).await.unwrap();
        drop(raw_tx);
        enrich(raw_rx, qq.clone(), in_tx).await;
        assert_eq!(in_rx.recv().await.unwrap().text, "原文");
        assert_eq!(in_rx.recv().await.unwrap().text, "[回复 nick: 原文]原文");
        assert!(requests.try_recv().is_err());
        assert_eq!(qq.shared.cached("2").unwrap().text, "原文");
    }

    #[tokio::test]
    async fn enrich_tracks_each_chat_but_not_notices() {
        let (tx, _requests) = mpsc::channel(8);
        let qq = QqHandle { tx, shared: Arc::new(shared(99)) };
        let (raw_tx, raw_rx) = mpsc::channel(8);
        let (in_tx, _in_rx) = mpsc::channel(8);
        for (chat, mid, notice) in [
            ("group:34", Some("1"), false),
            ("private:5", Some("2"), false),
            ("group:34", Some("3"), false),
            ("group:34", None, true),
            ("private:5", None, false),
        ] {
            let mut p = super::parse_event(&group(json!([text("body")])), &qq.shared).unwrap();
            p.env.chat_id = chat.into();
            p.env.message_id = mid.map(str::to_owned);
            p.env.notice = notice;
            if notice { p.env.sender_name.clear(); }
            raw_tx.send(p).await.unwrap();
        }
        drop(raw_tx);
        enrich(raw_rx, qq.clone(), in_tx).await;
        let last = qq.shared.last_inbound.lock().unwrap();
        assert_eq!(last.get("group:34"), Some(&Some("3".into())));
        assert_eq!(last.get("private:5"), Some(&None));
    }

    /// Reads one real quoted message; sends nothing.
    #[tokio::test]
    #[ignore = "requires NapCat with ONEBOT_URL / ONEBOT_TOKEN and REPLY_ID"]
    async fn live_get_msg() -> Result<()> {
        dotenvy::dotenv().ok();
        let (tx, _rx) = mpsc::channel(8);
        let qq = spawn(std::env::var("ONEBOT_URL")?, std::env::var("ONEBOT_TOKEN").ok(), tx);
        let data = qq.get_message(&std::env::var("REPLY_ID")?).await?;
        assert!(data.get("message").is_some_and(|m| m.is_array() || m.is_string()));
        Ok(())
    }

    /// Reads one real merged forward, read-only: set FORWARD_ID to a forward segment's `data.id`.
    #[tokio::test]
    #[ignore = "requires NapCat with ONEBOT_URL / ONEBOT_TOKEN and FORWARD_ID"]
    async fn live_get_forward_msg() -> Result<()> {
        dotenvy::dotenv().ok();
        let (tx, _rx) = mpsc::channel(8);
        let qq = spawn(std::env::var("ONEBOT_URL")?, std::env::var("ONEBOT_TOKEN").ok(), tx);
        let nodes = qq.forward_nodes(&std::env::var("FORWARD_ID")?).await?;
        assert!(!nodes.is_empty());
        let s = Shared::default();
        let msg = json!({"post_type": "message", "message_type": "private", "user_id": 5,
                         "message": [{"type": "forward", "data": {"id": "x", "content": nodes}}]});
        let env = parse_event(&msg, &s).unwrap();
        assert!(env.text.starts_with("[合并转发 "), "{}", env.text);
        Ok(())
    }

    /// Talks to the local NapCat: reads login info only, sends nothing.
    #[tokio::test]
    #[ignore = "requires NapCat with ONEBOT_URL / ONEBOT_TOKEN"]
    async fn live_onebot_login_info() -> Result<()> {
        dotenvy::dotenv().ok();
        let url = std::env::var("ONEBOT_URL")?;
        let token = std::env::var("ONEBOT_TOKEN").ok();
        let (tx, _rx) = mpsc::channel(8);
        let qq = spawn(url, token, tx);
        let info = qq.call("get_login_info", json!({})).await?;
        assert!(info.get("user_id").is_some(), "{info}");
        Ok(())
    }
}
