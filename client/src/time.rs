//! Platform-safe `Instant`.
//!
//! `std::time::Instant::now()` panics on `wasm32-unknown-unknown`, so the
//! browser build must use `web_time::Instant` (backed by
//! `performance.now()`); native builds keep `std`.

#[cfg(target_arch = "wasm32")]
pub use web_time::Instant;

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;
