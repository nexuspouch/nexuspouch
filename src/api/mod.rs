use crate::admin::auth::{self, AuthConfig};
use crate::agents::AgentRegistry;
use crate::events::{EventBus, StoreEvent};
use crate::protocol::{self, Frame};
use crate::store::{Local, OpError, MAX_CHUNK};
use crate::uri::{self, RefKind, StoreUri};
use axum::{
    body::Body,
    extract::{ConnectInfo, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::{self, StreamExt};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::convert::Infallible;
use std::io::{Read, Seek, SeekFrom};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

pub struct ApiState {
    pub store: Arc<Local>,
    pub auth: AuthConfig,
    pub device: String,
    pub events: Arc<EventBus>,
    pub mdns: bool,
    pub agents: Arc<AgentRegistry>,
}

pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/uri/resolve", get(uri_resolve))
        .route("/read", get(read_uri))
        .route("/list", get(list_uri))
        .route("/versions", get(versions_uri))
        .route("/manifest", get(manifest_uri))
        .route("/artifact/state", get(artifact_state_uri))
        .route("/search", get(search_uri))
        .route("/store", post(store_op))
        .route("/events", get(events_sse))
        .route("/events/recent", get(events_recent))
        .with_state(state)
}

fn authorize(
    auth: &AuthConfig,
    headers: &HeaderMap,
    query_token: Option<&str>,
    loopback: bool,
    required: &[&str],
) -> bool {
    auth.authorize_scopes(headers, query_token, loopback, required)
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("WWW-Authenticate", r#"Bearer realm="nexuspouch-api""#)],
        "unauthorized",
    )
        .into_response()
}

/// Enforce agent scope / quota / rate limits for a request carrying an agent
/// binding (bearer token `agent_id` or `x-agent-id` header). Returns `Some`
/// (403 response) on denial, `None` when no agent is bound or checks pass.
fn agent_check(
    state: &ApiState,
    headers: &HeaderMap,
    query_token: Option<&str>,
    op: &str,
    space: &str,
    write_size: u64,
) -> Option<Response> {
    let agent_id = state.auth.resolve_agent_id(headers, query_token)?;
    match state.agents.check(&agent_id, op, space, write_size) {
        Ok(()) => None,
        Err(msg) => {
            let code = msg.split(':').next().unwrap_or("acl_denied").trim();
            state.store.audit_action(code, &agent_id, op, &msg);
            Some((
                StatusCode::FORBIDDEN,
                Json(json!({"error": code, "message": msg, "agent_id": agent_id})),
            )
                .into_response())
        }
    }
}

fn op_error(e: OpError) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": e.code, "message": e.msg})),
    )
        .into_response()
}

