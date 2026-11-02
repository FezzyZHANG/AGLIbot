//! The single review entry (docs/DESIGN.md 5d). Four kinds of things come here: research
//! conclusions, actions with outside effects, scope-widening memories, and self-modification.
//!
//! What rules or execution can check is checked by the caller first and passed in as evidence;
//! a strong model agreeing is not proof. Items forwarded to the owner wait in the review queue and
//! go out in the daily digest; urgent ones also get an immediate alert while awake.

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
    pub urgent: bool,
}

#[derive(Deserialize)]
struct Raw {
    verdict: String,
    #[serde(default)]
    reasons: String,
    #[serde(default)]
    fixes: String,
    #[serde(default)]
    urgent: bool,
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
         只输出一个 JSON 对象：{{\"verdict\": \"通过|修改后再审|暂缓|转交主人\", \"reasons\": \"一两句理由\", \"fixes\": \"需要修改时写具体修改意见，否则留空\", \"urgent\": false}}。\
         urgent 只在需要主人尽快处理、不能等每日汇总时设 true，例如尚未核实的外部行动或今天到期的事项。",
        checklist(category)
    );
    // If no reviewer is available or it fails, the item goes to the owner instead of through.
    let scenario = if category == ReviewCategory::Evolve { "evolve" } else { "verify" };
    match models.ask_json_with_limit::<Raw>(scenario, &ProcessingPolicy::Any, system, &user, limit).await {
        Ok(raw) => Ok(Judgement { verdict: Verdict::parse(&raw.verdict), reasons: raw.reasons, fixes: raw.fixes, urgent: raw.urgent }),
        Err(e) => {
            tracing::warn!("review unavailable, forwarding to owner: {e:#}");
            Ok(Judgement { verdict: Verdict::Owner, reasons: format!("没有可用的审查模型：{e}"), fixes: String::new(), urgent: false })
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
        urgent: j.urgent,
        pushed: false,
    };
    store.save_review(&item)?;
    Ok(item)
}

/// Claim before sending: an uncertain send must not be repeated after a restart.
/// `pushed` is saved separately only after delivery succeeds; failures remain in the digest.
pub fn claim_urgent(store: &dyn Store, asleep: bool) -> Result<Option<ReviewItem>> {
    if asleep { return Ok(None) }
    for item in store.reviews()? {
        if item.status != ReviewStatus::Open || !item.urgent || item.pushed { continue }
        let key = format!("review_push_attempt:{}", item.id);
        if store.kv_get(&key)?.is_some() { continue }
        store.kv_set(&key, serde_json::json!(true))?;
        return Ok(Some(item));
    }
    Ok(None)
}

pub fn urgent_message(item: &ReviewItem) -> String {
    format!("有件急事需要你拍板：\n\n#{} [{:?}] {}\n  {}\n  审查：{}\n\n回复 /approve {} 或 /reject {} 理由。",
        item.id, item.category, item.subject, item.detail, item.verdict, item.id, item.id)
}

/// Only explicit same-day deadline wording is classified without a model call.
pub fn deadline_today(text: &str, today: chrono::NaiveDate) -> bool {
    let text = text.to_lowercase();
    let day = ["今天", "今日", "今晚", "today", "tonight"].iter().any(|s| text.contains(s))
        || text.contains(&today.to_string());
    let deadline = ["截止", "到期", "之前", "前完成", "前回复", "前给", "deadline", "due", "by ", "before "]
        .iter().any(|s| text.contains(s));
    day && deadline
}

/// Pending items plus urgent alerts (even if already resolved); marks them digest-notified.
pub fn digest(store: &dyn Store) -> Result<Option<String>> {
    let mut fresh: Vec<ReviewItem> =
        store.reviews()?.into_iter().filter(|r| !r.notified && (r.status == ReviewStatus::Open || r.pushed)).collect();
    if fresh.is_empty() {
        return Ok(None);
    }
    let mut out = String::from("今天的待决事项与已发提醒：\n");
    for r in &mut fresh {
        let sent = if r.pushed { "（已单独推送）" } else { "" };
        let status = match r.status {
            ReviewStatus::Approved => "（已批准）",
            ReviewStatus::Rejected => "（已驳回）",
            ReviewStatus::Closed => "（已关闭）",
            ReviewStatus::Open => "",
        };
        out.push_str(&format!("\n#{} [{:?}] {}{}{}\n  {}\n  审查：{}\n", r.id, r.category, r.subject, sent, status, r.detail, r.verdict));
        r.notified = true;
        store.save_review(r)?;
    }
    out.push_str("\n未决事项回复 /approve 编号 或 /reject 编号 理由。");
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

    #[test]
    fn relay_deadlines_require_today_and_a_deadline() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        for text in ["请转告，今天 18 点前完成", "Deadline today at 18:00", "due 2026-11-02", "今晚之前给我"] {
            assert!(deadline_today(text, day), "{text}");
        }
        for text in ["今天聊得很开心", "deadline tomorrow", "下周截止", "due 2026-11-03"] {
            assert!(!deadline_today(text, day), "{text}");
        }
    }

    #[tokio::test]
    async fn judge_urgency_persists_and_push_claims_survive_restart() {
        use std::sync::Arc;
        use crate::store::jsonl::JsonlStore;
        let dir = std::env::temp_dir().join(format!("agli-urgent-judge-{}", std::process::id()));
        let store = Arc::new(JsonlStore::open(&dir).unwrap());
        let models = Models::stub(store.clone(), vec![
            r#"{"verdict":"转交主人","urgent":true}"#.into(),
            r#"{"verdict":"转交主人"}"#.into(),
        ]);
        for expected in [true, false] {
            let j = review(&models, ReviewCategory::Act, "subject", "evidence").await.unwrap();
            let item = file(store.as_ref(), ReviewCategory::Act, "subject", "detail", &j, false, Value::Null).unwrap();
            assert_eq!(item.urgent, expected);
            assert!(!item.pushed && !item.notified);
        }
        assert_eq!(models.stub_prompts().len(), 2); // No separate urgency call.
        assert!(claim_urgent(store.as_ref(), true).unwrap().is_none());
        let mut item = claim_urgent(store.as_ref(), false).unwrap().unwrap();
        assert!(claim_urgent(store.as_ref(), false).unwrap().is_none());
        let reopened = JsonlStore::open(&dir).unwrap();
        assert!(claim_urgent(&reopened, false).unwrap().is_none());
        item.pushed = true;
        item.status = ReviewStatus::Approved;
        reopened.save_review(&item).unwrap();
        let digest = digest(&reopened).unwrap().unwrap();
        assert!(digest.contains("#1 [Act] subject（已单独推送）"));
        assert!(digest.contains("（已批准）"));
        assert!(digest.contains("#2 [Act] subject\n"));
        assert!(super::digest(&reopened).unwrap().is_none());
        let mut legacy = serde_json::to_value(&item).unwrap();
        legacy.as_object_mut().unwrap().remove("urgent");
        legacy.as_object_mut().unwrap().remove("pushed");
        let legacy: ReviewItem = serde_json::from_value(legacy).unwrap();
        assert!(!legacy.urgent && !legacy.pushed);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
