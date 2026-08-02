//! Reciprocal Rank Fusion for hybrid keyword + vector search (VECTOR P3).

use std::collections::HashMap;

/// Default RRF constant (Cormack et al.; common in search stacks).
pub const DEFAULT_K: f32 = 60.0;

/// Fuse multiple ranked URI lists. Rank is 1-based inside the formula
/// `score += 1 / (k + rank)`. Higher score is better.
pub fn fuse(lists: &[&[String]], k: f32) -> Vec<(String, f32)> {
    let k = if k > 0.0 { k } else { DEFAULT_K };
    let mut scores: HashMap<String, f32> = HashMap::new();
    for list in lists {
        for (i, uri) in list.iter().enumerate() {
            let rank = (i + 1) as f32;
            *scores.entry(uri.clone()).or_insert(0.0) += 1.0 / (k + rank);
        }
    }
    let mut out: Vec<(String, f32)> = scores.into_iter().collect();
    out.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_boosts_overlap() {
        let a = vec!["u1".into(), "u2".into(), "u3".into()];
        let b = vec!["u3".into(), "u1".into(), "u4".into()];
        let fused = fuse(&[&a, &b], DEFAULT_K);
        // Overlap beats single-list: u1 (ranks 1+2) > u2 (rank 2 only).
        assert_eq!(fused[0].0, "u1");
        assert!(fused.iter().any(|(u, _)| u == "u4"));
        let score_u1 = fused.iter().find(|(u, _)| u == "u1").unwrap().1;
        let score_u2 = fused.iter().find(|(u, _)| u == "u2").unwrap().1;
        let score_u3 = fused.iter().find(|(u, _)| u == "u3").unwrap().1;
        assert!(score_u1 > score_u2);
        assert!(score_u3 > score_u2);
    }
}
