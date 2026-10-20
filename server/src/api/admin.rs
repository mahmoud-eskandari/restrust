//! Admin endpoints (CONTRACT §4.4). Every call re-checks `system_role='admin'`
//! via the [`AdminUser`] extractor (AC-19) — a normal user gets 403
//! `access_denied` even if a stale admin UI is visible client-side.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, put};
use axum::{Json, Router};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{self, AdminUser};
use crate::error::{ApiError, ApiResult};
use crate::state::SharedState;

fn all_users(conn: &Connection) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, username, display_name, system_role, disabled, created_at
         FROM users ORDER BY created_at, username",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "username": row.get::<_, String>(1)?,
                "display_name": row.get::<_, String>(2)?,
                "system_role": row.get::<_, String>(3)?,
                "disabled": row.get::<_, i64>(4)? != 0,
                "created_at": row.get::<_, String>(5)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// `GET /api/v1/admin/users`
pub async fn list_users(
    State(state): State<SharedState>,
    AdminUser(_admin): AdminUser,
) -> ApiResult<Json<serde_json::Value>> {
    let users = state.with_db(all_users).map_err(ApiError::from)?;
    Ok(Json(Value::Array(users)))
}

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub system_role: Option<String>,
}

/// `POST /api/v1/admin/users {username,password,system_role}`
pub async fn create_user(
    State(state): State<SharedState>,
    AdminUser(_admin): AdminUser,
    Json(body): Json<CreateUserRequest>,
) -> ApiResult<Response> {
    let username = body.username.trim();
    if username.is_empty() || body.password.is_empty() {
        return Err(ApiError::bad_request(
            "`username` and `password` are required",
        ));
    }
    let system_role = body.system_role.as_deref().unwrap_or("user");
    if !matches!(system_role, "user" | "admin") {
        return Err(ApiError::bad_request(
            "`system_role` must be \"user\" or \"admin\"",
        ));
    }

    let id = state.with_db(|conn| -> ApiResult<String> {
        let taken: bool = conn
            .query_row(
                "SELECT 1 FROM users WHERE username = ?1",
                [username],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if taken {
            return Err(ApiError::bad_request("username already exists"));
        }
        let id = restrust_core::model::new_id();
        let hash = auth::hash_password(&body.password);
        conn.execute(
            "INSERT INTO users (id, username, password_hash, display_name, email, system_role, disabled, created_at, auth_provider, provider_subject)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, 0, ?6, 'local', NULL)",
            params![id, username, hash, username, system_role, restrust_core::model::now_rfc3339()],
        )
        .map_err(ApiError::from)?;
        Ok(id)
    })?;

    Ok((
        StatusCode::CREATED,
        Json(json!({"id": id, "username": username, "system_role": system_role})),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct PatchUserRequest {
    pub system_role: Option<String>,
    pub disabled: Option<bool>,
    pub password: Option<String>,
}

/// `PATCH /api/v1/admin/users/{id} {system_role?,disabled?,password?}`
pub async fn patch_user(
    State(state): State<SharedState>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
    Json(body): Json<PatchUserRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if let Some(role) = &body.system_role {
        if !matches!(role.as_str(), "user" | "admin") {
            return Err(ApiError::bad_request(
                "`system_role` must be \"user\" or \"admin\"",
            ));
        }
    }
    state.with_db(|conn| -> ApiResult<()> {
        let exists: bool = conn
            .query_row("SELECT 1 FROM users WHERE id = ?1", [&id], |_| Ok(()))
            .optional()?
            .is_some();
        if !exists {
            return Err(ApiError::not_found("user not found"));
        }
        if let Some(role) = &body.system_role {
            conn.execute(
                "UPDATE users SET system_role = ?1 WHERE id = ?2",
                params![role, id],
            )
            .map_err(ApiError::from)?;
        }
        if let Some(disabled) = body.disabled {
            conn.execute(
                "UPDATE users SET disabled = ?1 WHERE id = ?2",
                params![disabled as i64, id],
            )
            .map_err(ApiError::from)?;
            if disabled {
                // Kill the disabled user's sessions immediately.
                conn.execute("DELETE FROM sessions WHERE user_id = ?1", [&id])
                    .map_err(ApiError::from)?;
            }
        }
        if let Some(password) = &body.password {
            if password.is_empty() {
                return Err(ApiError::bad_request("`password` must not be empty"));
            }
            let hash = auth::hash_password(password);
            conn.execute(
                "UPDATE users SET password_hash = ?1 WHERE id = ?2",
                params![hash, id],
            )
            .map_err(ApiError::from)?;
        }
        Ok(())
    })?;
    Ok(Json(json!({"ok": true})))
}

fn all_organizations(conn: &Connection) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare("SELECT o.id, o.name FROM organizations o ORDER BY o.name")?;
    let orgs: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut out = Vec::new();
    for (id, name) in orgs {
        let mut members_stmt = conn.prepare(
            "SELECT m.user_id, u.username, m.role FROM org_members m
             JOIN users u ON u.id = m.user_id WHERE m.org_id = ?1 ORDER BY u.username",
        )?;
        let members: Vec<serde_json::Value> = members_stmt
            .query_map([&id], |row| {
                Ok(json!({
                    "user_id": row.get::<_, String>(0)?,
                    "username": row.get::<_, String>(1)?,
                    "role": row.get::<_, String>(2)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.push(json!({"id": id, "name": name, "members": members}));
    }
    Ok(out)
}

/// `GET /api/v1/admin/organizations`
pub async fn list_organizations(
    State(state): State<SharedState>,
    AdminUser(_admin): AdminUser,
) -> ApiResult<Json<serde_json::Value>> {
    let orgs = state.with_db(all_organizations).map_err(ApiError::from)?;
    Ok(Json(Value::Array(orgs)))
}

#[derive(Deserialize)]
pub struct CreateOrgRequest {
    pub name: String,
}

/// `POST /api/v1/admin/organizations {name}`
pub async fn create_organization(
    State(state): State<SharedState>,
    AdminUser(_admin): AdminUser,
    Json(body): Json<CreateOrgRequest>,
) -> ApiResult<Response> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("`name` must not be empty"));
    }
    let id = state.with_db(|conn| -> ApiResult<String> {
        let taken: bool = conn
            .query_row(
                "SELECT 1 FROM organizations WHERE name = ?1",
                [name],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if taken {
            return Err(ApiError::bad_request("organization name already exists"));
        }
        let id = restrust_core::model::new_id();
        conn.execute(
            "INSERT INTO organizations (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![id, name, restrust_core::model::now_rfc3339()],
        )
        .map_err(ApiError::from)?;
        Ok(id)
    })?;
    Ok((StatusCode::CREATED, Json(json!({"id": id, "name": name}))).into_response())
}

#[derive(Deserialize)]
pub struct MembersRequest {
    pub members: Vec<MemberIn>,
}

#[derive(Deserialize)]
pub struct MemberIn {
    pub username: String,
    pub role: String,
}

/// `PUT /api/v1/admin/organizations/{id}/members {members:[{username,role}]}`
/// — replaces the member list.
pub async fn put_members(
    State(state): State<SharedState>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
    Json(body): Json<MembersRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .with_db(|conn| -> ApiResult<serde_json::Value> {
            let exists: bool = conn
                .query_row("SELECT 1 FROM organizations WHERE id = ?1", [&id], |_| {
                    Ok(())
                })
                .optional()?
                .is_some();
            if !exists {
                return Err(ApiError::not_found("organization not found"));
            }

            // Resolve all usernames first so the replacement is atomic-ish.
            let mut resolved = Vec::new();
            for (i, member) in body.members.iter().enumerate() {
                if !matches!(member.role.as_str(), "admin" | "editor" | "viewer") {
                    return Err(ApiError::bad_request(format!(
                        "members[{i}].role must be \"admin\", \"editor\" or \"viewer\""
                    )));
                }
                let user_id: String = conn
                    .query_row(
                        "SELECT id FROM users WHERE username = ?1",
                        [member.username.trim()],
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or_else(|| {
                        ApiError::bad_request(format!(
                            "members[{i}]: unknown user {}",
                            member.username
                        ))
                    })?;
                resolved.push((user_id, member.role.clone()));
            }

            conn.execute("DELETE FROM org_members WHERE org_id = ?1", [&id])
                .map_err(ApiError::from)?;
            for (user_id, role) in &resolved {
                conn.execute(
                    "INSERT INTO org_members (org_id, user_id, role) VALUES (?1, ?2, ?3)",
                    params![id, user_id, role],
                )
                .map_err(ApiError::from)?;
            }
            Ok(json!({"ok": true, "members": resolved.len()}))
        })
        .map(Json)
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route("/users/{id}", patch(patch_user))
        .route(
            "/organizations",
            get(list_organizations).post(create_organization),
        )
        .route("/organizations/{id}/members", put(put_members))
}
