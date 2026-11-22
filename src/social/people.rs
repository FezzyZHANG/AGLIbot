//! The person registry (`persons` collection): who he has met, how close they are, what he knows.
//! Facts carry the scope of the conversation they came from; `privacy.rs` decides where they may appear.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use chrono::{DateTime, Utc};
use tracing::info;

use super::identity::Who;
use crate::channels::Envelope;
use crate::ns::{Audience, Namespace};
use crate::store::{Fact, Person, PersonKind, Store, Tier};

/// Don't rewrite the registry for every message from the same person.
const SEEN_RESOLUTION_MIN: i64 = 10;

pub struct People {
    cache: BTreeMap<String, Person>,
}

impl People {
    pub async fn load(store: &Store) -> Result<Self> {
        Ok(People { cache: store.persons().await? })
    }

    pub fn get(&self, person_id: &str) -> Option<&Person> {
        self.cache.get(person_id)
    }

    pub fn name_of(&self, person_id: &str) -> String {
        self.cache.get(person_id).map(|p| p.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| person_id.into())
    }

    /// Record that `who` spoke in `env`; creates or refreshes the person.
    pub async fn observe(&mut self, store: &Store, env: &Envelope, who: &Who, now: DateTime<Utc>) -> Result<()> {
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
                    last_update: None,
                    last_update_day: None,
                    facts: Vec::new(),
                    last_seen: now,
                    groups: env.group_key().into_iter().collect(),
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
                if let Some(g) = env.group_key() && !p.groups.contains(&g) {
                    p.groups.push(g);
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
            store.save_person(&self.cache[&who.person_id]).await?;
        }
        Ok(())
    }

    /// The owner always has a record, even before messaging on a new platform.
    pub async fn ensure(&mut self, store: &Store, person_id: &str, name: &str, tier: Tier) -> Result<()> {
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
            last_update: None,
            last_update_day: None,
            facts: Vec::new(),
            last_seen: Utc::now(),
            groups: Vec::new(),
        };
        store.save_person(&p).await?;
        self.cache.insert(person_id.into(), p);
        Ok(())
    }

    #[cfg(test)]
    pub async fn save(&mut self, store: &Store, person: Person) -> Result<()> {
        store.save_person(&person).await?;
        self.cache.insert(person.person_id.clone(), person);
        Ok(())
    }

    /// Membership is never revoked: someone who left may still hear about the group in private.
    pub async fn add_group(&mut self, store: &Store, person_id: &str, group: &str) -> Result<()> {
        let Some(p) = self.cache.get_mut(person_id) else { return Ok(()) };
        if p.groups.iter().any(|g| g == group) {
            return Ok(());
        }
        p.groups.push(group.into());
        store.save_person(p).await
    }

    /// Once per store: group membership from messages stored before `groups` existed.
    pub async fn backfill_groups(&mut self, store: &Store) -> Result<()> {
        const DONE: &str = "persons_groups_backfilled";
        if store.kv_get(DONE).await?.is_some() {
            return Ok(());
        }
        let mut added = 0;
        for ns in store.active_namespaces(DateTime::UNIX_EPOCH).await? {
            let Namespace::Group(g) = &ns else { continue };
            let senders: BTreeSet<String> = store.messages_after(&ns, DateTime::UNIX_EPOCH).await?
                .into_iter().filter(|m| !m.outgoing).map(|m| m.sender).collect();
            for id in senders {
                if self.cache.get(&id).is_some_and(|p| !p.groups.contains(g)) {
                    self.add_group(store, &id, g).await?;
                    added += 1;
                }
            }
        }
        info!("backfilled {added} group membership(s) from stored messages");
        store.kv_set(DONE, serde_json::json!(true)).await
    }

    /// The audience of a private chat with `person_id`.
    pub fn private_audience(&self, person_id: &str) -> Audience {
        Audience::private_member_of(person_id, self.cache.get(person_id).map(|p| p.groups.clone()).unwrap_or_default())
    }

    pub async fn add_fact(&mut self, store: &Store, person_id: &str, fact: Fact) -> Result<()> {
        if !self.cache.contains_key(person_id) {
            self.ensure(store, person_id, "", Tier::Stranger).await?;
        }
        let Some(p) = self.cache.get_mut(person_id) else { return Ok(()) };
        if p.facts.iter().any(|f| f.text == fact.text) {
            return Ok(());
        }
        p.facts.push(fact);
        store.save_person(p).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ns::Scope;
    use crate::store::MessageRecord;

    fn env(chat_id: &str, is_group: bool) -> Envelope {
        Envelope {
            authored_text: String::new(), notice: false, attachments: Vec::new(), arrived: None, platform: "qq", chat_id: chat_id.into(),
            message_id: None, sender_id: "5".into(), sender_name: "阿五".into(), sender_is_bot: false,
            is_group, mentions_me: false, text: "hi".into(),
        }
    }

    #[tokio::test]
    async fn speaking_in_a_group_makes_a_lasting_member() {
        let store = Store::memory();
        let mut people = People::load(&store).await.unwrap();
        let who = Who { person_id: "qq:5".into(), is_owner: false, is_agent: false };
        people.observe(&store, &env("private:5", false), &who, Utc::now()).await.unwrap();
        assert!(people.private_audience("qq:5").member_of.is_empty());
        people.observe(&store, &env("group:9", true), &who, Utc::now()).await.unwrap();
        people.observe(&store, &env("group:9", true), &who, Utc::now()).await.unwrap();
        assert_eq!(people.get("qq:5").unwrap().groups, ["qq:9"]);
        // Persisted, and still a member after a reload.
        let reloaded = People::load(&store).await.unwrap();
        assert_eq!(reloaded.private_audience("qq:5"), Audience::private_member_of("qq:5", ["qq:9".to_string()]));
        assert!(people.private_audience("qq:404").member_of.is_empty());
    }

    #[tokio::test]
    async fn backfill_reads_stored_group_senders_once() {
        let store = Store::memory();
        let mut people = People::load(&store).await.unwrap();
        people.ensure(&store, "owner", "主人", Tier::Owner).await.unwrap();
        let record = |sender: &str, ns: &str, outgoing: bool| MessageRecord {
            ts: Utc::now(), platform: "qq".into(), chat_id: "group:9".into(), sender: sender.into(),
            sender_name: String::new(), outgoing, text: "x".into(), ns: ns.parse().unwrap(),
            scope: Scope::Group { id: "qq:9".into() },
        };
        store.append_message(&record("owner", "group/qq:9", false)).await.unwrap();
        store.append_message(&record("me", "group/qq:8", true)).await.unwrap();
        people.backfill_groups(&store).await.unwrap();
        assert_eq!(people.get("owner").unwrap().groups, ["qq:9"]);
        store.append_message(&record("owner", "group/qq:7", false)).await.unwrap();
        people.backfill_groups(&store).await.unwrap();
        assert_eq!(people.get("owner").unwrap().groups, ["qq:9"], "runs only once");
    }
}
