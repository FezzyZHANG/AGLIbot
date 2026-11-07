//! `aglibot stickers-web`: the owner's page for reviewing collected stickers. A separate process so
//! it stays up while the bot restarts, open to the local network with no login. Edits reach the bot through
//! the store; changed text makes the worker rebuild that sticker's vector.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Config;
use crate::mind::embedding::{EmbeddingCfg, cosine};
use crate::mind::llm::LlmClient;
use crate::mind::media::{file_path, media_dir};
use crate::store::{MediaRecord, MediaStatus, StickerScope, Store};

const DEFAULT_PORT: u16 = 8790;
const PAGE: &str = include_str!("stickers_web.html");
/// Stops other sites' scripts from driving this page: a custom header forces a CORS preflight,
/// which nothing here answers.
const UI_HEADER: &str = "x-stickers-ui";
const LIST_MAX: usize = 300;
const SEARCH_HITS: usize = 5;

struct App {
    store: Arc<dyn Store>,
    dir: PathBuf,
    port: u16,
    llm: LlmClient,
    embedding: Option<EmbeddingCfg>,
}

type Shared = Arc<App>;

pub async fn run(args: impl Iterator<Item = OsString>) -> Result<()> {
    let args: Vec<String> = args.map(|a| a.to_string_lossy().into_owned()).collect();
    let port = match args.as_slice() {
        [] => DEFAULT_PORT,
        [flag, value] if flag == "--port" => value.parse().context("--port needs a number")?,
        _ => anyhow::bail!("usage: aglibot stickers-web [--port N]"),
    };
    let root = std::env::var_os("AGLI_ROOT").map(PathBuf::from).unwrap_or(std::env::current_dir()?);
    let cfg = Config::load(&root)?;
    let store = crate::store::open(&cfg.runtime.mongo, &cfg.data_dir())?;
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await.with_context(|| format!("binding port {port}"))?;
    let app = Arc::new(App {
        store, dir: media_dir(&cfg), port,
        llm: LlmClient::new(cfg.models.providers.clone())?,
        embedding: cfg.models.sticker_embedding.clone(),
    });
    println!("stickers at http://0.0.0.0:{port}");
    axum::serve(listener, router(app).into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}

fn router(app: Shared) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/api/stickers", get(list))
        .route("/api/stickers/{hash}", patch(edit).delete(remove))
        .route("/api/bulk", post(bulk))
        .route("/api/search", post(search))
        .route("/media/{hash}", get(file))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}

fn local_ip(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_private()
}

fn local_peer(peer: IpAddr) -> bool {
    matches!(peer, IpAddr::V4(ip) if local_ip(ip))
}

/// A rebinding attack arrives under its own domain name, so only local addresses are accepted.
fn local_host(host: &str, port: u16) -> bool {
    let Some((name, given)) = host.rsplit_once(':') else { return false };
    given.parse() == Ok(port) && (name == "localhost" || name.parse().is_ok_and(local_ip))
}

