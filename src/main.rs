mod agli;
mod channels;
mod config;
mod hands;
mod mind;
mod ns;
mod social;
mod store;
mod work;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{info, warn};

use agli::{Agli, Event};
use channels::Outlets;
use channels::telegram::Telegram;
use config::Config;
use mind::models::Models;

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
    let owner = &cfg.acl.owner;
    if owner.telegram_user_id == 0 && owner.qq_user_id == 0 {
        warn!("没有设置主人的 ID（OWNER_TELEGRAM_ID / OWNER_QQ_ID）：没人能使用主人权限，日志会打印发信人的 ID");
    }
    let store = store::open(&cfg.runtime.mongo, &cfg.data_dir())?;
    let models = Arc::new(Models::new(&cfg, store.clone()).await?);

    let (events_tx, mut events) = mpsc::channel::<Event>(256);
    let (inbound_tx, mut inbound) = mpsc::channel(256);
    let mut outlets = Outlets::default();

    match Telegram::from_env(&cfg.channels.telegram) {
        Ok(mut tg) => match tg.identify().await {
            Ok(()) => {
                outlets.telegram = Some(tg.clone());
                tokio::spawn(tg.run(inbound_tx.clone()));
                info!("telegram connected");
            }
            Err(e) => warn!("telegram unavailable: {e:#}"),
        },
        Err(e) => warn!("telegram disabled: {e:#}"),
    }
    let qq = &cfg.channels.qq;
    match (&qq.url, qq.enabled) {
        (Some(url), true) => outlets.qq = Some(channels::qq::spawn(url.clone(), qq.token.clone(), inbound_tx.clone())),
        (None, true) => warn!("qq disabled: {} is not set", qq.url_env),
        _ => {}
    }
    drop(inbound_tx);
    if outlets.telegram.is_none() && outlets.qq.is_none() {
        anyhow::bail!("没有可用的聊天渠道：请配置 TELEGRAM_BOT_TOKEN 或 ONEBOT_URL");
    }

    {
        let tx = events_tx.clone();
        tokio::spawn(async move {
            while let Some(env) = inbound.recv().await {
                if tx.send(Event::Inbound(env)).await.is_err() {
                    break;
                }
            }
        });
    }
    {
        let tx = events_tx.clone();
        let every = Duration::from_secs(cfg.runtime.heartbeat.tick_s.max(10));
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(every);
            loop {
                interval.tick().await;
                if tx.send(Event::Tick).await.is_err() {
                    break;
                }
            }
        });
    }

    let mut agli = Agli::new(cfg.clone(), store, models, outlets, events_tx)?;
    info!("AGLI is up");
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            ev = events.recv() => match ev {
                Some(ev) => agli.handle(ev).await,
                None => break,
            },
        }
    }
    info!("bye");
    Ok(())
}
