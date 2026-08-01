use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: String,
    pub token: String,
    pub scopes: Vec<String>,
    pub label: String,
    pub created_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TokenFile {
    tokens: Vec<ApiToken>,
}

pub struct TokenStore {
    path: PathBuf,
    inner: Mutex<TokenFile>,
}

impl TokenStore {
    pub fn open(root: impl AsRef<Path>) -> Self {
        let path = root.as_ref().join(".system").join("api_tokens.json");
        let file = if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            TokenFile::default()
        };
        Self {
            path,
            inner: Mutex::new(file),
        }
    }

    fn save(&self, file: &TokenFile) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let raw = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, raw).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn list_public(&self) -> Vec<Map<String, Value>> {
        self.inner
            .lock()
            .unwrap()
            .tokens
            .iter()
            .map(|t| {
                Map::from_iter([
                    ("id".into(), json!(t.id)),
                    ("label".into(), json!(t.label)),
                    ("scopes".into(), json!(t.scopes)),
                    ("created_ms".into(), json!(t.created_ms)),
                    ("agent_id".into(), json!(t.agent_id)),
                ])
            })
            .collect()
    }

    pub fn create(
        &self,
        label: &str,
        scopes: Vec<String>,
        agent_id: Option<String>,
    ) -> Result<ApiToken, String> {
        let scopes = normalize_scopes(scopes)?;
        if let Some(ref a) = agent_id {
            if !a.starts_with("a-") || a.len() != 9 {
                return Err(format!("invalid agent_id: {a}"));
            }
        }
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let token = format!("np_{}", hex::encode(bytes));
        let entry = ApiToken {
            id: format!("t-{}", &hex::encode(&bytes[..4])),
            token: token.clone(),
            scopes,
            label: label.to_string(),
            created_ms: now_ms(),
            agent_id,
        };
        let mut g = self.inner.lock().unwrap();
        g.tokens.push(entry.clone());
        self.save(&g)?;
        Ok(entry)
    }

    pub fn revoke(&self, id: &str) -> Result<bool, String> {
        let mut g = self.inner.lock().unwrap();
        let before = g.tokens.len();
        g.tokens.retain(|t| t.id != id);
        let removed = g.tokens.len() != before;
        if removed {
            self.save(&g)?;
        }
        Ok(removed)
    }

    pub fn resolve(&self, presented: &str) -> Option<Vec<String>> {
        if presented.is_empty() {
            return None;
        }
        let g = self.inner.lock().unwrap();
        for t in &g.tokens {
            if bool::from(t.token.as_bytes().ct_eq(presented.as_bytes())) {
                return Some(t.scopes.clone());
            }
        }
        None
    }

    pub fn resolve_agent(&self, presented: &str) -> Option<String> {
        if presented.is_empty() {
            return None;
        }
        let g = self.inner.lock().unwrap();
        for t in &g.tokens {
            if bool::from(t.token.as_bytes().ct_eq(presented.as_bytes())) {
                return t.agent_id.clone();
            }
        }
        None
    }
}

fn normalize_scopes(scopes: Vec<String>) -> Result<Vec<String>, String> {
    let allowed = ["admin", "read", "write", "events"];
    let mut out = Vec::new();
    for s in scopes {
        let s = s.trim().to_lowercase();
        if !allowed.contains(&s.as_str()) {
            return Err(format!("invalid scope: {s}"));
        }
        if !out.contains(&s) {
            out.push(s);
        }
    }
    if out.is_empty() {
        return Err("scopes required".into());
    }
    Ok(out)
}

pub fn scopes_allow(have: &[String], required: &[&str]) -> bool {
    if have.iter().any(|s| s == "admin") {
        return true;
    }
    required.iter().any(|r| have.iter().any(|h| h == r))
}

pub fn is_admin_only_op(op: &str) -> bool {
    matches!(
        op,
        "recycle.empty"
            | "master.migrate"
            | "import.grant"
            | "import.reject"
            | "reprotect.run"
            | "space.declare"
    ) || op.starts_with("admin.")
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

    #[test]
    fn create_resolve_revoke() {
        let dir = tempdir().unwrap();
        let store = TokenStore::open(dir.path());
        let t = store.create("ci", vec!["read".into()], None).unwrap();
        assert!(t.token.starts_with("np_"));
        let scopes = store.resolve(&t.token).unwrap();
        assert!(scopes_allow(&scopes, &["read"]));
        assert!(!scopes_allow(&scopes, &["write"]));
        assert!(store.revoke(&t.id).unwrap());
        assert!(store.resolve(&t.token).is_none());
    }

    #[test]
    fn admin_implies_all() {
        assert!(scopes_allow(&["admin".into()], &["read", "write"]));
    }
}
