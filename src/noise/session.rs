use super::identity::Identity;
use snow::{Builder, HandshakeState, TransportState};

pub const PROLOGUE: &[u8] = b"shepaw-acp/2.1";

const PATTERN: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2b";

pub struct Session {
    initiator: bool,
    handshake: Option<HandshakeState>,
    transport: Option<TransportState>,
    peer_pub: Option<[u8; 32]>,
}

impl Session {
    pub fn new_initiator(id: &Identity, peer_static: &[u8; 32]) -> Result<Self, String> {
        let hs = Builder::new(PATTERN.parse().map_err(|e: snow::Error| e.to_string())?)
            .local_private_key(&id.private_key)
            .remote_public_key(peer_static)
            .prologue(PROLOGUE)
            .build_initiator()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            initiator: true,
            handshake: Some(hs),
            transport: None,
            peer_pub: Some(*peer_static),
        })
    }

    pub fn new_responder(id: &Identity) -> Result<Self, String> {
        let hs = Builder::new(PATTERN.parse().map_err(|e: snow::Error| e.to_string())?)
            .local_private_key(&id.private_key)
            .prologue(PROLOGUE)
            .build_responder()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            initiator: false,
            handshake: Some(hs),
            transport: None,
            peer_pub: None,
        })
    }

    pub fn write_handshake1(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        if !self.initiator {
            return Err("not initiator or wrong phase".into());
        }
        let hs = self.handshake.as_mut().ok_or("not initiator or wrong phase")?;
        let mut buf = vec![0u8; payload.len() + 256];
        let len = hs
            .write_message(payload, &mut buf)
            .map_err(|e| e.to_string())?;
        buf.truncate(len);
        Ok(buf)
    }

    pub fn read_handshake1(&mut self, msg: &[u8]) -> Result<(Vec<u8>, [u8; 32]), String> {
        if self.initiator {
            return Err("not responder or wrong phase".into());
        }
        let hs = self.handshake.as_mut().ok_or("not responder or wrong phase")?;
        let mut payload = vec![0u8; msg.len()];
        let len = hs
            .read_message(msg, &mut payload)
            .map_err(|e| e.to_string())?;
        payload.truncate(len);
        let remote = hs
            .get_remote_static()
            .ok_or_else(|| "missing peer static".to_string())?;
        let mut peer = [0u8; 32];
        peer.copy_from_slice(remote);
        self.peer_pub = Some(peer);
        Ok((payload, peer))
    }

    pub fn write_handshake2(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        if self.initiator {
            return Err("not responder or wrong phase".into());
        }
        let mut hs = self.handshake.take().ok_or("not responder or wrong phase")?;
        let mut buf = vec![0u8; payload.len() + 256];
        let len = hs
            .write_message(payload, &mut buf)
            .map_err(|e| e.to_string())?;
        buf.truncate(len);
        let peer = self.peer_pub.ok_or("missing peer static")?;
        self.transport = Some(hs.into_transport_mode().map_err(|e| e.to_string())?);
        self.peer_pub = Some(peer);
        Ok(buf)
    }

    pub fn read_handshake2(&mut self, msg: &[u8]) -> Result<Vec<u8>, String> {
        if !self.initiator {
            return Err("not initiator or wrong phase".into());
        }
        let mut hs = self.handshake.take().ok_or("not initiator or wrong phase")?;
        let mut payload = vec![0u8; msg.len()];
        let len = hs
            .read_message(msg, &mut payload)
            .map_err(|e| e.to_string())?;
        payload.truncate(len);
        let peer = self.peer_pub.ok_or("missing peer static")?;
        self.transport = Some(hs.into_transport_mode().map_err(|e| e.to_string())?);
        self.peer_pub = Some(peer);
        Ok(payload)
    }

    pub fn peer_static(&self) -> Option<[u8; 32]> {
        self.peer_pub
    }

    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let transport = self.transport.as_mut().ok_or("session not ready")?;
        let mut buf = vec![0u8; plaintext.len() + 32];
        let len = transport
            .write_message(plaintext, &mut buf)
            .map_err(|e| e.to_string())?;
        buf.truncate(len);
        Ok(buf)
    }

    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, String> {
        let transport = self.transport.as_mut().ok_or("session not ready")?;
        let mut buf = vec![0u8; ciphertext.len()];
        let len = transport
            .read_message(ciphertext, &mut buf)
            .map_err(|e| e.to_string())?;
        buf.truncate(len);
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::envelope::{decode_frame, encode_frame, Frame as EnvFrame, FrameType};
    use crate::noise::identity::Identity;
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    #[test]
    fn identity_fingerprint_stable() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("id.json");
        let id = Identity::load_or_create(&path).unwrap();
        let sum = Sha256::digest(id.public_key);
        let want = hex::encode(&sum[..8]);
        assert_eq!(id.fingerprint(), want);
        let id2 = Identity::load_or_create(&path).unwrap();
        assert_eq!(id2.fingerprint(), id.fingerprint());
    }

    #[test]
    fn noise_ik_roundtrip() {
        let dir = tempdir().unwrap();
        let resp_id = Identity::load_or_create(dir.path().join("resp.json")).unwrap();
        let init_id = Identity::load_or_create(dir.path().join("init.json")).unwrap();

        let mut initiator = Session::new_initiator(&init_id, &resp_id.public_key).unwrap();
        let mut responder = Session::new_responder(&resp_id).unwrap();

        let msg1 = initiator
            .write_handshake1(br#"{"pairing_code":"ABCD2345","device_name":"phone","device_id":"x","timestamp":1}"#)
            .unwrap();
        let (payload, peer_pub) = responder.read_handshake1(&msg1).unwrap();
        assert!(!payload.is_empty());
        assert_eq!(peer_pub, init_id.public_key);

        let msg2 = responder
            .write_handshake2(br#"{"accepted":true,"device_name":"node","device_id":"y","peer_id":"peer-1"}"#)
            .unwrap();
        initiator.read_handshake2(&msg2).unwrap();

        let ct = initiator
            .encrypt(br#"{"op":"stats","payload":{}}"#)
            .unwrap();
        let pt = responder.decrypt(&ct).unwrap();
        assert_eq!(pt, br#"{"op":"stats","payload":{}}"#);

        let ct2 = responder.encrypt(br#"{"op":"result"}"#).unwrap();
        let pt2 = initiator.decrypt(&ct2).unwrap();
        assert_eq!(pt2, br#"{"op":"result"}"#);
    }

    #[test]
    fn envelope_roundtrip() {
        let enc = encode_frame(&EnvFrame {
            frame_type: FrameType::Hs,
            payload: vec![1, 2, 3],
        })
        .unwrap();
        let fr = decode_frame(&enc).unwrap();
        assert_eq!(fr.frame_type, FrameType::Hs);
        assert_eq!(fr.payload, vec![1, 2, 3]);
    }
}
