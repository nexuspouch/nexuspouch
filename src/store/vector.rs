//! Vector index (VECTOR_SEARCH_DESIGN.md P1).
//!
//! SQLite table at `<root>/.system/vector/embeddings.db`:
//! `embeddings(uri, chunk_index, embedding BLOB, model, created_ms)`.
//! Small-scale brute-force cosine; HNSW deferred to P3.

use super::embed::{self, Embedder};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct VectorIndex {
    conn: Mutex<Option<Connection>>,
    embedder: Arc<dyn Embedder>,
}

impl VectorIndex {
    pub fn open(root: impl AsRef<Path>, embedder: Arc<dyn Embedder>) -> Self {
        let dir = root.as_ref().join(".system").join("vector");
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("embeddings.db");
        let conn = open_conn(&db_path).ok();
        Self {
            conn: Mutex::new(conn),
            embedder,
        }
    }

    pub fn embedder_name(&self) -> String {
        self.embedder.name().to_string()
    }

    pub fn available(&self) -> bool {
        self.embedder.available()
    }

    /// Embed and upsert chunk 0 for a document (summary/body already prepared).
    pub fn upsert_text(&self, uri: &str, text: &str) {
        if !self.embedder.available() {
            return;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        // Cap input: prefer head of text (summary+body already truncated upstream).
        let slice: String = trimmed.chars().take(4000).collect();
        let Ok(vec) = self.embedder.embed(&slice) else {
            return;
        };
        let blob = embed::packing(&vec);
        let Ok(mut guard) = self.conn.lock() else {
            return;
        };
        let Some(conn) = guard.as_mut() else {
            return;
        };
        let now = now_ms();
        let _ = conn.execute(
            "INSERT INTO embeddings(uri, chunk_index, embedding, model, created_ms)
             VALUES(?1, 0, ?2, ?3, ?4)
             ON CONFLICT(uri, chunk_index) DO UPDATE SET
               embedding=excluded.embedding,
               model=excluded.model,
               created_ms=excluded.created_ms",
            params![uri, blob, self.embedder.name(), now],
        );
    }

    pub fn remove(&self, uri: &str) {
        let Ok(mut guard) = self.conn.lock() else {
            return;
        };
        let Some(conn) = guard.as_mut() else {
            return;
        };
        let _ = conn.execute("DELETE FROM embeddings WHERE uri = ?1", params![uri]);
    }

    /// Brute-force cosine over stored vectors. Returns JSON hit objects with score.
    pub fn search(
        &self,
        q: &str,
        space: Option<&str>,
        device: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Value>, String> {
        if !self.embedder.available() {
            return Err("vector_unavailable".into());
        }
        let qv = self.embedder.embed(q)?;
        let guard = self.conn.lock().map_err(|e| e.to_string())?;
        let conn = guard
            .as_ref()
            .ok_or_else(|| "vector index unavailable".to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT uri, chunk_index, embedding, model FROM embeddings ORDER BY uri, chunk_index",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;

        let mut scored: Vec<(f32, String, i64, String)> = Vec::new();
        for row in rows {
            let (uri, chunk, blob, model) = row.map_err(|e| e.to_string())?;
            if let Some(sp) = space {
                if !uri.starts_with(&format!("store://{sp}/")) {
                    continue;
                }
            }
            if let Some(dev) = device {
                // store://space/device/...
                let parts: Vec<&str> = uri.trim_start_matches("store://").split('/').collect();
                if parts.get(1).copied() != Some(dev) {
                    continue;
                }
            }
            let vec = embed::unpacking(&blob);
            if vec.len() != qv.len() {
                continue;
            }
            let score = embed::cosine(&qv, &vec);
            scored.push((score, uri, chunk, model));
        }
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit.clamp(1, 200));
        Ok(scored
            .into_iter()
            .map(|(score, uri, chunk, model)| {
                let (space, device, path) = split_uri(&uri);
                json!({
                    "uri": uri,
                    "space": space,
                    "device": device,
                    "path": path,
                    "chunk_index": chunk,
                    "score": score,
                    "score_type": "vector",
                    "model": model,
                })
            })
            .collect())
    }
}

fn open_conn(path: &PathBuf) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS embeddings (
           uri TEXT NOT NULL,
           chunk_index INTEGER NOT NULL,
           embedding BLOB NOT NULL,
           model TEXT NOT NULL,
           created_ms INTEGER NOT NULL,
           PRIMARY KEY (uri, chunk_index)
         );",
    )?;
    Ok(conn)
}

fn split_uri(uri: &str) -> (String, String, String) {
    let rest = uri.strip_prefix("store://").unwrap_or(uri);
    let mut it = rest.splitn(3, '/');
    let space = it.next().unwrap_or("").to_string();
    let device = it.next().unwrap_or("").to_string();
    let path = it.next().unwrap_or("").to_string();
    (space, device, path)
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
    use crate::store::embed::HashingEmbedder;

    #[test]
    fn upsert_and_semantic_search() {
        let dir = tempfile::tempdir().unwrap();
        let emb: Arc<dyn Embedder> = Arc::new(HashingEmbedder {
            dims: HashingEmbedder::DEFAULT_DIMS,
        });
        let idx = VectorIndex::open(dir.path(), emb);
        idx.upsert_text(
            "store://artifacts/aaaaaaaaaaaaaaaa/t/a.md",
            "quarterly revenue grew twenty percent",
        );
        idx.upsert_text(
            "store://artifacts/aaaaaaaaaaaaaaaa/t/b.md",
            "cats and dogs playing in the garden",
        );
        let hits = idx
            .search("revenue growth report", Some("artifacts"), None, 5)
            .unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0]["path"], "t/a.md");
        assert_eq!(hits[0]["score_type"], "vector");
    }
}
