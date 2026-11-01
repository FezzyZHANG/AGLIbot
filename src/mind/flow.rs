//! Flow as bucket sort (docs/DESIGN.md 4c). While he is focused on one namespace, inputs from
//! other namespaces are bucketed instead of breaking the focus; on exit the buckets are worked
//! through one namespace at a time, so each reuses its own cached prefix.
//!
//! The bucket key is the namespace, which comes from where an input arrived; no model call is
//! needed to tag chat messages. State is persisted to `data/state/flow.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ns::Namespace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    /// Same namespace as the focus: merge at the next step boundary.
    Related,
    /// Another namespace: hold, handle together later.
    Pending,
    /// The owner: always gets through; an instant reflection decides whether to switch.
    Urgent,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Flow {
    pub focus: Option<Namespace>,
    pub task_id: Option<String>,
    pub title: Option<String>,
    pub since: Option<DateTime<Utc>>,
    /// Held inputs per namespace (count only; the messages themselves are in the store).
    pub pending: BTreeMap<String, usize>,
}

pub struct Exit {
    pub minutes: i64,
    pub pending: BTreeMap<String, usize>,
}

impl Flow {
    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("state").join("flow.json")
    }

    pub fn load(data_dir: &Path) -> Flow {
        std::fs::read_to_string(Self::path(data_dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let path = Self::path(data_dir);
        std::fs::create_dir_all(path.parent().unwrap_or(data_dir))?;
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn is_focused(&self) -> bool {
        self.focus.is_some()
    }

    pub fn enter(&mut self, ns: Namespace, task_id: &str, title: &str, now: DateTime<Utc>) {
        self.focus = Some(ns);
        self.task_id = Some(task_id.into());
        self.title = Some(title.into());
        self.since = Some(now);
    }

    pub fn classify(&self, ns: &Namespace, from_owner: bool) -> Option<Bucket> {
        let focus = self.focus.as_ref()?;
        Some(if from_owner {
            Bucket::Urgent
        } else if focus == ns {
            Bucket::Related
        } else {
            Bucket::Pending
        })
    }

    pub fn hold(&mut self, ns: &Namespace) {
        *self.pending.entry(ns.to_string()).or_default() += 1;
    }

    pub fn exit(&mut self, now: DateTime<Utc>) -> Exit {
        let minutes = self.since.map(|s| (now - s).num_minutes()).unwrap_or(0);
        self.task_id = None;
        let exit = Exit { minutes, pending: std::mem::take(&mut self.pending) };
        self.focus = None;
        self.title = None;
        self.since = None;
        exit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_follow_the_focus() {
        let mut f = Flow::default();
        let work: Namespace = "project/pvsnp".parse().unwrap();
        let group: Namespace = "group/qq:1".parse().unwrap();
        assert_eq!(f.classify(&group, false), None);
        f.enter(work.clone(), "t1", "P vs NP", Utc::now());
        assert_eq!(f.classify(&group, false), Some(Bucket::Pending));
        assert_eq!(f.classify(&work, false), Some(Bucket::Related));
        assert_eq!(f.classify(&group, true), Some(Bucket::Urgent));
        f.hold(&group);
        f.hold(&group);
        let exit = f.exit(Utc::now());
        assert_eq!(exit.pending["group/qq:1"], 2);
        assert!(!f.is_focused() && f.pending.is_empty());
    }
}
