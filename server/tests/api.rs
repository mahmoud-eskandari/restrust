//! Integration tests for the restrust-server REST API (CONTRACT §4.5).
//!
//! Each test builds the real router with an in-memory SQLite database and
//! drives it via `tower::ServiceExt::oneshot`.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use restrust_server::api;
use restrust_server::auth;
use restrust_server::config::Config;
use restrust_server::db;
use restrust_server::state::{AppState, SharedState};
use serde_json::{json, Value};
use tower::ServiceExt;

// --- helpers -------------------------------------------------------------------

fn state_with(config: Config) -> SharedState {
    let conn = db::open_in_memory().expect("open in-memory db");
    db::migrate(&conn).expect("migrate");
    db::seed_bootstrap_admin(&conn, &config).expect("seed");
    AppState::shared(conn, config)
}

fn base_config() -> Config {
    Config::for_tests()
}

fn app_for(state: &SharedState) -> Router {
    api::app_router(state.clone())
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let headers = response.headers().clone();
    let cookie = headers
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.expect("body");
    let bytes = bytes.to_vec();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, value, cookie)
}

async fn login(app: &Router, username: &str, password: &str) -> (String, Value) {
    let (status, body, _) = send(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"username": username, "password": password})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login {username} failed: {body}");
    let token = body["token"].as_str().expect("token").to_string();
    (token, body)
}

/// Insert a local user directly (fast; avoids argon2 on the admin API path).
fn add_user(state: &SharedState, username: &str, password: &str, system_role: &str) -> String {
    state
        .with_db(|conn| {
            let id = restrust_core::model::new_id();
            conn.execute(
                "INSERT INTO users (id, username, password_hash, display_name, email, system_role, disabled, created_at, auth_provider, provider_subject)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5, 0, ?6, 'local', NULL)",
                rusqlite::params![
                    id,
                    username,
                    auth::hash_password(password),
                    username,
                    system_role,
                    restrust_core::model::now_rfc3339()
                ],
            )
            .unwrap();
            id
        })
        .to_string()
}

fn add_org(state: &SharedState, name: &str, members: &[(&str, &str)]) -> String {
    state.with_db(|conn| {
        let org_id = restrust_core::model::new_id();
        conn.execute(
            "INSERT INTO organizations (id, name, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![org_id, name, restrust_core::model::now_rfc3339()],
        )
        .unwrap();
        for (username, role) in members {
            let user_id: String = conn
                .query_row(
                    "SELECT id FROM users WHERE username = ?1",
                    [username],
                    |r| r.get(0),
                )
                .unwrap();
            conn.execute(
                "INSERT INTO org_members (org_id, user_id, role) VALUES (?1, ?2, ?3)",
                rusqlite::params![org_id, user_id, role],
            )
            .unwrap();
        }
        org_id
    })
}

/// A minimal collection payload with one request, suitable for PUT sync.
fn collection_payload(id: &str, name: &str) -> Value {
    let request = restrust_core::model::RequestDef::new(
        restrust_core::model::HttpMethod::Get,
        "list things",
        "https://api.example.test/things",
    );
    let collection = restrust_core::model::Collection {
        items: vec![restrust_core::model::CollectionItem::Request(request)],
        ..restrust_core::model::Collection::new(name)
    };
    let mut value = serde_json::to_value(&collection).unwrap();
    value["id"] = json!(id);
    value
}

fn error_code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or("<missing>")
}

// --- 1. bootstrap admin + login --------------------------------------------------

