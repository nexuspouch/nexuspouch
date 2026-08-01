use super::{io_err, payload_int64, Local, OpError, MAX_CHUNK};
use crate::protocol::{self, Frame};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct Upload {
    pub device: String,
    pub space: String,
    pub path: String,
    pub tmp: PathBuf,
    pub meta_path: PathBuf,
    pub declared_size: i64,
    pub declared_sha: String,
    pub received: Mutex<i64>,
    pub write_mu: Mutex<()>,
}

#[derive(Serialize, Deserialize)]
struct StagingMeta {
    path: String,
    size: i64,
    sha256: String,
    created_ms: i64,
}

pub fn write_begin(local: &Local, frame: &Frame, caller: &str) -> Result<Map<String, Value>, OpError> {
    let path = frame.payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let norm = protocol::normalize_path(path).map_err(|e| OpError::new("bad_path", e))?;
    let space = frame.space().unwrap_or("");
    let size = super::num(frame.payload.get("size")) as i64;
    let sha = frame.payload.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
    if size < 0 {
        return Err(OpError::new("bad_op", "negative size"));
    }
    if sha.is_empty() || !is_hex_sha256(sha) {
        return Err(OpError::new("bad_op", "invalid sha256"));
    }

    let id = frame
        .payload
        .get("upload_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("u-{}", now_nanos()));
    check_upload_id(&id)?;

    let staging_dir = local.root.join(caller).join(space).join(".staging");
    fs::create_dir_all(&staging_dir).map_err(io_err)?;
    let part_path = staging_dir.join(format!("{id}.part"));
    let meta_path = staging_dir.join(format!("{id}.json"));

    if let Ok(raw) = fs::read_to_string(&meta_path) {
        if let Ok(meta) = serde_json::from_str::<StagingMeta>(&raw) {
            if meta.path != norm
                || meta.size != size
                || (!sha.is_empty() && !meta.sha256.is_empty() && meta.sha256 != sha)
            {
                return Err(OpError::new(
                    "staging_state",
                    "upload_id conflicts with existing",
                ));
            }
            let received = fs::metadata(&part_path).map(|m| m.len() as i64).unwrap_or(0);
            let u = Arc::new(Upload {
                device: caller.to_string(),
                space: space.to_string(),
                path: norm.clone(),
                tmp: part_path.clone(),
                meta_path: meta_path.clone(),
                declared_size: size,
                declared_sha: meta.sha256.clone(),
                received: Mutex::new(received),
                write_mu: Mutex::new(()),
            });
            local.uploads().lock().unwrap().insert(id.clone(), u);
            return Ok(Map::from_iter([
                ("upload_id".into(), json!(id)),
                ("received".into(), json!(received)),
            ]));
        }
    }

    let meta = StagingMeta {
        path: norm.clone(),
        size,
        sha256: sha.to_string(),
        created_ms: now_ms(),
    };
    fs::write(
        &meta_path,
        serde_json::to_vec(&meta).map_err(|e| OpError::new("internal", e.to_string()))?,
    )
    .map_err(io_err)?;
    OpenOptions::new()
        .create(true)
        .write(true)
        .open(&part_path)
        .map_err(io_err)?;

    let u = Arc::new(Upload {
        device: caller.to_string(),
        space: space.to_string(),
        path: norm,
        tmp: part_path,
        meta_path,
        declared_size: size,
        declared_sha: sha.to_string(),
        received: Mutex::new(0),
        write_mu: Mutex::new(()),
    });
    local.uploads().lock().unwrap().insert(id.clone(), u);
    Ok(Map::from_iter([
        ("upload_id".into(), json!(id)),
        ("received".into(), json!(0)),
    ]))
}

pub fn write_chunk(local: &Local, frame: &Frame) -> Result<Map<String, Value>, OpError> {
    let id = frame
        .payload
        .get("upload_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    check_upload_id(id)?;
    let data_b64 = frame.payload.get("data").and_then(|v| v.as_str()).unwrap_or("");
    let raw = STANDARD
        .decode(data_b64)
        .map_err(|_| OpError::new("bad_op", "bad base64"))?;
    if raw.len() > MAX_CHUNK {
        return Err(OpError::new("bad_op", "chunk too large"));
    }
    let offset = super::num(frame.payload.get("offset")) as i64;

    let u = local
        .uploads()
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or_else(|| OpError::new("staging_state", "unknown upload"))?;

    let _g = u.write_mu.lock().unwrap();
    let mut received = *u.received.lock().unwrap();

    if offset > received {
        return Err(OpError::new(
            "staging_state",
            format!("offset {offset} beyond received {received}"),
        ));
    }
    if u.declared_size > 0 && offset + raw.len() as i64 > u.declared_size {
        return Err(OpError::new("bad_op", "chunk exceeds declared size"));
    }

    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&u.tmp)
        .map_err(io_err)?;
    f.seek(SeekFrom::Start(offset as u64)).map_err(io_err)?;
    f.write_all(&raw).map_err(io_err)?;
    let end = offset + raw.len() as i64;
    if end > received {
        received = end;
        *u.received.lock().unwrap() = received;
    }
    Ok(Map::from_iter([
        ("received".into(), json!(received)),
        ("accepted".into(), json!(raw.len())),
    ]))
}

pub fn commit(local: &Local, frame: &Frame, caller: &str) -> Result<Map<String, Value>, OpError> {
    let ids: Vec<String> = frame
        .payload
        .get("upload_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .or_else(|| {
            frame
                .payload
                .get("upload_id")
                .and_then(|v| v.as_str())
                .map(|s| vec![s.to_string()])
        })
        .unwrap_or_default();

    struct Pending {
        id: String,
        u: Arc<Upload>,
        sum: String,
        size: i64,
    }

    let mut batch = Vec::new();
    for id in ids {
        check_upload_id(&id)?;
        let u = local
            .uploads()
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| OpError::new("staging_state", format!("unknown {id}")))?;
        let mut meta = StagingMeta {
            path: u.path.clone(),
            size: u.declared_size,
            sha256: u.declared_sha.clone(),
            created_ms: 0,
        };
        if let Ok(raw) = fs::read_to_string(&u.meta_path) {
            let _ = serde_json::from_str::<StagingMeta>(&raw).map(|m| meta = m);
        }
        let (sum, size) = file_sha_exact(&u.tmp)?;
        if meta.size >= 0 && size != meta.size {
            return Err(OpError::new("hash_mismatch", format!("{id}: size")));
        }
        if !meta.sha256.is_empty() && sum != meta.sha256 {
            return Err(OpError::new("hash_mismatch", id.clone()));
        }
        batch.push(Pending {
            id,
            u,
            sum,
            size,
        });
    }

    let retention_device = batch.first().map(|p| p.u.device.clone());
    let retention_space = batch.first().map(|p| p.u.space.clone());

    let mut committed = Vec::new();
    let mut failed = Vec::new();
    for p in batch {
        let dest = local
            .root
            .join(&p.u.device)
            .join(&p.u.space)
            .join(p.u.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = dest.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                failed.push(json!({"upload_id": p.id, "error": e.to_string()}));
                continue;
            }
        }
        let space_root = local.root.join(&p.u.device).join(&p.u.space);
        if let Err(e) = reject_symlink_under(&space_root, &dest) {
            failed.push(json!({"upload_id": p.id, "error": e.msg}));
            continue;
        }
        if dest.exists() {
            if let Err(e) = super::browse::move_to_recycle(local, &p.u.device, &p.u.space, &p.u.path) {
                failed.push(json!({"upload_id": p.id, "error": e.to_string()}));
                continue;
            }
        }
        if let Err(e) = fs::rename(&p.u.tmp, &dest) {
            failed.push(json!({"upload_id": p.id, "error": e.to_string()}));
            continue;
        }
        let _ = fs::remove_file(&p.u.meta_path);
        local.forget_upload(&p.id);
        committed.push(json!({
            "upload_id": p.id,
            "path": p.u.path,
            "size": p.size,
            "sha256": p.sum,
        }));
        let uri = crate::uri::StoreUri {
            space: p.u.space.clone(),
            device: p.u.device.clone(),
            path: p.u.path.clone(),
        };
        local.emit_event(
            crate::events::StoreEvent::new("commit", &p.u.device)
                .with_uri(uri.format())
                .with_path(&p.u.space, &p.u.path)
                .with_detail(Map::from_iter([
                    ("size".into(), json!(p.size)),
                    ("sha256".into(), json!(p.sum)),
                    ("upload_id".into(), json!(p.id)),
                ])),
        );
    }

    if !committed.is_empty() {
        if let (Some(device), Some(space)) = (retention_device, retention_space) {
            if let Some(retention) = frame.payload.get("retention") {
                super::retention::apply_retention(local, &device, &space, retention);
            }
        }
    }

    let mut out = Map::new();
    out.insert("files".into(), Value::Array(committed.clone()));
    out.insert("committed".into(), Value::Array(committed));
    out.insert("failed".into(), Value::Array(failed));

    if let Some(upto) = payload_int64(&frame.payload, "upto_seq") {
        if out.get("failed").and_then(|v| v.as_array()).map(|a| a.is_empty()).unwrap_or(false) {
            let applied = local.cursors().advance(caller, upto)?;
            out.insert("applied_seq".into(), json!(applied));
        }
    }
    Ok(out)
}

