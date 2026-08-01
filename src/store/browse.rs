use super::{io_err, payload_int64, Local, OpError, MAX_CHUNK};
use crate::protocol::{self, Frame};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{NaiveDate, TimeZone, Utc};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub fn resolve_path(local: &Local, space: &str, device: &str, rel: &str) -> Result<PathBuf, OpError> {
    let device = if device.is_empty() {
        local.device_id.as_str()
    } else {
        device
    };
    let base = local.root.join(device).join(space);
    if rel.is_empty() || rel == "/" {
        reject_symlink_under(&base, &base)?;
        return Ok(base);
    }
    let norm = protocol::normalize_path(rel).map_err(|e| OpError::new("bad_path", e))?;
    let full = base.join(norm.replace('/', std::path::MAIN_SEPARATOR_STR));
    let base_with_sep = format!("{}{}", base.display(), std::path::MAIN_SEPARATOR);
    let full_str = full.to_string_lossy();
    if full != base && !full_str.starts_with(&base_with_sep) {
        return Err(OpError::new("bad_path", "escape"));
    }
    reject_symlink_under(&base, &full)?;
    Ok(full)
}

pub fn list(local: &Local, frame: &Frame, caller: &str) -> Result<Map<String, Value>, OpError> {
    local.require_import_grant(frame, caller)?;
    let mut device = frame.device().unwrap_or("").to_string();
    if device.is_empty() {
        device = caller.to_string();
    }
    let path = frame.payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let space = frame.space().unwrap_or("");
    let space_root = resolve_path(local, space, &device, "")?;
    let dir = resolve_path(local, space, &device, path)?;
    let limit = super::num(frame.payload.get("limit")) as usize;
    let limit = if limit == 0 { 1000 } else { limit };

    let mut entries = Vec::new();
    walk_files(&dir, &space_root, &mut entries, limit)?;
    Ok(Map::from_iter([("entries".into(), json!(entries))]))
}

pub fn meta(local: &Local, frame: &Frame, caller: &str) -> Result<Map<String, Value>, OpError> {
    local.require_import_grant(frame, caller)?;
    let mut device = frame.device().unwrap_or("").to_string();
    if device.is_empty() {
        device = caller.to_string();
    }
    let path = frame.payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let space = frame.space().unwrap_or("");
    let full = resolve_path(local, space, &device, path)?;
    let st = fs::metadata(&full).map_err(|e| OpError::new("not_found", e.to_string()))?;
    if st.is_dir() {
        return Ok(Map::from_iter([("kind".into(), json!("dir"))]));
    }
    let (sha256, size) = file_sha(&full);
    Ok(Map::from_iter([
        ("kind".into(), json!("file")),
        ("size".into(), json!(size)),
        ("sha256".into(), json!(sha256)),
        ("mtime".into(), json!(mtime_ms(&st))),
    ]))
}

pub fn read(local: &Local, frame: &Frame, caller: &str) -> Result<Map<String, Value>, OpError> {
    local.require_import_grant(frame, caller)?;
    let mut device = frame.device().unwrap_or("").to_string();
    if device.is_empty() {
        device = caller.to_string();
    }
    let path = frame.payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let space = frame.space().unwrap_or("");
    let full = resolve_path(local, space, &device, path)?;
    let mut f = fs::File::open(&full).map_err(|e| OpError::new("not_found", e.to_string()))?;
    let offset = super::num(frame.payload.get("offset")) as u64;
    let mut length = super::num(frame.payload.get("length")) as usize;
    if length == 0 || length > MAX_CHUNK {
        length = MAX_CHUNK;
    }
    if f.seek(SeekFrom::Start(offset)).is_err() {
        return Ok(Map::from_iter([
            ("data".into(), json!("")),
            ("size".into(), json!(0)),
            ("eof".into(), json!(true)),
        ]));
    }
    let mut buf = vec![0u8; length];
    let n = f.read(&mut buf).unwrap_or(0);
    buf.truncate(n);
    let eof = n < length;
    Ok(Map::from_iter([
        ("data".into(), json!(STANDARD.encode(&buf))),
        ("size".into(), json!(n)),
        ("eof".into(), json!(eof)),
    ]))
}

