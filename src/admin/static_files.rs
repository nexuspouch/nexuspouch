//! Serve built admin SPA from `web/admin/dist/*` (see `web/admin/README.md`).
//!
//! Resolution: `NEXUSPOUCH_ADMIN_STATIC` → `web/admin/dist` (manifest or cwd).

use axum::{
    body::Body,
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use std::path::PathBuf;
use tower_http::services::{ServeDir, ServeFile};

const SESSIONS_INDEX: &str = "sessions/index.html";

pub struct AdminStatic {
    pub root: PathBuf,
}

impl AdminStatic {
    pub fn discover() -> Option<Self> {
        let root = admin_dist_root()?;
        if !root.join(SESSIONS_INDEX).is_file() {
            tracing::warn!(
                "admin static: {} missing {}; run `npm run build` in web/admin",
                root.display(),
                SESSIONS_INDEX
            );
            return None;
        }
        tracing::info!("admin static: serving from {}", root.display());
        Some(Self { root })
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// `/admin/sessions` redirect + `/admin/sessions/*` static + SPA fallback.
    pub fn router(&self) -> axum::Router {
        let dir = self.sessions_dir();
        let index = dir.join("index.html");
        let serve = ServeDir::new(&dir).not_found_service(ServeFile::new(index));
        axum::Router::new()
            .route(
                "/admin/sessions",
                axum::routing::get(|| async { Redirect::permanent("/admin/sessions/") }),
            )
            .nest_service("/admin/sessions/", serve)
    }
}

fn admin_dist_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NEXUSPOUCH_ADMIN_STATIC") {
        let p = PathBuf::from(p);
        if p.join(SESSIONS_INDEX).is_file() {
            return Some(p);
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for p in [
        manifest.join("web/admin/dist"),
        PathBuf::from("web/admin/dist"),
    ] {
        if p.join(SESSIONS_INDEX).is_file() {
            return Some(p);
        }
    }
    None
}

pub fn fallback_embedded_sessions() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(super::sessions_ui::HTML))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
