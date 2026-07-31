use super::{io_err, Local, OpError};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct DeviceCursors {
    path: PathBuf,
    mu: Mutex<()>,
}

impl DeviceCursors {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            path: root.as_ref().join(".system").join("device_cursors.json"),
            mu: Mutex::new(()),
        }
    }

    pub fn applied_seq(&self, device_id: &str) -> Result<i64, OpError> {
        let m = self.all()?;
        Ok(m.get(device_id).copied().unwrap_or(0))
    }

    pub fn all(&self) -> Result<HashMap<String, i64>, OpError> {
        let _g = self.mu.lock().unwrap();
        self.load_locked()
    }

    pub fn advance(&self, device_id: &str, upto: i64) -> Result<i64, OpError> {
        let _g = self.mu.lock().unwrap();
        let mut m = self.load_locked()?;
        let cur = m.get(device_id).copied().unwrap_or(0);
        let next = cur.max(upto);
        m.insert(device_id.to_string(), next);
        self.save_locked(&m)?;
        Ok(next)
    }

    pub fn remove(&self, device_id: &str) -> Result<(), OpError> {
        let _g = self.mu.lock().unwrap();
        let mut m = self.load_locked()?;
        if m.remove(device_id).is_some() {
            self.save_locked(&m)?;
        }
        Ok(())
    }

    fn load_locked(&self) -> Result<HashMap<String, i64>, OpError> {
        if !self.path.exists() {
            return Ok(HashMap::new());
        }
        let raw = fs::read_to_string(&self.path).map_err(io_err)?;
        let decoded: HashMap<String, Value> =
            serde_json::from_str(&raw).map_err(|e| OpError::new("internal", e.to_string()))?;
        let mut out = HashMap::new();
        for (k, v) in decoded {
            if let Some(n) = super::any_to_i64(&v) {
                out.insert(k, n);
            }
        }
        Ok(out)
    }

    fn save_locked(&self, m: &HashMap<String, i64>) -> Result<(), OpError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        let raw = serde_json::to_string(m).map_err(|e| OpError::new("internal", e.to_string()))?;
        let tmp = self.path.with_extension("tmp");
        fs::write(&tmp, &raw).map_err(io_err)?;
        fs::rename(&tmp, &self.path).map_err(io_err)?;
        Ok(())
    }
}

pub fn sync_hello(local: &Local, caller: &str) -> Result<Map<String, Value>, OpError> {
    if !crate::protocol::is_valid_device_id(caller) {
        return Err(OpError::new("bad_path", "invalid caller"));
    }
    let seq = local.cursors().applied_seq(caller)?;
    let mut m = Map::new();
    m.insert("applied_seq".into(), json!(seq));
    Ok(m)
}

pub fn sync_cursors(local: &Local) -> Result<Map<String, Value>, OpError> {
    let m = local.cursors().all()?;
    let mut out = Map::new();
    for (k, v) in m {
        out.insert(k, json!(v));
    }
    let mut result = Map::new();
    result.insert("cursors".into(), Value::Object(out));
    Ok(result)
}
