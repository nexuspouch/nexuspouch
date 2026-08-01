pub mod diag;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;
use std::collections::HashMap;
use std::net::{IpAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;

pub const SERVICE_TYPE: &str = "_nexuspouch._tcp.local.";

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredPeer {
    pub name: String,
    pub fingerprint: String,
    pub host: String,
    pub port: u16,
    pub endpoint: String,
    pub same_host: bool,
}

pub struct Discovery {
    daemon: ServiceDaemon,
    fullname: Option<String>,
    fingerprint: String,
    advertising: bool,
}

impl Discovery {
    /// Start mDNS daemon; register service unless `advertise` is false.
    pub fn start(
        device_name: &str,
        fingerprint: &str,
        port: u16,
        advertise: bool,
    ) -> Option<Arc<Self>> {
        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                warn!("mdns: failed to create daemon: {e}");
                return None;
            }
        };

        let mut fullname = None;
        let mut advertising = false;

        if advertise {
            let instance = sanitize_instance_name(device_name);
            let properties = [
                ("fp", fingerprint),
                ("name", device_name),
                ("path", "/peer/ws"),
                ("proto", &crate::protocol::PROTOCOL_VERSION.to_string()),
            ];
            match ServiceInfo::new(
                SERVICE_TYPE,
                &instance,
                "nexuspouch.local.",
                "",
                port,
                &properties[..],
            ) {
                Ok(info) => {
                    let info = info.enable_addr_auto();
                    match daemon.register(info.clone()) {
                        Ok(()) => {
                            fullname = Some(info.get_fullname().to_string());
                            advertising = true;
                        }
                        Err(e) => warn!("mdns: register failed: {e}"),
                    }
                }
                Err(e) => warn!("mdns: service info failed: {e}"),
            }
        }

        Some(Arc::new(Self {
            daemon,
            fullname,
            fingerprint: fingerprint.to_string(),
            advertising,
        }))
    }

    pub fn is_advertising(&self) -> bool {
        self.advertising
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn browse(&self, timeout: Duration, exclude_fp: Option<&str>) -> Vec<DiscoveredPeer> {
        let receiver = match self.daemon.browse(SERVICE_TYPE) {
            Ok(r) => r,
            Err(e) => {
                warn!("mdns: browse failed: {e}");
                return Vec::new();
            }
        };

        let local_ips = local_interface_ips();
        let deadline = Instant::now() + timeout;
        let mut peers: HashMap<String, DiscoveredPeer> = HashMap::new();

        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let wait = remaining.min(Duration::from_millis(200));
            match receiver.recv_timeout(wait) {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    let fp = info
                        .get_property_val_str("fp")
                        .unwrap_or("")
                        .to_string();
                    if exclude_fp.is_some_and(|want| want == fp) {
                        continue;
                    }
                    let name = info
                        .get_property_val_str("name")
                        .or_else(|| info.get_property_val_str("instance"))
                        .unwrap_or(info.get_fullname())
                        .to_string();
                    let path = info
                        .get_property_val_str("path")
                        .unwrap_or("/peer/ws");
                    let port = info.get_port();
                    let (host, endpoint) = peer_host_endpoint(&info, path, port);
                    let same_host = info
                        .get_addresses()
                        .iter()
                        .map(|ip| ip.to_ip_addr())
                        .any(|ip| local_ips.contains(&ip));
                    peers.insert(
                        info.get_fullname().to_string(),
                        DiscoveredPeer {
                            name,
                            fingerprint: fp,
                            host,
                            port,
                            endpoint,
                            same_host,
                        },
                    );
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }

        let mut out: Vec<_> = peers.into_values().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        if let Some(ref fullname) = self.fullname {
            let _ = self.daemon.unregister(fullname);
        }
    }
}

fn peer_host_endpoint(
    info: &mdns_sd::ResolvedService,
    path: &str,
    port: u16,
) -> (String, String) {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    if let Some(scoped) = info.get_addresses().iter().next() {
        let ip = scoped.to_ip_addr();
        let host = ip.to_string();
        let bracketed = match ip {
            IpAddr::V6(_) => format!("[{host}]"),
            IpAddr::V4(_) => host.clone(),
        };
        return (host, format!("ws://{bracketed}:{port}{path}"));
    }
    let hostname = info.get_hostname().trim_end_matches('.').to_string();
    (hostname.clone(), format!("ws://{hostname}:{port}{path}"))
}

fn local_interface_ips() -> Vec<IpAddr> {
    let mut ips = Vec::new();
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                ips.push(addr.ip());
            }
        }
    }
    ips.push(IpAddr::from([127, 0, 0, 1]));
    ips.push(IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1]));
    ips
}

/// Sanitize a device display name for DNS-SD instance labels.
pub fn sanitize_instance_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        let ok = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_';
        let mapped = if ch == ' ' {
            '-'
        } else if ok {
            ch
        } else {
            '-'
        };
        if mapped == '-' && out.ends_with('-') {
            continue;
        }
        out.push(mapped);
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "nexuspouch".to_string()
    } else if out.len() > 63 {
        out[..63].trim_end_matches('-').to_string()
    } else {
        out
    }
}

/// Parse the TCP listen port from a bind address string (`:8787`, `0.0.0.0:8787`, `[::]:8787`).
pub fn parse_listen_port(listen: &str) -> u16 {
    let listen = listen.trim();
    if let Some(rest) = listen.strip_prefix(':') {
        return rest.parse().unwrap_or(8787);
    }
    if listen.starts_with('[') {
        if let Some((_, port)) = listen.rsplit_once(':') {
            return port.parse().unwrap_or(8787);
        }
        return 8787;
    }
    listen
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse().ok())
        .unwrap_or(8787)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_listen_port_variants() {
        assert_eq!(parse_listen_port(":8787"), 8787);
        assert_eq!(parse_listen_port("0.0.0.0:8787"), 8787);
        assert_eq!(parse_listen_port("[::]:9090"), 9090);
        assert_eq!(parse_listen_port("127.0.0.1:3000"), 3000);
        assert_eq!(parse_listen_port("bad"), 8787);
    }

    #[test]
    fn sanitize_instance_name_strips_invalid() {
        assert_eq!(sanitize_instance_name("My Device!"), "My-Device");
        assert_eq!(sanitize_instance_name("  "), "nexuspouch");
        assert_eq!(sanitize_instance_name("ok_name-1"), "ok_name-1");
    }
}
