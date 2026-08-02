//! Retrieval evaluation harness (RECALL_ACCURACY_DESIGN R1).
//!
//! Loads a self-contained eval fixture (`docs/storage_fixtures/recall_eval.json`),
//! materializes the corpus into a temp store (sessions space), rebuilds the
//! real index pipeline (adapt → scrub → FTS5 + vector chunks), then runs each
//! query through `Local::search_query` and reports Recall@K / MRR / nDCG,
//! grouped by query type.
//!
//! Determinism: run with the hashing embedder (`NEXUSPOUCH_EMBED=hash`,
//! default under `cargo test`; the `recall eval` CLI forces it unless the
//! operator picked an embedder explicitly).

use super::{Local, OpError, SearchFilter};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

pub const DEFAULT_FIXTURE: &str = "docs/storage_fixtures/recall_eval.json";
pub const EVAL_DEVICE: &str = "aaaaaaaaaaaaaaaa";

#[derive(Debug, Clone, Deserialize)]
pub struct EvalFixture {
    pub version: u32,
    #[serde(default)]
    pub device: Option<String>,
    pub corpus: Vec<CorpusDoc>,
    pub queries: Vec<EvalQuery>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CorpusDoc {
    /// Relative path inside the sessions space (`<agent>/<project>/<file>`).
    pub path: String,
    /// Raw transcript content (JSONL, claude/codex/acp formats).
    pub content: String,
    /// Optional mtime override so time-range filters are exercisable.
    #[serde(default)]
    pub mtime_ms: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EvalQuery {
    pub q: String,
    /// Gold store URIs (`store://sessions/<device>/<path>`).
    pub gold: Vec<String>,
    #[serde(default = "default_qtype")]
    pub r#type: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub filter: Option<QueryFilter>,
}

fn default_qtype() -> String {
    "semantic".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct QueryFilter {
    pub agent: Option<String>,
    pub project: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
}

impl From<&QueryFilter> for SearchFilter {
    fn from(f: &QueryFilter) -> Self {
        SearchFilter {
            agent: f.agent.clone(),
            project: f.project.clone(),
            since_ms: f.since_ms,
            until_ms: f.until_ms,
        }
    }
}

// ---------------------------------------------------------------------------
// Offline metrics (pure functions over ranked URI lists).

/// Fraction of gold URIs present in the top-k of `ranked`.
pub fn recall_at_k(ranked: &[String], gold: &[String], k: usize) -> f64 {
    if gold.is_empty() {
        return 0.0;
    }
    let k = k.max(1);
    let hits = gold
        .iter()
        .filter(|g| ranked.iter().take(k).any(|u| u == *g))
        .count();
    hits as f64 / gold.len() as f64
}

/// Reciprocal rank of the first gold hit (0 when absent).
pub fn mrr(ranked: &[String], gold: &[String]) -> f64 {
    for (i, u) in ranked.iter().enumerate() {
        if gold.iter().any(|g| g == u) {
            return 1.0 / (i + 1) as f64;
        }
    }
    0.0
}

/// nDCG@k with binary relevance (gold = relevant, everything else not).
pub fn ndcg(ranked: &[String], gold: &[String], k: usize) -> f64 {
    if gold.is_empty() {
        return 0.0;
    }
    let k = k.max(1);
    let mut dcg = 0.0;
    for (i, u) in ranked.iter().take(k).enumerate() {
        if gold.iter().any(|g| g == u) {
            dcg += 1.0 / (i as f64 + 2.0).log2();
        }
    }
    let mut idcg = 0.0;
    for i in 0..gold.len().min(k) {
        idcg += 1.0 / (i as f64 + 2.0).log2();
    }
    if idcg <= 0.0 {
        0.0
    } else {
        dcg / idcg
    }
}

// ---------------------------------------------------------------------------
// Report structures.

#[derive(Debug, Clone, Serialize)]
pub struct QueryOutcome {
    pub q: String,
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub gold: Vec<String>,
    /// Deduped ranked URIs (top-k as returned by the search pipeline).
    pub ranked: Vec<String>,
    /// 1-based rank of the first gold hit; null when absent.
    pub first_hit_rank: Option<usize>,
    pub recall_at_k: f64,
    pub mrr: f64,
    pub ndcg_at_k: f64,
    pub score_type: String,
    pub degraded: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TypeReport {
    pub r#type: String,
    pub n: usize,
    pub recall_at_1: f64,
    pub recall_at_5: f64,
    pub recall_at_k: f64,
    pub mrr: f64,
    pub ndcg_at_k: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalReport {
    pub fixture: String,
    pub device: String,
    pub embedder: String,
    /// `hybrid` (semantic) or `keyword`.
    pub mode: String,
    pub k: usize,
    pub corpus_docs: usize,
    pub total_queries: usize,
    /// Queries with no gold hit at any returned rank.
    pub zero_hit_queries: usize,
    pub recall_at_1: f64,
    pub recall_at_5: f64,
    pub recall_at_k: f64,
    pub mrr: f64,
    pub ndcg_at_k: f64,
    pub by_type: Vec<TypeReport>,
    pub queries: Vec<QueryOutcome>,
}

impl EvalReport {
    /// Severe-regression gate for CI: over half the queries hit nothing, or
    /// Recall@K below the (optional) caller gate.
    pub fn is_degraded(&self, min_recall: f64) -> bool {
        let zero_frac = self.zero_hit_queries as f64 / self.total_queries.max(1) as f64;
        zero_frac > 0.5 || self.recall_at_k < min_recall
    }

    pub fn to_json(&self) -> Value {
        json!(self)
    }
}

// ---------------------------------------------------------------------------
// Fixture loading + store construction.

pub fn default_fixture_path() -> PathBuf {
    let candidates = [
        PathBuf::from(DEFAULT_FIXTURE),
        PathBuf::from("..").join(DEFAULT_FIXTURE),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_FIXTURE),
    ];
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_FIXTURE))
}

pub fn load_fixture(path: &Path) -> Result<EvalFixture, OpError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        OpError::new("bad_fixture", format!("read {}: {e}", path.display()))
    })?;
    let fixture: EvalFixture = serde_json::from_str(&raw).map_err(|e| {
        OpError::new("bad_fixture", format!("parse {}: {e}", path.display()))
    })?;
    if fixture.version != 1 {
        return Err(OpError::new(
            "bad_fixture",
            format!("unsupported fixture version {}", fixture.version),
        ));
    }
    if fixture.corpus.is_empty() || fixture.queries.is_empty() {
        return Err(OpError::new("bad_fixture", "corpus/queries must be non-empty"));
    }
    for doc in &fixture.corpus {
        let p = Path::new(&doc.path);
        if p.is_absolute()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(OpError::new(
                "bad_fixture",
                format!("corpus path escapes space: {}", doc.path),
            ));
        }
    }
    for query in &fixture.queries {
        if query.gold.is_empty() {
            return Err(OpError::new(
                "bad_fixture",
                format!("query without gold: {}", query.q),
            ));
        }
    }
    Ok(fixture)
}

