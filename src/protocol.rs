use serde_json::Value;

pub const PROTOCOL_VERSION: i32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AclVerdict {
    Allow,
    DenyUntrusted,
    DenyAcl,
    DenyBadOp,
    DenyBadPath,
}

impl AclVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::DenyUntrusted => "denyUntrusted",
            Self::DenyAcl => "denyAcl",
            Self::DenyBadOp => "denyBadOp",
            Self::DenyBadPath => "denyBadPath",
        }
    }
}

pub const TRUST_OWNER: &str = "owner";
pub const TRUST_FRIEND: &str = "friend";

#[derive(Debug, Clone)]
pub struct Frame {
    pub op: String,
    pub payload: serde_json::Map<String, Value>,
}

impl Frame {
    pub fn from_parts(op: impl Into<String>, payload: serde_json::Map<String, Value>) -> Self {
        Self {
            op: op.into(),
            payload,
        }
    }

    pub fn space(&self) -> Option<&str> {
        self.payload.get("space").and_then(|v| v.as_str())
    }

    pub fn device(&self) -> Option<&str> {
        self.payload.get("device").and_then(|v| v.as_str())
    }

    pub fn payload_bool(&self, key: &str) -> bool {
        self.payload.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
    }
}

pub fn is_valid_device_id(device: &str) -> bool {
    device.len() == 16
        && device
            .chars()
            .all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

fn is_drive_or_unc(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return true;
    }
    raw.starts_with("\\\\")
}

pub fn is_valid_space(s: &str) -> bool {
    matches!(s, "artifacts" | "files" | "attachments" | "backups")
}

pub fn shared_readable(s: &str) -> bool {
    s == "artifacts" || s == "files"
}

/// Mirrors Dart `normalizeStorePath`.
pub fn normalize_path(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("empty path".into());
    }
    if raw.contains('\0') {
        return Err("NUL in path".into());
    }
    if raw.starts_with('/') || raw.starts_with('~') {
        return Err("absolute path".into());
    }
    if is_drive_or_unc(raw) {
        return Err("drive/unc path".into());
    }
    let normalized = raw.replace('\\', "/");
    let mut out = Vec::new();
    for seg in normalized.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err("path traversal".into());
        }
        if seg.starts_with('.') {
            return Err(format!("dot segment: {seg}"));
        }
        out.push(seg);
    }
    if out.is_empty() {
        return Err("resolves to empty".into());
    }
    Ok(out.join("/"))
}

/// Built-in space visibility: `Some(shared?)`.
pub fn builtin_visibility(space: &str) -> Option<bool> {
    match space {
        "artifacts" | "files" => Some(true),
        "attachments" | "backups" => Some(false),
        _ => None,
    }
}

/// Mirrors Dart `checkStoreAcl` for the four built-in spaces.
pub fn check_acl(frame: &Frame, caller_device_id: &str, trust_level: &str, loopback: bool) -> AclVerdict {
    check_acl_with(frame, caller_device_id, trust_level, loopback, &builtin_visibility)
}

