use super::*;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Local;
use serde_json::json;

use crate::mind::{decide, models::Models, reflect};
use crate::social::people::People;
use crate::store::jsonl::JsonlStore;

struct Fixture {
    dir: PathBuf,
    store: Arc<JsonlStore>,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("aglibot-commitments-{name}-{}", std::process::id()));
        let store = Arc::new(JsonlStore::open(&dir.join("data")).unwrap());
        Self { dir, store }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}

fn time() -> DateTime<Utc> {
    "2026-11-02T12:00:00.123456789Z".parse().unwrap()
}

fn message(offset: i64, outgoing: bool, text: &str) -> MessageRecord {
    MessageRecord {
        ts: time() + Duration::seconds(offset), platform: "telegram".into(), chat_id: "42".into(),
        sender: if outgoing { "me" } else { "owner" }.into(), sender_name: String::new(),
        outgoing, text: text.into(), ns: Namespace::Person("owner".into()),
        scope: Scope::Persons { ids: vec!["owner".into()] },
    }
}

fn item(id: &str, days: Option<i64>, status: Status) -> Commitment {
    Commitment {
        id: id.into(), who: "owner".into(), what: format!("promise {id}"),
        due: days.map(|d| time().with_timezone(&Local).date_naive() + Duration::days(d)),
        ns: Namespace::Person("owner".into()), scope: Scope::Persons { ids: vec!["owner".into()] },
        created: time(), status, evidence: time(), completed_evidence: None,
    }
}

#[tokio::test]
async fn reflection_extracts_own_promises_and_intentions_once_with_source_scope() {
    let f = Fixture::new("extract");
    let mut people = People::load(f.store.as_ref()).unwrap();
    let messages = [
        message(0, false, "我明天会发资料给你"),
        message(1, true, "我明天把报告给你；我也打算学 Rust"),
        MessageRecord { scope: Scope::Public, ..message(2, false, "好") },
    ];
    for m in &messages {
        f.store.append_message(m).unwrap();
    }
    let response = json!({
        "episodes": [{"summary": "商量了报告", "participants": ["owner", "me"]}],
        "facts": [], "lessons": [],
        "commitments": [
            {"who":"owner", "what":"发送报告", "due":"2026-11-03", "evidence":messages[1].ts,
             "ns":"group/forged", "scope":{"kind":"public"}, "id":"overwrite", "status":"done"},
            {"who":"me", "what":"学 Rust", "due":null, "evidence":messages[1].ts},
            {"who":"owner", "what":"别人的承诺", "evidence":messages[0].ts},
            {"who":"stranger", "what":"编造对象", "evidence":messages[1].ts},
            {"who":"owner", "what":"编造证据", "evidence":time() + Duration::days(1)},
            {"who":"owner", "what":"  ", "evidence":messages[1].ts}
        ]
    });
    let models = Models::stub(f.store.clone(), vec![format!("```json\n{response}\n```")]);
    let ns = &messages[0].ns;
    let outcome = reflect::extract_ns(&models, f.store.as_ref(), &mut people, ns).await.unwrap();
    assert_eq!((outcome.messages, outcome.episodes), (3, 1));
    let items = load(f.store.as_ref()).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].who, "owner");
    assert_eq!(items[0].due, NaiveDate::from_ymd_opt(2026, 11, 3));
    assert_eq!(items[1].who, "me");
    assert_eq!(items[1].due, None);
    for c in &items {
        assert_eq!(c.ns, *ns);
        assert_eq!(c.scope, messages[1].scope);
        assert_eq!(c.evidence, messages[1].ts);
        assert_eq!(c.status, Status::Open);
        assert!(c.id.starts_with("commitment-"));
    }
    assert_eq!(reflect::watermark(f.store.as_ref(), ns).unwrap(), messages[2].ts);
    assert_eq!(reflect::extract_ns(&models, f.store.as_ref(), &mut people, ns).await.unwrap().messages, 0);
    let calls = models.stub_prompts();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "extract");
    assert!(calls[0].2.contains(&messages[1].ts.with_timezone(&Local).to_rfc3339()));
    assert!(calls[0].2.contains("completed_commitments"));

    // Retrying before a watermark write cannot duplicate or reopen the same evidence.
    apply(f.store.as_ref(), ns, &messages, serde_json::from_value(response).unwrap(), time()).unwrap();
    assert_eq!(load(f.store.as_ref()).unwrap().len(), 2);
    let reopened = JsonlStore::open(&f.dir.join("data")).unwrap();
    assert_eq!(load(&reopened).unwrap()[0].id, items[0].id);
}

