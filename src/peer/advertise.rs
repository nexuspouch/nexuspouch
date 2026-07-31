use std::net::{IpAddr, UdpSocket};

pub fn advertise_local_ws(listen: &str) -> String {
    let (host, port) = match parse_host_port(listen) {
        Some(v) => v,
        None => return "ws://127.0.0.1:8787/peer/ws".to_string(),
    };
    let host = if host.is_empty() || host == "0.0.0.0" || host == "::" {
        first_non_loopback_ipv4().unwrap_or_else(|| "127.0.0.1".to_string())
    } else {
        host
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    format!("ws://{host}:{port}/peer/ws")
}

fn parse_host_port(listen: &str) -> Option<(String, String)> {
    if let Some(rest) = listen.strip_prefix(':') {
        return Some((String::new(), rest.to_string()));
    }
    if let Some((h, p)) = listen.rsplit_once(':') {
        return Some((h.to_string(), p.to_string()));
    }
    None
}

fn first_non_loopback_ipv4() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    match addr.ip() {
        IpAddr::V4(v4) if !v4.is_loopback() => Some(v4.to_string()),
        _ => None,
    }
}
