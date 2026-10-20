//! restrust-interchange: import/export bridges between the restrust core
//! model and external formats — Postman collections (v2.1 and v3.0) and
//! environments, OpenAPI 3.0/3.1 documents, and cURL commands — plus
//! code-snippet generation for saved requests.
//!
//! Every import returns an [`Imported`] whose [`Report`] lists, in
//! human-readable entries, what was preserved, what was imported only
//! approximately (lossy), and what could not be represented at all.

pub mod curl;
pub mod openapi;
pub mod postman;
pub mod snippet;

use restrust_core::model::{Collection, Environment};
use serde_json::Value;

/// Errors raised by interchange import/export operations.
#[derive(Debug, thiserror::Error)]
pub enum InterchangeError {
    /// The input could not be parsed (malformed JSON/YAML/shell syntax, or a
    /// required structure is missing).
    #[error("bad input: {0}")]
    BadInput(String),
    /// The input parses but declares a format this crate does not support.
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    /// An operation failed with extra context.
    #[error("{context}: {message}")]
    WithDetail { context: String, message: String },
}

/// Human-readable notes produced by an import: what was preserved, what was
/// imported with loss, and what could not be imported at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Data preserved as-is (possibly inert, e.g. script text that restrust
    /// stores but does not execute).
    pub preserved: Vec<String>,
    /// Data imported only partially or approximately.
    pub lossy: Vec<String>,
    /// Data that could not be represented and was dropped.
    pub unsupported: Vec<String>,
}

impl Report {
    pub(crate) fn note_preserved(&mut self, entry: impl Into<String>) {
        self.preserved.push(entry.into());
    }

    pub(crate) fn note_lossy(&mut self, entry: impl Into<String>) {
        self.lossy.push(entry.into());
    }

    pub(crate) fn note_unsupported(&mut self, entry: impl Into<String>) {
        self.unsupported.push(entry.into());
    }
}

/// The result of an import: collections and/or environments plus the report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub collections: Vec<Collection>,
    pub environments: Vec<Environment>,
    pub report: Report,
}

/// Reads a Postman-style description field, which may be a plain string or
/// an object of the shape `{"content": "...", "type": "..."}`.
pub(crate) fn description_from(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Object(o)) => o.get("content").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}
