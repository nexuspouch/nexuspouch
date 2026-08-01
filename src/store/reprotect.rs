//! Mirror-tree reprotect — ShePaw-compatible encrypted pack.
//!
//! Layout under `<store>/.system/reprotect/YYYYMMDD-HHMMSS/`:
//! - `manifest.json` (`kind: mirror_reprotect`)
//! - `mirror.tar.enc` (XChaCha20-Poly1305 of ustar)
//!
//! Without a password: writes integrity `manifest` only (mode=`manifest`).
//! Legacy packs under `<device>/backups/reprotect-*` are relocated on run.

use super::snapshot_crypto;
use super::{io_err, Local, OpError};
use crate::events::StoreEvent;
use crate::store::browse::file_sha;
use base64::Engine;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReprotectMode {
    /// Integrity listing only (no ciphertext).
    Manifest,
    /// Plain tree copy under `tree/` (debug / migration aid).
    Copy,
    /// ShePaw-compatible encrypted tar pack.
    Encrypt,
}

pub struct ReprotectOpts {
    pub mode: ReprotectMode,
    /// Required for [`ReprotectMode::Encrypt`].
    pub password: Option<String>,
}

impl Default for ReprotectOpts {
    fn default() -> Self {
        Self {
            mode: ReprotectMode::Manifest,
            password: None,
        }
    }
}

/// Resolve options from env + optional password override.
pub fn opts_from_env(password: Option<String>) -> ReprotectOpts {
    let copy = std::env::var("NEXUSPOUCH_REPROTECT_COPY")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let env_pw = std::env::var("NEXUSPOUCH_REPROTECT_PASSWORD").ok();
    let password = password.or(env_pw).filter(|s| !s.is_empty());
    let mode = if copy {
        ReprotectMode::Copy
    } else if password.is_some() {
        ReprotectMode::Encrypt
    } else {
        ReprotectMode::Manifest
    };
    ReprotectOpts { mode, password }
}

