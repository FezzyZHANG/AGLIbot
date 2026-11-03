//! `/技能:<名>` replies with that skill's in-game voice line, without going through the model.
//! Lines are from the archived 庄方宜 voice records (workspace/references/zhuang-fangyi).

const SKILLS: &[(&[&str], &[&str])] = &[
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
    (!name.is_empty()).then(|| name.to_string())
}

/// The reply for `name`; `pick` chooses among a skill's variants.
pub fn reply(name: &str, pick: usize) -> String {
    match SKILLS.iter().find(|(names, _)| names.contains(&name)) {
        Some((_, lines)) => lines[pick % lines.len()].to_string(),
        None => {
            let known: Vec<&str> = SKILLS.iter().map(|(names, _)| names[0]).collect();
            format!("没有「{name}」这个技能。可用：{}", known.join("、"))
        }
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
        assert_eq!(requested("/技能:"), None);
        assert_eq!(requested("/技能战技"), None);
        assert_eq!(requested("看 /技能:战技"), None);
    }

    #[test]
    fn replies_with_a_variant_or_the_known_list() {
        assert_eq!(reply("战技", 4), "释剑去！");
        assert_eq!(reply("大招", 0), "惊霆无声，流电掣雨。");
        assert_eq!(reply("飞天", 0), "没有「飞天」这个技能。可用：战技、连携技、终结技");
    }
}
