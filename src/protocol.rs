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

/// Mirrors Dart `checkStoreAcl`.
pub fn check_acl(frame: &Frame, caller_device_id: &str, trust_level: &str, loopback: bool) -> AclVerdict {
    if trust_level != TRUST_OWNER {
        return AclVerdict::DenyUntrusted;
    }
    let space = frame.space().unwrap_or("");
    let device = frame.device().unwrap_or("");

    match frame.op.as_str() {
        "write.begin" | "write.chunk" | "commit" => {
            if !space.is_empty() && !is_valid_space(space) {
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
        "delete" => {
            if space.is_empty() || !is_valid_space(space) {
                return AclVerdict::DenyBadOp;
            }
            let target_own = device.is_empty() || device == caller_device_id;
            if !target_own && !shared_readable(space) {
                return AclVerdict::DenyAcl;
            }
            if !device.is_empty() && !is_valid_device_id(device) {
                return AclVerdict::DenyBadOp;
            }
            AclVerdict::Allow
        }
        "list" | "meta" | "read" => {
            if space.is_empty() || !is_valid_space(space) {
                return AclVerdict::DenyBadOp;
            }
            let target_own = device.is_empty() || device == caller_device_id;
            if !target_own && !shared_readable(space) {
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
        "stats" => AclVerdict::Allow,
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
}
