//! Sessions Web UI backend (I P1.5, SESSION_HISTORY_DESIGN §0.5): overview /
//! detail / search over the `sessions` space.
//!
//! Pure logic takes `&Local` for testability; thin HTTP wrappers live in
//! `admin/mod.rs`. Read-only: transcripts are returned verbatim (scrubbing is
//! an index-layer concern, §4) — the UI marks detail content as 原文未脱敏.

use crate::protocol::{self, Frame};
use crate::store::{browse, session_adapt, spaces, versions, Local, OpError};
use crate::uri::RefKind;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;
use std::sync::Mutex;

/// Head bytes parsed for format detection + first turns (title/start time).
const HEAD_BYTES: u64 = 64 * 1024;
/// Detail reads at most this many bytes before marking `truncated`.
const DETAIL_CAP_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TURNS: usize = 5000;
const MAX_TURN_CONTENT_CHARS: usize = 100 * 1024;
const DEFAULT_LIMIT: usize = 500;
const MAX_LIMIT: usize = 2000;
const MAX_CACHE_ENTRIES: usize = 8192;

/// In-memory per-file overview cache keyed by (device, path); entries are
/// invalidated by (size, mtime) change — appends flip both, so live sessions
/// stay fresh while unchanged files skip re-parsing.
pub struct SummaryCache {
    inner: Mutex<HashMap<(String, String), CachedSummary>>,
}

#[derive(Clone)]
struct CachedSummary {
    size: u64,
    mtime: i64,
    title: String,
    start_ms: i64,
    kind: String,
    events: u64,
}

impl SummaryCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, device: &str, path: &str, size: u64, mtime: i64) -> Option<CachedSummary> {
        let map = self.inner.lock().ok()?;
        let c = map.get(&(device.to_string(), path.to_string()))?;
        (c.size == size && c.mtime == mtime).then(|| c.clone())
    }

    fn put(&self, device: &str, path: &str, summary: CachedSummary) {
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        if map.len() >= MAX_CACHE_ENTRIES {
            map.clear();
        }
        map.insert((device.to_string(), path.to_string()), summary);
    }
}

/// Aggregate session metadata across devices that have a `sessions/` tree.
pub fn overview(
    local: &Local,
    device: Option<&str>,
    limit: usize,
    cache: &SummaryCache,
) -> Result<Map<String, Value>, OpError> {
    let limit = if limit == 0 {
        DEFAULT_LIMIT
    } else {
        limit.clamp(1, MAX_LIMIT)
    };
    let devices = enum_devices(local, device)?;
    let mut sessions = Vec::new();
    for dev in &devices {
        let base = local.root.join(dev).join(spaces::SESSIONS_SPACE);
        let mut files = Vec::new();
        walk_jsonl(&base, &base, &mut files);
        for (rel, size, mtime) in files {
            let summary = match cache.get(dev, &rel, size, mtime) {
                Some(s) => s,
                None => {
                    let s = summarize_file(
                        &base.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR)),
                        &rel,
                        size,
                        mtime,
                    );
                    cache.put(dev, &rel, s.clone());
                    s
                }
            };
            sessions.push(json!({
                "uri": format!("store://{}/{dev}/{rel}", spaces::SESSIONS_SPACE),
                "device": dev,
                "path": rel,
                "agent": session_adapt::agent_from_path(&rel),
                "project": project_from_path(&rel),
                "session_id": session_adapt::session_from_path(&rel),
                "kind": summary.kind,
                "title": summary.title,
                "start_ms": summary.start_ms,
                "mtime": mtime,
                "duration_ms": (summary.start_ms > 0).then(|| (mtime - summary.start_ms).max(0)),
                "events": summary.events,
                "size": size,
            }));
        }
    }
    sessions.sort_by(|a, b| {
        let am = a.get("mtime").and_then(|v| v.as_i64()).unwrap_or(0);
        let bm = b.get("mtime").and_then(|v| v.as_i64()).unwrap_or(0);
        bm.cmp(&am).then_with(|| {
            a.get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .cmp(b.get("path").and_then(|v| v.as_str()).unwrap_or(""))
        })
    });
    let total = sessions.len();
    let truncated = total > limit;
    sessions.truncate(limit);
    Ok(Map::from_iter([
        ("sessions".into(), Value::Array(sessions)),
        ("devices".into(), json!(devices)),
        ("total".into(), json!(total)),
        ("truncated".into(), json!(truncated)),
    ]))
}

