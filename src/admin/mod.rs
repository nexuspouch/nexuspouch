pub mod auth;
pub mod handler;
pub mod sessions;
pub mod sessions_ui;
pub mod ui;

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use handler::AdminState;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub fn router(state: Arc<AdminState>) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/stats", get(stats))
        .route("/recycle", get(recycle))
        .route("/recycle/empty", post(recycle_empty))
        .route("/recycle/restore", post(recycle_restore))
        .route("/import/pending", get(import_pending))
        .route("/import/grant", post(import_grant))
        .route("/import/reject", post(import_reject))
        .route("/import/grants", get(import_grants))
        .route("/pairing/start", post(pairing_start))
        .route("/pairing/pending", get(pairing_pending))
        .route("/pairing/decide", post(pairing_decide))
        .route("/peers", get(peers))
        .route("/peers/remove", post(peer_remove))
        .route("/gc", post(gc))
        .route("/master/migrate", post(master_migrate))
        .route("/browse", get(browse))
        .route("/browse/delete", post(browse_delete))
        .route("/devices/purge", post(device_purge))
        .route("/devices/wipe-self", post(device_wipe_self))
        .route("/discovery", get(discovery))
        .route("/diagnostics", get(diagnostics))
        .route("/audit", get(audit))
        .route("/tokens", get(tokens_list))
        .route("/tokens", post(tokens_create))
        .route("/tokens/revoke", post(tokens_revoke))
        .route("/agents", get(agents_list))
        .route("/agents", post(agents_create))
        .route("/agents/revoke", post(agents_revoke))
        .route("/agents/reset-quota", post(agents_reset))
        .route("/index/rebuild", post(index_rebuild))
        .route("/spaces", get(spaces_list))
        .route("/spaces", post(spaces_declare))
        .route("/bindings", get(bindings_list))
        .route("/bindings/sync", post(bindings_sync))
        .route("/reprotect", post(reprotect))
        .route("/versions", get(versions_overview))
        .route("/sessions/overview", get(sessions_overview))
        .route("/sessions/detail", get(sessions_detail))
        .route("/sessions/search", get(sessions_search))
        .with_state(state.clone());

    Router::new()
        .route("/admin", get(ui_page))
        .route("/admin/", get(ui_page))
        .route("/admin/sessions", get(sessions_page))
        .route("/admin/sessions/", get(sessions_page))
        .nest("/admin/api", api)
        .with_state(state)
}

async fn ui_page(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    if !state.auth.authorize_headers(&headers, None) {
        return unauthorized();
    }
    Html(ui::HTML).into_response()
}

async fn sessions_page(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    if !state.auth.authorize_headers(&headers, None) {
        return unauthorized();
    }
    Html(sessions_ui::HTML).into_response()
}

async fn health(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    if !state.auth.authorize_headers(&headers, None) {
        return unauthorized();
    }
    Json(json!({
        "ok": true,
        "device": state.device,
        "protocol": crate::protocol::PROTOCOL_VERSION,
        "admin": true,
    }))
    .into_response()
}

async fn stats(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, |s| handler::stats(s)).await
}

async fn recycle(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, |s| handler::recycle_list(s)).await
}

async fn recycle_empty(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, |s| handler::recycle_empty(s)).await
}

#[derive(Deserialize)]
struct RecycleRestoreBody {
    recycle_path: String,
}

async fn recycle_restore(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<RecycleRestoreBody>,
) -> Response {
    auth_json(state, headers, None, |s| handler::recycle_restore(s, body.recycle_path)).await
}

async fn import_pending(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, |s| handler::import_pending(s)).await
}

#[derive(Deserialize)]
struct ImportGrantBody {
    request_id: String,
}

async fn import_grant(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<ImportGrantBody>,
) -> Response {
    auth_json(state, headers, None, |s| handler::import_grant(s, body.request_id)).await
}

#[derive(Deserialize)]
struct ImportRejectBody {
    request_id: String,
}

async fn import_reject(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<ImportRejectBody>,
) -> Response {
    auth_json(state, headers, None, |s| handler::import_reject(s, body.request_id)).await
}

#[derive(Deserialize)]
struct GrantsQuery {
    role: Option<String>,
}

async fn import_grants(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Query(q): Query<GrantsQuery>,
) -> Response {
    auth_json(state, headers, None, |s| handler::import_grants(s, q.role)).await
}

async fn pairing_start(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, |s| handler::pairing_start(s)).await
}

async fn pairing_pending(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, |s| handler::pairing_pending(s)).await
}

#[derive(Deserialize)]
struct PairingDecideBody {
    accept: bool,
}

async fn pairing_decide(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<PairingDecideBody>,
) -> Response {
    if !state.auth.authorize_headers(&headers, None) {
        return unauthorized();
    }
    match handler::pairing_decide(&state, body.accept) {
        Ok(v) => Json(v).into_response(),
        Err(e) => bad_request(e),
    }
}

