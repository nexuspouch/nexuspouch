//! Space registry (Step 2): attribute-driven custom spaces.
//!
//! The four built-in spaces stay implicit (protocol constants). Custom spaces
//! are declared into `<root>/.system/spaces.json` with a profile:
//! visibility / encryption / retention / import_grant. Behavior (ACL) is
//! driven by the profile attributes, not the name.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const BUILTIN_SPACES: [&str; 4] = ["artifacts", "files", "attachments", "backups"];

/// Well-known custom space for distilled agent memory (vector search P2).
pub const MEMORY_SPACE: &str = "memory";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceProfile {
    pub name: String,
    pub visibility: String,
    pub encryption: String,
    pub retention: String,
    pub import_grant: String,
    pub created_ms: i64,
}

impl SpaceProfile {
    pub fn shared(&self) -> bool {
        self.visibility == "shared"
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SpaceFile {
    spaces: Vec<SpaceProfile>,
}

pub struct SpaceRegistry {
    path: PathBuf,
    inner: Mutex<SpaceFile>,
}

impl SpaceRegistry {
    pub fn open(root: impl AsRef<Path>) -> Self {
        let path = root.as_ref().join(".system").join("spaces.json");
        let file = if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            SpaceFile::default()
        };
        Self {
            path,
            inner: Mutex::new(file),
        }
    }

    fn save(&self, file: &SpaceFile) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let raw = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, raw).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn declare(
        &self,
        name: &str,
        visibility: &str,
        encryption: &str,
        retention: &str,
        import_grant: &str,
    ) -> Result<SpaceProfile, String> {
        validate_space_name(name)?;
        if BUILTIN_SPACES.contains(&name) {
            return Err(format!("{name} is a reserved built-in space"));
        }
        let visibility = valid_attr("visibility", visibility, &["shared", "private"])?;
        let encryption = valid_attr("encryption", encryption, &["client", "none"])?;
        let retention = valid_attr("retention", retention, &["keep_last", "gfs", "none"])?;
        let import_grant = valid_attr("import_grant", import_grant, &["allowed", "denied"])?;
        let mut g = self.inner.lock().unwrap();
        if g.spaces.iter().any(|s| s.name == name) {
            return Err(format!("space already declared: {name}"));
        }
        let profile = SpaceProfile {
            name: name.to_string(),
            visibility: visibility.to_string(),
            encryption: encryption.to_string(),
            retention: retention.to_string(),
            import_grant: import_grant.to_string(),
            created_ms: now_ms(),
        };
        g.spaces.push(profile.clone());
        self.save(&g)?;
        Ok(profile)
    }

    pub fn get(&self, name: &str) -> Option<SpaceProfile> {
        self.inner
            .lock()
            .unwrap()
            .spaces
            .iter()
            .find(|s| s.name == name)
            .cloned()
    }

    pub fn list(&self) -> Vec<SpaceProfile> {
        self.inner.lock().unwrap().spaces.clone()
    }

    pub fn is_known(&self, name: &str) -> bool {
        BUILTIN_SPACES.contains(&name) || self.get(name).is_some()
    }

    /// Seed the well-known `memory` space if missing (VECTOR_SEARCH_DESIGN P2).
    /// Default: private, client encryption, keep_last, import denied.
    pub fn ensure_memory(&self) -> Result<SpaceProfile, String> {
        if let Some(p) = self.get(MEMORY_SPACE) {
            return Ok(p);
        }
        self.declare(MEMORY_SPACE, "private", "client", "keep_last", "denied")
    }

    /// Visibility for ACL: `Some(shared?)` for known spaces, `None` unknown.
    pub fn visibility(&self, name: &str) -> Option<bool> {
        match name {
            "artifacts" | "files" => Some(true),
            "attachments" | "backups" => Some(false),
            _ => self.get(name).map(|p| p.shared()),
        }
    }

    pub fn list_json(&self) -> Value {
        let mut out: Vec<Value> = BUILTIN_SPACES
            .iter()
            .map(|n| {
                json!({
                    "name": n,
                    "builtin": true,
                    "visibility": match *n {
                        "artifacts" | "files" => "shared",
                        _ => "private",
                    },
                    "encryption": if *n == "attachments" || *n == "backups" { "client" } else { "none" },
                    "retention": if *n == "backups" { "gfs" } else { "none" },
                    "import_grant": "allowed",
                })
            })
            .collect();
        for s in self.list() {
            let mut row = json!({
                "name": s.name,
                "builtin": false,
                "visibility": s.visibility,
                "encryption": s.encryption,
                "retention": s.retention,
                "import_grant": s.import_grant,
                "created_ms": s.created_ms,
            });
            if s.name == MEMORY_SPACE {
                row.as_object_mut().unwrap().insert(
                    "well_known".into(),
                    json!(true),
                );
                row.as_object_mut().unwrap().insert(
                    "convention".into(),
                    json!("<device>/memory/<topic>/<ts>.md"),
                );
            }
            out.push(row);
        }
        json!(out)
    }
}

fn valid_attr<'a>(key: &str, value: &'a str, allowed: &[&str]) -> Result<&'a str, String> {
    if allowed.contains(&value) {
        Ok(value)
    } else {
        Err(format!("invalid {key}: {value} (expected {allowed:?})"))
    }
}

fn validate_space_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let first = chars.next();
    let ok = matches!(first, Some(c) if c.is_ascii_lowercase())
        && name.len() <= 32
        && name
            .chars()
            .skip(1)
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid space name {name:?}: ^[a-z][a-z0-9-]{{0,31}}$ required"
        ))
    }
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

    #[test]
    fn declare_and_validate() {
        let dir = tempfile::tempdir().unwrap();
        let reg = SpaceRegistry::open(dir.path());
        assert!(reg.is_known("files"));
        assert!(!reg.is_known("models"));

        let p = reg
            .declare("models", "shared", "none", "none", "allowed")
            .unwrap();
        assert_eq!(p.visibility, "shared");
        assert!(reg.is_known("models"));
        assert_eq!(reg.visibility("models"), Some(true));
        assert!(reg.declare("models", "private", "none", "none", "allowed").is_err());
        assert!(reg.declare("artifacts", "shared", "none", "none", "allowed").is_err());
        assert!(reg.declare("Bad Name", "shared", "none", "none", "allowed").is_err());
        assert!(reg.declare(".hidden", "shared", "none", "none", "allowed").is_err());
        assert!(reg.declare("Vault", "shared", "none", "none", "allowed").is_err());
        assert!(reg.declare("vault", "public", "none", "none", "allowed").is_err());

        // Persistence across reopen.
        let reg2 = SpaceRegistry::open(dir.path());
        assert!(reg2.is_known("models"));
        let listed = reg2.list_json();
        assert_eq!(listed.as_array().unwrap().len(), 5); // 4 builtin + models
    }

    #[test]
    fn ensure_memory_seeds_once() {
        let dir = tempfile::tempdir().unwrap();
        let reg = SpaceRegistry::open(dir.path());
        let p1 = reg.ensure_memory().unwrap();
        assert_eq!(p1.name, "memory");
        assert_eq!(p1.visibility, "private");
        assert_eq!(p1.encryption, "client");
        let p2 = reg.ensure_memory().unwrap();
        assert_eq!(p1.created_ms, p2.created_ms);
        let mem = reg
            .list_json()
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "memory")
            .unwrap()
            .clone();
        assert_eq!(mem["well_known"], true);
        assert!(mem["convention"].as_str().unwrap().contains("<topic>"));
    }
}