/// Materialize the corpus under `<root>/<device>/sessions/` and run the real
/// index pipeline (`Local::rebuild_index`).
pub fn build_store(root: &Path, device: &str, fixture: &EvalFixture) -> Result<Local, OpError> {
    let store = Local::open(root, device)?;
    let base = root.join(device).join(super::spaces::SESSIONS_SPACE);
    for doc in &fixture.corpus {
        let full = base.join(&doc.path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(super::io_err)?;
        }
        std::fs::write(&full, &doc.content).map_err(super::io_err)?;
        if let Some(ms) = doc.mtime_ms.filter(|ms| *ms >= 0) {
            let f = std::fs::File::options()
                .write(true)
                .open(&full)
                .map_err(super::io_err)?;
            f.set_modified(UNIX_EPOCH + Duration::from_millis(ms as u64))
                .map_err(super::io_err)?;
        }
    }
    store.rebuild_index()?;
    Ok(store)
}

// ---------------------------------------------------------------------------
// Evaluation.

/// Full harness: fixture → temp store → per-query runs → aggregated report.
pub fn run_eval(fixture_path: &Path, k: usize, semantic: bool) -> Result<EvalReport, OpError> {
    let fixture = load_fixture(fixture_path)?;
    let device = fixture
        .device
        .clone()
        .unwrap_or_else(|| EVAL_DEVICE.to_string());
    let dir = tempfile::tempdir().map_err(super::io_err)?;
    let store = build_store(dir.path(), &device, &fixture)?;
    evaluate(&store, &fixture, fixture_path, k, semantic)
}

