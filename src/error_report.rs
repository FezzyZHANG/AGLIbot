use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

use crate::channels::telegram::Telegram;

const COOLDOWN: Duration = Duration::from_secs(60);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(6);
const QUEUE_SIZE: usize = 64;

// Only fixed catalog entries cross the notification boundary. Underlying errors may
// contain request URLs, message contents or credentials and stay with the local caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Fault {
    StartupConfig,
    StartupStore,
    StartupModels,
    StartupRuntime,
    StartupChannels,
    TelegramIdentity,
    TelegramPoll,
    QqConnection,
    Delivery,
    ChatReply,
    WebRead,
    FileRead,
    TaskExecution,
    ReviewModel,
    EventHandling,
    StateSave,
    Background,
    EvolutionRestart,
}

impl Fault {
    fn entry(self) -> (&'static str, &'static str) {
        match self {
            Self::StartupConfig => ("E1001", "启动时无法读取配置或建立配置监视。"),
            Self::StartupStore => ("E1002", "启动时无法打开数据存储。"),
            Self::StartupModels => ("E1003", "启动时无法初始化模型服务。"),
            Self::StartupRuntime => ("E1004", "机器人运行状态初始化失败。"),
            Self::StartupChannels => ("E1005", "启动时没有可用的聊天渠道。"),
            Self::TelegramIdentity => ("E2001", "Telegram 机器人身份验证失败。"),
            Self::TelegramPoll => ("E2002", "Telegram 消息接收失败，正在重试。"),
            Self::QqConnection => ("E2003", "QQ 渠道连接失败或中断，正在重连。"),
            Self::Delivery => ("E2004", "聊天消息投递失败。"),
            Self::ChatReply => ("E3001", "聊天回复生成失败。"),
            Self::WebRead => ("E3002", "网页读取或处理失败。"),
            Self::FileRead => ("E3003", "文件读取或处理失败。"),
            Self::TaskExecution => ("E3004", "任务执行失败。"),
            Self::ReviewModel => ("E3005", "审查模型调用失败，已转交主人审查。"),
            Self::EventHandling => ("E4001", "运行事件处理失败。"),
            Self::StateSave => ("E4002", "运行状态保存失败。"),
            Self::Background => ("E4003", "后台作业执行失败。"),
            Self::EvolutionRestart => ("E4004", "进化重启失败，原进程继续运行。"),
        }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (code, description) = self.entry();
        write!(f, "错误码 {code}　{description}")
    }
}

impl std::error::Error for Fault {}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum OwnerState {
    Frozen,
    Unfrozen,
}

impl OwnerState {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Frozen => {
                "状态码 S1001　已冻结：不再主动找人、不开始任务、不动用工具。聊天照常。"
            }
            Self::Unfrozen => "状态码 S1002　已解冻。",
        }
    }
}

#[derive(Clone, Copy)]
enum Notice {
    Error(Fault),
    State(OwnerState),
}

impl fmt::Display for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error(fault) => write!(f, "{fault}\n/freeze\n/unfreeze"),
            Self::State(state) => f.write_str(state.message()),
        }
    }
}

struct Queue {
    tx: mpsc::Sender<Notice>,
    last: Mutex<HashMap<Fault, Instant>>,
}

impl Queue {
    fn submit(&self, notice: Notice) {
        let result = match notice {
            Notice::Error(fault) => {
                let mut last = self.last.lock().expect("owner report cooldown");
                let now = Instant::now();
                if last
                    .get(&fault)
                    .is_some_and(|t| now.duration_since(*t) < COOLDOWN)
                {
                    return;
                }
                let result = self.tx.try_send(notice);
                if result.is_ok() {
                    last.insert(fault, now);
                }
                result
            }
            Notice::State(_) => self.tx.try_send(notice),
        };
        if result.is_err() {
            tracing::warn!(%notice, "owner notification queue unavailable");
        }
    }
}

static REPORTS: OnceLock<Queue> = OnceLock::new();

pub(crate) fn report(fault: Fault) {
    tracing::error!(%fault, "runtime error");
    #[cfg(test)]
    let _ = CAPTURE.try_with(|list| list.borrow_mut().push(fault));
    if let Some(queue) = REPORTS.get() {
        queue.submit(Notice::Error(fault));
    }
}

pub(crate) fn notify_state(state: OwnerState) {
    tracing::info!(state = state.message(), "owner state changed");
    if let Some(queue) = REPORTS.get() {
        queue.submit(Notice::State(state));
    }
}

#[cfg(test)]
tokio::task_local!(static CAPTURE: std::cell::RefCell<Vec<Fault>>);

#[cfg(test)]
pub(crate) async fn capture<T>(future: impl Future<Output = T>) -> (T, Vec<Fault>) {
    CAPTURE
        .scope(Default::default(), async {
            let output = future.await;
            (output, CAPTURE.with(|list| list.take()))
        })
        .await
}

pub(crate) struct Reporter {
    stop: oneshot::Sender<()>,
    worker: JoinHandle<()>,
}

impl Reporter {
    pub(crate) fn start(tg: Option<&Telegram>, owner_id: i64) -> Option<Self> {
        let tg = tg.filter(|_| owner_id > 0)?;
        let (queue, reporter) = Self::spawn(tg.clone(), owner_id);
        REPORTS
            .set(queue)
            .unwrap_or_else(|_| panic!("owner reporter initialized twice"));
        Some(reporter)
    }