pub fn delete(local: &Local, frame: &Frame, caller: &str) -> Result<Map<String, Value>, OpError> {
    let mut device = frame.device().unwrap_or("").to_string();
    if device.is_empty() {
        device = caller.to_string();
    }
    let path = frame.payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let space = frame.space().unwrap_or("");
    let norm = protocol::normalize_path(path).map_err(|e| OpError::new("bad_path", e))?;
    let mut out = Map::new();
    match move_to_recycle(local, &device, space, &norm) {
        Ok(recycled) => {
            out.insert("recycled".into(), json!(recycled));
            local.remove_index(&format!("store://{space}/{device}/{norm}"));
            emit_delete_event(local, space, &device, &norm);
        }
        Err(e) if e.code == "not_found" => {
            out.insert("recycled".into(), json!(""));
            out.insert("already_gone".into(), json!(true));
            local.remove_index(&format!("store://{space}/{device}/{norm}"));
            emit_delete_event(local, space, &device, &norm);
        }
        Err(e) => return Err(e),
    }
    if let Some(upto) = payload_int64(&frame.payload, "upto_seq") {
        let applied = local.cursors().advance(caller, upto)?;
        out.insert("applied_seq".into(), json!(applied));
    }
    Ok(out)
}

fn emit_delete_event(local: &Local, space: &str, device: &str, path: &str) {
    let uri = crate::uri::StoreUri {
        space: space.to_string(),
        device: device.to_string(),
        path: path.to_string(),
        ref_kind: crate::uri::RefKind::Latest,
    };
    local.emit_event(
        crate::events::StoreEvent::new("delete", device)
            .with_uri(uri.format())
            .with_path(space, path),
    );
}

pub fn stats(local: &Local) -> Result<Map<String, Value>, OpError> {
    let mut devices = Map::new();
    for entry in fs::read_dir(&local.root).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.file_type().map_err(io_err)?.is_dir() || !protocol::is_valid_device_id(&name) {
            continue;
        }
        let mut per_space = Map::new();
        for sp in ["artifacts", "files", "attachments", "backups"] {
            per_space.insert(
                sp.into(),
                json!(dir_size(&local.root.join(&name).join(sp), true)),
            );
        }
        devices.insert(name.clone(), Value::Object(per_space));
    }
    let mut staging_bytes = 0i64;
    for key in devices.keys() {
        for sp in ["artifacts", "files", "attachments", "backups"] {
            staging_bytes += dir_size(&local.root.join(key).join(sp).join(".staging"), false);
        }
    }
    let mut out = Map::new();
    out.insert("devices".into(), Value::Object(devices));
    out.insert("staging_bytes".into(), json!(staging_bytes));
    out.insert(
        "recycle_bytes".into(),
        json!(dir_size(&local.root.join(".recycle"), false)),
    );
    if let Ok(p) = local.load_pointer() {
        out.insert("master".into(), json!(p.master));
        out.insert("master_epoch".into(), json!(p.epoch));
    }
    if let Ok(cursors) = local.cursors().all() {
        if !cursors.is_empty() {
            out.insert(
                "device_cursors".into(),
                json!(cursors),
            );
        }
    }
    if let Some((total, free)) = super::volume::probe_volume(&local.root) {
        let mut used_ratio = 0.0;
        if total > 0 {
            let mut used = total - free;
            if used < 0 {
                used = 0;
            }
            if used > total {
                used = total;
            }
            used_ratio = used as f64 / total as f64;
        }
        out.insert("volume_total_bytes".into(), json!(total));
        out.insert("volume_free_bytes".into(), json!(free));
        out.insert("volume_used_ratio".into(), json!(used_ratio));
        out.insert("volume_warn".into(), json!(used_ratio >= 0.8));
    }
    Ok(out)
}

pub fn recycle_list(local: &Local) -> Result<Map<String, Value>, OpError> {
    let dir = local.root.join(".recycle");
    let mut out = Vec::new();
    walk_recycle(&local.root, &dir, &mut out);
    out.sort_by(|a, b| {
        let ai = a.get("deleted_at").and_then(|v| v.as_i64()).unwrap_or(0);
        let aj = b.get("deleted_at").and_then(|v| v.as_i64()).unwrap_or(0);
        aj.cmp(&ai)
    });
    Ok(Map::from_iter([("entries".into(), json!(out))]))
}

