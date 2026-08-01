//! Task manifest (M2): machine-readable lineage for a task directory.
//!
//! Stored at `<device>/<space>/<task>/.nexuspouch/manifest.json` — a
//! dot-prefixed directory, so external list/read/write frames cannot touch it;
//! only the internal `manifest` op reads it. This is the agent equivalent of a
//! commit message + blame: producer, parent URIs, file hashes, state.

use super::{io_err, Local, OpError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub task_id: String,
    pub device_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parent_uris: Vec<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
    pub size: i64,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// First path segment = task root.
pub fn task_root(rel: &str) -> Option<String> {
    rel.split('/').next().filter(|s| !s.is_empty()).map(str::to_string)
}

fn manifest_path(local: &Local, device: &str, space: &str, task: &str) -> PathBuf {
    local
        .root
        .join(device)
        .join(space)
        .join(task.replace('/', std::path::MAIN_SEPARATOR_STR))
        .join(".nexuspouch")
        .join("manifest.json")
}

fn load(local: &Local, device: &str, space: &str, task: &str) -> Manifest {
    let p = manifest_path(local, device, space, task);
    fs::read_to_string(&p)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save(local: &Local, device: &str, space: &str, task: &str, m: &Manifest) -> Result<(), OpError> {
    let p = manifest_path(local, device, space, task);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(m).map_err(|e| OpError::new("internal", e.to_string()))?)
        .map_err(io_err)?;
    fs::rename(&tmp, &p).map_err(io_err)
}

/// Merge a commit batch into the task manifest.
///
/// `committed` entries carry `{path, sha256, size}`. The optional
/// `manifest` payload may carry `producer`, `parent_uris`, `summary`, `state`;
/// `publish: true` moves the task state to `published` (versions protected).
pub fn update(
    local: &Local,
    device: &str,
    space: &str,
    task: &str,
    committed: &[Value],
    manifest_payload: Option<&Value>,
    publish: bool,
) -> Result<(), OpError> {
    let mut m = load(local, device, space, task);
    if m.task_id.is_empty() {
        m.task_id = task.to_string();
        m.device_id = device.to_string();
        m.created_at = now_ms();
    }
    if let Some(p) = manifest_payload {
        if p.is_object() {
            if let Some(producer) = p.get("producer") {
                m.producer = Some(producer.clone());
            }
            if let Some(uris) = p.get("parent_uris").and_then(|v| v.as_array()) {
                for u in uris {
                    if let Some(s) = u.as_str() {
                        if !m.parent_uris.contains(&s.to_string()) {
                            m.parent_uris.push(s.to_string());
                        }
                    }
                }
            }
            if let Some(s) = p.get("summary").and_then(|v| v.as_str()) {
                m.summary = Some(s.to_string());
            }
            if let Some(s) = p.get("state").and_then(|v| v.as_str()) {
                m.state = s.to_string();
            }
        }
    }
    for c in committed {
        let path = c.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let sha256 = c.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
        let size = c.get("size").and_then(|v| v.as_i64()).unwrap_or(0);
        if path.is_empty() || sha256.is_empty() {
            continue;
        }
        if let Some(existing) = m.files.iter_mut().find(|f| f.path == path) {
            existing.sha256 = sha256.to_string();
            existing.size = size;
        } else {
            m.files.push(FileEntry {
                path: path.to_string(),
                sha256: sha256.to_string(),
                size,
            });
        }
    }
    if publish && m.state != "published" {
        m.state = "published".to_string();
    }
    if m.state.is_empty() {
        m.state = "committed".to_string();
    }
    save(local, device, space, task, &m)
}

/// `manifest` op: `{space, device, path}` (path = task dir) -> manifest JSON.
pub fn read(local: &Local, frame: &crate::protocol::Frame, caller: &str) -> Result<Map<String, Value>, OpError> {
    let mut device = frame.device().unwrap_or("").to_string();
    if device.is_empty() {
        device = caller.to_string();
    }
    let space = frame.space().unwrap_or("");
    let path = frame.payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let task = task_root(path)
        .ok_or_else(|| OpError::new("bad_path", "manifest requires a task directory path"))?;
    let p = manifest_path(local, &device, space, &task);
    let raw = fs::read_to_string(&p).map_err(|e| OpError::new("not_found", e.to_string()))?;
    let val: Value = serde_json::from_str(&raw).map_err(|e| OpError::new("internal", e.to_string()))?;
    Ok(Map::from_iter([
        ("space".into(), json!(space)),
        ("device".into(), json!(device)),
        ("task".into(), json!(task)),
        ("manifest".into(), val),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Local;

    const DEV: &str = "aaaaaaaaaaaaaaaa";

    #[test]
    fn task_root_segments() {
        assert_eq!(task_root("task-41/report.md").as_deref(), Some("task-41"));
        assert_eq!(task_root("report.md").as_deref(), Some("report.md"));
        assert_eq!(task_root(""), None);
    }

    #[test]
    fn update_and_read_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::open(dir.path(), DEV).unwrap();
        let task = "task-41";
        let payload = json!({
            "producer": {"agent_id": "a-1", "model": "claude-sonnet-4"},
            "parent_uris": ["store://artifacts/bbbbbbbbbbbbbbbb/task-40/out.json"],
            "summary": "Q2 report draft"
        });
        let committed = vec![
            json!({"path": "task-41/report.md", "sha256": "a".repeat(64), "size": 10}),
            json!({"path": "task-41/chart.png", "sha256": "b".repeat(64), "size": 20}),
        ];
        update(&local, DEV, "artifacts", task, &committed, Some(&payload), true).unwrap();

        let raw = fs::read_to_string(
            local
                .root
                .join(DEV)
                .join("artifacts")
                .join(task)
                .join(".nexuspouch")
                .join("manifest.json"),
        )
        .unwrap();
        let m: Manifest = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.task_id, "task-41");
        assert_eq!(m.device_id, DEV);
        assert_eq!(m.state, "published");
        assert_eq!(m.producer.as_ref().unwrap()["agent_id"], "a-1");
        assert_eq!(m.parent_uris.len(), 1);
        assert_eq!(m.summary.as_deref(), Some("Q2 report draft"));
        assert_eq!(m.files.len(), 2);

        // .nexuspouch is not addressable via normalize_path.
        assert!(crate::protocol::normalize_path(".nexuspouch/manifest.json").is_err());
    }
}
