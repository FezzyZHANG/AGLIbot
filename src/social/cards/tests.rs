use super::*;
use crate::{social::people::People, store::{jsonl::JsonlStore, Episode, Fact}, ns::Scope};
use chrono::Utc;

#[test]
fn private_filtered_capped_stable_cards_remove_tier_drops_and_agents() {
    let dir = std::env::temp_dir().join(format!("agli-cards-{}-{}", std::process::id(), Utc::now().timestamp_nanos_opt().unwrap()));
    let store = JsonlStore::open(&dir).unwrap();
    let mut people = People::load(&store).unwrap();
    for (id, tier) in [("owner", Tier::Owner), ("qq:1", Tier::Acquaintance), ("agent", Tier::Acquaintance),
        ("known", Tier::Acquaintance), ("confirmed", Tier::Acquaintance), ("far", Tier::Groupmate)] {
        people.ensure(&store, id, id, tier).unwrap();
    }
    let mut agent = people.get("agent").unwrap().clone();
    agent.kind = PersonKind::Agent;
    people.save(&store, agent).unwrap();
    let mut known = people.get("known").unwrap().clone();
    known.platform_ids.push("qq:99".into());
    people.save(&store, known).unwrap();
    bot_flags::settle(&store, "confirmed", true).unwrap();
    let ns: crate::ns::Namespace = "person/qq:1".parse().unwrap();
    for i in 0..25 {
        people.add_fact(&store, "qq:1", Fact { text: format!("allowed-{i}"), ns: ns.clone(), scope: Scope::Public, ts: Utc::now() }).unwrap();
    }
    people.add_fact(&store, "qq:1", Fact { text: "SECRET-FACT".into(), ns: ns.clone(), scope: Scope::Persons { ids: vec!["qq:1".into()] }, ts: Utc::now() }).unwrap();
    for i in 0..15 {
        store.append_episode(&Episode { id: i.to_string(), ts: Utc::now(), participants: vec!["qq:1".into()],
            summary: format!("episode-{i}"), emotion: String::new(), salience: 0.5, ns: ns.clone(),
            scope: Scope::Public, sources: vec![] }).unwrap();
    }
    store.append_episode(&Episode { id: "secret".into(), ts: Utc::now(), participants: vec!["qq:1".into()],
        summary: "SECRET-EPISODE".into(), emotion: String::new(), salience: 1.0, ns,
        scope: Scope::SelfOnly, sources: vec![] }).unwrap();
    let export = || nightly(&store, &dir, "owner", &["qq:99".into()]).unwrap();
    export();
    let path = dir.join("people/qq%3A1.md");
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(!content.contains("SECRET"));
    assert_eq!(content.matches("allowed-").count(), 20);
    assert_eq!(content.matches("episode-").count(), 10);
    assert_eq!(std::fs::read_dir(dir.join("people")).unwrap().count(), 1);
    let time = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(time).unwrap();
    let time = std::fs::metadata(&path).unwrap().modified().unwrap();
    export();
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), time);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    let mut p = people.get("qq:1").unwrap().clone();
    p.tier = Tier::Groupmate;
    people.save(&store, p).unwrap();
    export();
    assert!(!path.exists());
    assert_eq!(filename("../CON"), "%2E%2E%2FCON.md");
    assert_eq!(filename("CON"), "%43ON.md");
    std::fs::remove_dir_all(dir).unwrap();
}