/// Build a reprotect snapshot under `.system/reprotect/`.
pub fn run(local: &Local, opts: ReprotectOpts) -> Result<Map<String, Value>, OpError> {
    local.require_master()?;
    let system_dir = local.root.join(".system").join("reprotect");
    fs::create_dir_all(&system_dir).map_err(io_err)?;
    relocate_legacy(local, &system_dir);
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let id = format!("reprotect-{ts}");
    let snap_dir = system_dir.join(&id);
    fs::create_dir_all(&snap_dir).map_err(io_err)?;

    let mut files_meta = Vec::new();
    let mut total_bytes: i64 = 0;
    let mut file_count: i64 = 0;
    let mut collected: Vec<(String, Vec<u8>)> = Vec::new();

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
                if space == "backups" {
                    let top = rel.split('/').next().unwrap_or("");
                    if top.starts_with("reprotect-") {
                        return;
                    }
                }
                let (sha, size) = file_sha(full);
                total_bytes += size;
                file_count += 1;
                files_meta.push(json!({
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
                match opts.mode {
                    ReprotectMode::Encrypt => {
                        if let Ok(bytes) = fs::read(full) {
                            let entry = format!("{name}/{space}/{rel}");
                            collected.push((entry, bytes));
                        }
                    }
                    ReprotectMode::Copy => {
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
                    ReprotectMode::Manifest => {}
                }
            });
        }
    }

    let mode_str = match opts.mode {
        ReprotectMode::Manifest => "manifest",
        ReprotectMode::Copy => "copy",
        ReprotectMode::Encrypt => "encrypt",
    };

    let mut manifest = json!({
        "kind": "mirror_reprotect",
        "id": id,
        "mode": mode_str,
        "created_at": now_ms(),
        "master_device": local.device_id,
        "file_count": file_count,
        "bytes": total_bytes,
    });

    match opts.mode {
        ReprotectMode::Encrypt => {
            let password = opts.password.ok_or_else(|| {
                OpError::new("invalid_argument", "reprotect encrypt requires password")
            })?;
            let tar_bytes = build_tar(&collected).map_err(|e| OpError::new("internal", e))?;
            let plain_sha = hex::encode(Sha256::digest(&tar_bytes));
            let salt = snapshot_crypto::new_snapshot_salt();
            let h = snapshot_crypto::hash_password(&password);
            let key = snapshot_crypto::derive_key_from_hash(&h, &salt);
            let enc = snapshot_crypto::encrypt(&tar_bytes, &key)
                .map_err(|e| OpError::new("internal", e))?;
            let enc_sha = hex::encode(Sha256::digest(&enc));
            fs::write(snap_dir.join("mirror.tar.enc"), &enc).map_err(io_err)?;
            if let Some(obj) = manifest.as_object_mut() {
                obj.insert("plain_sha256".into(), json!(plain_sha));
                obj.insert("enc_sha256".into(), json!(enc_sha));
                obj.insert(
                    "kdf_salt".into(),
                    json!(base64::engine::general_purpose::STANDARD.encode(salt)),
                );
                obj.insert(
                    "kdf_iterations".into(),
                    json!(snapshot_crypto::KDF_ITERATIONS),
                );
            }
        }
        ReprotectMode::Manifest | ReprotectMode::Copy => {
            if let Some(obj) = manifest.as_object_mut() {
                obj.insert("files".into(), json!(files_meta));
                obj.insert(
                    "note".into(),
                    json!(match opts.mode {
                        ReprotectMode::Manifest =>
                            "Integrity manifest only. Set NEXUSPOUCH_REPROTECT_PASSWORD or POST password for encrypted pack.",
                        ReprotectMode::Copy => "Plain tree copy under tree/.",
                        ReprotectMode::Encrypt => "",
                    }),
                );
            }
            fs::write(
                snap_dir.join("README.txt"),
                match opts.mode {
                    ReprotectMode::Manifest => {
                        "Nexuspouch reprotect snapshot (manifest-only).\n\
                         Provide NEXUSPOUCH_REPROTECT_PASSWORD or POST {\"password\":\"...\"} for ShePaw-compatible mirror.tar.enc.\n"
                    }
                    ReprotectMode::Copy => {
                        "Nexuspouch reprotect snapshot (plain tree copy).\n"
                    }
                    ReprotectMode::Encrypt => "",
                },
            )
            .map_err(io_err)?;
        }
    }

    fs::write(
        snap_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(io_err)?;

    prune_reprotect(local, &system_dir);

    local.emit_event(StoreEvent {
        id: uuid::Uuid::new_v4().to_string(),
        seq: 0,
        ts_ms: now_ms(),
        kind: "reprotect".into(),
        device: local.device_id.clone(),
        agent_id: None,
        space: None,
        path: Some(format!(".system/reprotect/{id}")),
        uri: None,
        detail: Map::from_iter([
            ("id".into(), json!(id)),
            ("mode".into(), json!(mode_str)),
            ("file_count".into(), json!(file_count)),
            ("bytes".into(), json!(total_bytes)),
        ]),
    });

    Ok(Map::from_iter([
        ("id".into(), json!(id)),
        ("files".into(), json!(file_count)),
        ("bytes".into(), json!(total_bytes)),
        ("mode".into(), json!(mode_str)),
        ("path".into(), json!(format!(".system/reprotect/{id}"))),
    ]))
}

/// Relocate legacy `<device>/backups/reprotect-*` packs into `.system/reprotect/`
/// (best-effort; the old user-visible location is no longer used).
fn relocate_legacy(local: &Local, system_dir: &Path) {
    let Ok(entries) = fs::read_dir(&local.root) else {
        return;
    };
    for ent in entries.flatten() {
        let name = ent.file_name().to_string_lossy().to_string();
        if !crate::protocol::is_valid_device_id(&name) {
            continue;
        }
        let backups = ent.path().join("backups");
        let Ok(packs) = fs::read_dir(&backups) else {
            continue;
        };
        for pack in packs.flatten() {
            let pack_name = pack.file_name().to_string_lossy().to_string();
            if !pack_name.starts_with("reprotect-") {
                continue;
            }
            let dest = system_dir.join(&pack_name);
            if !dest.exists() {
                let _ = fs::rename(pack.path(), &dest);
            } else {
                let _ = fs::remove_dir_all(pack.path());
            }
        }
    }
}

/// Keep the latest 4 reprotect packs; older ones move to `.recycle/<date>/...`
/// so the standard 30-day GC reclaims them.
fn prune_reprotect(local: &Local, system_dir: &Path) {
    let Ok(entries) = fs::read_dir(system_dir) else {
        return;
    };
    let mut packs: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            (n.starts_with("reprotect-") && e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .then(|| (n, e.path()))
        })
        .collect();
    packs.sort_by(|a, b| b.0.cmp(&a.0)); // newest first (timestamp names)
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    for (name, path) in packs.iter().skip(4) {
        let dest = local
            .root
            .join(".recycle")
            .join(&date)
            .join(&local.device_id)
            .join("system")
            .join(name);
        if let Some(parent) = dest.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::rename(path, &dest);
    }
}

