use axum::http::HeaderMap;
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct AuthConfig {
    pub token: String,
}

impl AuthConfig {
    pub fn authorize_headers(&self, headers: &HeaderMap, query_token: Option<&str>) -> bool {
        if !self.token.is_empty() {
            let got = bearer_token(headers)
                .or_else(|| headers.get("x-admin-token").and_then(|v| v.to_str().ok()))
                .or(query_token)
                .unwrap_or("");
            return got.as_bytes().ct_eq(self.token.as_bytes()).into();
        }
        // Token empty: loopback-only is enforced at connection layer in main.
        true
    }
}

pub fn require_auth<F>(cfg: AuthConfig, f: F) -> F {
    let _ = cfg;
    f
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let h = headers.get("authorization")?.to_str().ok()?;
    let rest = h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer "))?;
    Some(rest.trim())
}

pub fn is_loopback(addr: &str) -> bool {
    let host = addr
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(addr)
        .trim_matches(['[', ']']);
    host == "127.0.0.1" || host == "::1" || host == "localhost"
}
