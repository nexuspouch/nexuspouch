use crate::peer::{encode_qr, encode_qr_svg, PairingHub, PeerStore, SessionRegistry};
use crate::protocol::{self, Frame};
use crate::store::{gc, Local, OpError};
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub struct AdminState {
    pub store: Arc<Local>,
    pub auth: super::auth::AuthConfig,
    pub device: String,
    pub agents: Arc<crate::agents::AgentRegistry>,
    pub hub: Option<Arc<PairingHub>>,
    pub peers: Option<Arc<PeerStore>>,
    pub sessions: Option<Arc<SessionRegistry>>,
    pub identity: Option<Arc<crate::noise::Identity>>,
    pub listen: String,
    pub channel_endpoint: String,
    pub discovery: Option<Arc<crate::discovery::Discovery>>,
    pub local_http: String,
    pub listen_port: u16,
    pub sessions_cache: Arc<super::sessions::SummaryCache>,
}

pub fn stats(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    let mut data = state.store.handle(
        Frame::from_parts("stats", Map::new()),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    )?;
    data.insert("self_device".into(), json!(state.device));
    Ok(data)
}

/// Version retention policy + published (protected) artifact inventory.
pub fn versions_overview(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    let published = crate::store::versions::list_published(&state.store);
    Ok(Map::from_iter([
        (
            "keep_last".into(),
            json!(crate::store::versions::keep_last_policy()),
        ),
        ("keep_env".into(), json!("NEXUSPOUCH_VERSIONS_KEEP")),
        ("published_count".into(), json!(published.len())),
        ("published".into(), json!(published)),
    ]))
}

pub fn recycle_list(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    state.store.handle(
        Frame::from_parts("recycle.list", Map::new()),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    )
}

pub fn recycle_empty(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    state.store.handle(
        Frame::from_parts("recycle.empty", Map::new()),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    )
}

pub fn recycle_restore(state: &AdminState, recycle_path: String) -> Result<Map<String, Value>, OpError> {
    let mut payload = Map::new();
    payload.insert("recycle_path".into(), json!(recycle_path));
    state.store.handle(
        Frame::from_parts("recycle.restore", payload),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    )
}

pub fn import_pending(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    state.store.handle(
        Frame::from_parts("import.pending", Map::new()),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    )
}

pub fn import_grant(state: &AdminState, request_id: String) -> Result<Map<String, Value>, OpError> {
    let mut payload = Map::new();
    payload.insert("request_id".into(), json!(request_id));
    let mut data = state.store.handle(
        Frame::from_parts("import.grant", payload),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    )?;
    let pushed = if let Some(grant) = data.get("grant").and_then(|v| v.as_object()) {
        state
            .sessions
            .as_ref()
            .map(|s| s.push_import_grant(grant))
            .unwrap_or(false)
    } else {
        false
    };
    data.insert("pushed".into(), json!(pushed));
    Ok(data)
}

pub fn import_reject(state: &AdminState, request_id: String) -> Result<Map<String, Value>, OpError> {
    let mut payload = Map::new();
    payload.insert("request_id".into(), json!(request_id));
    state.store.handle(
        Frame::from_parts("import.reject", payload),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    )
}

pub fn import_grants(state: &AdminState, role: Option<String>) -> Result<Map<String, Value>, OpError> {
    let mut payload = Map::new();
    payload.insert(
        "role".into(),
        json!(role.unwrap_or_else(|| "issued".into())),
    );
    state.store.handle(
        Frame::from_parts("import.grants", payload),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    )
}

pub fn pairing_start(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    let hub = state
        .hub
        .as_ref()
        .ok_or_else(|| OpError::new("internal", "pairing unavailable"))?;
    let identity = state
        .identity
        .as_ref()
        .ok_or_else(|| OpError::new("internal", "pairing unavailable"))?;
    let code = hub
        .start()
        .map_err(|e| OpError::new("internal", e.to_string()))?;
    let local = crate::peer::advertise_local_ws(&state.listen);
    let channel = state.channel_endpoint.trim();
    let qr = encode_qr(
        &local,
        channel,
        &code,
        &identity.fingerprint(),
        &identity.public_key,
    );
    let qr_svg = encode_qr_svg(&qr).map_err(|e| OpError::new("internal", e))?;
    let mut out = Map::from_iter([
        ("code".into(), json!(code)),
        ("qr".into(), json!(qr)),
        ("qr_svg".into(), json!(qr_svg)),
        ("local_endpoint".into(), json!(local)),
        ("fingerprint".into(), json!(identity.fingerprint())),
    ]);
    if !channel.is_empty() {
        out.insert("channel_endpoint".into(), json!(channel));
    }
    Ok(out)
}

