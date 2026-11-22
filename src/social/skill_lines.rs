//! `/技能:<名>` replies with that skill's in-game voice line, without going through the model.
//! Lines are from the archived 庄方宜 voice records (workspace/references/zhuang-fangyi).

use std::{io::ErrorKind, path::Path};

use serde::Deserialize;
use tracing::warn;

const DEFAULT_SKILLS: &[(&[&str], &[&str])] = &[
    (&["战技"], &["落青霆！", "释剑去！", "风雷动！"]),
    (&["连携技", "连携"], &["还不服软？", "何苦胡搅蛮缠！"]),
    (&["终结技", "大招"], &["惊霆无声，流电掣雨。", "雷雨并作，化育万物。", "青霄碧落，乌云尽扫。"]),
];

/// The skill name if the whole message, apart from `@…` mentions, is `/技能:<名>`
/// (ASCII or full-width colon).
pub fn requested(text: &str) -> Option<String> {
    let text = text.split_whitespace().filter(|w| !w.starts_with('@')).collect::<Vec<_>>().join(" ");
    let rest = text.strip_prefix("/技能")?.trim_start();
    let name = rest.strip_prefix(':').or_else(|| rest.strip_prefix('：'))?.trim();
    // People copy the `<名>` placeholder literally.
    let name = name.strip_prefix('<').and_then(|n| n.strip_suffix('>')).unwrap_or(name).trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[derive(Deserialize)]
struct Skill {
    names: Vec<String>,
    lines: Vec<String>,
}

#[derive(Deserialize)]
pub struct SkillLines {
    skills: Vec<Skill>,
}

impl SkillLines {
    pub fn load(mind_dir: &Path) -> Self {
        let path = mind_dir.join("self/canned_replies.yaml");
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_yaml::from_str::<Self>(&text) {
                Ok(config) if config.skills.iter().all(|skill| {
                    [&skill.names, &skill.lines].into_iter().all(|values| {
                        !values.is_empty() && values.iter().all(|value| !value.trim().is_empty())
                    })
                }) => return config,
                Ok(_) => warn!("{}: each skill needs at least one nonblank name and line", path.display()),
                Err(err) => warn!("reading {}: {err}", path.display()),
            },
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => warn!("reading {}: {err}", path.display()),
        }
        Self {
            skills: DEFAULT_SKILLS.iter().map(|(names, lines)| Skill {
                names: names.iter().map(|name| (*name).into()).collect(),
                lines: lines.iter().map(|line| (*line).into()).collect(),
            }).collect(),
        }
    }

    /// `pick` chooses among a skill's configured variants.
    pub fn reply(&self, name: &str, pick: usize) -> String {
        if let Some(skill) = self.skills.iter().find(|skill| skill.names.iter().any(|alias| alias == name)) {
            return skill.lines[pick % skill.lines.len()].clone();
        }
        let mut reply = format!("没有「{name}」这个技能。");
        if !self.skills.is_empty() {
            let known: Vec<&str> = self.skills.iter().map(|skill| skill.names[0].as_str()).collect();
            reply.push_str(&format!("可用：{}", known.join("、")));
        }
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_whole_skill_requests() {
        assert_eq!(requested("/技能:战技").as_deref(), Some("战技"));
        assert_eq!(requested("  /技能： 终结技 ").as_deref(), Some("终结技"));
        assert_eq!(requested("@zhuang_bot /技能:连携 @zhuang_bot").as_deref(), Some("连携"));
        assert_eq!(requested("/技能:<连携技>").as_deref(), Some("连携技"));
        assert_eq!(requested("/技能:"), None);
        assert_eq!(requested("/技能战技"), None);
        assert_eq!(requested("看 /技能:战技"), None);
    }

    #[test]
    fn loads_custom_skills_and_falls_back_for_missing_or_invalid_files() {
        let dir = std::env::temp_dir().join(format!("aglibot-skill-lines-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("self")).unwrap();
        let path = dir.join("self/canned_replies.yaml");
        let defaults = SkillLines::load(&dir);
        assert_eq!(defaults.reply("战技", 4), "释剑去！");
        assert_eq!(defaults.reply("大招", 0), "惊霆无声，流电掣雨。");
        assert_eq!(defaults.reply("飞天", 0), "没有「飞天」这个技能。可用：战技、连携技、终结技");

        std::fs::write(&path, "skills:\n  - names: [飞天, 飞]\n    lines: [第一句, 第二句]\n").unwrap();
        let configured = SkillLines::load(&dir);
        assert_eq!(configured.reply("飞", 3), "第二句");
        assert_eq!(configured.reply("战技", 0), "没有「战技」这个技能。可用：飞天");

        std::fs::write(&path, "skills: [").unwrap();
        assert_eq!(SkillLines::load(&dir).reply("战技", 4), "释剑去！");
        std::fs::write(&path, "skills: [{names: [飞天], lines: []}]").unwrap();
        assert_eq!(SkillLines::load(&dir).reply("战技", 4), "释剑去！");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
