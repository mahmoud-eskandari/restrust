//! REST API assembly (CONTRACT §4.4). All routes are mounted under `/api/v1`
//! plus the `/public/{slug}` alias used by published collections.

pub mod admin;
pub mod auth;
pub mod collections;
pub mod public;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::state::SharedState;

/// `GET /api/v1/healthz` — unauthenticated; answers on both listeners.
pub async fn healthz(State(state): State<SharedState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "https": state.config.tls_active(),
        "version": env!("CARGO_PKG_VERSION"),
        "config": state.config.public_summary(),
    }))
}

/// Plain-text/JSON 404 for unmatched routes when no static dir is configured.
async fn fallback_404() -> Response {
    crate::error::ApiError::not_found("no such route").into_response()
}

/// The full application router (API + publications + optional static web
/// client). `/api/*` and `/public/*` always take precedence over the static
/// fallback service.
pub fn app_router(state: SharedState) -> Router {
    let v1 = Router::new()
        .route("/healthz", get(healthz))
        .route("/me", get(auth::session))
        .nest("/auth", auth::router())
        .merge(collections::router())
        .nest("/admin", admin::router())
        .nest("/public", public::public_routes());

    let mut app = Router::new()
        .nest("/api/v1", v1)
        // `/public/{slug}` serves the same publication HTML as
        // `/api/v1/public/{slug}` (AC-20).
        .nest("/public", public::public_routes());

    app = match &state.config.web_dir {
        Some(web_dir) => {
            let index = web_dir.join("index.html");
            let serve_dir = tower_http::services::ServeDir::new(web_dir)
                .append_index_html_on_directories(true)
                .fallback(tower_http::services::ServeFile::new(index));
            app.fallback_service(serve_dir)
        }
        None => app.fallback(fallback_404),
    };

    app.with_state(state)
}
