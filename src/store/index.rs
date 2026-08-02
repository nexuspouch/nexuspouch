//! Search index (M5): SQLite FTS5 over artifact/files metadata + text.
//!
//! Tables in `<root>/.system/index.db`:
//! - `files(id, uri UNIQUE, space, device, path, sha256, size, mtime, task, state)`
//! - `files_fts` (FTS5, `title, body`, contentless) keyed by `files.id`
//!
//! Body extraction is limited to text extensions (md/txt/json/log) and
//! files <= 1MB. Index updates ride the commit/delete hooks next to EventBus;
//! `nexuspouch index rebuild` / admin `/index/rebuild` rescan the tree.

use super::{browse, Local};
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const TEXT_EXTS: &[&str] = &["md", "txt", "json", "log", "markdown", "csv"];
const MAX_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub uri: String,
    pub space: String,
    pub device: String,
    pub path: String,
    pub sha256: String,
    pub size: i64,
    pub mtime: i64,
    pub task: String,
    pub state: String,
    pub summary: Option<String>,
    pub body: String,
}

pub struct SearchIndex {
    conn: Mutex<Option<Connection>>,
}

impl SearchIndex {
    pub fn open(root: impl AsRef<Path>) -> Self {
        let dir = root.as_ref().join(".system");
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("index.db");
        let conn = Self::open_conn(&db_path).ok();
        Self {
            conn: Mutex::new(conn),
        }
    }

    fn open_conn(db_path: &Path) -> rusqlite::Result<Connection> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS files (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               uri TEXT NOT NULL UNIQUE,
               space TEXT NOT NULL,
               device TEXT NOT NULL,
               path TEXT NOT NULL,
               sha256 TEXT NOT NULL,
               size INTEGER NOT NULL,
               mtime INTEGER NOT NULL,
               task TEXT NOT NULL,
               state TEXT NOT NULL DEFAULT 'committed'
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(title, body);",
        )?;
        Ok(conn)
    }

    pub fn index_file(&self, e: &IndexEntry) {
        let Ok(mut guard) = self.conn.lock() else {
            return;
        };
        let Some(conn) = guard.as_mut() else {
            return;
        };
        let title = format!("{} {}", e.path, e.summary.as_deref().unwrap_or(""));
        let body = format!("{} {}", e.body, e.summary.as_deref().unwrap_or(""));
        let res = (|| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO files (uri, space, device, path, sha256, size, mtime, task, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(uri) DO UPDATE SET
                   sha256=excluded.sha256, size=excluded.size, mtime=excluded.mtime,
                   state=excluded.state, task=excluded.task",
                params![
                    e.uri,
                    e.space,
                    e.device,
                    e.path,
                    e.sha256,
                    e.size,
                    e.mtime,
                    e.task,
                    e.state
                ],
            )?;
            let id: i64 = tx.query_row(
                "SELECT id FROM files WHERE uri = ?1",
                params![e.uri],
                |r| r.get(0),
            )?;
            tx.execute("DELETE FROM files_fts WHERE rowid = ?1", params![id])?;
            tx.execute(
                "INSERT INTO files_fts (rowid, title, body) VALUES (?1, ?2, ?3)",
                params![id, title, body],
            )?;
            tx.commit()
        })();
        if res.is_err() {
            // DB failure: drop the connection so subsequent writes are no-ops
            // instead of spamming errors; rebuild recovers.
            *guard = None;
        }
    }

    pub fn remove(&self, uri: &str) {
        let Ok(mut guard) = self.conn.lock() else {
            return;
        };
        let Some(conn) = guard.as_mut() else {
            return;
        };
        let _ = conn.execute("DELETE FROM files_fts WHERE rowid IN (SELECT id FROM files WHERE uri = ?1)", params![uri]);
        let _ = conn.execute("DELETE FROM files WHERE uri = ?1", params![uri]);
    }

    pub fn search(
        &self,
        q: &str,
        space: Option<&str>,
        device: Option<&str>,
        state: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Value>, String> {
        let guard = self.conn.lock().map_err(|e| e.to_string())?;
        let conn = guard.as_ref().ok_or_else(|| "index unavailable".to_string())?;
        // Phrase-match the query to avoid FTS5 syntax errors from user input.
        let matcher = format!("\"{}\"", q.replace('"', "\"\""));
        let mut sql = String::from(
            "SELECT f.uri, f.space, f.device, f.path, f.sha256, f.size, f.state,
                    snippet(files_fts, 1, '<b>', '</b>', '…', 12) AS snip,
                    bm25(files_fts) AS score
             FROM files_fts
             JOIN files f ON f.id = files_fts.rowid
             WHERE files_fts MATCH ?1",
        );
        let mut args: Vec<SqlValue> = vec![SqlValue::Text(matcher)];
        if let Some(s) = space {
            sql.push_str(" AND f.space = ?");
            args.push(SqlValue::Text(s.to_string()));
        }
        if let Some(d) = device {
            sql.push_str(" AND f.device = ?");
            args.push(SqlValue::Text(d.to_string()));
        }
        if let Some(st) = state {
            sql.push_str(" AND f.state = ?");
            args.push(SqlValue::Text(st.to_string()));
        }
        sql.push_str(" ORDER BY score LIMIT ?");
        let limit = limit.clamp(1, 200);
        args.push(SqlValue::Integer(limit as i64));

        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok(json!({
                    "uri": r.get::<_, String>(0)?,
                    "space": r.get::<_, String>(1)?,
                    "device": r.get::<_, String>(2)?,
                    "path": r.get::<_, String>(3)?,
                    "sha256": r.get::<_, String>(4)?,
                    "size": r.get::<_, i64>(5)?,
                    "state": r.get::<_, String>(6)?,
                    "snippet": r.get::<_, String>(7)?,
                    "score": r.get::<_, f64>(8)?,
                }))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn clear(&self) -> Result<(), String> {
        let guard = self.conn.lock().map_err(|e| e.to_string())?;
        let conn = guard
            .as_ref()
            .ok_or_else(|| "index unavailable".to_string())?;
        conn.execute_batch("DELETE FROM files_fts; DELETE FROM files;")
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Walk built-in + declared custom spaces for this device.
    pub fn scan_tree(&self, local: &Local) -> Vec<IndexEntry> {
        let mut spaces: Vec<String> = super::spaces::BUILTIN_SPACES
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        for p in local.spaces.list() {
            if !spaces.iter().any(|s| s == &p.name) {
                spaces.push(p.name);
            }
        }
        let mut out = Vec::new();
        for space in spaces {
            let root = local.root.join(&local.device_id).join(&space);
            out.extend(collect_tree(local, &root, &space, &local.device_id));
        }
        out
    }

    /// Rescan the store tree and rebuild the FTS index from the live tree.
    /// Prefer [`Local::rebuild_index`] so vectors are rebuilt too.
    pub fn rebuild(&self, local: &Local) -> Result<usize, String> {
        self.clear()?;
        let mut count = 0usize;
        for e in self.scan_tree(local) {
            self.index_file(&e);
            count += 1;
        }
        Ok(count)
    }
}

fn collect_tree(
    local: &Local,
    dir: &Path,
    space: &str,
    device: &str,
) -> Vec<IndexEntry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue; // .staging / .nexuspouch / system dirs
        }
        let path = ent.path();
        if path.is_dir() {
            out.extend(collect_tree(local, &path, space, device));
            continue;
        }
        if let Some(rel) = path.strip_prefix(&local.root.join(device).join(space)).ok() {
            if let Some(e) = build_entry(local, space, device, &rel.to_string_lossy()) {
                out.push(e);
            }
        }
    }
    out
}

