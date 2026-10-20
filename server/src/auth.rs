//! Password hashing (argon2id), session tokens, and the auth extractors.
//!
//! Sessions are 256-bit random hex tokens carried via `Authorization: Bearer`
//! and (for the served web client) an `rt_session` cookie; expiry is 30 days
//! and checked on every authenticated route (CONTRACT §4.3).

use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::ApiError;
use crate::state::SharedState;

pub const SESSION_COOKIE: &str = "rt_session";
pub const SESSION_DAYS: i64 = 30;
const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;
/// Encoded password format: `$argon2id$v=19$m=...,t=...,p=...$<salt>$<hash>`.
const PHC_PREFIX: &str = "$argon2id$v=19$";

// --- password hashing -------------------------------------------------------

/// Hash a password with argon2id (default params: m=19455 KiB, t=2, p=1) and a
/// fresh random salt.
pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; SALT_LEN];
    rand::fill(&mut salt);
    let argon2 = argon2::Argon2::default();
    let mut out = [0u8; HASH_LEN];
    argon2
        .hash_password_into(password.as_bytes(), &salt, &mut out)
        .expect("argon2 parameters are constant and valid");
    let (m, t, p) = default_params();
    format!(
        "{PHC_PREFIX}m={m},t={t},p={p}${}${}",
        B64.encode(salt),
        B64.encode(out)
    )
}

/// Verify a password against a stored [`hash_password`] string.
pub fn verify_password(password: &str, stored: &str) -> bool {
    let Some(rest) = stored.strip_prefix(PHC_PREFIX) else {
        return false;
    };
    let Some((param_str, rest)) = rest.split_once('$') else {
        return false;
    };
    let Some((salt_b64, hash_b64)) = rest.split_once('$') else {
        return false;
    };
    let (Ok(salt), Ok(expected)) = (B64.decode(salt_b64), B64.decode(hash_b64)) else {
        return false;
    };

    let mut m = 19455u32;
    let mut t = 2u32;
    let mut p = 1u32;
    for pair in param_str.split(',') {
        match pair.split_once('=') {
            Some(("m", v)) => m = v.parse().unwrap_or(m),
            Some(("t", v)) => t = v.parse().unwrap_or(t),
            Some(("p", v)) => p = v.parse().unwrap_or(p),
            _ => {}
        }
    }

    let params = match argon2::Params::new(m, t, p, Some(expected.len().max(4))) {
        Ok(params) => params,
        Err(_) => return false,
    };
    let argon2 = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = vec![0u8; expected.len()];
    if argon2
        .hash_password_into(password.as_bytes(), &salt, &mut out)
        .is_err()
    {
        return false;
    }
    constant_time_eq(&out, &expected)
}

fn default_params() -> (u32, u32, u32) {
    let defaults = argon2::Params::default();
    (defaults.m_cost(), defaults.t_cost(), defaults.p_cost())
}

/// Length-checked constant-time byte comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// --- sessions ----------------------------------------------------------------

/// 256-bit random token, hex-encoded.
pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Create a session for the user; returns the bearer token.
pub fn create_session(conn: &Connection, user_id: &str) -> rusqlite::Result<String> {
    let token = new_token();
    let now = Utc::now();
    conn.execute(
        "INSERT INTO sessions (token, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            token,
            user_id,
            now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            expiry_from(now).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        ],
    )?;
    Ok(token)
}

fn expiry_from(now: DateTime<Utc>) -> DateTime<Utc> {
    now + Duration::days(SESSION_DAYS)
}

/// Look up the user for a session token; `None` when unknown, expired, or the
/// user is disabled.
pub fn session_user(conn: &Connection, token: &str) -> Option<User> {
    let row = conn
        .query_row(
            "SELECT u.id, u.username, u.display_name, u.email, u.system_role, u.disabled, u.created_at, u.auth_provider, s.expires_at
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token = ?1",
            [token],
            |row| {
                Ok((
                    User {
                        id: row.get(0)?,
                        username: row.get(1)?,
                        display_name: row.get(2)?,
                        email: row.get(3)?,
                        system_role: row.get(4)?,
                        disabled: row.get::<_, i64>(5)? != 0,
                        created_at: row.get(6)?,
                        auth_provider: row.get(7)?,
                    },
                    row.get::<_, String>(8)?,
                ))
            },
        )
        .optional()
        .ok()??;

    let (user, expires_at) = row;
    let valid_expiry = DateTime::parse_from_rfc3339(&expires_at)
        .map(|t| t.with_timezone(&Utc) > Utc::now())
        .unwrap_or(false);
    if !valid_expiry || user.disabled {
        return None;
    }
    Some(user)
}

