//! Version store (M2): immutable old versions under `<root>/.versions/`.
//!
//! Layout per archived file:
//! ```text
//! <root>/.versions/<device>/<space>/<relpath>/
//! ├── <sha256>            # immutable version file
//! ├── <sha256>.meta.json  # {original_path, created_at, producer, superseded_by}
//! └── index.json          # {"protected": bool, "versions": [{"v", "sha256", ...}]}
//! ```
//! The current (latest) file stays in the real tree; `versions.list` merges it
//! as the highest `v`. `.versions` is dot-prefixed, so external frames cannot
//! address it (normalize_path rejects dot segments) — access is internal only.

use super::{browse, io_err, Local, OpError};
use crate::protocol;
use crate::uri::RefKind;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_KEEP: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionEntry {
    pub v: u64,
    pub sha256: String,
    pub size: i64,
    pub mtime: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionIndex {
    #[serde(default)]
    pub protected: bool,
    #[serde(default)]
    pub versions: Vec<VersionEntry>,
}

fn version_dir(local: &Local, device: &str, space: &str, rel: &str) -> PathBuf {
    local
        .root
        .join(".versions")
        .join(device)
        .join(space)
        .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
}

fn index_path(local: &Local, device: &str, space: &str, rel: &str) -> PathBuf {
    version_dir(local, device, space, rel).join("index.json")
}

fn load_index(local: &Local, device: &str, space: &str, rel: &str) -> VersionIndex {
    let p = index_path(local, device, space, rel);
    fs::read_to_string(&p)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_index(
    local: &Local,
    device: &str,
    space: &str,
    rel: &str,
    index: &VersionIndex,
) -> Result<(), OpError> {
    let p = index_path(local, device, space, rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(index).map_err(|e| OpError::new("internal", e.to_string()))?)
        .map_err(io_err)?;
    fs::rename(&tmp, &p).map_err(io_err)
}

fn keep_last() -> usize {
    std::env::var("NEXUSPOUCH_VERSIONS_KEEP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_KEEP)
}

fn file_sha256(path: &Path) -> Result<(String, i64), OpError> {
    let mut f = fs::File::open(path).map_err(io_err)?;
    let mut h = Sha256::new();
    let n = std::io::copy(&mut f, &mut h).map_err(io_err)?;
    Ok((hex::encode(h.finalize()), n as i64))
}

fn mtime_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Move the current file (if any) into the version store before it is
/// overwritten. Returns the archived version number (`None` if there was
/// nothing to archive). Idempotent per sha256.
pub fn archive_old(
    local: &Local,
    space: &str,
    device: &str,
    rel: &str,
    superseded_by: &str,
    producer: Option<&Value>,
) -> Result<Option<u64>, OpError> {
    let full = browse::resolve_path(local, space, device, rel)?;
    if !full.is_file() {
        return Ok(None);
    }
    let (sha, size) = file_sha256(&full)?;
    let st = fs::metadata(&full).map_err(io_err)?;
    let vdir = version_dir(local, device, space, rel);
    fs::create_dir_all(&vdir).map_err(io_err)?;

    let mut index = load_index(local, device, space, rel);
    let next_v = index.versions.iter().map(|e| e.v).max().unwrap_or(0) + 1;

    let file_path = vdir.join(&sha);
    if !file_path.exists() {
        super::fsutil::rename_replace(&full, &file_path).map_err(io_err)?;
    } else {
        // Same content already archived: drop the duplicate current copy.
        let _ = fs::remove_file(&full);
    }
    let meta_path = vdir.join(format!("{sha}.meta.json"));
    if !meta_path.exists() {
        fs::write(
            &meta_path,
            serde_json::to_vec_pretty(&json!({
                "original_path": rel,
                "created_at": now_ms(),
                "producer": producer,
                "superseded_by": superseded_by,
            }))
            .map_err(|e| OpError::new("internal", e.to_string()))?,
        )
        .map_err(io_err)?;
    }
    index.versions.push(VersionEntry {
        v: next_v,
        sha256: sha.clone(),
        size,
        mtime: mtime_ms(&st),
        producer: producer.cloned(),
        superseded_by: Some(superseded_by.to_string()),
    });
    save_index(local, device, space, rel, &index)?;
    prune(local, device, space, rel, &mut index)?;
    Ok(Some(next_v))
}

/// Enforce the keep-last budget (unless the artifact was published, which
/// protects all versions). Pruned version files move to `.recycle/<date>/...`
/// so the standard 30-day GC can reclaim them.
fn prune(
    local: &Local,
    device: &str,
    space: &str,
    rel: &str,
    index: &mut VersionIndex,
) -> Result<(), OpError> {
    if index.protected {
        return Ok(());
    }
    let keep = keep_last();
    if index.versions.len() <= keep {
        return Ok(());
    }
    let vdir = version_dir(local, device, space, rel);
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    while index.versions.len() > keep {
        let oldest = index.versions.remove(0);
        let src = vdir.join(&oldest.sha256);
        let dest = local
            .root
            .join(".recycle")
            .join(&date)
            .join(device)
            .join(space)
            .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
            .join(&oldest.sha256);
        if let Some(parent) = dest.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::rename(&src, &dest);
        let _ = fs::remove_file(vdir.join(format!("{}.meta.json", oldest.sha256)));
    }
    save_index(local, device, space, rel, index)
}

/// Mark the version history of a path as protected (published artifacts keep
/// every version permanently).
pub fn mark_protected(local: &Local, space: &str, device: &str, rel: &str) -> Result<(), OpError> {
    let mut index = load_index(local, device, space, rel);
    if !index.protected {
        index.protected = true;
        save_index(local, device, space, rel, &index)?;
    }
    Ok(())
}

/// All versions (archived + current), ordered by `v`.
pub fn all_versions(
    local: &Local,
    space: &str,
    device: &str,
    rel: &str,
) -> Result<Vec<VersionEntry>, OpError> {
    let mut entries = load_index(local, device, space, rel).versions;
    let current = browse::resolve_path(local, space, device, rel)?;
    if current.is_file() {
        let (sha, size) = browse::file_sha(&current);
        let st = fs::metadata(&current).map_err(io_err)?;
        let next_v = entries.iter().map(|e| e.v).max().unwrap_or(0) + 1;
        entries.push(VersionEntry {
            v: next_v,
            sha256: sha,
            size,
            mtime: mtime_ms(&st),
            producer: None,
            superseded_by: None,
        });
    }
    entries.sort_by_key(|e| e.v);
    Ok(entries)
}

/// Resolve a ref to a concrete path (version file or current file).
pub fn resolve(
    local: &Local,
    space: &str,
    device: &str,
    rel: &str,
    ref_kind: &RefKind,
) -> Result<PathBuf, OpError> {
    match ref_kind {
        RefKind::Latest => browse::resolve_path(local, space, device, rel),
        RefKind::Seq(n) => {
            let entries = all_versions(local, space, device, rel)?;
            let vdir = version_dir(local, device, space, rel);
            let last_v = entries.last().map(|e| e.v);
            for e in &entries {
                if e.v == *n {
                    if last_v == Some(e.v) {
                        return browse::resolve_path(local, space, device, rel);
                    }
                    let p = vdir.join(&e.sha256);
                    if p.is_file() {
                        return Ok(p);
                    }
                    return Err(OpError::new("not_found", format!("no version v{n}")));
                }
            }
            Err(OpError::new("not_found", format!("no version v{n}")))
        }
        RefKind::Hash(prefix) => {
            let entries = all_versions(local, space, device, rel)?;
            let vdir = version_dir(local, device, space, rel);
            let current_sha = browse::resolve_path(local, space, device, rel)
                .ok()
                .filter(|p| p.is_file())
                .map(|p| browse::file_sha(&p).0);
            let mut matched: Vec<String> = Vec::new();
            for e in &entries {
                if e.sha256.starts_with(prefix) && !matched.contains(&e.sha256) {
                    matched.push(e.sha256.clone());
                }
            }
            match matched.len() {
                0 => Err(OpError::new("not_found", format!("no content {prefix}"))),
                1 => {
                    let sha = &matched[0];
                    let p = vdir.join(sha);
                    if p.is_file() {
                        Ok(p)
                    } else if current_sha.as_deref() == Some(sha.as_str()) {
                        browse::resolve_path(local, space, device, rel)
                    } else {
                        Err(OpError::new("not_found", format!("no content {prefix}")))
                    }
                }
                _ => Err(OpError::new(
                    "ambiguous_ref",
                    format!("hash prefix {prefix} matches {} versions", matched.len()),
                )),
            }
        }
    }
}

fn require_file_rel(rel: &str) -> Result<(), OpError> {
    if rel.is_empty() {
        return Err(OpError::new("bad_path", "versions require a file path"));
    }
    Ok(())
}

/// `versions.list` op: `{space, device, path}` -> versions + protected flag.
pub fn list(local: &Local, frame: &protocol::Frame, caller: &str) -> Result<Map<String, Value>, OpError> {
    let mut device = frame.device().unwrap_or("").to_string();
    if device.is_empty() {
        device = caller.to_string();
    }
    let space = frame.space().unwrap_or("");
    let path = frame.payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
    require_file_rel(path)?;
    let norm = protocol::normalize_path(path).map_err(|e| OpError::new("bad_path", e))?;
    let entries = all_versions(local, space, &device, &norm)?;
    let index = load_index(local, &device, space, &norm);
    Ok(Map::from_iter([
        ("space".into(), json!(space)),
        ("device".into(), json!(device)),
        ("path".into(), json!(norm)),
        ("protected".into(), json!(index.protected)),
        ("versions".into(), json!(entries)),
    ]))
}

/// `versions.read` op: `{space, device, path, ref, offset, length}`.
pub fn read(local: &Local, frame: &protocol::Frame, caller: &str) -> Result<Map<String, Value>, OpError> {
    let mut device = frame.device().unwrap_or("").to_string();
    if device.is_empty() {
        device = caller.to_string();
    }
    let space = frame.space().unwrap_or("");
    let path = frame.payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
    require_file_rel(path)?;
    let norm = protocol::normalize_path(path).map_err(|e| OpError::new("bad_path", e))?;
    let ref_kind = ref_from_frame(frame);
    let full = resolve(local, space, &device, &norm, &ref_kind)?;
    let mut f = fs::File::open(&full).map_err(|e| OpError::new("not_found", e.to_string()))?;
    let offset = super::num(frame.payload.get("offset")) as u64;
    let mut length = super::num(frame.payload.get("length")) as usize;
    if length == 0 || length > super::MAX_CHUNK {
        length = super::MAX_CHUNK;
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
        ("ref".into(), json!(ref_kind_to_str(&ref_kind))),
    ]))
}

/// Meta of a specific version (used by /uri/resolve with a ref).
pub fn meta(
    local: &Local,
    space: &str,
    device: &str,
    rel: &str,
    ref_kind: &RefKind,
) -> Result<Map<String, Value>, OpError> {
    let full = resolve(local, space, device, rel, ref_kind)?;
    let st = fs::metadata(&full).map_err(|e| OpError::new("not_found", e.to_string()))?;
    let (sha256, size) = browse::file_sha(&full);
    Ok(Map::from_iter([
        ("kind".into(), json!("file")),
        ("size".into(), json!(size)),
        ("sha256".into(), json!(sha256)),
        ("mtime".into(), json!(mtime_ms(&st))),
        ("ref".into(), json!(ref_kind_to_str(ref_kind))),
    ]))
}

fn ref_from_frame(frame: &protocol::Frame) -> RefKind {
    match frame.payload.get("ref").and_then(|v| v.as_str()) {
        Some("latest") => RefKind::Latest,
        Some(s) if s.len() >= 16 && s.chars().all(|c| c.is_ascii_hexdigit()) => {
            RefKind::Hash(s.to_ascii_lowercase())
        }
        Some(s) if s.starts_with('v') => s[1..]
            .parse::<u64>()
            .ok()
            .filter(|n| *n >= 1)
            .map(RefKind::Seq)
            .unwrap_or(RefKind::Latest),
        _ => RefKind::Latest,
    }
}

fn ref_kind_to_str(kind: &RefKind) -> String {
    match kind {
        RefKind::Latest => "latest".into(),
        RefKind::Hash(h) => h.clone(),
        RefKind::Seq(n) => format!("v{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Local;
    use std::sync::Mutex;

    const DEV: &str = "aaaaaaaaaaaaaaaa";
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn open() -> (tempfile::TempDir, Local) {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::open(dir.path(), DEV).unwrap();
        (dir, local)
    }

    fn write_current(local: &Local, space: &str, rel: &str, content: &str) -> String {
        let sha = hex::encode(Sha256::digest(content.as_bytes()));
        let full = browse::resolve_path(local, space, DEV, rel).unwrap();
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, content).unwrap();
        sha
    }

    #[test]
    fn archive_and_list_versions() {
        let (_d, local) = open();
        let space = "artifacts";
        let rel = "task-1/out.txt";

        write_current(&local, space, rel, "v1");
        let arch = archive_old(&local, space, DEV, rel, "sha-new-1", None).unwrap();
        assert_eq!(arch, Some(1));
        write_current(&local, space, rel, "v2");
        let arch = archive_old(&local, space, DEV, rel, "sha-new-2", None).unwrap();
        assert_eq!(arch, Some(2));
        write_current(&local, space, rel, "v3");

        let entries = all_versions(&local, space, DEV, rel).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].v, 1);
        assert_eq!(entries[1].v, 2);
        assert_eq!(entries[2].v, 3);

        // @v1 and @v2 resolve to archived content.
        let p1 = resolve(&local, space, DEV, rel, &RefKind::Seq(1)).unwrap();
        assert_eq!(fs::read_to_string(&p1).unwrap(), "v1");
        let p2 = resolve(&local, space, DEV, rel, &RefKind::Seq(2)).unwrap();
        assert_eq!(fs::read_to_string(&p2).unwrap(), "v2");
        let p3 = resolve(&local, space, DEV, rel, &RefKind::Seq(3)).unwrap();
        assert_eq!(fs::read_to_string(&p3).unwrap(), "v3");

        // Hash prefix resolution.
        let sha1 = hex::encode(Sha256::digest(b"v1"));
        let by_hash = resolve(&local, space, DEV, rel, &RefKind::Hash(sha1[..16].into())).unwrap();
        assert_eq!(fs::read_to_string(by_hash).unwrap(), "v1");

        // Unknown seq / hash -> not_found.
        assert_eq!(
            resolve(&local, space, DEV, rel, &RefKind::Seq(99))
                .unwrap_err()
                .code,
            "not_found"
        );
        assert_eq!(
            resolve(&local, space, DEV, rel, &RefKind::Hash("ffffffffffffffff".into()))
                .unwrap_err()
                .code,
            "not_found"
        );

        // .versions is not reachable via resolve_path (dot segments rejected).
        assert!(protocol::normalize_path(".versions/x").is_err());
    }

    #[test]
    fn prune_keeps_last_but_publish_protects() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("NEXUSPOUCH_VERSIONS_KEEP", "2");
        // keep=2: 4 commits -> archived v3,v4 kept, current v5.
        let (_d, local) = open();
        let space = "files";
        let rel = "a.txt";
        for i in 1..=4 {
            write_current(&local, space, rel, &format!("v{i}"));
            let _ = archive_old(&local, space, DEV, rel, &format!("new-{i}"), None).unwrap();
        }
        write_current(&local, space, rel, "v5");
        let entries = all_versions(&local, space, DEV, rel).unwrap();
        assert_eq!(entries.len(), 3, "got {entries:?}");
        assert_eq!(entries[0].v, 3);
        assert_eq!(entries[2].v, 5);
        // v1/v2 no longer resolvable.
        assert_eq!(
            resolve(&local, space, DEV, rel, &RefKind::Seq(1))
                .unwrap_err()
                .code,
            "not_found"
        );

        // Protected artifacts keep everything.
        let (_d2, local2) = open();
        let rel2 = "b.txt";
        mark_protected(&local2, space, DEV, rel2).unwrap();
        for i in 1..=4 {
            write_current(&local2, space, rel2, &format!("v{i}"));
            let _ = archive_old(&local2, space, DEV, rel2, &format!("new-{i}"), None).unwrap();
        }
        write_current(&local2, space, rel2, "v5");
        let entries = all_versions(&local2, space, DEV, rel2).unwrap();
        assert_eq!(entries.len(), 5);
    }

    #[test]
    fn hash_prefix_ambiguity() {
        let (_d, local) = open();
        let space = "files";
        let rel = "c.txt";
        let vdir = version_dir(&local, DEV, space, rel);
        fs::create_dir_all(&vdir).unwrap();
        // Two distinct files sharing a 16-hex prefix.
        let a = "aaaaaaaaaaaaaaaa000000000000000000000000000000000000000000000000";
        let b = "aaaaaaaaaaaaaaaa111111111111111111111111111111111111111111111111";
        fs::write(vdir.join(a), b"a").unwrap();
        fs::write(vdir.join(b), b"b").unwrap();
        let mut index = VersionIndex::default();
        index.versions.push(VersionEntry {
            v: 1,
            sha256: a.into(),
            size: 1,
            mtime: 0,
            producer: None,
            superseded_by: None,
        });
        index.versions.push(VersionEntry {
            v: 2,
            sha256: b.into(),
            size: 1,
            mtime: 0,
            producer: None,
            superseded_by: None,
        });
        save_index(&local, DEV, space, rel, &index).unwrap();
        let err = resolve(&local, space, DEV, rel, &RefKind::Hash("aaaaaaaaaaaaaaaa".into())).unwrap_err();
        assert_eq!(err.code, "ambiguous_ref");
    }

    fn commit_text(local: &Local, space: &str, rel: &str, content: &str, publish: bool) {
        let sha = hex::encode(Sha256::digest(content.as_bytes()));
        let mut begin = Map::new();
        begin.insert("space".into(), json!(space));
        begin.insert("path".into(), json!(rel));
        begin.insert("size".into(), json!(content.len() as i64));
        begin.insert("sha256".into(), json!(sha));
        let out = local
            .handle(
                crate::protocol::Frame::from_parts("write.begin", begin),
                DEV,
                "owner",
                true,
            )
            .unwrap();
        let upload_id = out["upload_id"].as_str().unwrap().to_string();
        let mut chunk = Map::new();
        chunk.insert("upload_id".into(), json!(upload_id));
        chunk.insert("offset".into(), json!(0));
        chunk.insert("data".into(), json!(STANDARD.encode(content.as_bytes())));
        local.handle(
            crate::protocol::Frame::from_parts("write.chunk", chunk),
            DEV,
            "owner",
            true,
        )
        .unwrap();
        let mut commit = Map::new();
        commit.insert("space".into(), json!(space));
        commit.insert("upload_ids".into(), json!([upload_id]));
        commit.insert("publish".into(), json!(publish));
        if publish {
            commit.insert(
                "manifest".into(),
                json!({"producer": {"agent_id": "a-1"}, "parent_uris": []}),
            );
        }
        let out = local
            .handle(
                crate::protocol::Frame::from_parts("commit", commit),
                DEV,
                "owner",
                true,
            )
            .unwrap();
        assert_eq!(out["committed"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn commit_versions_and_manifest_end_to_end() {
        let (_d, local) = open();
        let space = "artifacts";
        let rel = "task-1/out.txt";
        commit_text(&local, space, rel, "v1", false);
        commit_text(&local, space, rel, "v2", false);
        commit_text(&local, space, rel, "v3", true);

        // versions.list via store frames.
        let mut p = Map::new();
        p.insert("space".into(), json!(space));
        p.insert("device".into(), json!(DEV));
        p.insert("path".into(), json!(rel));
        let out = local
            .handle(
                crate::protocol::Frame::from_parts("versions.list", p),
                DEV,
                "owner",
                true,
            )
            .unwrap();
        let versions = out["versions"].as_array().unwrap();
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0]["v"], 1);
        assert_eq!(versions[2]["v"], 3);
        assert_eq!(out["protected"], true);

        // versions.read @v2 returns the old content.
        let mut p = Map::new();
        p.insert("space".into(), json!(space));
        p.insert("device".into(), json!(DEV));
        p.insert("path".into(), json!(rel));
        p.insert("ref".into(), json!("v2"));
        let out = local
            .handle(
                crate::protocol::Frame::from_parts("versions.read", p),
                DEV,
                "owner",
                true,
            )
            .unwrap();
        let data = STANDARD.decode(out["data"].as_str().unwrap()).unwrap();
        assert_eq!(String::from_utf8(data).unwrap(), "v2");

        // manifest op exposes lineage + published state.
        let mut p = Map::new();
        p.insert("space".into(), json!(space));
        p.insert("device".into(), json!(DEV));
        p.insert("path".into(), json!("task-1"));
        let out = local
            .handle(
                crate::protocol::Frame::from_parts("manifest", p),
                DEV,
                "owner",
                true,
            )
            .unwrap();
        assert_eq!(out["manifest"]["state"], "published");
        assert_eq!(out["manifest"]["producer"]["agent_id"], "a-1");
        assert_eq!(
            out["manifest"]["files"][0]["sha256"],
            hex::encode(Sha256::digest(b"v3"))
        );
    }
}
