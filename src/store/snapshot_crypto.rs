//! ShePaw-compatible snapshot / reprotect crypto.
//!
//! Matches Flutter `lib/storage/snapshot_crypto.dart`:
//! ```text
//! H   = PBKDF2-HMAC-SHA256(password, "shepaw.storage.v1", 120_000, 32)
//! key = HMAC-SHA256(H, "shepaw.snapshot.key" ‖ snapshot_salt)
//! ct  = XChaCha20-Poly1305: nonce(24) ‖ ciphertext ‖ tag(16)
//! ```

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::Hmac;
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use sha2::Sha256;

const PBKDF2_ITERATIONS: u32 = 120_000;
const HASH_DOMAIN: &[u8] = b"shepaw.storage.v1";
const KEY_DOMAIN: &[u8] = b"shepaw.snapshot.key";
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;

type HmacSha256 = Hmac<Sha256>;

pub fn hash_password(password: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), HASH_DOMAIN, PBKDF2_ITERATIONS, &mut out);
    out
}

pub fn new_snapshot_salt() -> [u8; 32] {
    let mut salt = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut salt);
    salt
}

pub fn derive_key_from_hash(password_hash: &[u8; 32], snapshot_salt: &[u8; 32]) -> [u8; 32] {
    use hmac::Mac;
    let mut mac = <HmacSha256 as Mac>::new_from_slice(password_hash).expect("hmac key");
    mac.update(KEY_DOMAIN);
    mac.update(snapshot_salt);
    mac.finalize().into_bytes().into()
}

/// Encrypt plaintext → nonce(24) ‖ ciphertext ‖ tag(16).
pub fn encrypt(plain: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plain)
        .map_err(|e| format!("encrypt: {e}"))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypt packed nonce‖ct‖tag.
pub fn decrypt(packed: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    if packed.len() < NONCE_LEN + TAG_LEN {
        return Err("ciphertext too short".into());
    }
    let (nonce_bytes, rest) = packed.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XNonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, rest)
        .map_err(|_| "decrypt failed (wrong password or tampered data)".into())
}

pub const KDF_ITERATIONS: u32 = PBKDF2_ITERATIONS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_encrypt_decrypt() {
        let h = hash_password("test-password");
        let salt = new_snapshot_salt();
        let key = derive_key_from_hash(&h, &salt);
        let plain = b"hello mirror reprotect";
        let enc = encrypt(plain, &key).unwrap();
        assert_eq!(enc.len(), NONCE_LEN + plain.len() + TAG_LEN);
        let dec = decrypt(&enc, &key).unwrap();
        assert_eq!(dec, plain);
    }

    #[test]
    fn wrong_password_fails() {
        let salt = new_snapshot_salt();
        let key_ok = derive_key_from_hash(&hash_password("a"), &salt);
        let key_bad = derive_key_from_hash(&hash_password("b"), &salt);
        let enc = encrypt(b"secret", &key_ok).unwrap();
        assert!(decrypt(&enc, &key_bad).is_err());
    }

    /// Cross-language golden vector (see docs/storage_fixtures/reprotect_kdf_vector.json).
    #[test]
    fn shepaw_kdf_golden_vector() {
        let h = hash_password("vector-password");
        assert_eq!(
            hex::encode(h),
            "e16e67533b238dd9b81256288ff7acbf3007619dcc6e6facf2279fd163dfe3de"
        );
        let salt = [0xABu8; 32];
        let key = derive_key_from_hash(&h, &salt);
        assert_eq!(
            hex::encode(key),
            "27471c6533ccb021a64d371ac66c11028977c85cc0a79ee6d6a274ebbd75c0f0"
        );
    }
}
