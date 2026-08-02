use super::peers::{now_ms, Peer, PeerStore};
use crate::noise::{Identity};
use crate::protocol;
use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use url::form_urlencoded;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequest {
    pub pairing_code: String,
    pub device_name: String,
    pub device_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_endpoint: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingResponse {
    pub accepted: bool,
    pub device_name: String,
    pub device_id: String,
    pub peer_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reject_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingRequest {
    pub device_name: String,
    pub device_id: String,
    pub fingerprint: String,
    pub session_id: String,
}

pub(crate) struct ActivePairing {
    #[allow(dead_code)]
    pub(crate) request: PairingRequest,
    #[allow(dead_code)]
    pub(crate) session_id: String,
}

pub struct PairingHub {
    identity: Arc<Identity>,
    peers: Arc<PeerStore>,
    device_name: String,
    inner: Mutex<HubState>,
}

struct HubState {
    code: String,
    pending: Option<PendingRequest>,
    active: Option<ActivePairing>,
    wait_accept: Option<std::sync::mpsc::Sender<bool>>,
}

impl PairingHub {
    pub fn new(identity: Arc<Identity>, peers: Arc<PeerStore>, device_name: impl Into<String>) -> Self {
        Self {
            identity,
            peers,
            device_name: device_name.into(),
            inner: Mutex::new(HubState {
                code: String::new(),
                pending: None,
                active: None,
                wait_accept: None,
            }),
        }
    }

    pub fn start(&self) -> Result<String, std::io::Error> {
        let code = super::peers::generate_pairing_code()?;
        let mut g = self.inner.lock().unwrap();
        g.code = code.clone();
        g.pending = None;
        g.active = None;
        g.wait_accept = None;
        Ok(code)
    }

    pub fn code(&self) -> String {
        self.inner.lock().unwrap().code.clone()
    }

    pub fn pending(&self) -> Option<PendingRequest> {
        self.inner.lock().unwrap().pending.clone()
    }

    pub fn decide(&self, accept: bool) -> Result<(), String> {
        let ch = self.inner.lock().unwrap().wait_accept.clone();
        let Some(ch) = ch else {
            return Err("no pending pairing".into());
        };
        ch.send(accept).map_err(|_| "decision already sent".into())
    }

    pub(crate) fn begin_accept_wait(&self) -> std::sync::mpsc::Receiver<bool> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut g = self.inner.lock().unwrap();
        g.wait_accept = Some(tx);
        rx
    }

    pub(crate) fn set_pending(&self, pending: PendingRequest, active: ActivePairing) {
        let mut g = self.inner.lock().unwrap();
        g.pending = Some(pending);
        g.active = Some(active);
    }

    pub(crate) fn clear_pairing(&self) {
        let mut g = self.inner.lock().unwrap();
        g.pending = None;
        g.active = None;
        g.wait_accept = None;
        g.code.clear();
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn peers(&self) -> &PeerStore {
        &self.peers
    }
}

pub fn encode_qr(
    local_endpoint: &str,
    channel_endpoint: &str,
    code: &str,
    fingerprint: &str,
    public_key: &[u8; 32],
) -> String {
    let pk = URL_SAFE_NO_PAD.encode(public_key);
    let mut ser = form_urlencoded::Serializer::new(String::new());
    if !local_endpoint.is_empty() {
        ser.append_pair("local", local_endpoint);
    }
    if !channel_endpoint.is_empty() {
        ser.append_pair("channel", channel_endpoint);
    }
    ser.append_pair("code", code);
    format!("shepaw://peer?{}#fp={fingerprint}&pk={pk}", ser.finish())
}

pub fn encode_qr_svg(payload: &str) -> Result<String, String> {
    use qrcode::render::svg;
    use qrcode::QrCode;
    let code = QrCode::new(payload.as_bytes()).map_err(|e| e.to_string())?;
    let svg = code
        .render::<svg::Color>()
        .min_dimensions(220, 220)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    // Drop XML prologue so the fragment can be injected via innerHTML.
    Ok(svg
        .trim_start()
        .strip_prefix("<?xml version=\"1.0\" standalone=\"yes\"?>")
        .unwrap_or(&svg)
        .trim_start()
        .to_string())
}

pub fn fingerprint_from_key(pub_key: &[u8; 32]) -> String {
    let sum = Sha256::digest(pub_key);
    hex::encode(&sum[..8])
}

pub fn parse_pairing_request(b: &[u8]) -> Result<PairingRequest, serde_json::Error> {
    serde_json::from_slice(b)
}

pub fn encode_pairing_response(r: &PairingResponse) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(r)
}

pub async fn wait_pairing_decision(rx: std::sync::mpsc::Receiver<bool>) -> bool {
    tokio::task::spawn_blocking(move || {
        rx.recv_timeout(Duration::from_secs(120))
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

pub fn build_pairing_response(
    hub: &PairingHub,
    accept: bool,
    peer_id: &str,
    local_endpoint: &str,
    channel_endpoint: &str,
) -> PairingResponse {
    let mut resp = PairingResponse {
        accepted: accept,
        device_name: hub.device_name().to_string(),
        device_id: hub.identity().fingerprint(),
        peer_id: if accept { peer_id.to_string() } else { String::new() },
        channel_endpoint: None,
        local_endpoint: None,
        reject_reason: None,
    };
    if accept {
        if !local_endpoint.is_empty() {
            resp.local_endpoint = Some(local_endpoint.to_string());
        }
        if !channel_endpoint.is_empty() {
            resp.channel_endpoint = Some(channel_endpoint.to_string());
        }
    } else {
        resp.reject_reason = Some("rejected".into());
    }
    resp
}

pub fn save_paired_peer(
    peers: &PeerStore,
    fp: &str,
    peer_pub: &[u8; 32],
    req: &PairingRequest,
    peer_id: &str,
) -> Result<(), std::io::Error> {
    peers.upsert(Peer {
        fingerprint: fp.to_string(),
        public_key_b64: STANDARD.encode(peer_pub),
        device_name: req.device_name.clone(),
        peer_id: peer_id.to_string(),
        trust_level: protocol::TRUST_OWNER.to_string(),
        paired_at_ms: now_ms(),
        local_endpoint: req.local_endpoint.clone(),
        channel_endpoint: req.channel_endpoint.clone(),
    })
}
