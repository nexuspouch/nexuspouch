use crate::protocol::{self, Frame};
use crate::store::OpError;
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefKind {
    Latest,
    Hash(String),
    Seq(u64),
}

#[derive(Debug)]
pub struct StoreUri {
    pub space: String,
    pub device: String,
    pub path: String,
    pub ref_kind: RefKind,
}

const SPACES: &[&str] = &["artifacts", "files", "attachments", "backups"];

/// Syntactic space-name check: `^[a-z][a-z0-9-]{0,31}$` (custom spaces
/// declared via `space.declare` must match; built-ins trivially do).
fn syntactically_valid_space(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    s.len() <= 32
        && s.chars()
            .skip(1)
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn parse(s: &str) -> Result<StoreUri, OpError> {
    // Parse layer accepts any syntactically valid space name; undeclared
    // custom spaces are rejected later by the space registry / ACL.
    parse_with(s, &|sp| syntactically_valid_space(sp))
}

/// Parse a store URI accepting spaces for which `known(space)` is true
/// (built-ins or declared custom spaces). Unknown spaces → `bad_uri`.
pub fn parse_with(s: &str, known: &dyn Fn(&str) -> bool) -> Result<StoreUri, OpError> {
    // Reject dot-segments on the RAW string first: `url::Url` silently
    // normalizes `..` away for some schemes, which would hide traversal
    // from normalize_path below.
    if has_dot_segment(s) {
        return Err(OpError::new("bad_path", "path traversal"));
    }
    let parsed = url::Url::parse(s).map_err(|e| OpError::new("bad_uri", e.to_string()))?;
    if parsed.scheme() != "store" {
        return Err(OpError::new("bad_uri", "scheme must be store"));
    }

    let host = parsed.host_str().unwrap_or("");
    let path_segments: Vec<&str> = parsed
        .path()
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    let (space, rest) = if !host.is_empty() && syntactically_valid_space(&host) && known(&host) {
        (host.to_string(), path_segments)
    } else if let Some(first) = path_segments.first() {
        if !syntactically_valid_space(first) || !known(first) {
            return Err(OpError::new("bad_uri", format!("unknown space: {first}")));
        }
        (first.to_string(), path_segments[1..].to_vec())
    } else {
        return Err(OpError::new("bad_uri", "missing space"));
    };

    if !syntactically_valid_space(&space) || !known(&space) {
        return Err(OpError::new("bad_uri", format!("invalid space: {space}")));
    }

    let device = rest
        .first()
        .ok_or_else(|| OpError::new("bad_uri", "missing device"))?
        .to_string();
    if !protocol::is_valid_device_id(&device) {
        return Err(OpError::new("bad_uri", "invalid device id"));
    }

    let mut rel = if rest.len() > 1 {
        rest[1..].join("/")
    } else {
        String::new()
    };

    // v4.2 version ref: `store://.../file@<ref>` (trailing `@` split) or `?ref=`.
    let mut ref_kind = RefKind::Latest;
    if let Some(at) = rel.rfind('@') {
        let (head, suffix) = rel.split_at(at);
        let suffix = &suffix[1..];
        if let Some(kind) = parse_ref_kind(suffix) {
            rel = head.to_string();
            ref_kind = kind;
        } else if looks_like_ref_attempt(suffix) {
            // Reserved syntax: `@v<bad>` / hash-like suffix -> bad_uri.
            return Err(OpError::new(
                "bad_uri",
                format!("invalid version ref: @{suffix}"),
            ));
        }
    }
    if ref_kind == RefKind::Latest {
        if let Some(ref_value) = parsed.query_pairs().find(|(k, _)| k == "ref") {
            let v = ref_value.1.to_string();
            ref_kind = parse_ref_kind(&v)
                .ok_or_else(|| OpError::new("bad_uri", format!("invalid ref: {v}")))?;
        }
    }

    let path = if rel.is_empty() {
        String::new()
    } else {
        protocol::normalize_path(&rel).map_err(|e| OpError::new("bad_path", e))?
    };

    Ok(StoreUri {
        space,
        device,
        path,
        ref_kind,
    })
}

/// True if the raw URI contains a `.` / `..` segment (after percent-decoding),
/// including backslash-encoded variants (`%5C%2E%2E`).
fn has_dot_segment(s: &str) -> bool {
    let rest = s.strip_prefix("store://").unwrap_or(s);
    let after_host = match rest.find('/') {
        Some(i) => &rest[i..],
        None => "",
    };
    let raw = percent_decode(after_host);
    let decoded = String::from_utf8_lossy(&raw);
    decoded
        .split(['/', '\\'])
        .any(|seg| seg == "." || seg == "..")
}

fn percent_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse a `<ref>` token: `@<sha256[:16+]>` (hex, >= 16 chars) or `@v<N>` (N >= 1).
fn parse_ref_kind(s: &str) -> Option<RefKind> {
    if s.len() >= 16 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(RefKind::Hash(s.to_ascii_lowercase()));
    }
    if let Some(rest) = s.strip_prefix('v') {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = rest.parse::<u64>() {
                if n >= 1 {
                    return Some(RefKind::Seq(n));
                }
            }
        }
    }
    None
}

