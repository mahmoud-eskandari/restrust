//! restrust-server binary: env/args handling, database bootstrap, and the
//! listener topology per TLS mode (CONTRACT §4.1, §4.4 "TLS"):
//!
//! - `none` — one HTTP listener on `{BIND_ADDR}:{HTTP_PORT}`.
//! - `files` — rustls from PEM files on `{HTTPS_PORT}`; the HTTP listener
//!   keeps `/api/v1/healthz` and redirects everything else to HTTPS.
//! - `acme` — rustls-acme on `{HTTPS_PORT}` (HTTP-01 challenges answered on
//!   the HTTP listener) + the same HTTP redirect behavior.

use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures::StreamExt;
use restrust_server::api;
use restrust_server::config::{Config, TlsMode};
use restrust_server::state::SharedState;

const HELP: &str = "\
restrust-server — self-hosted collaboration server for restrust

USAGE:
  restrust-server [--help] [--version]

Configuration comes from environment variables (env > SQLite `settings`
rows > defaults):

  RESTRUST_BIND_ADDR                 listen address (default 0.0.0.0)
  RESTRUST_HTTP_PORT                 HTTP port (default 8080)
  RESTRUST_HTTPS_ENABLED             enable TLS listener (default false)
  RESTRUST_HTTPS_PORT                HTTPS port (default 8443)
  RESTRUST_DOMAIN                    public domain; required for ACME
  RESTRUST_TLS_MODE                  none | files | acme (default none)
  RESTRUST_TLS_CERT_FILE             cert PEM path (mode=files)
  RESTRUST_TLS_KEY_FILE              key PEM path (mode=files)
  RESTRUST_ACME_DIRECTORY            production | staging | full URL (default production)
  RESTRUST_ACME_CONTACT_EMAIL        ACME account email
  RESTRUST_DATA_DIR                  root for db/files/acme state (default ./data)
  RESTRUST_DB_PATH                   SQLite path (default {DATA_DIR}/restrust.db)
  RESTRUST_FILES_DIR                 uploaded/exported files (default {DATA_DIR}/files)
  RESTRUST_WEB_DIR                   static web client dir served at /
  RESTRUST_PUBLIC_BASE_URL           absolute base used in links (else derived)
  RESTRUST_BOOTSTRAP_ADMIN_USER      create sysadmin if none exists
  RESTRUST_BOOTSTRAP_ADMIN_PASSWORD  ... its password
  RESTRUST_GOOGLE_CLIENT_ID          Google OIDC sign-in
  RESTRUST_GOOGLE_CLIENT_SECRET
  RESTRUST_SSO_{i}_NAME              generic OIDC provider (i = 1..5)
  RESTRUST_SSO_{i}_ISSUER
  RESTRUST_SSO_{i}_CLIENT_ID
  RESTRUST_SSO_{i}_CLIENT_SECRET