/// The network is not enough on its own: any web page a visitor opens can reach it. The peer check
/// keeps out anything forwarded from outside, Host stops DNS rebinding, and the custom header stops
/// cross-site writes.
async fn guard(State(app): State<Shared>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request, next: Next) -> Response {
    let host = request.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
    if !local_peer(peer.ip()) || !local_host(host, app.port) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if request.method() != axum::http::Method::GET && !request.headers().contains_key(UI_HEADER) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

async fn page() -> impl IntoResponse {
    // Model-written text is only ever inserted as text, and this page loads nothing from elsewhere.
    ([(header::CONTENT_SECURITY_POLICY, "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'")], Html(PAGE))
}

fn failure(e: anyhow::Error) -> Response {
    eprintln!("stickers-web: {e:#}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "store unavailable"}))).into_response()
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Images described for someone's private chat are never shown, and neither are their files.
fn visible(record: &MediaRecord) -> bool {
    record.status != MediaStatus::Seen
}

fn found(store: &dyn Store, hash: &str) -> Result<Option<MediaRecord>> {
    if !valid_hash(hash) { return Ok(None); }
    Ok(store.media(hash)?.filter(visible))
}

#[derive(Deserialize, Default)]
struct Filter {
    #[serde(default)]
    status: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    q: String,
    #[serde(default)]
    sort: String,
}

fn parse_status(text: &str) -> Option<MediaStatus> {
    match text {
        "active" => Some(MediaStatus::Active),
        "rejected" => Some(MediaStatus::Rejected),
        "disabled" => Some(MediaStatus::Disabled),
        _ => None,
    }
}

fn parse_scope(text: &str) -> Option<StickerScope> {
    match text {
        "any" => Some(StickerScope::Any),
        "origin" => Some(StickerScope::Origin),
        _ => None,
    }
}

fn matches(record: &MediaRecord, filter: &Filter) -> bool {
    let q = filter.q.trim().to_lowercase();
    visible(record)
        && (filter.status.is_empty() || parse_status(&filter.status) == Some(record.status))
        && (filter.scope.is_empty() || parse_scope(&filter.scope) == Some(record.scope))
        && (filter.origin.is_empty() || filter.origin == record.origin)
        && (q.is_empty() || [&record.description, &record.text, &record.use_when, &record.avoid].iter().any(|t| t.to_lowercase().contains(&q))
            || record.tags.iter().any(|t| t.to_lowercase().contains(&q)))
}

async fn list(State(app): State<Shared>, Query(filter): Query<Filter>) -> Response {
    let all = match app.store.all_media() { Ok(all) => all, Err(e) => return failure(e) };
    let mut origins: BTreeMap<String, usize> = BTreeMap::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for record in all.iter().filter(|r| visible(r)) {
        *origins.entry(record.origin.clone()).or_default() += 1;
        *counts.entry(match record.status {
            MediaStatus::Active => "active",
            MediaStatus::Rejected => "rejected",
            MediaStatus::Disabled => "disabled",
            MediaStatus::Seen => "seen",
        }).or_default() += 1;
    }
    let mut items: Vec<MediaRecord> = all.iter().filter(|r| matches(r, &filter)).cloned().collect();
    if filter.sort == "sent" {
        items.sort_by(|a, b| b.sent_count.cmp(&a.sent_count).then(b.last_seen.cmp(&a.last_seen)));
    } else {
        items.sort_by(|a, b| b.first_seen.cmp(&a.first_seen));
    }
    let total = items.len();
    items.truncate(LIST_MAX);
    let items: Vec<Value> = items.iter().map(|record| {
        let has_file = file_path(&app.dir, &record.hash, &record.ext).exists();
        let mut value = serde_json::to_value(record).unwrap_or_default();
        value["has_file"] = json!(has_file);
        value
    }).collect();
    Json(json!({"total": total, "items": items, "origins": origins, "counts": counts})).into_response()
}

async fn file(State(app): State<Shared>, Path(hash): Path<String>) -> Response {
    let record = match found(app.store.as_ref(), &hash) {
        Ok(Some(record)) => record,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return failure(e),
    };
    let kind = match record.ext.as_str() {
        "jpg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    match std::fs::read(file_path(&app.dir, &record.hash, &record.ext)) {
        Ok(bytes) => ([(header::CONTENT_TYPE, kind), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize, Default)]
struct Edit {
    description: Option<String>,
    use_when: Option<String>,
    avoid: Option<String>,
    tags: Option<Vec<String>>,
    scope: Option<String>,
    status: Option<String>,
}

async fn edit(State(app): State<Shared>, Path(hash): Path<String>, Json(edit): Json<Edit>) -> Response {
    let mut record = match found(app.store.as_ref(), &hash) {
        Ok(Some(record)) => record,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return failure(e),
    };
    if let Some(text) = edit.description { record.description = text.trim().to_string(); }
    if let Some(text) = edit.use_when { record.use_when = text.trim().to_string(); }
    if let Some(text) = edit.avoid { record.avoid = text.trim().to_string(); }
    if let Some(tags) = edit.tags { record.tags = tags.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(); }
    if let Some(text) = edit.scope {
        let Some(scope) = parse_scope(&text) else { return StatusCode::BAD_REQUEST.into_response() };
        record.scope = scope;
    }
    if let Some(text) = edit.status {
        let Some(status) = parse_status(&text) else { return StatusCode::BAD_REQUEST.into_response() };
        record.status = status;
    }
    match app.store.save_media(&record) {
        Ok(()) => Json(record).into_response(),
        Err(e) => failure(e),
    }
}

fn delete(app: &App, record: &MediaRecord) -> Result<()> {
    app.store.delete_media(&record.hash)?;
    let path = file_path(&app.dir, &record.hash, &record.ext);
    if path.exists() { std::fs::remove_file(path)?; }
    Ok(())
}

async fn remove(State(app): State<Shared>, Path(hash): Path<String>) -> Response {
    let record = match found(app.store.as_ref(), &hash) {
        Ok(Some(record)) => record,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return failure(e),
    };
    match delete(&app, &record) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => failure(e),
    }
}

#[derive(Deserialize)]
struct Bulk {
    hashes: Vec<String>,
    action: String,
}

async fn bulk(State(app): State<Shared>, Json(bulk): Json<Bulk>) -> Response {
    let status = match bulk.action.as_str() {
        "delete" => None,
        other => match parse_status(other) {
            Some(status) => Some(status),
            None => return StatusCode::BAD_REQUEST.into_response(),
        },
    };
    let mut done = 0;
    for hash in &bulk.hashes {
        let Ok(Some(mut record)) = found(app.store.as_ref(), hash) else { continue };
        let result = match status {
            Some(status) => {
                record.status = status;
                app.store.save_media(&record)
            }
            None => delete(&app, &record),
        };
        if let Err(e) = result { return failure(e); }
        done += 1;
    }
    Json(json!({"done": done})).into_response()
}

#[derive(Deserialize)]
struct Search {
    text: String,
}

#[derive(Serialize)]
struct Hit {
    score: f64,
    record: MediaRecord,
}

/// The same ranking the bot will use, so a bad result here is a bad result there.
async fn search(State(app): State<Shared>, Json(search): Json<Search>) -> Response {
    let Some(cfg) = &app.embedding else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "没有配置 sticker_embedding"}))).into_response();
    };
    let (query, _) = match app.llm.embed(cfg, &search.text).await {
        Ok(result) => result,
        Err(_) => return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "向量模型不可用，Ollama 在运行吗？"}))).into_response(),
    };
    let (vectors, all) = match (app.store.sticker_vectors(&cfg.model_id()), app.store.all_media()) {
        (Ok(vectors), Ok(all)) => (vectors, all),
        (Err(e), _) | (_, Err(e)) => return failure(e),
    };
    let mut hits: Vec<Hit> = all.into_iter().filter(|r| r.status == MediaStatus::Active).filter_map(|record| {
        let stored = vectors.get(&record.hash)?;
        let score = cosine(&query, &stored.vector).ok()?;
        Some(Hit { score, record })
    }).collect();
    let waiting = app.store.all_media().map(|all| all.iter().filter(|r| r.status == MediaStatus::Active && !vectors.contains_key(&r.hash)).count()).unwrap_or(0);
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    hits.truncate(SEARCH_HITS);
    Json(json!({"hits": hits, "waiting": waiting})).into_response()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::mind::media::sticker_text;
    use crate::store::jsonl::JsonlStore;

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const PRIVATE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn record(hash: &str, status: MediaStatus) -> MediaRecord {
        MediaRecord {
            hash: hash.into(), ext: "png".into(), status, keep: true, description: "猫瞪着屏幕".into(), text: String::new(),
            use_when: "无语时".into(), avoid: String::new(), tags: vec!["猫".into()], scope: StickerScope::Any,
            scope_reason: String::new(), origin: "qq:group:1".into(), first_seen: Utc::now(), last_seen: Utc::now(),
            sightings: 1, sent_count: 0, last_sent: None,
        }
    }

    struct Running {
        base: String,
        port: u16,
        store: Arc<dyn Store>,
        dir: PathBuf,
        http: reqwest::Client,
    }

    async fn start(name: &str) -> Running {
        let dir = std::env::temp_dir().join(format!("aglibot-test-web-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store: Arc<dyn Store> = Arc::new(JsonlStore::open(&dir).unwrap());
        store.save_media(&record(HASH, MediaStatus::Active)).unwrap();
        store.save_media(&record(PRIVATE, MediaStatus::Seen)).unwrap();
        for hash in [HASH, PRIVATE] { std::fs::write(file_path(&dir, hash, "png"), b"png").unwrap(); }
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Arc::new(App { store: store.clone(), dir: dir.clone(), port, llm: LlmClient::new(Default::default()).unwrap(), embedding: None });
        tokio::spawn(async move { axum::serve(listener, router(app).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        Running { base: format!("http://127.0.0.1:{port}"), port, store, dir, http }
    }

    #[tokio::test]
    async fn the_list_never_shows_private_images() {
        let web = start("list").await;
        let listed: Value = web.http.get(format!("{}/api/stickers", web.base)).send().await.unwrap().json().await.unwrap();
        assert_eq!(listed["total"], 1);
        assert_eq!(listed["items"][0]["hash"], HASH);
        assert_eq!(listed["items"][0]["has_file"], true);
        assert_eq!(web.http.get(format!("{}/media/{HASH}", web.base)).send().await.unwrap().status(), 200);
        assert_eq!(web.http.get(format!("{}/media/{PRIVATE}", web.base)).send().await.unwrap().status(), 404);
        // Not a hash, so never reaches the filesystem.
        assert_eq!(web.http.get(format!("{}/media/..%2F..%2Fsecret", web.base)).send().await.unwrap().status(), 404);
    }

    #[tokio::test]
    async fn writes_need_the_page_header_and_a_local_host() {
        let web = start("guard").await;
        let url = format!("{}/api/stickers/{HASH}", web.base);
        let change = json!({"status": "disabled"});
        assert_eq!(web.http.patch(&url).json(&change).send().await.unwrap().status(), 403);
        assert_eq!(web.http.get(format!("{}/api/stickers", web.base)).header("host", "evil.example").send().await.unwrap().status(), 403);
        assert_eq!(web.store.media(HASH).unwrap().unwrap().status, MediaStatus::Active);
        for host in ["8.8.8.8", "192.168.1.9.evil.example"] {
            let response = web.http.get(format!("{}/api/stickers", web.base)).header("host", format!("{host}:{}", web.port)).send().await.unwrap();
            assert_eq!(response.status(), 403, "{host}");
        }
        assert_eq!(web.http.get(format!("{}/api/stickers", web.base)).header("host", "192.168.1.9:1").send().await.unwrap().status(), 403);
        assert_eq!(web.http.get(format!("{}/api/stickers", web.base)).header("host", format!("192.168.1.9:{}", web.port)).send().await.unwrap().status(), 200);

        let response = web.http.patch(&url).header(UI_HEADER, "1").json(&change).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(web.store.media(HASH).unwrap().unwrap().status, MediaStatus::Disabled);
    }

    #[test]
    fn only_local_network_peers_are_served() {
        for peer in ["127.0.0.1", "192.168.1.20", "10.0.0.2", "172.22.96.5"] {
            assert!(local_peer(peer.parse().unwrap()), "{peer}");
        }
        for peer in ["8.8.8.8", "169.254.1.1", "::1", "2001:db8::1"] {
            assert!(!local_peer(peer.parse().unwrap()), "{peer}");
        }
    }

    #[tokio::test]
    async fn edits_change_the_embedded_text_and_delete_removes_the_file() {
        let web = start("edit").await;
        let url = format!("{}/api/stickers/{HASH}", web.base);
        let before = sticker_text(&web.store.media(HASH).unwrap().unwrap());
        let body = json!({"description": "狗", "tags": ["狗", " "], "scope": "origin"});
        assert_eq!(web.http.patch(&url).header(UI_HEADER, "1").json(&body).send().await.unwrap().status(), 200);
        let edited = web.store.media(HASH).unwrap().unwrap();
        assert_eq!(edited.tags, ["狗"]);
        assert_eq!(edited.scope, StickerScope::Origin);
        assert_ne!(sticker_text(&edited), before);
        assert_eq!(web.http.patch(&url).header(UI_HEADER, "1").json(&json!({"scope": "x"})).send().await.unwrap().status(), 400);

        assert_eq!(web.http.delete(&url).header(UI_HEADER, "1").send().await.unwrap().status(), 204);
        assert!(web.store.media(HASH).unwrap().is_none());
        assert!(!file_path(&web.dir, HASH, "png").exists());
    }

    #[tokio::test]
    async fn bulk_skips_private_images_and_search_says_when_it_cannot_run() {
        let web = start("bulk").await;
        let body = json!({"hashes": [HASH, PRIVATE, "nonsense"], "action": "disabled"});
        let done: Value = web.http.post(format!("{}/api/bulk", web.base)).header(UI_HEADER, "1").json(&body).send().await.unwrap().json().await.unwrap();
        assert_eq!(done["done"], 1);
        assert_eq!(web.store.media(PRIVATE).unwrap().unwrap().status, MediaStatus::Seen);
        let search = web.http.post(format!("{}/api/search", web.base)).header(UI_HEADER, "1").json(&json!({"text": "猫"})).send().await.unwrap();
        assert_eq!(search.status(), 503);
    }
}
