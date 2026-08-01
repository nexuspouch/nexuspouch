use crate::protocol::{self, Frame};
use crate::store::OpError;
use serde_json::{json, Map, Value};

pub struct StoreUri {
    pub space: String,
    pub device: String,
    pub path: String,
}

const SPACES: &[&str] = &["artifacts", "files", "attachments", "backups"];

pub fn parse(s: &str) -> Result<StoreUri, OpError> {
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

    let (space, rest) = if !host.is_empty() && SPACES.contains(&host) {
        (host.to_string(), path_segments)
    } else if let Some(first) = path_segments.first() {
        if !SPACES.contains(first) {
            return Err(OpError::new("bad_uri", format!("unknown space: {first}")));
        }
        (first.to_string(), path_segments[1..].to_vec())
    } else {
        return Err(OpError::new("bad_uri", "missing space"));
    };

    if !protocol::is_valid_space(&space) {
        return Err(OpError::new("bad_uri", format!("invalid space: {space}")));
    }

    let device = rest
        .first()
        .ok_or_else(|| OpError::new("bad_uri", "missing device"))?
        .to_string();
    if !protocol::is_valid_device_id(&device) {
        return Err(OpError::new("bad_uri", "invalid device id"));
    }

    let rel = if rest.len() > 1 {
        rest[1..].join("/")
    } else {
        String::new()
    };

    let path = if rel.is_empty() {
        String::new()
    } else {
        protocol::normalize_path(&rel).map_err(|e| OpError::new("bad_path", e))?
    };

    Ok(StoreUri {
        space,
        device,
        path,
    })
}

impl StoreUri {
    pub fn format(&self) -> String {
        if self.path.is_empty() {
            format!("store://{}/{}/", self.space, self.device)
        } else {
            format!("store://{}/{}/{}", self.space, self.device, self.path)
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
        assert_eq!(u.format(), "store://artifacts/aaaaaaaaaaaaaaaa/task-1/out.txt");
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