async fn peers(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, |s| handler::peers(s)).await
}

#[derive(Deserialize)]
struct PeerRemoveBody {
    fingerprint: String,
}

async fn peer_remove(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<PeerRemoveBody>,
) -> Response {
    auth_json(state, headers, None, |s| handler::peer_remove(s, body.fingerprint)).await
}

async fn gc(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, |s| handler::gc(s)).await
}

async fn master_migrate(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, |s| handler::master_migrate(s)).await
}

#[derive(Deserialize)]
struct BrowseQuery {
    device: Option<String>,
    space: Option<String>,
    path: Option<String>,
}

async fn browse(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Query(q): Query<BrowseQuery>,
) -> Response {
    auth_json(state, headers, None, |s| {
        handler::browse(s, q.device, q.space, q.path)
    })
    .await
}

#[derive(Deserialize)]
struct BrowseDeleteBody {
    device: Option<String>,
    space: String,
    path: String,
}

async fn browse_delete(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<BrowseDeleteBody>,
) -> Response {
    auth_json(state, headers, None, |s| {
        handler::browse_delete(s, body.device, body.space, body.path)
    })
    .await
}

#[derive(Deserialize)]
struct DevicePurgeBody {
    device_id: String,
}

async fn device_purge(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<DevicePurgeBody>,
) -> Response {
    auth_json(state, headers, None, |s| handler::device_purge(s, body.device_id)).await
}

#[derive(Deserialize)]
struct DeviceWipeBody {
    confirm: String,
}

async fn device_wipe_self(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<DeviceWipeBody>,
) -> Response {
    auth_json(state, headers, None, |s| handler::device_wipe_self(s, &body.confirm)).await
}

async fn discovery(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, |s| handler::discovery_browse(s)).await
}

async fn diagnostics(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    if !state.auth.authorize_headers(&headers, None) {
        return unauthorized();
    }
    let state = state.clone();
    let report = tokio::task::spawn_blocking(move || handler::diagnostics(&state))
        .await
        .unwrap_or_default();
    Json(Value::Object(report)).into_response()
}

#[derive(Deserialize)]
struct AuditQuery {
    limit: Option<usize>,
}

async fn audit(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Query(q): Query<AuditQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(50).min(200);
    auth_json(state, headers, None, |s| handler::audit_list(s, limit)).await
}

async fn tokens_list(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, |s| handler::tokens_list(s)).await
}

#[derive(Deserialize)]
struct TokenCreateBody {
    label: Option<String>,
    scopes: Option<Vec<String>>,
    agent_id: Option<String>,
}

async fn tokens_create(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<TokenCreateBody>,
) -> Response {
    let label = body.label.unwrap_or_else(|| "api".into());
    let scopes = body.scopes.unwrap_or_else(|| vec!["read".into()]);
    auth_json(state, headers, None, move |s| {
        handler::tokens_create(s, label, scopes, body.agent_id)
    })
    .await
}

#[derive(Deserialize)]
struct TokenRevokeBody {
    id: String,
}

async fn tokens_revoke(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<TokenRevokeBody>,
) -> Response {
    auth_json(state, headers, None, move |s| handler::tokens_revoke(s, body.id)).await
}

#[derive(Deserialize)]
struct AgentCreateBody {
    name: String,
    scopes: Vec<String>,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
}

fn default_max_bytes() -> u64 {
    1024 * 1024 * 1024
}

async fn agents_list(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, handler::agents_list).await
}

async fn agents_create(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<AgentCreateBody>,
) -> Response {
    auth_json(state, headers, None, move |s| {
        handler::agents_create(s, body.name, body.scopes, body.max_bytes)
    })
    .await
}

#[derive(Deserialize)]
struct AgentIdBody {
    id: String,
}

async fn agents_revoke(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<AgentIdBody>,
) -> Response {
    auth_json(state, headers, None, move |s| handler::agents_revoke(s, body.id)).await
}

async fn agents_reset(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<AgentIdBody>,
) -> Response {
    auth_json(state, headers, None, move |s| handler::agents_reset_quota(s, body.id)).await
}

async fn index_rebuild(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, handler::index_rebuild).await
}

async fn spaces_list(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, handler::spaces_list).await
}

#[derive(Deserialize)]
struct SpaceDeclareBody {
    name: String,
    #[serde(default = "default_space_visibility")]
    visibility: String,
    encryption: Option<String>,
    retention: Option<String>,
    import_grant: Option<String>,
}

fn default_space_visibility() -> String {
    "shared".into()
}

