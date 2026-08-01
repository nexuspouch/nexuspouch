//! Handoff state machine (M3): artifact lifecycle + agent-to-agent handoff.
//!
//! ```text
//! draft ──commit──> committed ──publish──> published ──ack──> acked
//!                      │                        │
//!                      └──supersede─────────────┘──> superseded
//! ```
//!
//! Per-artifact state lives in `<task>/.nexuspouch/state.json`; acks in
//! `<task>/.nexuspouch/acks.json`; lineage stays in the task manifest. All
//! three are dot-prefixed system dirs, unreachable from external frames.

use super::{manifest, write, Local, OpError};
use crate::events::StoreEvent;
use crate::protocol::Frame;
use crate::uri;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactState {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acked_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acked_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateFile {
    #[serde(default)]
    pub artifacts: BTreeMap<String, ArtifactState>,
    #[serde(default)]
    pub superseded: Vec<SupersededRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupersededRecord {
    pub path: String,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acked_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acked_at: Option<i64>,
    pub superseded_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckRecord {
    pub uri: String,
    pub agent_id: String,
    pub ts_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AcksFile {
    #[serde(default)]
    pub acks: Vec<AckRecord>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn state_path(local: &Local, device: &str, space: &str, task: &str) -> PathBuf {
    local
        .root
        .join(device)
        .join(space)
        .join(task.replace('/', std::path::MAIN_SEPARATOR_STR))
        .join(".nexuspouch")
        .join("state.json")
}

fn acks_path(local: &Local, device: &str, space: &str, task: &str) -> PathBuf {
    local
        .root
        .join(device)
        .join(space)
        .join(task.replace('/', std::path::MAIN_SEPARATOR_STR))
        .join(".nexuspouch")
        .join("acks.json")
}

fn load_states(local: &Local, device: &str, space: &str, task: &str) -> StateFile {
    fs::read_to_string(state_path(local, device, space, task))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_states(
    local: &Local,
    device: &str,
    space: &str,
    task: &str,
    file: &StateFile,
) -> Result<(), OpError> {
    let p = state_path(local, device, space, task);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(super::io_err)?;
    }
    let tmp = p.with_extension("json.tmp");
    fs::write(
        &tmp,
        serde_json::to_vec_pretty(file).map_err(|e| OpError::new("internal", e.to_string()))?,
    )
    .map_err(super::io_err)?;
    fs::rename(&tmp, &p).map_err(super::io_err)
}

fn load_acks(local: &Local, device: &str, space: &str, task: &str) -> AcksFile {
    fs::read_to_string(acks_path(local, device, space, task))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_acks(
    local: &Local,
    device: &str,
    space: &str,
    task: &str,
    file: &AcksFile,
) -> Result<(), OpError> {
    let p = acks_path(local, device, space, task);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(super::io_err)?;
    }
    let tmp = p.with_extension("json.tmp");
    fs::write(
        &tmp,
        serde_json::to_vec_pretty(file).map_err(|e| OpError::new("internal", e.to_string()))?,
    )
    .map_err(super::io_err)?;
    fs::rename(&tmp, &p).map_err(super::io_err)
}

fn task_of(rel: &str) -> Result<String, OpError> {
    manifest::task_root(rel).ok_or_else(|| OpError::new("bad_path", "artifact requires a task path"))
}

/// Called by commit: transitions the previous artifact state to `superseded`
/// and records the new state (`published` for artifacts space / publish:true,
/// else `committed`).
pub fn on_commit(
    local: &Local,
    space: &str,
    device: &str,
    path: &str,
    sha: &str,
    publish: bool,
) -> Result<(), OpError> {
    let task = task_of(path)?;
    let mut file = load_states(local, device, space, &task);
    let old_sha = super::versions::all_versions(local, space, device, path)
        .ok()
        .and_then(|entries| {
            let n = entries.len();
            (n >= 2).then(|| entries[n - 2].sha256.clone())
        });
    if let Some(old) = file.artifacts.get(path) {
        if (old.state == "published" || old.state == "acked") && old_sha.is_some() {
            file.superseded.push(SupersededRecord {
                path: path.to_string(),
                sha256: old_sha.clone().unwrap(),
                acked_by: old.acked_by.clone(),
                acked_at: old.acked_at,
                superseded_by: sha.to_string(),
            });
        }
    }
    let state = if publish || space == "artifacts" {
        "published"
    } else {
        "committed"
    };
    file.artifacts.insert(
        path.to_string(),
        ArtifactState {
            state: state.to_string(),
            acked_by: None,
            acked_at: None,
            superseded_by: None,
        },
    );
    save_states(local, device, space, &task, &file)
}

/// `handoff.create`: commit with publish + context, emit `handoff.created`.
pub fn create(local: &Local, frame: &Frame, caller: &str) -> Result<Map<String, Value>, OpError> {
    let mut payload = frame.payload.clone();
    payload.insert("publish".into(), json!(true));
    if let Some(ctx) = frame.payload.get("context").and_then(|v| v.as_str()) {
        let mut manifest = frame
            .payload
            .get("manifest")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        manifest.insert("context".into(), json!(ctx));
        payload.insert("manifest".into(), Value::Object(manifest));
    }
    let commit_frame = Frame::from_parts("commit", payload);
    let out = write::commit(local, &commit_frame, caller)?;

    let mut detail = Map::new();
    if let Some(to) = frame.payload.get("to_agent").and_then(|v| v.as_str()) {
        detail.insert("to_agent".into(), json!(to));
    }
    if let Some(ctx) = frame.payload.get("context") {
        detail.insert("context".into(), ctx.clone());
    }
    let uri = committed_uri(frame, &out, caller);
    if let Some(uri) = &uri {
        local.emit_event(
            StoreEvent::new("handoff.created", caller)
                .with_uri(uri)
                .with_detail(detail),
        );
    }

    let mut result = out;
    if let Some(u) = uri {
        result.insert("handoff_uri".into(), json!(u));
    }
    result.insert("state".into(), json!("published"));
    Ok(result)
}

fn committed_uri(frame: &Frame, out: &Map<String, Value>, device: &str) -> Option<String> {
    let space = frame.payload.get("space").and_then(|v| v.as_str())?;
    let first = out
        .get("committed")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())?;
    let path = first.get("path").and_then(|v| v.as_str())?;
    Some(format!("store://{space}/{device}/{path}"))
}

/// `handoff.ack`: consume-side confirmation; idempotent per uri+agent_id.
pub fn ack(local: &Local, frame: &Frame, _caller: &str) -> Result<Map<String, Value>, OpError> {
    let uri_str = frame
        .payload
        .get("uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| OpError::new("bad_op", "ack requires uri"))?;
    let agent_id = frame
        .payload
        .get("agent_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| OpError::new("bad_op", "ack requires agent_id"))?;
    let parsed = uri::parse(uri_str).map_err(|e| OpError::new(e.code, e.msg))?;
    let task = task_of(&parsed.path)?;

    let mut states = load_states(local, &parsed.device, &parsed.space, &task);
    let entry = states
        .artifacts
        .get_mut(&parsed.path)
        .ok_or_else(|| OpError::new("not_found", "no artifact state"))?;
    if entry.acked_by.as_deref() == Some(agent_id) {
        return Ok(Map::from_iter([
            ("uri".into(), json!(uri_str)),
            ("state".into(), json!("acked")),
            ("already_acked".into(), json!(true)),
        ]));
    }
    if entry.state != "published" {
        return Err(OpError::new(
            "state_conflict",
            format!("cannot ack artifact in state {}", entry.state),
        ));
    }
    entry.state = "acked".to_string();
    entry.acked_by = Some(agent_id.to_string());
    entry.acked_at = Some(now_ms());
    save_states(local, &parsed.device, &parsed.space, &task, &states)?;

    let mut acks = load_acks(local, &parsed.device, &parsed.space, &task);
    if !acks
        .acks
        .iter()
        .any(|r| r.uri == uri_str && r.agent_id == agent_id)
    {
        acks.acks.push(AckRecord {
            uri: uri_str.to_string(),
            agent_id: agent_id.to_string(),
            ts_ms: now_ms(),
        });
        save_acks(local, &parsed.device, &parsed.space, &task, &acks)?;
    }

    local.emit_event(
        StoreEvent::new("handoff.acked", &parsed.device)
            .with_uri(uri_str)
            .with_path(&parsed.space, &parsed.path)
            .with_agent(agent_id),
    );
    Ok(Map::from_iter([
        ("uri".into(), json!(uri_str)),
        ("state".into(), json!("acked")),
        ("acked_by".into(), json!(agent_id)),
        ("already_acked".into(), json!(false)),
    ]))
}

/// `artifact.state`: status + lineage for a store URI.
pub fn state(local: &Local, frame: &Frame, caller: &str) -> Result<Map<String, Value>, OpError> {
    let uri_str = frame
        .payload
        .get("uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| OpError::new("bad_op", "state requires uri"))?;
    let parsed = uri::parse(uri_str).map_err(|e| OpError::new(e.code, e.msg))?;
    if parsed.path.is_empty() {
        return Err(OpError::new("bad_path", "state requires a file path"));
    }
    let task = task_of(&parsed.path)?;
    let states = load_states(local, &parsed.device, &parsed.space, &task);
    // A ref points at a specific version; check superseded history first.
    let ref_sha = version_sha(local, &parsed);
    let entry = if let Some(sha) = &ref_sha {
        states
            .superseded
            .iter()
            .find(|r| r.path == parsed.path && &r.sha256 == sha)
            .map(|r| ArtifactState {
                state: "superseded".into(),
                acked_by: r.acked_by.clone(),
                acked_at: r.acked_at,
                superseded_by: Some(r.superseded_by.clone()),
            })
    } else {
        None
    };
    let entry = entry.or_else(|| states.artifacts.get(&parsed.path).cloned());

    let mut out = Map::new();
    out.insert("uri".into(), json!(uri_str));
    out.insert("space".into(), json!(parsed.space));
    out.insert("device".into(), json!(parsed.device));
    out.insert("path".into(), json!(parsed.path));
    match entry {
        Some(e) => {
            out.insert("state".into(), json!(e.state));
            if let Some(by) = e.acked_by {
                out.insert("acked_by".into(), json!(by));
            }
            if let Some(at) = e.acked_at {
                out.insert("acked_at".into(), json!(at));
            }
            if let Some(sb) = e.superseded_by {
                out.insert("superseded_by".into(), json!(sb));
            }
        }
        None => {
            // Legacy file without state records: treat as committed.
            let full =
                super::browse::resolve_path(local, &parsed.space, &parsed.device, &parsed.path)?;
            if !full.is_file() {
                return Err(OpError::new("not_found", "no such artifact"));
            }
            out.insert("state".into(), json!("committed"));
        }
    }
    // Lineage from the task manifest.
    let mut mf = Map::new();
    mf.insert("space".into(), json!(parsed.space));
    mf.insert("device".into(), json!(parsed.device));
    mf.insert("path".into(), json!(task));
    if let Ok(m) = manifest::read(local, &Frame::from_parts("manifest", mf), caller) {
        if let Some(manifest_val) = m.get("manifest") {
            if let Some(producer) = manifest_val.get("producer") {
                out.insert("producer".into(), producer.clone());
            }
            if let Some(uris) = manifest_val.get("parent_uris") {
                out.insert("parent_uris".into(), uris.clone());
            }
            if let Some(ctx) = manifest_val.get("context") {
                out.insert("context".into(), ctx.clone());
            }
        }
    }
    Ok(out)
}

fn version_sha(local: &Local, parsed: &uri::StoreUri) -> Option<String> {
    match &parsed.ref_kind {
        crate::uri::RefKind::Latest => None,
        kind => super::versions::all_versions(local, &parsed.space, &parsed.device, &parsed.path)
            .ok()?
            .into_iter()
            .find(|e| match kind {
                crate::uri::RefKind::Seq(n) => e.v == *n,
                crate::uri::RefKind::Hash(h) => e.sha256.starts_with(h),
                crate::uri::RefKind::Latest => false,
            })
            .map(|e| e.sha256),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Local;
    use base64::Engine as _;
    use sha2::Digest;

    const DEV: &str = "aaaaaaaaaaaaaaaa";

    fn open() -> (tempfile::TempDir, Local) {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::open(dir.path(), DEV).unwrap();
        (dir, local)
    }

    fn commit_text(local: &Local, space: &str, rel: &str, content: &str, publish: bool) {
        use sha2::{Digest, Sha256};
        let sha = hex::encode(Sha256::digest(content.as_bytes()));
        let mut begin = Map::new();
        begin.insert("space".into(), json!(space));
        begin.insert("path".into(), json!(rel));
        begin.insert("size".into(), json!(content.len() as i64));
        begin.insert("sha256".into(), json!(sha));
        let out = local
            .handle(Frame::from_parts("write.begin", begin), DEV, "owner", true)
            .unwrap();
        let upload_id = out["upload_id"].as_str().unwrap().to_string();
        let mut chunk = Map::new();
        chunk.insert("upload_id".into(), json!(upload_id));
        chunk.insert("offset".into(), json!(0));
        chunk.insert(
            "data".into(),
            json!(base64::engine::general_purpose::STANDARD.encode(content.as_bytes())),
        );
        local
            .handle(Frame::from_parts("write.chunk", chunk), DEV, "owner", true)
            .unwrap();
        let mut commit = Map::new();
        commit.insert("space".into(), json!(space));
        commit.insert("upload_ids".into(), json!([upload_id]));
        commit.insert("publish".into(), json!(publish));
        local
            .handle(Frame::from_parts("commit", commit), DEV, "owner", true)
            .unwrap();
    }

    fn state_of(local: &Local, uri: &str) -> Value {
        let mut p = Map::new();
        p.insert("uri".into(), json!(uri));
        Value::Object(
            local
            .handle(Frame::from_parts("artifact.state", p), DEV, "owner", true)
            .unwrap(),
        )
    }

    #[test]
    fn publish_then_ack_then_supersede() {
        let (_d, local) = open();
        let rel = "t-1/out.txt";
        commit_text(&local, "artifacts", rel, "v1", false); // artifacts -> published
        let uri = format!("store://artifacts/{DEV}/{rel}");
        let s = state_of(&local, &uri);
        assert_eq!(s["state"], "published");

        let mut p = Map::new();
        p.insert("uri".into(), json!(uri));
        p.insert("agent_id".into(), json!("a-9"));
        let out = local
            .handle(Frame::from_parts("handoff.ack", p), DEV, "owner", true)
            .unwrap();
        assert_eq!(out["state"], "acked");
        assert_eq!(out["acked_by"], "a-9");

        // Duplicate ack is idempotent.
        let mut p = Map::new();
        p.insert("uri".into(), json!(uri));
        p.insert("agent_id".into(), json!("a-9"));
        let out = local
            .handle(Frame::from_parts("handoff.ack", p), DEV, "owner", true)
            .unwrap();
        assert_eq!(out["already_acked"], true);

        // Overwrite -> old artifact superseded.
        commit_text(&local, "artifacts", rel, "v2", false);
        let s = state_of(&local, &uri);
        assert_eq!(s["state"], "published");
        let old = state_of(&local, &format!("{uri}@v1"));
        assert_eq!(old["state"], "superseded");
        assert!(old.get("superseded_by").is_some());
    }

    #[test]
    fn ack_requires_published_state() {
        let (_d, local) = open();
        let rel = "t-2/doc.txt";
        // files space without publish -> committed.
        commit_text(&local, "files", rel, "v1", false);
        let uri = format!("store://files/{DEV}/{rel}");
        let s = state_of(&local, &uri);
        assert_eq!(s["state"], "committed");

        let mut p = Map::new();
        p.insert("uri".into(), json!(uri));
        p.insert("agent_id".into(), json!("a-1"));
        let err = local
            .handle(Frame::from_parts("handoff.ack", p), DEV, "owner", true)
            .unwrap_err();
        assert_eq!(err.code, "state_conflict");
    }

    #[test]
    fn handoff_create_sets_context_and_uri() {
        let (_d, local) = open();
        let sha = hex::encode(sha2::Sha256::digest(b"draft plan"));
        let mut begin = Map::new();
        begin.insert("space".into(), json!("artifacts"));
        begin.insert("path".into(), json!("t-3/plan.md"));
        begin.insert("size".into(), json!(10));
        begin.insert("sha256".into(), json!(sha));
        let out = local
            .handle(Frame::from_parts("write.begin", begin), DEV, "owner", true)
            .unwrap();
        let upload_id = out["upload_id"].as_str().unwrap().to_string();
        let mut chunk = Map::new();
        chunk.insert("upload_id".into(), json!(upload_id));
        chunk.insert("offset".into(), json!(0));
        chunk.insert(
            "data".into(),
            json!(base64::engine::general_purpose::STANDARD.encode(b"draft plan")),
        );
        local
            .handle(Frame::from_parts("write.chunk", chunk), DEV, "owner", true)
            .unwrap();

        let mut p = Map::new();
        p.insert("space".into(), json!("artifacts"));
        p.insert("path".into(), json!("t-3/plan.md"));
        p.insert("upload_ids".into(), json!([upload_id]));
        p.insert("context".into(), json!("Polish the draft and ack when done"));
        p.insert("to_agent".into(), json!("a-desktop"));
        let out = local
            .handle(Frame::from_parts("handoff.create", p), DEV, "owner", true)
            .unwrap();
        assert_eq!(out["state"], "published");
        let uri = out["handoff_uri"].as_str().unwrap();
        assert_eq!(uri, format!("store://artifacts/{DEV}/t-3/plan.md"));

        let s = state_of(&local, uri);
        assert_eq!(s["state"], "published");
        assert_eq!(s["context"], "Polish the draft and ack when done");
    }
}
