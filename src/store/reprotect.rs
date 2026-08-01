use super::{io_err, Local, OpError};
use crate::events::StoreEvent;
use crate::store::browse::file_sha;
use crate::store::retention;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Build a manifest-only (or optional copy) reprotect snapshot under backups/.
pub fn run(local: &Local, copy_tree: bool) -> Result<Map<String, Value>, OpError> {
    local.require_master()?;
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let id = format!("reprotect-{ts}");
    let snap_rel = id.clone();
    let snap_dir = local
        .root
        .join(&local.device_id)
        .join("backups")
        .join(&snap_rel);
    fs::create_dir_all(&snap_dir).map_err(io_err)?;

    let mut files = Vec::new();
    let mut total_bytes: i64 = 0;
    let mut file_count: i64 = 0;

    for ent in fs::read_dir(&local.root).map_err(io_err)? {
        let ent = ent.map_err(io_err)?;
        let name = ent.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || !ent.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if !crate::protocol::is_valid_device_id(&name) {
            continue;
        }
        for space in ["artifacts", "files", "attachments", "backups"] {
            let space_root = ent.path().join(space);
            if !space_root.is_dir() {
                continue;
            }
            walk_files(&space_root, &space_root, &mut |rel, full, meta| {
                // Skip nesting reprotect snapshots into themselves.
                if name == local.device_id && space == "backups" && rel.starts_with("reprotect-") {
                    return;
                }
                let (sha, size) = file_sha(full);
                total_bytes += size;
                file_count += 1;
                files.push(json!({
                    "device": name,
                    "space": space,
                    "path": rel,
                    "size": size,
                    "sha256": sha,
                    "mtime": meta.modified().ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0),
                }));
                if copy_tree {
                    let dest = snap_dir
                        .join("tree")
                        .join(&name)
                        .join(space)
                        .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
                    if let Some(parent) = dest.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::copy(full, &dest);
                }
            });
        }
    }

    let mode = if copy_tree { "copy" } else { "manifest" };
    let manifest = json!({
        "id": id,
        "mode": mode,
        "master": local.device_id,
        "created_ms": now_ms(),
        "file_count": file_count,
        "bytes": total_bytes,
        "files": files,
        "note": "Encrypted packaging deferred; this is an integrity manifest snapshot.",
    });
    fs::write(
        snap_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(io_err)?;
    fs::write(
        snap_dir.join("README.txt"),
        "Nexuspouch reprotect snapshot (manifest). Full encrypted pack is not yet enabled.\n",
    )
    .map_err(io_err)?;

    let mut retention = Map::new();
    retention.insert("policy".into(), json!("keep_last"));
    retention.insert("keep".into(), json!(4));
    retention.insert("include_prefix".into(), json!("reprotect-"));
    retention::apply_retention(local, &local.device_id, "backups", &Value::Object(retention));

    local.emit_event(StoreEvent {
        id: uuid::Uuid::new_v4().to_string(),
        ts_ms: now_ms(),
        kind: "reprotect".into(),
        device: local.device_id.clone(),
        space: Some("backups".into()),
        path: Some(snap_rel.clone()),
        uri: Some(format!(
            "store://backups/{}/{}",
            local.device_id, snap_rel
        )),
        detail: Map::from_iter([
            ("id".into(), json!(id)),
            ("mode".into(), json!(mode)),
            ("file_count".into(), json!(file_count)),
            ("bytes".into(), json!(total_bytes)),
        ]),
    });

    Ok(Map::from_iter([
        ("id".into(), json!(id)),
        ("files".into(), json!(file_count)),
        ("bytes".into(), json!(total_bytes)),
        ("mode".into(), json!(mode)),
        ("path".into(), json!(format!("backups/{snap_rel}"))),
    ]))
}

fn walk_files(base: &Path, cur: &Path, f: &mut dyn FnMut(String, &Path, &fs::Metadata)) {
    let Ok(rd) = fs::read_dir(cur) else {
        return;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        let Ok(meta) = ent.metadata() else {
            continue;
        };
        let name = ent.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            if meta.is_dir() {
                continue;
            }
            continue;
        }
        if meta.is_dir() {
            walk_files(base, &path, f);
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(base)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if !rel.is_empty() {
                f(rel, &path, &meta);
            }
        }
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
    fn reprotect_writes_manifest() {
        let dir = tempdir().unwrap();
        let device = "aaaaaaaaaaaaaaaa";
        let store = Local::open(dir.path(), device).unwrap();
        let path = dir
            .path()
            .join(device)
            .join("artifacts")
            .join("task-1");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("a.txt"), b"hello").unwrap();

        let out = run(&store, false).unwrap();
        assert_eq!(out.get("files").and_then(|v| v.as_i64()), Some(1));
        let id = out.get("id").and_then(|v| v.as_str()).unwrap();
        let manifest = dir
            .path()
            .join(device)
            .join("backups")
            .join(id)
            .join("manifest.json");
        assert!(manifest.exists());
    }
}
