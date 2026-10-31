mod channels;
mod config;
mod mind;
mod ns;
mod social;
mod store;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tracing::{error, info, warn};

use channels::Envelope;
use channels::telegram::Telegram;
use config::Config;
use mind::speak::Mind;
use social::identity::{Sender, classify};
use store::jsonl::JsonlStore;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let root = std::env::var_os("AGLI_ROOT").map(PathBuf::from).unwrap_or(std::env::current_dir()?);
    let cfg = Arc::new(Config::load(&root)?);
    if cfg.acl.owner.telegram_user_id == 0 {
        warn!(
            "{} 未设置：所有消息都会被忽略，日志会打印发信人的 ID",
            cfg.acl.owner.telegram_user_id_env
        );
    }
    let store = Arc::new(JsonlStore::open(&cfg.data_dir())?);
    let mut mind = Mind::new(cfg.clone(), store)?;
    let mut tg = Telegram::from_env(&cfg.channels.telegram)?;
    info!("AGLI is up");

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            polled = tg.poll() => match polled {
                Ok(envs) => {
                    for env in envs {
                        handle(&cfg, &mut mind, &tg, &env).await;
                    }
                }
                Err(e) => {
                    warn!("telegram poll failed: {e:#}");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            },
        }
    }
    info!("bye");
    Ok(())
}

async fn handle(cfg: &Config, mind: &mut Mind, tg: &Telegram, env: &Envelope) {
    match classify(&cfg.acl, env) {
        Sender::Owner if !env.is_group => {
            let reply = match mind.reply_owner(env).await {
                Ok(text) => text,
                Err(e) => {
                    error!("reply failed: {e:#}");
                    "（刚才走神了，你再说一遍？）".to_string()
                }
            };
            if let Err(e) = tg.send(&env.chat_id, &reply).await {
                error!("send failed: {e:#}");
            }
        }
        // Groups and other people arrive with the social layer (phase 4).
        _ => info!(
            "ignored message from telegram user {} ({}) in chat {}",
            env.sender_id, env.sender_name, env.chat_id
        ),
    }
}