async fn spaces_declare(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(body): Json<SpaceDeclareBody>,
) -> Response {
    auth_json(state, headers, None, move |s| {
        handler::spaces_declare(
            s,
            body.name,
            body.visibility,
            body.encryption,
            body.retention,
            body.import_grant,
        )
    })
    .await
}

async fn versions_overview(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, handler::versions_overview).await
}

#[derive(Deserialize)]
struct SessionsOverviewQuery {
    device: Option<String>,
    limit: Option<usize>,
}

async fn sessions_overview(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Query(q): Query<SessionsOverviewQuery>,
) -> Response {
    auth_json_blocking(state, headers, move |s| {
        sessions::overview(
            &s.store,
            q.device.as_deref(),
            q.limit.unwrap_or(0),
            &s.sessions_cache,
        )
    })
    .await
}

#[derive(Deserialize)]
struct SessionsDetailQuery {
    uri: String,
}

async fn sessions_detail(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Query(q): Query<SessionsDetailQuery>,
) -> Response {
    auth_json_blocking(state, headers, move |s| sessions::detail(&s.store, &q.uri)).await
}

#[derive(Deserialize)]
struct SessionsSearchQuery {
    q: String,
    device: Option<String>,
    limit: Option<usize>,
    #[serde(default)]
    semantic: bool,
    agent: Option<String>,
    project: Option<String>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    rerank: Option<bool>,
    dedup: Option<bool>,
    context_turns: Option<usize>,
}

async fn sessions_search(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Query(q): Query<SessionsSearchQuery>,
) -> Response {
    auth_json_blocking(state, headers, move |s| {
        let filter = crate::store::SearchFilter {
            agent: q.agent.filter(|s| !s.is_empty()),
            project: q.project.filter(|s| !s.is_empty()),
            since_ms: q.since_ms,
            until_ms: q.until_ms,
        };
        let filter = (!filter.is_empty()).then_some(filter);
        let opts = crate::store::SearchOptions {
            rerank: q.rerank,
            dedup: q.dedup,
            context_turns: q.context_turns,
        };
        sessions::search(
            &s.store,
            &q.q,
            q.device.as_deref(),
            q.limit.unwrap_or(0),
            q.semantic,
            filter.as_ref(),
            &opts,
        )
    })
    .await
}

async fn bindings_list(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, handler::bindings_list).await
}

async fn bindings_sync(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    auth_json(state, headers, None, handler::bindings_sync).await
}

#[derive(Deserialize, Default)]
struct ReprotectBody {
    password: Option<String>,
}

async fn reprotect(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    body: Result<Json<ReprotectBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let password = body.ok().and_then(|b| b.0.password);
    auth_json(state, headers, None, move |s| {
        handler::reprotect_run(s, password)
    })
    .await
}

async fn auth_json<F>(
    state: Arc<AdminState>,
    headers: HeaderMap,
    token: Option<String>,
    f: F,
) -> Response
where
    F: FnOnce(&AdminState) -> Result<Map<String, Value>, crate::store::OpError>,
{
    if !state.auth.authorize_headers(&headers, token.as_deref()) {
        return unauthorized();
    }
    match f(&state) {
        Ok(v) => Json(Value::Object(v)).into_response(),
        Err(e) => bad_request(e.msg),
    }
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("WWW-Authenticate", r#"Bearer realm="shepaw-admin""#)],
        "unauthorized",
    )
        .into_response()
}

/// `auth_json` for tree-scanning handlers: run the closure off the async
/// worker so peer ws / /store are not blocked (same pattern as `diagnostics`).
async fn auth_json_blocking<F>(
    state: Arc<AdminState>,
    headers: HeaderMap,
    f: F,
) -> Response
where
    F: FnOnce(&AdminState) -> Result<Map<String, Value>, crate::store::OpError>
        + Send
        + 'static,
{
    if !state.auth.authorize_headers(&headers, None) {
        return unauthorized();
    }
    let state2 = Arc::clone(&state);
    match tokio::task::spawn_blocking(move || f(&state2)).await {
        Ok(Ok(v)) => Json(Value::Object(v)).into_response(),
        Ok(Err(e)) => bad_request(e.msg),
        Err(e) => bad_request(format!("task join: {e}")),
    }
}

fn bad_request(msg: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": "internal", "message": msg.into()})),
    )
        .into_response()
}

pub fn store_handler(
    state: Arc<AdminState>,
    headers: HeaderMap,
    remote_loopback: bool,
    op: String,
    payload: Map<String, Value>,
) -> Result<Map<String, Value>, crate::store::OpError> {
    if !state.auth.authorize_headers(&headers, None) && !remote_loopback {
        return Err(crate::store::OpError::new("unauthorized", "unauthorized"));
    }
    state.store.handle(
        crate::protocol::Frame::from_parts(op, payload),
        &state.device,
        crate::protocol::TRUST_OWNER,
        remote_loopback,
    )
}