pub fn pairing_pending(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    let pending = state.hub.as_ref().and_then(|h| h.pending());
    Ok(Map::from_iter([("pending".into(), json!(pending))]))
}

pub fn pairing_decide(state: &AdminState, accept: bool) -> Result<Map<String, Value>, String> {
    let hub = state
        .hub
        .as_ref()
        .ok_or_else(|| "pairing unavailable".to_string())?;
    hub.decide(accept)?;
    Ok(Map::from_iter([
        ("ok".into(), json!(true)),
        ("accept".into(), json!(accept)),
    ]))
}

pub fn peers(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    let peers = state
        .peers
        .as_ref()
        .and_then(|p| p.list().ok())
        .unwrap_or_default();
    Ok(Map::from_iter([("peers".into(), json!(peers))]))
}

pub fn peer_remove(state: &AdminState, fingerprint: String) -> Result<Map<String, Value>, OpError> {
    let peers = state
        .peers
        .as_ref()
        .ok_or_else(|| OpError::new("internal", "peers unavailable"))?;
    let ok = peers
        .remove(&fingerprint)
        .map_err(|e| OpError::new("internal", e.to_string()))?;
    if !ok {
        return Ok(Map::from_iter([
            ("ok".into(), json!(false)),
            ("error".into(), json!("not_found")),
        ]));
    }
    Ok(Map::from_iter([
        ("ok".into(), json!(true)),
        ("fingerprint".into(), json!(fingerprint)),
    ]))
}

pub fn gc(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    let staging_removed = gc::gc_staging(&state.store, std::time::Duration::ZERO)?;
    let recycle_bytes = gc::gc_recycle(&state.store, std::time::Duration::ZERO)?;
    Ok(Map::from_iter([
        ("ok".into(), json!(true)),
        ("staging_removed".into(), json!(staging_removed)),
        ("recycle_bytes".into(), json!(recycle_bytes)),
    ]))
}

pub fn master_migrate(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    let mut data = state.store.handle(
        Frame::from_parts("master.migrate", Map::new()),
        &state.device,
        protocol::TRUST_OWNER,
        true,
    )?;
    if let (Some(master), Some(epoch)) = (
        data.get("master").and_then(|v| v.as_str()),
        data.get("epoch").and_then(|v| v.as_i64()),
    ) {
        let n = state
            .sessions
            .as_ref()
            .map(|s| s.fanout_master_pointer(master, epoch, &state.device))
            .unwrap_or(0);
        data.insert("broadcast_peers".into(), json!(n));
    }
    state
        .store
        .audit_action("migrate", &state.device, "master.migrate", "admin migrate");
    Ok(data)
}

pub fn audit_list(state: &AdminState, limit: usize) -> Result<Map<String, Value>, OpError> {
    Ok(state.store.audit().recent_json(limit))
}

pub fn tokens_list(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    let tokens = state
        .auth
        .tokens
        .as_ref()
        .map(|t| t.list_public())
        .unwrap_or_default();
    Ok(Map::from_iter([(
        "tokens".into(),
        Value::Array(tokens.into_iter().map(Value::Object).collect()),
    )]))
}

pub fn tokens_create(
    state: &AdminState,
    label: String,
    scopes: Vec<String>,
    agent_id: Option<String>,
) -> Result<Map<String, Value>, OpError> {
    let store = state
        .auth
        .tokens
        .as_ref()
        .ok_or_else(|| OpError::new("internal", "token store unavailable"))?;
    let t = store
        .create(&label, scopes, agent_id)
        .map_err(|e| OpError::new("bad_op", e))?;
    Ok(Map::from_iter([
        ("id".into(), json!(t.id)),
        ("token".into(), json!(t.token)),
        ("label".into(), json!(t.label)),
        ("scopes".into(), json!(t.scopes)),
        ("created_ms".into(), json!(t.created_ms)),
        ("agent_id".into(), json!(t.agent_id)),
    ]))
}

pub fn agents_list(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    Ok(Map::from_iter([(
        "agents".into(),
        Value::Array(
            state
                .agents
                .list_public()
                .into_iter()
                .map(Value::Object)
                .collect(),
        ),
    )]))
}

