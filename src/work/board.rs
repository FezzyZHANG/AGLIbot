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

    pub fn add(&mut self, title: &str, kind: TaskKind, requester: &str, status: TaskStatus) -> Result<Task> {
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
        let t = self.file.tasks.iter_mut().find(|t| t.id == id).ok_or_else(|| anyhow!("no task {id}"))?;
        t.status = status;
        t.updated = Utc::now();
        if !note.is_empty() {
            t.note = note.into();
        }
        self.save()?;
        self.log(id, &format!("状态 → {status:?}{}", if note.is_empty() { String::new() } else { format!("：{note}") }))
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
    fn tasks_get_dirs_logs_and_recover() {
        let dir = std::env::temp_dir().join(format!("aglibot-test-board-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut b = Board::load(&dir).unwrap();
        let t = b.add("查 OpenAI 关于 P vs NP 的说法", TaskKind::Research, "owner", TaskStatus::Inbox).unwrap();
        assert!(b.task_dir(&t.id).join("brief.md").exists());
        b.set_status(&t.id, TaskStatus::Active, "").unwrap();
        let mut b = Board::load(&dir).unwrap();
        assert_eq!(b.recover().unwrap(), 1);
        assert_eq!(b.get(&t.id).unwrap().status, TaskStatus::Inbox);
        let log = std::fs::read_to_string(b.task_dir(&t.id).join("log.md")).unwrap();
        assert!(log.contains("中断"));
        let t2 = b.add("查 OpenAI 关于 P vs NP 的说法", TaskKind::Research, "owner", TaskStatus::Inbox).unwrap();
        assert_ne!(t.id, t2.id);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
