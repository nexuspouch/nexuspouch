use super::{browse, Local};
use chrono::{Datelike, Local as ChronoLocal, NaiveDate};
use serde_json::Value;
use std::fs;
use std::path::Path;

struct TopEntry {
    id: String,
    mtime_ms: i64,
}

impl Clone for TopEntry {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            mtime_ms: self.mtime_ms,
        }
    }
}

/// Runs optional commit.retention after a successful promote.
pub fn apply_retention(local: &Local, device: &str, space: &str, raw: &Value) {
    let Some(m) = raw.as_object() else {
        return;
    };
    let policy = m.get("policy").and_then(|v| v.as_str()).unwrap_or("");
    let include = m.get("include_prefix").and_then(|v| v.as_str()).unwrap_or("");
    let exclude = m.get("exclude_prefix").and_then(|v| v.as_str()).unwrap_or("");
    let dir = local.root.join(device).join(space);
    let entries = match list_top_level(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut filtered = Vec::new();
    for e in entries {
        if !include.is_empty() && !e.id.starts_with(include) {
            continue;
        }
        if !exclude.is_empty() && e.id.starts_with(exclude) {
            continue;
        }
        filtered.push(e);
    }
    let to_delete = match policy {
        "keep_last" => {
            let keep = int_from(m.get("keep"), -1);
            if keep < 0 {
                return;
            }
            select_keep_last(&filtered, keep)
        }
        "gfs" => {
            let daily = int_from(m.get("daily"), 7);
            let weekly = int_from(m.get("weekly"), 28);
            let monthly = int_from(m.get("monthly"), 12);
            select_gfs_delete(&filtered, daily, weekly, monthly)
        }
        _ => return,
    };
    for id in to_delete {
        let _ = browse::move_to_recycle(local, device, space, &id);
    }
}

fn list_top_level(dir: &Path) -> Result<Vec<TopEntry>, std::io::Error> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let info = entry.metadata()?;
        let mtime_ms = info
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        out.push(TopEntry { id: name, mtime_ms });
    }
    Ok(out)
}

fn int_from(v: Option<&Value>, def: i32) -> i32 {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .map(|x| x as i32)
            .unwrap_or(def),
        _ => def,
    }
}

fn select_keep_last(entries: &[TopEntry], keep: i32) -> Vec<String> {
    let mut sorted: Vec<_> = entries.to_vec();
    sorted.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    if sorted.len() <= keep as usize {
        return Vec::new();
    }
    sorted[keep as usize..]
        .iter()
        .map(|e| e.id.clone())
        .collect()
}

/// Mirrors Dart gfs_retention.selectGfs deleteIds.
fn select_gfs_delete(entries: &[TopEntry], daily: i32, weekly: i32, monthly: i32) -> Vec<String> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<_> = entries.to_vec();
    sorted.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    let now = ChronoLocal::now();
    let mut keep = std::collections::HashSet::new();
    let mut seen_days = std::collections::HashSet::new();
    let mut seen_weeks = std::collections::HashSet::new();
    let mut seen_months = std::collections::HashSet::new();

    for e in &sorted {
        let t = chrono::DateTime::from_timestamp_millis(e.mtime_ms)
            .map(|dt| dt.with_timezone(&ChronoLocal))
            .unwrap_or_else(|| now);
        let day_key = format!("{}-{}-{}", t.year(), t.month(), t.day());
        let year = t.iso_week().year();
        let week_num = t.iso_week().week();
        let week_key = format!("{year}-W{week_num}");
        let month_key = t.year() * 12 + t.month() as i32;

        let day_diff = date_only(now).signed_duration_since(date_only(t)).num_days();
        let week_start_now = date_only(now)
            - chrono::Duration::days(now.weekday().num_days_from_monday() as i64);
        let week_start_t = date_only(t)
            - chrono::Duration::days(t.weekday().num_days_from_monday() as i64);
        let week_start_diff = week_start_now.signed_duration_since(week_start_t).num_days();
        let month_diff = (now.year() * 12 + now.month() as i32) - month_key;

        if day_diff < daily as i64 {
            if seen_days.insert(day_key) {
                keep.insert(e.id.clone());
            }
        }
        if week_start_diff < weekly as i64 {
            if seen_weeks.insert(week_key) {
                keep.insert(e.id.clone());
            }
        }
        if month_diff < monthly {
            if seen_months.insert(month_key) {
                keep.insert(e.id.clone());
            }
        }
    }

    sorted
        .into_iter()
        .filter(|e| !keep.contains(&e.id))
        .map(|e| e.id)
        .collect()
}

