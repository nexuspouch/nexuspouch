use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use subtle::ConstantTimeEq;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub fingerprint: String,
    pub public_key_b64: String,
    pub device_name: String,
    pub peer_id: String,
    pub trust_level: String,
    pub paired_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_endpoint: Option<String>,
}

pub struct PeerStore {
    path: PathBuf,
    mu: Mutex<()>,
}

impl PeerStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            path: root.join(".system").join("paired_peers.json"),
            mu: Mutex::new(()),
        }
    }

    pub fn list(&self) -> Result<Vec<Peer>, std::io::Error> {
        let _g = self.mu.lock().unwrap();
        self.load()
    }

    pub fn upsert(&self, p: Peer) -> Result<(), std::io::Error> {
        let _g = self.mu.lock().unwrap();
        let mut peers = self.load()?;
        peers.retain(|x| x.fingerprint != p.fingerprint);
        peers.push(p);
        self.save(&peers)
    }

    pub fn merge_endpoints(&self, fp: &str, local: &str, channel: &str) -> Result<(), std::io::Error> {
        if local.is_empty() && channel.is_empty() {
            return Ok(());
        }
        let mut p = match self.get(fp)? {
            Some(p) => p,
            None => return Ok(()),
        };
        if !local.is_empty() {
            p.local_endpoint = Some(local.to_string());
        }
        if !channel.is_empty() {
            p.channel_endpoint = Some(channel.to_string());
        }
        self.upsert(p)
    }

    pub fn get(&self, fp: &str) -> Result<Option<Peer>, std::io::Error> {
        Ok(self.list()?.into_iter().find(|p| p.fingerprint == fp))
    }

    pub fn remove(&self, fp: &str) -> Result<bool, std::io::Error> {
        let _g = self.mu.lock().unwrap();
        let peers = self.load()?;
        let mut found = false;
        let out: Vec<Peer> = peers
            .into_iter()
            .filter(|x| {
                if x.fingerprint == fp {
                    found = true;
                    false
                } else {
                    true
                }
            })
            .collect();
        if found {
            self.save(&out)?;
        }
        Ok(found)
    }

    fn load(&self) -> Result<Vec<Peer>, std::io::Error> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let raw = fs::read_to_string(&self.path)?;
        let peers: Vec<Peer> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(peers)
    }

    fn save(&self, peers: &[Peer]) -> Result<(), std::io::Error> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(peers).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, e)
        })?;
        let tmp = self.path.with_extension("tmp");
        fs::write(&tmp, raw)?;
        fs::rename(tmp, &self.path)
    }
}

const PAIRING_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

pub fn generate_pairing_code() -> Result<String, std::io::Error> {
    let mut out = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut out);
    Ok(out
        .iter()
        .map(|&b| PAIRING_ALPHABET[(b as usize) % PAIRING_ALPHABET.len()] as char)
        .collect())
}

pub fn constant_time_equal(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

pub fn new_peer_id() -> String {
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    format!("peer-{}", hex::encode(b))
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