/// `@` followed by something that resembles a ref (v-prefixed, hex-like, or a
/// long alphanumeric token) is a malformed ref -> `bad_uri`. Anything else
/// (`a@home.txt`) is treated as part of the filename.
fn looks_like_ref_attempt(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    s.starts_with('v')
        || s.chars().all(|c| c.is_ascii_hexdigit())
        || (s.len() >= 16 && s.chars().all(|c| c.is_ascii_alphanumeric()))
}

impl StoreUri {
    pub fn format(&self) -> String {
        if self.path.is_empty() {
            format!("store://{}/{}/", self.space, self.device)
        } else {
            format!("store://{}/{}/{}", self.space, self.device, self.path)
        }
    }

    pub fn format_with_ref(&self) -> String {
        let base = self.format();
        match &self.ref_kind {
            RefKind::Latest => base,
            RefKind::Hash(h) => format!("{base}@{h}"),
            RefKind::Seq(n) => format!("{base}@v{n}"),
        }
    }

    pub fn to_frame_list(&self) -> Frame {
        let mut payload = base_payload(self);
        if !self.path.is_empty() {
            payload.insert("path".into(), json!(self.path));
        }
        Frame::from_parts("list", payload)
    }

    pub fn to_frame_meta(&self) -> Frame {
        let mut payload = base_payload(self);
        if !self.path.is_empty() {
            payload.insert("path".into(), json!(self.path));
        }
        Frame::from_parts("meta", payload)
    }

    pub fn to_frame_read(&self, offset: i64, length: usize) -> Frame {
        let mut payload = base_payload(self);
        payload.insert("path".into(), json!(self.path));
        payload.insert("offset".into(), json!(offset));
        payload.insert("length".into(), json!(length));
        Frame::from_parts("read", payload)
    }
}