";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("restrust-server {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if let Some(other) = args.first() {
        eprintln!("unknown argument: {other}\n\n{HELP}");
        std::process::exit(2);
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    if let Err(err) = run().await {
        log::error!("fatal: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // DB location must be known before the DB can be consulted for settings.
    let (data_dir, db_path) = restrust_server::config::resolve_db_location();
    let conn = restrust_server::db::open(&db_path)?;
    restrust_server::db::migrate(&conn)?;
    let config = Config::load(&conn, data_dir.clone(), db_path.clone());
    restrust_server::db::seed_bootstrap_admin(&conn, &config)?;

    config.log_effective();

    std::fs::create_dir_all(&config.files_dir)?;
    if config.tls_mode == TlsMode::Acme {
        std::fs::create_dir_all(config.data_dir.join("acme"))?;
    }

    let state = restrust_server::state::AppState::shared(conn, config.clone());
    let app = api::app_router(state.clone());

    match config.tls_mode {
        TlsMode::None => serve_http_only(state, app).await?,
        TlsMode::Files => serve_tls_files(state, app).await?,
        TlsMode::Acme => serve_tls_acme(state, app).await?,
    }
    Ok(())
}

fn resolve_addr(host: &str, port: u16) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    use std::net::ToSocketAddrs;
    let candidate = format!("{host}:{port}");
    candidate
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| format!("could not resolve listen address {candidate}").into())
}

async fn serve_http_only(
    state: SharedState,
    app: Router,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = &state.config;
    let addr = resolve_addr(&config.bind_addr, config.http_port)?;
    let listener = std::net::TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    log::info!(
        "restrust-server {} listening on http://{addr}",
        env!("CARGO_PKG_VERSION")
    );

    let handle = axum_server::Handle::new();
    let server = tokio::spawn(
        axum_server::from_tcp(listener)?
            .handle(handle.clone())
            .serve(app.into_make_service()),
    );

    shutdown_on_ctrl_c().await;
    log::info!("shutting down");
    handle.graceful_shutdown(Some(Duration::from_secs(2)));
    server.await??;
    Ok(())
}

async fn serve_tls_files(
    state: SharedState,
    app: Router,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = &state.config;
    let (cert, key) =
        match (&config.tls_cert_file, &config.tls_key_file) {
            (Some(cert), Some(key)) => (cert.clone(), key.clone()),
            _ => return Err(
                "RESTRUST_TLS_MODE=files requires RESTRUST_TLS_CERT_FILE and RESTRUST_TLS_KEY_FILE"
                    .into(),
            ),
        };
    let rustls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;

    let https_addr = resolve_addr(&config.bind_addr, config.https_port)?;
    let http_addr = resolve_addr(&config.bind_addr, config.http_port)?;
    let http_app = redirect_router(state.clone());

    let https_listener = std::net::TcpListener::bind(https_addr)?;
    https_listener.set_nonblocking(true)?;
    let http_listener = std::net::TcpListener::bind(http_addr)?;
    http_listener.set_nonblocking(true)?;
    log::info!(
        "restrust-server {} listening on https://{https_addr}",
        env!("CARGO_PKG_VERSION")
    );
    log::info!("http://{http_addr} answers /api/v1/healthz and redirects other traffic to https");
    let https_handle = axum_server::Handle::new();
    let http_handle = axum_server::Handle::new();
    let https = tokio::spawn(
        axum_server::tls_rustls::from_tcp_rustls(https_listener, rustls_config)?
            .handle(https_handle.clone())
            .serve(app.into_make_service()),
    );
    let http = tokio::spawn(
        axum_server::from_tcp(http_listener)?
            .handle(http_handle.clone())
            .serve(http_app.into_make_service()),
    );

    shutdown_on_ctrl_c().await;
    log::info!("shutting down");
    https_handle.graceful_shutdown(Some(Duration::from_secs(2)));
    http_handle.graceful_shutdown(Some(Duration::from_secs(2)));
    https.await??;
    http.await??;
    Ok(())
}

async fn serve_tls_acme(state: SharedState, app: Router) -> Result<(), Box<dyn std::error::Error>> {
    let config = &state.config;
    let Some(domain) = config.domain.clone() else {
        return Err("RESTRUST_TLS_MODE=acme requires RESTRUST_DOMAIN".into());
    };

    let cache_dir = config.data_dir.join("acme");
    let mut builder = rustls_acme::AcmeConfig::new([domain.as_str()])
        .cache(rustls_acme::caches::DirCache::new(cache_dir))
        .challenge_type(rustls_acme::UseChallenge::Http01);
    builder = match config.acme_directory.trim().to_ascii_lowercase().as_str() {
        "production" => builder.directory_lets_encrypt(true),
        "staging" => builder.directory_lets_encrypt(false),
        url => builder.directory(url),
    };
    if let Some(email) = &config.acme_contact_email {
        builder = builder.contact_push(format!("mailto:{email}"));
    }

    let mut acme_state = builder.state();
    let acceptor = acme_state.axum_acceptor(acme_state.default_rustls_config());
    let challenge_service = acme_state.http01_challenge_tower_service();

    // Drive the ACME event stream (certificate issuance/renewal happens in
    // this task; without it no order is ever placed).
    tokio::spawn(async move {
        while let Some(event) = acme_state.next().await {
            match event {
                Ok(ok) => log::info!("acme event: {ok:?}"),
                Err(err) => log::error!("acme error: {err:?}"),
            }
        }
    });

    let https_addr = resolve_addr(&config.bind_addr, config.https_port)?;
    let http_addr = resolve_addr(&config.bind_addr, config.http_port)?;
    let http_app = redirect_router(state.clone()).route_service(
        "/.well-known/acme-challenge/{challenge_token}",
        challenge_service,
    );

    let https_listener = std::net::TcpListener::bind(https_addr)?;
    https_listener.set_nonblocking(true)?;
    let http_listener = std::net::TcpListener::bind(http_addr)?;
    http_listener.set_nonblocking(true)?;
    log::info!(
        "restrust-server {} listening on https://{https_addr} (ACME)",
        env!("CARGO_PKG_VERSION")
    );
    log::info!(
        "acme directory: {} (domain {domain})",
        config.acme_directory_url()
    );
    log::info!("http://{http_addr} answers ACME HTTP-01, /api/v1/healthz, and redirects to https");
    let https_handle = axum_server::Handle::new();
    let http_handle = axum_server::Handle::new();
    let https = tokio::spawn(
        axum_server::from_tcp(https_listener)?
            .acceptor(acceptor)
            .handle(https_handle.clone())
            .serve(app.into_make_service()),
    );
    let http = tokio::spawn(
        axum_server::from_tcp(http_listener)?
            .handle(http_handle.clone())
            .serve(http_app.into_make_service()),
    );

    shutdown_on_ctrl_c().await;
    log::info!("shutting down");
    https_handle.graceful_shutdown(Some(Duration::from_secs(2)));
    http_handle.graceful_shutdown(Some(Duration::from_secs(2)));
    https.await??;
    http.await??;
    Ok(())
}

/// Plain-HTTP companion router for TLS modes: `/api/v1/healthz` stays
/// reachable; everything else is redirected to HTTPS.
fn redirect_router(state: SharedState) -> Router {
    Router::new()
        .route("/api/v1/healthz", get(api::healthz))
        .fallback(redirect_to_https)
        .with_state(state)
}

async fn redirect_to_https(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    uri: Uri,
) -> Response {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost")
        .split(':')
        .next()
        .unwrap_or("localhost")
        .to_string();
    let https_port = state.config.https_port;
    let location = if https_port == 443 {
        format!("https://{host}{uri}")
    } else {
        format!("https://{host}:{https_port}{uri}")
    };
    (
        StatusCode::MOVED_PERMANENTLY,
        [(header::LOCATION, location)],
    )
        .into_response()
}

async fn shutdown_on_ctrl_c() {
    match tokio::signal::ctrl_c().await {
        Ok(()) => {}
        Err(err) => log::error!("failed to install ctrl-c handler: {err}"),
    }
}
