use crate::peer::{encode_qr, PairingHub, PeerStore, SessionRegistry};
use crate::protocol::{self, Frame};
use crate::store::{gc, Local, OpError};
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub struct AdminState {
    pub store: Arc<Local>,
    pub auth: super::auth::AuthConfig,
    pub device: String,
    pub hub: Option<Arc<PairingHub>>,
    pub peers: Option<Arc<PeerStore>>,
    pub sessions: Option<Arc<SessionRegistry>>,
    pub identity: Option<Arc<crate::noise::Identity>>,
    pub listen: String,
    pub channel_endpoint: String,
    pub discovery: Option<Arc<crate::discovery::Discovery>>,
    pub local_http: String,
    pub listen_port: u16,
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
    let mut out = Map::from_iter([
        ("code".into(), json!(code)),
        ("qr".into(), json!(qr)),
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
    Ok(data)
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
