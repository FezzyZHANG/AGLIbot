//! Who is talking. Authority comes only from the platform's numeric id, never a display name.

use crate::channels::Envelope;
use crate::config::AclCfg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sender {
    Owner,
    Other,
}

pub fn classify(acl: &AclCfg, env: &Envelope) -> Sender {
    let owner_id = acl.owner.telegram_user_id;
    let id_matches = owner_id != 0 && env.sender_id == owner_id.to_string();
    if env.platform == "telegram" && !env.sender_is_bot && id_matches {
        Sender::Owner
    } else {
        Sender::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Owner;

    fn env(sender_id: &str, name: &str) -> Envelope {
        Envelope {
            platform: "telegram",
            chat_id: sender_id.into(),
            sender_id: sender_id.into(),
            sender_name: name.into(),
            sender_is_bot: false,
            is_group: false,
            text: "hi".into(),
        }
    }

    #[test]
    fn owner_is_matched_by_id_not_name() {
        let acl = AclCfg { owner: Owner { person_id: "owner".into(), telegram_user_id: 42 } };
        assert_eq!(classify(&acl, &env("42", "whoever")), Sender::Owner);
        assert_eq!(classify(&acl, &env("7", "owner")), Sender::Other);
    }

    #[test]
    fn unset_owner_id_matches_nobody() {
        let acl = AclCfg { owner: Owner { person_id: "owner".into(), telegram_user_id: 0 } };
        assert_eq!(classify(&acl, &env("0", "x")), Sender::Other);
    }
}
