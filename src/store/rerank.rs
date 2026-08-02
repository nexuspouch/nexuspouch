//! Rule-based second-stage ranking (RECALL_ACCURACY_DESIGN R2).
//!
//! Pipeline after coarse hybrid/keyword recall:
//! 1. attach mtime → time-decay boost
//! 2. keyword token hit boost on path/snippet
//! 3. blend with base (RRF / bm25 / vector) score
//! 4. optional sessions dedup by agent/project
//! 5. optional ±N turn fragment around the best matching message

use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Default coarse pool size before rerank.
pub const DEFAULT_COARSE: usize = 50;
/// Half-life for exponential time decay (days).
pub const DEFAULT_HALF_LIFE_DAYS: f64 = 30.0;

#[derive(Debug, Clone)]
pub struct RerankOpts {
    pub half_life_days: f64,
    pub base_weight: f64,
    pub time_weight: f64,
    pub keyword_weight: f64,
    pub now_ms: i64,
}

impl Default for RerankOpts {
    fn default() -> Self {
        Self {
            half_life_days: half_life_from_env(),
            base_weight: 0.45,
            time_weight: 0.30,
            keyword_weight: 0.25,
            now_ms: now_ms(),
        }
    }
}

pub fn rerank_enabled_from_env() -> bool {
    match std::env::var("NEXUSPOUCH_RERANK")
        .unwrap_or_else(|_| "rules".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "0" | "false" | "off" | "none" => false,
        _ => true,
    }
}

