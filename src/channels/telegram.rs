//! Telegram Bot API over long polling.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::Envelope;
use crate::config::TelegramCfg;

/// Telegram's limit is 4096 UTF-16 units; stay well under it in chars.
const CHUNK_CHARS: usize = 3500;

pub struct Telegram {
    http: reqwest::Client,
    base: String,
    offset: i64,
    poll_timeout_s: u64,
}

impl Telegram {
    pub fn from_env(cfg: &TelegramCfg) -> Result<Self> {
        let token = std::env::var("TELEGRAM_BOT_TOKEN").context("TELEGRAM_BOT_TOKEN is not set")?;
        Ok(Telegram {
            http: reqwest::Client::new(),
            base: format!("https://api.telegram.org/bot{token}"),
            offset: 0,
            poll_timeout_s: cfg.poll_timeout_s,
        })
    }

    pub async fn poll(&mut self) -> Result<Vec<Envelope>> {
        let resp = self
            .http
            .get(format!("{}/getUpdates", self.base))
            .query(&[
                ("timeout", self.poll_timeout_s.to_string()),
                ("offset", self.offset.to_string()),
                ("allowed_updates", r#"["message"]"#.to_string()),
            ])
            .timeout(Duration::from_secs(self.poll_timeout_s + 10))
            .send()
            .await?;
        let v: Value = resp.json().await?;
        if v.get("ok").and_then(Value::as_bool) != Some(true) {
            bail!("getUpdates failed: {v}");
        }
        let mut out = Vec::new();
        for update in v["result"].as_array().into_iter().flatten() {
            if let Some(id) = update.get("update_id").and_then(Value::as_i64) {
                self.offset = self.offset.max(id + 1);
            }
            if let Some(env) = parse_message(update.get("message").unwrap_or(&Value::Null)) {
                out.push(env);
            }
        }
        Ok(out)
    }

    pub async fn send(&self, chat_id: &str, text: &str) -> Result<()> {
        for chunk in chunks(text, CHUNK_CHARS) {
            let v: Value = self
                .http
                .post(format!("{}/sendMessage", self.base))
                .json(&json!({"chat_id": chat_id, "text": chunk}))
                .send()
                .await?
                .json()
                .await?;
            if v.get("ok").and_then(Value::as_bool) != Some(true) {
                bail!("sendMessage failed: {v}");
            }
        }
        Ok(())
    }
}

fn parse_message(msg: &Value) -> Option<Envelope> {
    let text = msg.get("text")?.as_str()?.to_string();
    let from = msg.get("from")?;
    let chat = msg.get("chat")?;
    let chat_type = chat.get("type")?.as_str()?;
    Some(Envelope {
        platform: "telegram",
        chat_id: chat.get("id")?.as_i64()?.to_string(),
        sender_id: from.get("id")?.as_i64()?.to_string(),
        sender_name: from.get("first_name").and_then(Value::as_str).unwrap_or_default().to_string(),
        sender_is_bot: from.get("is_bot").and_then(Value::as_bool).unwrap_or(false),
        is_group: chat_type != "private",
        text,
    })
}

fn chunks(text: &str, max_chars: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars.chunks(max_chars).map(|c| c.iter().collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_private_text_message() {
        let msg = json!({
            "message_id": 1,
            "from": {"id": 42, "is_bot": false, "first_name": "F"},
            "chat": {"id": 42, "type": "private"},
            "text": "在吗"
        });
        let env = parse_message(&msg).unwrap();
        assert_eq!((env.chat_id.as_str(), env.sender_id.as_str(), env.is_group), ("42", "42", false));
        assert!(parse_message(&json!({"from": {"id": 1}, "chat": {"id": 1, "type": "private"}})).is_none());
    }

    #[test]
    fn chunks_respect_char_boundaries() {
        let text = "汉".repeat(7);
        assert_eq!(chunks(&text, 3), ["汉汉汉", "汉汉汉", "汉"]);
        assert!(chunks("", 3).is_empty());
    }
}
