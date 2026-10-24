//! Typed REST client for the restrust server (CONTRACT §4.4).
//!
//! Two halves, both unit-testable without a network: request *builders* that
//! produce [`ehttp::Request`]s (URL, bearer header, JSON body) and response
//! *parsers* for the server's JSON shapes, including the
//! `{"error":{"code","message"}}` envelope.

use restrust_core::model::Collection;
use restrust_core::share::ShareGrant;
use serde::Deserialize;

/// The server's API prefix.
pub const API_ROOT: &str = "/api/v1";

pub fn execute(base: &str, token: &str, request: ehttp::Request) -> ehttp::Request {
    let payload = restrust_core::execution::ExecuteRequest {
        method: request.method.as_str().into(),
        url: request.url,
        headers: request.headers.headers,
        body: request.body,
    };
    json_request(
        "POST",
        join_url(base, API_ROOT) + "/execute",
        Some(token),
        serde_json::to_string(&payload).expect("execution JSON"),
    )
}

pub fn parse_execution(
    result: &Result<ehttp::Response, String>,
) -> Result<(ehttp::Response, u64), ApiError> {
    parse_ok(result, |response| {
        let value: restrust_core::execution::ExecuteResponse =
            deser(response.text().unwrap_or(""))?;
        Ok((
            ehttp::Response {
                url: value.url,
                ok: (200..300).contains(&value.status),
                status: value.status,
                status_text: value.reason,
                headers: ehttp::Headers {
                    headers: value.headers,
                },
                bytes: value.body,
            },
            value.duration_ms,
        ))
    })
}

/// Errors from talking to the server.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    /// Server responded with the error envelope.
    #[error("{code}: {message}")]
    Server {
        status: u16,
        code: String,
        message: String,
    },
    /// 409 sync conflict; carries the server's current revision.
    #[error("conflict: server revision {server_revision}")]
    Conflict { server_revision: u64 },
    /// 401 — session expired (never revocation).
    #[error("unauthorized: session expired")]
    Unauthorized,
    /// Confirmed 403 `access_denied` (the only revocation trigger).
    #[error("access denied")]
    AccessDenied,
    /// Transport failure (DNS, refused, timeout, CORS…).
    #[error("{0}")]
    Transport(String),
    /// Response body could not be parsed.
    #[error("bad response: {0}")]
    BadResponse(String),
}

impl ApiError {
    /// The server's verbatim message when present (for exact-error policy).
    pub fn server_message(&self) -> String {
        match self {
            ApiError::Server { message, .. } => message.clone(),
            other => other.to_string(),
        }
    }
}

// --- DTOs (snake_case per the server wire format) -------------------------------

fn deser<'a, T: Deserialize<'a>>(body: &'a str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|e| ApiError::BadResponse(e.to_string()))
}

/// An auth provider advertised by `GET /auth/providers`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProviderDto {
    pub id: String,
    pub name: String,
    pub kind: String,
}

/// The signed-in user.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct SessionUser {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub system_role: String,
}

/// An organization the user belongs to.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SessionOrg {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub role: String,
}

/// `POST /auth/login` response.
#[derive(Debug, Clone, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: SessionUser,
}

/// `GET /auth/session` response.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionResponse {
    pub user: SessionUser,
    #[serde(default)]
    pub organizations: Vec<SessionOrg>,
}

/// One row of `GET /collections`.
#[derive(Debug, Clone, Deserialize)]
pub struct CollectionListEntry {
    pub id: String,
    pub name: String,
    pub namespace: NamespaceDto,
    pub revision: u64,
    #[serde(default)]
    pub access: String,
    #[serde(default)]
    pub synced_at: Option<String>,
}

/// Namespace of a server collection.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct NamespaceDto {
    pub kind: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// `GET /collections/{id}` response.
#[derive(Debug, Clone, Deserialize)]
pub struct CollectionGetResponse {
    pub collection: Collection,
    pub revision: u64,
    #[serde(default)]
    pub access: String,
}

/// `PUT /collections/{id}` / `POST /collections` response.
#[derive(Debug, Clone, Deserialize)]
pub struct RevisionResponse {
    pub revision: u64,
}

/// `POST /collections/{id}/publish` response.
#[derive(Debug, Clone, Deserialize)]
pub struct PublishResponse {
    pub slug: String,
}

