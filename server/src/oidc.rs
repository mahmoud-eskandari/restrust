//! OIDC sign-in: provider registry, discovery, authorization-code flow with
//! server-side state, id_token verification via JWKS, and user provisioning
//! (CONTRACT §4.3).

use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{Duration as ChronoDuration, Utc};
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::auth;
use crate::config::Config;
use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::state::SharedState;

/// One configured OIDC provider.
#[derive(Clone, Debug)]
pub struct ProviderConfig {
    /// Route id: `google` or `sso_{i}`.
    pub id: String,
    pub display_name: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
}

impl ProviderConfig {
    /// google preset + `RESTRUST_SSO_{i}_*` generic providers.
    pub fn from_config(config: &Config) -> Vec<ProviderConfig> {
        let mut providers = Vec::new();
        if let (Some(client_id), Some(client_secret)) =
            (&config.google_client_id, &config.google_client_secret)
        {
            providers.push(ProviderConfig {
                id: "google".to_string(),
                display_name: "Google".to_string(),
                issuer: "https://accounts.google.com".to_string(),
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
            });
        }
        for sso in &config.sso_providers {
            providers.push(ProviderConfig {
                id: format!("sso_{}", sso.index),
                display_name: sso.name.clone(),
                issuer: sso.issuer.trim_end_matches('/').to_string(),
                client_id: sso.client_id.clone(),
                client_secret: sso.client_secret.clone(),
            });
        }
        providers
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("reqwest client")
}

fn provider<'a>(state: &'a SharedState, id: &str) -> Option<&'a ProviderConfig> {
    state.oidc_providers.iter().find(|p| p.id == id)
}

/// Fetch (and cache) `{issuer}/.well-known/openid-configuration`.
async fn discovery(state: &SharedState, p: &ProviderConfig) -> ApiResult<Value> {
    {
        let cache = state.oidc_discovery_cache.lock().expect("cache mutex");
        if let Some(doc) = cache.get(&p.issuer) {
            return Ok(doc.clone());
        }
    }

    let url = format!("{}/.well-known/openid-configuration", p.issuer);
    let resp = http_client()
        .get(&url)
        .send()
        .await
        .and_then(|resp| resp.error_for_status())
        .map_err(|err| ApiError::internal(format!("OIDC discovery failed for {}: {err}", p.id)))?;
    let doc: Value = resp
        .json()
        .await
        .map_err(|err| ApiError::internal(format!("OIDC discovery failed for {}: {err}", p.id)))?;

    state
        .oidc_discovery_cache
        .lock()
        .expect("cache mutex")
        .insert(p.issuer.clone(), doc.clone());
    Ok(doc)
}

/// Fetch (and cache) the JWKS under a discovery `jwks_uri`.
async fn jwks(state: &SharedState, p: &ProviderConfig, jwks_uri: &str) -> ApiResult<JwkSet> {
    {
        let cache = state.oidc_discovery_cache.lock().expect("cache mutex");
        if let Some(Value::String(raw)) = cache.get(jwks_uri) {
            if let Ok(set) = serde_json::from_str(raw) {
                return Ok(set);
            }
        }
    }

    let resp = http_client()
        .get(jwks_uri)
        .send()
        .await
        .and_then(|resp| resp.error_for_status())
        .map_err(|err| ApiError::internal(format!("OIDC JWKS fetch failed for {}: {err}", p.id)))?;
    let set: JwkSet = resp
        .json()
        .await
        .map_err(|err| ApiError::internal(format!("OIDC JWKS fetch failed for {}: {err}", p.id)))?;

    if let Ok(raw) = serde_json::to_string(&set) {
        state
            .oidc_discovery_cache
            .lock()
            .expect("cache mutex")
            .insert(jwks_uri.to_string(), Value::String(raw));
    }
    Ok(set)
}

fn endpoint(doc: &Value, field: &str) -> ApiResult<String> {
    doc.get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ApiError::internal(format!("OIDC discovery lacks `{field}`")))
}

