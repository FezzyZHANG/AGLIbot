//! The single review entry (docs/DESIGN.md 5d). Four kinds of things come here: research
//! conclusions, actions with outside effects, scope-widening memories, and self-modification.
//!
//! What rules or execution can check is checked by the caller first and passed in as evidence;
//! a strong model agreeing is not proof. Items forwarded to the owner wait in the review queue and
//! go out in the daily digest, so review never becomes the owner's new chore.

use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;

use super::models::{Models, SpendLimit};
use crate::ns::ProcessingPolicy;
use crate::store::{ReviewCategory, ReviewItem, ReviewStatus, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Revise,
    Defer,
    Owner,
}

impl Verdict {
    pub fn zh(self) -> &'static str {
        match self {
            Verdict::Pass => "通过",
            Verdict::Revise => "修改后再审",
            Verdict::Defer => "暂缓",
            Verdict::Owner => "转交主人",
        }
    }

    fn parse(s: &str) -> Verdict {
        match s.trim() {
            "通过" | "pass" => Verdict::Pass,
            "修改后再审" | "revise" => Verdict::Revise,
            "暂缓" | "defer" => Verdict::Defer,
            // Anything unclear goes to the owner rather than through.
            _ => Verdict::Owner,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Judgement {
    pub verdict: Verdict,
    pub reasons: String,
    pub fixes: String,
}

#[derive(Deserialize)]
struct Raw {
    verdict: String,
    #[serde(default)]
    reasons: String,
    #[serde(default)]
    fixes: String,
}

fn checklist(category: ReviewCategory) -> &'static str {
    match category {
        ReviewCategory::Verify => {
            "证据：每条结论是否有可追溯的出处，一手还是二手，“声称”和“证实”是否分开；\
             任务符合度：是否回答了被问的问题；置信度和未确定项是否写清。"
        }
        ReviewCategory::Act => "权限：请求者是否有权要求这件事；影响：对外会造成什么后果、能否撤回；隐私：是否泄露他人信息。",
        ReviewCategory::Widen => {
            "隐私：去掉名字之后，时间、关系、独特事件的组合是否仍能认出当事人；\
             这条内容扩大可见范围后，原来告诉他的人会不会介意。"
        }
        ReviewCategory::Evolve => "修改影响：改了什么、为什么、测试证据是否充分、能否回滚；是否触碰边界或权限配置。",
    }
}

/// Ask the reviewer model. `evidence` should already contain rule/test/execution results.
pub async fn review(models: &Models, category: ReviewCategory, subject: &str, evidence: &str) -> Result<Judgement> {
    review_with_limit(models, category, subject, evidence, None).await
}

pub async fn review_with_limit(models: &Models, category: ReviewCategory, subject: &str, evidence: &str, limit: Option<SpendLimit>) -> Result<Judgement> {
    let system = "你是一个严格的审稿人。你只做判断，不改写内容。强模型同意不等于正确：没有证据支持的结论不能通过。";
    let user = format!(
        "审查类别：{category:?}\n检查要点：{}\n\n## 审查对象\n{subject}\n\n## 材料与证据\n{evidence}\n\n\
         只输出一个 JSON 对象：{{\"verdict\": \"通过|修改后再审|暂缓|转交主人\", \"reasons\": \"一两句理由\", \"fixes\": \"需要修改时写具体修改意见，否则留空\"}}",
        checklist(category)
    );
    // If no reviewer is available or it fails, the item goes to the owner instead of through.
    match models.ask_json_with_limit::<Raw>("verify", &ProcessingPolicy::Any, system, &user, limit).await {
        Ok(raw) => Ok(Judgement { verdict: Verdict::parse(&raw.verdict), reasons: raw.reasons, fixes: raw.fixes }),
        Err(e) => {
            tracing::warn!("review unavailable, forwarding to owner: {e:#}");
            Ok(Judgement { verdict: Verdict::Owner, reasons: format!("没有可用的审查模型：{e}"), fixes: String::new() })
        }
    }
}

/// Record a judgement. Forwarded items stay open for the owner; everything else is closed.
pub fn file(
    store: &dyn Store,
    category: ReviewCategory,
    subject: &str,
    detail: &str,
    j: &Judgement,
    forward: bool,
    payload: Value,
) -> Result<ReviewItem> {
    let item = ReviewItem {
        id: store.next_review_id()?,
        ts: Utc::now(),
        category,
        subject: subject.into(),
        detail: detail.into(),
        verdict: format!("{}：{}", j.verdict.zh(), j.reasons),
        status: if forward || j.verdict == Verdict::Owner { ReviewStatus::Open } else { ReviewStatus::Closed },
        payload,
        notified: false,
    };
    store.save_review(&item)?;
    Ok(item)
}

/// Open items not yet sent to the owner, formatted for one message; marks them notified.
pub fn digest(store: &dyn Store) -> Result<Option<String>> {
    let mut fresh: Vec<ReviewItem> =
        store.reviews()?.into_iter().filter(|r| r.status == ReviewStatus::Open && !r.notified).collect();
    if fresh.is_empty() {
        return Ok(None);
    }
    let mut out = String::from("今天有几件事需要你拍板：\n");
    for r in &mut fresh {
        out.push_str(&format!("\n#{} [{:?}] {}\n  {}\n  审查：{}\n", r.id, r.category, r.subject, r.detail, r.verdict));
        r.notified = true;
        store.save_review(r)?;
    }
    out.push_str("\n回复 /approve 编号 或 /reject 编号 理由。");
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unclear_verdicts_go_to_the_owner() {
        assert_eq!(Verdict::parse("通过"), Verdict::Pass);
        assert_eq!(Verdict::parse(" 暂缓 "), Verdict::Defer);
        assert_eq!(Verdict::parse("looks fine"), Verdict::Owner);
    }
}
