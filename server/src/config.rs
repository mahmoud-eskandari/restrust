//! Server configuration: environment variables > SQLite `settings` rows >
//! defaults (CONTRACT §4.1).
//!
//! Note on ordering: the database path itself must be known before the
//! database can be opened, so `RESTRUST_DATA_DIR` / `RESTRUST_DB_PATH` /
//! `RESTRUST_FILES_DIR` are resolved from env > default only; every other
//! key also consults the `settings` table of the opened database.

use std::path::PathBuf;

use rusqlite::Connection;

/// TLS listener mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    /// Single plain-HTTP listener (dev default).
    None,
    /// rustls from operator-supplied PEM files + HTTP redirect listener.
    Files,
    /// Automatic certificates via ACME (Let's Encrypt) + HTTP-01 listener.
    Acme,
}

impl TlsMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Some(TlsMode::None),
            "files" => Some(TlsMode::Files),
            "acme" => Some(TlsMode::Acme),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            TlsMode::None => "none",
            TlsMode::Files => "files",
            TlsMode::Acme => "acme",
        }
    }
}

/// A generic OIDC provider from `RESTRUST_SSO_{i}_*` variables.
#[derive(Clone, Debug)]
pub struct SsoProviderConfig {
    pub index: u8,
    pub name: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
}

/// Effective server configuration.
#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: String,
    pub http_port: u16,
    pub https_enabled: bool,
    pub https_port: u16,
    pub domain: Option<String>,
    pub tls_mode: TlsMode,
    pub tls_cert_file: Option<String>,
    pub tls_key_file: Option<String>,
    pub acme_directory: String,
    pub acme_contact_email: Option<String>,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub files_dir: PathBuf,
    pub web_dir: Option<PathBuf>,
    pub public_base_url: Option<String>,
    pub bootstrap_admin_user: Option<String>,
    pub bootstrap_admin_password: Option<String>,
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    pub sso_providers: Vec<SsoProviderConfig>,
}

pub const DEFAULT_BIND_ADDR: &str = "0.0.0.0";
pub const DEFAULT_HTTP_PORT: u16 = 8080;
pub const DEFAULT_HTTPS_PORT: u16 = 8443;
pub const DEFAULT_DATA_DIR: &str = "./data";

/// Let's Encrypt directory URLs (also the resolution of the
/// `production` / `staging` shorthand for `RESTRUST_ACME_DIRECTORY`).
pub const ACME_PRODUCTION_URL: &str = "https://acme-v02.api.letsencrypt.org/directory";
pub const ACME_STAGING_URL: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
        row.get::<_, String>(0)
    })
    .ok()
    .filter(|v| !v.trim().is_empty())
}

