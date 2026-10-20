//! Request history: the log of executed requests kept by the workspace
//! document, capped at [`HISTORY_LIMIT`] newest entries.

pub use crate::model::{HistoryEntry, ResponseSummary};

/// Maximum number of history entries retained; older entries are dropped.
pub const HISTORY_LIMIT: usize = 200;
