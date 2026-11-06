mod agli;
mod channels;
mod config;
mod config_watch;
mod evolution;
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
use tracing_subscriber::fmt::writer::{BoxMakeWriter, MakeWriterExt};

use agli::{Agli, Event, WorkItem, WorkQueue, next_work};
use channels::Outlets;
use channels::telegram::Telegram;
use config::Config;
use mind::models::Models;

async fn recv_work(events: &mut mpsc::Receiver<Event>, queued: &mut WorkQueue, acl: &config::AclCfg, session: &config::SessionCfg) -> Option<WorkItem> {
    if queued.is_empty() {
        queued.push(events.recv().await?, &session.debounce);
    }
    // Drain only the current backlog, so a busy producer cannot postpone Ctrl+C or the cap.
    for _ in 0..events.len() {
        if let Ok(ev) = events.try_recv() {
            queued.push(ev, &session.debounce);
        }
    }
    while let Some(deadline) = queued.deadline(acl, session) {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => break,
            ev = events.recv() => match ev {
                Some(ev) => queued.push(ev, &session.debounce),
                None => break,
            },
        }
    }
    next_work(queued, acl, session)
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("evolution-restart") {
        let mut args = std::env::args().skip(2);
        let parent = args.next().ok_or_else(|| anyhow::anyhow!("missing parent PID"))?.parse()?;
        let request = args.next().ok_or_else(|| anyhow::anyhow!("missing restart request"))?;
        return evolution::restart::helper(parent, request.into());
    }
    // The runtime root's .env first, so the bot need not run from a source checkout.
    if let Some(root) = std::env::var_os("AGLI_ROOT") {
        dotenvy::from_path(PathBuf::from(root).join(".env")).ok();
    }
    dotenvy::dotenv().ok();
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("import-jsonl")) {
        return store::import::run(std::env::args_os().skip(2));
    }
    // start-bot.ps1 sets this so the log shows in the bot's console window and still lands in a file.
    let log_writer = match std::env::var_os("AGLI_LOG_FILE") {
        Some(path) => {
            let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
            BoxMakeWriter::new(std::io::stdout.and(Arc::new(file)))
        }
        None => BoxMakeWriter::new(std::io::stdout),
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(log_writer)
        .init();

    let root = std::env::var_os("AGLI_ROOT").map(PathBuf::from).unwrap_or(std::env::current_dir()?);
    let cfg = Arc::new(Config::load(&root)?);
    let mut config_watcher = config_watch::ConfigWatcher::new(&cfg)?;
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

    let mut queued = WorkQueue::with_store(store.clone());
    let mut agli = Agli::new(cfg.clone(), store, models, outlets, events_tx)?;
    let mut config_poll = tokio::time::interval(Duration::from_secs(2));
    config_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    agli.acknowledge_evolution_start()?;
    info!("{}", evolution::restart::describe_release(&cfg.data_dir()));
    info!("AGLI is up");
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = config_poll.tick() => {
                agli.reload_config(&mut config_watcher);
                agli.poll_evolution().await;
            },
            work = recv_work(&mut events, &mut queued, &cfg.acl, &cfg.runtime.session) => match work {
                Some(work) => agli.handle(work).await,
                None => break,
            },
        }
        if let Some(restart) = agli.take_evolution_restart() {
            match restart.launch().await {
                Ok(()) => break,
                Err(e) => {
                    let _ = restart.receipt("failed", Some(&format!("{e:#}")));
                    warn!("evolution restart failed; old bot remains running: {e:#}");
                }
            }
        }
    }
    info!("bye");
    Ok(())
}
