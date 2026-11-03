//! The owner's live group lexicon (#80): names that address him like an @, and group slang with
//! its meaning. Entries live in the store (MongoDB `lexicon`, editable in Compass) and are
//! re-read at most every [`TTL_SECS`]. Matching is substring, ASCII case-insensitive. An entry
//! with a `scope` (a `platform:chat_id` group key) never applies outside that group.

use std::collections::BTreeSet;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::store::Store;

pub const TTL_SECS: i64 = 30;
/// Slang lines added to one turn, longest term first.
pub const SLANG_MAX: usize = 8;
const MEANING_MAX: usize = 120;
/// Set once the yaml aliases were offered as a seed, so an emptied collection stays empty.
const SEEDED: &str = "lexicon_seeded";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind { Alias, Slang }

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub term: String,
    pub kind: Kind,
    pub meaning: String,
    pub scope: Option<String>,
}

impl Entry {
    fn applies_to(&self, group: &str) -> bool {
        self.scope.as_deref().is_none_or(|s| s == group)
    }
}

/// One stored document, or why it was skipped. Disabled entries are `Ok(None)`.
fn parse(doc: &Value) -> Result<Option<Entry>, String> {
    let term = doc.get("term").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    if term.is_empty() {
        return Err("empty term".into());
    }
    if doc.get("enabled").is_some_and(|v| v.as_bool() == Some(false)) {
        return Ok(None);
    }
    let kind = match doc.get("kind").and_then(Value::as_str) {
        Some("alias") => Kind::Alias,
        Some("slang") => Kind::Slang,
        other => return Err(format!("{term}: unknown kind {other:?}")),
    };
    let meaning = doc.get("meaning").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    if kind == Kind::Slang && meaning.is_empty() {
        return Err(format!("{term}: slang without meaning"));
    }
    let meaning = if kind == Kind::Slang { meaning.chars().take(MEANING_MAX).collect() } else { String::new() };
    let scope = doc.get("scope").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    Ok(Some(Entry { term: term.into(), kind, meaning, scope }))
}

/// Text QQ shows for a reply starts with `[回复 名字: 引文]`; a quote of someone else's line does
/// not call him.
fn without_quote(text: &str) -> &str {
    match text.strip_prefix("[回复") {
        Some(rest) => rest.split_once(']').map_or("", |(_, after)| after),
        None => text,
    }
}

#[derive(Default)]
pub struct Lexicon {
    entries: Vec<Entry>,
    loaded_at: Option<DateTime<Utc>>,
    failing: bool,
    /// Why entries were skipped at the last reload; warned again only when this changes.
    skipped: Vec<String>,
}

impl Lexicon {
    /// Offer the yaml aliases as global aliases once, only into an empty lexicon.
    pub fn seed(store: &dyn Store, aliases: &[String]) -> Result<()> {
        if store.kv_get(SEEDED)?.is_some() {
            return Ok(());
        }
        if store.lexicon()?.is_empty() {
            let docs: Vec<Value> = aliases.iter().map(|a| a.trim()).filter(|a| !a.is_empty()).collect::<BTreeSet<_>>()
                .into_iter().map(|a| json!({"term": a, "kind": "alias", "scope": null, "enabled": true, "note": "seeded from social.yaml"}))
                .collect();
            if !docs.is_empty() {
                store.insert_lexicon(&docs)?;
                info!("lexicon seeded with {} alias(es) from social.yaml", docs.len());
            }
        }
        store.kv_set(SEEDED, json!(true))
    }