#[tokio::test]
async fn completion_requires_existing_open_item_in_source_namespace_and_real_evidence() {
    let f = Fixture::new("done");
    let mut group = item("other-namespace", Some(0), Status::Open);
    group.ns = Namespace::Group("g".into());
    group.scope = Scope::Group { id: "g".into() };
    let items = vec![item("fulfilled", Some(0), Status::Open), item("no-evidence", Some(0), Status::Open),
        item("dropped", Some(0), Status::Dropped), group];
    f.store.kv_set(KEY, json!(items)).unwrap();
    let m = message(10, false, "报告收到了，谢谢");
    f.store.append_message(&m).unwrap();
    let response = json!({"completed_commitments":[
        {"id":"fulfilled", "evidence":m.ts},
        {"id":"other-namespace", "evidence":m.ts},
        {"id":"no-evidence", "evidence":time() + Duration::days(1)},
        {"id":"dropped", "evidence":m.ts},
        {"id":"invented", "evidence":m.ts}
    ]});
    let models = Models::stub(f.store.clone(), vec![response.to_string()]);
    let mut people = People::load(f.store.as_ref()).unwrap();
    reflect::extract_ns(&models, f.store.as_ref(), &mut people, &m.ns).await.unwrap();
    let after = load(f.store.as_ref()).unwrap();
    assert_eq!(after.len(), 4);
    assert_eq!(after[0].status, Status::Done);
    assert_eq!(after[0].evidence, items[0].evidence);
    assert_eq!(after[0].completed_evidence, Some(m.ts));
    assert_eq!(after[1].status, Status::Open);
    assert_eq!(after[2].status, Status::Dropped);
    assert_eq!(after[3].status, Status::Open);
    let prompt = &models.stub_prompts()[0].2;
    assert!(prompt.contains("fulfilled"));
    assert!(!prompt.contains("other-namespace"));
    assert!(!prompt.contains("dropped"));
}