/// Attribute-driven ACL (Step 2): `vis(space)` returns `Some(shared?)` for
/// known spaces (built-in or declared), `None` for unknown.
pub fn check_acl_with(
    frame: &Frame,
    caller_device_id: &str,
    trust_level: &str,
    loopback: bool,
    vis: &dyn Fn(&str) -> Option<bool>,
) -> AclVerdict {
    if trust_level != TRUST_OWNER {
        return AclVerdict::DenyUntrusted;
    }
    let space = frame.space().unwrap_or("");
    let device = frame.device().unwrap_or("");
    let known = |sp: &str| is_valid_space(sp) || vis(sp).is_some();
    let shared = |sp: &str| vis(sp) == Some(true);

    match frame.op.as_str() {
        "write.begin" | "write.chunk" | "commit" | "handoff.create" => {
            if !space.is_empty() && !known(space) {
                return AclVerdict::DenyBadOp;
            }
            if frame.op == "write.begin" && space.is_empty() {
                return AclVerdict::DenyBadOp;
            }
            if !device.is_empty() && device != caller_device_id {
                return AclVerdict::DenyAcl;
            }
            AclVerdict::Allow
        }
        "delete" | "handoff.ack" => {
            if space.is_empty() || !known(space) {
                return AclVerdict::DenyBadOp;
            }
            let target_own = device.is_empty() || device == caller_device_id;
            if !target_own && !shared(space) {
                return AclVerdict::DenyAcl;
            }
            if !device.is_empty() && !is_valid_device_id(device) {
                return AclVerdict::DenyBadOp;
            }
            AclVerdict::Allow
        }
        "list" | "meta" | "read" | "versions.list" | "versions.read" | "manifest"
        | "artifact.state" => {
            if space.is_empty() || !known(space) {
                return AclVerdict::DenyBadOp;
            }
            let target_own = device.is_empty() || device == caller_device_id;
            if !target_own && !shared(space) {
                if frame.payload_bool("seed") {
                    if !device.is_empty() && !is_valid_device_id(device) {
                        return AclVerdict::DenyBadOp;
                    }
                    return AclVerdict::Allow;
                }
                let grant = frame.payload.get("grant").and_then(|v| v.as_str()).unwrap_or("");
                if grant.is_empty() {
                    return AclVerdict::DenyAcl;
                }
            }
            if !device.is_empty() && !is_valid_device_id(device) {
                return AclVerdict::DenyBadOp;
            }
            AclVerdict::Allow
        }
        "recycle.list" | "recycle.restore" => AclVerdict::Allow,
        "recycle.empty" => {
            if loopback {
                AclVerdict::Allow
            } else {
                AclVerdict::DenyAcl
            }
        }
        "import.request" => {
            let old = frame.payload.get("old_device").and_then(|v| v.as_str()).unwrap_or("");
            if !is_valid_device_id(old) || old == caller_device_id {
                return AclVerdict::DenyBadOp;
            }
            AclVerdict::Allow
        }
        "import.pending" => AclVerdict::Allow,
        "import.grant" | "import.reject" | "import.grants" => {
            if loopback {
                AclVerdict::Allow
            } else {
                AclVerdict::DenyAcl
            }
        }
        "stats" | "space.list" => AclVerdict::Allow,
        "space.declare" => {
            if loopback {
                AclVerdict::Allow
            } else {
                AclVerdict::DenyAcl
            }
        }
        "sync.hello" => {
            let d = frame.payload.get("device").and_then(|v| v.as_str()).unwrap_or("");
            if !is_valid_device_id(d) || d != caller_device_id {
                return AclVerdict::DenyAcl;
            }
            AclVerdict::Allow
        }
        "sync.cursors" | "master.pointer.query" | "master.migrate" | "master.pointer" => {
            AclVerdict::Allow
        }
        _ => AclVerdict::DenyBadOp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;
    use std::path::PathBuf;

    fn fixtures_dir() -> PathBuf {
        let candidates = [
            PathBuf::from("docs/storage_fixtures"),
            PathBuf::from("../docs/storage_fixtures"),
        ];
        for c in candidates {
            if c.is_dir() {
                return c.canonicalize().unwrap_or(c);
            }
        }
        panic!("docs/storage_fixtures not found");
    }

    #[test]
    fn path_attacks_fixture() {
        let raw = fs::read_to_string(fixtures_dir().join("path_attacks.json")).unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        for p in doc["attacks"].as_array().unwrap() {
            let p = p.as_str().unwrap();
            assert!(normalize_path(p).is_err(), "expected reject: {p:?}");
        }
        for row in doc["ok"].as_array().unwrap() {
            let inp = row[0].as_str().unwrap();
            let want = row[1].as_str().unwrap();
            let got = normalize_path(inp).unwrap_or_else(|e| panic!("normalize {inp:?}: {e}"));
            assert_eq!(got, want, "normalize {inp:?}");
        }
    }

    #[test]
    fn acl_fixture() {
        let raw = fs::read_to_string(fixtures_dir().join("acl_cases.json")).unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        let caller = doc["caller"].as_str().unwrap();
        for c in doc["cases"].as_array().unwrap() {
            let payload = c["payload"].as_object().cloned().unwrap_or_default();
            let frame = Frame::from_parts(c["op"].as_str().unwrap(), payload);
            let trust = c["trust"].as_str().unwrap();
            let loopback = c["loopback"].as_bool().unwrap();
            let expect = c["expect"].as_str().unwrap();
            let v = check_acl(&frame, caller, trust, loopback);
            assert_eq!(v.as_str(), expect, "{}", c["name"].as_str().unwrap());
        }
    }

    #[test]
    fn version_cases_fixture_contract() {
        // M0: validate the v4.2 URI/version contract fixture (docs/storage_protocol_spec.md §1.5).
        // Behavior is implemented in M2; this test locks the schema both ends must consume.
        let raw = fs::read_to_string(fixtures_dir().join("version_cases.json")).unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        let cases = doc["cases"].as_array().unwrap();
        assert!(!cases.is_empty());
        let allowed = ["ok", "bad_uri", "bad_path", "ambiguous_ref", "not_found"];
        let mut names = HashSet::new();
        for c in cases {
            let name = c["name"].as_str().unwrap();
            assert!(names.insert(name.to_string()), "duplicate case {name}");
            let uri = c["uri"].as_str().unwrap();
            assert!(uri.starts_with("store://"), "{name}: scheme must be store");
            let expect = c["expect"].as_str().unwrap();
            assert!(allowed.contains(&expect), "{name}: bad expect {expect}");
            if expect == "ok" {
                assert!(c.get("space").is_some(), "{name}: ok case needs space");
                assert!(c.get("device").is_some(), "{name}: ok case needs device");
                assert!(c.get("path").is_some(), "{name}: ok case needs path");
                let kind = c["ref_kind"].as_str().unwrap();
                assert!(
                    ["latest", "hash", "seq"].contains(&kind),
                    "{name}: bad ref_kind {kind}"
                );
            }
            if expect == "bad_path" {
                let rel = uri.split_once("store://").unwrap().1;
                assert!(
                    rel.split('/').any(|s| s.starts_with('.')),
                    "{name}: bad_path cases must target a dot-prefixed segment"
                );
            }
        }
    }

    #[test]
    fn agent_acl_cases_fixture_contract() {
        // M0: validate the agent ACL contract fixture (docs/AGENTS.md §4, spec §12).
        // Behavior is implemented in M4; this test locks the schema both ends must consume.
        let raw = fs::read_to_string(fixtures_dir().join("agent_acl_cases.json")).unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        let cases = doc["cases"].as_array().unwrap();
        assert!(!cases.is_empty());
        let allowed = ["allow", "denyAcl", "quota_exceeded", "denyUntrusted"];
        let mut names = HashSet::new();
        for c in cases {
            let name = c["name"].as_str().unwrap();
            assert!(names.insert(name.to_string()), "duplicate case {name}");
            let expect = c["expect"].as_str().unwrap();
            assert!(allowed.contains(&expect), "{name}: bad expect {expect}");
            assert!(c["op"].as_str().is_some(), "{name}: op required");
            // Loopback cases are device-only and need no agent binding.
            if c.get("loopback").and_then(|v| v.as_bool()).unwrap_or(false) {
                continue;
            }
            assert!(
                c.get("agent").is_some() || c.get("token_bound_agent").is_some(),
                "{name}: agent or token binding required"
            );
            assert!(c["registered"].as_bool().is_some(), "{name}: registered required");
            assert!(c["status"].as_str().is_some(), "{name}: status required");
            assert!(c["scopes"].as_array().is_some(), "{name}: scopes required");
            if c.get("quota").is_some() {
                let q = &c["quota"];
                assert!(q["max_bytes"].as_u64().is_some(), "{name}: quota.max_bytes");
                assert!(q["bytes_used"].as_u64().is_some(), "{name}: quota.bytes_used");
            }
        }
    }

    #[test]
    fn handoff_fixture() {
        let raw = fs::read_to_string(fixtures_dir().join("handoff_cases.json")).unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        let caller = doc["caller"].as_str().unwrap();
        for c in doc["cases"].as_array().unwrap() {
            let payload = c["payload"].as_object().cloned().unwrap_or_default();
            let frame = Frame::from_parts(c["op"].as_str().unwrap(), payload);
            let trust = c["trust"].as_str().unwrap();
            let loopback = c["loopback"].as_bool().unwrap();
            let expect = c["expect"].as_str().unwrap();
            let v = check_acl(&frame, caller, trust, loopback);
            assert_eq!(v.as_str(), expect, "{}", c["name"].as_str().unwrap());
        }
    }

    #[test]
    fn space_profile_fixture() {
        let raw = fs::read_to_string(fixtures_dir().join("space_profile_cases.json")).unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        let caller = doc["caller"].as_str().unwrap();
        let known = doc["known"].as_object().unwrap().clone();
        let vis = move |sp: &str| {
            known
                .get(sp)
                .and_then(|v| v.as_str())
                .map(|s| s == "shared")
        };
        for c in doc["cases"].as_array().unwrap() {
            let payload = c["payload"].as_object().cloned().unwrap_or_default();
            let frame = Frame::from_parts(c["op"].as_str().unwrap(), payload);
            let trust = c["trust"].as_str().unwrap();
            let loopback = c["loopback"].as_bool().unwrap();
            let expect = c["expect"].as_str().unwrap();
            let v = check_acl_with(&frame, caller, trust, loopback, &vis);
            assert_eq!(v.as_str(), expect, "{}", c["name"].as_str().unwrap());
        }
    }
}