pub fn recycle_restore(local: &Local, frame: &Frame) -> Result<Map<String, Value>, OpError> {
    let recycle_path = frame
        .payload
        .get("recycle_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let normalized = recycle_path.replace('\\', "/");
    if !normalized.starts_with(".recycle/") || normalized.contains("..") {
        return Err(OpError::new("bad_path", "invalid recycle path"));
    }
    let abs = local
        .root
        .join(normalized.replace('/', std::path::MAIN_SEPARATOR_STR));
    if !abs.exists() {
        return Err(OpError::new("not_found", recycle_path));
    }
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.len() < 5 {
        return Err(OpError::new("bad_path", "malformed recycle path"));
    }
    let device = parts[2];
    let space = parts[3];
    let mut origin_parts: Vec<String> = parts[4..].iter().map(|s| s.to_string()).collect();
    if let Some(last) = origin_parts.last_mut() {
        *last = strip_recycle_suffix(last);
    }
    let origin_rel = origin_parts.join("/");
    let dest = resolve_path(local, space, device, &origin_rel)?;
    if dest.exists() {
        move_to_recycle(local, device, space, &origin_rel)?;
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    fs::rename(&abs, &dest).map_err(io_err)?;
    prune_empty_recycle_dirs(local);
    Ok(Map::from_iter([("restored".into(), json!(origin_rel))]))
}

pub fn recycle_empty(local: &Local) -> Result<Map<String, Value>, OpError> {
    let dir = local.root.join(".recycle");
    let purged = dir_size(&dir, false);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(io_err)?;
    local.emit_event(
        crate::events::StoreEvent::new("recycle.empty", &local.device_id)
            .with_detail(Map::from_iter([("purged_bytes".into(), json!(purged))])),
    );
    Ok(Map::from_iter([("purged_bytes".into(), json!(purged))]))
}

pub fn move_to_recycle(
    local: &Local,
    device: &str,
    space: &str,
    normalized_rel: &str,
) -> Result<String, OpError> {
    let full = resolve_path(local, space, device, normalized_rel)?;
    if !full.exists() {
        return Err(OpError::new("not_found", "not found"));
    }
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut recycle_rel = format!(".recycle/{date}/{device}/{space}/{normalized_rel}");
    let mut dest = local
        .root
        .join(recycle_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    if dest.exists() {
        recycle_rel = format!("{recycle_rel}~{}", now_ms());
        dest = local
            .root
            .join(recycle_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    fs::rename(&full, &dest).map_err(io_err)?;
    Ok(recycle_rel)
}

pub fn admin_list(
    local: &Local,
    device: &str,
    space: &str,
    path: &str,
) -> Result<Map<String, Value>, OpError> {
    if !protocol::is_valid_device_id(device) {
        return Err(OpError::new("bad_path", "invalid device id"));
    }
    if !protocol::is_valid_space(space) {
        return Err(OpError::new("bad_op", "invalid space"));
    }
    let dir = resolve_path(local, space, device, path)?;
    let st = match fs::metadata(&dir) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Map::from_iter([("entries".into(), json!([]))]));
        }
        Err(e) => return Err(io_err(e)),
    };
    if !st.is_dir() {
        let (sha256, size) = file_sha(&dir);
        let rel = if path.is_empty() {
            dir.file_name().unwrap().to_string_lossy().to_string()
        } else {
            protocol::normalize_path(path).unwrap_or_else(|_| path.to_string())
        };
        return Ok(Map::from_iter([(
            "entries".into(),
            json!([{
                "path": rel,
                "size": size,
                "sha256": sha256,
                "mtime": mtime_ms(&st),
            }]),
        )]));
    }
    let mut entries = Vec::new();
    walk_files(&dir, &dir, &mut entries, usize::MAX)?;
    if !path.is_empty() {
        if let Ok(base) = protocol::normalize_path(path) {
            for e in &mut entries {
                if let Some(p) = e.get_mut("path").and_then(|v| v.as_str()) {
                    let combined = format!("{}/{}", base.trim_end_matches('/'), p.trim_start_matches('/'));
                    *e.get_mut("path").unwrap() = Value::String(combined.trim_start_matches('/').to_string());
                }
            }
        }
    }
    Ok(Map::from_iter([("entries".into(), json!(entries))]))
}

pub fn admin_delete(local: &Local, device: &str, space: &str, path: &str) -> Result<Map<String, Value>, OpError> {
    if !protocol::is_valid_device_id(device) {
        return Err(OpError::new("bad_path", "invalid device id"));
    }
    if !protocol::is_valid_space(space) {
        return Err(OpError::new("bad_op", "invalid space"));
    }
    let norm = protocol::normalize_path(path).map_err(|e| OpError::new("bad_path", e))?;
    let recycled = move_to_recycle(local, device, space, &norm)?;
    Ok(Map::from_iter([("recycled".into(), json!(recycled))]))
}

fn walk_files(
    dir: &Path,
    space_root: &Path,
    entries: &mut Vec<Map<String, Value>>,
    limit: usize,
) -> Result<(), OpError> {
    if entries.len() >= limit {
        return Ok(());
    }
    let read_dir = match fs::read_dir(dir) {
        Ok(d) => d,
        Err(_) => return Ok(()),
    };
    for entry in read_dir.flatten() {
        if entries.len() >= limit {
            break;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            if name.starts_with('.') || name == ".staging" {
                continue;
            }
            walk_files(&path, space_root, entries, limit)?;
        } else if !name.starts_with('.') {
            let rel = path
                .strip_prefix(space_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            let (sha256, size) = file_sha(&path);
            entries.push(Map::from_iter([
                ("path".into(), json!(rel)),
                ("size".into(), json!(size)),
                ("sha256".into(), json!(sha256)),
                ("mtime".into(), json!(mtime_ms(&meta))),
            ]));
        }
    }
    Ok(())
}

fn walk_recycle(root: &Path, dir: &Path, out: &mut Vec<Map<String, Value>>) {
    let read_dir = match fs::read_dir(dir) {
        Ok(d) => d,
        Err(_) => return,
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            walk_recycle(root, &path, out);
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            let parts: Vec<&str> = rel.split('/').collect();
            if parts.len() < 5 || parts[0] != ".recycle" {
                continue;
            }
            let date = parts[1];
            let device = parts[2];
            let space = parts[3];
            let mut origin_parts: Vec<String> = parts[4..].iter().map(|s| s.to_string()).collect();
            if let Some(last) = origin_parts.last_mut() {
                *last = strip_recycle_suffix(last);
            }
            out.push(Map::from_iter([
                ("recycle_path".into(), json!(rel)),
                ("origin_device".into(), json!(device)),
                ("space".into(), json!(space)),
                ("origin_path".into(), json!(origin_parts.join("/"))),
                ("size".into(), json!(meta.len() as i64)),
                (
                    "deleted_at".into(),
                    json!(parse_recycle_date_ms(date, mtime_ms(&meta))),
                ),
            ]));
        }
    }
}

fn prune_empty_recycle_dirs(local: &Local) {
    let root = local.root.join(".recycle");
    let mut dirs = Vec::new();
    collect_dirs(&root, &root, &mut dirs);
    dirs.sort_by(|a, b| b.cmp(a));
    for d in dirs {
        let _ = fs::remove_dir(d);
    }
}

fn collect_dirs(root: &Path, cur: &Path, dirs: &mut Vec<PathBuf>) {
    if let Ok(rd) = fs::read_dir(cur) {
        for entry in rd.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) && entry.path() != root {
                dirs.push(entry.path());
                collect_dirs(root, &entry.path(), dirs);
            }
        }
    }
}