/// Absolute base URL for link generation: explicit config, else derived from
/// the request Host header.
pub fn request_base_url(state: &SharedState, headers: &HeaderMap) -> String {
    if let Some(base) = &state.config.public_base_url {
        return base.trim_end_matches('/').to_string();
    }
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let scheme = if state.config.tls_active() {
        "https"
    } else {
        "http"
    };
    format!("{scheme}://{host}")
}

fn encode_query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                utf8_percent_encode(k, NON_ALPHANUMERIC),
                utf8_percent_encode(v, NON_ALPHANUMERIC)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

// --- state storage (settings row with expiry) ---------------------------------

fn store_oidc_state(
    conn: &Connection,
    provider_id: &str,
    oauth_state: &str,
    redirect_uri: &str,
) -> ApiResult<()> {
    let payload = json!({
        "state": oauth_state,
        "redirect_uri": redirect_uri,
        "expires_at": (Utc::now() + ChronoDuration::minutes(10))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    });
    db::set_setting(
        conn,
        &format!("oidc_state:{provider_id}"),
        &payload.to_string(),
    )
    .map_err(ApiError::from)
}

struct PendingOidc {
    redirect_uri: String,
}

fn take_oidc_state(
    conn: &Connection,
    provider_id: &str,
    oauth_state: &str,
) -> ApiResult<PendingOidc> {
    let key = format!("oidc_state:{provider_id}");
    let raw = db::get_setting(conn, &key)
        .ok_or_else(|| ApiError::bad_request("unknown or expired OIDC state"))?;
    db::delete_setting(conn, &key).map_err(ApiError::from)?;

    let parsed: Value =
        serde_json::from_str(&raw).map_err(|_| ApiError::bad_request("malformed OIDC state"))?;
    let stored = parsed
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if stored.is_empty() || stored != oauth_state {
        return Err(ApiError::bad_request("OIDC state mismatch"));
    }
    let expires_at = parsed
        .get("expires_at")
        .and_then(Value::as_str)
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .ok_or_else(|| ApiError::bad_request("malformed OIDC state expiry"))?;
    if expires_at.with_timezone(&Utc) < Utc::now() {
        return Err(ApiError::bad_request("OIDC state expired"));
    }
    let redirect_uri = parsed
        .get("redirect_uri")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(PendingOidc { redirect_uri })
}

// --- handlers ------------------------------------------------------------------

