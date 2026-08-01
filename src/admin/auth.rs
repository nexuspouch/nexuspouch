use crate::auth_tokens::{scopes_allow, TokenStore};
use axum::http::HeaderMap;
use std::sync::Arc;
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct AuthConfig {
    pub token: String,
    pub tokens: Option<Arc<TokenStore>>,
}

impl AuthConfig {
    pub fn new(token: impl Into<String>, tokens: Option<Arc<TokenStore>>) -> Self {
        Self {
            token: token.into(),
            tokens,
        }
    }

    /// Admin UI/API: requires admin scope, or open access when no credentials configured.
    pub fn authorize_headers(&self, headers: &HeaderMap, query_token: Option<&str>) -> bool {
        if let Some(scopes) = self.resolve_scopes(headers, query_token, false) {
            return scopes_allow(&scopes, &["admin"]);
        }
        self.no_auth_configured()
    }

    pub fn authorize_scopes(
        &self,
        headers: &HeaderMap,
        query_token: Option<&str>,
        loopback: bool,
        required: &[&str],
    ) -> bool {
        if let Some(scopes) = self.resolve_scopes(headers, query_token, loopback) {
            return scopes_allow(&scopes, required);
        }
        false
    }

    pub fn resolve_scopes(
        &self,
        headers: &HeaderMap,
        query_token: Option<&str>,
        loopback: bool,
    ) -> Option<Vec<String>> {
        let presented = presented_token(headers, query_token).unwrap_or("");
        if !self.token.is_empty()
            && !presented.is_empty()
            && bool::from(presented.as_bytes().ct_eq(self.token.as_bytes()))
        {
            return Some(vec!["admin".into()]);
        }
        if let Some(store) = &self.tokens {
            if let Some(scopes) = store.resolve(presented) {
                return Some(scopes);
            }
        }
        if self.no_auth_configured() && loopback {
            return Some(vec!["admin".into()]);
        }
        None
    }

    pub fn no_auth_configured(&self) -> bool {
        self.token.is_empty()
            && self
                .tokens
                .as_ref()
                .map(|t| t.list_public().is_empty())
                .unwrap_or(true)
    }
}

fn presented_token<'a>(headers: &'a HeaderMap, query_token: Option<&'a str>) -> Option<&'a str> {
    bearer_token(headers)
        .or_else(|| headers.get("x-admin-token").and_then(|v| v.to_str().ok()))
        .or(query_token)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let h = headers.get("authorization")?.to_str().ok()?;
    let rest = h
        .strip_prefix("Bearer ")
        .or_else(|| h.strip_prefix("bearer "))?;
    Some(rest.trim())
}

pub fn is_loopback(addr: &str) -> bool {
    let host = addr
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(addr)
        .trim_matches(['[', ']']);
    host == "127.0.0.1"
        || host == "::1"
        || host == "localhost"
        || host.starts_with("127.")
}
