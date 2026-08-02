//! Directory bindings (M6b): external directories ingested into the store.
//!
//! Registry: `<store>/.system/bindings.json`; per-binding reconcile snapshot
//! in `<store>/.system/binding_index/<id>.json`. Ingestion goes through the
//! loopback store frame path so versions / manifest / index / handoff hooks
//! fire automatically. v1 ingests by copy; reflink / hardlink modes are
//! recorded for future use (M6 design §3).

use super::{io_err, Local, OpError};
use crate::events::StoreEvent;
use crate::protocol::{self, Frame};
use notify::Watcher as _;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs;
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

const DEFAULT_MAX_FILES: usize = 500_000;
const DEFAULT_STABLE_MS: u64 = 300;
const THROTTLE_EVERY: usize = 1_000;
const META_KEY: &str = "__meta__";

fn binding_max_files() -> usize {
    std::env::var("NEXUSPOUCH_BINDING_MAX_FILES")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_FILES)
}

fn binding_stable_ms() -> u64 {
    if let Ok(s) = std::env::var("NEXUSPOUCH_BINDING_STABLE_MS") {
        if let Ok(n) = s.parse() {
            return n;
        }
    }
    // Unit tests skip the delay by default (set env to exercise the guard).
    if cfg!(test) {
        0
    } else {
        DEFAULT_STABLE_MS
    }
}

fn ignore_fingerprint(ignore: &[String]) -> String {
    let mut parts = ignore.to_vec();
    parts.sort();
    parts.join("\n")
}

#[derive(Debug, Clone, Default)]
pub struct BindingReport {
    pub binding_id: String,
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
    pub renamed: usize,
    pub skipped: usize,
    pub scanned: usize,
    pub truncated: bool,
    pub errors: Vec<String>,
}

impl BindingReport {
    pub fn to_json(&self) -> Value {
        json!({
            "binding_id": self.binding_id,
            "added": self.added,
            "updated": self.updated,
            "deleted": self.deleted,
            "renamed": self.renamed,
            "skipped": self.skipped,
            "scanned": self.scanned,
            "truncated": self.truncated,
            "errors": self.errors,
        })
    }
}

#[derive(Clone)]
struct ScanEntry {
    rel: String,
    full: PathBuf,
    size: i64,
    mtime: i64,
    sha: String,
    dev: u64,
    ino: u64,
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
        if space != "files" && space != "artifacts" && space != super::spaces::SESSIONS_SPACE {
            return Err("bindings support files/artifacts/sessions spaces".into());
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

    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let mut g = self.inner.lock().unwrap();
        let before = g.bindings.len();
        g.bindings.retain(|b| b.id != id);
        if g.bindings.len() == before {
            return Ok(false);
        }
        self.save(&g)?;
        Ok(true)
    }
}

/// Expand a leading `~/` using `$HOME` (admin UI convenience).
pub fn expand_external(path: &str) -> String {
    let p = path.trim();
    if p == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| p.to_string());
    }
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!(
                "{home}{}{rest}",
                std::path::MAIN_SEPARATOR
            );
        }
    }
    p.to_string()
}

fn binding_dest_rel(b: &Binding, rel: &str) -> String {
    if b.folder.is_empty() {
        rel.to_string()
    } else {
        format!("{}/{}", b.folder, rel)
    }
}

fn file_dev_ino(meta: &fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        (0, 0)
    }
}

fn index_entry_json(e: &ScanEntry) -> Value {
    json!({
        "sha": e.sha,
        "size": e.size,
        "mtime": e.mtime,
        "dev": e.dev,
        "ino": e.ino,
    })
}

