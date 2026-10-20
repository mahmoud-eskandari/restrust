//! restrust-server: the self-hosted collaboration server (CONTRACT §4).
//!
//! Library crate so integration tests (`tests/api.rs`) can build the router
//! and database directly; the `restrust-server` binary in `main.rs` is a thin
//! wrapper around [`api::app_router`] plus listener/TLS setup.

pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod oidc;
pub mod state;