/// Admin user row.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AdminUserDto {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub system_role: String,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub created_at: String,
}

/// Admin organization row.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AdminOrgDto {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub members: Vec<AdminOrgMember>,
}

/// One member of an organization.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AdminOrgMember {
    #[serde(default)]
    pub user_id: String,
    pub username: String,
    #[serde(default)]
    pub role: String,
}

// --- request builders ------------------------------------------------------------

fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn json_request(method: &str, url: String, token: Option<&str>, body: String) -> ehttp::Request {
    let headers = ehttp::Headers::new(&[
        ("Content-Type", "application/json"),
        ("Accept", "application/json"),
    ]);
    let mut req = ehttp::Request {
        method: ehttp::Method::parse(method).expect("constant method"),
        url,
        body: body.into_bytes(),
        headers,
        ..ehttp::Request::get("about:blank")
    };
    if let Some(token) = token {
        req.headers
            .insert("Authorization", format!("Bearer {token}"));
    }
    req
}

fn get_request(url: String, token: Option<&str>) -> ehttp::Request {
    let mut req = ehttp::Request::get(url);
    if let Some(token) = token {
        req.headers
            .insert("Authorization", format!("Bearer {token}"));
    }
    req
}

fn delete_request(url: String, token: Option<&str>) -> ehttp::Request {
    let mut req = ehttp::Request::delete(&url);
    if let Some(token) = token {
        req.headers
            .insert("Authorization", format!("Bearer {token}"));
    }
    req
}

/// `POST /auth/login`.
pub fn login(base: &str, username: &str, password: &str) -> ehttp::Request {
    let body = serde_json::json!({ "username": username, "password": password }).to_string();
    json_request("POST", join_url(base, API_ROOT) + "/auth/login", None, body)
}

/// `GET /auth/session`.
pub fn session(base: &str, token: &str) -> ehttp::Request {
    get_request(join_url(base, API_ROOT) + "/auth/session", Some(token))
}

/// `POST /auth/logout`.
pub fn logout(base: &str, token: &str) -> ehttp::Request {
    json_request(
        "POST",
        join_url(base, API_ROOT) + "/auth/logout",
        Some(token),
        "{}".to_string(),
    )
}

/// `POST /collections` response: `{id, revision}`.
pub fn parse_create(body: &str) -> Result<(String, u64), ApiError> {
    #[derive(Deserialize)]
    struct CreateResponse {
        pub id: String,
        pub revision: u64,
    }
    deser::<CreateResponse>(body).map(|r| (r.id, r.revision))
}

/// `GET /auth/providers`.
pub fn providers(base: &str) -> ehttp::Request {
    get_request(join_url(base, API_ROOT) + "/auth/providers", None)
}

/// `GET /collections`.
pub fn collections_list(base: &str, token: &str) -> ehttp::Request {
    get_request(join_url(base, API_ROOT) + "/collections", Some(token))
}

/// `GET /collections/{id}`.
pub fn collection_get(base: &str, token: &str, id: &str) -> ehttp::Request {
    get_request(join_url(base, API_ROOT) + "/collections/" + id, Some(token))
}

/// `POST /collections` (enable-sync for a local collection; creates it).
pub fn collection_create(base: &str, token: &str, name: &str) -> ehttp::Request {
    let body = serde_json::json!({ "name": name }).to_string();
    json_request(
        "POST",
        join_url(base, API_ROOT) + "/collections",
        Some(token),
        body,
    )
}

/// `PUT /collections/{id}` with a base revision (force = server revision after
/// a 409 "overwrite" choice).
pub fn collection_put(
    base: &str,
    token: &str,
    id: &str,
    base_revision: u64,
    collection: &Collection,
) -> ehttp::Request {
    let mut collection = collection.clone();
    collection.id = id.to_string();
    let body = serde_json::json!({
        "base_revision": base_revision,
        "collection": serde_json::to_value(&collection).unwrap_or(serde_json::Value::Null),
    })
    .to_string();
    json_request(
        "PUT",
        join_url(base, API_ROOT) + "/collections/" + id,
        Some(token),
        body,
    )
}

/// `DELETE /collections/{id}`.
pub fn collection_delete(base: &str, token: &str, id: &str) -> ehttp::Request {
    delete_request(join_url(base, API_ROOT) + "/collections/" + id, Some(token))
}