/// Parse one transcript (optionally at an archived version via `@vN`/`@hash`)
/// into normalized chat turns. Raw content is returned unscrubbed.
pub fn detail(local: &Local, uri: &str) -> Result<Map<String, Value>, OpError> {
    let parsed = crate::uri::parse_with(uri, &|sp| local.is_known_space(sp))?;
    if parsed.space != spaces::SESSIONS_SPACE {
        return Err(OpError::new("bad_op", "detail is sessions-only"));
    }
    if parsed.path.is_empty() {
        return Err(OpError::new("bad_path", "detail requires a file path"));
    }
    let full = match &parsed.ref_kind {
        RefKind::Latest => browse::resolve_path(local, &parsed.space, &parsed.device, &parsed.path)?,
        other => versions::resolve(local, &parsed.space, &parsed.device, &parsed.path, other)?,
    };
    let f =
        std::fs::File::open(&full).map_err(|e| OpError::new("not_found", e.to_string()))?;
    let mut buf = Vec::new();
    f.take(DETAIL_CAP_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| OpError::new("internal", e.to_string()))?;
    let mut truncated = buf.len() as u64 > DETAIL_CAP_BYTES;
    buf.truncate(DETAIL_CAP_BYTES as usize);
    let raw = String::from_utf8_lossy(&buf);
    let adapted = session_adapt::adapt_transcript(&raw, &parsed.path);
    let mut turns = Vec::new();
    for t in adapted.turns.iter().take(MAX_TURNS) {
        let content = if t.content.chars().count() > MAX_TURN_CONTENT_CHARS {
            truncated = true;
            t.content
                .chars()
                .take(MAX_TURN_CONTENT_CHARS)
                .collect::<String>()
        } else {
            t.content.clone()
        };
        turns.push(json!({
            "seq": t.seq,
            "ts_ms": t.ts_ms,
            "role": t.role,
            "content": content,
        }));
    }
    if adapted.turns.len() > MAX_TURNS {
        truncated = true;
    }
    // versions + protected via the public frame op.
    let mut payload = Map::new();
    payload.insert("space".into(), json!(parsed.space));
    payload.insert("device".into(), json!(parsed.device));
    payload.insert("path".into(), json!(parsed.path));
    let vinfo = versions::list(
        local,
        &Frame::from_parts("versions.list", payload),
        &local.device_id,
    )?;
    let version = match &parsed.ref_kind {
        RefKind::Latest => vinfo
            .get("versions")
            .and_then(|v| v.as_array())
            .and_then(|a| a.last())
            .and_then(|e| e.get("v"))
            .cloned()
            .unwrap_or(Value::Null),
        RefKind::Seq(n) => json!(n),
        RefKind::Hash(h) => json!(h),
    };
    let ref_label = match &parsed.ref_kind {
        RefKind::Latest => "latest".to_string(),
        RefKind::Seq(n) => format!("v{n}"),
        RefKind::Hash(h) => h.clone(),
    };
    Ok(Map::from_iter([
        ("uri".into(), json!(parsed.format())),
        ("device".into(), json!(parsed.device)),
        ("path".into(), json!(parsed.path)),
        ("agent".into(), json!(session_adapt::agent_from_path(&parsed.path))),
        ("project".into(), json!(project_from_path(&parsed.path))),
        (
            "session_id".into(),
            json!(session_adapt::session_from_path(&parsed.path)),
        ),
        ("kind".into(), json!(adapted.kind.as_str())),
        ("ref".into(), json!(ref_label)),
        ("version".into(), version),
        (
            "versions".into(),
            vinfo.get("versions").cloned().unwrap_or(json!([])),
        ),
        (
            "protected".into(),
            vinfo.get("protected").cloned().unwrap_or(json!(false)),
        ),
        (
            "size".into(),
            json!(std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0)),
        ),
        ("truncated".into(), json!(truncated)),
        ("turns".into(), Value::Array(turns)),
    ]))
}

/// Sessions-scoped proxy over `Local::search_query` (FTS5 keyword by default,
/// FTS5+vector RRF hybrid when `semantic`). Optional R1 filters narrow by
/// agent / project / mtime range.
/// R3: record click / adopt / wrong for a sessions search hit.
pub fn feedback(
    local: &Local,
    kind: &str,
    query: &str,
    uri: &str,
    rank: Option<u32>,
    note: Option<&str>,
    score_type: Option<&str>,
) -> Result<Map<String, Value>, OpError> {
    local.record_recall_feedback(
        kind,
        query,
        uri,
        rank,
        note,
        score_type,
        None,
        Some("admin.sessions"),
    )
}

