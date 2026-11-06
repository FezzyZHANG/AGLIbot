//! Chat platforms, normalised to `Envelope`; replies go out through `Outlets`.

pub mod qq;
pub mod telegram;

use std::borrow::Cow;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Result, bail};

/// Explicit reply markup only; ordinary prose and CQ codes always remain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyPiece {
    Text(String),
    Mention { name: String, qq_id: Option<i64> },
}

impl ReplyPiece {
    pub fn plain(&self) -> Cow<'_, str> {
        match self {
            Self::Text(text) => Cow::Borrowed(text),
            Self::Mention { name, .. } => Cow::Owned(format!("@{name}")),
        }
    }
}

/// The shared rendering for Telegram, storage, size limits and QQ fallbacks.
pub fn reply_text(pieces: &[ReplyPiece]) -> String {
    pieces.iter().map(ReplyPiece::plain).collect()
}

/// A UTF-8 byte prefix of the plain rendering. A cut mention becomes literal text.
pub fn reply_prefix(pieces: &[ReplyPiece], mut end: usize) -> Vec<ReplyPiece> {
    let mut prefix = Vec::new();
    for piece in pieces {
        if end == 0 { break; }
        let plain = piece.plain();
        if plain.len() <= end {
            prefix.push(piece.clone());
            end -= plain.len();
        } else {
            prefix.push(ReplyPiece::Text(plain[..end].to_string()));
            break;
        }
    }
    prefix
}

/// A platform media reference; QQ URLs expire within hours.
/// NapCat can refetch via `get_image {file}`, `get_record {file, out_format}`,
/// or `get_file {file_id}`. References are not persisted to the store.
#[allow(dead_code)] // `name` waits for later media work.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub kind: AttachmentKind,
    pub url: Option<String>,
    pub file_id: Option<String>,
    pub name: Option<String>,
    pub size: Option<u64>,
    /// Who sent the message this was quoted from; `None` when it is in the message itself.
    pub quoted_from: Option<String>,
    /// Its entry in the image worker, once queued.
    pub media_id: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum AttachmentKind {
    Image,
    Sticker,
    Voice,
    Video,
    File,
}

/// A media download passed its size limit.
#[derive(Debug)]
pub struct TooLarge;

impl std::fmt::Display for TooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("media file exceeds the size limit")
    }
}

impl std::error::Error for TooLarge {}

/// QQ CDN downloads connect directly: without `no_proxy` reqwest would use HTTP(S)_PROXY or the system proxy.
static MEDIA_HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(3))
        .no_proxy()
        .build()
        .unwrap_or_default()
});

