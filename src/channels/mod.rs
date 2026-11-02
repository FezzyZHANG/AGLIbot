//! Chat platforms, normalised to `Envelope`; replies go out through `Outlets`.

pub mod qq;
pub mod telegram;

use anyhow::{Result, bail};

/// A platform media reference; QQ URLs expire within hours.
/// NapCat can refetch via `get_image {file}`, `get_record {file, out_format}`,
/// or `get_file {file_id}`. References are not persisted to the store.
#[allow(dead_code)] // Carried for future multimodal consumers.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub kind: AttachmentKind,
    pub url: Option<String>,
    pub file_id: Option<String>,
    pub name: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum AttachmentKind {
    Image,
    Sticker,
    Voice,
    Video,
    File,
}

#[derive(Debug, Clone)]
pub struct Envelope {
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
    /// Top-level media references; empty when the channel supplies none.
    #[allow(dead_code)] // Carried for future multimodal consumers.
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
    pub async fn send(&self, platform: &str, chat_id: &str, text: &str) -> Result<()> {
        match platform {
            "telegram" => match &self.telegram {
                Some(tg) => tg.send(chat_id, text).await,
                None => bail!("telegram is not connected"),
            },
            "qq" => match &self.qq {
                Some(qq) => qq.send(chat_id, text).await,
                None => bail!("qq is not connected"),
            },
            other => bail!("unknown platform {other}"),
        }
    }
}
