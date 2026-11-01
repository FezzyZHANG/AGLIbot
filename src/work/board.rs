//! The work board (`work/board.yaml`) and one directory per task (`work/tasks/<id>/`).
//! Tasks outlive conversations: brief, plan, log (breakpoints), sources, claims and report all
//! live on disk, so work can resume after a restart and the owner can read everything.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Research,
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Approved and waiting to be started.
    Inbox,
    Active,
    /// Waiting for the owner (requests from others, or a stopped task).
    Waiting,
    Done,
    Dropped,
}

impl TaskStatus {
    pub fn is_open(self) -> bool {
        matches!(self, Self::Inbox | Self::Active | Self::Waiting)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRef {
    pub platform: String,
    pub chat_id: String,
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub kind: TaskKind,
    pub status: TaskStatus,
    /// person_id of whoever asked.
    pub requester: String,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub origin: Option<ThreadRef>,
    #[serde(default)]
    pub threads: Vec<ThreadRef>,
    /// Frozen at creation / status change; ordinary log appends must not change L2.
    #[serde(default)]
    pub status_log: String,
}

#[derive(Default, Serialize, Deserialize)]
struct BoardFile {
    tasks: Vec<Task>,
}

pub struct Board {
    dir: PathBuf,
    file: BoardFile,
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

impl Board {
    pub fn load(work_dir: &Path) -> Result<Self> {
        let path = work_dir.join("board.yaml");
        let file = match std::fs::read_to_string(&path) {
            Ok(text) => serde_yaml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BoardFile::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Board { dir: work_dir.to_path_buf(), file })
    }

    fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.dir.join("board.yaml"), serde_yaml::to_string(&self.file)?)?;
        Ok(())
    }

    pub fn tasks(&self) -> &[Task] {
        &self.file.tasks
    }

    pub fn get(&self, id: &str) -> Option<&Task> {
        self.file.tasks.iter().find(|t| t.id == id)
    }

    pub fn task_dir(&self, id: &str) -> PathBuf {
        self.dir.join("tasks").join(id)
    }

    pub fn add(&mut self, title: &str, kind: TaskKind, requester: &str, status: TaskStatus, origin: Option<ThreadRef>) -> Result<Task> {
        let now = Utc::now();
        let base = format!("{}-{}", now.with_timezone(&Local).format("%Y%m%d-%H%M"), slug(title));
        let mut id = base.clone();
        let mut n = 2;
        while self.get(&id).is_some() {
            id = format!("{base}-{n}");
            n += 1;
        }
        let task = Task {
            id: id.clone(),
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
        };
        let dir = self.task_dir(&id);
        std::fs::create_dir_all(dir.join("sources"))?;
        std::fs::write(dir.join("brief.md"), format!("# {}\n\n请求者：{}\n类型：{:?}\n时间：{}\n", task.title, requester, kind, now.with_timezone(&Local)))?;
        self.file.tasks.push(task.clone());
        self.save()?;
        self.log(&id, "任务创建")?;
        Ok(task)
    }

    pub fn set_status(&mut self, id: &str, status: TaskStatus, note: &str) -> Result<()> {
        let line = format!("状态 → {status:?}{}", if note.is_empty() { String::new() } else { format!("：{note}") });
        let t = self.file.tasks.iter_mut().find(|t| t.id == id).ok_or_else(|| anyhow!("no task {id}"))?;
        if t.status != status {
            t.status_log = line.clone();
        }
        t.status = status;
        t.updated = Utc::now();
        if !note.is_empty() {
            t.note = note.into();
        }
        self.save()?;
        self.log(id, &line)
    }

    pub fn follow_up(&mut self, id: &str, body: &str, thread: ThreadRef) -> Result<()> {
        use std::io::Write;
        let t = self.get(id).ok_or_else(|| anyhow!("没有这个任务"))?;
        anyhow::ensure!(t.status.is_open(), "任务已关闭（{:?}）", t.status);
        anyhow::ensure!(!body.trim().is_empty(), "补充内容为空");
        let entry = format!("补充（{} / {} / {}）：{}", thread.platform, thread.chat_id,
            thread.message_id.as_deref().unwrap_or("无消息编号"), body.trim());
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.task_dir(id).join("followups.md"))?;
        writeln!(f, "- {} {entry}", Local::now().format("%m-%d %H:%M"))?;
        self.log(id, &entry)?;
        let t = self.file.tasks.iter_mut().find(|t| t.id == id).unwrap();
        if !t.threads.contains(&thread) {
            t.threads.push(thread);
        }
        t.updated = Utc::now();
        self.save()
    }

    /// Compact cards depend only on the open set and status snapshots, never live logs.
    pub fn cards(&self) -> String {
        self.tasks().iter().filter(|t| t.status.is_open()).map(|t| {
            let title: String = t.title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(48).collect();
            let head = format!("- {} [{:?}] {title} | ", t.id, t.status);
            let log: String = t.status_log.split_whitespace().collect::<Vec<_>>().join(" ").chars()
                .take(160usize.saturating_sub(head.chars().count())).collect();
            format!("{head}{log}")
        }).collect::<Vec<_>>().join("\n")
    }

