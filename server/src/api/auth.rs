//! Auth endpoints: login/logout/session/providers/me (CONTRACT §4.4).

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::error::{ApiError, ApiResult};
use crate::oidc;
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// `POST /api/v1/auth/login` → `{token, user}` | 401.
pub async fn login(
    State(state): State<SharedState>,
    Json(body): Json<LoginRequest>,
) -> ApiResult<Response> {
    let (user_id, stored_hash, disabled) = state
        .with_db(|conn| {
            conn.query_row(
                "SELECT id, password_hash, disabled FROM users WHERE username = ?1",
                [&body.username.trim()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)? != 0,
                    ))
                },
            )
            .optional()
        })
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::unauthorized("invalid username or password"))?;

    let valid = stored_hash
        .as_deref()
        .map(|hash| auth::verify_password(&body.password, hash))
        .unwrap_or(false);
    if !valid {
        return Err(ApiError::unauthorized("invalid username or password"));
    }
    if disabled {
        return Err(ApiError::unauthorized("account is disabled"));
    }

    let (token, user_json) = state.with_db(|conn| {
        let token = auth::create_session(conn, &user_id)?;
        let user = user_by_id(conn, &user_id)
            .ok_or_else(|| ApiError::internal("user vanished during login"))?;
        Ok::<_, ApiError>((token, user.to_json()))
    })?;

    let body = json!({"token": token, "user": user_json});
    Ok((
        StatusCode::OK,
        [
            (header::SET_COOKIE, auth::session_cookie(&token)),
            (header::CONTENT_TYPE, "application/json".to_string()),
        ],
        body.to_string(),
    )
        .into_response())
}

pub(crate) fn user_by_id(conn: &Connection, id: &str) -> Option<crate::auth::User> {
    conn.query_row(
        "SELECT id, username, display_name, email, system_role, disabled, created_at, auth_provider
         FROM users WHERE id = ?1",
        [id],
        |row| {
            Ok(crate::auth::User {
                id: row.get(0)?,
                username: row.get(1)?,
                display_name: row.get(2)?,
                email: row.get(3)?,
                system_role: row.get(4)?,
                disabled: row.get::<_, i64>(5)? != 0,
                created_at: row.get(6)?,
                auth_provider: row.get(7)?,
            })
        },
    )
    .ok()
}

/// `POST /api/v1/auth/logout` — kills the presented session (if any).
pub async fn logout(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Some(token) = auth::token_from_headers(&headers) {
        let _ = state.with_db(|conn| auth::delete_session(conn, &token));
    }
    (
        StatusCode::OK,
        [
            (header::SET_COOKIE, auth::clear_session_cookie()),
            (header::CONTENT_TYPE, "application/json".to_string()),
        ],
        r#"{"ok":true}"#.to_string(),
    )
        .into_response()
}

fn user_organizations(
    conn: &Connection,
    user_id: &str,
) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT o.id, o.name, m.role FROM org_members m
         JOIN organizations o ON o.id = m.org_id
         WHERE m.user_id = ?1 ORDER BY o.name",
    )?;
    let rows = stmt
        .query_map([user_id], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "role": row.get::<_, String>(2)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// `GET /api/v1/auth/session` (and `GET /api/v1/me`)
/// → `{user, organizations:[{id,name,role}]}` | 401.
pub async fn session(
    State(state): State<SharedState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let organizations = state
        .with_db(|conn| user_organizations(conn, &user.id))
        .map_err(ApiError::from)?;
    Ok(Json(json!({
        "user": user.to_json(),
        "organizations": organizations,
    })))
}

/// `GET /api/v1/auth/providers` — only the configured ones.
pub async fn providers(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let providers: Vec<serde_json::Value> = state
        .oidc_providers
        .iter()
        .map(|p| json!({"id": p.id, "name": p.display_name, "kind": "oidc"}))
        .collect();
    Json(serde_json::Value::Array(providers))
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/session", get(session))
        .route("/providers", get(providers))
        .route("/oidc/{provider}/start", get(oidc::start))
        .route("/oidc/{provider}/callback", get(oidc::callback))
        .route("/oidc/done", get(oidc::done))
}
