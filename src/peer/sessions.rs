use crate::noise::{self, Session};
use axum::extract::ws::Message;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub struct LiveSession {
    pub fp: String,
    pub sess: Mutex<Session>,
    pub pending: Mutex<HashMap<String, std::sync::mpsc::Sender<Map<String, Value>>>>,
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

    pub fn add(&self, fp: String, sess: Session) {
        let mut g = self.by_fp.lock().unwrap();
        g.insert(
            fp.clone(),
            Arc::new(LiveSession {
                fp,
                sess: Mutex::new(sess),
                pending: Mutex::new(HashMap::new()),
            }),
        );
    }

    pub fn remove(&self, fp: &str) {
        let mut g = self.by_fp.lock().unwrap();
        if let Some(ls) = g.remove(fp) {
            let mut pending = ls.pending.lock().unwrap();
            for (_, ch) in pending.drain() {
                drop(ch);
            }
        }
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

    pub fn fanout_json(&self, plain: &Map<String, Value>) -> usize {
        let sessions: Vec<_> = self.by_fp.lock().unwrap().values().cloned().collect();
        let raw = serde_json::to_vec(plain).unwrap_or_default();
        let mut n = 0;
        for ls in sessions {
            if encrypt_write(&ls, &raw).is_ok() {
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
        encrypt_write(&ls, &raw).is_ok()
    }

    pub fn next_req_id(&self) -> String {
        format!("rpc-{}", self.seq.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
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
