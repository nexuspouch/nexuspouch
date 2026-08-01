use crate::store::PeerEnsure;
use super::peers::{Peer, PeerStore};
use super::sessions::{parse_store_control, SessionRegistry};
use crate::noise::{self, Identity, Session};
use axum::extract::ws::Message;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tungstenite::{stream::MaybeTlsStream, Message as WsMessage};
use tracing::warn;

const DIAL_TIMEOUT: Duration = Duration::from_secs(12);

type WsConn = tungstenite::WebSocket<MaybeTlsStream<TcpStream>>;

pub struct Dialer {
    identity: Arc<Identity>,
    peers: Arc<PeerStore>,
    sessions: Arc<SessionRegistry>,
    local_endpoint: String,
    channel_endpoint: String,
    dialed: Arc<Mutex<std::collections::HashMap<String, Arc<Mutex<WsConn>>>>>,
}

impl Dialer {
    pub fn new(
        identity: Arc<Identity>,
        peers: Arc<PeerStore>,
        sessions: Arc<SessionRegistry>,
        local_endpoint: String,
        channel_endpoint: String,
    ) -> Self {
        Self {
            identity,
            peers,
            sessions,
            local_endpoint,
            channel_endpoint,
            dialed: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub fn ensure(&self, device_id: &str) -> Result<(), String> {
        if self.sessions.has(device_id) {
            return Ok(());
        }
        let p = self
            .peers
            .get(device_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("peer not paired: {device_id}"))?;
        let mut endpoints = Vec::new();
        if let Some(ep) = p.local_endpoint.as_deref() {
            let ep = ep.trim();
            if !ep.is_empty() {
                endpoints.push(ep.to_string());
            }
        }
        if let Some(ep) = p.channel_endpoint.as_deref() {
            let ep = ep.trim();
            if !ep.is_empty() {
                endpoints.push(ep.to_string());
            }
        }
        if endpoints.is_empty() {
            return Err(format!("no endpoints for {device_id}"));
        }
        let mut last_err = String::from("dial failed");
        for ep in endpoints {
            match self.dial_one(device_id, &p, &ep) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    warn!("dial {device_id} via {ep}: {e}");
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    pub fn release(&self, device_id: &str) {
        let conn = {
            let mut g = self.dialed.lock().unwrap();
            g.remove(device_id)
        };
        if let Some(conn) = conn {
            let _ = conn.lock().unwrap().close(None);
        }
    }

    fn dial_one(&self, device_id: &str, p: &Peer, endpoint: &str) -> Result<(), String> {
        let pub_bytes = STANDARD
            .decode(&p.public_key_b64)
            .map_err(|_| "bad peer public key".to_string())?;
        if pub_bytes.len() != 32 {
            return Err("bad peer public key".into());
        }
        let mut peer_pub = [0u8; 32];
        peer_pub.copy_from_slice(&pub_bytes);

        let (mut ws, _) = tungstenite::connect(endpoint).map_err(|e| e.to_string())?;

        let mut init_sess = Session::new_initiator(&self.identity, &peer_pub)?;
        let mut body = Map::new();
        body.insert("type".into(), json!("reconnect"));
        body.insert("device_id".into(), json!(self.identity.fingerprint()));
        if !self.local_endpoint.is_empty() {
            body.insert("local_endpoint".into(), json!(self.local_endpoint));
        }
        if !self.channel_endpoint.is_empty() {
            body.insert("channel_endpoint".into(), json!(self.channel_endpoint));
        }
        let payload = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
        let msg1 = init_sess.write_handshake1(&payload)?;
        let frame1 = noise::encode_frame(&noise::Frame {
            frame_type: noise::FrameType::Hs,
            payload: msg1,
        })?;
        ws.send(WsMessage::Text(frame1))
            .map_err(|e| e.to_string())?;

        let raw2 = match ws.read() {
            Ok(WsMessage::Text(t)) => t,
            Ok(_) => return Err("expected text frame".into()),
            Err(e) => return Err(e.to_string()),
        };
        let fr2 = noise::decode_frame(&raw2)?;
        if fr2.frame_type != noise::FrameType::Hs {
            return Err("expected hs frame".into());
        }
        let ack_payload = init_sess.read_handshake2(&fr2.payload)?;
        let ack: Map<String, Value> =
            serde_json::from_slice(&ack_payload).unwrap_or_default();
        if ack.get("type").and_then(|v| v.as_str()) == Some("reconnect_nack") {
            let reason = ack
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(format!("reconnect_nack: {reason}"));
        }
        if ack.get("type").and_then(|v| v.as_str()) != Some("reconnect_ack") {
            return Err(format!("unexpected ack type {:?}", ack.get("type")));
        }

        let _ = DIAL_TIMEOUT; // handshake bounded by connect + read above

        let ws = Arc::new(Mutex::new(ws));
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
        let session_id = self
            .sessions
            .register(device_id.to_string(), init_sess, out_tx);
        {
            let mut g = self.dialed.lock().unwrap();
            if let Some(old) = g.insert(device_id.to_string(), Arc::clone(&ws)) {
                let _ = old.lock().unwrap().close(None);
            }
        }

        let ws_writer = Arc::clone(&ws);
        std::thread::spawn(move || {
            while let Some(msg) = out_rx.blocking_recv() {
                if let Message::Text(t) = msg {
                    if ws_writer
                        .lock()
                        .unwrap()
                        .send(WsMessage::Text(t.to_string()))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });

        let sessions = Arc::clone(&self.sessions);
        let dialed = Arc::clone(&self.dialed);
        let fp = device_id.to_string();
        std::thread::spawn(move || {
            serve_outbound(sessions, dialed, ws, fp, session_id);
        });

        Ok(())
    }
}

impl PeerEnsure for Dialer {
    fn ensure(&self, device_id: &str) -> Result<(), String> {
        Dialer::ensure(self, device_id)
    }

    fn release(&self, device_id: &str) {
        Dialer::release(self, device_id)
    }
}

fn serve_outbound(
    sessions: Arc<SessionRegistry>,
    dialed: Arc<Mutex<std::collections::HashMap<String, Arc<Mutex<WsConn>>>>>,
    ws: Arc<Mutex<WsConn>>,
    fp: String,
    session_id: u64,
) {
    let _guard = OutboundGuard {
        sessions: Arc::clone(&sessions),
        dialed,
        fp: fp.clone(),
        session_id,
        ws: Arc::clone(&ws),
    };
    loop {
        let msg = match ws.lock().unwrap().read() {
            Ok(WsMessage::Text(t)) => t,
            Ok(WsMessage::Close(_)) | Ok(_) => break,
            Err(_) => break,
        };
        let fr = match noise::decode_frame(&msg) {
            Ok(f) if f.frame_type == noise::FrameType::Data => f,
            _ => continue,
        };
        let ls = match sessions.get(&fp) {
            Some(ls) => ls,
            None => break,
        };
        let plain = {
            let mut sess = ls.sess.lock().unwrap();
            match sess.decrypt(&fr.payload) {
                Ok(p) => p,
                Err(e) => {
                    warn!("outbound decrypt {fp}: {e}");
                    break;
                }
            }
        };
        let raw: Map<String, Value> = match serde_json::from_slice(&plain) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let op = raw.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let req_id = raw
            .get("req_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if (op == "result" || op == "error") && !req_id.is_empty() {
            let _ = sessions.deliver_reply(&fp, &req_id, raw);
        } else if parse_store_control(&plain).is_some() {
            // Outbound dial is RPC-only.
        }
    }
}

struct OutboundGuard {
    sessions: Arc<SessionRegistry>,
    dialed: Arc<Mutex<std::collections::HashMap<String, Arc<Mutex<WsConn>>>>>,
    fp: String,
    session_id: u64,
    ws: Arc<Mutex<WsConn>>,
}

impl Drop for OutboundGuard {
    fn drop(&mut self) {
        self.sessions.remove(&self.fp, self.session_id);
        let mut g = self.dialed.lock().unwrap();
        if g.get(&self.fp).map(|c| Arc::ptr_eq(c, &self.ws)).unwrap_or(false) {
            g.remove(&self.fp);
        }
        let _ = self.ws.lock().unwrap().close(None);
    }
}
