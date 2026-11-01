//! Debounced config polling. The event loop owns the only mutable running config.
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::config::{Config, SocialCfg};

#[derive(Clone, PartialEq, Eq)]
struct FileState {
    modified: Option<SystemTime>,
    hash: [u8; 32],
    bytes: Vec<u8>,
}

type Snapshot = BTreeMap<String, FileState>;

fn snapshot(root: &Path) -> Result<Snapshot> {
    let mut files = Snapshot::new();
    for entry in std::fs::read_dir(root.join("config"))? {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "yaml") { continue }
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        files.insert(path.file_name().unwrap().to_string_lossy().into_owned(), FileState {
            modified: path.metadata()?.modified().ok(),
            hash: Sha256::digest(&bytes).into(),
            bytes,
        });
    }
    Ok(files)
}

pub struct ConfigWatcher {
    initial: Snapshot,
    pending: Snapshot,
    attempted: Snapshot,
    restart_logged: HashSet<(String, [u8; 32])>,
}

impl ConfigWatcher {
    pub fn new(cfg: &Config) -> Result<Self> {
        let initial = snapshot(&cfg.root)?;
        // Also compare the initial snapshot with the running values on the first poll:
        // a save may have happened between startup's Config::load and watcher creation.
        Ok(Self { pending: initial.clone(), attempted: Snapshot::new(), initial, restart_logged: HashSet::new() })
    }

    /// Two equal observations are required, including for invalid edits. A failed load
    /// is retried only after another change. Check again after loading to reject save races.
    pub fn poll(&mut self, running: &mut Arc<Config>) -> Result<Vec<String>> {
        let observed = snapshot(&running.root)?;
        if observed != self.pending {
            self.pending = observed;
            return Ok(vec![]);
        }
        if observed == self.attempted { return Ok(vec![]) }
        self.attempted = observed.clone();
        let loaded = Config::load(&running.root)?;
        if snapshot(&running.root)? != observed { return Ok(vec![]) }

        let mut messages = Vec::new();
        for (file, state) in &observed {
            let differs = match file.as_str() {
                "social.yaml" => false,
                "models.yaml" => loaded.models != running.models,
                "scenarios.yaml" => loaded.scenarios != running.scenarios,
                "acl.yaml" => loaded.acl != running.acl,
                "channels.yaml" => loaded.channels != running.channels,
                "runtime.yaml" => loaded.runtime != running.runtime,
                _ => {
                    let value: serde_yaml::Value = serde_yaml::from_slice(&state.bytes)?;
                    let old = self.initial.get(file).map(|s| serde_yaml::from_slice::<serde_yaml::Value>(&s.bytes)).transpose()?;
                    old.as_ref() != Some(&value)
                }
            };
            if differs && self.restart_logged.insert((file.clone(), state.hash)) {
                messages.push(format!("config/{file} changed; restart required"));
            }
        }
        if loaded.social != running.social {
            messages.push(format!("config/social.yaml reloaded; {}", social_diff(&running.social, &loaded.social)));
            let mut next = (**running).clone();
            next.social = loaded.social;
            *running = Arc::new(next);
        }
        for message in &messages { tracing::info!("{message}"); }
        Ok(messages)
    }
}