pub fn agents_create(
    state: &AdminState,
    name: String,
    scopes: Vec<String>,
    max_bytes: u64,
) -> Result<Map<String, Value>, OpError> {
    let a = state
        .agents
        .create(&name, scopes, max_bytes)
        .map_err(|e| OpError::new("bad_op", e))?;
    Ok(Map::from_iter([
        ("id".into(), json!(a.id)),
        ("name".into(), json!(a.name)),
        ("scopes".into(), json!(a.scopes)),
        ("status".into(), json!(a.status)),
        (
            "quota".into(),
            json!({"max_bytes": a.quota.max_bytes, "bytes_used": a.quota.bytes_used}),
        ),
    ]))
}

pub fn agents_revoke(state: &AdminState, id: String) -> Result<Map<String, Value>, OpError> {
    let ok = state
        .agents
        .revoke(&id)
        .map_err(|e| OpError::new("internal", e))?;
    Ok(Map::from_iter([("ok".into(), json!(ok)), ("id".into(), json!(id))]))
}

pub fn agents_reset_quota(state: &AdminState, id: String) -> Result<Map<String, Value>, OpError> {
    let ok = state
        .agents
        .reset_quota(&id)
        .map_err(|e| OpError::new("internal", e))?;
    Ok(Map::from_iter([("ok".into(), json!(ok)), ("id".into(), json!(id))]))
}

pub fn index_rebuild(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    let stale_before = state.store.vector_stale_count();
    let indexed = state.store.rebuild_index()?;
    Ok(Map::from_iter([
        ("indexed".into(), json!(indexed)),
        ("vectors".into(), json!(state.store.vector_total_count())),
        ("stale_before".into(), json!(stale_before)),
        ("embedder".into(), json!(state.store.embedder_name())),
        ("ok".into(), json!(true)),
    ]))
}

pub fn spaces_list(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    Ok(Map::from_iter([(
        "spaces".into(),
        state.store.list_spaces_json(),
    )]))
}

pub fn spaces_declare(
    state: &AdminState,
    name: String,
    visibility: String,
    encryption: Option<String>,
    retention: Option<String>,
    import_grant: Option<String>,
) -> Result<Map<String, Value>, OpError> {
    state.store.declare_space_profile(
        &name,
        &visibility,
        encryption.as_deref().unwrap_or("none"),
        retention.as_deref().unwrap_or("none"),
        import_grant.as_deref().unwrap_or("allowed"),
    )
}

pub fn bindings_list(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    let registry = crate::store::bindings::BindingsRegistry::open(&state.store.root);
    let bindings: Vec<Value> = registry
        .list()
        .iter()
        .map(|b| {
            json!({
                "id": b.id,
                "label": b.label,
                "external": b.external,
                "space": b.space,
                "folder": b.folder,
                "mode": b.mode,
                "ignore": b.ignore,
            })
        })
        .collect();
    Ok(Map::from_iter([("bindings".into(), Value::Array(bindings))]))
}

pub fn bindings_sync(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    let reports = crate::store::bindings::sync_all(&state.store);
    Ok(Map::from_iter([(
        "reports".into(),
        Value::Array(reports.iter().map(|r| r.to_json()).collect()),
    )]))
}

/// Add a directory binding and optionally sync it immediately.
pub fn bindings_add(
    state: &AdminState,
    external: String,
    space: String,
    folder: String,
    mode: Option<String>,
    label: Option<String>,
    sync_now: bool,
) -> Result<Map<String, Value>, OpError> {
    let external = crate::store::bindings::expand_external(&external);
    if external.is_empty() {
        return Err(OpError::new("bad_op", "external path required"));
    }
    let ext_path = std::path::Path::new(&external);
    if !ext_path.is_dir() {
        return Err(OpError::new(
            "bad_path",
            format!("external directory not found: {external}"),
        ));
    }
    let space = space.trim();
    let folder = folder.trim();
    if space.is_empty() || folder.is_empty() {
        return Err(OpError::new("bad_op", "space and folder required"));
    }
    let label = label
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("{folder}@admin"));
    let mode = mode.unwrap_or_else(|| "auto".into());
    let registry = crate::store::bindings::BindingsRegistry::open(&state.store.root);
    let binding = registry
        .add(&label, &external, space, folder, &mode, vec![])
        .map_err(|e| OpError::new("bad_op", e))?;
    let mut out = Map::from_iter([
        ("ok".into(), json!(true)),
        (
            "binding".into(),
            json!({
                "id": binding.id,
                "label": binding.label,
                "external": binding.external,
                "space": binding.space,
                "folder": binding.folder,
                "mode": binding.mode,
            }),
        ),
    ]);
    if sync_now {
        let report = crate::store::bindings::sync_binding(&state.store, &binding);
        out.insert("report".into(), report.to_json());
    }
    Ok(out)
}

