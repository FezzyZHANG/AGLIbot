//! Persistent task metadata and local artifacts.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{Local, Utc};
use serde::Deserialize;

use crate::store::{RECOVERY_NOTE, Store, task_status_line};
pub use crate::store::{
    TaskFollowup, TaskKind, TaskRecord as Task, TaskStatus, TaskVerdict, ThreadRef,
};

#[derive(Deserialize)]
struct BoardFile {
    tasks: Vec<Task>,
}

#[derive(Clone)]
pub struct Board {
    dir: PathBuf,
    store: Store,
}

fn slug(title: &str) -> String {
    let s: String = title
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
        .take(24)
        .collect::<String>()
        .trim()
        .replace(' ', "-");
    if s.is_empty() { "task".into() } else { s }
}

fn validate_id(id: &str) -> Result<()> {
    use std::path::Component;
    anyhow::ensure!(
        !id.contains(['\\', '/', ':'])
            && matches!(Path::new(id).components().next(), Some(Component::Normal(part)) if part == id),
        "任务编号无效：{id}"
    );
    Ok(())
}

impl Board {
    pub async fn open(work_dir: &Path, store: Store) -> Result<Self> {
        const IMPORT: &str = "tasks:legacy_board_import";
        let path = work_dir.join("board.yaml");
        let flag = store.kv_get(IMPORT).await?;
        if flag != Some(serde_json::Value::Bool(true))
            && (flag.is_some() || store.tasks().await?.is_empty())
        {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let file: BoardFile = serde_yaml::from_str(&text)
                        .with_context(|| format!("parsing {}", path.display()))?;
                    for task in &file.tasks {
                        validate_id(&task.id)?;
                    }
                    store.kv_set(IMPORT, false.into()).await?;
                    for task in file.tasks {
                        if store.task(&task.id).await?.is_none() {
                            store.save_task(&task).await?;
                        }
                    }
                    store.kv_set(IMPORT, true.into()).await?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && flag.is_none() => {}
                Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
            }
        }
        Ok(Self {
            dir: work_dir.to_owned(),
            store,
        })
    }

    pub async fn tasks(&self) -> Result<Vec<Task>> {
        self.store.tasks().await
    }

    pub async fn get(&self, id: &str) -> Result<Option<Task>> {
        self.store.task(id).await
    }

    pub fn task_dir(&self, id: &str) -> PathBuf {
        self.dir.join("tasks").join(id)
    }

    pub async fn add(
        &self,
        title: &str,
        kind: TaskKind,
        requester: &str,
        status: TaskStatus,
        origin: Option<ThreadRef>,
    ) -> Result<Task> {
        let now = Utc::now();
        let base = format!(
            "{}-{}",
            now.with_timezone(&Local).format("%Y%m%d-%H%M"),
            slug(title)
        );
        std::fs::create_dir_all(self.dir.join("tasks"))?;
        let mut suffix = 1;
        let (id, dir) = loop {
            let id = if suffix == 1 {
                base.clone()
            } else {
                format!("{base}-{suffix}")
            };
            let dir = self.task_dir(&id);
            if self.get(&id).await?.is_none() {
                match std::fs::create_dir(&dir) {
                    Ok(()) => break (id, dir),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e.into()),
                }
            }
            suffix += 1;
        };
        let task = Task {
            id,
            title: title.trim().to_string(),
            kind,
            status,
            requester: requester.into(),
            created: now,
            updated: now,
            note: String::new(),
            origin,
            threads: Vec::new(),
            status_log: "任务创建".into(),
            followups: Vec::new(),
            current_round: 0,
            last_verdict: None,
        };
        std::fs::create_dir(dir.join("sources"))?;
        std::fs::write(
            dir.join("brief.md"),
            format!(
                "# {}\n\n请求者：{}\n类型：{:?}\n时间：{}\n",
                task.title,
                requester,
                kind,
                now.with_timezone(&Local)
            ),
        )?;
        self.log(&task.id, "任务创建")?;
        self.store.save_task(&task).await?;
        Ok(task)
    }

    pub async fn set_status(&self, id: &str, status: TaskStatus, note: &str) -> Result<()> {
        self.set_status_with_verdict(id, status, note, None).await
    }

    pub async fn set_status_with_verdict(
        &self,
        id: &str,
        status: TaskStatus,
        note: &str,
        verdict: Option<TaskVerdict>,
    ) -> Result<()> {
        self.store
            .set_task_status(id, status, note, Utc::now(), verdict)
            .await?;
        self.log(id, &task_status_line(status, note))
    }

    pub async fn follow_up(&self, id: &str, body: &str, thread: ThreadRef) -> Result<Task> {
        let entry = format!(
            "补充（{} / {} / {}）：{}",
            thread.platform,
            thread.chat_id,
            thread.message_id.as_deref().unwrap_or("无消息编号"),
            body.trim()
        );
        let task = self
            .store
            .append_task_followup(id, body, thread, Utc::now())
            .await?;
        self.log(id, &entry)?;
        Ok(task)
    }

    pub async fn mark_followups_consumed(
        &self,
        id: &str,
        up_to_seq: u64,
        round: u32,
    ) -> Result<()> {
        self.store
            .mark_task_followups_consumed(id, up_to_seq, round)
            .await
    }

    /// Compact cards depend only on the open set and status snapshots, never live logs.
    pub async fn cards(&self) -> Result<String> {
        Ok(self
            .tasks()
            .await?
            .iter()
            .filter(|t| t.status.is_open())
            .map(|t| {
                let title: String = t
                    .title
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(48)
                    .collect();
                let head = format!("- {} [{:?}] {title} | ", t.id, t.status);
                let log: String = t
                    .status_log
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(160usize.saturating_sub(head.chars().count()))
                    .collect();
                format!("{head}{log}")
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// Append a breakpoint line to `log.md`.
    pub fn log(&self, id: &str, line: &str) -> Result<()> {
        use std::io::Write;
        validate_id(id)?;
        let dir = self.task_dir(id);
        std::fs::create_dir_all(&dir)?;
        anyhow::ensure!(!super::linked(&dir.symlink_metadata()?), "任务目录包含链接");
        let path = dir.join("log.md");
        match path.symlink_metadata() {
            Ok(meta) => anyhow::ensure!(
                meta.is_file() && !super::linked(&meta),
                "任务日志不是普通文件"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let entry = format!("- {} {line}\n", Local::now().format("%m-%d %H:%M"));
        f.write_all(entry.as_bytes())?;
        Ok(())
    }

    pub async fn recover(&self) -> Result<usize> {
        let recovered = self.store.recover_active_tasks(Utc::now()).await?;
        for id in &recovered {
            self.log(id, &task_status_line(TaskStatus::Inbox, RECOVERY_NOTE))?;
        }
        Ok(recovered.len())
    }

    pub async fn summary(&self) -> Result<String> {
        let open = self
            .tasks()
            .await?
            .into_iter()
            .filter(|t| t.status.is_open())
            .map(|t| {
                let note = if t.note.is_empty() {
                    String::new()
                } else {
                    format!("（{}）", t.note)
                };
                format!("- {} [{:?}/{:?}] {}{note}", t.id, t.kind, t.status, t.title)
            })
            .collect::<Vec<_>>();
        if open.is_empty() {
            Ok("看板上没有未完成的任务。".into())
        } else {
            Ok(open.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_board_defaults_conversation_fields() {
        let file: BoardFile = serde_yaml::from_str("tasks:\n  - id: old\n    title: 旧研究\n    kind: research\n    status: inbox\n    requester: owner\n    created: 2026-11-01T00:00:00Z\n    updated: 2026-11-01T00:00:00Z\n").unwrap();
        let t = &file.tasks[0];
        assert!(t.followups.is_empty());
        assert_eq!(t.current_round, 0);
        assert!(t.last_verdict.is_none());
        assert!(t.origin.is_none());
        assert!(t.threads.is_empty());
        assert!(t.status_log.is_empty());
    }

    #[tokio::test]
    async fn cards_change_only_with_open_set_or_status_and_survive_reload() {
        let dir = std::env::temp_dir().join(format!("aglibot-test-cards-{}", std::process::id()));
        let store = Store::memory();
        let b = Board::open(&dir, store.clone()).await.unwrap();
        let origin = ThreadRef {
            platform: "telegram".into(),
            chat_id: "42".into(),
            message_id: Some("10".into()),
        };
        let t = b
            .add(
                &"研究标题".repeat(40),
                TaskKind::Research,
                "owner",
                TaskStatus::Inbox,
                Some(origin.clone()),
            )
            .await
            .unwrap();
        let initial = b.cards().await.unwrap();
        assert!(initial.contains(&t.id) && initial.contains("任务创建"));
        assert!(initial.chars().count() <= 160);
        b.log(&t.id, "开始搜集资料").unwrap();
        b.follow_up(&t.id, "改成方案 B", origin.clone())
            .await
            .unwrap();
        b.set_status(&t.id, TaskStatus::Inbox, "仍在等待")
            .await
            .unwrap();
        assert_eq!(b.cards().await.unwrap(), initial);
        let b = Board::open(&dir, store.clone()).await.unwrap();
        assert_eq!(b.cards().await.unwrap(), initial);
        assert_eq!(
            b.get(&t.id).await.unwrap().unwrap().origin.as_ref(),
            Some(&origin)
        );
        assert_eq!(b.get(&t.id).await.unwrap().unwrap().threads, [origin]);
        b.set_status(&t.id, TaskStatus::Active, "正在研究")
            .await
            .unwrap();
        let active = b.cards().await.unwrap();
        assert_ne!(active, initial);
        assert!(active.contains("正在研究"));
        b.log(&t.id, "另一条进展").unwrap();
        assert_eq!(
            Board::open(&dir, store)
                .await
                .unwrap()
                .cards()
                .await
                .unwrap(),
            active
        );
        b.set_status(&t.id, TaskStatus::Done, "完成").await.unwrap();
        assert!(b.cards().await.unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn tasks_get_dirs_logs_and_recover() {
        let dir = std::env::temp_dir().join(format!("aglibot-test-board-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::memory();
        let b = Board::open(&dir, store.clone()).await.unwrap();
        let t = b
            .add(
                "查 OpenAI 关于 P vs NP 的说法",
                TaskKind::Research,
                "owner",
                TaskStatus::Inbox,
                None,
            )
            .await
            .unwrap();
        assert!(b.task_dir(&t.id).join("brief.md").exists());
        b.set_status(&t.id, TaskStatus::Active, "").await.unwrap();
        let b = Board::open(&dir, store).await.unwrap();
        assert_eq!(b.recover().await.unwrap(), 1);
        assert_eq!(
            b.get(&t.id).await.unwrap().unwrap().status,
            TaskStatus::Inbox
        );
        let log = std::fs::read_to_string(b.task_dir(&t.id).join("log.md")).unwrap();
        assert!(log.contains("中断"));
        let t2 = b
            .add(
                "查 OpenAI 关于 P vs NP 的说法",
                TaskKind::Research,
                "owner",
                TaskStatus::Inbox,
                None,
            )
            .await
            .unwrap();
        assert_ne!(t.id, t2.id);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn imports_legacy_metadata_once_without_writing_the_source() {
        let dir = std::env::temp_dir().join(format!("agli-legacy-board-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("board.yaml");
        let text = "tasks:\n  - id: old\n    title: 旧研究\n    kind: research\n    status: active\n    requester: owner\n    created: 2026-11-01T00:00:00Z\n    updated: 2026-11-01T00:00:00Z\n";
        std::fs::write(&path, text).unwrap();
        let store = Store::memory();
        let board = Board::open(&dir, store.clone()).await.unwrap();
        assert_eq!(board.recover().await.unwrap(), 1);
        assert_eq!(board.get("old").await.unwrap().unwrap().note, RECOVERY_NOTE);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let changed = text.replace("旧研究", "改变了标题");
        std::fs::write(&path, &changed).unwrap();
        let reopened = Board::open(&dir, store).await.unwrap();
        let tasks = reopened.tasks().await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "旧研究");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), changed);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_followups_get_unique_sequences_and_stable_consumption_receipts() {
        let dir =
            std::env::temp_dir().join(format!("agli-concurrent-board-{}", std::process::id()));
        let board = Board::open(&dir, Store::memory()).await.unwrap();
        let task = board
            .add(
                "followups",
                TaskKind::Research,
                "owner",
                TaskStatus::Active,
                None,
            )
            .await
            .unwrap();
        let thread = ThreadRef {
            platform: "qq".into(),
            chat_id: "private:1".into(),
            message_id: None,
        };
        let appends = (0..24)
            .map(|n| {
                let board = board.clone();
                let id = task.id.clone();
                let thread = thread.clone();
                tokio::spawn(async move {
                    board
                        .follow_up(&id, &format!("补充 {n}"), thread)
                        .await
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        for append in appends {
            append.await.unwrap();
        }

        board
            .mark_followups_consumed(&task.id, 12, 2)
            .await
            .unwrap();
        board
            .mark_followups_consumed(&task.id, 20, 1)
            .await
            .unwrap();
        let saved = board.get(&task.id).await.unwrap().unwrap();
        assert_eq!(saved.current_round, 2);
        assert_eq!(
            saved.followups.iter().map(|f| f.seq).collect::<Vec<_>>(),
            (1..=24).collect::<Vec<_>>()
        );
        assert!(
            saved.followups[..12]
                .iter()
                .all(|f| f.consumed_round == Some(2))
        );
        assert!(
            saved.followups[12..20]
                .iter()
                .all(|f| f.consumed_round == Some(1))
        );
        assert_eq!(saved.unconsumed_followups().len(), 4);
        assert_eq!(saved.threads, std::slice::from_ref(&thread));
        assert!(
            board
                .follow_up(&task.id, " ", thread.clone())
                .await
                .is_err()
        );
        let verdict = TaskVerdict {
            review_id: 7,
            round: 2,
            verdict: "通过".into(),
            reasons: "证据充分".into(),
            fixes: String::new(),
        };
        board
            .set_status_with_verdict(&task.id, TaskStatus::Done, "", Some(verdict))
            .await
            .unwrap();
        assert!(board.follow_up(&task.id, "closed", thread).await.is_err());
        assert_eq!(
            board
                .get(&task.id)
                .await
                .unwrap()
                .unwrap()
                .last_verdict
                .unwrap()
                .review_id,
            7
        );
        assert!(!dir.join("board.yaml").exists());
        assert!(!board.task_dir(&task.id).join("followups.md").exists());
        let log = std::fs::read_to_string(board.task_dir(&task.id).join("log.md")).unwrap();
        assert_eq!(
            log.lines().filter(|line| line.contains("补充（")).count(),
            24
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