/// `GET /collections/{id}/revisions` — the git-style change log.
pub fn collection_revisions(base: &str, token: &str, id: &str) -> ehttp::Request {
    get_request(
        join_url(base, API_ROOT) + "/collections/" + id + "/revisions",
        Some(token),
    )
}

/// One row of `GET /collections/{id}/revisions` (server shape; the change
/// records reuse the core wire format).
#[derive(Debug, Clone, Deserialize)]
pub struct ServerRevisionDto {
    pub revision: u64,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub content_hash: u64,
    #[serde(default)]
    pub changes: Vec<restrust_core::revision::ChangeRecord>,
}

/// `GET /collections/{id}/revisions` response.
pub fn parse_collection_revisions(
    body: &str,
) -> Result<Vec<restrust_core::revision::CollectionRevision>, ApiError> {
    #[derive(Deserialize)]
    struct RevisionsResponse {
        #[serde(default)]
        revisions: Vec<ServerRevisionDto>,
    }
    let response = deser::<RevisionsResponse>(body)?;
    Ok(response
        .revisions
        .into_iter()
        .map(|dto| restrust_core::revision::CollectionRevision {
            revision: dto.revision,
            content_hash: dto.content_hash,
            time: dto.time,
            author: dto.author,
            summary: dto.summary,
            changes: dto.changes,
        })
        .collect())
}

/// Converts a core grant to the server wire shape (§4.4):
/// `{subject:{kind, id?, name?}, level:"read"|"full"}`.
fn grant_to_wire(grant: &ShareGrant) -> serde_json::Value {
    use restrust_core::share::{AccessLevel, ShareSubject};
    let (kind, id, name) = match &grant.subject {
        ShareSubject::User { id, username } => ("user", Some(id.clone()), Some(username.clone())),
        ShareSubject::Organization { id, name } => ("org", Some(id.clone()), Some(name.clone())),
        ShareSubject::Public => ("public", None, None),
    };
    let mut subject = serde_json::json!({ "kind": kind });
    if let Some(id) = id {
        subject["id"] = serde_json::json!(id);
    }
    if let Some(name) = name {
        subject["name"] = serde_json::json!(name);
    }
    serde_json::json!({
        "subject": subject,
        "level": match grant.level {
            AccessLevel::ReadOnly => "read",
            AccessLevel::FullAccess => "full",
        }
    })
}

/// `PUT /collections/{id}/shares` with the full replacement grant list.
pub fn shares_put(base: &str, token: &str, id: &str, grants: &[ShareGrant]) -> ehttp::Request {
    let body = serde_json::json!({
        "grants": grants.iter().map(grant_to_wire).collect::<Vec<_>>(),
    })
    .to_string();
    json_request(
        "PUT",
        join_url(base, API_ROOT) + "/collections/" + id + "/shares",
        Some(token),
        body,
    )
}

/// `POST /collections/{id}/publish`.
pub fn publish(base: &str, token: &str, id: &str, slug: Option<&str>) -> ehttp::Request {
    let body = serde_json::json!({ "slug": slug }).to_string();
    json_request(
        "POST",
        join_url(base, API_ROOT) + "/collections/" + id + "/publish",
        Some(token),
        body,
    )
}

/// `DELETE /collections/{id}/publish`.
pub fn unpublish(base: &str, token: &str, id: &str) -> ehttp::Request {
    delete_request(
        join_url(base, API_ROOT) + "/collections/" + id + "/publish",
        Some(token),
    )
}

/// `GET /admin/users`.
pub fn admin_users_list(base: &str, token: &str) -> ehttp::Request {
    get_request(join_url(base, API_ROOT) + "/admin/users", Some(token))
}

/// `POST /admin/users`.
pub fn admin_user_create(
    base: &str,
    token: &str,
    username: &str,
    password: &str,
    system_role: &str,
) -> ehttp::Request {
    let body = serde_json::json!({
        "username": username,
        "password": password,
        "system_role": system_role,
    })
    .to_string();
    json_request(
        "POST",
        join_url(base, API_ROOT) + "/admin/users",
        Some(token),
        body,
    )
}