async fn health(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !authorize(&state.auth, &headers, None, auth::is_loopback(&addr.ip().to_string()), &["read", "admin"]) {
        return unauthorized();
    }
    Json(json!({
        "ok": true,
        "device": state.device,
        "mdns": state.mdns,
        "protocol": protocol::PROTOCOL_VERSION,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct UriQuery {
    uri: String,
}

async fn uri_resolve(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<UriQuery>,
) -> Response {
    if !authorize(&state.auth, &headers, None, auth::is_loopback(&addr.ip().to_string()), &["read", "admin"]) {
        return unauthorized();
    }
    let parsed = match uri::parse_with(&q.uri, &|sp| state.store.is_known_space(sp)) {
        Ok(u) => u,
        Err(e) => return op_error(e),
    };
    if let Some(resp) = agent_check(&state, &headers, None, "meta", &parsed.space, 0) {
        return resp;
    }
    let meta = match &parsed.ref_kind {
        RefKind::Latest => match state.store.handle(
            parsed.to_frame_meta(),
            &state.device,
            protocol::TRUST_OWNER,
            true,
        ) {
            Ok(m) => m,
            Err(e) => return op_error(e),
        },
        _ => match crate::store::versions::meta(
            &state.store,
            &parsed.space,
            &parsed.device,
            &parsed.path,
            &parsed.ref_kind,
        ) {
            Ok(m) => m,
            Err(e) => return op_error(e),
        },
    };
    Json(json!({
        "uri": parsed.format_with_ref(),
        "space": parsed.space,
        "device": parsed.device,
        "path": parsed.path,
        "meta": meta,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct VersionsQuery {
    uri: String,
}

async fn versions_uri(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<VersionsQuery>,
) -> Response {
    if !authorize(&state.auth, &headers, None, auth::is_loopback(&addr.ip().to_string()), &["read", "admin"]) {
        return unauthorized();
    }
    let parsed = match uri::parse_with(&q.uri, &|sp| state.store.is_known_space(sp)) {
        Ok(u) => u,
        Err(e) => return op_error(e),
    };
    if parsed.path.is_empty() {
        return op_error(OpError::new("bad_path", "versions require a file path"));
    }
    if let Some(resp) = agent_check(&state, &headers, None, "versions.list", &parsed.space, 0) {
        return resp;
    }
    let mut payload = Map::new();
    payload.insert("space".into(), json!(parsed.space));
    payload.insert("device".into(), json!(parsed.device));
    payload.insert("path".into(), json!(parsed.path));
    match state.store.handle(
        Frame::from_parts("versions.list", payload),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    ) {
        Ok(data) => Json(json!({
            "uri": parsed.format(),
            "space": parsed.space,
            "device": parsed.device,
            "path": parsed.path,
            "protected": data.get("protected").cloned().unwrap_or(json!(false)),
            "versions": data.get("versions").cloned().unwrap_or(json!([])),
        }))
        .into_response(),
        Err(e) => op_error(e),
    }
}

async fn manifest_uri(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<VersionsQuery>,
) -> Response {
    if !authorize(&state.auth, &headers, None, auth::is_loopback(&addr.ip().to_string()), &["read", "admin"]) {
        return unauthorized();
    }
    let parsed = match uri::parse_with(&q.uri, &|sp| state.store.is_known_space(sp)) {
        Ok(u) => u,
        Err(e) => return op_error(e),
    };
    if let Some(resp) = agent_check(&state, &headers, None, "manifest", &parsed.space, 0) {
        return resp;
    }
    let mut payload = Map::new();
    payload.insert("space".into(), json!(parsed.space));
    payload.insert("device".into(), json!(parsed.device));
    payload.insert("path".into(), json!(parsed.path));
    match state.store.handle(
        Frame::from_parts("manifest", payload),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    ) {
        Ok(data) => Json(json!({
            "uri": parsed.format(),
            "space": data.get("space").cloned().unwrap_or(json!(parsed.space)),
            "device": data.get("device").cloned().unwrap_or(json!(parsed.device)),
            "task": data.get("task").cloned().unwrap_or(json!(null)),
            "manifest": data.get("manifest").cloned().unwrap_or(json!(null)),
        }))
        .into_response(),
        Err(e) => op_error(e),
    }
}

async fn artifact_state_uri(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<VersionsQuery>,
) -> Response {
    if !authorize(&state.auth, &headers, None, auth::is_loopback(&addr.ip().to_string()), &["read", "admin"]) {
        return unauthorized();
    }
    let parsed = match uri::parse_with(&q.uri, &|sp| state.store.is_known_space(sp)) {
        Ok(u) => u,
        Err(e) => return op_error(e),
    };
    if let Some(resp) = agent_check(&state, &headers, None, "artifact.state", &parsed.space, 0) {
        return resp;
    }
    let mut payload = Map::new();
    payload.insert("uri".into(), json!(q.uri));
    match state.store.handle(
        Frame::from_parts("artifact.state", payload),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    ) {
        Ok(data) => Json(json!({"uri": data.get("uri").cloned().unwrap_or(json!(null)), "data": data}))
            .into_response(),
        Err(e) => op_error(e),
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    space: Option<String>,
    device: Option<String>,
    state: Option<String>,
    #[serde(default = "default_search_limit")]
    limit: usize,
    #[serde(default)]
    semantic: bool,
}

fn default_search_limit() -> usize {
    50
}

async fn search_uri(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<SearchQuery>,
) -> Response {
    if !authorize(&state.auth, &headers, None, auth::is_loopback(&addr.ip().to_string()), &["read", "admin"]) {
        return unauthorized();
    }
    if let Some(resp) = agent_check(
        &state,
        &headers,
        None,
        "search",
        q.space.as_deref().unwrap_or(""),
        0,
    ) {
        return resp;
    }
    match state.store.search_query(
        &q.q,
        q.space.as_deref(),
        q.device.as_deref(),
        q.state.as_deref(),
        q.limit,
        q.semantic,
    ) {
        Ok(out) => Json(Value::Object(out)).into_response(),
        Err(e) => op_error(e),
    }
}

#[derive(Deserialize)]
struct ReadQuery {
    uri: String,
    #[serde(default)]
    offset: i64,
    #[serde(default = "default_length")]
    length: usize,
}

fn default_length() -> usize {
    MAX_CHUNK
}

async fn read_uri(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<ReadQuery>,
) -> Response {
    if !authorize(&state.auth, &headers, None, auth::is_loopback(&addr.ip().to_string()), &["read", "admin"]) {
        return unauthorized();
    }
    let parsed = match uri::parse_with(&q.uri, &|sp| state.store.is_known_space(sp)) {
        Ok(u) => u,
        Err(e) => return op_error(e),
    };
    if parsed.path.is_empty() {
        return op_error(OpError::new("bad_path", "read requires file path"));
    }
    if let Some(resp) = agent_check(&state, &headers, None, "read", &parsed.space, 0) {
        return resp;
    }
    let length = if q.length == 0 || q.length > MAX_CHUNK {
        MAX_CHUNK
    } else {
        q.length
    };
    let full = match &parsed.ref_kind {
        RefKind::Latest => state
            .store
            .resolve(&parsed.space, &parsed.device, &parsed.path),
        _ => crate::store::versions::resolve(
            &state.store,
            &parsed.space,
            &parsed.device,
            &parsed.path,
            &parsed.ref_kind,
        ),
    };
    let full = match full {
        Ok(p) => p,
        Err(e) => return op_error(e),
    };
    let mut f = match std::fs::File::open(&full) {
        Ok(f) => f,
        Err(e) => return op_error(OpError::new("not_found", e.to_string())),
    };
    if f.seek(SeekFrom::Start(q.offset as u64)).is_err() {
        return (
            StatusCode::OK,
            [(header::CONTENT_LENGTH, "0")],
            Body::empty(),
        )
            .into_response();
    }
    let mut buf = vec![0u8; length];
    let n = f.read(&mut buf).unwrap_or(0);
    buf.truncate(n);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CONTENT_LENGTH, &n.to_string()),
        ],
        Body::from(buf),
    )
        .into_response()
}

#[derive(Deserialize)]
struct ListQuery {
    uri: Option<String>,
    space: Option<String>,
    device: Option<String>,
    path: Option<String>,
}

async fn list_uri(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    if !authorize(&state.auth, &headers, None, auth::is_loopback(&addr.ip().to_string()), &["read", "admin"]) {
        return unauthorized();
    }
    let parsed = if let Some(uri) = q.uri {
        match uri::parse_with(&uri, &|sp| state.store.is_known_space(sp)) {
            Ok(u) => u,
            Err(e) => return op_error(e),
        }
    } else {
        let space = q.space.unwrap_or_else(|| "files".into());
        let device = q.device.unwrap_or_else(|| state.device.clone());
        let path = q.path.unwrap_or_default();
        StoreUri {
            space,
            device,
            path,
            ref_kind: RefKind::Latest,
        }
    };
    if let Some(resp) = agent_check(&state, &headers, None, "list", &parsed.space, 0) {
        return resp;
    }
    match state.store.handle(
        parsed.to_frame_list(),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    ) {
        Ok(data) => Json(json!({
            "uri": parsed.format(),
            "space": parsed.space,
            "device": parsed.device,
            "path": parsed.path,
            "entries": data.get("entries").cloned().unwrap_or(json!([])),
        }))
        .into_response(),
        Err(e) => op_error(e),
    }
}

#[derive(Deserialize)]
struct StoreBody {
    op: String,
    #[serde(default)]
    payload: Map<String, Value>,
}

async fn store_op(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<StoreBody>,
) -> Response {
    let loopback = auth::is_loopback(&addr.ip().to_string());
    if !authorize(&state.auth, &headers, None, loopback, &["write", "admin"]) {
        return unauthorized();
    }
    let scopes = state
        .auth
        .resolve_scopes(&headers, None, loopback)
        .unwrap_or_default();
    if crate::auth_tokens::is_admin_only_op(&body.op)
        && !crate::auth_tokens::scopes_allow(&scopes, &["admin"])
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"op":"error","code":"forbidden","message":"admin scope required"})),
        )
            .into_response();
    }
    let space = body
        .payload
        .get("space")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let write_size = body
        .payload
        .get("size")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if let Some(resp) = agent_check(
        &state,
        &headers,
        None,
        &body.op,
        space,
        if body.op == "write.begin" { write_size } else { 0 },
    ) {
        return resp;
    }
    match state.store.handle(
        Frame::from_parts(body.op.clone(), body.payload),
        &state.device,
        protocol::TRUST_OWNER,
        loopback,
    ) {
        Ok(data) => {
            if let Some(agent) = state.auth.resolve_agent_id(&headers, None) {
                if let Some(bytes) = data
                    .get("files")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|f| f.get("size").and_then(|s| s.as_u64()))
                            .sum::<u64>()
                    })
                {
                    state.agents.record_usage(&agent, bytes);
                }
            }
            Json(json!({"op": "result", "data": data})).into_response()
        }
        Err(e) => Json(json!({
            "op": "error",
            "code": e.code,
            "message": e.msg,
        }))
        .into_response(),
    }
}

