use super::{Local, OpError};
use crate::protocol;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Deserialize)]
struct StagingMeta {
    created_ms: i64,
}

pub fn gc_staging(local: &Local, older_than: Duration) -> Result<usize, OpError> {
    let older_than = if older_than.is_zero() {
        Duration::from_secs(24 * 3600)
    } else {
        older_than
    };
    let deadline = SystemTime::now()
        .checked_sub(older_than)
        .unwrap_or(UNIX_EPOCH);
    let mut removed = 0usize;
    let ents = match fs::read_dir(&local.root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(super::io_err(e)),
    };
    for e in ents.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) || !protocol::is_valid_device_id(&name) {
            continue;
        }
        for sp in ["artifacts", "files", "attachments", "backups"] {
            let staging = local.root.join(&name).join(sp).join(".staging");
            let uploads = match fs::read_dir(&staging) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let mut seen = std::collections::HashSet::new();
            for u in uploads.flatten() {
                let fname = u.file_name().to_string_lossy().to_string();
                let path = u.path();
                if u.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    if let Ok(meta) = u.metadata() {
                        if meta.modified().unwrap_or(UNIX_EPOCH) < deadline {
                            if fs::remove_dir_all(&path).is_ok() {
                                removed += 1;
                                local.forget_upload(&fname);
                            }
                        }
                    }
                    continue;
                }
                let id = staging_session_id(&fname);
                if id.is_empty() || !seen.insert(id.clone()) {
                    continue;
                }
                let created = staging_created(&staging, &id, &path);
                if created < deadline {
                    let _ = fs::remove_file(staging.join(format!("{id}.part")));
                    let _ = fs::remove_file(staging.join(format!("{id}.json")));
                    removed += 1;
                    local.forget_upload(&id);
                }
            }
        }
    }
    Ok(removed)
}

pub fn gc_recycle(local: &Local, older_than: Duration) -> Result<i64, OpError> {
    let older_than = if older_than.is_zero() {
        Duration::from_secs(30 * 24 * 3600)
    } else {
        older_than
    };
    let recycle_dir = local.root.join(".recycle");
    let ents = match fs::read_dir(&recycle_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(super::io_err(e)),
    };
    let now = chrono::Local::now().date_naive();
    let cutoff = now - chrono::Duration::days(older_than.as_secs() as i64 / 86400);
    let mut purged = 0i64;
    for e in ents.flatten() {
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        let parsed = match chrono::NaiveDate::parse_from_str(&name, "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => continue,
        };
        if parsed >= cutoff {
            continue;
        }
        let path = recycle_dir.join(&name);
        purged += super::browse::dir_size(&path, false);
        let _ = fs::remove_dir_all(&path);
    }
    Ok(purged)
}

fn staging_session_id(name: &str) -> String {
    if let Some(id) = name.strip_suffix(".json") {
        id.to_string()
    } else if let Some(id) = name.strip_suffix(".part") {
        id.to_string()
    } else {
        String::new()
    }
}

fn staging_created(staging_dir: &PathBuf, id: &str, fallback_path: &PathBuf) -> SystemTime {
    let meta_path = staging_dir.join(format!("{id}.json"));
    if let Ok(raw) = fs::read_to_string(&meta_path) {
        if let Ok(meta) = serde_json::from_str::<StagingMeta>(&raw) {
            if meta.created_ms > 0 {
                return UNIX_EPOCH + Duration::from_millis(meta.created_ms as u64);
            }
        }
    }
    fs::metadata(fallback_path)
        .and_then(|m| m.modified())
        .unwrap_or(UNIX_EPOCH)
}