fn build_entry(
    local: &Local,
    space: &str,
    device: &str,
    rel: &str,
) -> Option<IndexEntry> {
    let full = browse::resolve_path(local, space, device, rel).ok()?;
    let meta = std::fs::metadata(&full).ok()?;
    if !meta.is_file() {
        return None;
    }
    let (sha, size) = browse::file_sha(&full);
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let task = super::manifest::task_root(rel).unwrap_or_else(|| "".into());
    let summary = task_summary(local, space, device, &task);
    let body = if is_text_file(rel) && size as usize <= MAX_BODY_BYTES {
        std::fs::read(&full)
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    } else {
        String::new()
    };
    Some(IndexEntry {
        uri: format!("store://{space}/{device}/{rel}"),
        space: space.to_string(),
        device: device.to_string(),
        path: rel.to_string(),
        sha256: sha,
        size,
        mtime,
        task,
        state: "committed".into(),
        summary,
        body,
    })
}

fn task_summary(local: &Local, space: &str, device: &str, task: &str) -> Option<String> {
    if task.is_empty() {
        return None;
    }
    let mut p = serde_json::Map::new();
    p.insert("space".into(), json!(space));
    p.insert("device".into(), json!(device));
    p.insert("path".into(), json!(task));
    super::manifest::read(
        local,
        &crate::protocol::Frame::from_parts("manifest", p),
        device,
    )
    .ok()
    .and_then(|m| {
        m.get("manifest")
            .and_then(|v| v.get("summary"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    })
}

fn is_text_file(rel: &str) -> bool {
    rel.rsplit_once('.')
        .map(|(_, ext)| TEXT_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn index_entry_from_commit(
    local: &Local,
    space: &str,
    device: &str,
    path: &str,
    sha: &str,
    size: i64,
    publish: bool,
) -> Option<IndexEntry> {
    let mut e = build_entry(local, space, device, path)?;
    e.sha256 = sha.to_string();
    e.size = size;
    e.state = if publish { "published" } else { "committed" }.into();
    Some(e)
}

/// Optional summary hook: writes a one-line summary into the task manifest
/// (best-effort, fire-and-forget). Enabled only when configured.
pub fn maybe_summarize(local: &Local, space: &str, device: &str, path: &str) {
    let url = std::env::var("NEXUSPOUCH_SUMMARY_URL").ok().filter(|s| !s.is_empty());
    let cmd = std::env::var("NEXUSPOUCH_SUMMARY_CMD").ok().filter(|s| !s.is_empty());
    if url.is_none() && cmd.is_none() {
        return;
    }
    let full = match browse::resolve_path(local, space, device, path) {
        Ok(f) => f,
        Err(_) => return,
    };
    if full.metadata().map(|m| m.len()).unwrap_or(0) > 8 * 1024 {
        return;
    }
    let content = match std::fs::read(&full) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(_) => return,
    };
    let task = match super::manifest::task_root(path) {
        Some(t) => t,
        None => return,
    };
    let local_owned = reopen_local(local);
    let device_owned = device.to_string();
    let space_owned = space.to_string();
    let full_owned = full.clone();
    let _ = std::thread::spawn(move || {
        let summary = if let Some(u) = url {
            summarize_http(&u, &content)
        } else {
            summarize_cmd(cmd.as_deref().unwrap_or(""), &full_owned)
        };
        if let Some(s) = summary {
            let payload = serde_json::json!({"summary": s});
            let _ = super::manifest::update(
                &local_owned,
                &device_owned,
                &space_owned,
                &task,
                &[],
                Some(&payload),
                false,
            );
        }
    });
}

fn summarize_http(url: &str, content: &str) -> Option<String> {
    let model = std::env::var("NEXUSPOUCH_SUMMARY_MODEL")
        .unwrap_or_else(|_| "gpt-4o-mini".into());
    let token = std::env::var("NEXUSPOUCH_SUMMARY_TOKEN").unwrap_or_default();
    let body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": "Summarize in <=80 chars, plain text, Chinese or English matching input."},
            {"role": "user", "content": content}
        ],
        "max_tokens": 120,
    });
    let req = ureq::post(url).set("Content-Type", "application/json");
    let req = if token.is_empty() {
        req
    } else {
        req.set("Authorization", &format!("Bearer {token}"))
    };
    let resp = req.send_bytes(body.to_string().as_bytes()).ok()?;
    let raw = resp.into_string().ok()?;
    let val: Value = serde_json::from_str(&raw).ok()?;
    val["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
}

fn summarize_cmd(cmd: &str, full: &PathBuf) -> Option<String> {
    let out = std::process::Command::new(cmd).arg(full).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Reopen the same store root (for background threads; on-disk state shared).
pub fn reopen_local(local: &Local) -> Local {
    Local::open(&local.root, &local.device_id).expect("reopen store")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Local;

    const DEV: &str = "aaaaaaaaaaaaaaaa";

    #[test]
    fn index_search_remove() {
        let dir = tempfile::tempdir().unwrap();
        let idx = SearchIndex::open(dir.path());
        idx.index_file(&IndexEntry {
            uri: "store://artifacts/aaaaaaaaaaaaaaaa/t-1/report.md".into(),
            space: "artifacts".into(),
            device: DEV.into(),
            path: "t-1/report.md".into(),
            sha256: "a".repeat(64),
            size: 4,
            mtime: 0,
            task: "t-1".into(),
            state: "published".into(),
            summary: Some("Q2 sales report".into()),
            body: "revenue grew 20%".into(),
        });
        let hits = idx.search("revenue", None, None, None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["path"], "t-1/report.md");
        assert!(hits[0]["snippet"].as_str().unwrap().contains("revenue"));

        let hits = idx.search("grew", Some("files"), None, None, 10).unwrap();
        assert!(hits.is_empty());

        idx.remove("store://artifacts/aaaaaaaaaaaaaaaa/t-1/report.md");
        assert!(idx.search("revenue", None, None, None, 10).unwrap().is_empty());
    }

    #[test]
    fn rebuild_from_tree() {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::open(dir.path(), DEV).unwrap();
        let path = local.root.join(DEV).join("artifacts").join("t-1").join("notes.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "alpha beta gamma").unwrap();
        let idx = SearchIndex::open(dir.path());
        let n = idx.rebuild(&local).unwrap();
        assert!(n >= 1);
        let hits = idx.search("beta", None, None, None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["path"], "t-1/notes.md");
    }
}
