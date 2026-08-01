use super::{io_err, Local, OpError};
use crate::protocol::{self, Frame};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const IMPORT_DEFAULT_TTL: Duration = Duration::from_secs(24 * 3600);
const GRANT_SPACES: &[&str] = &["backups", "attachments"];

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ImportRequest {
    request_id: String,
    old_device: String,
    new_device: String,
    requested_at: i64,
    status: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ImportGrant {
    pub grant_id: String,
    pub old_device: String,
    pub new_device: String,
    pub spaces: Vec<String>,
    pub issued_at: i64,
    pub expires_at: i64,
    pub revoked: bool,
}

impl ImportGrant {
    fn to_map(&self) -> Map<String, Value> {
        Map::from_iter([
            ("grant_id".into(), json!(self.grant_id)),
            ("old_device".into(), json!(self.old_device)),
            ("new_device".into(), json!(self.new_device)),
            ("spaces".into(), json!(self.spaces)),
            ("issued_at".into(), json!(self.issued_at)),
            ("expires_at".into(), json!(self.expires_at)),
            ("revoked".into(), json!(self.revoked)),
        ])
    }

    fn expired(&self, now: i64) -> bool {
        now > self.expires_at
    }
}

pub struct ImportAuth {
    root: PathBuf,
    mu: Mutex<()>,
}

impl ImportAuth {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            mu: Mutex::new(()),
        }
    }

    fn system_path(&self, name: &str) -> PathBuf {
        self.root.join(".system").join(name)
    }

    pub(crate) fn create_request(&self, old_device: &str, new_device: &str) -> Result<(ImportRequest, bool), OpError> {
        let _g = self.mu.lock().unwrap();
        let mut reqs = self.load_requests()?;
        for r in &reqs {
            if r.old_device == old_device && r.new_device == new_device && r.status == "pending" {
                return Ok((r.clone(), false));
            }
        }
        let req = ImportRequest {
            request_id: format!("ir-{}", random_id()),
            old_device: old_device.to_string(),
            new_device: new_device.to_string(),
            requested_at: now_ms(),
            status: "pending".into(),
        };
        reqs.push(req.clone());
        self.save_requests(&reqs)?;
        Ok((req, true))
    }

    pub(crate) fn pending_requests(&self) -> Result<Vec<ImportRequest>, OpError> {
        let _g = self.mu.lock().unwrap();
        Ok(self
            .load_requests()?
            .into_iter()
            .filter(|r| r.status == "pending")
            .collect())
    }

    pub fn grant(&self, request_id: &str) -> Result<ImportGrant, OpError> {
        let _g = self.mu.lock().unwrap();
        let mut reqs = self.load_requests()?;
        let pos = reqs
            .iter()
            .position(|r| r.request_id == request_id)
            .ok_or_else(|| OpError::new("not_found", "request not found"))?;
        if reqs[pos].status != "pending" {
            return Err(OpError::new(
                "bad_op",
                format!("request already {}", reqs[pos].status),
            ));
        }
        let now = now_ms();
        let g = ImportGrant {
            grant_id: format!("ig-{}", random_id()),
            old_device: reqs[pos].old_device.clone(),
            new_device: reqs[pos].new_device.clone(),
            spaces: GRANT_SPACES.iter().map(|s| s.to_string()).collect(),
            issued_at: now,
            expires_at: now + IMPORT_DEFAULT_TTL.as_millis() as i64,
            revoked: false,
        };
        let mut grants = self.load_grants()?;
        grants.push(g.clone());
        self.save_grants(&grants)?;
        reqs[pos].status = "granted".into();
        self.save_requests(&reqs)?;
        Ok(g)
    }

    pub fn reject(&self, request_id: &str) -> Result<(), OpError> {
        let _g = self.mu.lock().unwrap();
        let mut reqs = self.load_requests()?;
        for r in &mut reqs {
            if r.request_id == request_id {
                r.status = "rejected".into();
            }
        }
        self.save_requests(&reqs)
    }

    pub fn validate(&self, grant_id: &str, old_device: &str, new_device: &str, space: &str) -> Result<bool, OpError> {
        let _g = self.mu.lock().unwrap();
        let grants = self.load_grants()?;
        let now = now_ms();
        for g in grants {
            if g.grant_id != grant_id || g.old_device != old_device || g.new_device != new_device {
                continue;
            }
            if g.revoked || g.expired(now) {
                continue;
            }
            if g.spaces.iter().any(|s| s == space) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn save_received(&self, g: ImportGrant) -> Result<(), OpError> {
        let _g = self.mu.lock().unwrap();
        let mut received = self.load_received()?;
        received.retain(|x| x.grant_id != g.grant_id);
        received.push(g);
        self.save_json(&self.system_path("import_received.json"), &received)
    }

    pub fn issued_grants(&self) -> Result<Vec<ImportGrant>, OpError> {
        let _g = self.mu.lock().unwrap();
        let now = now_ms();
        Ok(self
            .load_grants()?
            .into_iter()
            .filter(|g| !g.revoked && !g.expired(now))
            .collect())
    }

    pub fn received_grants(&self) -> Result<Vec<ImportGrant>, OpError> {
        let _g = self.mu.lock().unwrap();
        let now = now_ms();
        Ok(self
            .load_received()?
            .into_iter()
            .filter(|g| !g.revoked && !g.expired(now))
            .collect())
    }

    fn load_requests(&self) -> Result<Vec<ImportRequest>, OpError> {
        let path = self.system_path("import_requests.json");
        if !path.exists() {
            return Ok(vec![]);
        }
        let raw = fs::read_to_string(&path).map_err(io_err)?;
        serde_json::from_str(&raw).map_err(|e| OpError::new("internal", e.to_string()))
    }

    fn save_requests(&self, reqs: &[ImportRequest]) -> Result<(), OpError> {
        self.save_json(&self.system_path("import_requests.json"), reqs)
    }

    fn load_grants(&self) -> Result<Vec<ImportGrant>, OpError> {
        let path = self.system_path("import_grants.json");
        if !path.exists() {
            return Ok(vec![]);
        }
        let raw = fs::read_to_string(&path).map_err(io_err)?;
        serde_json::from_str(&raw).map_err(|e| OpError::new("internal", e.to_string()))
    }

    fn save_grants(&self, grants: &[ImportGrant]) -> Result<(), OpError> {
        self.save_json(&self.system_path("import_grants.json"), grants)
    }

    fn load_received(&self) -> Result<Vec<ImportGrant>, OpError> {
        let path = self.system_path("import_received.json");
        if !path.exists() {
            return Ok(vec![]);
        }
        let raw = fs::read_to_string(&path).map_err(io_err)?;
        serde_json::from_str(&raw).map_err(|e| OpError::new("internal", e.to_string()))
    }

    fn save_json<T: Serialize + ?Sized>(&self, path: &Path, data: &T) -> Result<(), OpError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        let raw = serde_json::to_vec(data).map_err(|e| OpError::new("internal", e.to_string()))?;
        let tmp = path.with_extension(format!("{}.tmp", random_id()));
        fs::write(&tmp, raw).map_err(io_err)?;
        fs::rename(&tmp, path).map_err(io_err)?;
        Ok(())
    }
}

