//! Online recall feedback loop (RECALL_ACCURACY_DESIGN R3).
//!
//! Records click / adopt / wrong signals against search hits into
//! `<root>/.system/recall_feedback.jsonl` for later export and eval
//! candidate generation. EventBus + audit are wired by the caller
//! (`Local::record_recall_feedback`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_NOTE_CHARS: usize = 2_000;
const MAX_QUERY_CHARS: usize = 2_000;

/// Positive/negative feedback kinds accepted by the API.
pub const KINDS: &[&str] = &["click", "adopt", "wrong"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedbackEntry {
    pub id: String,
    pub ts_ms: i64,
    /// `click` | `adopt` | `wrong`
    pub kind: String,
    pub query: String,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FeedbackFilter {
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub kind: Option<String>,
    pub limit: Option<usize>,
}

pub fn feedback_path(root: impl AsRef<Path>) -> PathBuf {
    root.as_ref().join(".system").join("recall_feedback.jsonl")
}

pub fn parse_kind(s: &str) -> Result<&str, String> {
    let k = s.trim().to_ascii_lowercase();
    KINDS
        .iter()
        .copied()
        .find(|x| *x == k)
        .ok_or_else(|| format!("kind must be one of {}", KINDS.join("|")))
}

pub fn validate_request(
    kind: &str,
    query: &str,
    uri: &str,
    note: Option<&str>,
) -> Result<(String, String, String, Option<String>), String> {
    let kind = parse_kind(kind)?.to_string();
    let query = query.trim();
    if query.is_empty() {
        return Err("query required".into());
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(format!("query too long (max {MAX_QUERY_CHARS} chars)"));
    }
    let uri = uri.trim();
    if uri.is_empty() || !uri.starts_with("store://") {
        return Err("uri must be a store:// URI".into());
    }
    let note = note
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let t: String = s.chars().take(MAX_NOTE_CHARS).collect();
            t
        });
    Ok((kind, query.to_string(), uri.to_string(), note))
}

pub fn append(root: impl AsRef<Path>, entry: &FeedbackEntry) -> Result<(), String> {
    let path = feedback_path(&root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > MAX_FILE_BYTES {
            let bak = path.with_extension("jsonl.1");
            let _ = fs::rename(&path, &bak);
        }
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())?;
    Ok(())
}

pub fn list(root: impl AsRef<Path>, filter: &FeedbackFilter) -> Result<Vec<FeedbackEntry>, String> {
    let path = feedback_path(&root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = fs::File::open(&path).map_err(|e| e.to_string())?;
    let reader = BufReader::new(f);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(e) = serde_json::from_str::<FeedbackEntry>(&line) else {
            continue;
        };
        if let Some(since) = filter.since_ms {
            if e.ts_ms < since {
                continue;
            }
        }
        if let Some(until) = filter.until_ms {
            if e.ts_ms > until {
                continue;
            }
        }
        if let Some(ref kind) = filter.kind {
            if !e.kind.eq_ignore_ascii_case(kind) {
                continue;
            }
        }
        out.push(e);
    }
    if let Some(limit) = filter.limit {
        if out.len() > limit {
            let start = out.len() - limit;
            out = out.split_off(start);
        }
    }
    Ok(out)
}

pub fn summary(entries: &[FeedbackEntry]) -> Map<String, Value> {
    let mut by_kind: BTreeMapCount = BTreeMapCount::default();
    for e in entries {
        by_kind.inc(&e.kind);
    }
    Map::from_iter([
        ("total".into(), json!(entries.len())),
        ("by_kind".into(), json!(by_kind.0)),
        (
            "positive".into(),
            json!(entries.iter().filter(|e| e.kind == "click" || e.kind == "adopt").count()),
        ),
        (
            "negative".into(),
            json!(entries.iter().filter(|e| e.kind == "wrong").count()),
        ),
    ])
}

/// Build reviewable eval-candidate objects from positive feedback
/// (click/adopt). Gold URI = the feedback uri; human still decides.
pub fn as_eval_candidates(entries: &[FeedbackEntry]) -> Vec<Value> {
    entries
        .iter()
        .filter(|e| e.kind == "click" || e.kind == "adopt")
        .map(|e| {
            json!({
                "q": e.query,
                "gold": [e.uri],
                "type": "feedback",
                "note": e.note.clone().unwrap_or_else(|| format!("from {} feedback {}", e.kind, e.id)),
                "source_feedback_id": e.id,
                "source_ts_ms": e.ts_ms,
            })
        })
        .collect()
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Default)]
struct BTreeMapCount(std::collections::BTreeMap<String, u64>);

impl BTreeMapCount {
    fn inc(&mut self, k: &str) {
        *self.0.entry(k.to_string()).or_default() += 1;
    }
}

/// Apply env overrides for the duration of `f`, then restore.
pub fn with_env_overrides<T>(
    overrides: &[(String, Option<String>)],
    f: impl FnOnce() -> T,
) -> T {
    let mut saved: Vec<(String, Option<std::ffi::OsString>)> = Vec::new();
    for (k, v) in overrides {
        saved.push((k.clone(), std::env::var_os(k)));
        match v {
            Some(val) => std::env::set_var(k, val),
            None => std::env::remove_var(k),
        }
    }
    let out = f();
    for (k, prev) in saved {
        match prev {
            Some(v) => std::env::set_var(&k, v),
            None => std::env::remove_var(&k),
        }
    }
    out
}

