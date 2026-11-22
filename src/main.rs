mod bot;
mod channels;
mod config;
mod config_watch;
mod control;
mod error_report;
mod evolution;
mod hands;
mod mind;
mod ns;
mod social;
mod store;
mod stickers_web;
mod work;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{info, warn};
use tracing_subscriber::fmt::writer::{BoxMakeWriter, MakeWriterExt};

use bot::{Bot, Event};
use bot::ingress::{WorkItem, WorkQueue, next_work, try_handle_fast_control};
use channels::Outlets;
use channels::telegram::Telegram;
use config::Config;
use error_report::{Fault, Reporter, report};
use mind::models::Models;

async fn recv_work(events: &mut mpsc::Receiver<Event>, queued: &mut WorkQueue, acl: &config::AclCfg, session: &config::SessionCfg) -> Option<WorkItem> {
    if queued.is_empty() {
        queued.push(events.recv().await?);
    }
    // Drain only the current backlog, so a busy producer cannot postpone Ctrl+C or the cap.
    for _ in 0..events.len() {
        if let Ok(ev) = events.try_recv() {
            queued.push(ev);
        }
    }
    while let Some(deadline) = queued.deadline(acl, session) {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => break,
            ev = events.recv() => match ev {
                Some(ev) => queued.push(ev),
                None => break,
            },
        }
    }
    next_work(queued, acl, session)
}

#[tokio::main]
async fn main() -> Result<()> {
    // The runtime root's .env first, so the bot need not run from a source checkout.
    if let Some(root) = std::env::var_os("AGLI_ROOT") {
        dotenvy::from_path(PathBuf::from(root).join(".env")).ok();
    }
    dotenvy::dotenv().ok();
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("import-jsonl")) {
        return store::import::run(std::env::args_os().skip(2));
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("stickers-web")) {
        return stickers_web::run(std::env::args_os().skip(2)).await;
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("stickers-recaption")) {
        return mind::media::recaption_command(std::env::args_os().skip(2)).await;
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
    let cfg = match Config::load(&root) {
        Ok(cfg) => Arc::new(cfg),
        Err(error) => {
            let reporter = Config::owner_telegram(&root).ok().and_then(|(id, cfg)| {
                Reporter::start(Telegram::from_env(&cfg).ok().as_ref(), id)
            });
            report(Fault::StartupConfig);
            if let Some(reporter) = reporter {
                reporter.shutdown().await;
            } else {
                warn!("startup error could not be sent to the Telegram owner");
            }
            return Err(error);
        }
    };
    let telegram = Telegram::from_env(&cfg.channels.telegram);
    let reporter = Reporter::start(telegram.as_ref().ok(), cfg.acl.owner.telegram_user_id);
    if reporter.is_none() {
        warn!("Telegram owner notifications unavailable; check bot token, proxy and owner user ID");
    }
    let result = run(cfg, telegram).await;
    if let Some(reporter) = reporter { reporter.shutdown().await; }
    result
}

async fn run(cfg: Arc<Config>, telegram: Result<Telegram>) -> Result<()> {
    let mut config_watcher = config_watch::ConfigWatcher::new(&cfg)
        .inspect_err(|_| report(Fault::StartupConfig))?;
    let owner = &cfg.acl.owner;
    if owner.telegram_user_id == 0 && owner.qq_user_id == 0 {
        warn!("没有设置主人的 ID（OWNER_TELEGRAM_ID / OWNER_QQ_ID）：没人能使用主人权限，日志会打印发信人的 ID");
    }
    let store = store::open(&cfg.runtime.mongo, &cfg.data_dir())
        .inspect_err(|_| report(Fault::StartupStore))?;
    let models = Arc::new(Models::new(&cfg, store.clone()).await
        .inspect_err(|_| report(Fault::StartupModels))?);

    let (events_tx, mut events) = mpsc::channel::<Event>(256);
    let (inbound_tx, mut inbound) = mpsc::channel(256);
    let mut outlets = Outlets::default();

    match telegram {
        Ok(mut tg) => match tg.identify().await {
            Ok(()) => {
                outlets.telegram = Some(tg.clone());
                tokio::spawn(tg.run(inbound_tx.clone()));
                info!("telegram connected");
            }
            Err(e) => {
                report(Fault::TelegramIdentity);
                warn!("telegram unavailable: {e:#}");
            }
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
        report(Fault::StartupChannels);
        anyhow::bail!("没有可用的聊天渠道：请配置 TELEGRAM_BOT_TOKEN 或 ONEBOT_URL");
    }

    let control = control::ControlPlane::load(store.as_ref());
    let (cfg_tx, cfg_rx) = tokio::sync::watch::channel(cfg.clone());
    let mut bot = Bot::new(cfg.clone(), control.clone(), store.clone(), models, outlets.clone(), events_tx.clone())
        .inspect_err(|_| report(Fault::StartupRuntime))?;
    let watchdog_handle = control::spawn_watchdog(control.clone(), cfg_rx.clone(), events_tx.clone(), Duration::from_secs(15));

    // The bounded actor channel can fill while a turn is running. Keep that backpressure
    // in the relay so ingress continues looking for owner controls in arrival order.
    let (dispatch, mut pending) = mpsc::unbounded_channel();
    let relay = {
        let tx = events_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = pending.recv().await {
                if tx.send(event).await.is_err() { break }
            }
        })
    };
    let forwarder = {
        let control = control.clone();
        tokio::spawn(async move {
            let mut shutdown = control.shutdown_token();
            loop {
                let env = tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    env = inbound.recv() => env,
                };
                let Some(env) = env else { break };
                let cfg = cfg_rx.borrow().clone();
                if try_handle_fast_control(&cfg, store.as_ref(), &outlets, &control, &dispatch, &env).await {
                    continue;
                }
                if dispatch.send(Event::Inbound(env)).is_err() { break }
            }
        })
    };
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

    let mut queued = WorkQueue::new();
    let mut config_poll = tokio::time::interval(Duration::from_secs(2));
    config_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    bot.start_media();
    info!("{}", evolution::describe_release(&cfg.data_dir()));
    info!("AGLI is up");
    let signal = tokio::signal::ctrl_c();
    tokio::pin!(signal);
    loop {
        let active_cfg = bot.cfg();
        tokio::select! {
            _ = &mut signal => break,
            _ = config_poll.tick() => {
                if bot.reload_config(&mut config_watcher) { cfg_tx.send_replace(bot.cfg()); }
            },
            work = recv_work(&mut events, &mut queued, &active_cfg.acl, &active_cfg.runtime.session) => match work {
                Some(work) => tokio::select! {
                    _ = &mut signal => break,
                    _ = bot.handle(work) => {}
                },
                None => break,
            },
        }
    }
    control.shutdown();
    relay.abort();
    control.jobs().stop_all().await;
    let _ = watchdog_handle.await;
    let _ = forwarder.await;
    info!("bye");
    Ok(())
}