fn date_only(t: chrono::DateTime<ChronoLocal>) -> NaiveDate {
    t.date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol;
    use crate::protocol::Frame;
    use crate::store::Local;
    use base64::Engine;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::thread;
    use std::time::Duration as StdDuration;
    use tempfile::tempdir;

    fn put_file(store: &Local, device: &str, path: &str, content: &[u8]) {
        let sha = hex::encode(Sha256::digest(content));
        let mut begin_payload = serde_json::Map::new();
        begin_payload.insert("space".into(), json!("backups"));
        begin_payload.insert("path".into(), json!(path));
        begin_payload.insert("size".into(), json!(content.len()));
        begin_payload.insert("sha256".into(), json!(sha));
        let begin = store
            .handle(
                Frame::from_parts("write.begin", begin_payload),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
        let upload_id = begin["upload_id"].as_str().unwrap().to_string();
        let mut chunk_payload = serde_json::Map::new();
        chunk_payload.insert("upload_id".into(), json!(upload_id));
        chunk_payload.insert("offset".into(), json!(0));
        chunk_payload.insert(
            "data".into(),
            json!(base64::engine::general_purpose::STANDARD.encode(content)),
        );
        store
            .handle(
                Frame::from_parts("write.chunk", chunk_payload),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
        let mut commit_payload = serde_json::Map::new();
        commit_payload.insert("space".into(), json!("backups"));
        commit_payload.insert("upload_ids".into(), json!([upload_id]));
        store
            .handle(
                Frame::from_parts("commit", commit_payload),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
    }

    #[test]
    fn retention_keep_last_deletes_older_dirs() {
        let dir = tempdir().unwrap();
        let device = "aaaaaaaaaaaaaaaa";
        let store = Local::open(dir.path(), device).unwrap();
        put_file(&store, device, "snap-0/f.txt", b"x");
        thread::sleep(StdDuration::from_millis(5));
        put_file(&store, device, "snap-1/f.txt", b"x");
        thread::sleep(StdDuration::from_millis(5));
        put_file(&store, device, "snap-2/f.txt", b"x");
        thread::sleep(StdDuration::from_millis(5));

        let content = b"y";
        let sha = hex::encode(Sha256::digest(content));
        let mut begin_payload = serde_json::Map::new();
        begin_payload.insert("space".into(), json!("backups"));
        begin_payload.insert("path".into(), json!("snap-3/f.txt"));
        begin_payload.insert("size".into(), json!(content.len()));
        begin_payload.insert("sha256".into(), json!(sha));
        let begin = store
            .handle(
                Frame::from_parts("write.begin", begin_payload),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
        let upload_id = begin["upload_id"].as_str().unwrap().to_string();
        let mut chunk_payload = serde_json::Map::new();
        chunk_payload.insert("upload_id".into(), json!(upload_id));
        chunk_payload.insert("offset".into(), json!(0));
        chunk_payload.insert(
            "data".into(),
            json!(base64::engine::general_purpose::STANDARD.encode(content)),
        );
        store
            .handle(
                Frame::from_parts("write.chunk", chunk_payload),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();
        let mut commit_payload = serde_json::Map::new();
        commit_payload.insert("space".into(), json!("backups"));
        commit_payload.insert("upload_ids".into(), json!([upload_id]));
        commit_payload.insert(
            "retention".into(),
            json!({"policy": "keep_last", "keep": 2}),
        );
        store
            .handle(
                Frame::from_parts("commit", commit_payload),
                device,
                protocol::TRUST_OWNER,
                false,
            )
            .unwrap();

        let backups = dir.path().join(device).join("backups");
        let dirs = fs::read_dir(&backups)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .count();
        assert_eq!(dirs, 2);
    }
}