fn base_payload(uri: &StoreUri) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("space".into(), json!(uri.space));
    m.insert("device".into(), json!(uri.device));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_host_style() {
        let u = parse("store://artifacts/aaaaaaaaaaaaaaaa/task-1/out.txt").unwrap();
        assert_eq!(u.space, "artifacts");
        assert_eq!(u.device, "aaaaaaaaaaaaaaaa");
        assert_eq!(u.path, "task-1/out.txt");
        assert_eq!(u.ref_kind, RefKind::Latest);
        assert_eq!(u.format(), "store://artifacts/aaaaaaaaaaaaaaaa/task-1/out.txt");
    }

    #[test]
    fn parse_version_refs() {
        let u = parse("store://artifacts/aaaaaaaaaaaaaaaa/task-1/out.txt@3f9a2c1d4e5f6071").unwrap();
        assert_eq!(u.path, "task-1/out.txt");
        assert_eq!(u.ref_kind, RefKind::Hash("3f9a2c1d4e5f6071".into()));
        assert_eq!(u.format_with_ref(), "store://artifacts/aaaaaaaaaaaaaaaa/task-1/out.txt@3f9a2c1d4e5f6071");

        let u = parse("store://artifacts/aaaaaaaaaaaaaaaa/a.txt@v3").unwrap();
        assert_eq!(u.path, "a.txt");
        assert_eq!(u.ref_kind, RefKind::Seq(3));

        let u = parse("store://artifacts/aaaaaaaaaaaaaaaa/a.txt?ref=v2").unwrap();
        assert_eq!(u.ref_kind, RefKind::Seq(2));

        // Invalid refs -> bad_uri.
        assert_eq!(parse("store://artifacts/aaaaaaaaaaaaaaaa/a.txt@v0").unwrap_err().code, "bad_uri");
        assert_eq!(parse("store://artifacts/aaaaaaaaaaaaaaaa/a.txt@vx").unwrap_err().code, "bad_uri");
        assert_eq!(parse("store://artifacts/aaaaaaaaaaaaaaaa/a.txt@3f9a2c1d").unwrap_err().code, "bad_uri");
        assert_eq!(parse("store://artifacts/aaaaaaaaaaaaaaaa/a.txt@zzzzzzzzzzzzzzzz").unwrap_err().code, "bad_uri");
        assert_eq!(parse("store://artifacts/aaaaaaaaaaaaaaaa/a.txt?ref=oops").unwrap_err().code, "bad_uri");
    }

    #[test]
    fn parse_ref_not_swallowed_in_filename() {
        // `@` inside a filename with a non-ref suffix stays part of the path.
        let u = parse("store://files/aaaaaaaaaaaaaaaa/contact@home.txt").unwrap();
        assert_eq!(u.path, "contact@home.txt");
        assert_eq!(u.ref_kind, RefKind::Latest);
    }

    #[test]
    fn version_cases_fixture_parse() {
        // Shared fixture (docs/storage_fixtures/version_cases.json, spec §1.5).
        // Parse-level: ok/bad_uri/bad_path are checked here; ambiguous_ref and
        // not_found are resolution semantics implemented in the version store
        // (they must still PARSE as ok).
        let raw = std::fs::read_to_string("docs/storage_fixtures/version_cases.json")
            .or_else(|_| std::fs::read_to_string("../docs/storage_fixtures/version_cases.json"))
            .expect("version_cases.json");
        let doc: Value = serde_json::from_str(&raw).unwrap();
        for c in doc["cases"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let uri = c["uri"].as_str().unwrap();
            let expect = c["expect"].as_str().unwrap();
            match parse(uri) {
                Ok(u) => {
                    assert!(
                        matches!(expect, "ok" | "ambiguous_ref" | "not_found"),
                        "{name}: expected {expect}, parsed ok"
                    );
                    if expect == "ok" {
                        assert_eq!(u.space, c["space"].as_str().unwrap(), "{name}");
                        assert_eq!(u.device, c["device"].as_str().unwrap(), "{name}");
                        assert_eq!(u.path, c["path"].as_str().unwrap(), "{name}");
                        match c["ref_kind"].as_str().unwrap() {
                            "latest" => assert_eq!(u.ref_kind, RefKind::Latest, "{name}"),
                            "hash" => {
                                assert!(matches!(u.ref_kind, RefKind::Hash(_)), "{name}");
                            }
                            "seq" => {
                                assert!(matches!(u.ref_kind, RefKind::Seq(_)), "{name}");
                            }
                            other => panic!("{name}: bad ref_kind {other}"),
                        }
                    }
                }
                Err(e) => {
                    assert!(
                        matches!(expect, "bad_uri" | "bad_path"),
                        "{name}: expected {expect}, got error {e:?}"
                    );
                    assert_eq!(e.code, expect, "{name}");
                }
            }
        }
    }

    #[test]
    fn parse_path_style() {
        let u = parse("store:///files/bbbbbbbbbbbbbbbb/docs/readme.md").unwrap();
        assert_eq!(u.space, "files");
        assert_eq!(u.device, "bbbbbbbbbbbbbbbb");
        assert_eq!(u.path, "docs/readme.md");
    }

    #[test]
    fn roundtrip_device_only() {
        let u = parse("store://artifacts/cccccccccccccccc/").unwrap();
        assert_eq!(u.path, "");
        assert_eq!(u.format(), "store://artifacts/cccccccccccccccc/");
    }

    #[test]
    fn reject_bad_scheme() {
        assert!(parse("file://artifacts/aaaaaaaaaaaaaaaa/x").is_err());
    }

    #[test]
    fn reject_traversal() {
        assert!(parse("store://artifacts/aaaaaaaaaaaaaaaa/../secret").is_err());
    }

    #[test]
    fn reject_bad_device() {
        assert!(parse("store://artifacts/not-a-device/x").is_err());
    }

    #[test]
    fn frame_helpers() {
        let u = parse("store://files/aaaaaaaaaaaaaaaa/a/b.txt").unwrap();
        let f = u.to_frame_read(10, 4096);
        assert_eq!(f.op, "read");
        assert_eq!(f.payload["offset"], json!(10));
        assert_eq!(f.payload["path"], json!("a/b.txt"));
    }
}
