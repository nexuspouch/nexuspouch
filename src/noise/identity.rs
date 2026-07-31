use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use snow::Builder;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Identity {
    pub public_key: [u8; 32],
    pub private_key: [u8; 32],
    pub created_at: i64,
}

#[derive(Serialize, Deserialize)]
struct IdentityFile {
    public_key_b64: String,
    private_key_b64: String,
    created_at_ms: i64,
}

impl Identity {
    pub fn fingerprint(&self) -> String {
        let sum = Sha256::digest(self.public_key);
        hex::encode(&sum[..8])
    }

    pub fn public_key_base64(&self) -> String {
        STANDARD.encode(self.public_key)
    }

    pub fn load_or_create(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        if path.exists() {
            let raw = fs::read_to_string(path)?;
            let f: IdentityFile = serde_json::from_str(&raw)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let pub_bytes = STANDARD
                .decode(&f.public_key_b64)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let priv_bytes = STANDARD
                .decode(&f.private_key_b64)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            if pub_bytes.len() != 32 || priv_bytes.len() != 32 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "bad key length",
                ));
            }
            let mut public_key = [0u8; 32];
            let mut private_key = [0u8; 32];
            public_key.copy_from_slice(&pub_bytes);
            private_key.copy_from_slice(&priv_bytes);
            return Ok(Self {
                public_key,
                private_key,
                created_at: f.created_at_ms,
            });
        }

        let kp = Builder::new("Noise_IK_25519_ChaChaPoly_BLAKE2b".parse().unwrap())
            .generate_keypair()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        let mut public_key = [0u8; 32];
        let mut private_key = [0u8; 32];
        public_key.copy_from_slice(&kp.public);
        private_key.copy_from_slice(&kp.private);
        let id = Self {
            public_key,
            private_key,
            created_at: now_ms(),
        };
        id.save(path)?;
        Ok(id)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let f = IdentityFile {
            public_key_b64: STANDARD.encode(self.public_key),
            private_key_b64: STANDARD.encode(self.private_key),
            created_at_ms: self.created_at,
        };
        let raw = serde_json::to_string_pretty(&f)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, raw)?;
        fs::rename(tmp, path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(path) {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                let _ = fs::set_permissions(path, perms);
            }
        }
        Ok(())
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
