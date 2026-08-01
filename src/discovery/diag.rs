use super::{parse_listen_port, Discovery, DiscoveredPeer};
use serde::Serialize;
use serde_json::{Map, Value};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};
use url::Url;

const LOCAL_TIMEOUT: Duration = Duration::from_secs(2);
const LAN_BROWSE_TIMEOUT: Duration = Duration::from_millis(1500);
const CHANNEL_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize)]
pub struct LocalCheck {
    pub ok: bool,
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LanCheck {
    pub ok: bool,
    pub discovered: usize,
    pub peers: Vec<DiscoveredPeer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelCheck {
    pub configured: bool,
    /// Overall: DNS + TCP + WS upgrade all succeeded (when configured).
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<Vec<String>>,
    pub dns_ok: bool,
    pub tcp_ok: bool,
    pub ws_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Nexuspouch does not host a relay; Channel is dial-out config only.
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsReport {
    pub local: LocalCheck,
    pub lan: LanCheck,
    pub channel: ChannelCheck,
    pub mdns_advertising: bool,
}

impl DiagnosticsReport {
    pub fn to_map(&self) -> Map<String, Value> {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    }
}

fn channel_note() -> String {
    "Channel is config-only (Nexuspouch dials out; it does not host a relay).".into()
}

pub fn check_local_health(listen_port: u16) -> LocalCheck {
    let endpoint = format!("http://127.0.0.1:{listen_port}/health");
    let agent = ureq::AgentBuilder::new().timeout(LOCAL_TIMEOUT).build();
    let started = Instant::now();
    match agent.get(&endpoint).call() {
        Ok(resp) if resp.status() == 200 => LocalCheck {
            ok: true,
            endpoint,
            latency_ms: Some(started.elapsed().as_millis() as u64),
            error: None,
        },
        Ok(resp) => LocalCheck {
            ok: false,
            endpoint,
            latency_ms: Some(started.elapsed().as_millis() as u64),
            error: Some(format!("HTTP {}", resp.status())),
        },
        Err(e) => LocalCheck {
            ok: false,
            endpoint,
            latency_ms: None,
            error: Some(e.to_string()),
        },
    }
}

pub fn check_lan(discovery: Option<&Discovery>, exclude_fp: Option<&str>) -> LanCheck {
    let Some(discovery) = discovery else {
        return LanCheck {
            ok: false,
            discovered: 0,
            peers: Vec::new(),
            error: Some("mDNS unavailable".into()),
        };
    };
    let peers = discovery.browse(LAN_BROWSE_TIMEOUT, exclude_fp);
    LanCheck {
        ok: true,
        discovered: peers.len(),
        peers,
        error: None,
    }
}

pub fn check_channel(endpoint: &str) -> ChannelCheck {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return ChannelCheck {
            configured: false,
            ok: true,
            endpoint: None,
            scheme: None,
            host: None,
            port: None,
            resolved: None,
            dns_ok: false,
            tcp_ok: false,
            ws_ok: false,
            ws_status: None,
            latency_ms: None,
            error: None,
            note: channel_note(),
        };
    }

    let parsed = match parse_channel_url(endpoint) {
        Ok(v) => v,
        Err(e) => {
            return ChannelCheck {
                configured: true,
                ok: false,
                endpoint: Some(endpoint.to_string()),
                scheme: None,
                host: None,
                port: None,
                resolved: None,
                dns_ok: false,
                tcp_ok: false,
                ws_ok: false,
                ws_status: None,
                latency_ms: None,
                error: Some(e),
                note: channel_note(),
            };
        }
    };

    let started = Instant::now();
    let addrs = match (parsed.host.as_str(), parsed.port).to_socket_addrs() {
        Ok(iter) => iter.collect::<Vec<_>>(),
        Err(e) => {
            return ChannelCheck {
                configured: true,
                ok: false,
                endpoint: Some(endpoint.to_string()),
                scheme: Some(parsed.scheme.clone()),
                host: Some(parsed.host.clone()),
                port: Some(parsed.port),
                resolved: None,
                dns_ok: false,
                tcp_ok: false,
                ws_ok: false,
                ws_status: None,
                latency_ms: None,
                error: Some(format!("dns: {e}")),
                note: channel_note(),
            };
        }
    };
    if addrs.is_empty() {
        return ChannelCheck {
            configured: true,
            ok: false,
            endpoint: Some(endpoint.to_string()),
            scheme: Some(parsed.scheme.clone()),
            host: Some(parsed.host.clone()),
            port: Some(parsed.port),
            resolved: Some(vec![]),
            dns_ok: false,
            tcp_ok: false,
            ws_ok: false,
            ws_status: None,
            latency_ms: None,
            error: Some("dns: no addresses resolved".into()),
            note: channel_note(),
        };
    }
    let resolved: Vec<String> = addrs.iter().map(|a| a.to_string()).collect();

    let tcp_ok = addrs
        .iter()
        .any(|addr| TcpStream::connect_timeout(addr, CHANNEL_TIMEOUT).is_ok());
    if !tcp_ok {
        return ChannelCheck {
            configured: true,
            ok: false,
            endpoint: Some(endpoint.to_string()),
            scheme: Some(parsed.scheme.clone()),
            host: Some(parsed.host.clone()),
            port: Some(parsed.port),
            resolved: Some(resolved),
            dns_ok: true,
            tcp_ok: false,
            ws_ok: false,
            ws_status: None,
            latency_ms: None,
            error: Some(format!(
                "tcp: connect timeout to {}:{}",
                parsed.host, parsed.port
            )),
            note: channel_note(),
        };
    }

    let (ws_ok, ws_status, ws_err) = probe_websocket(&parsed.ws_url);
    let latency_ms = Some(started.elapsed().as_millis() as u64);
    let ok = ws_ok;
    ChannelCheck {
        configured: true,
        ok,
        endpoint: Some(endpoint.to_string()),
        scheme: Some(parsed.scheme.clone()),
        host: Some(parsed.host.clone()),
        port: Some(parsed.port),
        resolved: Some(resolved),
        dns_ok: true,
        tcp_ok: true,
        ws_ok,
        ws_status,
        latency_ms,
        error: ws_err,
        note: channel_note(),
    }
}

pub struct ParsedChannel {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub ws_url: String,
}

/// Normalize channel URL and produce a ws/wss URL for upgrade probe.
pub fn parse_channel_url(endpoint: &str) -> Result<ParsedChannel, String> {
    let endpoint = endpoint.trim();
    let url = Url::parse(endpoint).map_err(|e| e.to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "missing host".to_string())?
        .to_string();
    let scheme = url.scheme().to_string();
    let port = url.port().unwrap_or_else(|| match url.scheme() {
        "https" | "wss" => 443,
        "http" | "ws" => 80,
        _ => 80,
    });
    let ws_scheme = match scheme.as_str() {
        "ws" | "http" => "ws",
        "wss" | "https" => "wss",
        other => return Err(format!("unsupported scheme: {other}")),
    };
    let mut ws = url.clone();
    let _ = ws.set_scheme(ws_scheme);
    Ok(ParsedChannel {
        scheme,
        host,
        port,
        ws_url: ws.to_string(),
    })
}

/// Parse ws/wss/http(s) channel URL into `(host, port)`.
pub fn parse_channel_host_port(endpoint: &str) -> Result<(String, u16), String> {
    let p = parse_channel_url(endpoint)?;
    Ok((p.host, p.port))
}

fn probe_websocket(ws_url: &str) -> (bool, Option<String>, Option<String>) {
    // Short connect; we only need the HTTP upgrade, then close.
    match tungstenite::connect(ws_url) {
        Ok((mut socket, resp)) => {
            let status = format!("{}", resp.status());
            let _ = socket.close(None);
            (true, Some(status), None)
        }
        Err(tungstenite::Error::Http(resp)) => {
            let code = resp.status().as_u16();
            (
                true,
                Some(format!("HTTP {code}")),
                Some(format!(
                    "ws handshake rejected (HTTP stack reachable): {code}"
                )),
            )
        }
        Err(e) => {
            let msg = e.to_string();
            (false, None, Some(format!("ws: {msg}")))
        }
    }
}

pub fn run_diagnostics(
    listen: &str,
    channel_endpoint: &str,
    discovery: Option<&Arc<Discovery>>,
    self_fp: &str,
) -> DiagnosticsReport {
    let listen_port = parse_listen_port(listen);
    let local = check_local_health(listen_port);
    let lan = check_lan(discovery.map(|d| d.as_ref()), Some(self_fp));
    let channel = check_channel(channel_endpoint);
    let mdns_advertising = discovery.is_some_and(|d| d.is_advertising());

    DiagnosticsReport {
        local,
        lan,
        channel,
        mdns_advertising,
    }
}

/// Offline-friendly channel-only check (no local/LAN).
pub fn run_channel_only(channel_endpoint: &str) -> ChannelCheck {
    check_channel(channel_endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_channel_urls() {
        assert_eq!(
            parse_channel_host_port("ws://example.com:9001/path").unwrap(),
            ("example.com".into(), 9001)
        );
        assert_eq!(
            parse_channel_host_port("wss://relay.example.com/peer").unwrap(),
            ("relay.example.com".into(), 443)
        );
        assert_eq!(
            parse_channel_host_port("http://127.0.0.1:8787").unwrap(),
            ("127.0.0.1".into(), 8787)
        );
        let p = parse_channel_url("https://relay.example.com/v1").unwrap();
        assert!(p.ws_url.starts_with("wss://"));
    }

    #[test]
    fn unconfigured_channel_is_ok() {
        let c = check_channel("");
        assert!(!c.configured);
        assert!(c.ok);
        assert!(!c.note.is_empty());
    }

    #[test]
    fn bad_scheme_fails() {
        let c = check_channel("ftp://example.com/x");
        assert!(c.configured);
        assert!(!c.ok);
        assert!(c.error.as_ref().unwrap().contains("scheme"));
    }
}
