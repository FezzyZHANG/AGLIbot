//! Turn-taking (conversation analysis): when to speak in a group, and reply caps that stop two
//! bots from talking forever. Most of the time the right move in a group is to stay quiet.

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Duration, Utc};

use crate::config::SocialCfg;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FloorDecision {
    /// Addressed directly: answer.
    Speak,
    /// Enough has happened that it is worth asking the triage model whether to chime in.
    Consider,
    Silent(&'static str),
}

#[derive(Default)]
struct GroupFloor {
    unanswered: usize,
    last_unsolicited: Option<DateTime<Utc>>,
}

#[derive(Default)]
pub struct Floor {
    groups: HashMap<String, GroupFloor>,
    /// (chat key, sender key) → timestamps of his replies to that sender.
    replies: HashMap<(String, String), VecDeque<DateTime<Utc>>>,
}

impl Floor {
    /// A group message arrived. `busy` means he is focused on work (flow): no unsolicited chiming in.
    pub fn on_group_message(&mut self, cfg: &SocialCfg, chat: &str, mentioned: bool, busy: bool, now: DateTime<Utc>) -> FloorDecision {
        let g = self.groups.entry(chat.to_string()).or_default();
        g.unanswered += 1;
        if mentioned {
            return FloorDecision::Speak;
        }
        if busy {
            return FloorDecision::Silent("busy");
        }
        if g.unanswered < cfg.floor.check_every {
            return FloorDecision::Silent("lurking");
        }
        let cooldown = Duration::minutes(cfg.floor.unsolicited_cooldown_min);
        if g.last_unsolicited.is_some_and(|t| now - t < cooldown) {
            g.unanswered = 0; // re-arm after the next window instead of asking on every message
            return FloorDecision::Silent("cooldown");
        }
        FloorDecision::Consider
    }

    /// The triage model was asked; reset the counter whatever it said.
    pub fn considered(&mut self, chat: &str) {
        if let Some(g) = self.groups.get_mut(chat) {
            g.unanswered = 0;
        }
    }

    /// He spoke in a group; `unsolicited` when nobody addressed him.
    pub fn spoke(&mut self, chat: &str, unsolicited: bool, now: DateTime<Utc>) {
        let g = self.groups.entry(chat.to_string()).or_default();
        g.unanswered = 0;
        if unsolicited {
            g.last_unsolicited = Some(now);
        }
    }

    /// Reply cap per (chat, sender). Returns false when he should let it go; records the reply otherwise.
    pub fn allow_reply(&mut self, cfg: &SocialCfg, chat: &str, sender: &str, is_agent: bool, now: DateTime<Utc>) -> bool {
        let limit = if is_agent { &cfg.agent_limit } else { &cfg.human_limit };
        let window = Duration::minutes(limit.window_min);
        let q = self.replies.entry((chat.to_string(), sender.to_string())).or_default();
        while q.front().is_some_and(|t| now - *t > window) {
            q.pop_front();
        }
        if q.len() >= limit.max_replies {
            return false;
        }
        q.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FloorCfg, TurnLimit};

    fn cfg() -> SocialCfg {
        SocialCfg {
            groups: vec![],
            ignored_groups: vec![],
            aliases: vec![],
            read_only_reflect: Default::default(),
            private_from_others: true,
            group_reply_max: 150,
            interjection_max: 80,
            floor: FloorCfg { check_every: 3, unsolicited_cooldown_min: 30 },
            human_limit: TurnLimit { max_replies: 5, window_min: 10 },
            agent_limit: TurnLimit { max_replies: 2, window_min: 10 },
        }
    }

    #[test]
    fn lurks_until_enough_happens_then_respects_cooldown() {
        let (cfg, mut f, t0) = (cfg(), Floor::default(), Utc::now());
        assert_eq!(f.on_group_message(&cfg, "g", false, false, t0), FloorDecision::Silent("lurking"));
        assert_eq!(f.on_group_message(&cfg, "g", false, false, t0), FloorDecision::Silent("lurking"));
        assert_eq!(f.on_group_message(&cfg, "g", false, false, t0), FloorDecision::Consider);
        f.spoke("g", true, t0);
        for _ in 0..2 {
            f.on_group_message(&cfg, "g", false, false, t0);
        }
        assert_eq!(f.on_group_message(&cfg, "g", false, false, t0), FloorDecision::Silent("cooldown"));
        assert_eq!(f.on_group_message(&cfg, "g", true, false, t0), FloorDecision::Speak);
        assert_eq!(f.on_group_message(&cfg, "g", false, true, t0), FloorDecision::Silent("busy"));
    }

    #[test]
    fn agents_hit_the_reply_cap_first() {
        let (cfg, mut f, t0) = (cfg(), Floor::default(), Utc::now());
        assert!(f.allow_reply(&cfg, "g", "bot", true, t0));
        assert!(f.allow_reply(&cfg, "g", "bot", true, t0));
        assert!(!f.allow_reply(&cfg, "g", "bot", true, t0));
        assert!(f.allow_reply(&cfg, "g", "human", false, t0));
        // The window slides.
        assert!(f.allow_reply(&cfg, "g", "bot", true, t0 + Duration::minutes(11)));
    }
}