    /// Re-read the store when the cache is older than the TTL. A failed read keeps the last good
    /// set and warns once until a read succeeds again.
    pub fn refresh(&mut self, store: &dyn Store, now: DateTime<Utc>) {
        if self.loaded_at.is_some_and(|t| now - t < chrono::Duration::seconds(TTL_SECS)) {
            return;
        }
        self.loaded_at = Some(now);
        let docs = match store.lexicon() {
            Ok(docs) => docs,
            Err(e) => {
                if !self.failing {
                    warn!("lexicon reload failed, keeping {} entries: {e:#}", self.entries.len());
                }
                self.failing = true;
                return;
            }
        };
        self.failing = false;
        let mut skipped = Vec::new();
        let mut entries: Vec<Entry> = docs.iter().filter_map(|d| parse(d).unwrap_or_else(|why| { skipped.push(why); None })).collect();
        entries.sort();
        entries.dedup();
        if skipped != self.skipped && !skipped.is_empty() {
            warn!("lexicon: skipped {} invalid entr(ies): {}", skipped.len(), skipped.join("; "));
        }
        self.skipped = skipped;
        if entries != self.entries {
            let names = |v: &[Entry], k: Kind| v.iter().filter(|e| e.kind == k).map(|e| e.term.clone()).collect::<BTreeSet<_>>();
            let (old_a, new_a) = (names(&self.entries, Kind::Alias), names(&entries, Kind::Alias));
            let (old_s, new_s) = (names(&self.entries, Kind::Slang), names(&entries, Kind::Slang));
            info!(
                "lexicon reloaded: aliases=+{:?}-{:?} slang=+{}-{}",
                new_a.difference(&old_a).collect::<Vec<_>>(), old_a.difference(&new_a).collect::<Vec<_>>(),
                new_s.difference(&old_s).count(), old_s.difference(&new_s).count(),
            );
            self.entries = entries;
        }
    }

    /// The alias a message in `group` calls him by.
    pub fn called_by(&self, group: &str, text: &str) -> Option<&str> {
        let text = without_quote(text).to_lowercase();
        self.entries.iter()
            .filter(|e| e.kind == Kind::Alias && e.applies_to(group))
            .find(|e| text.contains(&e.term.to_lowercase()))
            .map(|e| e.term.as_str())
    }