/// `GET /api/v1/auth/oidc/{p}/start` — 302 to the provider.
pub async fn start(
    State(state): State<SharedState>,
    Path(provider_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let p = provider(&state, &provider_id)
        .ok_or_else(|| ApiError::not_found(format!("unknown OIDC provider `{provider_id}`")))?
        .clone();

    let base = request_base_url(&state, &headers);
    let redirect_uri = format!("{base}/api/v1/auth/oidc/{}/callback", p.id);

    let doc = discovery(&state, &p).await?;
    let authorization_endpoint = endpoint(&doc, "authorization_endpoint")?;

    let oauth_state = auth::new_token();
    state.with_db(|conn| store_oidc_state(conn, &p.id, &oauth_state, &redirect_uri))?;

    let query = encode_query(&[
        ("client_id", p.client_id.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("response_type", "code"),
        ("scope", "openid email profile"),
        ("state", oauth_state.as_str()),
    ]);
    let location = format!("{authorization_endpoint}?{query}");
    // CONTRACT §4.3 says 302; axum's `Redirect` helpers are 303/307/308 only.
    Ok((
        StatusCode::FOUND,
        [(axum::http::header::LOCATION, location)],
    )
        .into_response())
}

/// `GET /api/v1/auth/oidc/{p}/callback` — exchange, verify, provision, 302 to
/// the client (or the paste-token done page).
pub async fn callback(
    State(state): State<SharedState>,
    Path(provider_id): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let p = provider(&state, &provider_id)
        .ok_or_else(|| ApiError::not_found(format!("unknown OIDC provider `{provider_id}`")))?
        .clone();

    let code = query
        .get("code")
        .ok_or_else(|| ApiError::bad_request("missing `code` parameter"))?;
    let oauth_state = query
        .get("state")
        .ok_or_else(|| ApiError::bad_request("missing `state` parameter"))?;

    let pending = state.with_db(|conn| take_oidc_state(conn, &p.id, oauth_state))?;

    let doc = discovery(&state, &p).await?;
    let token_endpoint = endpoint(&doc, "token_endpoint")?;
    let jwks_uri = endpoint(&doc, "jwks_uri")?;

    let form_body = encode_query(&[
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", pending.redirect_uri.as_str()),
        ("client_id", p.client_id.as_str()),
        ("client_secret", p.client_secret.as_str()),
    ]);
    let resp = http_client()
        .post(&token_endpoint)
        .header(
            axum::http::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form_body)
        .send()
        .await
        .and_then(|resp| resp.error_for_status())
        .map_err(|err| ApiError::internal(format!("OIDC token exchange failed: {err}")))?;
    let token_response: Value = resp
        .json()
        .await
        .map_err(|err| ApiError::internal(format!("OIDC token exchange failed: {err}")))?;

    let id_token = token_response
        .get("id_token")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("token endpoint returned no id_token"))?;

    let claims = verify_id_token(&state, &p, id_token, &jwks_uri).await?;
    let session_token = state.with_db(|conn| upsert_oidc_user(conn, &p, &claims))?;

    // Hand the session to the client: same-origin web client gets the cookie;
    // the desktop client copies the token from the done page (or the
    // `#token=` redirect when a public base URL is configured).
    let cookie = auth::session_cookie(&session_token);
    if let Some(base) = &state.config.public_base_url {
        let location = format!("{}/#token={session_token}", base.trim_end_matches('/'));
        Ok((
            StatusCode::FOUND,
            [
                (axum::http::header::LOCATION, location),
                (axum::http::header::SET_COOKIE, cookie),
            ],
        )
            .into_response())
    } else {
        let _ = headers;
        Ok((
            StatusCode::OK,
            [
                (axum::http::header::SET_COOKIE, cookie),
                (
                    axum::http::header::CONTENT_TYPE,
                    "text/html; charset=utf-8".to_string(),
                ),
            ],
            done_page_html(&session_token),
        )
            .into_response())
    }
}

/// `GET /api/v1/auth/oidc/done?token=...` — minimal paste-token page.
pub async fn done(Query(query): Query<std::collections::HashMap<String, String>>) -> Response {
    let token = query.get("token").cloned().unwrap_or_default();
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        done_page_html(&token),
    )
        .into_response()
}

fn done_page_html(token: &str) -> String {
    let escaped = html_escape(token);
    format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><title>restrust sign-in</title>
<style>
 body {{ font-family: system-ui, sans-serif; max-width: 40rem; margin: 4rem auto; padding: 0 1rem; }}
 code {{ display: block; padding: 0.75rem 1rem; background: #f3f4f6; border-radius: 6px;
        word-break: break-all; font-size: 0.9rem; }}
 button {{ margin-top: 1rem; padding: 0.5rem 1rem; cursor: pointer; }}
</style></head>
<body>
<h1>Signed in to restrust</h1>
<p>Copy this session token and paste it into the restrust desktop client's
paste-token field:</p>
<code id="token">{escaped}</code>
<button onclick="navigator.clipboard.writeText(document.getElementById('token').textContent)
 .then(() => this.textContent = 'Copied!')">Copy token</button>
</body></html>"#
    )
}

pub fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// --- id_token verification ------------------------------------------------------