#[derive(Deserialize)]
struct EventsQuery {
    token: Option<String>,
    since: Option<u64>,
}

async fn events_sse(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<EventsQuery>,
) -> Response {
    let loopback = auth::is_loopback(&addr.ip().to_string());
    if !authorize(&state.auth, &headers, q.token.as_deref(), loopback, &["events", "read", "admin"]) {
        return unauthorized();
    }

    let snapshot: futures_util::stream::BoxStream<'static, Result<Event, Infallible>> = match q.since {
        Some(since) => Box::pin(stream::iter(
            state
                .events
                .replay(since)
                .into_iter()
                .map(|ev| sse_event("snapshot", &ev)),
        )),
        None => Box::pin(stream::iter(
            state
                .events
                .recent()
                .into_iter()
                .map(|ev| sse_event("snapshot", &ev)),
        )),
    };
    let rx = state.events.subscribe();
    let live = stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(ev) => return Some((sse_event("store", &ev), rx)),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return None,
            }
        }
    });

    Sse::new(snapshot.chain(live))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

fn sse_event(name: &str, ev: &StoreEvent) -> Result<Event, Infallible> {
    let data = serde_json::to_string(ev).unwrap_or_else(|_| "{}".into());
    Ok(Event::default().event(name).data(data))
}

#[derive(Deserialize)]
struct RecentQuery {
    #[serde(default = "default_recent_limit")]
    limit: usize,
    token: Option<String>,
}

