//! SQLite access: open, WAL, embedded migrations (CONTRACT §4.2), settings
//! helpers, and the idempotent bootstrap-admin seed.

use std::path::Path;

use rusqlite::Connection;

use crate::auth;
use crate::config::Config;

/// Open (creating parent directories and the file if needed) and prepare the
/// on-disk database: WAL journal + sane busy timeout.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

/// In-memory database for tests (same migrations).
pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

/// Embedded migrations — `CREATE TABLE IF NOT EXISTS` per CONTRACT §4.2.
/// Column names are binding; do not rename.
pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT UNIQUE COLLATE NOCASE,
            password_hash TEXT NULL,
            display_name TEXT,
            email TEXT NULL,
            system_role TEXT DEFAULT 'user',
            disabled INT DEFAULT 0,
            created_at TEXT,
            auth_provider TEXT DEFAULT 'local',
            provider_subject TEXT NULL,
            UNIQUE(auth_provider, provider_subject)
        );

        CREATE TABLE IF NOT EXISTS sessions (
            token TEXT PRIMARY KEY,
            user_id TEXT,
            created_at TEXT,
            expires_at TEXT
        );

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT
        );

        CREATE TABLE IF NOT EXISTS organizations (
            id TEXT PRIMARY KEY,
            name TEXT UNIQUE,
            created_at TEXT
        );

        CREATE TABLE IF NOT EXISTS org_members (
            org_id TEXT,
            user_id TEXT,
            role TEXT,
            PRIMARY KEY(org_id, user_id)
        );

        CREATE TABLE IF NOT EXISTS collections (
            id TEXT PRIMARY KEY,
            namespace_kind TEXT,
            owner_user_id TEXT NULL,
            org_id TEXT NULL,
            name TEXT,
            data TEXT,
            revision INTEGER,
            updated_at TEXT,
            deleted INT DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS shares (
            collection_id TEXT,
            subject_kind TEXT,
            subject_id TEXT NULL,
            level TEXT,
            PRIMARY KEY(collection_id, subject_kind, subject_id)
        );

        CREATE TABLE IF NOT EXISTS publications (
            collection_id TEXT PRIMARY KEY,
            slug TEXT UNIQUE,
            published_at TEXT
        );

        -- Git-style per-collection change history (RestRust-native). One row
        -- per accepted revision; `data` keeps the full snapshot so any
        -- revision can be replayed.
        CREATE TABLE IF NOT EXISTS collection_revisions (
            collection_id TEXT NOT NULL,
            revision INTEGER NOT NULL,
            author_user_id TEXT NULL,
            author_username TEXT NOT NULL,
            time TEXT NOT NULL,
            summary TEXT NOT NULL,
            content_hash INTEGER NOT NULL DEFAULT 0,
            changes TEXT NOT NULL,
            data TEXT NOT NULL,
            PRIMARY KEY (collection_id, revision)
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
        CREATE INDEX IF NOT EXISTS idx_collections_owner ON collections(owner_user_id) WHERE deleted = 0;
        CREATE INDEX IF NOT EXISTS idx_collections_org ON collections(org_id) WHERE deleted = 0;
        CREATE INDEX IF NOT EXISTS idx_shares_subject ON shares(subject_kind, subject_id);
        CREATE INDEX IF NOT EXISTS idx_revisions_collection ON collection_revisions(collection_id, revision);
        "#,
    )
}

/// Create the bootstrap sysadmin from config when no admin exists
/// (idempotent). Does nothing when the env vars are unset or an admin is
/// already present.
pub fn seed_bootstrap_admin(conn: &Connection, config: &Config) -> rusqlite::Result<()> {
    let (Some(username), Some(password)) = (
        config.bootstrap_admin_user.as_ref(),
        config.bootstrap_admin_password.as_ref(),
    ) else {
        return Ok(());
    };

    let admin_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM users WHERE system_role = 'admin'",
        [],
        |row| row.get(0),
    )?;
    if admin_count > 0 {
        log::info!("bootstrap admin skipped: an admin user already exists");
        return Ok(());
    }

    let hash = auth::hash_password(password);
    let now = restrust_core::model::now_rfc3339();
    // If the configured username exists as a non-admin local user without a
    // password, promote it instead of failing the UNIQUE constraint.
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM users WHERE username = ?1",
            [username],
            |row| row.get(0),
        )
        .ok();
    match existing {
        Some(id) => {
            conn.execute(
                "UPDATE users SET system_role = 'admin', password_hash = ?1 WHERE id = ?2",
                rusqlite::params![hash, id],
            )?;
            log::info!("bootstrap admin: promoted existing user '{username}' to admin");
        }
        None => {
            let id = restrust_core::model::new_id();
            conn.execute(
                "INSERT INTO users (id, username, password_hash, display_name, email, system_role, disabled, created_at, auth_provider, provider_subject)
                 VALUES (?1, ?2, ?3, ?4, NULL, 'admin', 0, ?5, 'local', NULL)",
                rusqlite::params![id, username, hash, username, now],
            )?;
            log::info!("bootstrap admin: created admin user '{username}'");
        }
    }
    Ok(())
}

/// Read one settings row.
pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
        row.get::<_, String>(0)
    })
    .ok()
}

/// Upsert one settings row.
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}

/// Delete one settings row (no error when absent).
pub fn delete_setting(conn: &Connection, key: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    Ok(())
}
