use crate::admin::auth::{self, AuthConfig};
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
}

pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/uri/resolve", get(uri_resolve))
        .route("/read", get(read_uri))
        .route("/list", get(list_uri))
        .route("/versions", get(versions_uri))
        .route("/manifest", get(manifest_uri))
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
    let parsed = match uri::parse(&q.uri) {
        Ok(u) => u,
        Err(e) => return op_error(e),
    };
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
    let parsed = match uri::parse(&q.uri) {
        Ok(u) => u,
        Err(e) => return op_error(e),
    };
    if parsed.path.is_empty() {
        return op_error(OpError::new("bad_path", "versions require a file path"));
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
    let parsed = match uri::parse(&q.uri) {
        Ok(u) => u,
        Err(e) => return op_error(e),
    };
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
    let parsed = match uri::parse(&q.uri) {
        Ok(u) => u,
        Err(e) => return op_error(e),
    };
    if parsed.path.is_empty() {
        return op_error(OpError::new("bad_path", "read requires file path"));
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
        match uri::parse(&uri) {
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
    match state.store.handle(
        Frame::from_parts(body.op, body.payload),
        &state.device,
        protocol::TRUST_OWNER,
        loopback,
    ) {
        Ok(data) => Json(json!({"op": "result", "data": data})).into_response(),
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

    let recent = state.events.recent();
    let rx = state.events.subscribe();

    let snapshot = stream::iter(recent.into_iter().map(|ev| sse_event("snapshot", &ev)));
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

    async fn spawn_api() -> String {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Local::open(dir.path(), DEV).unwrap());
        let events = EventBus::new();
        store.set_event_bus(Arc::clone(&events));
        let auth = crate::AuthConfig::new("", None);
        let state = Arc::new(ApiState {
            store,
            auth,
            device: DEV.to_string(),
            events,
            mdns: false,
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
        format!("http://{addr}")
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
        let base = spawn_api().await;
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
}
