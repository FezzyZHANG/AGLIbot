//! Who is talking. Authority comes only from the platform's numeric id, never a display name.

use crate::channels::Envelope;
use crate::config::AclCfg;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Who {
    /// `owner` for the owner on any platform; otherwise `platform:native_id`.
    pub person_id: String,
    pub is_owner: bool,
    pub is_agent: bool,
}

pub fn identify(acl: &AclCfg, env: &Envelope) -> Who {
    let owner_id = match env.platform {
        "telegram" => acl.owner.telegram_user_id,
        "qq" => acl.owner.qq_user_id,
        _ => 0,
    };
    let key = env.sender_key();
    let is_agent = env.sender_is_bot || acl.known_agents.iter().any(|a| a == &key);
    let is_owner = owner_id != 0 && !is_agent && env.sender_id == owner_id.to_string();
    let person_id = if is_owner { acl.owner.person_id.clone() } else { key };
    Who { person_id, is_owner, is_agent }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Owner;

    fn env(platform: &'static str, sender_id: &str, name: &str) -> Envelope {
        Envelope {
            arrived: None,
            platform,
            chat_id: sender_id.into(),
            message_id: None,
            sender_id: sender_id.into(),
            sender_name: name.into(),
            sender_is_bot: false,
            is_group: false,
            mentions_me: true,
            text: "hi".into(),
        }
    }

    fn acl(tg: i64, qq: i64) -> AclCfg {
        AclCfg {
            owner: Owner {
                person_id: "owner".into(),
                telegram_user_id_env: String::new(),
                qq_user_id_env: None,
                telegram_user_id: tg,
                qq_user_id: qq,
            },
            known_agents: vec!["qq:777".into()],
        }
    }

    #[test]
    fn owner_is_matched_by_id_not_name() {
        let acl = acl(42, 1000);
        assert!(identify(&acl, &env("telegram", "42", "whoever")).is_owner);
        assert!(identify(&acl, &env("qq", "1000", "x")).is_owner);
        let other = identify(&acl, &env("telegram", "7", "owner"));
        assert!(!other.is_owner);
        assert_eq!(other.person_id, "telegram:7");
        // The same number on another platform is a different person.
        assert!(!identify(&acl, &env("qq", "42", "x")).is_owner);
    }

    #[test]
    fn unset_owner_id_matches_nobody_and_agents_are_flagged() {
        let acl = acl(0, 0);
        assert!(!identify(&acl, &env("telegram", "0", "x")).is_owner);
        assert!(identify(&acl, &env("qq", "777", "bot")).is_agent);
        let mut bot = env("telegram", "5", "b");
        bot.sender_is_bot = true;
        assert!(identify(&acl, &bot).is_agent);
    }
}
