//! Contextual integrity in code (docs/DESIGN.md 2b): only memories whose scope covers everyone
//! listening may enter a context. This filter runs before any prompt is built, so a secret told
//! in private is never even shown to the model in a group — the model's discretion is not relied on.

use anyhow::Result;
use chrono::{DateTime, Utc};
use tracing::info;

use crate::ns::{Audience, Namespace, ProcessingPolicy, Scope};
use crate::store::{Episode, Fact, Lesson, Person, Store};

#[cfg(test)]
mod semantic_tests;

#[derive(Debug, Default)]
pub struct Recall {
    /// (person name, fact)
    pub facts: Vec<(String, Fact)>,
    pub episodes: Vec<Episode>,
    pub lessons: Vec<Lesson>,
    /// How many candidate items the scope check held back.
    pub withheld: usize,
}

impl Recall {
    /// The strictest processing policy among the included items.
    pub fn policy(&self) -> ProcessingPolicy {
        let scopes = self
            .facts
            .iter()
            .map(|(_, f)| &f.scope)
            .chain(self.episodes.iter().map(|e| &e.scope))
            .chain(self.lessons.iter().map(|l| &l.scope));
        if scopes.into_iter().any(|s| *s == Scope::SelfOnly) {
            ProcessingPolicy::LocalOnly
        } else {
            ProcessingPolicy::Any
        }
    }

    pub fn is_empty(&self) -> bool {
        self.facts.is_empty() && self.episodes.is_empty() && self.lessons.is_empty()
    }
}

pub struct Query<'a> {
    pub audience: &'a Audience,
    /// The conversation's own namespace: its episodes come first.
    pub ns: &'a Namespace,
    /// People in the conversation, whose facts and shared episodes are relevant.
    pub partners: &'a [&'a Person],
    pub max_episodes: usize,
    pub max_facts: usize,
    /// Self-scoped lessons are only for solo thinking, and only when a local model can process them.
    pub include_lessons: bool,
    pub half_life_days: f64,
}

pub fn recall(store: &dyn Store, q: &Query<'_>) -> Result<Recall> {
    recall_at(store, q, Utc::now())
}

/// Brute force over permitted, active episodes. Source text never goes to the embedder here.
/// TODO: replace the scan with an index when volume warrants it; retain this scope gate.
pub fn recall_semantic(
    store: &dyn Store, q: &Query<'_>, cfg: &crate::mind::embedding::EmbeddingCfg, vector: &[f32],
) -> Result<Recall> {
    recall_semantic_at(store, q, cfg, vector, Utc::now())
}

fn recall_semantic_at(
    store: &dyn Store, q: &Query<'_>, cfg: &crate::mind::embedding::EmbeddingCfg,
    vector: &[f32], now: DateTime<Utc>,
) -> Result<Recall> {
    let mut out = recall_at(store, q, now)?;
    let own = out.episodes.iter().filter(|e| &e.ns == q.ns).count();
    let available = q.max_episodes.saturating_sub(own).min(cfg.max_cross_namespace);
    if available == 0 { return Ok(out) }
    let embeddings = store.load_embeddings(&cfg.model_id())?;
    let mut hits = Vec::new();
    for episode in store.episodes()? {
        // Before looking up or comparing vectors, including malformed cached vectors.
        if !episode.scope.permits(q.audience) || &episode.ns == q.ns { continue }
        let Some(cached) = embeddings.get(&episode.id) else { continue };
        let similarity = crate::mind::embedding::cosine(vector, cached)?;
        if similarity >= cfg.threshold {
            let score = similarity * episode.effective_salience(now, q.half_life_days);
            hits.push((score, episode));
        }
    }
    hits.sort_by(|(a, ae), (b, be)| b.total_cmp(a).then(be.ts.cmp(&ae.ts)).then(ae.id.cmp(&be.id)));
    hits.truncate(available);
    tracing::debug!(hits = hits.len(), "semantic recall hits added");
    if hits.is_empty() { return Ok(out) }
    // Keep all selected own-namespace memories, then semantic hits in score order. Retain
    // ordinary participant matches in any leftover slots, without duplicates.
    let (mut selected, others): (Vec<_>, Vec<_>) = out.episodes.into_iter().partition(|e| &e.ns == q.ns);
    selected.extend(hits.into_iter().map(|(_, episode)| episode));
    for episode in others {
        if selected.len() == q.max_episodes { break }
        if !selected.iter().any(|e| e.id == episode.id) { selected.push(episode); }
    }
    out.episodes = selected;
    Ok(out)
}

fn recall_at(store: &dyn Store, q: &Query<'_>, now: DateTime<Utc>) -> Result<Recall> {
    let mut out = Recall::default();

    for p in q.partners {
        let (ok, held): (Vec<&Fact>, Vec<&Fact>) = p.facts.iter().partition(|f| f.scope.permits(q.audience));
        out.withheld += held.len();
        let skip = ok.len().saturating_sub(q.max_facts);
        out.facts.extend(ok.into_iter().skip(skip).map(|f| (p.name.clone(), f.clone())));
    }

    let partner_ids: Vec<&str> = q.partners.iter().map(|p| p.person_id.as_str()).collect();
    let mut relevant: Vec<Episode> = Vec::new();
    for e in store.episodes()? {
        let related = &e.ns == q.ns || e.participants.iter().any(|p| partner_ids.contains(&p.as_str()));
        if !related {
            continue;
        }
        if e.scope.permits(q.audience) {
            relevant.push(e);
        } else {
            out.withheld += 1;
        }
    }
    // Own namespace first, then by decayed salience and recency.
    relevant.sort_by(|a, b| {
        (&b.ns == q.ns)
            .cmp(&(&a.ns == q.ns))
            .then(b.effective_salience(now, q.half_life_days).total_cmp(&a.effective_salience(now, q.half_life_days)))
            .then(b.ts.cmp(&a.ts))
    });
    relevant.truncate(q.max_episodes);
    relevant.sort_by_key(|e| e.ts);
    out.episodes = relevant;

    if q.include_lessons && q.audience.is_solo() {
        out.lessons = store.lessons()?.into_iter().filter(|l| l.scope.permits(q.audience)).collect();
    }
    if out.withheld > 0 {
        info!("privacy: withheld {} memory item(s) from {} (audience {:?})", out.withheld, q.ns, q.audience);
    }
    Ok(out)
}