pub fn search(
    local: &Local,
    q: &str,
    device: Option<&str>,
    limit: usize,
    semantic: bool,
    filter: Option<&crate::store::SearchFilter>,
    opts: &crate::store::SearchOptions,
) -> Result<Map<String, Value>, OpError> {
    let q = q.trim();
    if q.is_empty() {
        return Err(OpError::new("bad_op", "q required"));
    }
    if let Some(d) = device {
        if !protocol::is_valid_device_id(d) {
            return Err(OpError::new("bad_path", "invalid device id"));
        }
    }
    let limit = if limit == 0 { 50 } else { limit.clamp(1, 200) };
    local.search_query_ex(
        q,
        Some(spaces::SESSIONS_SPACE),
        device,
        None,
        limit,
        semantic,
        filter,
        opts,
    )
}

/// Devices with a `sessions/` directory; self (when present) sorts first.
fn enum_devices(local: &Local, device: Option<&str>) -> Result<Vec<String>, OpError> {
    if let Some(d) = device {
        if !protocol::is_valid_device_id(d) {
            return Err(OpError::new("bad_path", "invalid device id"));
        }
        return Ok(vec![d.to_string()]);
    }
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&local.root) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !protocol::is_valid_device_id(&name) {
                continue;
            }
            if entry.path().join(spaces::SESSIONS_SPACE).is_dir() {
                out.push(name);
            }
        }
    }
    out.sort();
    if let Some(pos) = out.iter().position(|d| d == &local.device_id) {
        let d = out.remove(pos);
        out.insert(0, d);
    }
    Ok(out)
}

/// Collect (rel_path, size, mtime_ms) for non-hidden `.jsonl` files under dir.
fn walk_jsonl(dir: &Path, base: &Path, out: &mut Vec<(String, u64, i64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            walk_jsonl(&path, base, out);
            continue;
        }
        if !name.to_ascii_lowercase().ends_with(".jsonl") {
            continue;
        }
        let rel = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        out.push((rel, meta.len(), mtime_ms(&meta)));
    }
}

/// Parse the head for kind/title/start time; count newlines over the whole
/// file for an event count. O(file size) worst case, tiny constant.
fn summarize_file(full: &Path, rel: &str, size: u64, mtime: i64) -> CachedSummary {
    let session_id = session_adapt::session_from_path(rel);
    let mut summary = CachedSummary {
        size,
        mtime,
        title: session_id.clone(),
        start_ms: 0,
        kind: "generic".into(),
        events: 0,
    };
    let Ok(mut f) = std::fs::File::open(full) else {
        return summary;
    };
    let mut head = Vec::new();
    if f.by_ref().take(HEAD_BYTES).read_to_end(&mut head).is_err() {
        return summary;
    }
    let adapted = session_adapt::adapt_transcript(&String::from_utf8_lossy(&head), rel);
    summary.kind = adapted.kind.as_str().to_string();
    if let Some(t) = adapted.turns.iter().find(|t| t.ts_ms > 0) {
        summary.start_ms = t.ts_ms;
    }
    // Title: first substantive user turn (skip system-reminder style `<…`).
    if let Some(t) = adapted.turns.iter().find(|t| {
        t.role == "user" && !t.content.trim().is_empty() && !t.content.trim_start().starts_with('<')
    }) {
        let title: String = t
            .content
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .chars()
            .take(100)
            .collect();
        if !title.is_empty() {
            summary.title = title;
        }
    }
    // Event count: newlines in the head plus the rest of the file.
    let mut events = head.iter().filter(|b| **b == b'\n').count() as u64;
    let mut last_byte = head.last().copied();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                events += buf[..n].iter().filter(|b| **b == b'\n').count() as u64;
                last_byte = Some(buf[n - 1]);
            }
            Err(_) => break,
        }
    }
    // A trailing partial line still counts as an event.
    if let Some(b) = last_byte {
        if b != b'\n' {
            events += 1;
        }
    }
    summary.events = events;
    summary
}

