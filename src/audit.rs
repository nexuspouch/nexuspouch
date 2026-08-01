use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const RING: usize = 200;
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub ts_ms: i64,
    pub kind: String,
    pub caller: String,
    pub trust: String,
    pub op: String,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

pub struct AuditLog {
    path: PathBuf,
    ring: Mutex<VecDeque<AuditEntry>>,
}

impl AuditLog {
    pub fn open(root: impl AsRef<Path>) -> Self {
        let dir = root.as_ref().join(".system");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("audit.jsonl");
        let mut ring = VecDeque::new();
        if let Ok(raw) = fs::read_to_string(&path) {
            for line in raw.lines().rev() {
                if ring.len() >= RING {
                    break;
                }
                if let Ok(e) = serde_json::from_str::<AuditEntry>(line) {
                    ring.push_front(e);
                }
            }
        }
        Self {
            path,
            ring: Mutex::new(ring),
        }
    }

    pub fn record(&self, mut entry: AuditEntry) {
        if entry.ts_ms == 0 {
            entry.ts_ms = now_ms();
        }
        if let Ok(mut g) = self.ring.lock() {
            g.push_back(entry.clone());
            while g.len() > RING {
                g.pop_front();
            }
        }
        let _ = self.append_file(&entry);
    }

    pub fn recent(&self, limit: usize) -> Vec<AuditEntry> {
        let g = self.ring.lock().unwrap();
        let n = limit.min(g.len());
        g.iter().rev().take(n).cloned().collect()
    }

    pub fn recent_json(&self, limit: usize) -> Map<String, Value> {
        let entries: Vec<Value> = self
            .recent(limit)
            .into_iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();
        Map::from_iter([
            ("entries".into(), Value::Array(entries)),
            ("count".into(), json!(self.ring.lock().unwrap().len())),
        ])
    }

    fn append_file(&self, entry: &AuditEntry) -> std::io::Result<()> {
        if let Ok(meta) = fs::metadata(&self.path) {
            if meta.len() > MAX_FILE_BYTES {
                let bak = self.path.with_extension("jsonl.1");
                let _ = fs::rename(&self.path, &bak);
            }
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(f, "{}", serde_json::to_string(entry).unwrap_or_default())?;
        Ok(())
    }
}

pub fn deny_entry(
    kind: &str,
    caller: &str,
    trust: &str,
    op: &str,
    code: &str,
    message: &str,
    space: Option<&str>,
    device: Option<&str>,
    path: Option<&str>,
) -> AuditEntry {
    AuditEntry {
        ts_ms: now_ms(),
        kind: kind.to_string(),
        caller: caller.to_string(),
        trust: trust.to_string(),
        op: op.to_string(),
        code: code.to_string(),
        message: message.to_string(),
        space: space.map(str::to_string),
        device: device.map(str::to_string),
        path: path.map(str::to_string),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn records_and_lists() {
        let dir = tempdir().unwrap();
        let log = AuditLog::open(dir.path());
        log.record(deny_entry(
            "untrusted",
            "aaaaaaaaaaaaaaaa",
            "friend",
            "list",
            "untrusted",
            "denyUntrusted",
            Some("artifacts"),
            None,
            None,
        ));
        let recent = log.recent(10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].kind, "untrusted");
        assert!(dir.path().join(".system/audit.jsonl").exists());
    }
}