fn social_diff(old: &SocialCfg, new: &SocialCfg) -> String {
    let added: Vec<_> = new.groups.iter().filter(|g| !old.groups.contains(g)).collect();
    let removed: Vec<_> = old.groups.iter().filter(|g| !new.groups.contains(g)).collect();
    let mut changed = Vec::new();
    if old.ignored_groups != new.ignored_groups { changed.push("ignored_groups") }
    if old.private_from_others != new.private_from_others { changed.push("private_from_others") }
    if old.floor != new.floor { changed.push("floor") }
    if old.human_limit != new.human_limit { changed.push("human_limit") }
    if old.agent_limit != new.agent_limit { changed.push("agent_limit") }
    if old.group_reply_max != new.group_reply_max { changed.push("group_reply_max") }
    if old.interjection_max != new.interjection_max { changed.push("interjection_max") }
    if old.read_only_reflect != new.read_only_reflect { changed.push("read_only_reflect") }
    format!("groups added={added:?}, removed={removed:?}; changed={changed:?}")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    pub(crate) struct Fixture {
        pub root: PathBuf,
    }

    impl Fixture {
        pub fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("agli-config-watch-{name}-{}", std::process::id()));
            std::fs::create_dir_all(root.join("config")).unwrap();
            let fixture = Self { root };
            for (name, text) in [
                ("models.yaml", include_str!("../config/models.yaml")),
                ("scenarios.yaml", include_str!("../config/scenarios.yaml")),
                ("channels.yaml", include_str!("../config/channels.yaml")),
                ("runtime.yaml", include_str!("../config/runtime.yaml")),
                ("acl.yaml", "owner: {person_id: owner, telegram_user_id_env: AGLI_TEST_UNSET_CONFIG_OWNER}"),
            ] { fixture.write(name, text); }
            fixture.social("[]", 150);
            fixture
        }

        pub fn write(&self, file: &str, text: &str) {
            std::fs::write(self.root.join("config").join(file), text).unwrap();
        }

        pub fn social(&self, groups: &str, cap: usize) {
            self.write("social.yaml", &format!("groups: {groups}\nprivate_from_others: true\ngroup_reply_max: {cap}\nfloor: {{check_every: 3, unsolicited_cooldown_min: 30}}\nhuman_limit: {{max_replies: 5, window_min: 10}}\nagent_limit: {{max_replies: 2, window_min: 10}}\n"));
        }

        fn config(&self) -> Arc<Config> { Arc::new(Config::load(&self.root).unwrap()) }
    }

    impl Drop for Fixture {
        fn drop(&mut self) { std::fs::remove_dir_all(&self.root).unwrap(); }
    }

    #[test]
    fn stable_social_edit_swaps_only_social_and_reports_diff() {
        let f = Fixture::new("social");
        let mut cfg = f.config();
        let original = cfg.clone();
        let mut watcher = ConfigWatcher::new(&cfg).unwrap();
        f.social("['telegram:-7']", 60);
        assert!(watcher.poll(&mut cfg).unwrap().is_empty());
        assert!(Arc::ptr_eq(&cfg, &original));
        let messages = watcher.poll(&mut cfg).unwrap();
        assert_eq!(cfg.social.groups, ["telegram:-7"]);
        assert_eq!(cfg.social.group_reply_max, 60);
        assert_eq!(cfg.runtime, original.runtime);
        assert_eq!(cfg.models, original.models);
        assert_eq!(cfg.acl, original.acl);
        assert_eq!(cfg.channels, original.channels);
        assert_eq!(cfg.scenarios, original.scenarios);
        assert!(original.social.groups.is_empty());
        assert!(messages[0].contains("config/social.yaml reloaded"));
        assert!(messages[0].contains("telegram:-7") && messages[0].contains("group_reply_max"));
        assert!(watcher.poll(&mut cfg).unwrap().is_empty());
    }

    #[test]
    fn debounce_invalid_and_half_written_edits_keep_running_config() {
        let f = Fixture::new("debounce");
        let mut cfg = f.config();
        let original = cfg.clone();
        let mut watcher = ConfigWatcher::new(&cfg).unwrap();
        f.write("social.yaml", "groups: [");
        assert!(watcher.poll(&mut cfg).unwrap().is_empty());
        assert!(watcher.poll(&mut cfg).is_err());
        assert!(Arc::ptr_eq(&cfg, &original));
        assert!(watcher.poll(&mut cfg).unwrap().is_empty());
        f.social("['telegram:1']", 60);
        watcher.poll(&mut cfg).unwrap();
        f.social("['telegram:2']", 70);
        watcher.poll(&mut cfg).unwrap();
        assert!(Arc::ptr_eq(&cfg, &original));
        watcher.poll(&mut cfg).unwrap();
        assert_eq!(cfg.social.groups, ["telegram:2"]);
        assert_eq!(cfg.social.group_reply_max, 70);
    }

    #[test]
    fn content_hash_detects_edit_even_when_mtime_is_unchanged() {
        let f = Fixture::new("hash");
        let mut cfg = f.config();
        let mut watcher = ConfigWatcher::new(&cfg).unwrap();
        let path = f.root.join("config/social.yaml");
        let modified = path.metadata().unwrap().modified().unwrap();
        f.social("[]", 160);
        std::fs::File::options().write(true).open(&path).unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();
        assert_eq!(path.metadata().unwrap().modified().unwrap(), modified);
        watcher.poll(&mut cfg).unwrap();
        assert_eq!(cfg.social.group_reply_max, 150);
        watcher.poll(&mut cfg).unwrap();
        assert_eq!(cfg.social.group_reply_max, 160);
    }

    #[test]
    fn full_validation_rejects_social_reload_until_other_file_is_fixed() {
        let f = Fixture::new("validation");
        let mut cfg = f.config();
        let original = cfg.clone();
        let mut watcher = ConfigWatcher::new(&cfg).unwrap();
        f.social("['telegram:1']", 60);
        f.write("scenarios.yaml", "{}");
        watcher.poll(&mut cfg).unwrap();
        assert!(watcher.poll(&mut cfg).is_err());
        assert!(Arc::ptr_eq(&cfg, &original));
        f.write("scenarios.yaml", include_str!("../config/scenarios.yaml"));
        watcher.poll(&mut cfg).unwrap();
        watcher.poll(&mut cfg).unwrap();
        assert_eq!(cfg.social.groups, ["telegram:1"]);
    }

    #[test]
    fn non_social_change_is_not_applied_and_logs_once_per_content() {
        let f = Fixture::new("restart");
        let mut cfg = f.config();
        let original = cfg.clone();
        let mut watcher = ConfigWatcher::new(&cfg).unwrap();
        let baseline = std::fs::read_to_string(f.root.join("config/runtime.yaml")).unwrap();
        let mut value: serde_yaml::Value = serde_yaml::from_str(&baseline).unwrap();
        value["session"]["max_turns"] = serde_yaml::Value::from(987);
        let edited = serde_yaml::to_string(&value).unwrap();
        f.write("runtime.yaml", &edited);
        watcher.poll(&mut cfg).unwrap();
        assert_eq!(watcher.poll(&mut cfg).unwrap(), ["config/runtime.yaml changed; restart required"]);
        assert!(Arc::ptr_eq(&cfg, &original));
        assert!(watcher.poll(&mut cfg).unwrap().is_empty());
        // A comment-only change to the original has the running parsed value.
        f.write("runtime.yaml", &format!("{baseline}\n# comment\n"));
        watcher.poll(&mut cfg).unwrap();
        assert!(watcher.poll(&mut cfg).unwrap().is_empty());
        f.write("runtime.yaml", &edited);
        f.social("['telegram:1']", 60);
        watcher.poll(&mut cfg).unwrap();
        let messages = watcher.poll(&mut cfg).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("social.yaml reloaded"));
        assert_eq!(cfg.runtime, original.runtime);
        f.write("runtime.yaml", &format!("{edited}\n# distinct content\n"));
        watcher.poll(&mut cfg).unwrap();
        assert_eq!(watcher.poll(&mut cfg).unwrap(), ["config/runtime.yaml changed; restart required"]);
    }
}
