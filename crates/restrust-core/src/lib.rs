//! restrust-core: domain models and application logic for the restrust API
//! client. UI- and OS-independent — no egui, no tokio, no filesystem or
//! threading; compiles for native targets and wasm32-unknown-unknown.
//!
//! Layout (per mydocs/design/CONTRACT.md §2):
//! - [`model`]: collections, requests, auth, bodies, environments, history
//! - [`vars`]: `{{var}}` interpolation with scope precedence
//! - [`share`]: sharing grants and access levels
//! - [`workspace_doc`]: the persisted workspace document and its store errors
//! - [`history`]: history types and the retention limit
//! - [`revision`]: git-style collection change history (diff, summary, log)

pub mod execution;
pub mod history;
pub mod model;
pub mod revision;
pub mod share;
pub mod vars;
pub mod workspace_doc;
