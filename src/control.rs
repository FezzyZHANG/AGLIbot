use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use crate::Result;
use crate::bot::Event;
use crate::config::{Config, HandsCfg};
use crate::store::Store;
use crate::work::research::Outcome;

pub struct CancelSource {
    tx: watch::Sender<bool>,
    token: CancelToken,
}

#[derive(Clone)]
pub struct CancelToken {
    rx: watch::Receiver<bool>,
}

impl CancelSource {
    pub fn new() -> (Self, CancelToken) {
        let (tx, rx) = watch::channel(false);
        let token = CancelToken { rx };
        (Self { tx, token: token.clone() }, token)
    }

    pub fn token(&self) -> CancelToken { self.token.clone() }

    pub fn cancel(&self) { let _ = self.tx.send(true); }
}

impl CancelToken {
    pub fn is_cancelled(&self) -> bool { *self.rx.borrow() }

    pub async fn cancelled(&mut self) {
        while !*self.rx.borrow_and_update() {
            if self.rx.changed().await.is_err() {
                // A dropped source cannot issue another cancellation.
                std::future::pending::<()>().await;
            }
        }
    }
}

struct ActiveJob {
    id: String,
    title: String,
    log_path: PathBuf,
    started: Instant,
    last_growth: Instant,
    last_log: Option<(u64, Option<SystemTime>)>,
    terminal_claimed: Arc<AtomicBool>,
    cancel: CancelSource,
    handle: Option<JoinHandle<()>>,
}

fn log_stamp(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().ok()))
}

impl ActiveJob {
    fn can_stop(&self) -> bool {
        self.handle.as_ref().is_some_and(|handle| !handle.is_finished())
            && !self.terminal_claimed.load(Ordering::Acquire)
    }

    fn claim_stop(&self) -> bool {
        self.can_stop() && !self.terminal_claimed.swap(true, Ordering::AcqRel)
    }

    fn overdue(&mut self, now: Instant, cfg: &HandsCfg) -> Option<String> {
        if !self.can_stop() { return None }
        // Owner follow-ups append to log.md and count as activity.
        if let Some(stamp) = log_stamp(&self.log_path) && Some(stamp) != self.last_log {
            self.last_log = Some(stamp);
            self.last_growth = now;
        }
        if now.saturating_duration_since(self.started) >= Duration::from_secs(cfg.total_budget_minutes.saturating_mul(60)) {
            Some(format!("看门狗暂停：总运行时长已达 {} 分钟预算", cfg.total_budget_minutes))
        } else if now.saturating_duration_since(self.last_growth) >= Duration::from_secs(cfg.stall_minutes.saturating_mul(60)) {
            Some(format!("看门狗暂停：log.md 已 {} 分钟没有增长或更新", cfg.stall_minutes))
        } else {
            None
        }
    }

    fn abort(&mut self) -> Option<JoinHandle<()>> {
        self.cancel.cancel();
        let handle = self.handle.take()?;
        handle.abort();
        Some(handle)
    }
}

#[derive(Default)]
pub struct JobRegistry(Mutex<BTreeMap<String, ActiveJob>>);

