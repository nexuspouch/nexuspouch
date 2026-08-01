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
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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

pub fn check_lan(
    discovery: Option<&Discovery>,
    exclude_fp: Option<&str>,
) -> LanCheck {
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
            latency_ms: None,
            error: None,
        };
    }

    let (host, port) = match parse_channel_host_port(endpoint) {
        Ok(v) => v,
        Err(e) => {
            return ChannelCheck {
                configured: true,
                ok: false,
                endpoint: Some(endpoint.to_string()),
                latency_ms: None,
                error: Some(e),
            };
        }
    };

    let started = Instant::now();
    match tcp_connect(&host, port, CHANNEL_TIMEOUT) {
        Ok(()) => ChannelCheck {
            configured: true,
            ok: true,
            endpoint: Some(endpoint.to_string()),
            latency_ms: Some(started.elapsed().as_millis() as u64),
            error: None,
        },
        Err(e) => ChannelCheck {
            configured: true,
            ok: false,
            endpoint: Some(endpoint.to_string()),
            latency_ms: None,
            error: Some(e),
        },
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

/// Parse ws/wss/http(s) channel URL into `(host, port)`.
pub fn parse_channel_host_port(endpoint: &str) -> Result<(String, u16), String> {
    let endpoint = endpoint.trim();
    let url = Url::parse(endpoint).map_err(|e| e.to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "missing host".to_string())?
        .to_string();
    let port = url.port().unwrap_or_else(|| match url.scheme() {
        "https" | "wss" => 443,
        "http" | "ws" => 80,
        _ => 80,
    });
    Ok((host, port))
}

fn tcp_connect(host: &str, port: u16, timeout: Duration) -> Result<(), String> {
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .collect();
    if addrs.is_empty() {
        return Err("no addresses resolved".into());
    }
    for addr in addrs {
        if TcpStream::connect_timeout(&addr, timeout).is_ok() {
            return Ok(());
        }
    }
    Err(format!("connect timeout to {host}:{port}"))
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
    }
}