#[tokio::test]
async fn due_soon_is_bounded_sorted_open_only_and_reaches_decide() {
    let f = Fixture::new("due");
    let items = vec![item("undated", None, Status::Open), item("future", Some(4), Status::Open),
        item("done", Some(-20), Status::Done), item("dropped", Some(-20), Status::Dropped),
        item("soon", Some(3), Status::Open), item("tomorrow", Some(1), Status::Open),
        item("today", Some(0), Status::Open), item("overdue", Some(-1), Status::Open)];
    f.store.kv_set(KEY, json!(items)).unwrap();
    let mut s = decide::Situation { text: "quiet day".into(), commitments: vec![], startable: vec![], may_message_owner: true };
    s.include_commitments(f.store.as_ref(), time(), false).unwrap();
    assert_eq!(s.commitments.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["overdue", "today", "tomorrow", "soon"]);
    let models = Models::stub(f.store.clone(), vec![r#"{"action":"message","target":"owner","reason":"check report"}"#.into()]);
    let d = decide::decide(&models, &ProcessingPolicy::Any, "identity", &s).await.unwrap();
    assert!(matches!(decide::validate(&d, &s), decide::Action::MessageOwner { .. }));
    let prompt = &models.stub_prompts()[0].2;
    assert!(prompt.contains("promise overdue"));
    assert!(prompt.contains("promise soon"));
    for excluded in ["undated", "future", "done", "dropped"] {
        assert!(!prompt.contains(&format!("promise {excluded}")));
    }
    let mut many = items;
    many.extend((2..10).map(|i| item(&format!("old-{i}"), Some(-i), Status::Open)));
    f.store.kv_set(KEY, json!(many)).unwrap();
    s.include_commitments(f.store.as_ref(), time(), false).unwrap();
    assert_eq!(s.commitments.len(), 5);
    assert_eq!(s.commitments[0].id, "old-9");
    s.commitments[0].what = "many\nlines\r\n".repeat(100);
    assert_eq!(summary(&s.commitments).lines().count(), 5);
    assert!(summary(&s.commitments).chars().count() < 1000);
}

#[tokio::test]
async fn self_scoped_material_requires_local_processing_in_extraction_and_decide() {
    let f = Fixture::new("policy");
    let mut c = item("private", Some(0), Status::Open);
    c.scope = Scope::SelfOnly;
    f.store.kv_set(KEY, json!([c])).unwrap();
    let m = message(1, true, "自己想想");
    let (_, policy) = extraction_context(f.store.as_ref(), &m.ns, std::slice::from_ref(&m)).unwrap();
    assert_eq!(policy, ProcessingPolicy::LocalOnly);
    f.store.kv_set(KEY, json!([])).unwrap();
    let restricted = MessageRecord { scope: Scope::SelfOnly, ..m.clone() };
    let (_, policy) = extraction_context(f.store.as_ref(), &m.ns, &[restricted, m.clone()]).unwrap();
    assert_eq!(policy, ProcessingPolicy::LocalOnly);
    f.store.kv_set(KEY, json!([c])).unwrap();
    let mut s = decide::Situation { text: String::new(), commitments: vec![], startable: vec![], may_message_owner: false };
    s.include_commitments(f.store.as_ref(), time(), false).unwrap();
    assert!(s.commitments.is_empty());
    s.include_commitments(f.store.as_ref(), time(), true).unwrap();
    let models = Models::stub(f.store.clone(), vec![r#"{"action":"nothing"}"#.into()]);
    decide::decide(&models, &ProcessingPolicy::Any, "identity", &s).await.unwrap();
    assert_eq!(models.stub_prompts()[0].1, ProcessingPolicy::LocalOnly);
}

#[test]
fn rendered_file_is_a_rebuildable_private_view_of_store() {
    let f = Fixture::new("render");
    let mut done = item("done", Some(0), Status::Done);
    done.completed_evidence = Some(time() + Duration::hours(1));
    f.store.kv_set(KEY, json!([item("open", None, Status::Open), done, item("dropped", None, Status::Dropped)])).unwrap();
    let mind = f.dir.join("mind");
    render_view(f.store.as_ref(), &mind).unwrap();
    let path = mind.join("agenda/commitments.md");
    let text = std::fs::read_to_string(&path).unwrap();
    for expected in ["promise open", "owner", "未定", "person/owner", "persons", "open", "done", "dropped", "完成证据"] {
        assert!(text.contains(expected), "missing {expected}");
    }
    assert!(text.contains(&time().to_rfc3339()));
    std::fs::write(&path, "edited view").unwrap();
    assert_eq!(load(f.store.as_ref()).unwrap().len(), 3);
    render_view(f.store.as_ref(), &mind).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert!(include_str!("../../../.gitignore").lines().any(|l| l == "/mind/agenda/"));
}

#[tokio::test]
async fn legacy_extraction_json_still_works_and_bad_storage_is_not_overwritten() {
    let f = Fixture::new("legacy");
    let m = message(0, true, "你好");
    f.store.append_message(&m).unwrap();
    let models = Models::stub(f.store.clone(), vec!["{}".into()]);
    let mut people = People::load(f.store.as_ref()).unwrap();
    reflect::extract_ns(&models, f.store.as_ref(), &mut people, &m.ns).await.unwrap();
    assert!(load(f.store.as_ref()).unwrap().is_empty());
    f.store.kv_set(KEY, json!("corrupt")).unwrap();
    assert!(apply(f.store.as_ref(), &m.ns, std::slice::from_ref(&m), Extracted::default(), time()).is_err());
    assert_eq!(f.store.kv_get(KEY).unwrap(), Some(json!("corrupt")));
}