    /// One `<当前状态>` line explaining the slang that appears in `text` (quotes included), or None.
    pub fn slang_note(&self, group: &str, text: &str) -> Option<String> {
        let text = text.to_lowercase();
        let mut hits: Vec<&Entry> = self.entries.iter()
            .filter(|e| e.kind == Kind::Slang && e.applies_to(group) && text.contains(&e.term.to_lowercase()))
            .collect();
        // A group-scoped meaning overrides a global one for the same term.
        hits.sort_by(|a, b| b.term.chars().count().cmp(&a.term.chars().count())
            .then_with(|| a.term.cmp(&b.term)).then_with(|| b.scope.is_some().cmp(&a.scope.is_some())));
        hits.dedup_by(|b, a| a.term == b.term);
        hits.truncate(SLANG_MAX);
        (!hits.is_empty()).then(|| {
            format!("群里的说法：{}", hits.iter().map(|e| format!("{}＝{}", e.term, e.meaning)).collect::<Vec<_>>().join("；"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::jsonl::JsonlStore;

    struct Dir(std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn store(name: &str) -> (Dir, JsonlStore) {
        let dir = std::env::temp_dir().join(format!("aglibot-lexicon-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = JsonlStore::open(&dir).unwrap();
        (Dir(dir), store)
    }

    fn t0() -> DateTime<Utc> { "2026-11-03T04:00:00Z".parse().unwrap() }

    #[test]
    fn seed_runs_once_and_never_refills_an_emptied_lexicon() {
        let (dir, s) = store("seed");
        Lexicon::seed(&s, &["小庄".into(), " ".into(), "小庄".into()]).unwrap();
        assert_eq!(s.lexicon().unwrap().len(), 1);
        Lexicon::seed(&s, &["别的".into()]).unwrap();
        assert_eq!(s.lexicon().unwrap().len(), 1);
        std::fs::write(dir.0.join("lexicon.json"), "[]").unwrap();
        Lexicon::seed(&s, &["小庄".into()]).unwrap();
        assert!(s.lexicon().unwrap().is_empty());
        let (_d, fresh) = store("seed-existing");
        fresh.insert_lexicon(&[json!({"term": "庄", "kind": "alias"})]).unwrap();
        Lexicon::seed(&fresh, &["小庄".into()]).unwrap();
        assert_eq!(fresh.lexicon().unwrap().len(), 1, "a non-empty lexicon is never seeded");
    }

    #[test]
    fn disabled_and_invalid_entries_are_skipped() {
        let (_d, s) = store("invalid");
        s.insert_lexicon(&[
            json!({"term": "小庄", "kind": "alias"}),
            json!({"term": "庄方宜", "kind": "alias", "enabled": false}),
            json!({"term": "肉编器", "kind": "slang", "meaning": "手写汇编的人"}),
            json!({"term": "空", "kind": "slang"}),
            json!({"term": "怪", "kind": "other"}),
            json!({"term": " ", "kind": "alias"}),
        ]).unwrap();
        let mut lx = Lexicon::default();
        lx.refresh(&s, t0());
        assert_eq!(lx.entries.len(), 2);
        assert_eq!(lx.skipped.len(), 3);
        assert_eq!(lx.called_by("qq:1", "小庄在吗"), Some("小庄"));
        assert_eq!(lx.called_by("qq:1", "庄方宜在吗"), None);
        assert_eq!(lx.slang_note("qq:1", "空 怪 肉编器").unwrap(), "群里的说法：肉编器＝手写汇编的人");
    }

    #[test]
    fn ttl_refresh_picks_up_inserts_and_a_failed_read_keeps_the_last_set() {
        let (dir, s) = store("ttl");
        s.insert_lexicon(&[json!({"term": "小庄", "kind": "alias"})]).unwrap();
        let mut lx = Lexicon::default();
        lx.refresh(&s, t0());
        s.insert_lexicon(&[json!({"term": "zhuang", "kind": "alias"})]).unwrap();
        lx.refresh(&s, t0() + chrono::Duration::seconds(TTL_SECS - 1));
        assert_eq!(lx.called_by("qq:1", "hey Zhuang"), None, "cached inside the TTL");
        lx.refresh(&s, t0() + chrono::Duration::seconds(TTL_SECS));
        assert_eq!(lx.called_by("qq:1", "hey Zhuang"), Some("zhuang"));
        std::fs::write(dir.0.join("lexicon.json"), "{broken").unwrap();
        lx.refresh(&s, t0() + chrono::Duration::seconds(3 * TTL_SECS));
        assert!(lx.failing);
        assert_eq!(lx.called_by("qq:1", "小庄"), Some("小庄"));
        assert_eq!(lx.called_by("qq:1", "zhuang"), Some("zhuang"));
    }

    #[test]
    fn scope_quotes_order_and_cap() {
        let mut lx = Lexicon { entries: vec![
            Entry { term: "小庄".into(), kind: Kind::Alias, meaning: String::new(), scope: Some("qq:1".into()) },
            Entry { term: "姐夫".into(), kind: Kind::Slang, meaning: "全局义".into(), scope: None },
            Entry { term: "姐夫".into(), kind: Kind::Slang, meaning: "本群义".into(), scope: Some("qq:1".into()) },
            Entry { term: "肉编器".into(), kind: Kind::Slang, meaning: "手写汇编的人".into(), scope: Some("qq:2".into()) },
        ], ..Default::default() };
        assert_eq!(lx.called_by("qq:1", "小庄"), Some("小庄"));
        assert_eq!(lx.called_by("qq:2", "小庄"), None);
        assert_eq!(lx.called_by("qq:1", "[回复 某人: 小庄说的]不同意"), None);
        assert_eq!(lx.slang_note("qq:1", "[回复 某人: 姐夫]肉编器").unwrap(), "群里的说法：姐夫＝本群义");
        assert_eq!(lx.slang_note("qq:3", "姐夫").unwrap(), "群里的说法：姐夫＝全局义");
        lx.entries = (0..12).map(|i| Entry { term: "词".repeat(i + 1), kind: Kind::Slang, meaning: i.to_string(), scope: None }).collect();
        let note = lx.slang_note("qq:1", &"词".repeat(12)).unwrap();
        assert_eq!(note.matches('＝').count(), SLANG_MAX);
        assert!(note.starts_with(&format!("群里的说法：{}＝11", "词".repeat(12))));
    }
}
