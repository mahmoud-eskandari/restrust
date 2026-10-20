//! Public publication endpoints: read-only HTML rendering and the JSON
//! variant, both unauthenticated (CONTRACT §4.4, AC-11/AC-20).

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use restrust_core::model::{Collection, CollectionItem};
use rusqlite::Connection;
use serde_json::json;

use crate::error::{ApiError, ApiResult};
use crate::oidc::html_escape;
use crate::state::SharedState;
fn published_collection(conn: &Connection, slug: &str) -> Option<Collection> {
    let data: String = conn
        .query_row(
            "SELECT c.data FROM publications p
             JOIN collections c ON c.id = p.collection_id
             WHERE p.slug = ?1 AND c.deleted = 0",
            [slug],
            |row| row.get(0),
        )
        .ok()?;
    serde_json::from_str(&data).ok()
}

/// `GET /api/v1/public/{slug}` and `GET /public/{slug}` — HTML.
pub async fn public_html(State(state): State<SharedState>, Path(slug): Path<String>) -> Response {
    let collection = state.with_db(|conn| published_collection(conn, &slug));
    match collection {
        Some(collection) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            render_html(&collection),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>Not found</title></head>\
             <body><h1>404 — no such publication</h1>\
             <p>This collection is not published (or was unpublished).</p></body></html>"
                .to_string(),
        )
            .into_response(),
    }
}

/// `GET /api/v1/public/{slug}/json` — `{collection}` without auth.
pub async fn public_json(
    State(state): State<SharedState>,
    Path(slug): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let collection = state
        .with_db(|conn| published_collection(conn, &slug))
        .ok_or_else(|| ApiError::not_found("no such publication"))?;
    Ok(Json(json!({"collection": collection})))
}

/// Server-side read-only rendering: collection name/description plus every
/// request (method + URL + description). All user strings are escaped.
pub fn render_html(collection: &Collection) -> String {
    let mut body = String::new();
    render_items(&collection.items, &mut body, 0);
    let description = collection
        .description
        .as_deref()
        .map(|d| format!("<p class=\"desc\">{}</p>", html_escape(d)))
        .unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} — restrust</title>
<style>
 body {{ font-family: system-ui, -apple-system, sans-serif; max-width: 52rem;
        margin: 2rem auto; padding: 0 1rem; color: #111827; }}
 h1 {{ margin-bottom: 0.25rem; }}
 .desc {{ color: #4b5563; }}
 details {{ margin: 0.5rem 0; padding: 0.25rem 0.5rem; background: #f9fafb;
           border-radius: 6px; }}
 summary {{ cursor: pointer; font-weight: 600; }}
 .req {{ display: flex; gap: 0.75rem; padding: 0.4rem 0.5rem; align-items: baseline; }}
 .method {{ font-family: ui-monospace, monospace; font-weight: 700; font-size: 0.8rem;
           padding: 0.1rem 0.4rem; border-radius: 4px; background: #dbeafe; color: #1e3a8a;
           min-width: 3.5rem; text-align: center; }}
 .url {{ font-family: ui-monospace, monospace; word-break: break-all; }}
 .note {{ color: #4b5563; font-size: 0.9rem; margin: 0.25rem 0 0.25rem 4.75rem; }}
 footer {{ margin-top: 2rem; color: #9ca3af; font-size: 0.8rem; }}
</style></head>
<body>
<h1>{title}</h1>
{description}
{body}
<footer>Published read-only via restrust.</footer>
</body></html>"#,
        title = html_escape(&collection.name),
    )
}

fn render_items(items: &[CollectionItem], out: &mut String, depth: usize) {
    if depth > 12 {
        return; // defensive: no unbounded nesting output
    }
    for item in items {
        match item {
            CollectionItem::Folder(folder) => {
                out.push_str(&format!(
                    "<details open><summary>{}</summary><div style=\"margin-left:1rem\">",
                    html_escape(&folder.name)
                ));
                render_items(&folder.items, out, depth + 1);
                out.push_str("</div></details>");
            }
            CollectionItem::Request(request) => {
                out.push_str(&format!(
                    "<div class=\"req\"><span class=\"method\">{}</span><span class=\"url\">{}</span></div>",
                    html_escape(request.method.as_str()),
                    html_escape(&request.url)
                ));
                if let Some(description) = &request.description {
                    if !description.trim().is_empty() {
                        out.push_str(&format!(
                            "<p class=\"note\">{}</p>",
                            html_escape(description)
                        ));
                    }
                }
            }
        }
    }
}

pub fn public_routes() -> axum::Router<SharedState> {
    axum::Router::new()
        .route("/{slug}", axum::routing::get(public_html))
        .route("/{slug}/json", axum::routing::get(public_json))
}
