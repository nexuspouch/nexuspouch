//! Directory bindings (M6b): external directories ingested into the store.
//!
//! Registry: `<store>/.system/bindings.json`; per-binding reconcile snapshot
//! in `<store>/.system/binding_index/<id>.json`. Ingestion goes through the
//! loopback store frame path so versions / manifest / index / handoff hooks
//! fire automatically. v1 ingests by copy; reflink / hardlink modes are
//! recorded for future use (M6 design §3).

use super::{io_err, Local, OpError, MAX_CHUNK};
use crate::protocol::{self, Frame};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use notify::Watcher as _;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Binding {
    pub id: String,
    pub label: String,
    pub external: String,
    pub space: String,
    pub folder: String,
    pub mode: String,
    pub ignore: Vec<String>,
    pub created_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct BindingReport {
    pub binding_id: String,
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

impl BindingReport {
    pub fn to_json(&self) -> Value {
        json!({
            "binding_id": self.binding_id,
            "added": self.added,
            "updated": self.updated,
            "deleted": self.deleted,
            "skipped": self.skipped,
            "errors": self.errors,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct BindingFile {
    bindings: Vec<Binding>,
}

pub struct BindingsRegistry {
    path: PathBuf,
    inner: Mutex<BindingFile>,
}

impl BindingsRegistry {
    pub fn open(root: impl AsRef<Path>) -> Self {
        let path = root.as_ref().join(".system").join("bindings.json");
        let file = if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            BindingFile::default()
        };
        Self {
            path,
            inner: Mutex::new(file),
        }
    }

    fn save(&self, file: &BindingFile) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let raw = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, raw).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn list(&self) -> Vec<Binding> {
        self.inner.lock().unwrap().bindings.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().bindings.is_empty()
    }

    /// Idempotent-ish add: skips if an identical external+folder exists.
    pub fn add(
        &self,
        label: &str,
        external: &str,
        space: &str,
        folder: &str,
        mode: &str,
        ignore: Vec<String>,
    ) -> Result<Binding, String> {
        if space != "files" && space != "artifacts" {
            return Err("bindings support files/artifacts spaces".into());
        }
        let norm_folder =
            protocol::normalize_path(if folder.is_empty() { "bind" } else { folder })
                .map_err(|e| format!("bad folder: {e}"))?;
        let mode = match mode {
            "copy" | "hardlink-immutable" => mode,
            _ => "auto",
        };
        let mut g = self.inner.lock().unwrap();
        if g.bindings
            .iter()
            .any(|b| b.external == external && b.folder == norm_folder)
        {
            return Err("binding already exists".into());
        }
        let mut bytes = [0u8; 4];
        rand::thread_rng().fill_bytes(&mut bytes);
        let binding = Binding {
            id: format!("b-{}", hex::encode(bytes)),
            label: label.to_string(),
            external: external.to_string(),
            space: space.to_string(),
            folder: norm_folder,
            mode: mode.to_string(),
            ignore,
            created_ms: now_ms(),
        };
        g.bindings.push(binding.clone());
        self.save(&g)?;
        Ok(binding)
    }
}

/// Sync one binding: full scan + sha256/size/mtime diff, ingest via loopback
/// frames (hooks fire), delete via recycle.
pub fn sync_binding(local: &Local, b: &Binding) -> BindingReport {
    let mut report = BindingReport {
        binding_id: b.id.clone(),
        ..Default::default()
    };
    let ext = PathBuf::from(&b.external);
    if !ext.is_dir() {
        report
            .errors
            .push(format!("external dir not found: {}", b.external));
        return report;
    }
    let index_path = local
        .root
        .join(".system")
        .join("binding_index")
        .join(format!("{}.json", b.id));
    let mut index: Map<String, Value> = fs::read_to_string(&index_path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let mut seen = HashSet::new();

    walk(&ext, &ext, &b.ignore, &mut |rel, full| {
        seen.insert(rel.to_string());
        let Ok(meta) = fs::metadata(full) else {
            report.errors.push(format!("{rel}: stat failed"));
            return;
        };
        let size = meta.len() as i64;
        let mtime = mtime_ms(&meta);
        let same_meta = index
            .get(rel)
            .map(|v| v.get("size") == Some(&json!(size)) && v.get("mtime") == Some(&json!(mtime)))
            .unwrap_or(false);
        if same_meta {
            report.skipped += 1;
            return;
        }
        let (sha, _) = super::browse::file_sha(full);
        let same_content = index
            .get(rel)
            .map(|v| v.get("sha") == Some(&json!(sha)) && v.get("size") == Some(&json!(size)))
            .unwrap_or(false);
        if same_content {
            report.skipped += 1;
            return;
        }
        let dest_rel = if b.folder.is_empty() {
            rel.to_string()
        } else {
            format!("{}/{}", b.folder, rel)
        };
        match ingest_file(local, &b.space, &local.device_id, &dest_rel, full, size, &sha) {
            Ok(()) => {
                let existed = index.contains_key(rel);
                index.insert(
                    rel.to_string(),
                    json!({"sha": sha, "size": size, "mtime": mtime}),
                );
                if existed {
                    report.updated += 1;
                } else {
                    report.added += 1;
                }
            }
            Err(e) => report.errors.push(format!("{rel}: {e}")),
        }
    });

    let rels: Vec<String> = index.keys().cloned().collect();
    for rel in rels {
        if seen.contains(&rel) {
            continue;
        }
        let dest_rel = if b.folder.is_empty() {
            rel.clone()
        } else {
            format!("{}/{}", b.folder, rel)
        };
        let mut p = Map::new();
        p.insert("space".into(), json!(b.space));
        p.insert("path".into(), json!(dest_rel));
        match local.handle(
            Frame::from_parts("delete", p),
            &local.device_id,
            protocol::TRUST_OWNER,
            true,
        ) {
            Ok(_) => {
                index.remove(&rel);
                report.deleted += 1;
            }
            Err(e) if e.code == "not_found" => {
                index.remove(&rel);
            }
            Err(e) => report.errors.push(format!("delete {rel}: {e}")),
        }
    }

    if let Some(parent) = index_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(
        &index_path,
        serde_json::to_vec_pretty(&index).unwrap_or_default(),
    );
    report
}

pub fn sync_all(local: &Local) -> Vec<BindingReport> {
    let registry = BindingsRegistry::open(&local.root);
    registry
        .list()
        .iter()
        .map(|b| sync_binding(local, b))
        .collect()
}

/// Periodic sync daemon task (blocking work offloaded).
pub fn start_periodic(local: std::sync::Arc<Local>, every: Duration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let l = std::sync::Arc::clone(&local);
            tokio::task::spawn_blocking(move || {
                let _ = sync_all(&l);
            });
        }
    });
}

/// Event-driven watcher (M6 §4): watches each binding's external directory
/// and triggers a debounced reconciliation on filesystem events. The periodic
/// sync remains as a safety net for missed events (inotify overflow etc.).
pub fn start_watcher(
    local: std::sync::Arc<Local>,
    bindings: Vec<Binding>,
) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut watcher = match notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    let _ = tx.send(());
                }
            },
        ) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("bindings watcher unavailable: {e}");
                return;
            }
        };
        for b in &bindings {
            if let Err(e) = watcher.watch(
                std::path::Path::new(&b.external),
                notify::RecursiveMode::Recursive,
            ) {
                tracing::warn!("watch {}: {e}", b.external);
            }
        }
        while rx.recv().is_ok() {
            // Debounce: coalesce bursts (editor atomic renames etc.).
            std::thread::sleep(Duration::from_millis(500));
            while rx.try_recv().is_ok() {}
            let l = std::sync::Arc::clone(&local);
            std::thread::spawn(move || {
                let _ = sync_all(&l);
            });
        }
    });
    Ok(())
}