/// Render recalled memories for the L2 frame.
pub fn render(r: &Recall) -> String {
    let mut out = String::new();
    if !r.facts.is_empty() {
        out.push_str("\n\n## 你记得的关于对方的事\n");
        for (name, f) in &r.facts {
            out.push_str(&format!("\n- {name}：{}", f.text));
        }
    }
    if !r.episodes.is_empty() {
        out.push_str("\n\n## 相关的经历\n");
        for e in &r.episodes {
            out.push_str(&format!("\n- [{}] {}", e.ts.with_timezone(&chrono::Local).format("%m-%d"), e.summary));
        }
    }
    if !r.lessons.is_empty() {
        out.push_str("\n\n## 你自己总结过的经验（只属于你自己）\n");
        for l in &r.lessons {
            out.push_str(&format!("\n- {}", l.text));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::jsonl::JsonlStore;
    use crate::store::{PersonKind, Tier};
    use chrono::Utc;

    fn episode(id: &str, ns: &str, scope: Scope, who: &str, text: &str) -> Episode {
        Episode {
            id: id.into(),
            ts: Utc::now(),
            participants: vec![who.into()],
            summary: text.into(),
            emotion: String::new(),
            salience: 0.5,
            ns: ns.parse().unwrap(),
            scope,
            sources: vec![],
        }
    }

    #[test]
    fn recall_ranks_by_decayed_salience_with_configured_half_life() {
        let dir = std::env::temp_dir().join(format!("aglibot-test-decay-recall-{}", std::process::id()));
        let store = JsonlStore::open(&dir).unwrap();
        let now: DateTime<Utc> = "2026-11-01T00:00:00Z".parse().unwrap();
        let mut old = episode("old", "person/owner", Scope::Public, "owner", "old");
        old.ts = now - chrono::Duration::days(60);
        old.salience = 0.8;
        let mut recent = Episode { id: "recent".into(), ts: now, salience: 0.3, ..old.clone() };
        store.append_episode(&old).unwrap();
        store.append_episode(&recent).unwrap();
        let ns = old.ns.clone();
        let audience = Audience::private("owner");
        let mut q = Query { audience: &audience, ns: &ns, partners: &[], max_episodes: 1,
            max_facts: 0, include_lessons: false, half_life_days: 30.0 };
        assert_eq!(recall_at(&store, &q, now).unwrap().episodes[0].id, "recent");
        q.half_life_days = 300.0;
        assert_eq!(recall_at(&store, &q, now).unwrap().episodes[0].id, "old");
        // An archived original no longer competes with its replacement in recall.
        recent.id = "summary".into();
        recent.sources = vec![old.id.clone(), "recent".into()];
        store.archive_episode(&recent).unwrap();
        assert_eq!(recall_at(&store, &q, now).unwrap().episodes[0].id, "summary");
        assert_eq!(store.all_episodes().unwrap()[0].salience, 0.8);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// DESIGN 9: a secret told in private must not reach a group context, filtered at retrieval.
    #[test]
    fn private_secret_is_withheld_from_groups() {
        let dir = std::env::temp_dir().join(format!("aglibot-test-privacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = JsonlStore::open(&dir).unwrap();
        let private = Scope::Persons { ids: vec!["owner".into()] };
        store.append_episode(&episode("e1", "person/owner", private.clone(), "owner", "主人说他下个月要辞职")).unwrap();
        store.append_episode(&episode("e2", "group/qq:1", Scope::Group { id: "qq:1".into() }, "owner", "群里在聊 Rust 异步")).unwrap();
        let owner = Person {
            person_id: "owner".into(),
            platform_ids: vec![],
            name: "主人".into(),
            kind: PersonKind::Human,
            tier: Tier::Owner,
            trust: 1.0,
            intimacy: 1.0,
            last_update: None,
            last_update_day: None,
            facts: vec![Fact { text: "在考虑换工作".into(), ns: "person/owner".parse().unwrap(), scope: private, ts: Utc::now() }],
            last_seen: Utc::now(),
        };
        let partners = [&owner];

        let group_ns: Namespace = "group/qq:1".parse().unwrap();
        let group = Audience::group("qq:1", ["owner".to_string(), "qq:5".to_string()]);
        let q = Query { audience: &group, ns: &group_ns, partners: &partners, max_episodes: 10, max_facts: 10, include_lessons: false, half_life_days: 30.0 };
        let r = recall(&store, &q).unwrap();
        let text = render(&r);
        assert!(!text.contains("辞职") && !text.contains("换工作"), "{text}");
        assert!(text.contains("Rust"));
        assert_eq!(r.withheld, 2);

        let owner_ns: Namespace = "person/owner".parse().unwrap();
        let dm = Audience::private("owner");
        let q = Query { audience: &dm, ns: &owner_ns, partners: &partners, max_episodes: 10, max_facts: 10, include_lessons: false, half_life_days: 30.0 };
        let text = render(&recall(&store, &q).unwrap());
        assert!(text.contains("辞职") && text.contains("换工作"));
        // The group episode involved the owner but is scoped to the group: not in the DM either.
        assert!(!text.contains("Rust"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
