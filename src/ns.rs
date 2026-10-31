//! Namespace, scope and processing policy — three separate concepts (docs/DESIGN.md 2b).
//!
//! - `Namespace`: which context a piece of information belongs to (organisation + retrieval).
//! - `Scope`: who it may be disclosed to.
//! - `ProcessingPolicy`: which models may process it.
//!
//! A namespace is never an access grant; only `Scope::permits` decides what enters a context.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::config::ModelEntry;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Namespace {
    SelfNs,
    Person(String),
    Group(String),
    Project(String),
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Namespace::SelfNs => f.write_str("self"),
            Namespace::Person(id) => write!(f, "person/{id}"),
            Namespace::Group(id) => write!(f, "group/{id}"),
            Namespace::Project(id) => write!(f, "project/{id}"),
        }
    }
}

impl FromStr for Namespace {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        if s == "self" {
            return Ok(Namespace::SelfNs);
        }
        let (kind, id) = s.split_once('/').ok_or_else(|| anyhow!("bad namespace `{s}`"))?;
        if id.is_empty() || id.contains('/') {
            bail!("bad namespace id in `{s}`");
        }
        let id = id.to_string();
        match kind {
            "person" => Ok(Namespace::Person(id)),
            "group" => Ok(Namespace::Group(id)),
            "project" => Ok(Namespace::Project(id)),
            _ => bail!("unknown namespace kind `{kind}`"),
        }
    }
}

impl Serialize for Namespace {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Namespace {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Who is (potentially) listening. An empty audience means he is thinking alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Audience {
    pub members: BTreeSet<String>,
    pub group: Option<String>,
}

impl Audience {
    #[allow(dead_code)] // used by reflect / solo work (phase 2)
    pub fn solo() -> Self {
        Self::default()
    }

    pub fn private(person_id: &str) -> Self {
        Audience { members: BTreeSet::from([person_id.to_string()]), group: None }
    }

    #[allow(dead_code)] // used once group chats land (phase 4)
    pub fn group(group_id: &str, members: impl IntoIterator<Item = String>) -> Self {
        Audience { members: members.into_iter().collect(), group: Some(group_id.to_string()) }
    }

    pub fn is_solo(&self) -> bool {
        self.members.is_empty() && self.group.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    /// Only for his own solo thinking and projects.
    #[serde(rename = "self")]
    SelfOnly,
    /// May be disclosed in private to these people only.
    Persons { ids: Vec<String> },
    /// May be disclosed inside this group chat.
    Group { id: String },
    Public,
}

impl Scope {
    /// Whether a memory with this scope may enter a context heard by `aud`.
    /// Solo thinking may use everything he knows; anything else must be covered by the scope.
    pub fn permits(&self, aud: &Audience) -> bool {
        if aud.is_solo() {
            return true;
        }
        match self {
            Scope::SelfOnly => false,
            // We never know a group's full membership, so person-scoped memories stay out of groups.
            Scope::Persons { ids } => aud.group.is_none() && aud.members.iter().all(|m| ids.contains(m)),
            Scope::Group { id } => aud.group.as_deref() == Some(id.as_str()),
            Scope::Public => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcessingPolicy {
    LocalOnly,
    Any,
    Providers { names: Vec<String> },
}

impl ProcessingPolicy {
    /// Default derived from scope, so most namespaces need no manual policy.
    pub fn default_for(scope: &Scope) -> Self {
        match scope {
            Scope::SelfOnly => ProcessingPolicy::LocalOnly,
            _ => ProcessingPolicy::Any,
        }
    }

    pub fn allows(&self, model: &ModelEntry) -> bool {
        match self {
            ProcessingPolicy::LocalOnly => model.local,
            ProcessingPolicy::Any => true,
            ProcessingPolicy::Providers { names } => names.contains(&model.provider),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_round_trips() {
        for s in ["self", "person/owner", "group/123", "project/pvsnp"] {
            let ns: Namespace = s.parse().unwrap();
            assert_eq!(ns.to_string(), s);
        }
        assert!("person/".parse::<Namespace>().is_err());
        assert!("person/a/b".parse::<Namespace>().is_err());
        assert!("planet/x".parse::<Namespace>().is_err());
    }

    #[test]
    fn private_secret_stays_out_of_group() {
        let secret = Scope::Persons { ids: vec!["owner".into()] };
        assert!(secret.permits(&Audience::private("owner")));
        assert!(!secret.permits(&Audience::private("alice")));
        assert!(!secret.permits(&Audience::group("g1", ["owner".to_string()])));
        assert!(secret.permits(&Audience::solo()));
    }

    #[test]
    fn group_and_self_scopes() {
        let g = Scope::Group { id: "g1".into() };
        assert!(g.permits(&Audience::group("g1", ["alice".to_string()])));
        assert!(!g.permits(&Audience::group("g2", ["alice".to_string()])));
        assert!(!g.permits(&Audience::private("alice")));
        assert!(!Scope::SelfOnly.permits(&Audience::private("owner")));
        assert!(Scope::Public.permits(&Audience::group("g2", [])));
    }

    #[test]
    fn scope_serializes_with_kind_tag() {
        let json = serde_json::to_string(&Scope::SelfOnly).unwrap();
        assert_eq!(json, r#"{"kind":"self"}"#);
        let back: Scope = serde_json::from_str(r#"{"kind":"persons","ids":["owner"]}"#).unwrap();
        assert_eq!(back, Scope::Persons { ids: vec!["owner".into()] });
    }
}
