//! Chat platforms, normalised to `Envelope`; replies go out through `Outlets`.

pub mod qq;
pub mod telegram;
mod urls;
pub(crate) use urls::public_url;

use std::borrow::Cow;

use anyhow::{Result, anyhow, bail};

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
    Link,
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

#[derive(Debug, Clone)]
pub struct Envelope {
    /// Only text written by the sender, excluding forwarded messages and reply previews.
    pub authored_text: String,
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

impl Outlets {
    /// Channel file references can expire. Resolve QQ ids using the originating chat.
    pub async fn fetch_attachment(&self, platform: &str, chat_id: &str, attachment: &Attachment, max: usize) -> Result<Vec<u8>> {
        if attachment.size.is_some_and(|size| size > max as u64) {
            return Err(TooLarge.into());
        }
        match platform {
            "telegram" => {
                let (Some(telegram), Some(file_id)) = (&self.telegram, &attachment.file_id) else {
                    bail!("telegram attachment is not retrievable");
                };
                telegram.fetch_file(file_id, max).await
            }
            "qq" => {
                let error = if let Some(url) = &attachment.url {
                    match urls::download(url, max).await {
                        Ok(bytes) => return Ok(bytes),
                        Err(e) if e.is::<TooLarge>() => return Err(e),
                        Err(e) => e,
                    }
                } else {
                    anyhow!("附件缺少下载链接")
                };
                let (Some(qq), Some(file)) = (&self.qq, &attachment.file_id) else {
                    return Err(error);
                };
                let url = if matches!(attachment.kind, AttachmentKind::Image | AttachmentKind::Sticker) {
                    qq.image_url(file).await?
                } else {
                    qq.file_url(chat_id, file).await?
                };
                urls::download(&url, max).await
            }
            other => bail!("unknown platform {other}"),
        }
    }
    pub async fn send(&self, platform: &str, chat_id: &str, text: &str) -> Result<()> {
        if let ("qq", Some(qq)) = (platform, &self.qq) {
            return qq.send(chat_id, text).await;
        }
        self.send_reply(platform, chat_id, &[ReplyPiece::Text(text.into())], None).await
    }

    /// Only QQ sends stickers so far.
    pub async fn send_sticker(&self, platform: &str, chat_id: &str, bytes: &[u8], reply_to: Option<&str>) -> Result<()> {
        match (platform, &self.qq) {
            ("qq", Some(qq)) => qq.send_sticker(chat_id, bytes, reply_to).await,
            _ => bail!("{platform} cannot send stickers"),
        }
    }

    /// Only QQ has reactions so far.
    pub async fn react(&self, platform: &str, message_id: &str, emoji_id: &str) -> Result<()> {
        match (platform, &self.qq) {
            ("qq", Some(qq)) => qq.react(message_id, emoji_id).await,
            _ => bail!("{platform} cannot react"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn attachments_share_size_and_destination_checks() {
        let outlets = Outlets::default();
        for kind in [AttachmentKind::Image, AttachmentKind::Sticker, AttachmentKind::File] {
            let mut attachment = Attachment {
                kind, url: Some("http://127.0.0.1/secret-token".into()), size: Some(9),
                file_id: None, name: None, quoted_from: None, media_id: None,
            };
            let error = outlets.fetch_attachment("qq", "group:34", &attachment, 8).await.unwrap_err();
            assert!(error.is::<TooLarge>());
            attachment.size = None;
            let error = outlets.fetch_attachment("qq", "group:34", &attachment, 8).await.unwrap_err();
            assert!(error.to_string().contains("公网"));
            assert!(!format!("{error:#}").contains("secret-token"));
        }
    }

    #[tokio::test]
    async fn telegram_file_resolution_uses_the_same_destination_check() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let telegram = telegram::Telegram::fixture(format!("http://{}/botSECRET", listener.local_addr().unwrap()));
        let app = axum::Router::new().route("/botSECRET/getFile", axum::routing::get(|| async {
            axum::Json(serde_json::json!({"ok":true,"result":{"file_path":"photos/test.jpg"}}))
        }));
        let server = tokio::spawn(async { axum::serve(listener, app).await.unwrap() });
        let outlets = Outlets { telegram: Some(telegram), qq: None };
        let attachment = Attachment {
            kind: AttachmentKind::Image, url: None, file_id: Some("photo-id".into()),
            name: None, size: None, quoted_from: None, media_id: None,
        };
        let error = outlets.fetch_attachment("telegram", "34", &attachment, 8).await.unwrap_err();
        assert!(error.to_string().contains("公网"));
        assert!(!format!("{error:#}").contains("SECRET"));
        server.abort();
    }
}
