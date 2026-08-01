use crate::noise::{self, Session};
use axum::extract::ws::Message;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

pub struct LiveSession {
    pub fp: String,
    pub sess: Mutex<Session>,
    pub outbound: Mutex<Option<UnboundedSender<Message>>>,
    pub pending: Mutex<HashMap<String, std::sync::mpsc::Sender<Map<String, Value>>>>,
    session_id: u64,
}

pub struct SessionRegistry {
    by_fp: Mutex<HashMap<String, Arc<LiveSession>>>,
    seq: AtomicU64,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            by_fp: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(0),
        }
    }

    /// Registers or replaces the live session for `fp`. Returns a session token for remove().
    pub fn register(
        &self,
        fp: String,
        sess: Session,
        outbound: UnboundedSender<Message>,
    ) -> u64 {
        let session_id = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut g = self.by_fp.lock().unwrap();
        g.insert(
            fp.clone(),
            Arc::new(LiveSession {
                fp,
                sess: Mutex::new(sess),
                outbound: Mutex::new(Some(outbound)),
                pending: Mutex::new(HashMap::new()),
                session_id,
            }),
        );
        session_id
    }

    pub fn remove(&self, fp: &str, session_id: u64) {
        let mut g = self.by_fp.lock().unwrap();
        let Some(ls) = g.get(fp) else {
            return;
        };
        if ls.session_id != session_id {
            return;
        }
        let ls = g.remove(fp).unwrap();
        let mut pending = ls.pending.lock().unwrap();
        pending.clear();
    }

    pub fn has(&self, device_id: &str) -> bool {
        if device_id.is_empty() {
            return false;
        }
        self.by_fp.lock().unwrap().contains_key(device_id)
    }

    pub fn get(&self, fp: &str) -> Option<Arc<LiveSession>> {
        self.by_fp.lock().unwrap().get(fp).cloned()
    }

    pub fn deliver_reply(&self, fp: &str, req_id: &str, msg: Map<String, Value>) -> bool {
        let Some(ls) = self.get(fp) else {
            return false;
        };
        let mut pending = ls.pending.lock().unwrap();
        if let Some(ch) = pending.remove(req_id) {
            let _ = ch.send(msg);
            return true;
        }
        false
    }

    pub fn call(
        &self,
        device_id: &str,
        op: &str,
        payload: Map<String, Value>,
    ) -> Result<Map<String, Value>, String> {
        self.call_store(device_id, op, payload, Duration::from_secs(20))
    }

    pub fn call_store(
        &self,
        fp: &str,
        op: &str,
        payload: Map<String, Value>,
        timeout: Duration,
    ) -> Result<Map<String, Value>, String> {
        let ls = self.get(fp).ok_or_else(|| format!("peer offline: {fp}"))?;
        let req_id = format!("rpc-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        let mut frame = Map::new();
        frame.insert("type".into(), json!("store"));
        frame.insert("ns".into(), json!("store"));
        frame.insert("op".into(), json!(op));
        frame.insert("v".into(), json!(1));
        frame.insert("req_id".into(), json!(req_id.clone()));
        for (k, v) in payload.clone() {
            frame.insert(k, v);
        }
        let raw = serde_json::to_vec(&frame).map_err(|e| e.to_string())?;
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut pending = ls.pending.lock().unwrap();
            pending.insert(req_id.clone(), tx);
        }
        if let Err(e) = encrypt_and_send(&ls, &raw) {
            let mut pending = ls.pending.lock().unwrap();
            pending.remove(&req_id);
            return Err(e);
        }
        match rx.recv_timeout(timeout) {
            Ok(msg) => parse_call_reply(&msg),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let mut pending = ls.pending.lock().unwrap();
                pending.remove(&req_id);
                Err(format!("timeout calling {op} on {fp}"))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err("session closed".into())
            }
        }
    }

    pub fn fanout_json(&self, plain: &Map<String, Value>) -> usize {
        let sessions: Vec<_> = self.by_fp.lock().unwrap().values().cloned().collect();
        let raw = serde_json::to_vec(plain).unwrap_or_default();
        let mut n = 0;
        for ls in sessions {
            if encrypt_and_send(&ls, &raw).is_ok() {
                n += 1;
            }
        }
        n
    }

    pub fn fanout_master_pointer(&self, master: &str, epoch: i64, from_device: &str) -> usize {
        let mut m = Map::new();
        m.insert("type".into(), json!("store"));
        m.insert("ns".into(), json!("store"));
        m.insert("op".into(), json!("master.pointer"));
        m.insert("v".into(), json!(1));
        m.insert("master".into(), json!(master));
        m.insert("epoch".into(), json!(epoch));
        m.insert("from".into(), json!(from_device));
        self.fanout_json(&m)
    }

    pub fn push_import_grant(&self, grant: &Map<String, Value>) -> bool {
        let new_device = grant.get("new_device").and_then(|v| v.as_str()).unwrap_or("");
        if new_device.is_empty() {
            return false;
        }
        let mut m = Map::new();
        m.insert("type".into(), json!("store"));
        m.insert("ns".into(), json!("store"));
        m.insert("op".into(), json!("import.grant"));
        m.insert("v".into(), json!(1));
        for k in ["grant_id", "old_device", "spaces", "issued_at", "expires_at"] {
            if let Some(v) = grant.get(k) {
                m.insert(k.into(), v.clone());
            }
        }
        self.send_json(new_device, &m)
    }

    pub fn send_json(&self, fp: &str, plain: &Map<String, Value>) -> bool {
        let Some(ls) = self.get(fp) else {
            return false;
        };
        let raw = serde_json::to_vec(plain).unwrap_or_default();
        encrypt_and_send(&ls, &raw).is_ok()
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::store::PeerRpc for SessionRegistry {
    fn has(&self, device_id: &str) -> bool {
        if device_id.is_empty() {
            return false;
        }
        self.by_fp.lock().unwrap().contains_key(device_id)
    }

    fn call(
        &self,
        device_id: &str,
        op: &str,
        payload: Map<String, Value>,
    ) -> Result<Map<String, Value>, String> {
        self.call_store(device_id, op, payload, Duration::from_secs(20))
    }
}

fn parse_call_reply(msg: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let op = msg.get("op").and_then(|v| v.as_str()).unwrap_or("");
    if op == "error" {
        let code = msg
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("internal");
        let message = msg
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return Err(format!("{code}: {message}"));
    }
    if let Some(data) = msg.get("data").and_then(|v| v.as_object()) {
        return Ok(data.clone());
    }
    Ok(Map::new())
}

pub fn encrypt_and_send(ls: &LiveSession, plain: &[u8]) -> Result<(), String> {
    let msg = encrypt_write(ls, plain)?;
    let tx = ls.outbound.lock().unwrap();
    let Some(tx) = tx.as_ref() else {
        return Err("no outbound".into());
    };
    tx.send(msg).map_err(|e| e.to_string())
}

pub fn encrypt_write(ls: &LiveSession, plain: &[u8]) -> Result<Message, String> {
    let mut sess = ls.sess.lock().unwrap();
    let ct = sess.encrypt(plain)?;
    let enc = noise::encode_frame(&noise::Frame {
        frame_type: noise::FrameType::Data,
        payload: ct,
    })?;
    Ok(Message::Text(enc.into()))
}

pub fn parse_store_control(plain: &[u8]) -> Option<(String, String, Map<String, Value>)> {
    let raw: Value = serde_json::from_slice(plain).ok()?;
    let obj = raw.as_object()?;
    let op = obj.get("op")?.as_str()?.to_string();
    let req_id = obj.get("req_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if let Some(nested) = obj.get("payload").and_then(|v| v.as_object()) {
        if !obj.contains_key("ns") {
            return Some((op, req_id, nested.clone()));
        }
    }
    let mut payload = Map::new();
    for (k, v) in obj {
        if matches!(k.as_str(), "type" | "ns" | "op" | "v" | "req_id") {
            continue;
        }
        payload.insert(k.clone(), v.clone());
    }
    Some((op, req_id, payload))
}