/// Run every fixture query against an already-built store.
pub fn evaluate(
    store: &Local,
    fixture: &EvalFixture,
    fixture_path: &Path,
    k: usize,
    semantic: bool,
) -> Result<EvalReport, OpError> {
    let k = k.clamp(1, 200);
    let mut outcomes = Vec::with_capacity(fixture.queries.len());
    for query in &fixture.queries {
        let filter = query.filter.as_ref().map(SearchFilter::from);
        let out = store.search_query(
            &query.q,
            Some(super::spaces::SESSIONS_SPACE),
            None,
            None,
            k,
            semantic,
            filter.as_ref(),
        )?;
        // Dedupe: vector hits are per-chunk, fusion per-URI.
        let mut ranked: Vec<String> = Vec::new();
        for hit in out
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
        {
            if let Some(u) = hit.get("uri").and_then(|v| v.as_str()) {
                if !ranked.iter().any(|r| r == u) {
                    ranked.push(u.to_string());
                }
            }
        }
        let first_hit_rank = ranked
            .iter()
            .position(|u| query.gold.iter().any(|g| g == u))
            .map(|i| i + 1);
        outcomes.push(QueryOutcome {
            q: query.q.clone(),
            r#type: query.r#type.clone(),
            note: query.note.clone(),
            gold: query.gold.clone(),
            recall_at_k: recall_at_k(&ranked, &query.gold, k),
            mrr: mrr(&ranked, &query.gold),
            ndcg_at_k: ndcg(&ranked, &query.gold, k),
            first_hit_rank,
            ranked,
            score_type: out
                .get("score_type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            degraded: out
                .get("degraded")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        });
    }

    let agg = |pick: &dyn Fn(&QueryOutcome) -> f64| -> f64 {
        if outcomes.is_empty() {
            0.0
        } else {
            outcomes.iter().map(pick).sum::<f64>() / outcomes.len() as f64
        }
    };
    let recall_at_1 = agg(&|o| recall_at_k(&o.ranked, &o.gold, 1));
    let recall_at_5 = agg(&|o| recall_at_k(&o.ranked, &o.gold, 5));
    let recall_at_k_agg = agg(&|o| o.recall_at_k);
    let mrr_agg = agg(&|o| o.mrr);
    let ndcg_agg = agg(&|o| o.ndcg_at_k);

    let mut by_type_map: BTreeMap<String, Vec<&QueryOutcome>> = BTreeMap::new();
    for o in &outcomes {
        by_type_map.entry(o.r#type.clone()).or_default().push(o);
    }
    let by_type = by_type_map
        .into_iter()
        .map(|(ty, group)| {
            let n = group.len() as f64;
            let avg = |pick: &dyn Fn(&QueryOutcome) -> f64| {
                group.iter().map(|o| pick(o)).sum::<f64>() / n
            };
            TypeReport {
                r#type: ty,
                n: group.len(),
                recall_at_1: avg(&|o| recall_at_k(&o.ranked, &o.gold, 1)),
                recall_at_5: avg(&|o| recall_at_k(&o.ranked, &o.gold, 5)),
                recall_at_k: avg(&|o| o.recall_at_k),
                mrr: avg(&|o| o.mrr),
                ndcg_at_k: avg(&|o| o.ndcg_at_k),
            }
        })
        .collect();

    Ok(EvalReport {
        fixture: fixture_path.display().to_string(),
        device: store.device_id.clone(),
        embedder: store.embedder_name(),
        mode: if semantic { "hybrid" } else { "keyword" }.into(),
        k,
        corpus_docs: fixture.corpus.len(),
        total_queries: outcomes.len(),
        zero_hit_queries: outcomes.iter().filter(|o| o.first_hit_rank.is_none()).count(),
        recall_at_1,
        recall_at_5,
        recall_at_k: recall_at_k_agg,
        mrr: mrr_agg,
        ndcg_at_k: ndcg_agg,
        by_type,
        queries: outcomes,
    })
}

// ---------------------------------------------------------------------------
// Human-readable rendering (CLI default; --json uses EvalReport::to_json).

pub fn render_table(report: &EvalReport) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "recall eval  fixture={}\n",
        report.fixture
    ));
    s.push_str(&format!(
        "device={}  embedder={}  mode={}  k={}\n",
        report.device, report.embedder, report.mode, report.k
    ));
    let zero_frac = report.zero_hit_queries as f64 / report.total_queries.max(1) as f64;
    s.push_str(&format!(
        "corpus={} docs  queries={}  zero-hit={} ({:.1}%)\n\n",
        report.corpus_docs,
        report.total_queries,
        report.zero_hit_queries,
        zero_frac * 100.0
    ));
    // Columns: R@1, R@5, R@k — collapse duplicates when k == 1 or 5.
    let show_r5 = report.k != 1;
    let show_rk = report.k != 1 && report.k != 5;
    let row = |name: &str, n: usize, r1: f64, r5: f64, rk: f64, mrr: f64, ndcg: f64| {
        let mut line = format!("{name:<14} {n:>3} {r1:>7.3}");
        if show_r5 {
            line.push_str(&format!(" {r5:>7.3}"));
        }
        if show_rk {
            line.push_str(&format!(" {rk:>7.3}"));
        }
        line.push_str(&format!(" {mrr:>7.3} {ndcg:>9.3}\n"));
        line
    };
    let mut header = format!("{:<14} {:>3} {:>7}", "type", "n", "R@1");
    if show_r5 {
        header.push_str(&format!(" {:>7}", "R@5"));
    }
    if show_rk {
        header.push_str(&format!(" {:>7}", format!("R@{}", report.k)));
    }
    header.push_str(&format!(" {:>7} {:>9}\n", "MRR", format!("nDCG@{}", report.k)));
    s.push_str(&header);
    for t in &report.by_type {
        s.push_str(&row(
            &t.r#type,
            t.n,
            t.recall_at_1,
            t.recall_at_5,
            t.recall_at_k,
            t.mrr,
            t.ndcg_at_k,
        ));
    }
    s.push_str(&row(
        "ALL",
        report.total_queries,
        report.recall_at_1,
        report.recall_at_5,
        report.recall_at_k,
        report.mrr,
        report.ndcg_at_k,
    ));
    let misses: Vec<&QueryOutcome> = report
        .queries
        .iter()
        .filter(|o| o.first_hit_rank.is_none())
        .collect();
    if !misses.is_empty() {
        s.push_str(&format!("\nmisses (no gold in top-{}):\n", report.k));
        for o in misses {
            s.push_str(&format!(
                "  [{}] {:?} gold={}\n",
                o.r#type,
                o.q,
                o.gold.join(", ")
            ));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uris(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn recall_at_k_math() {
        let ranked = uris(&["a", "b", "c", "d"]);
        let gold = uris(&["b", "x"]);
        assert_eq!(recall_at_k(&ranked, &gold, 1), 0.0);
        assert_eq!(recall_at_k(&ranked, &gold, 2), 0.5);
        assert_eq!(recall_at_k(&ranked, &gold, 10), 0.5);
        let gold_a = uris(&["a"]);
        assert_eq!(recall_at_k(&ranked, &gold_a, 1), 1.0);
        assert_eq!(recall_at_k(&ranked, &[], 5), 0.0);
        assert_eq!(recall_at_k(&[], &gold_a, 5), 0.0);
    }

    #[test]
    fn mrr_math() {
        let ranked = uris(&["a", "b", "c"]);
        assert_eq!(mrr(&ranked, &uris(&["c"])), 1.0 / 3.0);
        assert_eq!(mrr(&ranked, &uris(&["a"])), 1.0);
        assert_eq!(mrr(&ranked, &uris(&["x"])), 0.0);
        // First hit counts, not the best gold.
        assert_eq!(mrr(&ranked, &uris(&["c", "b"])), 0.5);
    }

    #[test]
    fn ndcg_math() {
        let ranked = uris(&["a", "b", "c"]);
        // gold {b}, k=3: dcg = 1/log2(3), idcg = 1.
        let g = uris(&["b"]);
        let expect = 1.0 / 3.0f64.log2();
        assert!((ndcg(&ranked, &g, 3) - expect).abs() < 1e-9);
        // gold {a,b}, k=2: perfect ranking → 1.0.
        let g2 = uris(&["a", "b"]);
        assert!((ndcg(&ranked, &g2, 2) - 1.0).abs() < 1e-9);
        // No hits → 0; empty gold → 0.
        assert_eq!(ndcg(&ranked, &uris(&["x"]), 3), 0.0);
        assert_eq!(ndcg(&ranked, &[], 3), 0.0);
        // k caps the window: gold at rank 3 invisible at k=2.
        let g3 = uris(&["c"]);
        assert_eq!(ndcg(&ranked, &g3, 2), 0.0);
        assert!(ndcg(&ranked, &g3, 3) > 0.0);
    }

    #[test]
    fn fixture_eval_baseline_hits() {
        // Hermetic: cfg(test) embedder defaults to local-hash-v0.
        let path = default_fixture_path();
        let report = run_eval(&path, 10, true).expect("eval run");
        assert_eq!(report.embedder, "local-hash-v0");
        assert_eq!(report.corpus_docs, 16);
        assert_eq!(report.total_queries, 36);
        assert!(report.recall_at_k > 0.9, "recall@10 = {}", report.recall_at_k);
        assert!(report.mrr > 0.7, "mrr = {}", report.mrr);
        assert!(report.ndcg_at_k > 0.85, "ndcg@10 = {}", report.ndcg_at_k);

        // Representative queries must hit their gold.
        let hit = |q: &str| {
            report
                .queries
                .iter()
                .find(|o| o.q == q)
                .unwrap_or_else(|| panic!("query missing: {q}"))
                .first_hit_rank
        };
        assert_eq!(hit("9f3c2ab7d1e8"), Some(1)); // exact commit hash
        assert_eq!(hit("ImagePullBackOff"), Some(1)); // exact error string
        assert!(hit("上次部署报错怎么解决的").is_some()); // semantic zh
        assert!(hit("训练时 loss 变成 NaN 怎么办").is_some()); // semantic mixed
        // Time-filtered pair isolates the two Redis sessions by month.
        let redis_july = report
            .queries
            .iter()
            .find(|o| o.q == "Redis" && o.gold.iter().any(|g| g.contains("sess-203")))
            .expect("redis july query");
        assert_eq!(redis_july.first_hit_rank, Some(1));
        let redis_june = report
            .queries
            .iter()
            .find(|o| o.q == "Redis" && o.gold.iter().any(|g| g.contains("sess-201")))
            .expect("redis june query");
        assert_eq!(redis_june.first_hit_rank, Some(1));
    }

    #[test]
    fn fixture_eval_keyword_mode_runs() {
        // Keyword-only comparison mode: exact-term queries still hit.
        let path = default_fixture_path();
        let report = run_eval(&path, 10, false).expect("eval run");
        assert_eq!(report.mode, "keyword");
        let exact = report.by_type.iter().find(|t| t.r#type == "exact").unwrap();
        assert!(exact.recall_at_k > 0.9, "exact R@10 = {}", exact.recall_at_k);
    }

    #[test]
    fn degraded_gate() {
        let report = EvalReport {
            fixture: "x".into(),
            device: EVAL_DEVICE.into(),
            embedder: "local-hash-v0".into(),
            mode: "hybrid".into(),
            k: 10,
            corpus_docs: 1,
            total_queries: 4,
            zero_hit_queries: 3,
            recall_at_1: 0.0,
            recall_at_5: 0.0,
            recall_at_k: 0.25,
            mrr: 0.1,
            ndcg_at_k: 0.2,
            by_type: vec![],
            queries: vec![],
        };
        assert!(report.is_degraded(0.0)); // zero-hit ratio > 50%
        assert!(report.is_degraded(0.5)); // recall below gate
        let ok = EvalReport {
            zero_hit_queries: 1,
            recall_at_k: 0.75,
            ..report
        };
        assert!(!ok.is_degraded(0.5));
    }
}
