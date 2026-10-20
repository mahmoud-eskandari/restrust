//! Collection endpoints: list/create/get/PUT-sync/delete, shares, and
//! publish/unpublish (CONTRACT §4.4). Access control:
//!
//! - personal collection: owner has full control; shares grant read/full.
//! - full-access collaborator: GET + PUT collection data allowed; DELETE
//!   collection, PUT shares, publish/unpublish are owner-only (AC-09).
//! - org namespace: admin/editor can create/PUT/delete, viewer is read-only.
//! - revocation (share/membership removal) ⇒ next call 403 `access_denied`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use restrust_core::model::Collection;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::state::SharedState;

// --- access model ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    Read,
    Full,
    Owner,
}

impl Access {
    pub fn as_str(&self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Full => "full",
            Access::Owner => "owner",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CollectionRow {
    pub id: String,
    pub namespace_kind: String,
    pub owner_user_id: Option<String>,
    pub org_id: Option<String>,
    pub name: String,
    pub data: String,
    pub revision: i64,
    pub updated_at: String,
}

fn fetch_collection(conn: &Connection, id: &str) -> rusqlite::Result<Option<CollectionRow>> {
    conn.query_row(
        "SELECT id, namespace_kind, owner_user_id, org_id, name, data, revision, updated_at
         FROM collections WHERE id = ?1 AND deleted = 0",
        [id],
        |row| {
            Ok(CollectionRow {
                id: row.get(0)?,
                namespace_kind: row.get(1)?,
                owner_user_id: row.get(2)?,
                org_id: row.get(3)?,
                name: row.get(4)?,
                data: row.get(5)?,
                revision: row.get(6)?,
                updated_at: row.get(7)?,
            })
        },
    )
    .optional()
}

/// The caller's effective access, if any: strongest of ownership, org
/// membership role, direct user share, org share, and public share.
pub fn access_for(conn: &Connection, user_id: &str, row: &CollectionRow) -> Option<Access> {
    let mut best: Option<Access> = None;
    let mut consider = |a: Option<Access>| {
        if a > best {
            best = a;
        }
    };

    if row.owner_user_id.as_deref() == Some(user_id) {
        consider(Some(Access::Owner));
    }

    if let Some(org_id) = &row.org_id {
        let role: Option<String> = conn
            .query_row(
                "SELECT role FROM org_members WHERE org_id = ?1 AND user_id = ?2",
                params![org_id, user_id],
                |r| r.get(0),
            )
            .ok();
        consider(match role.as_deref() {
            Some("admin") => Some(Access::Owner),
            Some("editor") => Some(Access::Full),
            Some("viewer") => Some(Access::Read),
            _ => None,
        });
    }

    let mut stmt = match conn
        .prepare("SELECT subject_kind, subject_id, level FROM shares WHERE collection_id = ?1")
    {
        Ok(stmt) => stmt,
        Err(_) => return best,
    };
    let share_rows: Vec<(String, Option<String>, String)> = stmt
        .query_map([&row.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();

    let user_orgs: Vec<String> = conn
        .prepare("SELECT org_id FROM org_members WHERE user_id = ?1")
        .and_then(|mut stmt| {
            stmt.query_map([user_id], |r| r.get::<_, String>(0))
                .map(|rows| rows.flatten().collect())
        })
        .unwrap_or_default();

    for (kind, subject_id, level) in share_rows {
        let level = if level == "full" {
            Access::Full
        } else {
            Access::Read
        };
        match kind.as_str() {
            "user" if subject_id.as_deref() == Some(user_id) => consider(Some(level)),
            "org"
                if subject_id
                    .as_deref()
                    .map(|sid| user_orgs.iter().any(|o| o == sid))
                    .unwrap_or(false) =>
            {
                consider(Some(level));
            }
            // A public share grants read to every authenticated user.
            "public" => consider(Some(Access::Read)),
            _ => {}
        }
    }
    best
}

/// Rows visible in the listing: owned, org member, or direct user share
/// (public shares are not listed per CONTRACT §4.4).
fn list_visible(conn: &Connection, user_id: &str) -> rusqlite::Result<Vec<CollectionRow>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.namespace_kind, c.owner_user_id, c.org_id, c.name, c.data, c.revision, c.updated_at
         FROM collections c
         WHERE c.deleted = 0
           AND (c.owner_user_id = ?1
                OR EXISTS (SELECT 1 FROM org_members m WHERE m.org_id = c.org_id AND m.user_id = ?1)
                OR EXISTS (SELECT 1 FROM shares s
                           WHERE s.collection_id = c.id AND s.subject_kind = 'user' AND s.subject_id = ?1))
         ORDER BY c.updated_at DESC",
    )?;
    let rows = stmt
        .query_map([user_id], |row| {
            Ok(CollectionRow {
                id: row.get(0)?,
                namespace_kind: row.get(1)?,
                owner_user_id: row.get(2)?,
                org_id: row.get(3)?,
                name: row.get(4)?,
                data: row.get(5)?,
                revision: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn namespace_json(conn: &Connection, row: &CollectionRow) -> rusqlite::Result<serde_json::Value> {
    if row.namespace_kind == "organization" {
        let org_id = row.org_id.clone().unwrap_or_default();
        let name = conn
            .query_row(
                "SELECT name FROM organizations WHERE id = ?1",
                [&org_id],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_default();
        Ok(json!({"kind": "organization", "id": org_id, "name": name}))
    } else {
        let owner_id = row.owner_user_id.clone().unwrap_or_default();
        let owner_name = if owner_id.is_empty() {
            String::new()
        } else {
            conn.query_row(
                "SELECT username FROM users WHERE id = ?1",
                [&owner_id],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_default()
        };
        Ok(json!({"kind": "personal", "id": owner_id, "name": owner_name}))
    }
}

// --- handlers ---------------------------------------------------------------------

/// `GET /api/v1/collections`
pub async fn list(
    State(state): State<SharedState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let items = state
        .with_db(|conn| {
            let mut out = Vec::new();
            for row in list_visible(conn, &user.id)? {
                let Some(access) = access_for(conn, &user.id, &row) else {
                    continue;
                };
                // Name preference: stored row name, falling back to the data JSON.
                let name = if row.name.trim().is_empty() {
                    serde_json::from_str::<Collection>(&row.data)
                        .ok()
                        .map(|c| c.name)
                        .unwrap_or_default()
                } else {
                    row.name.clone()
                };
                out.push(json!({
                    "id": row.id,
                    "name": name,
                    "namespace": namespace_json(conn, &row)?,
                    "revision": row.revision,
                    "access": access.as_str(),
                    "synced_at": row.updated_at,
                    "origin": {
                        "server_url": state.config.public_base_url.clone(),
                        "collection_id": row.id,
                        "namespace": namespace_json(conn, &row)?,
                    },
                }));
            }
            Ok::<_, rusqlite::Error>(out)
        })
        .map_err(ApiError::from)?;
    Ok(Json(Value::Array(items)))
}

#[derive(Deserialize)]
pub struct CreateRequest {
    pub name: String,
    pub namespace: Option<NamespaceIn>,
}

#[derive(Deserialize)]
pub struct NamespaceIn {
    pub kind: Option<String>,
    pub id: Option<String>,
}

/// `POST /api/v1/collections {name, namespace:{kind,id?}}` → `{id, revision:1}`.
pub async fn create(
    State(state): State<SharedState>,
    AuthUser(user): AuthUser,
    Json(body): Json<CreateRequest>,
) -> ApiResult<Response> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("`name` must not be empty"));
    }

    let (namespace_kind, org_id) = match body.namespace.as_ref() {
        None => ("personal".to_string(), None),
        Some(ns) if ns.kind.as_deref().unwrap_or("personal") == "personal" => {
            ("personal".to_string(), None)
        }
        Some(ns) if ns.kind.as_deref() == Some("organization") => {
            let org_id = ns
                .id
                .clone()
                .ok_or_else(|| ApiError::bad_request("organization namespace requires `id`"))?;
            ("organization".to_string(), Some(org_id))
        }
        Some(ns) => {
            return Err(ApiError::bad_request(format!(
                "unknown namespace kind `{}`",
                ns.kind.clone().unwrap_or_default()
            )))
        }
    };

    let result = state.with_db(|conn| -> ApiResult<(String, String)> {
        if let Some(org_id) = &org_id {
            let role: Option<String> = conn
                .query_row(
                    "SELECT role FROM org_members WHERE org_id = ?1 AND user_id = ?2",
                    params![org_id, user.id],
                    |r| r.get(0),
                )
                .ok();
            match role.as_deref() {
                Some("admin") | Some("editor") => {}
                Some("viewer") => {
                    return Err(ApiError::access_denied(
                        "viewers cannot create organization collections",
                    ))
                }
                _ => return Err(ApiError::access_denied("not a member of this organization")),
            }
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM organizations WHERE id = ?1",
                    [org_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !exists {
                return Err(ApiError::not_found("organization not found"));
            }
        }

        let collection = Collection::new(name);
        let id = collection.id.clone();
        let data = serde_json::to_string(&collection)
            .map_err(|e| ApiError::bad_request(format!("collection serialization failed: {e}")))?;
        let now = restrust_core::model::now_rfc3339();
        conn.execute(
            "INSERT INTO collections (id, namespace_kind, owner_user_id, org_id, name, data, revision, updated_at, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, 0)",
            params![
                id,
                namespace_kind,
                if org_id.is_some() { None } else { Some(user.id.clone()) },
                org_id,
                name,
                data,
                now
            ],
        )
        .map_err(ApiError::from)?;
        Ok((id, now))
    })?;

    Ok((
        StatusCode::CREATED,
        Json(json!({"id": result.0, "revision": 1})),
    )
        .into_response())
}

/// `GET /api/v1/collections/{id}` → `{collection, revision, access}` | 403/404.
pub async fn get_one(
    State(state): State<SharedState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .with_db(|conn| -> ApiResult<serde_json::Value> {
            let row = fetch_collection(conn, &id)
                .map_err(ApiError::from)?
                .ok_or_else(|| ApiError::not_found("collection not found"))?;
            let access = access_for(conn, &user.id, &row).ok_or_else(|| {
                // Includes revoked grants: the client enters its revoked flow.
                ApiError::access_denied("you do not have access to this collection")
            })?;
            let collection: Collection = serde_json::from_str(&row.data)
                .map_err(|e| ApiError::internal(format!("stored collection is corrupt: {e}")))?;
            Ok(json!({
                "collection": collection,
                "revision": row.revision,
                "access": access.as_str(),
            }))
        })
        .map(Json)
}

#[derive(Deserialize)]
pub struct PutRequest {
    pub base_revision: i64,
    pub collection: Collection,
}

/// `PUT /api/v1/collections/{id} {base_revision, collection}` → `{revision}` |
/// 409 `{server_revision}` | 403 (read-only or revoked).
pub async fn update(
    State(state): State<SharedState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Json(body): Json<PutRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .with_db(|conn| -> ApiResult<serde_json::Value> {
            let row = fetch_collection(conn, &id)
                .map_err(ApiError::from)?
                .ok_or_else(|| ApiError::not_found("collection not found"))?;
            let access = access_for(conn, &user.id, &row).ok_or_else(|| {
                ApiError::access_denied("you do not have access to this collection")
            })?;
            if access < Access::Full {
                return Err(ApiError::access_denied(
                    "this collection is read-only for you",
                ));
            }
            if body.collection.id != id {
                return Err(ApiError::bad_request(
                    "`collection.id` does not match the URL",
                ));
            }
            if body.base_revision != row.revision {
                return Err(ApiError::Conflict {
                    message: format!(
                        "stale base_revision: server is at revision {} (you sent {})",
                        row.revision, body.base_revision
                    ),
                    server_revision: Some(row.revision),
                });
            }

            let data = serde_json::to_string(&body.collection).map_err(|e| {
                ApiError::bad_request(format!("collection serialization failed: {e}"))
            })?;
            let now = restrust_core::model::now_rfc3339();
            conn.execute(
            "UPDATE collections SET data = ?1, revision = revision + 1, updated_at = ?2, name = ?3
             WHERE id = ?4",
            params![data, now, body.collection.name, id],
        )
        .map_err(ApiError::from)?;
            Ok(json!({"revision": row.revision + 1}))
        })
        .map(Json)
}

/// `DELETE /api/v1/collections/{id}` — owner only (personal) / org admin.
/// A full-access collaborator gets 403 `access_denied` (AC-09).
pub async fn delete_collection(
    State(state): State<SharedState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state.with_db(|conn| -> ApiResult<()> {
        let row = fetch_collection(conn, &id)
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found("collection not found"))?;
        let access = access_for(conn, &user.id, &row)
            .ok_or_else(|| ApiError::access_denied("you do not have access to this collection"))?;
        if access < Access::Owner {
            return Err(ApiError::access_denied(
                "only the owner can delete this collection",
            ));
        }
        conn.execute("UPDATE collections SET deleted = 1 WHERE id = ?1", [&id])
            .map_err(ApiError::from)?;
        conn.execute("DELETE FROM shares WHERE collection_id = ?1", [&id])
            .map_err(ApiError::from)?;
        conn.execute("DELETE FROM publications WHERE collection_id = ?1", [&id])
            .map_err(ApiError::from)?;
        Ok(())
    })?;
    Ok(Json(json!({"ok": true})))
}

// --- shares -----------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SharesRequest {
    pub grants: Vec<GrantIn>,
}

#[derive(Deserialize)]
pub struct GrantIn {
    pub subject: SubjectIn,
    pub level: String,
}

#[derive(Deserialize)]
pub struct SubjectIn {
    pub kind: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
}

/// `PUT /api/v1/collections/{id}/shares` — owner only; replaces the grant list.
pub async fn put_shares(
    State(state): State<SharedState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Json(body): Json<SharesRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    state.with_db(|conn| -> ApiResult<serde_json::Value> {
        let row = fetch_collection(conn, &id)
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found("collection not found"))?;
        let access = access_for(conn, &user.id, &row).ok_or_else(|| {
            ApiError::access_denied("you do not have access to this collection")
        })?;
        if access < Access::Owner {
            return Err(ApiError::access_denied(
                "only the owner can change sharing",
            ));
        }

        conn.execute("DELETE FROM shares WHERE collection_id = ?1", [&id])
            .map_err(ApiError::from)?;

        let mut granted = Vec::new();
        for (i, grant) in body.grants.iter().enumerate() {
            let level = match grant.level.as_str() {
                "read" => "read",
                "full" => "full",
                other => {
                    return Err(ApiError::bad_request(format!(
                        "grants[{i}].level must be \"read\" or \"full\", got {other:?}"
                    )))
                }
            };
            let kind = grant.subject.kind.clone().unwrap_or_default();
            match kind.as_str() {
                "user" => {
                    let target = resolve_user(conn, &grant.subject)?;
                    conn.execute(
                        "INSERT OR REPLACE INTO shares (collection_id, subject_kind, subject_id, level)
                         VALUES (?1, 'user', ?2, ?3)",
                        params![id, target.0, level],
                    )
                    .map_err(ApiError::from)?;
                    granted.push(json!({
                        "subject": {"kind": "user", "id": target.0, "name": target.1},
                        "level": level,
                    }));
                }
                "org" | "organization" => {
                    let (org_id, org_name) = resolve_org(conn, &grant.subject)?;
                    conn.execute(
                        "INSERT OR REPLACE INTO shares (collection_id, subject_kind, subject_id, level)
                         VALUES (?1, 'org', ?2, ?3)",
                        params![id, org_id, level],
                    )
                    .map_err(ApiError::from)?;
                    granted.push(json!({
                        "subject": {"kind": "org", "id": org_id, "name": org_name},
                        "level": level,
                    }));
                }
                "public" => {
                    conn.execute(
                        "INSERT OR REPLACE INTO shares (collection_id, subject_kind, subject_id, level)
                         VALUES (?1, 'public', '', ?2)",
                        params![id, level],
                    )
                    .map_err(ApiError::from)?;
                    granted.push(json!({"subject": {"kind": "public"}, "level": level}));
                }
                other => {
                    return Err(ApiError::bad_request(format!(
                        "grants[{i}].subject.kind must be \"user\", \"org\" or \"public\", got {other:?}"
                    )))
                }
            }
        }
        Ok(json!({"grants": granted}))
    })
    .map(Json)
}

fn resolve_user(conn: &Connection, subject: &SubjectIn) -> ApiResult<(String, String)> {
    let found: Option<(String, String)> = conn
        .query_row(
            "SELECT id, username FROM users WHERE id = ?1 OR username = ?2",
            params![
                subject.id.clone().unwrap_or_default(),
                subject.name.clone().unwrap_or_default()
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    found.ok_or_else(|| ApiError::bad_request("share subject user not found"))
}

fn resolve_org(conn: &Connection, subject: &SubjectIn) -> ApiResult<(String, String)> {
    let found: Option<(String, String)> = conn
        .query_row(
            "SELECT id, name FROM organizations WHERE id = ?1 OR name = ?2",
            params![
                subject.id.clone().unwrap_or_default(),
                subject.name.clone().unwrap_or_default()
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    found.ok_or_else(|| ApiError::bad_request("share subject organization not found"))
}

// --- publish ----------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PublishRequest {
    pub slug: Option<String>,
}

/// `POST /api/v1/collections/{id}/publish {slug?}` → `{slug}` — owner only.
pub async fn publish(
    State(state): State<SharedState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    body: Option<Json<PublishRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    let requested_slug = body.and_then(|Json(b)| b.slug);
    state.with_db(|conn| -> ApiResult<serde_json::Value> {
        let row = fetch_collection(conn, &id)
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found("collection not found"))?;
        let access = access_for(conn, &user.id, &row).ok_or_else(|| {
            ApiError::access_denied("you do not have access to this collection")
        })?;
        if access < Access::Owner {
            return Err(ApiError::access_denied("only the owner can publish"));
        }

        let base = requested_slug
            .as_deref()
            .map(slugify)
            .unwrap_or_else(|| slugify(&row.name));
        if base.is_empty() {
            return Err(ApiError::bad_request(
                "cannot derive a slug; provide an explicit `slug`",
            ));
        }
        let slug = uniquify_slug(conn, &base, &id)?;
        let now = restrust_core::model::now_rfc3339();
        conn.execute(
            "INSERT INTO publications (collection_id, slug, published_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(collection_id) DO UPDATE SET slug = excluded.slug, published_at = excluded.published_at",
            params![id, slug, now],
        )
        .map_err(ApiError::from)?;
        Ok(json!({"slug": slug}))
    })
    .map(Json)
}

/// `DELETE /api/v1/collections/{id}/publish` — owner only.
pub async fn unpublish(
    State(state): State<SharedState>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    state.with_db(|conn| -> ApiResult<()> {
        let row = fetch_collection(conn, &id)
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found("collection not found"))?;
        let access = access_for(conn, &user.id, &row)
            .ok_or_else(|| ApiError::access_denied("you do not have access to this collection"))?;
        if access < Access::Owner {
            return Err(ApiError::access_denied("only the owner can unpublish"));
        }
        let removed = conn
            .execute("DELETE FROM publications WHERE collection_id = ?1", [&id])
            .map_err(ApiError::from)?;
        if removed == 0 {
            return Err(ApiError::not_found("collection is not published"));
        }
        Ok(())
    })?;
    Ok(Json(json!({"ok": true})))
}

/// Lowercase, alnum-and-dash slug (percent-encoding safe ASCII only).
pub fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !slug.is_empty() {
            slug.push('-');
            dash = true;
        }
    }
    slug.trim_end_matches('-').to_string()
}

fn uniquify_slug(conn: &Connection, base: &str, collection_id: &str) -> ApiResult<String> {
    for n in 1..1000 {
        let candidate = if n == 1 {
            base.to_string()
        } else {
            format!("{base}-{n}")
        };
        let taken_by_other: Option<String> = conn
            .query_row(
                "SELECT collection_id FROM publications WHERE slug = ?1",
                [&candidate],
                |r| r.get(0),
            )
            .ok();
        let free = match taken_by_other {
            None => true,
            Some(owner) => owner == collection_id,
        };
        if free {
            return Ok(candidate);
        }
    }
    Err(ApiError::internal("could not derive a unique slug"))
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/collections", get(list).post(create))
        .route(
            "/collections/{id}",
            get(get_one).put(update).delete(delete_collection),
        )
        .route("/collections/{id}/shares", put(put_shares))
        .route("/collections/{id}/publish", post(publish).delete(unpublish))
}
