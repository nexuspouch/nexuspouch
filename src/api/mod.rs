use crate::admin::auth::{self, AuthConfig};
use crate::events::{EventBus, StoreEvent};
use crate::protocol::{self, Frame};
use crate::store::{Local, OpError, MAX_CHUNK};
use crate::uri::{self, StoreUri};
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
) -> bool {
    if auth.token.is_empty() {
        return loopback;
    }
    auth.authorize_headers(headers, query_token)
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
    if !authorize(&state.auth, &headers, None, auth::is_loopback(&addr.ip().to_string())) {
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
    if !authorize(&state.auth, &headers, None, auth::is_loopback(&addr.ip().to_string())) {
        return unauthorized();
    }
    let parsed = match uri::parse(&q.uri) {
        Ok(u) => u,
        Err(e) => return op_error(e),
    };
    match state.store.handle(
        parsed.to_frame_meta(),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    ) {
        Ok(meta) => Json(json!({
            "uri": parsed.format(),
            "space": parsed.space,
            "device": parsed.device,
            "path": parsed.path,
            "meta": meta,
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
    if !authorize(&state.auth, &headers, None, auth::is_loopback(&addr.ip().to_string())) {
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
    let full = match state
        .store
        .resolve(&parsed.space, &parsed.device, &parsed.path)
    {
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
    if !authorize(&state.auth, &headers, None, auth::is_loopback(&addr.ip().to_string())) {
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
    if !authorize(&state.auth, &headers, None, loopback) {
        return unauthorized();
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
    if !authorize(&state.auth, &headers, q.token.as_deref(), loopback) {
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
    if !authorize(&state.auth, &headers, q.token.as_deref(), loopback) {
        return unauthorized();
    }
    let mut events = state.events.recent();
    if events.len() > q.limit {
        let skip = events.len() - q.limit;
        events = events.split_off(skip);
    }
    Json(json!({"events": events})).into_response()
}