/// Middle path segments between agent dir and file name, if any.
fn project_from_path(rel: &str) -> Option<String> {
    let parts: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() > 2 {
        Some(parts[1..parts.len() - 1].join("/"))
    } else {
        None
    }
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEV: &str = "aaaaaaaaaaaaaaaa";

    fn open_local() -> (tempfile::TempDir, Local) {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::open(dir.path(), DEV).unwrap();
        (dir, local)
    }

    fn write_session(local: &Local, rel: &str, body: &str) {
        write_session_at(local, DEV, rel, body);
    }

    fn write_session_at(local: &Local, device: &str, rel: &str, body: &str) {
        let path = local
            .root
            .join(device)
            .join(spaces::SESSIONS_SPACE)
            .join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("docs/storage_fixtures")
                .join(name),
        )
        .unwrap()
    }

    #[test]
    fn overview_empty() {
        let (_dir, local) = open_local();
        let cache = SummaryCache::new();
        let out = overview(&local, None, 0, &cache).unwrap();
        assert_eq!(out["sessions"].as_array().unwrap().len(), 0);
        assert_eq!(out["total"].as_u64().unwrap(), 0);
        assert_eq!(out["truncated"].as_bool().unwrap(), false);
    }

    #[test]
    fn overview_aggregates_and_filters() {
        let (_dir, local) = open_local();
        write_session(&local, "claude-code/proj-a/s-1.jsonl", &fixture("session_claude.jsonl"));
        write_session(&local, "acp-proxy/s-2.jsonl", &fixture("session_acp.jsonl"));
        // Non-jsonl and hidden files are skipped.
        write_session(&local, "notes.txt", "hello");
        write_session(&local, ".hidden.jsonl", "{\"role\":\"user\",\"content\":\"x\"}\n");
        // A second device tree is aggregated too.
        write_session_at(
            &local,
            "bbbbbbbbbbbbbbbb",
            "codex/s-3.jsonl",
            &fixture("session_codex.jsonl"),
        );

        let cache = SummaryCache::new();
        let out = overview(&local, None, 0, &cache).unwrap();
        let sessions = out["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 3);
        assert_eq!(out["total"].as_u64().unwrap(), 3);
        let devices = out["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].as_str().unwrap(), DEV); // self first

        let claude = sessions
            .iter()
            .find(|s| s["path"] == "claude-code/proj-a/s-1.jsonl")
            .unwrap();
        assert_eq!(claude["agent"], "claude-code");
        assert_eq!(claude["project"], "proj-a");
        assert_eq!(claude["session_id"], "s-1");
        assert_eq!(claude["kind"], "claude");
        assert_eq!(claude["uri"], format!("store://sessions/{DEV}/claude-code/proj-a/s-1.jsonl"));
        assert!(!claude["title"].as_str().unwrap().is_empty());
        assert!(claude["events"].as_u64().unwrap() > 0);
        assert!(claude["size"].as_u64().unwrap() > 0);
        assert!(claude["start_ms"].as_i64().unwrap() > 0);
        assert!(claude["duration_ms"].as_i64().unwrap() >= 0);

        let acp = sessions
            .iter()
            .find(|s| s["path"] == "acp-proxy/s-2.jsonl")
            .unwrap();
        assert_eq!(acp["kind"], "acp");
        assert!(acp["project"].is_null());

        // Device filter: valid but absent device → empty; invalid → bad_path.
        let only_self = overview(&local, Some(DEV), 0, &cache).unwrap();
        assert_eq!(only_self["sessions"].as_array().unwrap().len(), 2);
        let absent = overview(&local, Some("cccccccccccccccc"), 0, &cache).unwrap();
        assert_eq!(absent["sessions"].as_array().unwrap().len(), 0);
        assert!(overview(&local, Some("not-a-device"), 0, &cache).is_err());
    }

    #[test]
    fn overview_limit_truncates() {
        let (_dir, local) = open_local();
        for i in 0..3 {
            write_session(
                &local,
                &format!("claude-code/s-{i}.jsonl"),
                &fixture("session_claude.jsonl"),
            );
        }
        let cache = SummaryCache::new();
        let out = overview(&local, None, 2, &cache).unwrap();
        assert_eq!(out["sessions"].as_array().unwrap().len(), 2);
        assert_eq!(out["total"].as_u64().unwrap(), 3);
        assert_eq!(out["truncated"].as_bool().unwrap(), true);
    }

    #[test]
    fn overview_cache_invalidates_on_change() {
        let (_dir, local) = open_local();
        write_session(
            &local,
            "claude-code/s-1.jsonl",
            "{\"type\":\"user\",\"timestamp\":\"2026-05-16T08:00:00.000Z\",\"message\":{\"content\":\"first topic alpha\"}}\n",
        );
        let cache = SummaryCache::new();
        let out1 = overview(&local, None, 0, &cache).unwrap();
        assert_eq!(out1["sessions"][0]["title"], "first topic alpha");
        // Rewrite with different content (size/mtime change) → fresh summary.
        write_session(
            &local,
            "claude-code/s-1.jsonl",
            "{\"type\":\"user\",\"timestamp\":\"2026-05-16T09:00:00.000Z\",\"message\":{\"content\":\"second topic beta longer\"}}\n",
        );
        let out2 = overview(&local, None, 0, &cache).unwrap();
        assert_eq!(out2["sessions"][0]["title"], "second topic beta longer");
    }

    #[test]
    fn detail_latest_and_versions() {
        let (_dir, local) = open_local();
        write_session(&local, "claude-code/proj-a/s-1.jsonl", &fixture("session_claude.jsonl"));
        let out = detail(&local, &format!("store://sessions/{DEV}/claude-code/proj-a/s-1.jsonl"))
            .unwrap();
        assert_eq!(out["agent"], "claude-code");
        assert_eq!(out["project"], "proj-a");
        assert_eq!(out["session_id"], "s-1");
        assert_eq!(out["kind"], "claude");
        assert_eq!(out["ref"], "latest");
        assert_eq!(out["version"], 1); // current file counts as v1
        assert_eq!(out["protected"], false);
        assert_eq!(out["truncated"], false);
        let turns = out["turns"].as_array().unwrap();
        assert!(turns.len() >= 2);
        assert!(turns.iter().any(|t| t["role"] == "user"));
        // v1 resolves to the current file when nothing is archived yet.
        let v1 = detail(
            &local,
            &format!("store://sessions/{DEV}/claude-code/proj-a/s-1.jsonl@v1"),
        )
        .unwrap();
        assert_eq!(v1["ref"], "v1");
        assert_eq!(
            v1["turns"].as_array().unwrap().len(),
            turns.len()
        );
    }

    #[test]
    fn detail_rejects_bad_input() {
        let (_dir, local) = open_local();
        write_session(&local, "claude-code/s-1.jsonl", &fixture("session_claude.jsonl"));
        // Empty path.
        assert!(detail(&local, "store://sessions/aaaaaaaaaaaaaaaa/").is_err());
        // Escape attempts.
        assert!(detail(&local, "store://sessions/aaaaaaaaaaaaaaaa/../files/x").is_err());
        // Missing version.
        assert!(
            detail(&local, "store://sessions/aaaaaaaaaaaaaaaa/claude-code/s-1.jsonl@v99").is_err()
        );
        // Non-sessions space.
        assert!(detail(&local, "store://files/aaaaaaaaaaaaaaaa/x.jsonl").is_err());
        // Missing file.
        assert!(detail(&local, "store://sessions/aaaaaaaaaaaaaaaa/claude-code/none.jsonl").is_err());
    }

    #[test]
    fn detail_truncates_oversized_file() {
        let (_dir, local) = open_local();
        let line = format!(
            "{{\"role\":\"user\",\"content\":\"{}\"}}\n",
            "x".repeat(2048)
        );
        let mut body = String::new();
        while body.len() <= DETAIL_CAP_BYTES as usize {
            body.push_str(&line);
        }
        write_session(&local, "acp-proxy/big.jsonl", &body);
        let out = detail(&local, &format!("store://sessions/{DEV}/acp-proxy/big.jsonl")).unwrap();
        assert_eq!(out["truncated"], true);
        assert!(!out["turns"].as_array().unwrap().is_empty());
    }

    #[test]
    fn search_validates_and_hits() {
        let (_dir, local) = open_local();
        write_session(&local, "claude-code/s-1.jsonl", &fixture("session_claude.jsonl"));
        local.rebuild_index().unwrap();
        let opts = crate::store::SearchOptions::default();
        // Validation.
        assert!(search(&local, "", None, 0, false, None, &opts).is_err());
        assert!(search(&local, "   ", None, 0, false, None, &opts).is_err());
        assert!(search(&local, "q", Some("nope"), 0, false, None, &opts).is_err());
        // Content hit through the real index pipeline (fixture mentions deploy).
        let out = search(&local, "deploy", None, 0, false, None, &opts).unwrap();
        assert!(out["total"].as_u64().unwrap() >= 1);
        let uris: Vec<&str> = out["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["uri"].as_str())
            .collect();
        assert!(uris.iter().any(|u| u.contains("claude-code/s-1.jsonl")));
        assert_eq!(out["score_type"], "keyword");
        // Semantic path returns structured fields even without a model.
        let sem = search(&local, "deploy", None, 0, true, None, &opts).unwrap();
        assert!(sem.get("score_type").is_some());
        assert!(sem.get("degraded").is_some());
        // R1 agent filter: only matching path prefix.
        let agent = crate::store::SearchFilter {
            agent: Some("claude-code".into()),
            ..Default::default()
        };
        let filtered = search(&local, "deploy", None, 0, false, Some(&agent), &opts).unwrap();
        assert!(filtered["total"].as_u64().unwrap() >= 1);
        let miss = crate::store::SearchFilter {
            agent: Some("codex".into()),
            ..Default::default()
        };
        let empty = search(&local, "deploy", None, 0, false, Some(&miss), &opts).unwrap();
        assert_eq!(empty["total"], 0);
    }
}