pub fn coarse_from_env(limit: usize) -> usize {
    std::env::var("NEXUSPOUCH_SEARCH_COARSE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_COARSE)
        .clamp(limit, 200)
}

fn half_life_from_env() -> f64 {
    std::env::var("NEXUSPOUCH_TIME_DECAY_DAYS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|d: &f64| *d > 0.0)
        .unwrap_or(DEFAULT_HALF_LIFE_DAYS)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Attach `mtime` from the index map when missing.
pub fn attach_mtimes(hits: &mut [Value], mtimes: &HashMap<String, i64>) {
    for h in hits.iter_mut() {
        let Some(obj) = h.as_object_mut() else {
            continue;
        };
        if obj.get("mtime").and_then(|v| v.as_i64()).is_some() {
            continue;
        }
        if let Some(uri) = obj.get("uri").and_then(|v| v.as_str()) {
            if let Some(m) = mtimes.get(uri) {
                obj.insert("mtime".into(), json!(m));
            }
        }
    }
}

/// Re-score and sort hits in place. Writes `rerank_score` and keeps `score` as
/// the final ranking score.
pub fn rule_rerank(q: &str, hits: &mut [Value], opts: &RerankOpts) {
    if hits.is_empty() {
        return;
    }
    let tokens = query_tokens(q);
    let mut base_scores: Vec<f64> = hits
        .iter()
        .map(|h| h.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0))
        .collect();
    // Normalize base scores to [0,1] so weights are comparable.
    let max_base = base_scores.iter().cloned().fold(0.0_f64, f64::max);
    let min_base = base_scores.iter().cloned().fold(f64::INFINITY, f64::min);
    let span = (max_base - min_base).abs().max(1e-9);
    for s in base_scores.iter_mut() {
        *s = (*s - min_base) / span;
    }

    let half = opts.half_life_days.max(1.0) * 86_400_000.0;
    for (i, h) in hits.iter_mut().enumerate() {
        let Some(obj) = h.as_object_mut() else {
            continue;
        };
        let mtime = obj.get("mtime").and_then(|v| v.as_i64()).unwrap_or(0);
        let age = (opts.now_ms - mtime).max(0) as f64;
        let time_s = (-age / half).exp(); // 1.0 for now, ~0.5 at half-life

        let path = obj.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let snip = obj.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
        let hay = format!("{path} {snip}").to_lowercase();
        let kw_s = if tokens.is_empty() {
            0.0
        } else {
            let hits_n = tokens.iter().filter(|t| hay.contains(t.as_str())).count();
            hits_n as f64 / tokens.len() as f64
        };

        let final_s = opts.base_weight * base_scores[i]
            + opts.time_weight * time_s
            + opts.keyword_weight * kw_s;
        obj.insert("score".into(), json!(final_s));
        obj.insert("rerank_score".into(), json!(final_s));
        obj.insert("score_time".into(), json!(time_s));
        obj.insert("score_keyword".into(), json!(kw_s));
        // Preserve original score_type (hybrid/keyword/vector); presence of
        // rerank_score signals second-stage ranking.
    }
    hits.sort_by(|a, b| {
        let sa = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let sb = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        sb.partial_cmp(&sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.get("uri")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .cmp(b.get("uri").and_then(|v| v.as_str()).unwrap_or(""))
            })
    });
}

/// Collapse sessions hits that share the same agent/project bucket.
/// Keeps the best-scoring hit per bucket and annotates occurrence metadata.
pub fn dedup_by_session_group(hits: Vec<Value>) -> Vec<Value> {
    if hits.len() <= 1 {
        return hits;
    }
    let mut groups: HashMap<String, Vec<Value>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for h in hits {
        let key = session_group_key(&h);
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(h);
    }
    let mut out = Vec::new();
    for key in order {
        let mut g = groups.remove(&key).unwrap_or_default();
        if g.is_empty() {
            continue;
        }
        g.sort_by(|a, b| {
            let sa = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sb = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut primary = g[0].clone();
        let n = g.len();
        let mut by_mtime = g.clone();
        by_mtime.sort_by_key(|h| h.get("mtime").and_then(|v| v.as_i64()).unwrap_or(0));
        let earliest = by_mtime
            .first()
            .and_then(|h| h.get("uri").cloned())
            .unwrap_or(Value::Null);
        let latest = by_mtime
            .last()
            .and_then(|h| h.get("uri").cloned())
            .unwrap_or(Value::Null);
        if let Some(obj) = primary.as_object_mut() {
            obj.insert("occurrences".into(), json!(n));
            obj.insert("group_key".into(), json!(key));
            if n > 1 {
                obj.insert("earliest_uri".into(), earliest);
                obj.insert("latest_uri".into(), latest);
                obj.insert(
                    "occurrence_label".into(),
                    json!(if n >= 3 {
                        "多次出现"
                    } else {
                        "最近讨论"
                    }),
                );
                let related: Vec<Value> = g
                    .iter()
                    .skip(1)
                    .filter_map(|h| h.get("uri").cloned())
                    .collect();
                obj.insert("related_uris".into(), json!(related));
            }
        }
        out.push(primary);
    }
    out
}

fn session_group_key(h: &Value) -> String {
    let device = h.get("device").and_then(|v| v.as_str()).unwrap_or("");
    let path = h.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        [agent, project, ..] if parts.len() >= 3 => format!("{device}/{agent}/{project}"),
        [agent, ..] => format!("{device}/{agent}"),
        _ => h
            .get("uri")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    }
}

/// Build a ±`window` turn fragment around the best query-matching turn.
pub fn session_fragment(turns: &[super::session_adapt::SessionTurn], q: &str, window: usize) -> Option<Map<String, Value>> {
    if turns.is_empty() {
        return None;
    }
    let tokens = query_tokens(q);
    let mut best_i = 0usize;
    let mut best_score = -1i32;
    for (i, t) in turns.iter().enumerate() {
        let hay = t.content.to_lowercase();
        let score = if tokens.is_empty() {
            0
        } else {
            tokens.iter().filter(|tok| hay.contains(tok.as_str())).count() as i32
        };
        if score > best_score {
            best_score = score;
            best_i = i;
        }
    }
    let start = best_i.saturating_sub(window);
    let end = (best_i + window + 1).min(turns.len());
    let fragment: Vec<Value> = turns[start..end]
        .iter()
        .enumerate()
        .map(|(offset, t)| {
            json!({
                "role": t.role,
                "content": t.content,
                "seq": t.seq,
                "focus": start + offset == best_i,
            })
        })
        .collect();
    let text = turns[start..end]
        .iter()
        .map(|t| format!("{}: {}", t.role, t.content))
        .collect::<Vec<_>>()
        .join("\n");
    Some(Map::from_iter([
        ("turns".into(), json!(fragment)),
        ("text".into(), json!(text)),
        ("focus_index".into(), json!(best_i - start)),
    ]))
}

fn query_tokens(q: &str) -> Vec<String> {
    q.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || ('\u{4e00}'..='\u{9fff}').contains(&c)))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        // Drop ultra-short Latin tokens; keep CJK singles.
        .filter(|s| s.chars().count() >= 2 || s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)))
        .take(16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(uri: &str, path: &str, score: f64, mtime: i64, snip: &str) -> Value {
        json!({
            "uri": uri,
            "path": path,
            "device": "aaaaaaaaaaaaaaaa",
            "space": "sessions",
            "score": score,
            "mtime": mtime,
            "snippet": snip,
        })
    }

    #[test]
    fn time_decay_prefers_recent() {
        let mut hits = vec![
            hit(
                "store://sessions/aaaaaaaaaaaaaaaa/a/old.jsonl",
                "a/old.jsonl",
                1.0,
                1_000,
                "deploy",
            ),
            hit(
                "store://sessions/aaaaaaaaaaaaaaaa/a/new.jsonl",
                "a/new.jsonl",
                1.0,
                1_720_000_000_000,
                "deploy",
            ),
        ];
        let opts = RerankOpts {
            now_ms: 1_720_000_000_000 + 86_400_000,
            half_life_days: 30.0,
            ..Default::default()
        };
        rule_rerank("deploy", &mut hits, &opts);
        assert!(hits[0]["uri"].as_str().unwrap().contains("new.jsonl"));
    }

    #[test]
    fn keyword_boost_lifts_path_hit() {
        let mut hits = vec![
            hit(
                "store://sessions/aaaaaaaaaaaaaaaa/a/x.jsonl",
                "a/x.jsonl",
                1.0,
                1_720_000_000_000,
                "unrelated text here",
            ),
            hit(
                "store://sessions/aaaaaaaaaaaaaaaa/a/y.jsonl",
                "a/rollback-plan.jsonl",
                0.5,
                1_720_000_000_000,
                "see notes",
            ),
        ];
        let opts = RerankOpts {
            now_ms: 1_720_000_000_000,
            base_weight: 0.2,
            time_weight: 0.0,
            keyword_weight: 0.8,
            ..Default::default()
        };
        rule_rerank("rollback", &mut hits, &opts);
        assert!(hits[0]["path"].as_str().unwrap().contains("rollback"));
    }

    #[test]
    fn dedup_keeps_best_and_annotates() {
        let hits = vec![
            hit(
                "store://sessions/aaaaaaaaaaaaaaaa/codex/shop/a.jsonl",
                "codex/shop/a.jsonl",
                0.9,
                2000,
                "库存",
            ),
            hit(
                "store://sessions/aaaaaaaaaaaaaaaa/codex/shop/b.jsonl",
                "codex/shop/b.jsonl",
                0.5,
                1000,
                "库存",
            ),
            hit(
                "store://sessions/aaaaaaaaaaaaaaaa/claude/other/c.jsonl",
                "claude/other/c.jsonl",
                0.7,
                1500,
                "别的",
            ),
        ];
        let out = dedup_by_session_group(hits);
        assert_eq!(out.len(), 2);
        let shop = out
            .iter()
            .find(|h| h["path"].as_str().unwrap().starts_with("codex/shop/"))
            .unwrap();
        assert_eq!(shop["occurrences"], 2);
        assert_eq!(shop["uri"], "store://sessions/aaaaaaaaaaaaaaaa/codex/shop/a.jsonl");
        assert!(shop.get("related_uris").is_some());
    }

    #[test]
    fn fragment_centers_on_match() {
        use crate::store::session_adapt::SessionTurn;
        let turns = vec![
            SessionTurn {
                agent: "a".into(),
                session_id: "s".into(),
                seq: 1,
                ts_ms: 0,
                role: "user".into(),
                content: "hello".into(),
            },
            SessionTurn {
                agent: "a".into(),
                session_id: "s".into(),
                seq: 2,
                ts_ms: 0,
                role: "assistant".into(),
                content: "deploy failed on rollback".into(),
            },
            SessionTurn {
                agent: "a".into(),
                session_id: "s".into(),
                seq: 3,
                ts_ms: 0,
                role: "user".into(),
                content: "thanks".into(),
            },
        ];
        let frag = session_fragment(&turns, "rollback", 1).unwrap();
        assert_eq!(frag["focus_index"], 1);
        assert_eq!(frag["turns"].as_array().unwrap().len(), 3);
    }
}
