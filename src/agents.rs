//! Agent registry (M4): HTTP/MCP-layer identities with scopes, quotas and
//! rate limits. Noise peer frames stay device-authenticated; agents are a
//! bearer-token (or `x-agent-id` header) concept only.

use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub created_ms: i64,
    pub scopes: Vec<String>,
    pub quota: Quota,
    pub rate: Rate,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quota {
    pub max_bytes: u64,
    pub bytes_used: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rate {
    pub tokens_per_sec: f64,
    pub burst: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AgentFile {
    agents: Vec<Agent>,
}

#[derive(Default)]
struct Bucket {
    tokens: f64,
    last_ms: i64,
}

pub struct AgentRegistry {
    path: PathBuf,
    inner: Mutex<AgentFile>,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl AgentRegistry {
    pub fn open(root: impl AsRef<Path>) -> Self {
        let path = root.as_ref().join(".system").join("agents.json");
        let file = if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            AgentFile::default()
        };
        Self {
            path,
            inner: Mutex::new(file),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    fn save(&self, file: &AgentFile) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let raw = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, raw).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn create(
        &self,
        name: &str,
        scopes: Vec<String>,
        max_bytes: u64,
    ) -> Result<Agent, String> {
        let scopes = normalize_agent_scopes(scopes)?;
        let mut bytes = [0u8; 4];
        rand::thread_rng().fill_bytes(&mut bytes);
        let agent = Agent {
            id: format!("a-{}", hex::encode(bytes)),
            name: name.to_string(),
            created_ms: now_ms(),
            scopes,
            quota: Quota {
                max_bytes,
                bytes_used: 0,
            },
            rate: Rate {
                tokens_per_sec: 10.0,
                burst: 20,
            },
            status: "active".into(),
        };
        let mut g = self.inner.lock().unwrap();
        g.agents.push(agent.clone());
        self.save(&g)?;
        Ok(agent)
    }

    pub fn get(&self, id: &str) -> Option<Agent> {
        self.inner
            .lock()
            .unwrap()
            .agents
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }

    pub fn list_public(&self) -> Vec<Map<String, Value>> {
        self.inner
            .lock()
            .unwrap()
            .agents
            .iter()
            .map(|a| {
                Map::from_iter([
                    ("id".into(), json!(a.id)),
                    ("name".into(), json!(a.name)),
                    ("scopes".into(), json!(a.scopes)),
                    ("status".into(), json!(a.status)),
                    ("created_ms".into(), json!(a.created_ms)),
                    (
                        "quota".into(),
                        json!({"max_bytes": a.quota.max_bytes, "bytes_used": a.quota.bytes_used}),
                    ),
                ])
            })
            .collect()
    }

    pub fn revoke(&self, id: &str) -> Result<bool, String> {
        let mut g = self.inner.lock().unwrap();
        let before = g.agents.len();
        g.agents.retain(|a| a.id != id);
        let removed = g.agents.len() != before;
        if removed {
            self.save(&g)?;
        }
        Ok(removed)
    }

    pub fn reset_quota(&self, id: &str) -> Result<bool, String> {
        let mut g = self.inner.lock().unwrap();
        match g.agents.iter_mut().find(|a| a.id == id) {
            Some(a) => {
                a.quota.bytes_used = 0;
                self.save(&g)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub fn record_usage(&self, id: &str, bytes: u64) {
        let mut g = self.inner.lock().unwrap();
        if let Some(a) = g.agents.iter_mut().find(|a| a.id == id) {
            a.quota.bytes_used = a.quota.bytes_used.saturating_add(bytes);
            let _ = self.save(&g);
        }
    }

    /// Scope / quota / rate gate. Returns `Ok(())` or an Err whose message
    /// starts with the store-style code: `acl_denied`, `quota_exceeded`,
    /// `rate_limited`.
    pub fn check(
        &self,
        id: &str,
        op: &str,
        space: &str,
        write_size: u64,
    ) -> Result<(), String> {
        let agent = self
            .get(id)
            .ok_or_else(|| "acl_denied: unknown agent".to_string())?;
        if agent.status != "active" {
            return Err(format!("acl_denied: agent {id} is {}", agent.status));
        }
        if !scope_allows(&agent.scopes, op, space) {
            return Err(format!("acl_denied: scope {op}/{space} not granted"));
        }
        if is_write_op(op) && write_size > 0 {
            if agent.quota.bytes_used + write_size > agent.quota.max_bytes {
                return Err(format!(
                    "quota_exceeded: {} + {write_size} > {}",
                    agent.quota.bytes_used, agent.quota.max_bytes
                ));
            }
        }
        if is_rate_limited(&self.buckets, id, &agent.rate) {
            return Err("rate_limited: agent token bucket empty".to_string());
        }
        Ok(())
    }
}

fn is_write_op(op: &str) -> bool {
    matches!(
        op,
        "write.begin" | "write.chunk" | "commit" | "handoff.create" | "delete"
    )
}

fn scope_allows(scopes: &[String], op: &str, space: &str) -> bool {
    if scopes.iter().any(|s| s == "admin") {
        return true;
    }
    let read = matches!(
        op,
        "list"
            | "meta"
            | "read"
            | "versions.list"
            | "versions.read"
            | "manifest"
            | "artifact.state"
            | "stats"
    );
    if read {
        return scopes.iter().any(|s| s == "store:read" || s.starts_with("store:write"));
    }
    if op == "delete" {
        return scopes.iter().any(|s| s == "store:delete");
    }
    if is_write_op(op) {
        return scopes.iter().any(|s| match s.as_str() {
            "store:write" => true,
            "store:write:artifacts" => space == "artifacts",
            "store:write:files" => space == "files",
            _ => false,
        });
    }
    false
}

fn normalize_agent_scopes(scopes: Vec<String>) -> Result<Vec<String>, String> {
    let allowed = [
        "store:read",
        "store:write",
        "store:write:artifacts",
        "store:write:files",
        "store:delete",
    ];
    let mut out = Vec::new();
    for s in scopes {
        let s = s.trim().to_lowercase();
        if !allowed.contains(&s.as_str()) {
            return Err(format!("invalid agent scope: {s}"));
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

fn is_rate_limited(
    buckets: &Mutex<HashMap<String, Bucket>>,
    id: &str,
    rate: &Rate,
) -> bool {
    let now = now_ms();
    let mut map = buckets.lock().unwrap();
    let b = map.entry(id.to_string()).or_default();
    let dt = ((now - b.last_ms).max(0) as f64) / 1000.0;
    b.tokens = (b.tokens + dt * rate.tokens_per_sec).min(rate.burst as f64);
    b.last_ms = now;
    if b.tokens < 1.0 {
        true
    } else {
        b.tokens -= 1.0;
        false
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
    fn scopes_and_quota() {
        let dir = tempfile::tempdir().unwrap();
        let reg = AgentRegistry::open(dir.path());
        let a = reg
            .create(
                "readonly",
                vec!["store:read".into()],
                1024 * 1024,
            )
            .unwrap();
        // Read allowed, write denied.
        assert!(reg.check(&a.id, "read", "artifacts", 0).is_ok());
        assert!(reg.check(&a.id, "write.begin", "artifacts", 10).is_err());

        let w = reg
            .create(
                "artifacts-writer",
                vec!["store:write:artifacts".into()],
                100,
            )
            .unwrap();
        assert!(reg.check(&w.id, "write.begin", "artifacts", 10).is_ok());
        // Space scoping.
        assert!(reg.check(&w.id, "write.begin", "files", 10).is_err());
        // Quota.
        assert!(reg.check(&w.id, "write.begin", "artifacts", 101).is_err());
        reg.record_usage(&w.id, 60);
        assert!(reg.check(&w.id, "write.begin", "artifacts", 50).is_err());

        // Unknown / revoked.
        assert!(reg.check("a-deadbeef", "read", "artifacts", 0).is_err());
        reg.revoke(&w.id).unwrap();
        assert!(reg.check(&w.id, "read", "artifacts", 0).is_err());
    }

    #[test]
    fn delete_scope_required() {
        let dir = tempfile::tempdir().unwrap();
        let reg = AgentRegistry::open(dir.path());
        let a = reg
            .create("w", vec!["store:write".into()], 1000)
            .unwrap();
        assert!(reg.check(&a.id, "delete", "artifacts", 0).is_err());
        let b = reg
            .create(
                "wd",
                vec!["store:write".into(), "store:delete".into()],
                1000,
            )
            .unwrap();
        assert!(reg.check(&b.id, "delete", "artifacts", 0).is_ok());
    }
}