pub fn require_import_grant(local: &Local, frame: &Frame, caller: &str) -> Result<(), OpError> {
    let device = frame.device().unwrap_or("");
    if device.is_empty() || device == caller {
        return Ok(());
    }
    let space = frame.space().unwrap_or("");
    if protocol::shared_readable(space) {
        return Ok(());
    }
    if frame.payload_bool("seed") {
        if !local.is_seed_authorized(caller) {
            return Err(OpError::new("acl_denied", "seed not authorized"));
        }
        return Ok(());
    }
    let grant = frame.payload.get("grant").and_then(|v| v.as_str()).unwrap_or("");
    if grant.is_empty() {
        return Err(OpError::new("acl_denied", "import grant required"));
    }
    let ok = local
        .imports()
        .validate(grant, device, caller, space)?;
    if !ok {
        return Err(OpError::new("acl_denied", "invalid import grant"));
    }
    Ok(())
}

pub fn import_request(local: &Local, frame: &Frame, caller: &str) -> Result<Map<String, Value>, OpError> {
    let old = frame.payload.get("old_device").and_then(|v| v.as_str()).unwrap_or("");
    let (req, _) = local.imports().create_request(old, caller)?;
    Ok(Map::from_iter([
        ("request_id".into(), json!(req.request_id)),
        ("status".into(), json!(req.status)),
    ]))
}

