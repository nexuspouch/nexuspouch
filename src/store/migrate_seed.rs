// PeerRpc / PeerEnsure are defined in store::mod and re-exported here.
use super::{browse, io_err, any_to_i64, Local, OpError, MAX_CHUNK, PeerEnsure, PeerRpc};
use crate::protocol;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Arc;
use tracing::warn;

const MIRROR_SEED_LIST_LIMIT: usize = 50_000;
const MAX_REPORTED_HASH_MISMATCHES: usize = 50;

pub fn master_migrate(
    local: &Local,
    frame: &crate::protocol::Frame,
) -> Result<Map<String, Value>, OpError> {
    let cur = super::master::load_pointer(local)?;
    let require_match = frame
        .payload
        .get("require_hash_match")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let old_master = cur.master.clone();
    let mut reachable = false;
    let mut seeded_files = 0i64;
    let mut hash_gate = skipped_hash_gate();
    let mut dial_err = String::new();

    if let Some(rpc) = local.peer_rpc() {
        if protocol::is_valid_device_id(&old_master) && old_master != local.device_id {
            let mut release_ensure: Option<Arc<dyn PeerEnsure>> = None;
            if !rpc.has(&old_master) {
                if let Some(ensure) = local.peer_ensure() {
                    match ensure.ensure(&old_master) {
                        Err(e) => {
                            warn!("master.migrate dial old master {old_master}: {e}");
                            dial_err = e;
                        }
                        Ok(()) => release_ensure = Some(ensure),
                    }
                }
            }
            if rpc.has(&old_master) {
                match seed_from_live_master(local, rpc.as_ref(), &old_master) {
                    Ok((n, device_ids)) => {
                        reachable = true;
                        seeded_files = n;
                        hash_gate = run_hash_gate(local, rpc.as_ref(), &old_master, &device_ids);
                        if require_match {
                            let ok = hash_gate
                                .get("ok")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            if !ok {
                                return Err(OpError::new(
                                    "hash_gate",
                                    "hash gate failed: mismatches present",
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        warn!("master.migrate seed from {old_master}: {e}");
                    }
                }
            }
            if let Some(ensure) = release_ensure {
                ensure.release(&old_master);
            }
        }
    }

    let mut epoch = cur.epoch + 1;
    if epoch < 1 {
        epoch = 1;
    }
    let next = super::master::MasterPointer {
        master: local.device_id.clone(),
        epoch,
    };
    super::master::save_pointer(local, &next)?;
    let cursors_raw = local.cursors().all().unwrap_or_default();
    let mut cursors = Map::new();
    for (k, v) in cursors_raw {
        cursors.insert(k, json!(v));
    }
    let mut out = Map::new();
    out.insert("master".into(), json!(next.master));
    out.insert("epoch".into(), json!(next.epoch));
    out.insert("old_master_reachable".into(), json!(reachable));
    out.insert("cursors".into(), Value::Object(cursors));
    out.insert("broadcast_peers".into(), json!(0));
    out.insert("seeded_files".into(), json!(seeded_files));
    out.insert("hash_gate".into(), Value::Object(hash_gate));
    if !dial_err.is_empty() {
        out.insert("dial_error".into(), json!(dial_err));
    }
    Ok(out)
}

pub fn merge_cursors(local: &Local, seed: HashMap<String, i64>) -> Result<HashMap<String, i64>, OpError> {
    if seed.is_empty() {
        return local.cursors().all();
    }
    for (device_id, seq) in seed {
        if !protocol::is_valid_device_id(&device_id) || seq <= 0 {
            continue;
        }
        local.cursors().advance(&device_id, seq)?;
    }
    local.cursors().all()
}

fn skipped_hash_gate() -> Map<String, Value> {
    Map::from_iter([
        ("ran".into(), json!(false)),
        ("ok".into(), json!(true)),
        ("devices".into(), json!([])),
        ("mismatches".into(), json!([])),
        ("mismatch_count".into(), json!(0)),
    ])
}

fn seed_from_live_master(
    local: &Local,
    rpc: &dyn PeerRpc,
    old_master: &str,
) -> Result<(i64, Vec<String>), OpError> {
    let cursors_data = rpc
        .call(old_master, "sync.cursors", Map::new())
        .map_err(|e| OpError::new("internal", e))?;
    let mut seed = HashMap::new();
    if let Some(raw) = cursors_data.get("cursors").and_then(|v| v.as_object()) {
        for (k, v) in raw {
            seed.insert(k.clone(), any_to_i64(v).unwrap_or(0));
        }
    }

    let mut device_set = HashSet::new();
    for id in seed.keys() {
        device_set.insert(id.clone());
    }
    if let Ok(stats) = rpc.call(old_master, "stats", Map::new()) {
        if let Some(devices) = stats.get("devices").and_then(|v| v.as_object()) {
            for id in devices.keys() {
                device_set.insert(id.clone());
            }
        }
    }

    let mut device_ids = Vec::new();
    for id in device_set {
        if protocol::is_valid_device_id(&id) && id != local.device_id {
            device_ids.push(id);
        }
    }

    let mut written = 0i64;
    let mut completed = HashSet::new();
    completed.insert(local.device_id.clone());
    for device_id in &device_ids {
        let mut device_ok = true;
        for space in ["artifacts", "files", "attachments", "backups"] {
            match seed_space(local, rpc, old_master, device_id, space) {
                Ok(n) => written += n,
                Err(e) => {
                    warn!("seed {device_id}/{space}: {e}");
                    device_ok = false;
                }
            }
        }
        if device_ok {
            completed.insert(device_id.clone());
        }
    }

    let mut filtered = HashMap::new();
    for (id, seq) in seed {
        if completed.contains(&id) {
            filtered.insert(id, seq);
        }
    }
    merge_cursors(local, filtered)?;

    let out_ids: Vec<String> = completed
        .into_iter()
        .filter(|id| *id != local.device_id)
        .collect();
    Ok((written, out_ids))
}

fn run_hash_gate(
    local: &Local,
    rpc: &dyn PeerRpc,
    old_master: &str,
    device_ids: &[String],
) -> Map<String, Value> {
    let mut devices_out = Vec::new();
    let mut mismatches = Vec::new();
    for device_id in device_ids {
        if !protocol::is_valid_device_id(device_id) || device_id == &local.device_id {
            continue;
        }
        let mut remote_map = HashMap::new();
        let mut local_map = HashMap::new();
        for space in ["artifacts", "files", "attachments", "backups"] {
            for (k, sha) in list_remote_shas(rpc, old_master, device_id, space) {
                remote_map.insert(format!("{space}/{k}"), sha);
            }
            for (k, sha) in list_local_shas(local, device_id, space) {
                local_map.insert(format!("{space}/{k}"), sha);
            }
        }
        let remote_digest = digest_path_map(&remote_map);
        let local_digest = digest_path_map(&local_map);
        devices_out.push(json!({
            "device": device_id,
            "remote_digest": remote_digest,
            "local_digest": local_digest,
            "matched": remote_digest == local_digest,
        }));

        let mut keys = HashSet::new();
        for k in remote_map.keys().chain(local_map.keys()) {
            keys.insert(k.clone());
        }
        let mut key_list: Vec<_> = keys.into_iter().collect();
        key_list.sort();
        for key in key_list {
            if mismatches.len() >= MAX_REPORTED_HASH_MISMATCHES {
                break;
            }
            let (space, path) = match key.split_once('/') {
                Some((s, p)) => (s, p),
                None => (key.as_str(), ""),
            };
            let r = remote_map.get(&key);
            let loc = local_map.get(&key);
            match (r, loc) {
                (Some(r), None) => mismatches.push(json!({
                    "device": device_id, "space": space, "path": path,
                    "kind": "missing_local", "remote_sha256": r,
                })),
                (None, Some(loc)) => mismatches.push(json!({
                    "device": device_id, "space": space, "path": path,
                    "kind": "missing_remote", "local_sha256": loc,
                })),
                (Some(r), Some(loc)) if r != loc => mismatches.push(json!({
                    "device": device_id, "space": space, "path": path,
                    "kind": "hash_mismatch", "remote_sha256": r, "local_sha256": loc,
                })),
                _ => {}
            }
        }
    }
    let ok = mismatches.is_empty();
    let mismatch_count = mismatches.len();
    Map::from_iter([
        ("ran".into(), json!(true)),
        ("ok".into(), json!(ok)),
        ("devices".into(), Value::Array(devices_out)),
        ("mismatches".into(), Value::Array(mismatches)),
        ("mismatch_count".into(), json!(mismatch_count)),
    ])
}

fn list_remote_shas(
    rpc: &dyn PeerRpc,
    old_master: &str,
    device_id: &str,
    space: &str,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut payload = Map::new();
    payload.insert("space".into(), json!(space));
    payload.insert("device".into(), json!(device_id));
    payload.insert("path".into(), json!(""));
    payload.insert("seed".into(), json!(true));
    payload.insert("limit".into(), json!(MIRROR_SEED_LIST_LIMIT));
    let res = match rpc.call(old_master, "list", payload) {
        Ok(r) => r,
        Err(e) => {
            warn!("hash gate remote list {device_id}/{space}: {e}");
            return out;
        }
    };
    if let Some(entries) = res.get("entries").and_then(|v| v.as_array()) {
        for raw in entries {
            let Some(e) = raw.as_object() else { continue };
            let path = e.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let sha = e.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
            if !path.is_empty() && !sha.is_empty() {
                out.insert(path.to_string(), sha.to_string());
            }
        }
    }
    out
}

fn list_local_shas(local: &Local, device_id: &str, space: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let listed = browse::admin_list(local, device_id, space, "").unwrap_or_default();
    if let Some(entries) = listed.get("entries").and_then(|v| v.as_array()) {
        for raw in entries {
            let Some(e) = raw.as_object() else { continue };
            let path = e.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let sha = e.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
            if !path.is_empty() && !sha.is_empty() {
                out.insert(path.to_string(), sha.to_string());
            }
        }
    }
    out
}

fn digest_path_map(path_to_sha: &HashMap<String, String>) -> String {
    let mut keys: Vec<_> = path_to_sha.keys().cloned().collect();
    keys.sort();
    let mut b = String::new();
    for k in keys {
        b.push_str(&k);
        b.push('\0');
        b.push_str(path_to_sha.get(&k).unwrap());
        b.push('\n');
    }
    hex::encode(Sha256::digest(b.as_bytes()))
}

fn seed_space(
    local: &Local,
    rpc: &dyn PeerRpc,
    old_master: &str,
    device_id: &str,
    space: &str,
) -> Result<i64, OpError> {
    let mut payload = Map::new();
    payload.insert("space".into(), json!(space));
    payload.insert("device".into(), json!(device_id));
    payload.insert("path".into(), json!(""));
    payload.insert("seed".into(), json!(true));
    payload.insert("limit".into(), json!(MIRROR_SEED_LIST_LIMIT));
    let list_res = rpc
        .call(old_master, "list", payload)
        .map_err(|e| OpError::new("internal", e))?;
    let entries = list_res
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut written = 0i64;
    for raw in entries {
        let Some(e) = raw.as_object() else { continue };
        let path = e.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let sha = e.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
        let size = any_to_i64(e.get("size").unwrap_or(&Value::Null)).unwrap_or(0);
        if path.is_empty() || sha.is_empty() {
            continue;
        }
        if local_file_matches(local, device_id, space, path, sha) {
            continue;
        }
        let bytes = match read_all_remote(rpc, old_master, device_id, space, path, size) {
            Ok(b) => b,
            Err(e) => {
                warn!("seed read {device_id}/{space}/{path}: {e}");
                continue;
            }
        };
        let sum = hex::encode(Sha256::digest(&bytes));
        if sum != sha {
            warn!("seed hash mismatch {device_id}/{space}/{path}");
            continue;
        }
        if let Err(e) = put_mirror_file(local, device_id, space, path, &bytes) {
            warn!("seed write {device_id}/{space}/{path}: {e}");
            continue;
        }
        written += 1;
    }
    Ok(written)
}

fn local_file_matches(local: &Local, device: &str, space: &str, rel: &str, sha: &str) -> bool {
    let full = local.resolve(space, device, rel).ok();
    let Some(full) = full else {
        return false;
    };
    let (sum, _) = browse::file_sha(&full);
    sum == sha
}

fn read_all_remote(
    rpc: &dyn PeerRpc,
    old_master: &str,
    device: &str,
    space: &str,
    path: &str,
    size: i64,
) -> Result<Vec<u8>, OpError> {
    let mut out = Vec::new();
    let mut offset = 0i64;
    loop {
        let mut payload = Map::new();
        payload.insert("space".into(), json!(space));
        payload.insert("device".into(), json!(device));
        payload.insert("path".into(), json!(path));
        payload.insert("offset".into(), json!(offset));
        payload.insert("length".into(), json!(MAX_CHUNK));
        payload.insert("seed".into(), json!(true));
        let res = rpc
            .call(old_master, "read", payload)
            .map_err(|e| OpError::new("internal", e))?;
        let b64 = res.get("data").and_then(|v| v.as_str()).unwrap_or("");
        let chunk = STANDARD
            .decode(b64)
            .map_err(|e| OpError::new("internal", e.to_string()))?;
        out.extend_from_slice(&chunk);
        offset += chunk.len() as i64;
        let eof = res.get("eof").and_then(|v| v.as_bool()).unwrap_or(false);
        if eof || chunk.is_empty() {
            break;
        }
    }
    if size > 0 && out.len() as i64 != size {
        return Err(OpError::new("internal", "seed size mismatch"));
    }
    Ok(out)
}

fn put_mirror_file(
    local: &Local,
    device: &str,
    space: &str,
    rel: &str,
    data: &[u8],
) -> Result<(), OpError> {
    let full = local.resolve(space, device, rel)?;
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    let tmp = format!("{}.seed-tmp", full.display());
    fs::write(&tmp, data).map_err(io_err)?;
    fs::rename(&tmp, &full).map_err(io_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Frame;
    use base64::Engine;
    use sha2::{Digest, Sha256};
    use std::collections::HashMap as StdHashMap;
    use tempfile::tempdir;

    struct FakeRpc {
        has: StdHashMap<String, bool>,
        cursors: Map<String, Value>,
        list: Map<String, Value>,
        reads: StdHashMap<String, Vec<u8>>,
    }

    impl PeerRpc for FakeRpc {
        fn has(&self, device_id: &str) -> bool {
            self.has.get(device_id).copied().unwrap_or(false)
        }

        fn call(
            &self,
            _device_id: &str,
            op: &str,
            payload: Map<String, Value>,
        ) -> Result<Map<String, Value>, String> {
            match op {
                "sync.cursors" => Ok(Map::from_iter([(
                    "cursors".into(),
                    Value::Object(self.cursors.clone()),
                )])),
                "stats" => {
                    let mut devices = Map::new();
                    for id in self.cursors.keys() {
                        devices.insert(id.clone(), json!({}));
                    }
                    Ok(Map::from_iter([("devices".into(), Value::Object(devices))]))
                }
                "list" => {
                    let space = payload.get("space").and_then(|v| v.as_str()).unwrap_or("");
                    if space != "files" {
                        return Ok(Map::from_iter([("entries".into(), json!([]))]));
                    }
                    Ok(self.list.clone())
                }
                "read" => {
                    let device = payload.get("device").and_then(|v| v.as_str()).unwrap_or("");
                    let space = payload.get("space").and_then(|v| v.as_str()).unwrap_or("");
                    let path = payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let key = format!("{device}|{space}|{path}");
                    let data = self.reads.get(&key).cloned().unwrap_or_default();
                    let offset = payload
                        .get("offset")
                        .and_then(any_to_i64)
                        .unwrap_or(0) as usize;
                    if offset >= data.len() {
                        return Ok(Map::from_iter([
                            ("data".into(), json!("")),
                            ("size".into(), json!(0)),
                            ("eof".into(), json!(true)),
                        ]));
                    }
                    let chunk = &data[offset..];
                    let chunk = if chunk.len() > MAX_CHUNK {
                        &chunk[..MAX_CHUNK]
                    } else {
                        chunk
                    };
                    let eof = offset + chunk.len() >= data.len();
                    Ok(Map::from_iter([
                        (
                            "data".into(),
                            json!(base64::engine::general_purpose::STANDARD.encode(chunk)),
                        ),
                        ("size".into(), json!(chunk.len())),
                        ("eof".into(), json!(eof)),
                    ]))
                }
                _ => Ok(Map::new()),
            }
        }
    }

    #[test]
    fn master_migrate_seeds_from_live_peer_rpc() {
        let root = tempdir().unwrap();
        let self_id = "aaaaaaaaaaaaaaaa";
        let old = "bbbbbbbbbbbbbbbb";
        let other = "cccccccccccccccc";
        let store = Local::open(root.path(), self_id).unwrap();
        store
            .handle(
                Frame::from_parts(
                    "master.pointer",
                    Map::from_iter([
                        ("master".into(), json!(old)),
                        ("epoch".into(), json!(2)),
                    ]),
                ),
                self_id,
                protocol::TRUST_OWNER,
                true,
            )
            .unwrap();

        let payload = b"seeded-mirror-bytes";
        let sha = hex::encode(Sha256::digest(payload));
        let rpc = Arc::new(FakeRpc {
            has: StdHashMap::from([(old.to_string(), true)]),
            cursors: Map::from_iter([(other.to_string(), json!(7))]),
            list: Map::from_iter([(
                "entries".into(),
                json!([{
                    "path": "note.txt",
                    "size": payload.len(),
                    "sha256": sha,
                }]),
            )]),
            reads: StdHashMap::from([(format!("{other}|files|note.txt"), payload.to_vec())]),
        });
        store.set_peer_rpc(rpc);

        let res = store
            .handle(
                Frame::from_parts("master.migrate", Map::new()),
                self_id,
                protocol::TRUST_OWNER,
                true,
            )
            .unwrap();
        assert_eq!(res["old_master_reachable"], json!(true));
        assert_eq!(res["seeded_files"], json!(1));
        let gate = res["hash_gate"].as_object().unwrap();
        assert_eq!(gate["ran"], json!(true));
        assert_eq!(gate["ok"], json!(true));
        assert_eq!(res["master"], json!(self_id));
        assert_eq!(res["epoch"], json!(3));
        assert_eq!(res["cursors"][other], json!(7));
        let got = fs::read(root.path().join(other).join("files").join("note.txt")).unwrap();
        assert_eq!(got, payload);
    }

    #[test]
    fn master_migrate_hash_gate_missing_local() {
        let root = tempdir().unwrap();
        let self_id = "aaaaaaaaaaaaaaaa";
        let old = "bbbbbbbbbbbbbbbb";
        let other = "cccccccccccccccc";
        let store = Local::open(root.path(), self_id).unwrap();
        store
            .handle(
                Frame::from_parts(
                    "master.pointer",
                    Map::from_iter([
                        ("master".into(), json!(old)),
                        ("epoch".into(), json!(1)),
                    ]),
                ),
                self_id,
                protocol::TRUST_OWNER,
                true,
            )
            .unwrap();

        let payload = b"remote-only";
        let sha = hex::encode(Sha256::digest(payload));
        let rpc = Arc::new(FakeRpc {
            has: StdHashMap::from([(old.to_string(), true)]),
            cursors: Map::from_iter([(other.to_string(), json!(1))]),
            list: Map::from_iter([(
                "entries".into(),
                json!([{
                    "path": "only-remote.txt",
                    "size": payload.len(),
                    "sha256": sha,
                }]),
            )]),
            reads: StdHashMap::new(),
        });
        store.set_peer_rpc(rpc);

        let res = store
            .handle(
                Frame::from_parts("master.migrate", Map::new()),
                self_id,
                protocol::TRUST_OWNER,
                true,
            )
            .unwrap();
        let gate = res["hash_gate"].as_object().unwrap();
        assert_eq!(gate["ran"], json!(true));
        assert_eq!(gate["ok"], json!(false));

        store
            .handle(
                Frame::from_parts(
                    "master.pointer",
                    Map::from_iter([
                        ("master".into(), json!(old)),
                        ("epoch".into(), json!(5)),
                    ]),
                ),
                self_id,
                protocol::TRUST_OWNER,
                true,
            )
            .unwrap();
        let err = store
            .handle(
                Frame::from_parts(
                    "master.migrate",
                    Map::from_iter([("require_hash_match".into(), json!(true))]),
                ),
                self_id,
                protocol::TRUST_OWNER,
                true,
            )
            .unwrap_err();
        assert_eq!(err.code, "hash_gate");
    }
}