/// `PATCH /admin/users/{id}` (only provided fields are sent).
pub fn admin_user_patch(
    base: &str,
    token: &str,
    id: &str,
    system_role: Option<&str>,
    disabled: Option<bool>,
    password: Option<&str>,
) -> ehttp::Request {
    let mut body = serde_json::Map::new();
    if let Some(role) = system_role {
        body.insert("system_role".into(), serde_json::json!(role));
    }
    if let Some(disabled) = disabled {
        body.insert("disabled".into(), serde_json::json!(disabled));
    }
    if let Some(password) = password {
        body.insert("password".into(), serde_json::json!(password));
    }
    json_request(
        "PATCH",
        join_url(base, API_ROOT) + "/admin/users/" + id,
        Some(token),
        serde_json::Value::Object(body).to_string(),
    )
}

/// `GET /admin/organizations`.
pub fn admin_orgs_list(base: &str, token: &str) -> ehttp::Request {
    get_request(
        join_url(base, API_ROOT) + "/admin/organizations",
        Some(token),
    )
}

/// `POST /admin/organizations`.
pub fn admin_org_create(base: &str, token: &str, name: &str) -> ehttp::Request {
    let body = serde_json::json!({ "name": name }).to_string();
    json_request(
        "POST",
        join_url(base, API_ROOT) + "/admin/organizations",
        Some(token),
        body,
    )
}

/// `PUT /admin/organizations/{id}/members` with the full replacement list.
pub fn admin_org_members_put(
    base: &str,
    token: &str,
    org_id: &str,
    members: &[(String, String)], // (username, role)
) -> ehttp::Request {
    let members: Vec<serde_json::Value> = members
        .iter()
        .map(|(username, role)| serde_json::json!({ "username": username, "role": role }))
        .collect();
    let body = serde_json::json!({ "members": members }).to_string();
    json_request(
        "PUT",
        join_url(base, API_ROOT) + "/admin/organizations/" + org_id + "/members",
        Some(token),
        body,
    )
}

/// Absolute URL that starts the OIDC flow for a provider.
pub fn oidc_start_url(base: &str, provider: &str) -> String {
    format!("{}/auth/oidc/{provider}/start", join_url(base, API_ROOT))
}

/// Public web URL of a published collection.
pub fn public_url(base: &str, slug: &str) -> String {
    format!("{}/public/{slug}", join_url(base, API_ROOT))
}

// --- response parsing ------------------------------------------------------------

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

/// Parses an error status + body into a typed [`ApiError`]. `access_denied`
/// requires a *confirmed* 403 to matter, so this returns [`ApiError::AccessDenied`]
/// only for exactly that combination (the revocation guardrail).
pub fn parse_error(status: u16, body: &str) -> ApiError {
    let envelope = serde_json::from_str::<ErrorEnvelope>(body).ok();
    let (code, message) = match envelope {
        Some(ErrorEnvelope { error }) => (error.code, error.message),
        None => (String::new(), body.chars().take(200).collect()),
    };
    if status == 401 {
        return ApiError::Unauthorized;
    }
    if status == 403 && code == "access_denied" {
        return ApiError::AccessDenied;
    }
    if status == 409 {
        // The conflict body carries the server's current revision.
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
            let server_revision = value
                .get("server_revision")
                .and_then(|v| v.as_u64())
                .or_else(|| {
                    value
                        .get("error")
                        .and_then(|e| e.get("server_revision"))
                        .and_then(|v| v.as_u64())
                });
            if let Some(server_revision) = server_revision {
                return ApiError::Conflict { server_revision };
            }
        }
    }
    if code.is_empty() && message.is_empty() {
        return ApiError::Server {
            status,
            code: String::new(),
            message: format!("HTTP {status}"),
        };
    }
    ApiError::Server {
        status,
        code,
        message,
    }
}

/// Turns a raw HTTP result into a parse outcome: transport failures become
/// [`ApiError::Transport`], 2xx bodies are parsed by `f`.
pub fn parse_ok<T>(
    result: &Result<ehttp::Response, String>,
    f: impl FnOnce(&ehttp::Response) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    match result {
        Ok(resp) if (200..300).contains(&resp.status) => f(resp),
        Ok(resp) => Err(parse_error(resp.status, resp.text().unwrap_or(""))),
        Err(err) => Err(ApiError::Transport(err.clone())),
    }
}

/// `POST /auth/login` response.
pub fn parse_login(body: &str) -> Result<LoginResponse, ApiError> {
    deser(body)
}

/// `GET /auth/session` response.
pub fn parse_session(body: &str) -> Result<SessionResponse, ApiError> {
    deser(body)
}

