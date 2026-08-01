use super::pairing::{
    build_pairing_response, encode_pairing_response, parse_pairing_request, wait_pairing_decision,
    ActivePairing, PairingHub, PairingResponse, PendingRequest,
};
use super::peers::{constant_time_equal, new_peer_id, PeerStore};
use super::sessions::{encrypt_write, parse_store_control, SessionRegistry};
use crate::noise::{self, Identity, Session};
use crate::protocol::{self, Frame};
use crate::store::Local;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

pub struct PeerServer {
    pub store: Arc<Local>,
    pub hub: Arc<PairingHub>,
    pub peers: Arc<PeerStore>,
    pub sessions: Arc<SessionRegistry>,
    pub identity: Arc<Identity>,
    pub device_name: String,
    pub local_endpoint: String,
    pub channel_endpoint: String,
}

impl PeerServer {
    pub async fn handle_socket(self: Arc<Self>, mut socket: WebSocket) {
        let first = match socket.next().await {
            Some(Ok(Message::Text(t))) => t,
            _ => return,
        };
        let frame = match noise::decode_frame(&first) {
            Ok(f) if f.frame_type == noise::FrameType::Hs => f,
            _ => {
                let _ = socket
                    .send(Message::Text(r#"{"v":2,"t":"err","p":""}"#.into()))
                    .await;
                return;
            }
        };

        let mut sess = match Session::new_responder(&self.identity) {
            Ok(s) => s,
            Err(e) => {
                warn!("noise responder: {e}");
                return;
            }
        };
        let (payload, peer_pub) = match sess.read_handshake1(&frame.payload) {
            Ok(v) => v,
            Err(e) => {
                warn!("hs1: {e}");
                return;
            }
        };
        let fp = super::pairing::fingerprint_from_key(&peer_pub);
        let code = self.hub.code();

        if !code.is_empty() {
            if let Ok(req) = parse_pairing_request(&payload) {
                if !req.pairing_code.is_empty() {
                    self.handle_pairing(socket, sess, peer_pub, fp, req, code)
                        .await;
                    return;
                }
            }
        }
        self.handle_reconnect(socket, sess, fp, &payload).await;
    }

    async fn handle_pairing(
        self: Arc<Self>,
        mut socket: WebSocket,
        mut sess: Session,
        peer_pub: [u8; 32],
        fp: String,
        req: super::pairing::PairingRequest,
        code: String,
    ) {
        if !constant_time_equal(&req.pairing_code, &code) {
            let resp = PairingResponse {
                accepted: false,
                device_name: self.device_name.clone(),
                device_id: self.identity.fingerprint(),
                peer_id: String::new(),
                channel_endpoint: None,
                local_endpoint: None,
                reject_reason: Some("Invalid pairing code".into()),
            };
            if let Ok(b) = encode_pairing_response(&resp) {
                if let Ok(msg2) = sess.write_handshake2(&b) {
                    if let Ok(out) = noise::encode_frame(&noise::Frame {
                        frame_type: noise::FrameType::Hs,
                        payload: msg2,
                    }) {
                        let _ = socket.send(Message::Text(out.into())).await;
                    }
                }
            }
            return;
        }

        let session_id = new_peer_id();
        let rx = self.hub.begin_accept_wait();
        self.hub.set_pending(
            PendingRequest {
                device_name: req.device_name.clone(),
                device_id: req.device_id.clone(),
                fingerprint: fp.clone(),
                session_id: session_id.clone(),
            },
            ActivePairing {
                request: req.clone(),
                session_id,
            },
        );

        let accept = wait_pairing_decision(rx).await;
        self.hub.clear_pairing();

        let peer_id = if accept { new_peer_id() } else { String::new() };
        let resp = build_pairing_response(
            &self.hub,
            accept,
            &peer_id,
            &self.local_endpoint,
            &self.channel_endpoint,
        );
        let b = encode_pairing_response(&resp).unwrap_or_default();
        let msg2 = match sess.write_handshake2(&b) {
            Ok(m) => m,
            Err(e) => {
                warn!("hs2: {e}");
                return;
            }
        };
        let out = noise::encode_frame(&noise::Frame {
            frame_type: noise::FrameType::Hs,
            payload: msg2,
        })
        .unwrap_or_default();
        if socket.send(Message::Text(out.into())).await.is_err() {
            return;
        }
        if !accept {
            return;
        }
        let _ = super::pairing::save_paired_peer(&self.peers, &fp, &peer_pub, &req, &peer_id);
        self.serve_transport(socket, sess, fp).await;
    }

    async fn handle_reconnect(
        self: Arc<Self>,
        mut socket: WebSocket,
        mut sess: Session,
        fp: String,
        payload: &[u8],
    ) {
        let known = self.peers.get(&fp).ok().flatten();
        if known.is_none() {
            warn!("peer ws: reconnect rejected unknown fp={fp}");
            let ack = json!({
                "type": "reconnect_nack",
                "device_id": self.identity.fingerprint(),
                "reason": "unknown_peer",
            });
            if let Ok(b) = serde_json::to_vec(&ack) {
                if let Ok(msg2) = sess.write_handshake2(&b) {
                    if let Ok(out) = noise::encode_frame(&noise::Frame {
                        frame_type: noise::FrameType::Hs,
                        payload: msg2,
                    }) {
                        let _ = socket.send(Message::Text(out.into())).await;
                    }
                }
            }
            return;
        }

        if let Ok(req) = serde_json::from_slice::<Map<String, Value>>(payload) {
            let local = req.get("local_endpoint").and_then(|v| v.as_str()).unwrap_or("");
            let channel = req
                .get("channel_endpoint")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !local.is_empty() || !channel.is_empty() {
                let _ = self.peers.merge_endpoints(&fp, local, channel);
            }
        }

        let mut ack = Map::new();
        ack.insert("type".into(), json!("reconnect_ack"));
        ack.insert("device_id".into(), json!(self.identity.fingerprint()));
        if !self.local_endpoint.is_empty() {
            ack.insert("local_endpoint".into(), json!(self.local_endpoint));
        }
        if !self.channel_endpoint.is_empty() {
            ack.insert("channel_endpoint".into(), json!(self.channel_endpoint));
        }
        let ack_bytes = serde_json::to_vec(&ack).unwrap_or_default();
        let msg2 = match sess.write_handshake2(&ack_bytes) {
            Ok(m) => m,
            Err(e) => {
                warn!("reconnect hs2: {e}");
                return;
            }
        };
        let out = noise::encode_frame(&noise::Frame {
            frame_type: noise::FrameType::Hs,
            payload: msg2,
        })
        .unwrap_or_default();
        if socket.send(Message::Text(out.into())).await.is_err() {
            return;
        }
        self.serve_transport(socket, sess, fp).await;
    }

    async fn serve_transport(self: Arc<Self>, socket: WebSocket, sess: Session, fp: String) {
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel();
        let session_id = self.sessions.register(fp.clone(), sess, out_tx);
        let (mut sink, mut stream) = socket.split();

        loop {
            tokio::select! {
                msg = stream.next() => {
                    let msg = match msg {
                        Some(Ok(Message::Text(t))) => t,
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                        _ => continue,
                    };
            let fr = match noise::decode_frame(&msg) {
                Ok(f) if f.frame_type == noise::FrameType::Data => f,
                _ => continue,
            };
            let ls = match self.sessions.get(&fp) {
                Some(ls) => ls,
                None => break,
            };
            let plain = {
                let mut sess = ls.sess.lock().unwrap();
                match sess.decrypt(&fr.payload) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("decrypt: {e}");
                        break;
                    }
                }
            };
            let raw: Map<String, Value> = match serde_json::from_slice(&plain) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if raw.get("type").and_then(|v| v.as_str()) == Some("ping") {
                let pong = json!({
                    "type": "pong",
                    "timestamp": now_ms(),
                });
                if let Ok(b) = serde_json::to_vec(&pong) {
                    if let Ok(msg) = encrypt_write(&ls, &b) {
                        let _ = sink.send(msg).await;
                    }
                }
                continue;
            }
            let op = raw.get("op").and_then(|v| v.as_str()).unwrap_or("");
            let req_id = raw.get("req_id").and_then(|v| v.as_str()).unwrap_or("");
            if (op == "result" || op == "error") && !req_id.is_empty() {
                if self.sessions.deliver_reply(&fp, req_id, raw.clone()) {
                    continue;
                }
            }
            let Some((op, req_id, payload)) = parse_store_control(&plain) else {
                continue;
            };
            if req_id.is_empty() {
                match op.as_str() {
                    "master.pointer" => {
                        let _ = self.store.handle(
                            Frame::from_parts(op, payload),
                            &fp,
                            protocol::TRUST_OWNER,
                            false,
                        );
                    }
                    "import.grant" => {
                        if let Err(e) = self.store.receive_pushed_grant(&fp, &payload) {
                            warn!("import.grant push from {fp}: {e}");
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if let Ok(reply) = self.build_store_reply(&fp, &op, &req_id, payload) {
                if let Ok(msg) = encrypt_write(&ls, &reply) {
                    let _ = sink.send(msg).await;
                }
            }
                }
                Some(out) = out_rx.recv() => {
                    let _ = sink.send(out).await;
                }
            }
        }
        self.sessions.remove(&fp, session_id);
    }

    fn build_store_reply(
        &self,
        fp: &str,
        op: &str,
        req_id: &str,
        payload: Map<String, Value>,
    ) -> Result<Vec<u8>, String> {
        let trust = self
            .peers
            .get(fp)
            .ok()
            .flatten()
            .map(|p| p.trust_level)
            .unwrap_or_else(|| protocol::TRUST_OWNER.to_string());
        let result = self
            .store
            .handle(Frame::from_parts(op, payload), fp, &trust, false);
        let reply = match result {
            Ok(data) => {
                let mut m = Map::new();
                m.insert("type".into(), json!("store"));
                m.insert("ns".into(), json!("store"));
                m.insert("op".into(), json!("result"));
                m.insert("v".into(), json!(1));
                m.insert("data".into(), Value::Object(data));
                if !req_id.is_empty() {
                    m.insert("req_id".into(), json!(req_id));
                }
                m
            }
            Err(e) => {
                let mut m = Map::new();
                m.insert("type".into(), json!("store"));
                m.insert("ns".into(), json!("store"));
                m.insert("op".into(), json!("error"));
                m.insert("v".into(), json!(1));
                m.insert("code".into(), json!(e.code));
                m.insert("message".into(), json!(e.msg));
                if !req_id.is_empty() {
                    m.insert("req_id".into(), json!(req_id));
                }
                m
            }
        };
        serde_json::to_vec(&reply).map_err(|e| e.to_string())
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