impl JobRegistry {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, ActiveJob>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn spawn<F>(&self, id: String, title: String, log_path: PathBuf, events: mpsc::Sender<Event>, make_work: impl FnOnce(CancelToken) -> F)
    where F: Future<Output = Outcome> + Send + 'static {
        let mut jobs = self.lock();
        assert!(!jobs.contains_key(&id), "job {id} is already registered");
        let started = Instant::now();
        let last_log = log_stamp(&log_path);
        let (cancel, token) = CancelSource::new();
        let terminal_claimed = Arc::new(AtomicBool::new(false));
        let finished = terminal_claimed.clone();
        let work = make_work(token);
        let handle = tokio::spawn(async move {
            let outcome = work.await;
            if !finished.swap(true, Ordering::AcqRel) {
                let _ = events.send(Event::JobDone(outcome)).await;
            }
        });
        jobs.insert(id.clone(), ActiveJob {
            id, title, log_path, started, last_growth: started, last_log,
            terminal_claimed, cancel, handle: Some(handle),
        });
    }

    pub fn complete(&self, id: &str) -> Option<Instant> { self.lock().remove(id).map(|job| job.started) }
    pub fn ids(&self) -> Vec<String> { self.lock().keys().cloned().collect() }
    pub fn len(&self) -> usize { self.lock().len() }
    pub fn is_empty(&self) -> bool { self.lock().is_empty() }
    pub fn contains(&self, id: &str) -> bool { self.lock().contains_key(id) }
    pub fn first_title(&self) -> Option<String> { self.lock().values().next().map(|job| job.title.clone()) }
    pub fn summary(&self) -> String {
        if self.is_empty() { return "无".into() }
        let ids = self.ids();
        if ids.is_empty() { "无".into() } else { ids.join("、") }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub async fn stop_one(&self, id: &str) -> Option<String> {
        let mut job = {
            let mut jobs = self.lock();
            if !jobs.get(id)?.claim_stop() { return None }
            jobs.remove(id)?
        };
        if let Some(handle) = job.abort() { let _ = handle.await; }
        Some(job.id)
    }

    pub async fn stop_all(&self) -> Vec<String> {
        let stopped: Vec<_> = self.lock().extract_if(.., |_, job| job.claim_stop())
            .map(|(_, job)| (job.id.clone(), job)).collect();
        abort_and_join(stopped).await
    }

    pub async fn take_overdue(&self, now: Instant, cfg: &HandsCfg) -> Vec<(String, String)> {
        let overdue = {
            let mut jobs = self.lock();
            let claimed: Vec<_> = jobs.values_mut().filter_map(|job| {
                let reason = job.overdue(now, cfg)?;
                job.claim_stop().then(|| (job.id.clone(), reason))
            }).collect();
            claimed.into_iter().map(|(id, reason)| {
                let job = jobs.remove(&id).unwrap();
                ((id, reason), job)
            }).collect()
        };
        abort_and_join(overdue).await
    }
}

async fn abort_and_join<T>(stopped: Vec<(T, ActiveJob)>) -> Vec<T> {
    let handles: Vec<_> = stopped.into_iter().map(|(result, mut job)| (result, job.abort())).collect();
    let mut results = Vec::with_capacity(handles.len());
    for (result, handle) in handles {
        if let Some(handle) = handle { let _ = handle.await; }
        results.push(result);
    }
    results
}

pub struct ControlPlane {
    frozen: AtomicBool,
    jobs: Arc<JobRegistry>,
    shutdown: CancelSource,
    admission: tokio::sync::Mutex<()>,
}

impl ControlPlane {
    pub async fn load(store: &Store) -> Arc<Self> {
        Self::new(store.kv_get("frozen").await.ok().flatten().and_then(|v| v.as_bool()).unwrap_or(false))
    }

    pub fn new(frozen: bool) -> Arc<Self> {
        Arc::new(Self {
            frozen: AtomicBool::new(frozen), jobs: Arc::default(),
            shutdown: CancelSource::new().0, admission: tokio::sync::Mutex::new(()),
        })
    }

    pub fn is_frozen(&self) -> bool { self.frozen.load(Ordering::Acquire) }

    pub async fn set_frozen(&self, store: &Store, freeze: bool) -> Result<bool> {
        let _guard = self.admission.lock().await;
        let before = self.is_frozen();
        if before != freeze {
            store.kv_set("frozen", freeze.into()).await?;
            self.frozen.store(freeze, Ordering::Release);
        }
        Ok(before)
    }

    /// Serialize the board transition and registration with a freeze or shutdown.
    pub async fn admit_job<T>(&self, start: impl AsyncFnOnce() -> T) -> Option<T> {
        let _guard = self.admission.lock().await;
        if self.is_frozen() || self.shutdown.token.is_cancelled() {
            return None;
        }
        Some(start().await)
    }

    pub fn jobs(&self) -> &Arc<JobRegistry> { &self.jobs }
    pub async fn shutdown(&self) {
        let _guard = self.admission.lock().await;
        self.shutdown.cancel();
    }
    pub fn shutdown_token(&self) -> CancelToken { self.shutdown.token() }
}

