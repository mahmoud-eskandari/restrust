//! Shared application state threaded through every handler.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::oidc::ProviderConfig;

pub struct AppState {
    pub http: reqwest::Client,
    /// SQLite connection behind a std Mutex — handlers never hold the guard
    /// across an `.await`. Single process, WAL mode (CONTRACT §4).
    pub db: Mutex<rusqlite::Connection>,
    pub config: Config,
    /// Configured OIDC providers (google preset + generic SSO).
    pub oidc_providers: Vec<ProviderConfig>,
    /// In-process caches for OIDC discovery documents and JWKS
    /// (issuer -> fetched JSON), refreshed on miss.
    pub oidc_discovery_cache: Mutex<HashMap<String, serde_json::Value>>,
}

pub type SharedState = Arc<AppState>;

impl AppState {
    pub fn shared(db: rusqlite::Connection, config: Config) -> SharedState {
        let oidc_providers = ProviderConfig::from_config(&config);
        Arc::new(AppState {
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()
                .expect("HTTP client"),
            db: Mutex::new(db),
            config,
            oidc_providers,
            oidc_discovery_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Run a closure with the database connection.
    pub fn with_db<T>(&self, f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
        let conn = self.db.lock().expect("db mutex poisoned");
        f(&conn)
    }
}
