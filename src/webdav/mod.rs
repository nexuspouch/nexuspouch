use crate::admin::auth::{self, AuthConfig};
use crate::protocol;
use crate::store::Local;
use axum::{
    body::Body,
    extract::{ConnectInfo, Path, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;

pub struct WebDavState {
    pub store: Arc<Local>,
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavPath {
    pub space: String,
    pub device: String,
    pub path: String,
}

pub fn parse_dav_path(raw: &str) -> Result<DavPath, &'static str> {
    let trimmed = raw.trim_matches('/');
    if trimmed.is_empty() {
        return Err("empty path");
    }
    let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return Err("need space and device");
    }
    let space = parts[0];
    if !protocol::shared_readable(space) {
        return Err("space not readable");
    }
    let device = parts[1];
    if !protocol::is_valid_device_id(device) {
        return Err("invalid device");
    }
    let rel = if parts.len() > 2 {
        parts[2..].join("/")
    } else {
        String::new()
    };
    let path = if rel.is_empty() {
        String::new()
    } else {
        protocol::normalize_path(&rel).map_err(|_| "bad path")?
    };
    Ok(DavPath {
        space: space.to_string(),
        device: device.to_string(),
        path,
    })
}

pub fn router(state: Arc<WebDavState>) -> Router {
    Router::new()
        .route("/*path", any(dav_handler))
        .route("/", any(dav_root))
        .with_state(state)
}

async fn dav_root(
    State(state): State<Arc<WebDavState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    method: Method,
) -> Response {
    dav_dispatch(state, addr, headers, method, String::new()).await
}

async fn dav_handler(
    State(state): State<Arc<WebDavState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    method: Method,
    Path(path): Path<String>,
) -> Response {
    dav_dispatch(state, addr, headers, method, path).await
}

async fn dav_dispatch(
    state: Arc<WebDavState>,
    addr: SocketAddr,
    headers: HeaderMap,
    method: Method,
    raw_path: String,
) -> Response {
    let loopback = auth::is_loopback(&addr.ip().to_string());
    if !authorize(&state.auth, &headers, loopback) {
        return unauthorized();
    }

    match method {
        Method::OPTIONS => options_response(),
        Method::GET | Method::HEAD => match parse_dav_path(&raw_path) {
            Ok(p) => file_get(&state, &p, method == Method::HEAD).await,
            Err(msg) => bad_request(msg),
        },
        _ if method.as_str() == "PROPFIND" => {
            let depth = headers
                .get("depth")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("1");
            match parse_dav_path(&raw_path) {
                Ok(p) => propfind(&state, &p, depth),
                Err(msg) => bad_request(msg),
            }
        }
        _ => method_not_allowed(),
    }
}

fn authorize(auth: &AuthConfig, headers: &HeaderMap, loopback: bool) -> bool {
    if auth.token.is_empty() {
        return loopback;
    }
    auth.authorize_headers(headers, None)
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("WWW-Authenticate", r#"Bearer realm="nexuspouch-webdav""#)],
        "unauthorized",
    )
        .into_response()
}

fn bad_request(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, msg.to_string()).into_response()
}

fn method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "OPTIONS, GET, HEAD, PROPFIND")],
        "method not allowed",
    )
        .into_response()
}

fn options_response() -> Response {
    (
        StatusCode::OK,
        [
            (header::ALLOW, "OPTIONS, GET, HEAD, PROPFIND"),
            (header::HeaderName::from_static("dav"), "1"),
        ],
        Body::empty(),
    )
        .into_response()
}

async fn file_get(state: &WebDavState, dav: &DavPath, head_only: bool) -> Response {
    if dav.path.is_empty() {
        return (
            StatusCode::FORBIDDEN,
            "directory listing via GET not supported; use PROPFIND",
        )
            .into_response();
    }
    let full = match state.store.resolve(&dav.space, &dav.device, &dav.path) {
        Ok(p) => p,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let meta = match fs::metadata(&full) {
        Ok(m) => m,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    if meta.is_dir() {
        return StatusCode::FORBIDDEN.into_response();
    }
    if head_only {
        return (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (header::CONTENT_LENGTH, &meta.len().to_string()),
            ],
            Body::empty(),
        )
            .into_response();
    }
    let data = match fs::read(&full) {
        Ok(d) => d,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CONTENT_LENGTH, &data.len().to_string()),
        ],
        Body::from(data),
    )
        .into_response()
}

fn propfind(state: &WebDavState, dav: &DavPath, depth: &str) -> Response {
    let full = match state.store.resolve(&dav.space, &dav.device, &dav.path) {
        Ok(p) => p,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let meta = match fs::metadata(&full) {
        Ok(m) => m,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    let href = dav_href(dav);
    let mut responses = vec![entry_response(&href, &meta)];

    if meta.is_dir() && depth != "0" {
        if let Ok(read) = fs::read_dir(&full) {
            for entry in read.flatten() {
                let child_meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let name = entry.file_name().to_string_lossy().to_string();
                let child = DavPath {
                    space: dav.space.clone(),
                    device: dav.device.clone(),
                    path: if dav.path.is_empty() {
                        name
                    } else {
                        format!("{}/{}", dav.path, name)
                    },
                };
                responses.push(entry_response(&dav_href(&child), &child_meta));
            }
        }
    }

    let xml = multistatus(&responses);
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        xml,
    )
        .into_response()
}

fn dav_href(dav: &DavPath) -> String {
    let base = format!("/dav/{}/{}/", dav.space, dav.device);
    if dav.path.is_empty() {
        base
    } else {
        format!("{base}{}", dav.path)
    }
}

fn entry_response(href: &str, meta: &fs::Metadata) -> String {
    let kind = if meta.is_dir() {
        "<D:collection/>"
    } else {
        ""
    };
    let len = meta.len();
    let modified = format_http_date(meta);
    format!(
        r#"<D:response>
  <D:href>{href}</D:href>
  <D:propstat>
    <D:prop>
      <D:resourcetype>{kind}</D:resourcetype>
      <D:getcontentlength>{len}</D:getcontentlength>
      <D:getlastmodified>{modified}</D:getlastmodified>
    </D:prop>
    <D:status>HTTP/1.1 200 OK</D:status>
  </D:propstat>
</D:response>"#
    )
}

fn format_http_date(meta: &fs::Metadata) -> String {
    meta.modified()
        .ok()
        .and_then(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            Some(dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        })
        .unwrap_or_else(|| "Thu, 01 Jan 1970 00:00:00 GMT".into())
}

fn multistatus(responses: &[String]) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
{}
</D:multistatus>"#,
        responses.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dav_path_ok() {
        let p = parse_dav_path("artifacts/aaaaaaaaaaaaaaaa/task/x.txt").unwrap();
        assert_eq!(p.space, "artifacts");
        assert_eq!(p.device, "aaaaaaaaaaaaaaaa");
        assert_eq!(p.path, "task/x.txt");
    }

    #[test]
    fn parse_rejects_backups() {
        assert!(parse_dav_path("backups/aaaaaaaaaaaaaaaa/x").is_err());
    }

    #[test]
    fn parse_device_root() {
        let p = parse_dav_path("files/bbbbbbbbbbbbbbbb/").unwrap();
        assert_eq!(p.path, "");
    }
}