/// `GET /auth/providers` response.
pub fn parse_providers(body: &str) -> Result<Vec<ProviderDto>, ApiError> {
    deser(body)
}

/// `GET /collections` response.
pub fn parse_collection_list(body: &str) -> Result<Vec<CollectionListEntry>, ApiError> {
    deser(body)
}

/// `GET /collections/{id}` response.
pub fn parse_collection_get(body: &str) -> Result<CollectionGetResponse, ApiError> {
    deser(body)
}

/// `{revision}` responses (PUT / POST).
pub fn parse_revision(body: &str) -> Result<u64, ApiError> {
    deser::<RevisionResponse>(body).map(|r| r.revision)
}

/// `POST …/publish` response.
pub fn parse_publish(body: &str) -> Result<String, ApiError> {
    deser::<PublishResponse>(body).map(|r| r.slug)
}

/// `GET /admin/users` response.
pub fn parse_admin_users(body: &str) -> Result<Vec<AdminUserDto>, ApiError> {
    deser(body)
}

/// `GET /admin/organizations` response.
pub fn parse_admin_orgs(body: &str) -> Result<Vec<AdminOrgDto>, ApiError> {
    deser(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_request_shape() {
        let req = login("https://srv.test/", "alice", "pw");
        assert_eq!(req.method.as_str(), "POST");
        assert_eq!(req.url, "https://srv.test/api/v1/auth/login");
        assert_eq!(req.headers.get("content-type"), Some("application/json"));
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["username"], "alice");
        assert_eq!(body["password"], "pw");
        assert!(req.headers.get("authorization").is_none());
    }

    #[test]
    fn authed_requests_carry_bearer_token() {
        let req = collection_get("https://srv.test", "tok-1", "abc");
        assert_eq!(req.url, "https://srv.test/api/v1/collections/abc");
        assert_eq!(req.headers.get("authorization"), Some("Bearer tok-1"));

        let req = collections_list("http://srv.test/", "tok-2");
        assert_eq!(req.url, "http://srv.test/api/v1/collections");
        assert_eq!(req.headers.get("authorization"), Some("Bearer tok-2"));
    }

    #[test]
    fn collection_put_carries_base_revision_and_camel_case_model() {
        let collection = restrust_core::model::Collection::new("Demo");
        let req = collection_put("https://s", "t", "c1", 7, &collection);
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["base_revision"], 7);
        // Core model is embedded camelCase.
        assert_eq!(body["collection"]["name"], "Demo");
    }

    #[test]
    fn shares_put_sends_grants_array() {
        let grants = vec![restrust_core::share::ShareGrant {
            subject: restrust_core::share::ShareSubject::Public,
            level: restrust_core::share::AccessLevel::ReadOnly,
        }];
        let req = shares_put("https://s", "t", "c1", &grants);
        assert_eq!(req.url, "https://s/api/v1/collections/c1/shares");
        assert_eq!(req.method.as_str(), "PUT");
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["grants"][0]["subject"]["kind"], "public");
        assert_eq!(body["grants"][0]["level"], "read");
    }

    #[test]
    fn admin_member_put_replaces_list() {
        let req = admin_org_members_put(
            "https://s",
            "t",
            "o1",
            &[
                ("alice".into(), "admin".into()),
                ("bob".into(), "viewer".into()),
            ],
        );
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["members"].as_array().unwrap().len(), 2);
        assert_eq!(body["members"][0]["username"], "alice");
        assert_eq!(body["members"][1]["role"], "viewer");
    }

    #[test]
    fn parse_login_and_session() {
        let login = parse_login(
            r#"{"token":"abc","user":{"id":"u1","username":"alice","display_name":"Alice","system_role":"user"}}"#,
        )
        .unwrap();
        assert_eq!(login.token, "abc");
        assert_eq!(login.user.username, "alice");

        let session = parse_session(
            r#"{"user":{"id":"u1","username":"alice","system_role":"admin"},"organizations":[{"id":"o1","name":"Acme","role":"editor"}]}"#,
        )
        .unwrap();
        assert_eq!(session.user.system_role, "admin");
        assert_eq!(session.organizations.len(), 1);
        assert_eq!(session.organizations[0].name, "Acme");
    }

    #[test]
    fn parse_collection_shapes() {
        let list = parse_collection_list(
            r#"[{"id":"c1","name":"API","namespace":{"kind":"organization","id":"o1","name":"Acme"},"revision":3,"access":"read","synced_at":"2026-10-20T10:00:00Z"}]"#,
        )
        .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].namespace.kind, "organization");
        assert_eq!(list[0].revision, 3);

        let collection = restrust_core::model::Collection::new("API");
        let get = parse_collection_get(
            &serde_json::json!({
                "collection": collection,
                "revision": 5,
                "access": "full"
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(get.revision, 5);
        assert_eq!(get.collection.name, "API");
        assert_eq!(get.access, "full");

        assert_eq!(parse_revision(r#"{"revision":9}"#).unwrap(), 9);
        assert_eq!(parse_publish(r#"{"slug":"demo"}"#).unwrap(), "demo");
    }

    #[test]
    fn parse_admin_shapes() {
        let users = parse_admin_users(
            r#"[{"id":"u1","username":"alice","display_name":"Alice Brown","system_role":"admin","disabled":false,"created_at":"2026-10-02"}]"#,
        )
        .unwrap();
        assert_eq!(users[0].system_role, "admin");
        assert!(!users[0].disabled);

        let orgs = parse_admin_orgs(
            r#"[{"id":"o1","name":"Acme","members":[{"user_id":"u1","username":"alice","role":"admin"}]}]"#,
        )
        .unwrap();
        assert_eq!(orgs[0].members[0].username, "alice");
    }

    #[test]
    fn error_envelope_maps_to_typed_errors() {
        match parse_error(
            401,
            r#"{"error":{"code":"unauthorized","message":"bad token"}}"#,
        ) {
            ApiError::Unauthorized => {}
            other => panic!("expected Unauthorized, got {other:?}"),
        }
        match parse_error(403, r#"{"error":{"code":"access_denied","message":"no"}}"#) {
            ApiError::AccessDenied => {}
            other => panic!("expected AccessDenied, got {other:?}"),
        }
        // A 403 with a different code must NOT read as revocation-confirmed.
        assert!(matches!(
            parse_error(403, r#"{"error":{"code":"internal","message":"boom"}}"#),
            ApiError::Server { status: 403, .. }
        ));
        match parse_error(
            409,
            r#"{"error":{"code":"conflict","message":"stale","server_revision":4}}"#,
        ) {
            ApiError::Conflict { server_revision: 4 } => {}
            other => panic!("expected Conflict, got {other:?}"),
        }
        match parse_error(
            400,
            r#"{"error":{"code":"bad_request","message":"name required"}}"#,
        ) {
            ApiError::Server {
                status: 400,
                code,
                message,
            } => {
                assert_eq!(
                    (code.as_str(), message.as_str()),
                    ("bad_request", "name required")
                );
            }
            other => panic!("expected Server, got {other:?}"),
        }
        // Non-JSON body still produces something readable.
        assert!(matches!(
            parse_error(500, "Internal Server Error"),
            ApiError::Server { status: 500, .. }
        ));
    }

    #[test]
    fn parse_ok_routes_transport_and_http_errors() {
        let resp = ehttp::Response {
            url: "https://x".into(),
            ok: true,
            status: 200,
            status_text: "OK".into(),
            headers: ehttp::Headers::default(),
            bytes: br#"{"revision":2}"#.to_vec(),
        };
        assert_eq!(
            parse_ok(&Ok(resp), |r| parse_revision(r.text().unwrap())).unwrap(),
            2
        );

        let resp = ehttp::Response {
            url: "https://x".into(),
            ok: false,
            status: 401,
            status_text: "Unauthorized".into(),
            headers: ehttp::Headers::default(),
            bytes: Vec::new(),
        };
        assert!(matches!(
            parse_ok(&Ok(resp), parse_revision_from),
            Err(ApiError::Unauthorized)
        ));

        assert!(matches!(
            parse_ok(&Err("dns failure".to_string()), parse_revision_from),
            Err(ApiError::Transport(_))
        ));
    }

    fn parse_revision_from(r: &ehttp::Response) -> Result<u64, ApiError> {
        parse_revision(r.text().unwrap_or(""))
    }

    #[test]
    fn url_helpers() {
        assert_eq!(
            oidc_start_url("https://s", "google"),
            "https://s/api/v1/auth/oidc/google/start"
        );
        assert_eq!(
            public_url("https://s/", "demo"),
            "https://s/api/v1/public/demo"
        );
    }
}