pub fn bindings_remove(state: &AdminState, id: String) -> Result<Map<String, Value>, OpError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(OpError::new("bad_op", "id required"));
    }
    let registry = crate::store::bindings::BindingsRegistry::open(&state.store.root);
    let ok = registry
        .remove(id)
        .map_err(|e| OpError::new("internal", e))?;
    Ok(Map::from_iter([
        ("ok".into(), json!(ok)),
        ("id".into(), json!(id)),
    ]))
}

pub fn tokens_revoke(state: &AdminState, id: String) -> Result<Map<String, Value>, OpError> {
    let store = state
        .auth
        .tokens
        .as_ref()
        .ok_or_else(|| OpError::new("internal", "token store unavailable"))?;
    let ok = store
        .revoke(&id)
        .map_err(|e| OpError::new("internal", e))?;
    Ok(Map::from_iter([
        ("ok".into(), json!(ok)),
        ("id".into(), json!(id)),
    ]))
}

pub fn reprotect_run(
    state: &AdminState,
    password: Option<String>,
) -> Result<Map<String, Value>, OpError> {
    let opts = crate::store::reprotect::opts_from_env(password);
    let out = crate::store::reprotect::run(&state.store, opts)?;
    state
        .store
        .audit_action("reprotect", &state.device, "reprotect.run", "admin reprotect");
    Ok(out)
}

pub fn browse(
    state: &AdminState,
    device: Option<String>,
    space: Option<String>,
    path: Option<String>,
) -> Result<Map<String, Value>, OpError> {
    let device = device.unwrap_or_else(|| state.device.clone());
    let space = space.unwrap_or_else(|| "files".into());
    let path = path.unwrap_or_default();
    let mut data = crate::store::browse::admin_list(&state.store, &device, &space, &path)?;
    data.insert("device".into(), json!(device));
    data.insert("space".into(), json!(space));
    data.insert("path".into(), json!(path));
    Ok(data)
}

pub fn browse_delete(
    state: &AdminState,
    device: Option<String>,
    space: String,
    path: String,
) -> Result<Map<String, Value>, OpError> {
    let device = device.unwrap_or_else(|| state.device.clone());
    crate::store::browse::admin_delete(&state.store, &device, &space, &path)
}

pub fn device_purge(state: &AdminState, device_id: String) -> Result<Map<String, Value>, OpError> {
    let freed = crate::store::maintenance::purge_device(&state.store, &device_id, &state.device)?;
    state.store.audit_action(
        "purge",
        &state.device,
        "devices.purge",
        &format!("purged {device_id}"),
    );
    Ok(Map::from_iter([
        ("ok".into(), json!(true)),
        ("device_id".into(), json!(device_id)),
        ("purged_bytes".into(), json!(freed)),
    ]))
}

pub fn device_wipe_self(state: &AdminState, confirm: &str) -> Result<Map<String, Value>, OpError> {
    if confirm != "DELETE" {
        return Err(OpError::new("bad_op", r#"confirm must be "DELETE""#));
    }
    let freed = crate::store::maintenance::wipe_self(&state.store, &state.device)?;
    state
        .store
        .audit_action("wipe", &state.device, "devices.wipe-self", "wipe self");
    Ok(Map::from_iter([
        ("ok".into(), json!(true)),
        ("device_id".into(), json!(state.device)),
        ("freed_bytes".into(), json!(freed)),
    ]))
}

pub fn discovery_browse(state: &AdminState) -> Result<Map<String, Value>, OpError> {
    let discovery = state
        .discovery
        .as_ref()
        .ok_or_else(|| OpError::new("internal", "mDNS unavailable"))?;
    let peers = discovery.browse(
        std::time::Duration::from_millis(1500),
        Some(&state.device),
    );
    let count = peers.len();
    Ok(Map::from_iter([
        ("peers".into(), json!(peers)),
        ("count".into(), json!(count)),
    ]))
}

pub fn diagnostics(state: &AdminState) -> Map<String, Value> {
    let report = crate::discovery::diag::run_diagnostics(
        &state.listen,
        &state.channel_endpoint,
        state.discovery.as_ref(),
        &state.device,
    );
    report.to_map()
}