/// env > SQLite settings > default, as a raw string.
fn layered(conn: &Connection, env_key: &str, settings_key: &str) -> Option<String> {
    env_nonempty(env_key).or_else(|| setting(conn, settings_key))
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Resolve the DB location before the database can be opened (env > default).
pub fn resolve_db_location() -> (PathBuf, PathBuf) {
    let data_dir = PathBuf::from(
        env_nonempty("RESTRUST_DATA_DIR").unwrap_or_else(|| DEFAULT_DATA_DIR.to_string()),
    );
    let db_path = match env_nonempty("RESTRUST_DB_PATH") {
        Some(p) => PathBuf::from(p),
        None => data_dir.join("restrust.db"),
    };
    (data_dir, db_path)
}

impl Config {
    /// Load the effective configuration from env > settings > defaults.
    /// `conn` must already have the `settings` table (run migrations first).
    pub fn load(conn: &Connection, data_dir: PathBuf, db_path: PathBuf) -> Config {
        let l = |env_key: &str, settings_key: &str| layered(conn, env_key, settings_key);

        let bind_addr =
            l("RESTRUST_BIND_ADDR", "bind_addr").unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string());

        let http_port = l("RESTRUST_HTTP_PORT", "http_port")
            .and_then(|v| v.trim().parse::<u16>().ok())
            .unwrap_or(DEFAULT_HTTP_PORT);

        let https_enabled = l("RESTRUST_HTTPS_ENABLED", "https_enabled")
            .and_then(|v| parse_bool(&v))
            .unwrap_or(false);

        let https_port = l("RESTRUST_HTTPS_PORT", "https_port")
            .and_then(|v| v.trim().parse::<u16>().ok())
            .unwrap_or(DEFAULT_HTTPS_PORT);

        let domain = l("RESTRUST_DOMAIN", "domain");
        let tls_mode = l("RESTRUST_TLS_MODE", "tls_mode")
            .and_then(|v| TlsMode::parse(&v))
            .unwrap_or(TlsMode::None);

        let tls_cert_file = l("RESTRUST_TLS_CERT_FILE", "tls_cert_file");
        let tls_key_file = l("RESTRUST_TLS_KEY_FILE", "tls_key_file");
        let acme_directory =
            l("RESTRUST_ACME_DIRECTORY", "acme_directory").unwrap_or_else(|| "production".into());
        let acme_contact_email = l("RESTRUST_ACME_CONTACT_EMAIL", "acme_contact_email");

        let files_dir = match env_nonempty("RESTRUST_FILES_DIR") {
            Some(p) => PathBuf::from(p),
            None => data_dir.join("files"),
        };

        let web_dir = l("RESTRUST_WEB_DIR", "web_dir").map(PathBuf::from);

        let public_base_url = l("RESTRUST_PUBLIC_BASE_URL", "public_base_url")
            .or_else(|| default_public_base_url(&domain, https_enabled, &https_port, &http_port));

        // Bootstrap admin + OIDC credentials are env-only (secrets never
        // persist in the settings table).
        let bootstrap_admin_user = env_nonempty("RESTRUST_BOOTSTRAP_ADMIN_USER");
        let bootstrap_admin_password = env_nonempty("RESTRUST_BOOTSTRAP_ADMIN_PASSWORD");
        let google_client_id = env_nonempty("RESTRUST_GOOGLE_CLIENT_ID");
        let google_client_secret = env_nonempty("RESTRUST_GOOGLE_CLIENT_SECRET");

        let mut sso_providers = Vec::new();
        for index in 1u8..=5 {
            let (Some(name), Some(issuer), Some(client_id), Some(client_secret)) = (
                env_nonempty(&format!("RESTRUST_SSO_{index}_NAME")),
                env_nonempty(&format!("RESTRUST_SSO_{index}_ISSUER")),
                env_nonempty(&format!("RESTRUST_SSO_{index}_CLIENT_ID")),
                env_nonempty(&format!("RESTRUST_SSO_{index}_CLIENT_SECRET")),
            ) else {
                continue;
            };
            sso_providers.push(SsoProviderConfig {
                index,
                name,
                issuer,
                client_id,
                client_secret,
            });
        }

        Config {
            bind_addr,
            http_port,
            https_enabled,
            https_port,
            domain,
            tls_mode,
            tls_cert_file,
            tls_key_file,
            acme_directory,
            acme_contact_email,
            data_dir,
            db_path,
            files_dir,
            web_dir,
            public_base_url,
            bootstrap_admin_user,
            bootstrap_admin_password,
            google_client_id,
            google_client_secret,
            sso_providers,
        }
    }

    /// A minimal in-memory configuration for tests and embedded use.
    pub fn for_tests() -> Config {
        Config {
            bind_addr: "127.0.0.1".to_string(),
            http_port: 0,
            https_enabled: false,
            https_port: 0,
            domain: None,
            tls_mode: TlsMode::None,
            tls_cert_file: None,
            tls_key_file: None,
            acme_directory: "production".to_string(),
            acme_contact_email: None,
            data_dir: std::env::temp_dir(),
            db_path: PathBuf::from(":memory:"),
            files_dir: std::env::temp_dir(),
            web_dir: None,
            public_base_url: None,
            bootstrap_admin_user: None,
            bootstrap_admin_password: None,
            google_client_id: None,
            google_client_secret: None,
            sso_providers: Vec::new(),
        }
    }

    /// Effective TLS behavior: `files`/`acme` modes always run a TLS listener;
    /// otherwise TLS runs only when explicitly enabled.
    pub fn tls_active(&self) -> bool {
        matches!(self.tls_mode, TlsMode::Files | TlsMode::Acme) || self.https_enabled
    }

    /// Resolved ACME directory URL for the configured `acme_directory`
    /// (`production` / `staging` shorthand or a full URL).
    pub fn acme_directory_url(&self) -> String {
        match self.acme_directory.trim().to_ascii_lowercase().as_str() {
            "production" | "" => ACME_PRODUCTION_URL.to_string(),
            "staging" => ACME_STAGING_URL.to_string(),
            other => other.to_string(),
        }
    }

    /// Log the effective configuration with secrets masked (CONTRACT §4.1).
    pub fn log_effective(&self) {
        let mask = |v: &Option<String>| match v {
            Some(_) => "***".to_string(),
            None => "-".to_string(),
        };
        log::info!("effective configuration:");
        log::info!("  bind_addr               = {}", self.bind_addr);
        log::info!("  http_port               = {}", self.http_port);
        log::info!("  https_enabled           = {}", self.https_enabled);
        log::info!("  https_port              = {}", self.https_port);
        log::info!(
            "  domain                  = {}",
            self.domain.as_deref().unwrap_or("-")
        );
        log::info!("  tls_mode                = {}", self.tls_mode.as_str());
        log::info!(
            "  tls_cert_file           = {}",
            self.tls_cert_file.as_deref().unwrap_or("-")
        );
        log::info!(
            "  tls_key_file            = {}",
            self.tls_key_file.as_deref().unwrap_or("-")
        );
        log::info!("  acme_directory          = {}", self.acme_directory);
        log::info!(
            "  acme_contact_email      = {}",
            self.acme_contact_email.as_deref().unwrap_or("-")
        );
        log::info!("  data_dir                = {}", self.data_dir.display());
        log::info!("  db_path                 = {}", self.db_path.display());
        log::info!("  files_dir               = {}", self.files_dir.display());
        log::info!(
            "  web_dir                 = {}",
            self.web_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "-".into())
        );
        log::info!(
            "  public_base_url         = {}",
            self.public_base_url
                .as_deref()
                .unwrap_or("(derived from request host)")
        );
        log::info!(
            "  bootstrap_admin_user    = {}",
            self.bootstrap_admin_user.as_deref().unwrap_or("-")
        );
        log::info!(
            "  bootstrap_admin_password= {}",
            mask(&self.bootstrap_admin_password)
        );
        log::info!(
            "  google_client_id        = {}",
            self.google_client_id.as_deref().unwrap_or("-")
        );
        log::info!(
            "  google_client_secret    = {}",
            mask(&self.google_client_secret)
        );
        for p in &self.sso_providers {
            log::info!(
                "  sso_{}: name={} issuer={} client_id={} client_secret={}",
                p.index,
                p.name,
                p.issuer,
                p.client_id,
                mask(&Some(p.client_secret.clone()))
            );
        }
    }

    /// Non-secret effective config exposed via `GET /api/v1/healthz`.
    pub fn public_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "bind_addr": self.bind_addr,
            "http_port": self.http_port,
            "https_enabled": self.https_enabled,
            "https_port": self.https_port,
            "domain": self.domain,
            "tls_mode": self.tls_mode.as_str(),
            "acme_directory": self.acme_directory,
            "data_dir": self.data_dir.display().to_string(),
            "files_dir": self.files_dir.display().to_string(),
            "web_dir": self.web_dir.as_ref().map(|p| p.display().to_string()),
            "public_base_url": self.public_base_url,
            "google_sso": self.google_client_id.is_some(),
            "sso_providers": self.sso_providers.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        })
    }
}

/// Derive `public_base_url` from domain/ports when unset.
fn default_public_base_url(
    domain: &Option<String>,
    https_enabled: bool,
    https_port: &u16,
    http_port: &u16,
) -> Option<String> {
    let domain = domain.as_ref()?;
    if https_enabled || *https_port != 443 {
        // TLS (or a non-standard HTTPS port): link against https.
        Some(format!("https://{domain}:{https_port}"))
    } else if *http_port != 80 {
        Some(format!("http://{domain}:{http_port}"))
    } else {
        Some(format!("http://{domain}"))
    }
}
