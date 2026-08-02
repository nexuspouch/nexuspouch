pub mod browse;
pub mod bindings;
pub mod cursors;
pub mod embed;
pub mod gc;
pub mod handoff;
pub mod index;
pub mod import;
pub mod maintenance;
pub mod manifest;
pub mod master;
pub mod migrate_seed;
pub mod reprotect;
pub mod retention;
pub mod snapshot_crypto;
pub mod spaces;
pub mod vector;
pub mod versions;
pub mod volume;
pub mod write;

use crate::audit::{self, AuditLog};
use crate::events::EventBus;
use crate::protocol::{self, AclVerdict, Frame};
use cursors::DeviceCursors;
use import::ImportAuth;
use master::MasterPointer;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub trait PeerRpc: Send + Sync {
    fn has(&self, device_id: &str) -> bool;
    fn call(
        &self,
        device_id: &str,
        op: &str,
        payload: Map<String, Value>,
    ) -> Result<Map<String, Value>, String>;
}

pub trait PeerEnsure: Send + Sync {
    fn ensure(&self, device_id: &str) -> Result<(), String>;
    fn release(&self, device_id: &str);
}

pub const MAX_CHUNK: usize = 65536;

#[derive(Debug, Clone)]
pub struct OpError {
    pub code: String,
    pub msg: String,
}

impl OpError {
    pub fn new(code: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            msg: msg.into(),
        }
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.msg)
    }
}

impl std::error::Error for OpError {}

pub struct Local {
    pub root: PathBuf,
    pub device_id: String,
    uploads: Mutex<HashMap<String, Arc<write::Upload>>>,
    imports: ImportAuth,
    cursors: DeviceCursors,
    seed_auth: Mutex<HashMap<String, Instant>>,
    gc_stop: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    peer_rpc: Mutex<Option<Arc<dyn PeerRpc>>>,
    peer_ensure: Mutex<Option<Arc<dyn PeerEnsure>>>,
    event_bus: Mutex<Option<Arc<EventBus>>>,
    index: index::SearchIndex,
    vector: vector::VectorIndex,
    spaces: spaces::SpaceRegistry,
    audit: AuditLog,
}

