//! Deterministic owner-private person cards; never export into the tracked mind tree.
use std::{collections::BTreeSet, path::Path};
use anyhow::Result;
use sha2::{Digest, Sha256};
use crate::{ns::Audience, store::{PersonKind, Store, Tier}};
use super::bot_flags;

// Platform ids contain ':'; percent-encode unsafe bytes for portable, reversible names.
fn filename(id: &str) -> String {
    let mut out = String::new();
    for b in id.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' { out.push(char::from(b)); }
        else { out.push_str(&format!("%{b:02X}")); }
    }
    // Avoid Windows device names, including plain ids such as CON.
    let stem = out.to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit()) {
        out.replace_range(..1, &format!("%{:02X}", id.as_bytes()[0]));
    }
    format!("{out}.md")
}

pub fn nightly(store: &dyn Store, data: &Path, owner: &str, known_agents: &[String]) -> Result<()> {
    let dir = data.join("people");
    std::fs::create_dir_all(&dir)?;
    let audience = Audience::private(owner);
    let agents: BTreeSet<_> = known_agents.iter().cloned().chain(bot_flags::list(store, bot_flags::CONFIRMED)?).collect();
    let mut episodes = store.episodes()?;
    episodes.sort_by(|a, b| b.ts.cmp(&a.ts).then(a.id.cmp(&b.id)));
    let mut keep = BTreeSet::new();
    for p in store.persons()?.values() {
        // Acquaintance is the nearest non-owner tier in the current registry.
        if p.person_id == owner || p.tier == Tier::Owner || p.tier > Tier::Acquaintance
            || p.kind == PersonKind::Agent || agents.contains(&p.person_id)
            || p.platform_ids.iter().any(|id| agents.contains(id)) { continue; }
        anyhow::ensure!(!p.person_id.is_empty(), "empty person id");
        let name = filename(&p.person_id);
        let mut text = format!("# {}\n\n- ID: {}\n- Tier: {:?}\n- Trust: {}\n- Intimacy: {}\n- Last seen: {}\n\n## Facts\n",
            p.name, p.person_id, p.tier, p.trust, p.intimacy, p.last_seen.to_rfc3339());
        let mut facts: Vec<_> = p.facts.iter().filter(|f| f.scope.permits(&audience)).collect();
        facts.sort_by(|a, b| b.ts.cmp(&a.ts).then(a.text.cmp(&b.text)));
        for f in facts.into_iter().take(20) { text.push_str(&format!("\n- {}", f.text)); }
        text.push_str("\n\n## Recent episodes\n");
        for e in episodes.iter().filter(|e| e.participants.contains(&p.person_id) && e.scope.permits(&audience)).take(10) {
            text.push_str(&format!("\n- [{}] {}", e.ts.to_rfc3339(), e.summary));
        }
        text.push('\n');
        let path = dir.join(&name);
        let old = match std::fs::read(&path) {
            Ok(bytes) => Some(Sha256::digest(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if old != Some(Sha256::digest(text.as_bytes())) { std::fs::write(path, text)?; }
        keep.insert(name);
    }
    // This generated directory is owned by the exporter, including cards of removed people.
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|e| e == "md")
            && !keep.contains(&entry.file_name().to_string_lossy().into_owned()) {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