pub fn import_pending(local: &Local) -> Result<Map<String, Value>, OpError> {
    let pending = local.imports().pending_requests()?;
    let out: Vec<Value> = pending
        .into_iter()
        .map(|r| {
            json!({
                "request_id": r.request_id,
                "old_device": r.old_device,
                "new_device": r.new_device,
                "requested_at": r.requested_at,
                "status": r.status,
            })
        })
        .collect();
    Ok(Map::from_iter([("requests".into(), json!(out))]))
}

pub fn import_grant(local: &Local, frame: &Frame) -> Result<Map<String, Value>, OpError> {
    let request_id = frame
        .payload
        .get("request_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if request_id.is_empty() {
        return Err(OpError::new("bad_op", "request_id required"));
    }
    let g = local.imports().grant(request_id)?;
    Ok(Map::from_iter([("grant".into(), Value::Object(g.to_map()))]))
}

pub fn import_reject(local: &Local, frame: &Frame) -> Result<Map<String, Value>, OpError> {
    let request_id = frame
        .payload
        .get("request_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if request_id.is_empty() {
        return Err(OpError::new("bad_op", "request_id required"));
    }
    local.imports().reject(request_id)?;
    Ok(Map::from_iter([("rejected".into(), json!(true))]))
}

pub fn import_grants(local: &Local, frame: &Frame) -> Result<Map<String, Value>, OpError> {
    let role = frame
        .payload
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("received");
    let grants = if role == "issued" {
        local.imports().issued_grants()?
    } else {
        local.imports().received_grants()?
    };
    let out: Vec<Value> = grants
        .into_iter()
        .map(|g| Value::Object(g.to_map()))
        .collect();
    Ok(Map::from_iter([("grants".into(), json!(out))]))
}

pub fn receive_pushed_grant(
    local: &Local,
    from_device: &str,
    payload: &Map<String, Value>,
) -> Result<(), OpError> {
    let grant_id = payload.get("grant_id").and_then(|v| v.as_str()).unwrap_or("");
    if grant_id.is_empty() {
        return Err(OpError::new("bad_op", "grant_id required"));
    }
    let payload_old = payload.get("old_device").and_then(|v| v.as_str()).unwrap_or("");
    let mut old_device = from_device;
    if protocol::is_valid_device_id(payload_old) {
        old_device = payload_old;
    }
    if !protocol::is_valid_device_id(old_device) {
        return Err(OpError::new("bad_op", "invalid old_device"));
    }
    let spaces: Vec<String> = payload
        .get("spaces")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| GRANT_SPACES.iter().map(|s| s.to_string()).collect());
    let mut g = ImportGrant {
        grant_id: grant_id.to_string(),
        old_device: old_device.to_string(),
        new_device: local.device_id.clone(),
        spaces,
        issued_at: super::any_to_i64(payload.get("issued_at").unwrap_or(&Value::Null)).unwrap_or(0),
        expires_at: super::any_to_i64(payload.get("expires_at").unwrap_or(&Value::Null)).unwrap_or(0),
        revoked: false,
    };
    if g.expires_at <= 0 {
        g.expires_at = now_ms() + IMPORT_DEFAULT_TTL.as_millis() as i64;
    }
    local.imports().save_received(g.clone())?;
    local.emit_event(
        crate::events::StoreEvent::new("import.grant", &local.device_id)
            .with_detail(Map::from_iter([
                ("grant_id".into(), json!(grant_id)),
                ("old_device".into(), json!(old_device)),
            ])),
    );
    Ok(())
}

fn random_id() -> String {
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