fn file_sha_exact(path: &PathBuf) -> Result<(String, i64), OpError> {
    let mut f = fs::File::open(path).map_err(io_err)?;
    let mut h = Sha256::new();
    let n = std::io::copy(&mut f, &mut h).map_err(io_err)?;
    Ok((hex::encode(h.finalize()), n as i64))
}

pub fn reject_symlink_under(space_root: &PathBuf, path: &PathBuf) -> Result<(), OpError> {
    let space_root = space_root.canonicalize().unwrap_or_else(|_| space_root.clone());
    let path_clean = path.clone();
    if path_clean != space_root
        && !path_clean.starts_with(&space_root)
        && path_clean.parent().map(|p| p != space_root).unwrap_or(true)
    {
        // For not-yet-existing paths, walk relative segments
    }
    if let Ok(meta) = fs::symlink_metadata(&space_root) {
        if meta.file_type().is_symlink() {
            return Err(OpError::new("bad_path", "symlink not allowed"));
        }
    }
    if let Ok(rel) = path_clean.strip_prefix(&space_root) {
        let mut cur = space_root.clone();
        for seg in rel.components() {
            if let std::path::Component::Normal(s) = seg {
                cur.push(s);
                if let Ok(meta) = fs::symlink_metadata(&cur) {
                    if meta.file_type().is_symlink() {
                        return Err(OpError::new("bad_path", "symlink not allowed"));
                    }
                }
            }
        }
    }
    Ok(())
}

fn is_hex_sha256(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

fn check_upload_id(id: &str) -> Result<(), OpError> {
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(OpError::new("bad_op", "invalid upload_id"));
    }
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}