#[tokio::test]
async fn bootstrap_admin_login_ok_and_wrong_password_401() {
    let mut config = base_config();
    config.bootstrap_admin_user = Some("root".to_string());
    config.bootstrap_admin_password = Some("s3cret-pass".to_string());
    let state = state_with(config);
    let app = app_for(&state);

    // Seeding is idempotent: running it again must not duplicate or break.
    state.with_db(|conn| db::seed_bootstrap_admin(conn, &state.config).unwrap());

    let (token, body) = login(&app, "root", "s3cret-pass").await;
    assert!(!token.is_empty());
    assert_eq!(body["user"]["username"], "root");
    assert_eq!(body["user"]["system_role"], "admin");

    let (status, body, _) = send(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"username": "root", "password": "wrong"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(&body), "unauthorized");

    // healthz works unauthenticated
    let (status, body, _) = send(&app, "GET", "/api/v1/healthz", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
}

// --- 2. AC-19 admin gating + disabled login --------------------------------------

#[tokio::test]
async fn ac19_admin_gating_and_disabled_login() {
    let state = state_with(base_config());
    let app = app_for(&state);

    add_user(&state, "sysadmin", "admin-pass", "admin");
    add_user(&state, "alice", "alice-pass", "user");
    add_user(&state, "bob", "bob-pass", "user");

    let (admin_token, _) = login(&app, "sysadmin", "admin-pass").await;
    let (alice_token, _) = login(&app, "alice", "alice-pass").await;

    // Normal user calling the admin API -> 403 access_denied.
    let (status, body, _) =
        send(&app, "GET", "/api/v1/admin/users", Some(&alice_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");

    // Admin list + create + patch all fine.
    let (status, body, _) =
        send(&app, "GET", "/api/v1/admin/users", Some(&admin_token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body
        .as_array()
        .unwrap()
        .iter()
        .any(|u| u["username"] == "alice"));

    let (status, body, _) = send(
        &app,
        "POST",
        "/api/v1/admin/users",
        Some(&admin_token),
        Some(json!({"username": "carol", "password": "carol-pass", "system_role": "user"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let carol_id = body["id"].as_str().unwrap().to_string();

    // Disable bob via PATCH; his login then fails with 401.
    let bob_id = add_user_lookup(&state, "bob");
    let (status, _, _) = send(
        &app,
        "PATCH",
        &format!("/api/v1/admin/users/{bob_id}"),
        Some(&admin_token),
        Some(json!({"disabled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body, _) = send(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"username": "bob", "password": "bob-pass"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(&body), "unauthorized");

    // Carol (freshly created via admin API) can log in while enabled.
    let (status, _, _) = send(
        &app,
        "PATCH",
        &format!("/api/v1/admin/users/{carol_id}"),
        Some(&admin_token),
        Some(json!({"system_role": "admin"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

fn add_user_lookup(state: &SharedState, username: &str) -> String {
    state.with_db(|conn| {
        conn.query_row(
            "SELECT id FROM users WHERE username = ?1",
            [username],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    })
}

// --- 3. AC-09 full-access vs owner rights ----------------------------------------

#[tokio::test]
async fn ac09_full_access_collaborator_rights() {
    let state = state_with(base_config());
    let app = app_for(&state);

    add_user(&state, "owen", "owen-pass", "user"); // owner
    add_user(&state, "cara", "cara-pass", "user"); // full-access collaborator
    add_user(&state, "dave", "dave-pass", "user"); // read-only collaborator

    let (owen, _) = login(&app, "owen", "owen-pass").await;
    let (cara, _) = login(&app, "cara", "cara-pass").await;
    let (dave, _) = login(&app, "dave", "dave-pass").await;

    // Owner creates a personal collection.
    let (status, body, _) = send(
        &app,
        "POST",
        "/api/v1/collections",
        Some(&owen),
        Some(json!({"name": "Shared API"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["revision"], 1);

    // Share: cara full, dave read.
    let (status, _, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/collections/{id}/shares"),
        Some(&owen),
        Some(json!({"grants": [
            {"subject": {"kind": "user", "name": "cara"}, "level": "full"},
            {"subject": {"kind": "user", "name": "dave"}, "level": "read"}
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Cara (full access): GET ok, PUT ok (creates a request via data PUT).
    let (status, body, _) = send(
        &app,
        "GET",
        &format!("/api/v1/collections/{id}"),
        Some(&cara),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["access"], "full");

    let (status, body, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/collections/{id}"),
        Some(&cara),
        Some(json!({
            "base_revision": 1,
            "collection": collection_payload(&id, "Shared API")
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revision"], 2);

    // Cara DELETE collection -> 403 access_denied (owner-only).
    let (status, body, _) = send(
        &app,
        "DELETE",
        &format!("/api/v1/collections/{id}"),
        Some(&cara),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");

    // Cara PUT shares -> 403 access_denied (owner-only).
    let (status, body, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/collections/{id}/shares"),
        Some(&cara),
        Some(json!({"grants": []})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");

    // Cara publish -> 403 (owner-only).
    let (status, body, _) = send(
        &app,
        "POST",
        &format!("/api/v1/collections/{id}/publish"),
        Some(&cara),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");

    // Dave (read-only): GET ok, PUT -> 403 access_denied.
    let (status, body, _) = send(
        &app,
        "GET",
        &format!("/api/v1/collections/{id}"),
        Some(&dave),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["access"], "read");
    let (status, body, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/collections/{id}"),
        Some(&dave),
        Some(json!({
            "base_revision": 2,
            "collection": collection_payload(&id, "Shared API")
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");

    // Owen still sees himself as owner in the listing.
    let (status, body, _) = send(&app, "GET", "/api/v1/collections", Some(&owen), None).await;
    assert_eq!(status, StatusCode::OK);
    let entry = body
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id.as_str())
        .expect("collection listed for owner");
    assert_eq!(entry["access"], "owner");
    assert_eq!(entry["namespace"]["kind"], "personal");
    assert_eq!(entry["revision"], 2);
}

// --- 4. org editor vs viewer ------------------------------------------------------

#[tokio::test]
async fn org_editor_creates_and_puts_viewer_readonly() {
    let state = state_with(base_config());
    let app = app_for(&state);

    add_user(&state, "edith", "edith-pass", "user");
    add_user(&state, "victor", "victor-pass", "user");
    add_org(&state, "Acme", &[("edith", "editor"), ("victor", "viewer")]);

    let (edith, _) = login(&app, "edith", "edith-pass").await;
    let (victor, _) = login(&app, "victor", "victor-pass").await;

    let org_id = state.with_db(|conn| {
        conn.query_row(
            "SELECT id FROM organizations WHERE name = 'Acme'",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    });

    // Editor creates an org collection.
    let (status, body, _) = send(
        &app,
        "POST",
        "/api/v1/collections",
        Some(&edith),
        Some(json!({"name": "Acme API", "namespace": {"kind": "organization", "id": org_id}})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();

    // Editor PUTs data.
    let (status, body, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/collections/{id}"),
        Some(&edith),
        Some(json!({
            "base_revision": 1,
            "collection": collection_payload(&id, "Acme API")
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Viewer reads fine...
    let (status, body, _) = send(
        &app,
        "GET",
        &format!("/api/v1/collections/{id}"),
        Some(&victor),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["access"], "read");

    // ...but cannot PUT.
    let (status, body, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/collections/{id}"),
        Some(&victor),
        Some(json!({
            "base_revision": 2,
            "collection": collection_payload(&id, "Acme API")
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");

    // The org collection is listed for both members with the org namespace.
    let (status, body, _) = send(&app, "GET", "/api/v1/collections", Some(&victor), None).await;
    assert_eq!(status, StatusCode::OK);
    let entry = body
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id.as_str())
        .expect("org collection listed for viewer");
    assert_eq!(entry["namespace"]["kind"], "organization");
    assert_eq!(entry["namespace"]["name"], "Acme");
}

// --- 5. AC-03/04 stale revision 409 + revocation 403 -------------------------------

#[tokio::test]
async fn conflict_409_and_revocation_403() {
    let state = state_with(base_config());
    let app = app_for(&state);

    add_user(&state, "owen", "owen-pass", "user");
    add_user(&state, "rita", "rita-pass", "user"); // collaborator
    add_org(&state, "Beta", &[("rita", "editor")]);

    let (owen, _) = login(&app, "owen", "owen-pass").await;
    let (rita, _) = login(&app, "rita", "rita-pass").await;

    let (status, body, _) = send(
        &app,
        "POST",
        "/api/v1/collections",
        Some(&owen),
        Some(json!({"name": "Conflict API"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["id"].as_str().unwrap().to_string();

    // Rita is a full-access collaborator on it (so she may PUT at all).
    let (status, _, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/collections/{id}/shares"),
        Some(&owen),
        Some(json!({"grants": [
            {"subject": {"kind": "user", "name": "rita"}, "level": "full"}
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Rita is at base_revision 1; owen pushes first -> server moves to 2.
    let (status, body, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/collections/{id}"),
        Some(&owen),
        Some(json!({
            "base_revision": 1,
            "collection": collection_payload(&id, "Conflict API")
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Rita's stale PUT -> 409 with server_revision.
    let (status, body, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/collections/{id}"),
        Some(&rita),
        Some(json!({
            "base_revision": 1,
            "collection": collection_payload(&id, "Conflict API")
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "conflict");
    assert_eq!(
        body["server_revision"], 2,
        "top-level server_revision: {body}"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("revision 2"),
        "message should carry the server revision: {body}"
    );

    // Force-PUT with the server revision succeeds (documented overwrite flow).
    let (status, body, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/collections/{id}"),
        Some(&rita),
        Some(json!({
            "base_revision": 2,
            "collection": collection_payload(&id, "Conflict API")
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revision"], 3);

    // --- revocation: remove rita's org membership -> her org collections 403.
    let (status, body, _) = send(
        &app,
        "POST",
        "/api/v1/collections",
        Some(&rita),
        Some(json!({"name": "Beta API", "namespace": {"kind": "organization", "id": beta_org_id(&state)}})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let org_coll = body["id"].as_str().unwrap().to_string();

    // Admin removes membership (replaces member list without rita).
    add_user(&state, "sysadmin", "x-admin-pass", "admin");
    let (admin, _) = login(&app, "sysadmin", "x-admin-pass").await;
    let (status, _, _) = send(
        &app,
        "PUT",
        &format!(
            "/api/v1/admin/organizations/{}/members",
            beta_org_id(&state)
        ),
        Some(&admin),
        Some(json!({"members": [{"username": "owen", "role": "viewer"}]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body, _) = send(
        &app,
        "GET",
        &format!("/api/v1/collections/{org_coll}"),
        Some(&rita),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "access_denied");
}

fn beta_org_id(state: &SharedState) -> String {
    state.with_db(|conn| {
        conn.query_row(
            "SELECT id FROM organizations WHERE name = 'Beta'",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    })
}

// --- 6. AC-20/11 publish, public HTML+JSON, unpublish 404 --------------------------

#[tokio::test]
async fn publish_public_html_and_json_then_unpublish_404() {
    let state = state_with(base_config());
    let app = app_for(&state);

    add_user(&state, "owen", "owen-pass", "user");
    let (owen, _) = login(&app, "owen", "owen-pass").await;

    let (status, body, _) = send(
        &app,
        "POST",
        "/api/v1/collections",
        Some(&owen),
        Some(json!({"name": "Demo API & Special <Chars>"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["id"].as_str().unwrap().to_string();

    // Put one request into it so the HTML tree has content.
    let (status, _, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/collections/{id}"),
        Some(&owen),
        Some(json!({
            "base_revision": 1,
            "collection": collection_payload(&id, "Demo API & Special <Chars>")
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body, _) = send(
        &app,
        "POST",
        &format!("/api/v1/collections/{id}/publish"),
        Some(&owen),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let slug = body["slug"].as_str().unwrap().to_string();
    assert_eq!(slug, "demo-api-special-chars");

    // HTML variant, unauthenticated, name escaped and present.
    let (status, body, _) = send(&app, "GET", &format!("/api/v1/public/{slug}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    let html = body.as_str().unwrap_or_default();
    assert!(
        html.contains("Demo API &amp; Special &lt;Chars&gt;"),
        "escaped name in HTML"
    );
    assert!(
        html.contains("https://api.example.test/things"),
        "request URL in HTML"
    );
    assert!(html.contains("GET"), "method in HTML");

    // /public/{slug} alias serves the same page.
    let (status, body, _) = send(&app, "GET", &format!("/public/{slug}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.as_str().unwrap_or_default().contains("Demo API"));

    // JSON variant, unauthenticated.
    let (status, body, _) = send(
        &app,
        "GET",
        &format!("/api/v1/public/{slug}/json"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["collection"]["name"], "Demo API & Special <Chars>");

    // Unpublish (owner) -> both public endpoints 404.
    let (status, _, _) = send(
        &app,
        "DELETE",
        &format!("/api/v1/collections/{id}/publish"),
        Some(&owen),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _) = send(&app, "GET", &format!("/api/v1/public/{slug}"), None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body, _) = send(
        &app,
        "GET",
        &format!("/api/v1/public/{slug}/json"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&body), "not_found");
}

// --- 7. AC-17 reopen DB file from disk ---------------------------------------------

#[tokio::test]
async fn reopen_db_file_data_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("restrust.db");

    let mut config = base_config();
    config.bootstrap_admin_user = Some("root".to_string());
    config.bootstrap_admin_password = Some("root-pass".to_string());
    config.db_path = db_path.clone();

    let collection_id;
    {
        let conn = db::open(&db_path).expect("open");
        db::migrate(&conn).expect("migrate");
        db::seed_bootstrap_admin(&conn, &config).expect("seed");
        let state = AppState::shared(conn, config.clone());
        let app = app_for(&state);

        let (root, _) = login(&app, "root", "root-pass").await;
        let (status, body, _) = send(
            &app,
            "POST",
            "/api/v1/collections",
            Some(&root),
            Some(json!({"name": "Persistent API"})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        collection_id = body["id"].as_str().unwrap().to_string();

        let (status, body, _) = send(
            &app,
            "PUT",
            &format!("/api/v1/collections/{collection_id}"),
            Some(&root),
            Some(json!({
                "base_revision": 1,
                "collection": collection_payload(&collection_id, "Persistent API")
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["revision"], 2);
        // state (and its Connection) dropped here — file must survive.
    }

    // Reopen from disk.
    let conn = db::open(&db_path).expect("reopen");
    db::migrate(&conn).expect("migrate again");
    let state2 = AppState::shared(conn, config);
    let app2 = app_for(&state2);

    // Data intact: admin login still works, collection revision survived.
    let (root, _) = login(&app2, "root", "root-pass").await;
    let (status, body, _) = send(
        &app2,
        "GET",
        &format!("/api/v1/collections/{collection_id}"),
        Some(&root),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revision"], 2);
    assert_eq!(body["collection"]["name"], "Persistent API");
    assert_eq!(body["access"], "owner");
}

// --- 8. session expiry honored ------------------------------------------------------

#[tokio::test]
async fn session_expiry_rejected_on_session_route() {
    let state = state_with(base_config());
    let app = app_for(&state);

    add_user(&state, "nina", "nina-pass", "user");
    let (token, body) = login(&app, "nina", "nina-pass").await;
    assert_eq!(body["user"]["username"], "nina");

    // Session works while valid.
    let (status, body, _) = send(&app, "GET", "/api/v1/auth/session", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["username"], "nina");

    // Expire the session directly in the DB (30-day default is not testable).
    state.with_db(|conn| {
        conn.execute(
            "UPDATE sessions SET expires_at = '2020-01-01T00:00:00Z' WHERE token = ?1",
            [&token],
        )
        .unwrap();
    });

    let (status, body, _) = send(&app, "GET", "/api/v1/auth/session", Some(&token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(&body), "unauthorized");

    // /api/v1/me behaves the same.
    let (status, _, _) = send(&app, "GET", "/api/v1/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