pub fn delete_session(conn: &Connection, token: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM sessions WHERE token = ?1", [token])?;
    Ok(())
}

/// `Set-Cookie` header value for the session cookie (30d, HttpOnly).
pub fn session_cookie(token: &str) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax",
        SESSION_DAYS * 24 * 3600
    )
}

/// Cleared-session `Set-Cookie` header value.
pub fn clear_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax")
}

/// Extract the bearer/cookie token from request headers.
pub fn token_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(authz) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = authz
            .trim()
            .strip_prefix("Bearer ")
            .or_else(|| authz.trim().strip_prefix("bearer "))
        {
            let token = token.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    cookie_value_headers(headers, SESSION_COOKIE)
}

/// Extract the bearer/cookie token from request parts.
pub fn token_from_parts(parts: &Parts) -> Option<String> {
    token_from_headers(&parts.headers)
}

/// Pull one value out of a `Cookie` header (no cookie crate dependency).
pub fn cookie_value_headers(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let header = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie_value_str(header, name)
}

pub fn cookie_value_str(cookie_header: &str, name: &str) -> Option<String> {
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        if let Some(value) = pair.strip_prefix(&format!("{name}=")) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

// --- users -------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub email: Option<String>,
    pub system_role: String,
    pub disabled: bool,
    pub created_at: String,
    pub auth_provider: String,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.system_role == "admin"
    }

    /// Wire representation (server API uses snake_case per CONTRACT §4.4).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "username": self.username,
            "display_name": self.display_name,
            "system_role": self.system_role,
            "disabled": self.disabled,
            "created_at": self.created_at,
        })
    }
}

/// Append `-2`, `-3`, ... (then a random suffix) until the username is free.
pub fn uniquify_username(conn: &Connection, base: &str) -> rusqlite::Result<String> {
    let slug: String = base
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    let base = if slug.is_empty() {
        "user".to_string()
    } else {
        slug
    };

    for n in 1..100 {
        let candidate = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let taken: bool = conn
            .query_row(
                "SELECT 1 FROM users WHERE username = ?1",
                [&candidate],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !taken {
            return Ok(candidate);
        }
    }
    Ok(format!("{base}-{}", &new_token()[..8]))
}

// --- extractors ---------------------------------------------------------------

/// Authenticating extractor: any valid, non-expired session.
pub struct AuthUser(pub User);

impl FromRequestParts<SharedState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        let Some(token) = token_from_parts(parts) else {
            return Err(ApiError::unauthorized("missing session token"));
        };
        let conn = state.db.lock().expect("db mutex poisoned");
        session_user(&conn, &token)
            .map(AuthUser)
            .ok_or_else(|| ApiError::unauthorized("invalid or expired session"))
    }
}

/// Admin extractor: valid session AND `system_role = 'admin'`, re-checked on
/// every call (AC-19).
pub struct AdminUser(pub User);

impl FromRequestParts<SharedState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state)
            .await
            .map(|AuthUser(user)| user)?;
        if user.is_admin() {
            Ok(AdminUser(user))
        } else {
            Err(ApiError::access_denied(
                "this operation requires the admin role",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let stored = hash_password("correct horse battery staple");
        assert!(stored.starts_with("$argon2id$v=19$"));
        assert!(verify_password("correct horse battery staple", &stored));
        assert!(!verify_password("wrong", &stored));
        assert!(!verify_password(
            "correct horse battery staple",
            "$argon2id$garbage"
        ));
        assert!(!verify_password("x", "not-a-phc-string"));
    }

    #[test]
    fn different_salts_produce_different_hashes() {
        let a = hash_password("pw");
        let b = hash_password("pw");
        assert_ne!(a, b);
        assert!(verify_password("pw", &a) && verify_password("pw", &b));
    }

    #[test]
    fn constant_time_compare() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