fn walk(
    root: &Path,
    dir: &Path,
    ignore: &[String],
    f: &mut impl FnMut(&str, &Path),
) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || matches_ignore(&name, ignore) {
            continue;
        }
        let path = ent.path();
        if path.is_symlink() {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        if path.is_dir() {
            walk(root, &path, ignore, f);
        } else {
            f(&rel, &path);
        }
    }
}

fn matches_ignore(name: &str, ignore: &[String]) -> bool {
    ignore.iter().any(|pat| {
        pat == name
            || (pat.starts_with('*') && name.ends_with(&pat[1..]))
            || (pat.ends_with('*') && name.starts_with(&pat[..pat.len() - 1]))
    })
}

fn ingest_file(
    local: &Local,
    space: &str,
    device: &str,
    dest_rel: &str,
    src: &Path,
    size: i64,
    sha: &str,
) -> Result<(), OpError> {
    let mut begin = Map::new();
    begin.insert("space".into(), json!(space));
    begin.insert("path".into(), json!(dest_rel));
    begin.insert("size".into(), json!(size));
    begin.insert("sha256".into(), json!(sha));
    let out = local.handle(
        Frame::from_parts("write.begin", begin),
        device,
        protocol::TRUST_OWNER,
        true,
    )?;
    let upload_id = out
        .get("upload_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| OpError::new("internal", "missing upload_id"))?
        .to_string();

    let mut f = fs::File::open(src).map_err(io_err)?;
    let mut buf = vec![0u8; MAX_CHUNK];
    let mut offset: i64 = 0;
    loop {
        let n = f.read(&mut buf).map_err(io_err)?;
        if n == 0 {
            break;
        }
        let mut chunk = Map::new();
        chunk.insert("upload_id".into(), json!(upload_id));
        chunk.insert("offset".into(), json!(offset));
        chunk.insert("data".into(), json!(STANDARD.encode(&buf[..n])));
        local.handle(
            Frame::from_parts("write.chunk", chunk),
            device,
            protocol::TRUST_OWNER,
            true,
        )?;
        offset += n as i64;
    }
    let mut commit = Map::new();
    commit.insert("space".into(), json!(space));
    commit.insert("upload_ids".into(), json!([upload_id]));
    local.handle(
        Frame::from_parts("commit", commit),
        device,
        protocol::TRUST_OWNER,
        true,
    )?;
    Ok(())
}

fn mtime_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
    use tempfile::tempdir;

    const DEV: &str = "aaaaaaaaaaaaaaaa";

    fn make_local(dir: &Path) -> Local {
        Local::open(dir, DEV).unwrap()
    }

    fn store_file(dir: &Path, rel: &str) -> PathBuf {
        dir.join(DEV).join("files").join(rel)
    }

    #[test]
    fn binding_ingest_add_modify_delete() {
        let dir = tempdir().unwrap();
        let local = make_local(dir.path());
        let ext = tempdir().unwrap();

        let registry = BindingsRegistry::open(dir.path());
        let b = registry
            .add(
                "inbox",
                ext.path().to_str().unwrap(),
                "files",
                "inbox",
                "auto",
                vec!["*.tmp".into()],
            )
            .unwrap();

        // Add + ignore.
        fs::write(ext.path().join("a.txt"), b"one").unwrap();
        fs::write(ext.path().join("x.tmp"), b"skip").unwrap();
        let r1 = sync_binding(&local, &b);
        assert_eq!(r1.added, 1);
        assert!(r1.errors.is_empty());
        assert_eq!(
            fs::read_to_string(store_file(dir.path(), "inbox/a.txt")).unwrap(),
            "one"
        );
        assert!(!store_file(dir.path(), "inbox/x.tmp").exists());

        // Modify -> updated.
        fs::write(ext.path().join("a.txt"), b"two").unwrap();
        let r2 = sync_binding(&local, &b);
        assert_eq!(r2.updated, 1);
        assert_eq!(
            fs::read_to_string(store_file(dir.path(), "inbox/a.txt")).unwrap(),
            "two"
        );

        // Unchanged -> skipped.
        let r3 = sync_binding(&local, &b);
        assert_eq!(r3.skipped, 1);

        // Delete -> recycle.
        fs::remove_file(ext.path().join("a.txt")).unwrap();
        let r4 = sync_binding(&local, &b);
        assert_eq!(r4.deleted, 1);
        assert!(!store_file(dir.path(), "inbox/a.txt").exists());
        assert!(dir.path().join(".recycle").is_dir());
    }

    #[test]
    fn binding_missing_external_reports_error() {
        let dir = tempdir().unwrap();
        let local = make_local(dir.path());
        let registry = BindingsRegistry::open(dir.path());
        let b = registry
            .add(
                "gone",
                "/nonexistent/xyz",
                "files",
                "gone",
                "auto",
                vec![],
            )
            .unwrap();
        let r = sync_binding(&local, &b);
        assert!(!r.errors.is_empty());
        assert!(r.errors[0].contains("not found"));
    }
}