pub fn spawn_watchdog(control: Arc<ControlPlane>, cfg_rx: watch::Receiver<Arc<Config>>, events: mpsc::Sender<Event>, tick: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut shutdown = control.shutdown_token();
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut pending = VecDeque::new();
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                permit = events.reserve(), if !pending.is_empty() => {
                    let Ok(permit) = permit else { break };
                    permit.send(pending.pop_front().unwrap());
                }
                _ = interval.tick() => {
                    let hands = cfg_rx.borrow().runtime.hands.clone();
                    for (task_id, reason) in control.jobs().take_overdue(Instant::now(), &hands).await {
                        pending.push_back(Event::JobWatchdog { task_id, reason });
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::pending;
    use tokio::sync::oneshot;
    use tokio::time::{advance, timeout};

    #[tokio::test]
    async fn cancellation_wakes_waiters_and_is_visible_to_late_tokens() {
        let (source, mut token) = CancelSource::new();
        assert!(!token.is_cancelled());
        let (waiting, ready) = oneshot::channel();
        let waiter = tokio::spawn(async move {
            waiting.send(()).unwrap();
            token.cancelled().await;
            assert!(token.is_cancelled());
        });
        ready.await.unwrap();
        source.cancel();
        timeout(Duration::from_secs(1), waiter).await.unwrap().unwrap();
        assert!(source.token().is_cancelled());

        let (source, token) = CancelSource::new();
        drop(token);
        source.cancel();
        assert!(source.token().is_cancelled());
    }

    #[tokio::test]
    async fn stop_all_aborts_and_joins_jobs_without_polling_the_actor() {
        let jobs = JobRegistry::default();
        let (events, mut rx) = mpsc::channel(1);
        for id in ["a", "b"] {
            let (send, started) = oneshot::channel();
            jobs.spawn(id.into(), id.into(), PathBuf::new(), events.clone(), |token| async move {
                send.send(token).unwrap_or_else(|_| panic!("test dropped started receiver"));
                pending().await
            });
            assert!(!started.await.unwrap().is_cancelled());
        }
        let tokens: Vec<_> = jobs.lock().values().map(|job| job.cancel.token()).collect();
        let stopped = timeout(Duration::from_secs(1), jobs.stop_all()).await.unwrap();
        assert_eq!(stopped, ["a", "b"]);
        assert!(tokens.iter().all(CancelToken::is_cancelled));
        assert!(jobs.is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn watchdog_reaps_overdue_jobs_even_with_a_full_actor_channel() {
        let control = ControlPlane::new(false);
        let mut cfg = Config::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        cfg.runtime.hands.stall_minutes = 1;
        let (_cfg_tx, cfg_rx) = watch::channel(Arc::new(cfg));
        let (events, mut rx) = mpsc::channel(1);
        events.try_send(Event::Control(crate::bot::ingress::ControlEvent::Stopped { stopped: vec![] })).unwrap();
        let handle = spawn_watchdog(control.clone(), cfg_rx, events.clone(), Duration::from_millis(20));
        let jobs = control.jobs();
        jobs.spawn("first".into(), "first".into(), PathBuf::new(), events.clone(), |_| pending());
        advance(Duration::from_secs(30)).await;
        jobs.spawn("second".into(), "second".into(), PathBuf::new(), events, |_| pending());
        for id in ["first", "second"] {
            advance(Duration::from_secs(30) + Duration::from_millis(20)).await;
            timeout(Duration::from_secs(1), async {
                while jobs.contains(id) { tokio::time::sleep(Duration::from_millis(20)).await; }
            }).await.unwrap();
        }
        assert!(matches!(rx.recv().await, Some(Event::Control(_))));
        for id in ["first", "second"] {
            let event = timeout(Duration::from_secs(1), rx.recv()).await.unwrap();
            let Some(Event::JobWatchdog { task_id, reason }) = event else { panic!("lost watchdog event") };
            assert_eq!(task_id, id);
            assert!(reason.contains("log.md"));
        }
        assert!(rx.try_recv().is_err());
        control.shutdown().await;
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_claim_suppresses_late_completion() {
        let jobs = JobRegistry::default();
        let (events, mut rx) = mpsc::channel(1);
        let (finish, result) = oneshot::channel();
        jobs.spawn("id".into(), "title".into(), PathBuf::new(), events, |_| async { result.await.unwrap() });
        let job = jobs.lock().remove("id").unwrap();
        // A worker already in its final synchronous poll can return after cancellation claims it.
        assert!(job.claim_stop());
        finish.send(Outcome {
            task_id: "id".into(), title: "title".into(), ok: true, clarification: None,
            paused: None, summary: "late result".into(), verdict: None,
        }).unwrap();
        job.handle.unwrap().await.unwrap();
        assert!(rx.recv().await.is_none());
    }
}