impl Local {
    pub fn open(root: impl AsRef<Path>, device_id: &str) -> Result<Self, OpError> {
        if !protocol::is_valid_device_id(device_id) {
            return Err(OpError::new("internal", "bad device id"));
        }
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root).map_err(io_err)?;
        for sp in ["artifacts", "files", "attachments", "backups", spaces::MEMORY_SPACE] {
            std::fs::create_dir_all(root.join(device_id).join(sp)).map_err(io_err)?;
        }
        std::fs::create_dir_all(root.join(".recycle")).map_err(io_err)?;
        let local = Self {
            root: root.clone(),
            device_id: device_id.to_string(),
            uploads: Mutex::new(HashMap::new()),
            imports: ImportAuth::new(&root),
            cursors: DeviceCursors::new(&root),
            seed_auth: Mutex::new(HashMap::new()),
            gc_stop: Mutex::new(None),
            peer_rpc: Mutex::new(None),
            peer_ensure: Mutex::new(None),
            event_bus: Mutex::new(None),
            index: index::SearchIndex::open(&root),
            vector: vector::VectorIndex::open(&root, embed::from_env()),
            spaces: spaces::SpaceRegistry::open(&root),
            audit: AuditLog::open(&root),
        };
        // Well-known memory space for distilled agent recall (vector P2).
        let _ = local.spaces.ensure_memory();
        let ptr_path = local.pointer_path();
        if !ptr_path.exists() {
            local.save_pointer(&MasterPointer {
                master: device_id.to_string(),
                // Start at 1 so phone clients can adopt via applyPointer (epoch > 0).
                epoch: 1,
            })?;
        } else if let Ok(p) = local.load_pointer() {
            // Migrate legacy epoch=0 pointers so query/apply can bootstrap remotes.
            if p.epoch == 0 {
                let _ = local.save_pointer(&MasterPointer {
                    master: p.master,
                    epoch: 1,
                });
            }
        }
        Ok(local)
    }

    /// Ensure `root/<device_id>/{spaces}` exist so paired peers appear in stats/browse.
    pub fn ensure_device_spaces(&self, device_id: &str) -> Result<(), OpError> {
        if !protocol::is_valid_device_id(device_id) {
            return Err(OpError::new("bad_path", "invalid device id"));
        }
        for sp in ["artifacts", "files", "attachments", "backups"] {
            std::fs::create_dir_all(self.root.join(device_id).join(sp)).map_err(io_err)?;
        }
        Ok(())
    }

    pub fn handle(
        &self,
        frame: Frame,
        caller: &str,
        trust: &str,
        loopback: bool,
    ) -> Result<Map<String, Value>, OpError> {
        // uri-addressed ops carry space/device inside the uri; enrich the frame
        // so ACL and dispatch see them.
        let frame = match frame.op.as_str() {
            "handoff.ack" | "artifact.state" if frame.space().unwrap_or("").is_empty() => {
                match frame
                    .payload
                    .get("uri")
                    .and_then(|v| v.as_str())
                    .and_then(|u| crate::uri::parse(u).ok())
                {
                    Some(p) => {
                        let mut f = frame.clone();
                        f.payload.insert("space".into(), json!(p.space));
                        f.payload.insert("device".into(), json!(p.device));
                        f
                    }
                    None => frame,
                }
            }
            _ => frame,
        };
        let verdict = protocol::check_acl_with(&frame, caller, trust, loopback, &|sp| {
            self.spaces.visibility(sp)
        });
        if verdict != AclVerdict::Allow {
            let code = acl_code(verdict);
            self.audit.record(audit::deny_entry(
                code,
                caller,
                trust,
                &frame.op,
                code,
                verdict.as_str(),
                frame.space(),
                frame.device(),
                frame.payload.get("path").and_then(|v| v.as_str()),
                None,
            ));
            return Err(OpError::new(code, verdict.as_str()));
        }
        if needs_master_fence(&frame.op) {
            self.require_master()?;
        }
        match frame.op.as_str() {
            "list" => browse::list(self, &frame, caller),
            "meta" => browse::meta(self, &frame, caller),
            "read" => browse::read(self, &frame, caller),
            "versions.list" => versions::list(self, &frame, caller),
            "versions.read" => versions::read(self, &frame, caller),
            "manifest" => manifest::read(self, &frame, caller),
            "handoff.create" => handoff::create(self, &frame, caller),
            "handoff.ack" => handoff::ack(self, &frame, caller),
            "artifact.state" => handoff::state(self, &frame, caller),
            "space.list" => Ok(Map::from_iter([(
                "spaces".into(),
                self.spaces.list_json(),
            )])),
            "space.declare" => self.space_declare(&frame),
            "write.begin" => write::write_begin(self, &frame, caller),
            "write.chunk" => write::write_chunk(self, &frame),
            "commit" => write::commit(self, &frame, caller),
            "delete" => browse::delete(self, &frame, caller),
            "stats" => browse::stats(self),
            "search" => self.op_search(&frame),
            "events.list" => self.op_events_list(&frame),
            "recycle.list" => browse::recycle_list(self),
            "recycle.restore" => browse::recycle_restore(self, &frame),
            "recycle.empty" => browse::recycle_empty(self),
            "import.request" => import::import_request(self, &frame, caller),
            "import.pending" => import::import_pending(self),
            "import.grant" => import::import_grant(self, &frame),
            "import.reject" => import::import_reject(self, &frame),
            "import.grants" => import::import_grants(self, &frame),
            "sync.hello" => cursors::sync_hello(self, caller),
            "sync.cursors" => {
                self.authorize_seed(caller);
                cursors::sync_cursors(self)
            }
            "master.pointer.query" => master::pointer_query(self),
            "master.pointer" => master::pointer_apply(self, &frame),
            "master.migrate" => migrate_seed::master_migrate(self, &frame),
            op => Err(OpError::new("bad_op", op)),
        }
    }

    pub fn pointer_path(&self) -> PathBuf {
        self.root.join(".system").join("master_pointer.json")
    }

    pub fn load_pointer(&self) -> Result<MasterPointer, OpError> {
        master::load_pointer(self)
    }

    pub fn save_pointer(&self, p: &MasterPointer) -> Result<(), OpError> {
        master::save_pointer(self, p)
    }

    pub fn require_master(&self) -> Result<(), OpError> {
        let p = self.load_pointer()?;
        if p.master != self.device_id {
            return Err(OpError::new(
                "not_master",
                format!("master={} epoch={}", p.master, p.epoch),
            ));
        }
        Ok(())
    }

    pub fn resolve(&self, space: &str, device: &str, rel: &str) -> Result<PathBuf, OpError> {
        browse::resolve_path(self, space, device, rel)
    }

    pub fn require_import_grant(&self, frame: &Frame, caller: &str) -> Result<(), OpError> {
        import::require_import_grant(self, frame, caller)
    }

    pub fn authorize_seed(&self, caller: &str) {
        if caller.is_empty() {
            return;
        }
        let mut m = self.seed_auth.lock().unwrap();
        m.insert(caller.to_string(), Instant::now() + Duration::from_secs(15 * 60));
    }

    pub fn is_seed_authorized(&self, caller: &str) -> bool {
        let mut m = self.seed_auth.lock().unwrap();
        if let Some(exp) = m.get(caller) {
            if Instant::now() < *exp {
                return true;
            }
            m.remove(caller);
        }
        false
    }

    pub fn forget_upload(&self, id: &str) {
        self.uploads.lock().unwrap().remove(id);
    }

    pub fn uploads(&self) -> &Mutex<HashMap<String, Arc<write::Upload>>> {
        &self.uploads
    }

    pub fn cursors(&self) -> &DeviceCursors {
        &self.cursors
    }

    pub fn imports(&self) -> &ImportAuth {
        &self.imports
    }

    pub fn receive_pushed_grant(
        &self,
        from_device: &str,
        payload: &Map<String, Value>,
    ) -> Result<(), OpError> {
        import::receive_pushed_grant(self, from_device, payload)
    }

    pub fn set_peer_rpc(&self, rpc: Arc<dyn PeerRpc>) {
        *self.peer_rpc.lock().unwrap() = Some(rpc);
    }

    pub fn set_peer_ensure(&self, ensure: Arc<dyn PeerEnsure>) {
        *self.peer_ensure.lock().unwrap() = Some(ensure);
    }

    pub fn set_event_bus(&self, bus: Arc<EventBus>) {
        *self.event_bus.lock().unwrap() = Some(bus);
    }

    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    pub fn audit_action(&self, kind: &str, caller: &str, op: &str, message: &str) {
        self.audit.record(audit::deny_entry(
            kind,
            caller,
            protocol::TRUST_OWNER,
            op,
            kind,
            message,
            None,
            None,
            None,
            None,
        ));
    }

    pub(crate) fn emit_event(&self, event: crate::events::StoreEvent) {
        if let Some(bus) = self.event_bus.lock().unwrap().as_ref() {
            bus.publish(event);
        }
    }

    pub fn index_file(&self, e: &index::IndexEntry) {
        self.index.index_file(e);
        // Vector side: prefer summary + body text already on the index entry.
        let text = format!(
            "{} {}",
            e.summary.as_deref().unwrap_or(""),
            e.body
        );
        self.vector.upsert_text(&e.uri, text.trim());
    }

    pub fn remove_index(&self, uri: &str) {
        self.index.remove(uri);
        self.vector.remove(uri);
    }

    pub fn search_index(
        &self,
        q: &str,
        space: Option<&str>,
        device: Option<&str>,
        state: Option<&str>,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, OpError> {
        self.index
            .search(q, space, device, state, limit)
            .map_err(|e| OpError::new("internal", e))
    }

    /// Keyword and/or semantic search. When `semantic` and the embedder is
    /// unavailable / empty, falls back to FTS5 with `degraded: true`.
    pub fn search_query(
        &self,
        q: &str,
        space: Option<&str>,
        device: Option<&str>,
        state: Option<&str>,
        limit: usize,
        semantic: bool,
    ) -> Result<Map<String, Value>, OpError> {
        if semantic {
            match self.vector.search(q, space, device, limit) {
                Ok(results) if !results.is_empty() => {
                    return Ok(Map::from_iter([
                        ("query".into(), json!(q)),
                        ("total".into(), json!(results.len())),
                        ("results".into(), json!(results)),
                        ("score_type".into(), json!("vector")),
                        ("degraded".into(), json!(false)),
                        ("embedder".into(), json!(self.vector.embedder_name())),
                    ]));
                }
                Ok(_) | Err(_) => {
                    let results = self.search_index(q, space, device, state, limit)?;
                    return Ok(Map::from_iter([
                        ("query".into(), json!(q)),
                        ("total".into(), json!(results.len())),
                        ("results".into(), json!(results)),
                        ("score_type".into(), json!("keyword")),
                        ("degraded".into(), json!(true)),
                        ("embedder".into(), json!(self.vector.embedder_name())),
                    ]));
                }
            }
        }
        let results = self.search_index(q, space, device, state, limit)?;
        Ok(Map::from_iter([
            ("query".into(), json!(q)),
            ("total".into(), json!(results.len())),
            ("results".into(), json!(results)),
            ("score_type".into(), json!("keyword")),
            ("degraded".into(), json!(false)),
        ]))
    }

    fn op_search(&self, frame: &Frame) -> Result<Map<String, Value>, OpError> {
        let q = frame
            .payload
            .get("q")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if q.is_empty() {
            return Err(OpError::new("bad_op", "search requires non-empty q"));
        }
        let space = frame.payload.get("space").and_then(|v| v.as_str());
        if let Some(sp) = space {
            if !self.spaces.is_known(sp) {
                return Err(OpError::new("bad_op", "unknown space"));
            }
        }
        let device = frame.payload.get("device").and_then(|v| v.as_str());
        if let Some(d) = device {
            if !protocol::is_valid_device_id(d) {
                return Err(OpError::new("bad_op", "invalid device"));
            }
        }
        let state = frame.payload.get("state").and_then(|v| v.as_str());
        let limit = any_to_i64(frame.payload.get("limit").unwrap_or(&Value::Null))
            .unwrap_or(50)
            .clamp(1, 200) as usize;
        let semantic = frame
            .payload
            .get("semantic")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        self.search_query(q, space, device, state, limit, semantic)
    }

    fn op_events_list(&self, frame: &Frame) -> Result<Map<String, Value>, OpError> {
        let since = any_to_i64(frame.payload.get("since").unwrap_or(&json!(0)))
            .unwrap_or(0)
            .max(0) as u64;
        let limit = any_to_i64(frame.payload.get("limit").unwrap_or(&json!(50)))
            .unwrap_or(50)
            .clamp(1, 200) as usize;
        let kind = frame.payload.get("kind").and_then(|v| v.as_str());
        let bus = self.event_bus.lock().unwrap().clone();
        let Some(bus) = bus else {
            return Ok(Map::from_iter([
                ("events".into(), json!([])),
                ("latest_seq".into(), json!(0)),
            ]));
        };
        let mut events = bus.replay(since);
        if events.is_empty() && since == 0 {
            events = bus.recent();
        } else if events.is_empty() {
            events = bus
                .recent()
                .into_iter()
                .filter(|e| e.seq > since)
                .collect();
        }
        if let Some(k) = kind {
            events.retain(|e| e.kind == k);
        }
        if events.len() > limit {
            events.truncate(limit);
        }
        let latest_seq = events.iter().map(|e| e.seq).max().unwrap_or(since);
        let latest_seq = latest_seq.max(
            bus.recent()
                .iter()
                .map(|e| e.seq)
                .max()
                .unwrap_or(latest_seq),
        );
        Ok(Map::from_iter([
            ("events".into(), json!(events)),
            ("latest_seq".into(), json!(latest_seq)),
        ]))
    }

    pub fn rebuild_index(&self) -> Result<usize, OpError> {
        self.index
            .rebuild(self)
            .map_err(|e| OpError::new("internal", e))
    }

    pub fn is_known_space(&self, name: &str) -> bool {
        self.spaces.is_known(name)
    }

    pub fn list_spaces_json(&self) -> serde_json::Value {
        self.spaces.list_json()
    }

    fn space_declare(&self, frame: &Frame) -> Result<Map<String, Value>, OpError> {
        let name = frame
            .payload
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| OpError::new("bad_op", "space.declare requires name"))?;
        let visibility = frame
            .payload
            .get("visibility")
            .and_then(|v| v.as_str())
            .unwrap_or("shared");
        let encryption = frame
            .payload
            .get("encryption")
            .and_then(|v| v.as_str())
            .unwrap_or("none");
        let retention = frame
            .payload
            .get("retention")
            .and_then(|v| v.as_str())
            .unwrap_or("none");
        let import_grant = frame
            .payload
            .get("import_grant")
            .and_then(|v| v.as_str())
            .unwrap_or("allowed");
        self.declare_space_profile(name, visibility, encryption, retention, import_grant)
    }

    pub fn declare_space_profile(
        &self,
        name: &str,
        visibility: &str,
        encryption: &str,
        retention: &str,
        import_grant: &str,
    ) -> Result<Map<String, Value>, OpError> {
        let p = self
            .spaces
            .declare(name, visibility, encryption, retention, import_grant)
            .map_err(|e| OpError::new("bad_op", e))?;
        Ok(Map::from_iter([
            ("name".into(), json!(p.name)),
            ("visibility".into(), json!(p.visibility)),
            ("encryption".into(), json!(p.encryption)),
            ("retention".into(), json!(p.retention)),
            ("import_grant".into(), json!(p.import_grant)),
        ]))
    }

    pub(crate) fn peer_rpc(&self) -> Option<Arc<dyn PeerRpc>> {
        self.peer_rpc.lock().unwrap().clone()
    }

    pub(crate) fn peer_ensure(&self) -> Option<Arc<dyn PeerEnsure>> {
        self.peer_ensure.lock().unwrap().clone()
    }

    pub fn start_periodic_gc(self: &Arc<Self>, every: Duration) {
        let every = if every.is_zero() {
            Duration::from_secs(3600)
        } else {
            every
        };
        let mut guard = self.gc_stop.lock().unwrap();
        if guard.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        *guard = Some(tx);
        let store = Arc::clone(self);
        std::thread::spawn(move || loop {
            if rx.recv_timeout(every).is_ok() {
                break;
            }
            let _ = gc::gc_staging(&store, Duration::ZERO);
            let _ = gc::gc_recycle(&store, Duration::ZERO);
        });
    }
}

