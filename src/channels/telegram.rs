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
        let http = http_client(cfg)?;
        Ok(Telegram {
            http,
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

fn http_client(cfg: &TelegramCfg) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(cfg.poll_timeout_s + 10));
    if let Some(proxy) = &cfg.proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy).context("invalid Telegram proxy URL")?);
    }
    builder.build().context("building Telegram HTTP client")
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

    #[tokio::test]
    async fn polling_and_sending_use_configured_proxy() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = TelegramCfg {
            poll_timeout_s: 1,
            proxy_env: None,
            proxy: Some(format!("http://{}", listener.local_addr().unwrap())),
        };
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                let mut byte = [0; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let headers = String::from_utf8(request).unwrap();
                let length = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                }).unwrap_or(0);
                socket.read_exact(&mut vec![0; length]).unwrap();
                requests.push(headers.lines().next().unwrap().to_string());
                let body = r#"{"ok":true,"result":[]}"#;
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        let mut tg = Telegram {
            http: http_client(&cfg).unwrap(),
            base: "http://telegram.invalid/bot-test".into(),
            offset: 0,
            poll_timeout_s: cfg.poll_timeout_s,
        };
        assert!(tg.poll().await.unwrap().is_empty());
        tg.send("42", "proxy test").await.unwrap();
        let requests = server.join().unwrap();
        assert!(requests[0].starts_with("GET http://telegram.invalid/bot-test/getUpdates?"));
        assert_eq!(requests[1], "POST http://telegram.invalid/bot-test/sendMessage HTTP/1.1");
    }

    /// Reads bot metadata only; does not send messages or consume pending updates.
    #[tokio::test]
    #[ignore = "requires TELEGRAM_BOT_TOKEN, local proxy and network"]
    async fn live_telegram_proxy() -> Result<()> {
        dotenvy::dotenv().ok();
        let cfg = crate::config::Config::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))?;
        let tg = Telegram::from_env(&cfg.channels.telegram)?;
        let response = tg.http.get(format!("{}/getMe", tg.base)).send().await.map_err(reqwest::Error::without_url)?;
        let value: Value = response.json().await.map_err(reqwest::Error::without_url)?;
        assert_eq!(value["ok"], true, "Telegram getMe failed");
        Ok(())
    }
}