/// Parse `KEY=VAL` pairs for A/B env sides. Empty VAL unsets.
pub fn parse_env_pairs(pairs: &[String]) -> Result<Vec<(String, Option<String>)>, String> {
    let mut out = Vec::new();
    for p in pairs {
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| format!("expected KEY=VAL, got {p}"))?;
        let k = k.trim();
        if k.is_empty() {
            return Err(format!("empty key in {p}"));
        }
        let v = v.trim();
        out.push((
            k.to_string(),
            if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            },
        ));
    }
    Ok(out)
}

/// Side-by-side A/B comparison of two eval reports (R3).
#[derive(Debug, Clone)]
pub struct AbReport {
    pub label_a: String,
    pub label_b: String,
    pub a: super::recall_eval::EvalReport,
    pub b: super::recall_eval::EvalReport,
}

impl AbReport {
    pub fn run(
        fixture: &Path,
        k: usize,
        label_a: &str,
        env_a: &[(String, Option<String>)],
        semantic_a: bool,
        label_b: &str,
        env_b: &[(String, Option<String>)],
        semantic_b: bool,
    ) -> Result<Self, super::OpError> {
        let a = with_env_overrides(env_a, || super::recall_eval::run_eval(fixture, k, semantic_a))?;
        let b = with_env_overrides(env_b, || super::recall_eval::run_eval(fixture, k, semantic_b))?;
        Ok(Self {
            label_a: label_a.to_string(),
            label_b: label_b.to_string(),
            a,
            b,
        })
    }
}

impl AbReport {
    pub fn to_json(&self) -> Value {
        let delta = |a: f64, b: f64| json!({"a": a, "b": b, "delta": b - a});
        json!({
            "label_a": self.label_a,
            "label_b": self.label_b,
            "k": self.a.k,
            "metrics": {
                "recall_at_1": delta(self.a.recall_at_1, self.b.recall_at_1),
                "recall_at_5": delta(self.a.recall_at_5, self.b.recall_at_5),
                "recall_at_k": delta(self.a.recall_at_k, self.b.recall_at_k),
                "mrr": delta(self.a.mrr, self.b.mrr),
                "ndcg_at_k": delta(self.a.ndcg_at_k, self.b.ndcg_at_k),
            },
            "zero_hit": {
                "a": self.a.zero_hit_queries,
                "b": self.b.zero_hit_queries,
            },
            "a": self.a.to_json(),
            "b": self.b.to_json(),
        })
    }

    pub fn render_table(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "recall ab  A={}  B={}  k={}\n",
            self.label_a, self.label_b, self.a.k
        ));
        s.push_str(&format!(
            "{:<14} {:>8} {:>8} {:>8}\n",
            "metric", "A", "B", "Δ(B-A)"
        ));
        for (name, a, b) in [
            ("R@1", self.a.recall_at_1, self.b.recall_at_1),
            ("R@5", self.a.recall_at_5, self.b.recall_at_5),
            ("R@k", self.a.recall_at_k, self.b.recall_at_k),
            ("MRR", self.a.mrr, self.b.mrr),
            ("nDCG@k", self.a.ndcg_at_k, self.b.ndcg_at_k),
        ] {
            s.push_str(&format!(
                "{:<14} {:>8.3} {:>8.3} {:>+8.3}\n",
                name,
                a,
                b,
                b - a
            ));
        }
        s.push_str(&format!(
            "zero-hit        {:>8} {:>8}\n",
            self.a.zero_hit_queries, self.b.zero_hit_queries
        ));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_and_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let (kind, q, uri, note) = validate_request(
            "Adopt",
            "  deploy error  ",
            "store://sessions/aaaaaaaaaaaaaaaa/claude/p/s.jsonl",
            Some(" worked "),
        )
        .unwrap();
        assert_eq!(kind, "adopt");
        assert_eq!(q, "deploy error");
        assert!(note.as_deref() == Some("worked"));

        let e = FeedbackEntry {
            id: new_id(),
            ts_ms: 1000,
            kind,
            query: q,
            uri,
            rank: Some(1),
            note,
            space: Some("sessions".into()),
            score_type: Some("hybrid".into()),
            agent_id: None,
            device: Some("aaaaaaaaaaaaaaaa".into()),
            source: Some("test".into()),
        };
        append(dir.path(), &e).unwrap();
        append(
            dir.path(),
            &FeedbackEntry {
                id: new_id(),
                ts_ms: 2000,
                kind: "wrong".into(),
                query: "other".into(),
                uri: "store://sessions/aaaaaaaaaaaaaaaa/x.jsonl".into(),
                rank: Some(3),
                note: None,
                space: None,
                score_type: None,
                agent_id: None,
                device: None,
                source: None,
            },
        )
        .unwrap();

        let all = list(dir.path(), &FeedbackFilter::default()).unwrap();
        assert_eq!(all.len(), 2);
        let pos = list(
            dir.path(),
            &FeedbackFilter {
                kind: Some("adopt".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(pos.len(), 1);
        let sum = summary(&all);
        assert_eq!(sum["total"], 2);
        assert_eq!(sum["positive"], 1);
        assert_eq!(sum["negative"], 1);

        let cands = as_eval_candidates(&all);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0]["q"], "deploy error");
    }

    #[test]
    fn rejects_bad_kind_and_uri() {
        assert!(validate_request("meh", "q", "store://a/b", None).is_err());
        assert!(validate_request("click", "", "store://a/b", None).is_err());
        assert!(validate_request("click", "q", "http://x", None).is_err());
    }
}