fn default_recent_limit() -> usize {
    100
}

async fn events_recent(
    State(state): State<Arc<ApiState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<RecentQuery>,
) -> Response {
    let loopback = auth::is_loopback(&addr.ip().to_string());
    if !authorize(&state.auth, &headers, q.token.as_deref(), loopback, &["events", "read", "admin"]) {
        return unauthorized();
    }
    let mut events = state.events.recent();
    if events.len() > q.limit {
        let skip = events.len() - q.limit;
        events = events.split_off(skip);
    }
    Json(json!({"events": events})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventBus;
    use axum::Router;
    use base64::Engine as _;
    use sha2::Digest;
    use std::sync::Arc;

    const DEV: &str = "aaaaaaaaaaaaaaaa";

    async fn spawn_api() -> (String, Arc<AgentRegistry>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Local::open(dir.path(), DEV).unwrap());
        let events = EventBus::new();
        store.set_event_bus(Arc::clone(&events));
        let auth = crate::AuthConfig::new("", None);
        let agents = Arc::new(AgentRegistry::open(dir.path()));
        let state = Arc::new(ApiState {
            store,
            auth,
            device: DEV.to_string(),
            events,
            mdns: false,
            agents: Arc::clone(&agents),
        });
        let app = Router::new().nest("/api/v1", router(state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        (format!("http://{addr}"), agents)
    }

    fn commit_text(client: &crate::sdk::Client, space: &str, rel: &str, content: &str) {
        use sha2::{Digest, Sha256};
        let sha = hex::encode(Sha256::digest(content.as_bytes()));
        let mut begin = Map::new();
        begin.insert("space".into(), json!(space));
        begin.insert("path".into(), json!(rel));
        begin.insert("size".into(), json!(content.len() as i64));
        begin.insert("sha256".into(), json!(sha));
        let out = client.store_op("write.begin", begin).unwrap();
        let upload_id = out["upload_id"].as_str().unwrap().to_string();
        let mut chunk = Map::new();
        chunk.insert("upload_id".into(), json!(upload_id));
        chunk.insert("offset".into(), json!(0));
        chunk.insert("data".into(), json!(
            base64::engine::general_purpose::STANDARD.encode(content.as_bytes())
        ));
        client.store_op("write.chunk", chunk).unwrap();
        let mut commit = Map::new();
        commit.insert("space".into(), json!(space));
        commit.insert("upload_ids".into(), json!([upload_id]));
        client.store_op("commit", commit).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn versions_api_flow() {
        let (base, _agents) = spawn_api().await;
        let client = crate::sdk::Client::new(&base, "");
        let uri = format!("store://artifacts/{DEV}/t-9/out.txt");

        commit_text(&client, "artifacts", "t-9/out.txt", "first");
        commit_text(&client, "artifacts", "t-9/out.txt", "second");

        // /api/v1/versions lists both commits.
        let url = format!(
            "{base}/api/v1/versions?uri={}",
            url::form_urlencoded::byte_serialize(uri.as_bytes()).collect::<String>()
        );
        let versions = ureq::get(&url).call().unwrap().into_string().unwrap();
        let versions: Value = serde_json::from_str(&versions).unwrap();
        let arr = versions["versions"].as_array().unwrap();
        assert_eq!(arr.len(), 2, "{versions}");

        // /api/v1/read with @v1 returns the old content.
        let old = client
            .read_bytes(&format!("{uri}@v1"), 0, 64)
            .expect("read @v1");
        assert_eq!(String::from_utf8(old).unwrap(), "first");

        // /api/v1/uri/resolve with a hash ref reports the versioned meta.
        let sha1 = hex::encode(sha2::Sha256::digest(b"first"));
        let meta = client
            .resolve(&format!("{uri}@{}", &sha1[..16]))
            .unwrap();
        assert_eq!(meta["meta"]["size"], 5, "{meta}");
        assert_eq!(meta["meta"]["sha256"].as_str().unwrap(), sha1);

        // Unknown version -> not_found.
        let err = client.read_bytes(&format!("{uri}@v99"), 0, 64).unwrap_err();
        assert!(err.contains("not_found"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn agent_scope_enforcement() {
        let (base, agents) = spawn_api().await;

        // Unregistered agent -> acl_denied.
        let c = crate::sdk::Client::new(&base, "").with_agent("a-0000");
        let mut begin = Map::new();
        begin.insert("space".into(), json!("artifacts"));
        begin.insert("path".into(), json!("t/x.txt"));
        begin.insert("size".into(), json!(4));
        begin.insert("sha256".into(), json!("a".repeat(64)));
        let err = c.store_op("write.begin", begin).unwrap_err();
        assert!(err.contains("acl_denied"), "{err}");

        // Read-only scope cannot write.
        let ro = agents
            .create("ro", vec!["store:read".into()], 1024 * 1024)
            .unwrap();
        let c = crate::sdk::Client::new(&base, "").with_agent(&ro.id);
        let mut begin = Map::new();
        begin.insert("space".into(), json!("artifacts"));
        begin.insert("path".into(), json!("t/x.txt"));
        begin.insert("size".into(), json!(4));
        begin.insert("sha256".into(), json!("a".repeat(64)));
        let err = c.store_op("write.begin", begin).unwrap_err();
        assert!(err.contains("acl_denied"), "{err}");
        // Read is allowed.
        assert!(c.list("store://artifacts/aaaaaaaaaaaaaaaa/").is_ok());

        // Space-scoped write: artifacts ok, files denied.
        let w = agents
            .create("aw", vec!["store:write:artifacts".into()], 1024 * 1024)
            .unwrap();
        let c = crate::sdk::Client::new(&base, "").with_agent(&w.id);
        let mut begin = Map::new();
        begin.insert("space".into(), json!("artifacts"));
        begin.insert("path".into(), json!("t/x.txt"));
        begin.insert("size".into(), json!(4));
        begin.insert("sha256".into(), json!("a".repeat(64)));
        assert!(c.store_op("write.begin", begin).is_ok());
        let mut begin = Map::new();
        begin.insert("space".into(), json!("files"));
        begin.insert("path".into(), json!("t/x.txt"));
        begin.insert("size".into(), json!(4));
        begin.insert("sha256".into(), json!("a".repeat(64)));
        let err = c.store_op("write.begin", begin).unwrap_err();
        assert!(err.contains("acl_denied"), "{err}");

        // Quota exceeded.
        let q = agents.create("q", vec!["store:write".into()], 10).unwrap();
        let c = crate::sdk::Client::new(&base, "").with_agent(&q.id);
        let mut begin = Map::new();
        begin.insert("space".into(), json!("artifacts"));
        begin.insert("path".into(), json!("t/big.txt"));
        begin.insert("size".into(), json!(1024));
        begin.insert("sha256".into(), json!("b".repeat(64)));
        let err = c.store_op("write.begin", begin).unwrap_err();
        assert!(err.contains("quota_exceeded"), "{err}");

        // Delete requires store:delete.
        let wd = agents
            .create("wd", vec!["store:write".into()], 1024 * 1024)
            .unwrap();
        let c = crate::sdk::Client::new(&base, "").with_agent(&wd.id);
        let mut del = Map::new();
        del.insert("space".into(), json!("artifacts"));
        del.insert("path".into(), json!("t/x.txt"));
        let err = c.store_op("delete", del).unwrap_err();
        assert!(err.contains("acl_denied"), "{err}");

        // Revoked agent denied even for reads.
        agents.revoke(&ro.id).unwrap();
        let c = crate::sdk::Client::new(&base, "").with_agent(&ro.id);
        let err = c.list("store://artifacts/aaaaaaaaaaaaaaaa/").unwrap_err();
        assert!(err.contains("acl_denied"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn search_index_flow() {
        let (base, _agents) = spawn_api().await;
        let client = crate::sdk::Client::new(&base, "");
        commit_text(&client, "artifacts", "t-search/notes.md", "the quick brown fox jumps");

        let out = client
            .search("fox", Some("artifacts"), None, None, 10)
            .unwrap();
        assert_eq!(out["total"], 1, "{out}");
        assert_eq!(out["results"][0]["path"], "t-search/notes.md");
        assert!(
            out["results"][0]["snippet"]
                .as_str()
                .unwrap()
                .contains("fox"),
            "{out}"
        );

        // Space filter excludes other spaces.
        let out = client.search("fox", Some("files"), None, None, 10).unwrap();
        assert_eq!(out["total"], 0);

        // Delete removes from the index.
        let mut del = Map::new();
        del.insert("space".into(), json!("artifacts"));
        del.insert("path".into(), json!("t-search/notes.md"));
        client.store_op("delete", del).unwrap();
        let out = client.search("fox", None, None, None, 10).unwrap();
        assert_eq!(out["total"], 0, "{out}");
    }
}