pub fn dir_size(path: &Path, skip_dot_dirs: bool) -> i64 {
    let mut total = 0i64;
    if !path.exists() {
        return 0;
    }
    let _ = walk_dir_size(path, path, skip_dot_dirs, &mut total);
    total
}

fn walk_dir_size(base: &Path, path: &Path, skip_dot_dirs: bool, total: &mut i64) -> Result<(), ()> {
    for entry in fs::read_dir(path).map_err(|_| ())? {
        let entry = entry.map_err(|_| ())?;
        let meta = entry.metadata().map_err(|_| ())?;
        if meta.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if skip_dot_dirs && name.starts_with('.') && path != base {
                continue;
            }
            walk_dir_size(base, &entry.path(), skip_dot_dirs, total)?;
        } else {
            *total += meta.len() as i64;
        }
    }
    Ok(())
}

pub fn file_sha(path: &Path) -> (String, i64) {
    let mut f = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (String::new(), 0),
    };
    let mut h = Sha256::new();
    let n = std::io::copy(&mut f, &mut h).unwrap_or(0);
    (hex::encode(h.finalize()), n as i64)
}

fn mtime_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn strip_recycle_suffix(name: &str) -> String {
    if let Some(i) = name.rfind('~') {
        let suffix = &name[i + 1..];
        if suffix.len() >= 10 && suffix.chars().all(|c| c.is_ascii_digit()) {
            return name[..i].to_string();
        }
    }
    name.to_string()
}

fn parse_recycle_date_ms(date: &str, fallback: i64) -> i64 {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| Utc.from_utc_datetime(&dt).timestamp_millis())
        .unwrap_or(fallback)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn reject_symlink_under(space_root: &Path, path: &Path) -> Result<(), OpError> {
    super::write::reject_symlink_under(&space_root.to_path_buf(), &path.to_path_buf())
}