fn build_tar(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).map_err(|e| e.to_string())?;
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append(&header, data.as_slice())
                .map_err(|e| e.to_string())?;
        }
        builder.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf)
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
        let path = dir.path().join(device).join("artifacts").join("task-1");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("a.txt"), b"hello").unwrap();

        let out = run(&store, ReprotectOpts::default()).unwrap();
        assert_eq!(out.get("files").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(out.get("mode").and_then(|v| v.as_str()), Some("manifest"));
        let id = out.get("id").and_then(|v| v.as_str()).unwrap();
        let manifest = dir
            .path()
            .join(".system")
            .join("reprotect")
            .join(id)
            .join("manifest.json");
        assert!(manifest.exists());
    }

    #[test]
    fn reprotect_encrypt_writes_pack() {
        let dir = tempdir().unwrap();
        let device = "bbbbbbbbbbbbbbbb";
        let store = Local::open(dir.path(), device).unwrap();
        let path = dir.path().join(device).join("files").join("m6");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("hello.txt"), b"hello-mirror").unwrap();

        let out = run(
            &store,
            ReprotectOpts {
                mode: ReprotectMode::Encrypt,
                password: Some("reprotect-pw".into()),
            },
        )
        .unwrap();
        assert_eq!(out.get("mode").and_then(|v| v.as_str()), Some("encrypt"));
        let id = out.get("id").and_then(|v| v.as_str()).unwrap();
        let snap = dir
            .path()
            .join(".system")
            .join("reprotect")
            .join(id);
        assert!(snap.join("mirror.tar.enc").exists());
        let manifest: Value =
            serde_json::from_slice(&fs::read(snap.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(
            manifest.get("kind").and_then(|v| v.as_str()),
            Some("mirror_reprotect")
        );
        assert!(manifest.get("kdf_salt").is_some());
        assert!(manifest.get("plain_sha256").is_some());
        assert!(manifest.get("enc_sha256").is_some());

        // Round-trip decrypt
        let salt_b64 = manifest.get("kdf_salt").and_then(|v| v.as_str()).unwrap();
        let salt_vec = base64::engine::general_purpose::STANDARD
            .decode(salt_b64)
            .unwrap();
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&salt_vec);
        let h = snapshot_crypto::hash_password("reprotect-pw");
        let key = snapshot_crypto::derive_key_from_hash(&h, &salt);
        let enc = fs::read(snap.join("mirror.tar.enc")).unwrap();
        let tar = snapshot_crypto::decrypt(&enc, &key).unwrap();
        assert_eq!(
            hex::encode(Sha256::digest(&tar)),
            manifest.get("plain_sha256").and_then(|v| v.as_str()).unwrap()
        );
    }

    #[test]
    fn reprotect_skips_existing_reprotect_dirs() {
        let dir = tempdir().unwrap();
        let device = "cccccccccccccccc";
        let store = Local::open(dir.path(), device).unwrap();
        let old = dir
            .path()
            .join(device)
            .join("backups")
            .join("reprotect-20000101-000000");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("mirror.tar.enc"), vec![0u8; 64 * 1024]).unwrap();
        fs::write(
            old.join("manifest.json"),
            br#"{"kind":"mirror_reprotect","file_count":0}"#,
        )
        .unwrap();
        let path = dir.path().join(device).join("files");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("only.txt"), b"x").unwrap();

        let out = run(
            &store,
            ReprotectOpts {
                mode: ReprotectMode::Encrypt,
                password: Some("skip-pw".into()),
            },
        )
        .unwrap();
        assert_eq!(out.get("files").and_then(|v| v.as_i64()), Some(1));
        // Legacy pack relocated out of user backups into .system/reprotect/.
        assert!(!old.exists());
        assert!(
            dir.path()
                .join(".system")
                .join("reprotect")
                .join("reprotect-20000101-000000")
                .exists()
        );
    }
}
