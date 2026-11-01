//! QQ via a OneBot v11 forward WebSocket (NapCat). One task owns the socket and reconnects with
//! backoff; senders talk to it through a channel and get the action's response back by `echo`.
//!
//! Personal-account QQ bots go through an unofficial protocol: use a secondary account (DESIGN 7).

use std::collections::{HashMap, VecDeque};
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

struct Request {
    action: &'static str,
    params: Value,
    reply: oneshot::Sender<Result<Value>>,
}

#[derive(Default)]
struct Shared {
    self_id: AtomicI64,
    own_messages: Mutex<VecDeque<String>>,
}

impl Shared {
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
    let shared = Arc::new(Shared::default());
    tokio::spawn(run(url, token, rx, inbound, shared.clone()));
    QqHandle { tx, shared }
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
    inbound: mpsc::Sender<Envelope>,
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
    inbound: &mpsc::Sender<Envelope>,
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
                if let Some(env) = parse_event(&v, shared)
                    && inbound.send(env).await.is_err()
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

fn parse_event(v: &Value, shared: &Shared) -> Option<Envelope> {
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
    let mut text = String::new();
    let mut mentions_me = false;
    for seg in v.get("message")?.as_array()? {
        let data = &seg["data"];
        match seg.get("type").and_then(Value::as_str).unwrap_or_default() {
            "text" => text.push_str(data.get("text").and_then(Value::as_str).unwrap_or_default()),
            "at" => {
                let target = id_string(&data["qq"]);
                if target == self_id.to_string() {
                    mentions_me = true;
                } else {
                    let name = data.get("name").and_then(Value::as_str).unwrap_or(&target);
                    text.push_str(&format!("@{name} "));
                }
            }
            "reply" => mentions_me |= shared.is_mine(&id_string(&data["id"])),
            "image" => text.push_str("[图片]"),
            "face" => text.push_str("[表情]"),
            "record" => text.push_str("[语音]"),
            "video" => text.push_str("[视频]"),
            "file" => text.push_str("[文件]"),
            _ => {}
        }
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let sender = v.get("sender").cloned().unwrap_or_default();
    let name = [sender.get("card"), sender.get("nickname")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|s| !s.is_empty())
        .unwrap_or_default()
        .to_string();
    Some(Envelope {
        platform: "qq",
        chat_id,
        sender_id: user_id.to_string(),
        sender_name: name,
        sender_is_bot: false,
        is_group,
        mentions_me: mentions_me || !is_group,
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(me: i64) -> Shared {
        let s = Shared::default();
        s.self_id.store(me, Ordering::Relaxed);
        s
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
