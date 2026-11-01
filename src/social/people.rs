//! The person registry (`persons` collection): who he has met, how close they are, what he knows.
//! Facts carry the scope of the conversation they came from; `privacy.rs` decides where they may appear.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Utc};

use super::identity::Who;
use crate::channels::Envelope;
use crate::store::{Fact, Person, PersonKind, Store, Tier};

/// Don't rewrite the registry for every message from the same person.
const SEEN_RESOLUTION_MIN: i64 = 10;

pub struct People {
    cache: BTreeMap<String, Person>,
}

impl People {
    pub fn load(store: &dyn Store) -> Result<Self> {
        Ok(People { cache: store.persons()? })
    }

    pub fn get(&self, person_id: &str) -> Option<&Person> {
        self.cache.get(person_id)
    }

    pub fn name_of(&self, person_id: &str) -> String {
        self.cache.get(person_id).map(|p| p.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| person_id.into())
    }

    /// Record that `who` spoke in `env`; creates or refreshes the person.
    pub fn observe(&mut self, store: &dyn Store, env: &Envelope, who: &Who, now: DateTime<Utc>) -> Result<()> {
        let tier = if who.is_owner {
            Tier::Owner
        } else if env.is_group {
            Tier::Groupmate
        } else {
            Tier::Acquaintance
        };
        let key = env.sender_key();
        let kind = if who.is_agent { PersonKind::Agent } else { PersonKind::Human };
        let changed = match self.cache.get_mut(&who.person_id) {
            None => {
                let p = Person {
                    person_id: who.person_id.clone(),
                    platform_ids: vec![key],
                    name: env.sender_name.clone(),
                    kind,
                    tier,
                    trust: 0.0,
                    intimacy: 0.0,
                    facts: Vec::new(),
                    last_seen: now,
                };
                self.cache.insert(who.person_id.clone(), p);
                true
            }
            Some(p) => {
                let mut changed = (now - p.last_seen).num_minutes() >= SEEN_RESOLUTION_MIN;
                if !p.platform_ids.contains(&key) {
                    p.platform_ids.push(key);
                    changed = true;
                }
                if tier < p.tier {
                    p.tier = tier; // closer tiers win: a groupmate who DMs becomes an acquaintance
                    changed = true;
                }
                if !env.sender_name.is_empty() && p.name != env.sender_name {
                    p.name = env.sender_name.clone();
                    changed = true;
                }
                if who.is_agent && p.kind != PersonKind::Agent {
                    p.kind = PersonKind::Agent;
                    changed = true;
                }
                p.last_seen = now;
                changed
            }
        };
        if changed {
            store.save_person(&self.cache[&who.person_id])?;
        }
        Ok(())
    }

    /// The owner always has a record, even before messaging on a new platform.
    pub fn ensure(&mut self, store: &dyn Store, person_id: &str, name: &str, tier: Tier) -> Result<()> {
        if self.cache.contains_key(person_id) {
            return Ok(());
        }
        let p = Person {
            person_id: person_id.into(),
            platform_ids: Vec::new(),
            name: name.into(),
            kind: PersonKind::Human,
            tier,
            trust: 0.0,
            intimacy: 0.0,
            facts: Vec::new(),
            last_seen: Utc::now(),
        };
        store.save_person(&p)?;
        self.cache.insert(person_id.into(), p);
        Ok(())
    }

    pub fn add_fact(&mut self, store: &dyn Store, person_id: &str, fact: Fact) -> Result<()> {
        if !self.cache.contains_key(person_id) {
            self.ensure(store, person_id, "", Tier::Stranger)?;
        }
        let Some(p) = self.cache.get_mut(person_id) else { return Ok(()) };
        if p.facts.iter().any(|f| f.text == fact.text) {
            return Ok(());
        }
        p.facts.push(fact);
        store.save_person(p)
    }
}