    fn spawn(tg: Telegram, owner_id: i64) -> (Queue, Self) {
        let (tx, mut rx) = mpsc::channel::<Notice>(QUEUE_SIZE);
        let (stop, mut stopping) = oneshot::channel();
        let worker = tokio::spawn(async move {
            let chat = owner_id.to_string();
            let mut draining = false;
            loop {
                let notice = tokio::select! {
                    biased;
                    _ = &mut stopping, if !draining => {
                        rx.close();
                        draining = true;
                        continue;
                    }
                    next = rx.recv() => match next {
                        Some(notice) => notice,
                        None => break,
                    }
                };
                let reason = match timeout(SEND_TIMEOUT, tg.send(&chat, &notice.to_string())).await
                {
                    Ok(Ok(())) => continue,
                    Err(_) => "timeout",
                    Ok(Err(_)) => "request failed",
                };
                // A timeout may follow acceptance by Telegram. Do not duplicate an
                // unconfirmed send or report its own failure through this queue.
                tracing::warn!(%notice, reason, "owner Telegram notification delivery unconfirmed");
            }
        });
        (
            Queue {
                tx,
                last: Mutex::default(),
            },
            Self { stop, worker },
        )
    }

    pub(crate) async fn shutdown(mut self) {
        let _ = self.stop.send(());
        if timeout(SHUTDOWN_TIMEOUT, &mut self.worker).await.is_err() {
            self.worker.abort();
            tracing::warn!("timed out draining owner notifications");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::post};
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn sends_only_catalog_text_to_owner_even_after_a_delivery_failure() {
        let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", socket.local_addr().unwrap());
        let (sent, mut received) = mpsc::unbounded_channel();
        let mock =
            Router::new().route(
                "/sendMessage",
                post(move |Json(body): Json<Value>| {
                    let failed = body["text"].as_str().unwrap().contains("E3001");
                    sent.send(body).unwrap();
                    async move {
                        Json(json!({"ok": !failed, "description": "sensitive upstream response"}))
                    }
                }),
            );
        let server = tokio::spawn(async move { axum::serve(socket, mock).await.unwrap() });
        let (queue, reporter) = Reporter::spawn(Telegram::fixture(base), 42);

        for _ in 0..QUEUE_SIZE * 2 {
            queue.submit(Notice::Error(Fault::ChatReply));
        }
        queue.submit(Notice::Error(Fault::StateSave));
        for state in [OwnerState::Frozen, OwnerState::Unfrozen, OwnerState::Frozen] {
            queue.submit(Notice::State(state));
        }
        reporter.shutdown().await;
        let mut requests = vec![];
        while let Ok(body) = received.try_recv() {
            requests.push(body);
        }
        assert_eq!(
            requests,
            [
                "错误码 E3001　聊天回复生成失败。\n/freeze\n/unfreeze",
                "错误码 E4002　运行状态保存失败。\n/freeze\n/unfreeze",
                OwnerState::Frozen.message(),
                OwnerState::Unfrozen.message(),
                OwnerState::Frozen.message(),
            ]
            .map(|text| json!({"chat_id": "42", "text": text}))
        );
        server.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn full_queue_does_not_start_cooldown_and_each_code_expires_separately() {
        let (tx, mut rx) = mpsc::channel(1);
        let queue = Queue {
            tx,
            last: Mutex::default(),
        };
        let a = Notice::Error(Fault::ChatReply);
        let b = Notice::Error(Fault::Delivery);
        queue.submit(a);
        queue.submit(b);
        assert!(matches!(rx.try_recv(), Ok(Notice::Error(Fault::ChatReply))));
        tokio::time::advance(COOLDOWN / 2).await;
        queue.submit(b);
        assert!(matches!(rx.try_recv(), Ok(Notice::Error(Fault::Delivery))));
        queue.submit(a);
        assert!(rx.try_recv().is_err());
        tokio::time::advance(COOLDOWN / 2).await;
        queue.submit(b);
        queue.submit(a);
        assert!(matches!(rx.try_recv(), Ok(Notice::Error(Fault::ChatReply))));
    }

    #[tokio::test]
    async fn slow_telegram_cannot_hold_up_shutdown() {
        let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", socket.local_addr().unwrap());
        let (entered, mut requests) = mpsc::unbounded_channel();
        let mock = Router::new().route(
            "/sendMessage",
            post(move || {
                entered.send(()).unwrap();
                std::future::pending::<String>()
            }),
        );
        let server = tokio::spawn(async move { axum::serve(socket, mock).await.unwrap() });
        let (queue, reporter) = Reporter::spawn(Telegram::fixture(base), 42);
        queue.submit(Notice::Error(Fault::ChatReply));
        queue.submit(Notice::Error(Fault::Delivery));
        timeout(Duration::from_secs(2), requests.recv())
            .await
            .unwrap()
            .unwrap();
        tokio::time::pause();
        let started = Instant::now();
        reporter.shutdown().await;
        assert!(started.elapsed() <= SHUTDOWN_TIMEOUT + Duration::from_millis(1));
        server.abort();
    }
}