/// Read a response body, stopping as soon as it passes `max` bytes. Errors never carry the URL.
pub(crate) async fn read_capped(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    if response.content_length().is_some_and(|length| length > max as u64) {
        return Err(TooLarge.into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(reqwest::Error::without_url)? {
        if body.len() + chunk.len() > max { return Err(TooLarge.into()); }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Locators come from the channel, never from message text or a model.
async fn fetch_url(url: &str, max: usize) -> Result<Vec<u8>> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        bail!("media locator is not an HTTP URL");
    }
    let response = MEDIA_HTTP.get(url).send().await.map_err(reqwest::Error::without_url)?
        .error_for_status().map_err(reqwest::Error::without_url)?;
    read_capped(response, max).await
}

#[derive(Debug, Clone)]
pub struct Envelope {
    /// Platform notice rendered as a transcript line, not a chat message.
    pub notice: bool,
    /// `telegram` or `qq`.
    pub platform: &'static str,
    /// Platform-native send target. Telegram: the chat id. QQ: `private:<user>` or `group:<group>`.
    pub chat_id: String,
    /// Platform-native id of the inbound message, when supplied by the channel.
    pub message_id: Option<String>,
    /// Platform-native numeric user id, as a string.
    pub sender_id: String,
    pub sender_name: String,
    pub sender_is_bot: bool,
    pub is_group: bool,
    /// Someone @-ed him or replied to one of his messages.
    pub mentions_me: bool,
    pub text: String,
    /// Top-level media references plus images in a quoted message; empty when the channel supplies none.
    pub attachments: Vec<Attachment>,
    /// When the channel received it. Messages can wait in the event queue while a reply is
    /// generated; the debounce window measures gaps from arrival, not from dequeueing.
    pub arrived: Option<tokio::time::Instant>,
}

impl Envelope {
    /// `platform:native_id` of the sender.
    pub fn sender_key(&self) -> String {
        format!("{}:{}", self.platform, self.sender_id)
    }

    /// `platform:group_id` for group chats, the key used in `config/social.yaml`.
    pub fn group_key(&self) -> Option<String> {
        self.is_group.then(|| format!("{}:{}", self.platform, self.chat_id.trim_start_matches("group:")))
    }
}

/// Where replies go, by platform.
#[derive(Clone, Default)]
pub struct Outlets {
    pub telegram: Option<telegram::Telegram>,
    pub qq: Option<qq::QqHandle>,
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    fn serve_once(status: &str, body: Vec<u8>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/secret-token", listener.local_addr().unwrap());
        let status = status.to_string();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0; 1];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            write!(socket, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            socket.write_all(&body).unwrap();
        });
        url
    }

    #[tokio::test]
    async fn downloads_stop_at_the_limit_and_errors_hide_the_url() {
        assert_eq!(fetch_url(&serve_once("200 OK", vec![7; 8]), 8).await.unwrap(), vec![7; 8]);
        assert!(fetch_url(&serve_once("200 OK", vec![7; 9]), 8).await.unwrap_err().downcast_ref::<TooLarge>().is_some());
        let error = fetch_url(&serve_once("403 Forbidden", vec![]), 8).await.unwrap_err();
        assert!(!format!("{error:#}").contains("secret-token"), "{error:#}");
        assert!(fetch_url("file:///C:/Windows/win.ini", 8).await.is_err());
        assert!(fetch_url("C:/Windows/win.ini", 8).await.is_err());
    }

    /// Whether this TLS stack can talk to QQ's image CDN at all; a 4xx still proves the handshake.
    #[tokio::test]
    #[ignore = "requires network"]
    async fn live_qq_cdn_handshake() {
        let response = MEDIA_HTTP.get("https://multimedia.nt.qq.com.cn/download?appid=1407").send().await.unwrap();
        println!("status {}", response.status());
    }
}

impl Outlets {
    pub async fn send(&self, platform: &str, chat_id: &str, text: &str) -> Result<()> {
        if let ("qq", Some(qq)) = (platform, &self.qq) {
            return qq.send(chat_id, text).await;
        }
        self.send_reply(platform, chat_id, &[ReplyPiece::Text(text.into())], None).await
    }

    /// The attachment's bytes. QQ URLs expire, so a failed one is refreshed through NapCat once.
    pub async fn fetch_media(&self, platform: &str, attachment: &Attachment, max: usize) -> Result<Vec<u8>> {
        match platform {
            "telegram" => {
                let (Some(telegram), Some(file_id)) = (&self.telegram, &attachment.file_id) else {
                    bail!("telegram media is not retrievable");
                };
                telegram.fetch_file(file_id, max).await
            }
            "qq" => {
                let first = match &attachment.url {
                    Some(url) => fetch_url(url, max).await,
                    None => Err(anyhow::anyhow!("segment has no url")),
                };
                let error = match first {
                    Ok(bytes) => return Ok(bytes),
                    Err(e) if e.downcast_ref::<TooLarge>().is_some() => return Err(e),
                    Err(e) => e,
                };
                let (Some(qq), Some(file)) = (&self.qq, &attachment.file_id) else { return Err(error) };
                fetch_url(&qq.image_url(file).await?, max).await
            }
            other => bail!("unknown platform {other}"),
        }
    }

    /// Telegram currently ignores the optional reply target.
    pub async fn send_reply(&self, platform: &str, chat_id: &str, pieces: &[ReplyPiece], reply_to: Option<&str>) -> Result<()> {
        match platform {
            "telegram" => match &self.telegram {
                Some(tg) => tg.send(chat_id, &reply_text(pieces)).await,
                None => bail!("telegram is not connected"),
            },
            "qq" => match &self.qq {
                Some(qq) => qq.send_reply(chat_id, pieces, reply_to).await,
                None => bail!("qq is not connected"),
            },
            other => bail!("unknown platform {other}"),
        }
    }
}