fn rename_bound_path(
    local: &Local,
    space: &str,
    from_rel: &str,
    to_rel: &str,
) -> Result<(), String> {
    let device = &local.device_id;
    let from = super::browse::resolve_path(local, space, device, from_rel)
        .map_err(|e| e.to_string())?;
    let to =
        super::browse::resolve_path(local, space, device, to_rel).map_err(|e| e.to_string())?;
    if !from.is_file() {
        return Err(format!("rename source missing: {from_rel}"));
    }
    if to.exists() {
        return Err(format!("rename dest exists: {to_rel}"));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    super::fsutil::rename_replace(&from, &to).map_err(|e| e.to_string())?;

    let v_from = local
        .root
        .join(".versions")
        .join(device)
        .join(space)
        .join(from_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    let v_to = local
        .root
        .join(".versions")
        .join(device)
        .join(space)
        .join(to_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    if v_from.exists() {
        if let Some(parent) = v_to.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = super::fsutil::rename_replace(&v_from, &v_to);
    }

    let from_uri = format!("store://{space}/{device}/{from_rel}");
    let to_uri = format!("store://{space}/{device}/{to_rel}");
    local.remove_index(&from_uri);
    // Rebuild index entry from the renamed live file.
    if let Ok(meta) = fs::metadata(&to) {
        let (sha, size) = super::browse::file_sha(&to);
        let body = if size as usize <= 1024 * 1024 {
            fs::read_to_string(&to).unwrap_or_default()
        } else {
            String::new()
        };
        local.index_file(&super::index::IndexEntry {
            uri: to_uri.clone(),
            space: space.into(),
            device: device.into(),
            path: to_rel.into(),
            sha256: sha,
            size,
            mtime: mtime_ms(&meta),
            task: super::manifest::task_root(to_rel).unwrap_or_default(),
            state: "committed".into(),
            summary: None,
            body,
        });
    }

    let mut detail = Map::new();
    detail.insert("from_uri".into(), json!(from_uri));
    detail.insert("to_uri".into(), json!(to_uri));
    local.emit_event(
        StoreEvent::new("artifact.renamed", device)
            .with_path(space, to_rel)
            .with_uri(to_uri)
            .with_detail(detail),
    );
    Ok(())
}

/// Sync one binding: scan → rename detect → ingest/update → delete.
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
    let ignore_fp = ignore_fingerprint(&b.ignore);
    let prev_ignore = index
        .get(META_KEY)
        .and_then(|m| m.get("ignore"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let force_full = prev_ignore != ignore_fp;
    let max_files = binding_max_files();
    let stable_ms = binding_stable_ms();
    let mut truncated = false;
    let mut scanned = 0usize;
    let mut entries: Vec<ScanEntry> = Vec::new();

    walk(
        &ext,
        &ext,
        &b.ignore,
        max_files,
        &mut scanned,
        &mut truncated,
        &mut |rel, full| {
            let Ok(meta) = fs::metadata(full) else {
                report.errors.push(format!("{rel}: stat failed"));
                return;
            };
            let size = meta.len() as i64;
            let mtime = mtime_ms(&meta);
            let (dev, ino) = file_dev_ino(&meta);
            let same_meta = !force_full
                && index
                    .get(rel)
                    .map(|v| {
                        v.get("size") == Some(&json!(size)) && v.get("mtime") == Some(&json!(mtime))
                    })
                    .unwrap_or(false);
            if same_meta {
                report.skipped += 1;
                // Keep identity fresh without hashing.
                if let Some(prev) = index.get(rel).cloned() {
                    let mut obj = prev.as_object().cloned().unwrap_or_default();
                    obj.insert("dev".into(), json!(dev));
                    obj.insert("ino".into(), json!(ino));
                    index.insert(rel.to_string(), Value::Object(obj));
                }
                entries.push(ScanEntry {
                    rel: rel.to_string(),
                    full: full.to_path_buf(),
                    size,
                    mtime,
                    sha: index
                        .get(rel)
                        .and_then(|v| v.get("sha"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    dev,
                    ino,
                });
                return;
            }
            if wait_stable(full, size, mtime, stable_ms).is_none() {
                report.skipped += 1;
                return;
            }
            let (sha, _) = super::browse::file_sha(full);
            entries.push(ScanEntry {
                rel: rel.to_string(),
                full: full.to_path_buf(),
                size,
                mtime,
                sha,
                dev,
                ino,
            });
        },
    );
    report.scanned = scanned;
    report.truncated = truncated;
    if truncated {
        report
            .errors
            .push(format!("file_limit_exceeded: max_files={max_files}"));
    }

    let seen: HashSet<String> = entries.iter().map(|e| e.rel.clone()).collect();
    let vanished: Vec<String> = index
        .keys()
        .filter(|k| k.as_str() != META_KEY && !seen.contains(k.as_str()))
        .cloned()
        .collect();
    let mut appeared: Vec<&ScanEntry> = entries
        .iter()
        .filter(|e| !index.contains_key(&e.rel))
        .collect();
    let mut renamed_from: HashSet<String> = HashSet::new();
    let mut renamed_to: HashSet<String> = HashSet::new();

    for from_rel in &vanished {
        let prev = match index.get(from_rel) {
            Some(v) => v,
            None => continue,
        };
        let prev_sha = prev.get("sha").and_then(|v| v.as_str()).unwrap_or("");
        let prev_dev = prev.get("dev").and_then(|v| v.as_u64()).unwrap_or(0);
        let prev_ino = prev.get("ino").and_then(|v| v.as_u64()).unwrap_or(0);
        let match_idx = appeared.iter().position(|a| {
            if renamed_to.contains(&a.rel) {
                return false;
            }
            if prev_dev != 0 && prev_ino != 0 && a.dev == prev_dev && a.ino == prev_ino {
                return true;
            }
            !prev_sha.is_empty() && a.sha == prev_sha
        });
        let Some(i) = match_idx else {
            continue;
        };
        let to = appeared.remove(i);
        let from_dest = binding_dest_rel(b, from_rel);
        let to_dest = binding_dest_rel(b, &to.rel);
        match rename_bound_path(local, &b.space, &from_dest, &to_dest) {
            Ok(()) => {
                index.remove(from_rel);
                index.insert(to.rel.clone(), index_entry_json(to));
                renamed_from.insert(from_rel.clone());
                renamed_to.insert(to.rel.clone());
                report.renamed += 1;
            }
            Err(e) => report.errors.push(format!("rename {from_rel}->{}: {e}", to.rel)),
        }
    }

    for e in &entries {
        if renamed_to.contains(&e.rel) {
            continue;
        }
        if e.sha.is_empty() {
            // Unchanged meta-skip path already counted as skipped.
            continue;
        }
        let same_content = index
            .get(&e.rel)
            .map(|v| {
                v.get("sha") == Some(&json!(e.sha)) && v.get("size") == Some(&json!(e.size))
            })
            .unwrap_or(false);
        if same_content {
            index.insert(e.rel.clone(), index_entry_json(e));
            if !force_full {
                // already counted skipped when meta matched; if content-only refresh:
            }
            continue;
        }
        let dest_rel = binding_dest_rel(b, &e.rel);
        match ingest_file(
            local,
            &b.space,
            &local.device_id,
            &dest_rel,
            &e.full,
            e.size,
            &e.sha,
            &b.mode,
        ) {
            Ok(()) => {
                let existed = index.contains_key(&e.rel);
                index.insert(e.rel.clone(), index_entry_json(e));
                if existed {
                    report.updated += 1;
                } else {
                    report.added += 1;
                }
            }
            Err(err) => report.errors.push(format!("{}: {err}", e.rel)),
        }
    }

    index.insert(META_KEY.into(), json!({"ignore": ignore_fp}));

    let rels: Vec<String> = index.keys().cloned().collect();
    for rel in rels {
        if rel == META_KEY || seen.contains(&rel) || renamed_from.contains(&rel) {
            continue;
        }
        let dest_rel = binding_dest_rel(b, &rel);
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
    max_files: usize,
    scanned: &mut usize,
    truncated: &mut bool,
    f: &mut impl FnMut(&str, &Path),
) {
    if *truncated {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for ent in rd.flatten() {
        if *truncated {
            return;
        }
        let name = ent.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || matches_ignore(&name, ignore) {
            continue;
        }
        let path = ent.path();
        // Skip symlinks and Windows junctions / reparse points.
        if super::fsutil::is_symlink_or_reparse(&path) {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        if path.is_dir() {
            walk(root, &path, ignore, max_files, scanned, truncated, f);
        } else {
            if *scanned >= max_files {
                *truncated = true;
                return;
            }
            *scanned += 1;
            if *scanned % THROTTLE_EVERY == 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
            f(&rel, &path);
        }
    }
}

/// Re-stat after a short delay; `None` if the file vanished or is still changing.
fn wait_stable(path: &Path, size: i64, mtime: i64, stable_ms: u64) -> Option<(i64, i64)> {
    if stable_ms == 0 {
        return Some((size, mtime));
    }
    std::thread::sleep(Duration::from_millis(stable_ms));
    let meta = fs::metadata(path).ok()?;
    let size2 = meta.len() as i64;
    let mtime2 = mtime_ms(&meta);
    if size2 == size && mtime2 == mtime {
        Some((size2, mtime2))
    } else {
        None
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
    mode: &str,
) -> Result<(), OpError> {
    let dest = local
        .root
        .join(device)
        .join(space)
        .join(dest_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(io_err)?;
    }
    // Archive the previous version (reuses versions/keep_last machinery).
    if dest.exists() {
        super::versions::archive_old(local, space, device, dest_rel, sha, None)?;
    }
    // Materialize per mode: reflink (auto) > hardlink (immutable) > copy.
    match mode {
        "hardlink-immutable" => {
            if fs::hard_link(src, &dest).is_err() {
                fs::copy(src, &dest).map_err(io_err)?;
            }
        }
        "auto" => {
            if !reflink_copy(src, &dest) {
                fs::copy(src, &dest).map_err(io_err)?;
            }
        }
        _ => {
            fs::copy(src, &dest).map_err(io_err)?;
        }
    }
    // Content verification (hardlink shares the inode; copy/reflink reads dest).
    let (sum, _) = super::browse::file_sha(&dest);
    if sum != sha {
        let _ = fs::remove_file(&dest);
        return Err(OpError::new("hash_mismatch", "binding ingest hash mismatch"));
    }
    // Hooks (lineage / handoff / index) via the shared finish_committed path.
    let task = super::manifest::task_root(dest_rel).unwrap_or_default();
    let entry = json!({
        "path": dest_rel,
        "size": size,
        "sha256": sha,
        "version": 1,
    });
    let commits = vec![(space.to_string(), device.to_string(), task, entry.clone())];
    super::write::finish_committed(local, &commits, false, None, None);
    local.emit_event(
        StoreEvent::new("commit", device)
            .with_uri(format!("store://{space}/{device}/{dest_rel}"))
            .with_path(space, dest_rel)
            .with_detail(Map::from_iter([
                ("size".into(), json!(size)),
                ("sha256".into(), json!(sha)),
                ("ingest".into(), json!(true)),
                ("mode".into(), json!(mode)),
            ])),
    );
    Ok(())
}

/// CoW clone when the FS supports it; otherwise false → caller uses `fs::copy`.
/// macOS: `clonefile(2)`; Linux: `ioctl(FICLONE)`; Windows: unsupported.
#[cfg(target_os = "macos")]
fn reflink_copy(src: &Path, dest: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Ok(s) = CString::new(src.as_os_str().as_bytes()) else {
        return false;
    };
    let Ok(d) = CString::new(dest.as_os_str().as_bytes()) else {
        return false;
    };
    // Prefer native clonefile; fall back to `cp -c` if libc binding fails at runtime.
    let rc = unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) };
    if rc == 0 {
        return true;
    }
    std::process::Command::new("cp")
        .arg("-c")
        .arg(src)
        .arg(dest)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn reflink_copy(src: &Path, dest: &Path) -> bool {
    use std::os::unix::io::AsRawFd;
    let Ok(src_f) = fs::File::open(src) else {
        return false;
    };
    let Ok(dest_f) = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
    else {
        return false;
    };
    // FICLONE = _IOW(0x94, 9, int) — clone whole file when FS supports it.
    const FICLONE: libc::c_ulong = 0x4004_9409;
    let rc = unsafe { libc::ioctl(dest_f.as_raw_fd(), FICLONE, src_f.as_raw_fd()) };
    if rc == 0 {
        return true;
    }
    let _ = fs::remove_file(dest);
    // Fallback: GNU cp --reflink=auto.
    std::process::Command::new("cp")
        .arg("--reflink=auto")
        .arg(src)
        .arg(dest)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn reflink_copy(src: &Path, dest: &Path) -> bool {
    std::process::Command::new("cp")
        .arg("--reflink=auto")
        .arg(src)
        .arg(dest)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn reflink_copy(_src: &Path, _dest: &Path) -> bool {
    false
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
    fn expand_external_home() {
        std::env::set_var("HOME", "/tmp/home-test");
        let got = expand_external("~/foo");
        assert!(got.contains("home-test"));
        assert!(got.ends_with("foo"));
        assert_eq!(expand_external("/abs/path"), "/abs/path");
    }

    #[test]
    fn binding_remove() {
        let dir = tempfile::tempdir().unwrap();
        let registry = BindingsRegistry::open(dir.path());
        let b = registry
            .add("t", dir.path().to_str().unwrap(), "sessions", "claude-code", "auto", vec![])
            .unwrap();
        assert!(registry.remove(&b.id).unwrap());
        assert!(!registry.remove(&b.id).unwrap());
        assert!(registry.list().is_empty());
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
    fn binding_rename_preserves_store_path_history() {
        std::env::set_var("NEXUSPOUCH_BINDING_STABLE_MS", "0");
        let dir = tempdir().unwrap();
        let local = make_local(dir.path());
        let ext = tempdir().unwrap();
        let registry = BindingsRegistry::open(dir.path());
        let b = registry
            .add(
                "mv",
                ext.path().to_str().unwrap(),
                "files",
                "mv",
                "copy",
                vec![],
            )
            .unwrap();
        fs::write(ext.path().join("a.txt"), b"hello-rename").unwrap();
        let r1 = sync_binding(&local, &b);
        assert_eq!(r1.added, 1);
        assert!(store_file(dir.path(), "mv/a.txt").exists());

        fs::rename(ext.path().join("a.txt"), ext.path().join("b.txt")).unwrap();
        let r2 = sync_binding(&local, &b);
        assert_eq!(r2.renamed, 1, "errors={:?}", r2.errors);
        assert_eq!(r2.added, 0);
        assert_eq!(r2.deleted, 0);
        assert!(!store_file(dir.path(), "mv/a.txt").exists());
        assert_eq!(
            fs::read_to_string(store_file(dir.path(), "mv/b.txt")).unwrap(),
            "hello-rename"
        );
    }

    #[test]
    fn binding_file_limit_truncates() {
        std::env::set_var("NEXUSPOUCH_BINDING_MAX_FILES", "2");
        let dir = tempdir().unwrap();
        let local = make_local(dir.path());
        let ext = tempdir().unwrap();
        let registry = BindingsRegistry::open(dir.path());
        let b = registry
            .add(
                "cap",
                ext.path().to_str().unwrap(),
                "files",
                "cap",
                "copy",
                vec![],
            )
            .unwrap();
        fs::write(ext.path().join("a.txt"), b"a").unwrap();
        fs::write(ext.path().join("b.txt"), b"b").unwrap();
        fs::write(ext.path().join("c.txt"), b"c").unwrap();
        let r = sync_binding(&local, &b);
        assert!(r.truncated);
        assert_eq!(r.scanned, 2);
        assert!(r.errors.iter().any(|e| e.contains("file_limit_exceeded")));
        std::env::remove_var("NEXUSPOUCH_BINDING_MAX_FILES");
    }

    #[test]
    fn binding_ignore_change_forces_reconcile() {
        let dir = tempdir().unwrap();
        let local = make_local(dir.path());
        let ext = tempdir().unwrap();
        let registry = BindingsRegistry::open(dir.path());
        let mut b = registry
            .add(
                "ig",
                ext.path().to_str().unwrap(),
                "files",
                "ig",
                "copy",
                vec!["*.tmp".into()],
            )
            .unwrap();
        fs::write(ext.path().join("keep.txt"), b"k").unwrap();
        fs::write(ext.path().join("x.tmp"), b"tmp").unwrap();
        let r1 = sync_binding(&local, &b);
        assert_eq!(r1.added, 1);
        assert!(!store_file(dir.path(), "ig/x.tmp").exists());

        // Drop ignore → previously skipped file is ingested on full reconcile.
        b.ignore.clear();
        let r2 = sync_binding(&local, &b);
        assert_eq!(r2.added, 1);
        assert!(store_file(dir.path(), "ig/x.tmp").exists());
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

    #[test]
    fn binding_hardlink_immutable_no_duplicate_storage() {
        let dir = tempdir().unwrap();
        let local = make_local(dir.path());
        let ext = tempdir().unwrap();
        let registry = BindingsRegistry::open(dir.path());
        let b = registry
            .add(
                "hl",
                ext.path().to_str().unwrap(),
                "files",
                "hl",
                "hardlink-immutable",
                vec![],
            )
            .unwrap();
        fs::write(ext.path().join("big.bin"), vec![7u8; 64 * 1024]).unwrap();

        let r = sync_binding(&local, &b);
        assert_eq!(r.added, 1);
        assert!(r.errors.is_empty());
        assert!(store_file(dir.path(), "hl/big.bin").is_file());

        // Same inode => no duplicate storage (Unix). Windows hardlink shares
        // the file id but MetadataExt is Unix-only; content check covers both.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let ext_ino = fs::metadata(ext.path().join("big.bin")).unwrap().ino();
            let store_ino = fs::metadata(store_file(dir.path(), "hl/big.bin"))
                .unwrap()
                .ino();
            assert_eq!(ext_ino, store_ino);
        }
        let (sha_ext, _) = crate::store::browse::file_sha(&ext.path().join("big.bin"));
        let (sha_store, _) = crate::store::browse::file_sha(&store_file(dir.path(), "hl/big.bin"));
        assert_eq!(sha_ext, sha_store);

        // Hooks fired: version store has the ingested file.
        let versions =
            crate::store::versions::all_versions(&local, "files", DEV, "hl/big.bin").unwrap();
        assert_eq!(versions.len(), 1);
    }
}