async fn verify_id_token(
    state: &SharedState,
    p: &ProviderConfig,
    id_token: &str,
    jwks_uri: &str,
) -> ApiResult<Value> {
    let header = jsonwebtoken::decode_header(id_token)
        .map_err(|err| ApiError::bad_request(format!("malformed id_token header: {err}")))?;
    if header.alg != Algorithm::RS256 {
        return Err(ApiError::bad_request("id_token must be signed with RS256"));
    }

    let set = jwks(state, p, jwks_uri).await?;
    let jwk = match &header.kid {
        Some(kid) => set
            .keys
            .iter()
            .find(|k| k.common.key_id.as_deref() == Some(kid.as_str())),
        None => set
            .keys
            .iter()
            .find(|k| matches!(k.algorithm, AlgorithmParameters::RSA(_))),
    }
    .ok_or_else(|| ApiError::bad_request("signing key not found in JWKS"))?;
    if !matches!(jwk.algorithm, AlgorithmParameters::RSA(_)) {
        return Err(ApiError::bad_request("signing key is not an RSA key"));
    }

    let key = DecodingKey::from_jwk(jwk)
        .map_err(|err| ApiError::bad_request(format!("unusable JWKS key: {err}")))?;

    let mut validation = Validation::new(Algorithm::RS256);
    // Accept the issuer in both `https://host` and bare `host` forms (Google
    // historically uses both).
    let issuer_trimmed = p.issuer.trim_end_matches('/');
    let bare_host = issuer_trimmed
        .strip_prefix("https://")
        .unwrap_or(issuer_trimmed);
    validation.set_issuer(&[issuer_trimmed, bare_host]);
    validation.set_audience(&[p.client_id.as_str()]);

    let data = jsonwebtoken::decode::<Value>(id_token, &key, &validation)
        .map_err(|err| ApiError::bad_request(format!("id_token verification failed: {err}")))?;
    Ok(data.claims)
}

// --- user provisioning ------------------------------------------------------------

/// Match by (provider, subject) -> link by verified email -> provision.
/// Creates a session and returns its token.
fn upsert_oidc_user(conn: &Connection, p: &ProviderConfig, claims: &Value) -> ApiResult<String> {
    let subject: String = claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("id_token lacks `sub`"))?
        .to_string();
    let email: Option<String> = claims
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_string);
    let email_verified = claims
        .get("email_verified")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let preferred_username: Option<String> = claims
        .get("preferred_username")
        .and_then(Value::as_str)
        .map(str::to_string);
    let display_name: Option<String> = claims
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);

    let now = restrust_core::model::now_rfc3339();

    // 1. existing link
    let user_id: Option<String> = conn
        .query_row(
            "SELECT id FROM users WHERE auth_provider = ?1 AND provider_subject = ?2",
            params![p.id, subject],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(ApiError::from)?;

    let user_id = match user_id {
        Some(id) => id,
        None => {
            // 2. link by verified email
            if let (Some(email), true) = (email.clone(), email_verified) {
                let by_email: Option<String> = conn
                    .query_row("SELECT id FROM users WHERE email = ?1", [&email], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()
                    .map_err(ApiError::from)?;
                if let Some(id) = by_email {
                    conn.execute(
                        "UPDATE users SET auth_provider = ?1, provider_subject = ?2 WHERE id = ?3",
                        params![p.id, subject, id],
                    )
                    .map_err(ApiError::from)?;
                    return finish_session(conn, &id);
                }
            }

            // 3. provision
            let base_username = preferred_username
                .clone()
                .or_else(|| {
                    email
                        .as_deref()
                        .map(|e| e.split('@').next().unwrap_or("user").to_string())
                })
                .unwrap_or_else(|| format!("{}-{}", p.id, &subject[..subject.len().min(8)]));
            let username = auth::uniquify_username(conn, &base_username).map_err(ApiError::from)?;
            let display = display_name
                .or(email.clone())
                .unwrap_or_else(|| username.clone());
            let id = restrust_core::model::new_id();
            conn.execute(
                "INSERT INTO users (id, username, password_hash, display_name, email, system_role, disabled, created_at, auth_provider, provider_subject)
                 VALUES (?1, ?2, NULL, ?3, ?4, 'user', 0, ?5, ?6, ?7)",
                params![id, username, display, email, now, p.id, subject],
            )
            .map_err(ApiError::from)?;
            id
        }
    };
    finish_session(conn, &user_id)
}

fn finish_session(conn: &Connection, user_id: &str) -> ApiResult<String> {
    let token = auth::create_session(conn, user_id).map_err(ApiError::from)?;
    Ok(token)
}