fn needs_master_fence(op: &str) -> bool {
    matches!(
        op,
        "write.begin" | "write.chunk" | "commit" | "delete" | "sync.hello"
    )
}

fn acl_code(v: AclVerdict) -> &'static str {
    match v {
        AclVerdict::DenyUntrusted => "untrusted",
        AclVerdict::DenyAcl => "acl_denied",
        AclVerdict::DenyBadOp => "bad_op",
        AclVerdict::DenyBadPath => "bad_path",
        AclVerdict::Allow => "internal",
    }
}

pub fn io_err(e: std::io::Error) -> OpError {
    OpError::new("internal", e.to_string())
}

pub fn payload_int64(payload: &Map<String, Value>, key: &str) -> Option<i64> {
    payload.get(key).and_then(any_to_i64)
}

pub fn any_to_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        _ => None,
    }
}

pub fn num(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub fn map_value(m: Map<String, Value>) -> Value {
    Value::Object(m)
}

pub fn ok_map(data: Map<String, Value>) -> Result<Map<String, Value>, OpError> {
    Ok(data)
}

pub fn json_map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Frame;
    use base64::Engine;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn write_read_roundtrip() {
        let dir = tempdir().unwrap();
        let device = "aaaaaaaaaaaaaaaa";
        let store = Local::open(dir.path(), device).unwrap();
        let path = "task-41/x.txt";
        let content = b"hello store";
        use sha2::{Digest, Sha256};
        let sha = hex::encode(Sha256::digest(content));

        let mut begin_payload = Map::new();
        begin_payload.insert("space".into(), json!("artifacts"));
        begin_payload.insert("path".into(), json!(path));
        begin_payload.insert("size".into(), json!(content.len()));
        begin_payload.insert("sha256".into(), json!(sha));
        let begin = store
            .handle(
                Frame::from_parts("write.begin", begin_payload),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
        let upload_id = begin["upload_id"].as_str().unwrap().to_string();

        let mut chunk_payload = Map::new();
        chunk_payload.insert("upload_id".into(), json!(upload_id));
        chunk_payload.insert("offset".into(), json!(0));
        chunk_payload.insert(
            "data".into(),
            json!(base64::engine::general_purpose::STANDARD.encode(content)),
        );
        store
            .handle(
                Frame::from_parts("write.chunk", chunk_payload),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();

        let mut commit_payload = Map::new();
        commit_payload.insert("space".into(), json!("artifacts"));
        commit_payload.insert("upload_ids".into(), json!([upload_id]));
        store
            .handle(
                Frame::from_parts("commit", commit_payload),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();

        let mut read_payload = Map::new();
        read_payload.insert("space".into(), json!("artifacts"));
        read_payload.insert("path".into(), json!(path));
        read_payload.insert("offset".into(), json!(0));
        read_payload.insert("length".into(), json!(65536));
        let read = store
            .handle(
                Frame::from_parts("read", read_payload),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
        let data_b64 = read["data"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD.decode(data_b64).unwrap();
        assert_eq!(decoded, content);
    }

    #[test]
    fn custom_space_flow() {
        let dir = tempdir().unwrap();
        let device = "aaaaaaaaaaaaaaaa";
        let store = Local::open(dir.path(), device).unwrap();

        // Declare a custom shared space (loopback only).
        let mut declare = Map::new();
        declare.insert("name".into(), json!("models"));
        declare.insert("visibility".into(), json!("shared"));
        let out = store
            .handle(
                Frame::from_parts("space.declare", declare),
                device,
                protocol::TRUST_OWNER,
                true,
            )
            .unwrap();
        assert_eq!(out["name"], "models");

        // Remote declare denied.
        let mut declare = Map::new();
        declare.insert("name".into(), json!("vault"));
        let err = store
            .handle(
                Frame::from_parts("space.declare", declare),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap_err();
        assert_eq!(err.code, "acl_denied");

        // Write into the custom space.
        let path = "model-a/weights.bin";
        let content = b"weights";
        use sha2::{Digest, Sha256};
        let sha = hex::encode(Sha256::digest(content));
        let mut begin = Map::new();
        begin.insert("space".into(), json!("models"));
        begin.insert("path".into(), json!(path));
        begin.insert("size".into(), json!(content.len()));
        begin.insert("sha256".into(), json!(sha));
        let out = store
            .handle(
                Frame::from_parts("write.begin", begin),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
        let upload_id = out["upload_id"].as_str().unwrap().to_string();
        let mut chunk = Map::new();
        chunk.insert("upload_id".into(), json!(upload_id));
        chunk.insert("offset".into(), json!(0));
        chunk.insert(
            "data".into(),
            json!(base64::engine::general_purpose::STANDARD.encode(content)),
        );
        store
            .handle(
                Frame::from_parts("write.chunk", chunk),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
        let mut commit = Map::new();
        commit.insert("space".into(), json!("models"));
        commit.insert("upload_ids".into(), json!([upload_id]));
        store
            .handle(
                Frame::from_parts("commit", commit),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();

        let mut list = Map::new();
        list.insert("space".into(), json!("models"));
        list.insert("path".into(), json!(""));
        let out = store
            .handle(
                Frame::from_parts("list", list),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
        assert!(
            out["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["path"].as_str().unwrap().ends_with("weights.bin"))
        );

        // space.list returns builtins + declared.
        let out = store
            .handle(
                Frame::from_parts("space.list", Map::new()),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
        let names: Vec<&str> = out["spaces"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["name"].as_str())
            .collect();
        assert!(names.contains(&"models"));
        assert!(names.contains(&"files"));
        assert!(names.contains(&"memory"));

        // Unknown (undeclared) space denied.
        let mut mystery = Map::new();
        mystery.insert("space".into(), json!("mystery"));
        mystery.insert("path".into(), json!("x"));
        let err = store
            .handle(
                Frame::from_parts("list", mystery),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap_err();
        assert_eq!(err.code, "bad_op");
    }

    #[test]
    fn search_and_events_list_frames() {
        let dir = tempfile::tempdir().unwrap();
        let device = "aaaaaaaaaaaaaaaa";
        let store = Local::open(dir.path(), device).unwrap();
        let bus = crate::events::EventBus::open(dir.path());
        store.set_event_bus(bus.clone());

        store.index_file(&index::IndexEntry {
            uri: format!("store://artifacts/{device}/t-1/out.md"),
            space: "artifacts".into(),
            device: device.into(),
            path: "t-1/out.md".into(),
            sha256: "a".repeat(64),
            size: 12,
            mtime: 0,
            task: "t-1".into(),
            state: "published".into(),
            summary: None,
            body: "handoff zebra99 payload".into(),
        });

        let mut q = Map::new();
        q.insert("q".into(), json!("zebra99"));
        q.insert("space".into(), json!("artifacts"));
        let out = store
            .handle(
                Frame::from_parts("search", q),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
        assert_eq!(out["total"], 1);
        assert_eq!(out["results"][0]["path"], "t-1/out.md");

        bus.publish(
            crate::events::StoreEvent::new("handoff.created", device)
                .with_uri(format!("store://artifacts/{device}/t-1/out.md")),
        );
        let mut ev = Map::new();
        ev.insert("since".into(), json!(0));
        ev.insert("kind".into(), json!("handoff.created"));
        let out = store
            .handle(
                Frame::from_parts("events.list", ev),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
        assert!(out["latest_seq"].as_u64().unwrap() >= 1);
        assert_eq!(out["events"].as_array().unwrap().len(), 1);
        assert_eq!(out["events"][0]["kind"], "handoff.created");

        let err = store
            .handle(
                Frame::from_parts("search", Map::from_iter([("q".into(), json!(""))])),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap_err();
        assert_eq!(err.code, "bad_op");
    }
}