    /// Append a breakpoint line to `log.md`.
    pub fn log(&self, id: &str, line: &str) -> Result<()> {
        use std::io::Write;
        let path = self.task_dir(id).join("log.md");
        std::fs::create_dir_all(path.parent().unwrap_or(&self.dir))?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(f, "- {} {line}", Local::now().format("%m-%d %H:%M"))?;
        Ok(())
    }

    /// Tasks left `Active` by a crash go back to the inbox; their log keeps the breakpoint.
    pub fn recover(&mut self) -> Result<usize> {
        let stale: Vec<String> =
            self.file.tasks.iter().filter(|t| t.status == TaskStatus::Active).map(|t| t.id.clone()).collect();
        for id in &stale {
            self.set_status(id, TaskStatus::Inbox, "上次运行中断，回到待办")?;
        }
        Ok(stale.len())
    }

    pub fn summary(&self) -> String {
        let open: Vec<&Task> = self
            .file
            .tasks
            .iter()
            .filter(|t| matches!(t.status, TaskStatus::Inbox | TaskStatus::Active | TaskStatus::Waiting))
            .collect();
        if open.is_empty() {
            return "看板上没有未完成的任务。".into();
        }
        open.iter()
            .map(|t| format!("- {} [{:?}/{:?}] {}{}", t.id, t.kind, t.status, t.title, if t.note.is_empty() { String::new() } else { format!("（{}）", t.note) }))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_board_defaults_conversation_fields() {
        let file: BoardFile = serde_yaml::from_str("tasks:\n  - id: old\n    title: 旧研究\n    kind: research\n    status: inbox\n    requester: owner\n    created: 2026-11-01T00:00:00Z\n    updated: 2026-11-01T00:00:00Z\n").unwrap();
        let t = &file.tasks[0];
        assert!(t.origin.is_none());
        assert!(t.threads.is_empty());
        assert!(t.status_log.is_empty());
    }

    #[test]
    fn cards_change_only_with_open_set_or_status_and_survive_reload() {
        let dir = std::env::temp_dir().join(format!("aglibot-test-cards-{}", std::process::id()));
        let mut b = Board::load(&dir).unwrap();
        let origin = ThreadRef { platform: "telegram".into(), chat_id: "42".into(), message_id: Some("10".into()) };
        let t = b.add(&"研究标题".repeat(40), TaskKind::Research, "owner", TaskStatus::Inbox, Some(origin.clone())).unwrap();
        let initial = b.cards();
        assert!(initial.contains(&t.id) && initial.contains("任务创建"));
        assert!(initial.chars().count() <= 160);
        b.log(&t.id, "开始搜集资料").unwrap();
        b.follow_up(&t.id, "改成方案 B", origin.clone()).unwrap();
        b.set_status(&t.id, TaskStatus::Inbox, "仍在等待").unwrap();
        assert_eq!(b.cards(), initial);
        let mut b = Board::load(&dir).unwrap();
        assert_eq!(b.cards(), initial);
        assert_eq!(b.get(&t.id).unwrap().origin.as_ref(), Some(&origin));
        assert_eq!(b.get(&t.id).unwrap().threads, [origin]);
        b.set_status(&t.id, TaskStatus::Active, "正在研究").unwrap();
        let active = b.cards();
        assert_ne!(active, initial);
        assert!(active.contains("正在研究"));
        b.log(&t.id, "另一条进展").unwrap();
        assert_eq!(Board::load(&dir).unwrap().cards(), active);
        b.set_status(&t.id, TaskStatus::Done, "完成").unwrap();
        assert!(b.cards().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tasks_get_dirs_logs_and_recover() {
        let dir = std::env::temp_dir().join(format!("aglibot-test-board-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut b = Board::load(&dir).unwrap();
        let t = b.add("查 OpenAI 关于 P vs NP 的说法", TaskKind::Research, "owner", TaskStatus::Inbox, None).unwrap();
        assert!(b.task_dir(&t.id).join("brief.md").exists());
        b.set_status(&t.id, TaskStatus::Active, "").unwrap();
        let mut b = Board::load(&dir).unwrap();
        assert_eq!(b.recover().unwrap(), 1);
        assert_eq!(b.get(&t.id).unwrap().status, TaskStatus::Inbox);
        let log = std::fs::read_to_string(b.task_dir(&t.id).join("log.md")).unwrap();
        assert!(log.contains("中断"));
        let t2 = b.add("查 OpenAI 关于 P vs NP 的说法", TaskKind::Research, "owner", TaskStatus::Inbox, None).unwrap();
        assert_ne!(t.id, t2.id);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
