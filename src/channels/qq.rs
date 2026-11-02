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
//! | reply | nothing; a reply to his message sets `mentions_me` |
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

use super::Envelope;

const CHUNK_CHARS: usize = 1500;
const ACTION_TIMEOUT: Duration = Duration::from_secs(20);
/// How many of his own message ids to remember, to recognise replies to him.
const OWN_MESSAGES: usize = 300;
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
    own_messages: Mutex<VecDeque<String>>,
    /// Unknown segment types already logged.
    unknown_types: Mutex<HashSet<String>>,
}

impl Shared {
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
        let forwards = if has_forward(&p.message) {
            collect_forwards(&p.message, |id| {
                let qq = qq.clone();
                async move { qq.forward_nodes(&id).await }
            })
            .await
        } else {
            Forwards::new()
        };
        if let Some(env) = finish(p, &forwards, &qq.shared)
            && inbound.send(env).await.is_err()
        {
            return;
        }
    }
}

impl QqHandle {
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
        let (action, key, id) = match chat_id.split_once(':') {
            Some(("private", id)) => ("send_private_msg", "user_id", id),
            Some(("group", id)) => ("send_group_msg", "group_id", id),
            _ => bail!("bad qq chat id `{chat_id}`"),
        };
        let id: i64 = id.parse().with_context(|| format!("bad qq chat id `{chat_id}`"))?;
        for chunk in super::telegram::chunks(text, CHUNK_CHARS) {
            let params = json!({ key: id, "message": [{"type": "text", "data": {"text": chunk}}] });
            let data = self.call(action, params).await?;
            if let Some(mid) = data.get("message_id") {
                self.shared.remember(id_string(mid));
            }
        }
        Ok(())
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
}

/// Merged-forward nodes by forward id; `None` when fetching failed.
type Forwards = HashMap<String, Option<Vec<Value>>>;

fn parse_event(v: &Value, shared: &Shared) -> Option<Pending> {
    if v.get("post_type")?.as_str()? != "message" {
        return None;
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
            arrived: Some(tokio::time::Instant::now()),
            platform: "qq",
            chat_id,
            message_id: v.get("message_id").filter(|id| !id.is_null()).map(id_string),
            sender_id: user_id.to_string(),
            sender_name: name,
            sender_is_bot: false,
            is_group,
            mentions_me: !is_group,
            text: String::new(),
        },
        message,
    })
}

/// Render the text; `None` when nothing readable is left.
fn finish(p: Pending, forwards: &Forwards, shared: &Shared) -> Option<Envelope> {
    let r = Render { self_id: shared.self_id.load(Ordering::Relaxed).to_string(), shared, forwards };
    let mut mentions_me = false;
    let text = r.segments(&p.message, 0, Some(&mut mentions_me)).trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some(Envelope { text, mentions_me: p.env.mentions_me || mentions_me, ..p.env })
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

    fn shared(me: i64) -> Shared {
        let s = Shared::default();
        s.self_id.store(me, Ordering::Relaxed);
        s
    }

    fn parse_event(v: &Value, s: &Shared) -> Option<Envelope> {
        parse_with(v, s, &Forwards::new())
    }

    fn parse_with(v: &Value, s: &Shared, forwards: &Forwards) -> Option<Envelope> {
        finish(super::parse_event(v, s)?, forwards, s)
    }

    fn group(message: Value) -> Value {
        json!({"post_type": "message", "message_type": "group", "user_id": 12, "group_id": 34, "message_id": 1,
               "sender": {"nickname": "nick"}, "message": message})
    }

    fn text(t: &str) -> Value {
        json!({"type": "text", "data": {"text": t}})
    }

    /// A node as `get_forward_msg` returns it: a whole message.
    fn node(name: &str, message: Value) -> Value {
        json!({"user_id": 7, "sender": {"user_id": 7, "nickname": name, "card": ""}, "message_format": "array", "message": message})
    }

    fn text_of(s: &Shared, message: Value) -> String {
        parse_event(&group(message), s).unwrap().text
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
        assert_eq!(env.text, "@老王 对");
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
        assert!(parse_event(&group(json!([{"type": "reply", "data": {"id": "1"}}])), &s).is_none());
        assert!(parse_event(&group(json!([text("  ")])), &s).is_none());
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
