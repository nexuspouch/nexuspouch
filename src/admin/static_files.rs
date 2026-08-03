//! Serve built admin SPA from `web/admin/dist/*` (see `web/admin/README.md`).
//!
//! Resolution: `NEXUSPOUCH_ADMIN_STATIC` → `web/admin/dist` (manifest or cwd).
//! Main UI assets are served at `/admin/assets/*` (Vite `base: '/admin/'`).
//! Sessions SPA remains under `/admin/sessions/`.

use axum::{
    body::Body,
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use std::path::PathBuf;
use tower_http::services::{ServeDir, ServeFile};

const MAIN_INDEX: &str = "main/index.html";
const SESSIONS_INDEX: &str = "sessions/index.html";

pub struct AdminStatic {
    pub root: PathBuf,
    pub has_main: bool,
    pub has_sessions: bool,
}

impl AdminStatic {
    pub fn discover() -> Option<Self> {
        let root = admin_dist_root()?;
        let has_main = root.join(MAIN_INDEX).is_file();
        let has_sessions = root.join(SESSIONS_INDEX).is_file();
        if !has_main && !has_sessions {
            tracing::warn!(
                "admin static: {} has neither {} nor {}; run `npm run build` in web/admin",
                root.display(),
                MAIN_INDEX,
                SESSIONS_INDEX
            );
            return None;
        }
        tracing::info!(
            has_main,
            has_sessions,
            "admin static: serving from {}",
            root.display()
        );
        Some(Self {
            root,
            has_main,
            has_sessions,
        })
    }

    pub fn router(&self) -> axum::Router {
        let mut r = axum::Router::new();
        if self.has_main {
            r = r.merge(self.main_router());
        }
        if self.has_sessions {
            r = r.merge(self.sessions_router());
        }
        r
    }

    fn main_router(&self) -> axum::Router {
        let dir = self.root.join("main");
        let index = dir.join("index.html");
        let assets = dir.join("assets");
        let mut r = axum::Router::new()
            .route(
                "/admin",
                get(|| async { Redirect::permanent("/admin/") }),
            )
            .route_service("/admin/", ServeFile::new(index));
        if assets.is_dir() {
            r = r.nest_service("/admin/assets", ServeDir::new(assets));
        }
        r
    }

    fn sessions_router(&self) -> axum::Router {
        let dir = self.root.join("sessions");
        let index = dir.join("index.html");
        let serve = ServeDir::new(&dir).not_found_service(ServeFile::new(index));
        axum::Router::new()
            .route(
                "/admin/sessions",
                get(|| async { Redirect::permanent("/admin/sessions/") }),
            )
            .nest_service("/admin/sessions/", serve)
    }
}

fn admin_dist_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NEXUSPOUCH_ADMIN_STATIC") {
        let p = PathBuf::from(p);
        if p.join(MAIN_INDEX).is_file() || p.join(SESSIONS_INDEX).is_file() {
            return Some(p);
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for p in [
        manifest.join("web/admin/dist"),
        PathBuf::from("web/admin/dist"),
    ] {
        if p.join(MAIN_INDEX).is_file() || p.join(SESSIONS_INDEX).is_file() {
            return Some(p);
        }
    }
    None
}

pub fn fallback_embedded_main() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(super::ui::HTML))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub fn fallback_embedded_sessions() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(super::sessions_ui::HTML))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
