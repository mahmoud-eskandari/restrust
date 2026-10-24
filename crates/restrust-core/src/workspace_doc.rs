//! The persisted workspace document: everything the client stores locally
//! (collections with their sync state, variables, environments, history, and
//! client settings). Serialization is camelCase JSON; the document is
//! versioned via `formatVersion`.

use crate::history::HISTORY_LIMIT;
use crate::model::{Collection, Environment, HistoryEntry, KeyValue, RequestDef, ResponseSummary};
use crate::revision::{CollectionRevision, REVISIONS_LIMIT};
use crate::share::AccessLevel;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Errors raised when reading or writing a [`WorkspaceDoc`].
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// An I/O failure occurred while reading/writing the backing store.
    /// (restrust-core itself never touches the filesystem; the caller's
    /// storage service reports its failures through this variant.)
    #[error("i/o error: {0}")]
    Io(String),
    /// The document could not be serialized to or deserialized from JSON.
    #[error("serialization error: {0}")]
    Serialization(String),
    /// The document's `formatVersion` is not one this build understands.
    /// A missing version is reported as `found: 0`.
    #[error("unsupported workspace document version {found} (expected {expected})")]
    UnsupportedVersion { found: u64, expected: u64 },
}

/// Synchronization state of a stored collection (PRD §7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SyncState {
    /// Local-only; never pushed to a server.
    Local,
    /// In sync with the server at the given revision.
    Synced { revision: u64 },
    /// Locally edited on top of the given last-known server revision.
    PendingEdits { base_revision: u64 },
    /// Access was revoked by the server (confirmed authorization denial, not
    /// a transient network error).
    Revoked,
}

/// The namespace a server-side collection belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NamespaceKind {
    Personal,
    Organization,
}

/// Reference to a namespace (personal user or organization).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceRef {
    pub kind: NamespaceKind,
    pub id: String,
    pub name: String,
}

/// Where a synced collection came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerOrigin {
    pub server_url: String,
    pub collection_id: String,
    pub namespace: NamespaceRef,
}

/// A collection as stored in the workspace, with its sync metadata.
/// `access` is the caller's own grant for server-synced collections
/// (owner-level access is implied when `sync` is `Local` and `origin` is
/// `None`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCollection {
    pub collection: Collection,
    pub sync: SyncState,
    pub origin: Option<ServerOrigin>,
    pub access: Option<AccessLevel>,
}

/// Client-level settings persisted with the workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientSettings {
    pub server_url: Option<String>,
    /// Remembered endpoints only; session credentials stay in memory.
    #[serde(default)]
    pub servers: Vec<String>,
    pub active_environment: Option<String>,
    pub theme: String,
}

impl Default for ClientSettings {
    fn default() -> Self {
        ClientSettings {
            server_url: None,
            servers: Vec::new(),
            active_environment: None,
            theme: "light".to_string(),
        }
    }
}

impl ClientSettings {
    /// Convenience constructor: no server, no environment, light theme.
    pub fn new() -> Self {
        ClientSettings::default()
    }
}

/// The whole persisted workspace: the personal and organization collections,
/// global variables, environments, history, and settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDoc {
    /// Always [`WorkspaceDoc::FORMAT_VERSION`] for documents this build writes.
    pub format_version: u32,
    pub personal: Vec<StoredCollection>,
    /// Organization collections (synced, or local copies of revoked sources).
    pub organizations: Vec<StoredCollection>,
    pub globals: Vec<KeyValue>,
    pub environments: Vec<Environment>,
    pub history: Vec<HistoryEntry>,
    pub settings: ClientSettings,
    /// Git-style change log per local collection id (RestRust-native history,
    /// §"collection revisions"). Entries are newest-last, capped at
    /// [`REVISIONS_LIMIT`] per collection. Keyed by *local* collection id;
    /// server-fetched entries replace/merge by revision number.
    #[serde(default)]
    pub collection_revisions: BTreeMap<String, Vec<CollectionRevision>>,
}

impl Default for WorkspaceDoc {
    fn default() -> Self {
        WorkspaceDoc::new()
    }
}

impl WorkspaceDoc {
    /// The only `formatVersion` this build writes and accepts.
    pub const FORMAT_VERSION: u32 = 1;

    /// An empty personal-local workspace with default client settings.
    pub fn new() -> Self {
        WorkspaceDoc {
            format_version: WorkspaceDoc::FORMAT_VERSION,
            personal: Vec::new(),
            organizations: Vec::new(),
            globals: Vec::new(),
            environments: Vec::new(),
            history: Vec::new(),
            settings: ClientSettings::default(),
            collection_revisions: BTreeMap::new(),
        }
    }

    /// Serializes the document to pretty-printed JSON (the on-disk format).
    pub fn to_json(&self) -> Result<String, StoreError> {
        serde_json::to_string_pretty(self).map_err(|e| StoreError::Serialization(e.to_string()))
    }

    /// Parses a document previously written by [`WorkspaceDoc::to_json`].
    /// Rejects any `formatVersion` other than
    /// [`FORMAT_VERSION`](WorkspaceDoc::FORMAT_VERSION) (a missing version is
    /// treated as unsupported and reported as `found: 0`).
    pub fn from_json(json: &str) -> Result<Self, StoreError> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|e| StoreError::Serialization(e.to_string()))?;
        let found = value.get("formatVersion").and_then(|v| v.as_u64());
        match found {
            Some(v) if v == WorkspaceDoc::FORMAT_VERSION as u64 => {}
            other => {
                return Err(StoreError::UnsupportedVersion {
                    found: other.unwrap_or(0),
                    expected: WorkspaceDoc::FORMAT_VERSION as u64,
                })
            }
        }
        serde_json::from_value(value).map_err(|e| StoreError::Serialization(e.to_string()))
    }

    /// Appends a history entry, keeping only the newest
    /// [`HISTORY_LIMIT`](crate::history::HISTORY_LIMIT) entries.
    pub fn push_history(&mut self, entry: HistoryEntry) {
        self.history.push(entry);
        let overflow = self.history.len().saturating_sub(HISTORY_LIMIT);
        if overflow > 0 {
            self.history.drain(..overflow);
        }
    }

    /// Records an executed request as a new history entry (fresh id and
    /// RFC3339 timestamp) and applies the history cap.
    pub fn add_history(&mut self, request: RequestDef, response: Option<ResponseSummary>) {
        let entry = HistoryEntry {
            id: crate::model::new_id(),
            time: crate::model::now_rfc3339(),
            request,
            response,
        };
        self.push_history(entry);
    }

    // --- collection revisions (git-style history) --------------------------------

    /// Appends a revision to a collection's log, keeping only the newest
    /// [`REVISIONS_LIMIT`] entries.
    pub fn push_revision(&mut self, collection_id: &str, revision: CollectionRevision) {
        let log = self
            .collection_revisions
            .entry(collection_id.to_string())
            .or_default();
        log.push(revision);
        let overflow = log.len().saturating_sub(REVISIONS_LIMIT);
        if overflow > 0 {
            log.drain(..overflow);
        }
    }

    /// The stored log of a collection (may be empty).
    pub fn revisions_of(&self, collection_id: &str) -> &[CollectionRevision] {
        self.collection_revisions
            .get(collection_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The hash of the newest recorded revision for a collection, if any.
    pub fn last_revision_hash(&self, collection_id: &str) -> Option<u64> {
        self.revisions_of(collection_id)
            .last()
            .map(|r| r.content_hash)
    }

    /// Assigns `revision` to every pending local checkpoint (revision `0`) of
    /// a collection — called after a successful push so the log stays a
    /// linear story ("local edit" becomes the server revision it landed as).
    pub fn mark_pending_pushed(&mut self, collection_id: &str, revision: u64) {
        if let Some(log) = self.collection_revisions.get_mut(collection_id) {
            for entry in log.iter_mut().filter(|entry| entry.revision == 0) {
                entry.revision = revision;
            }
        }
    }

    /// Replaces all entries at revision >= 1 of a collection with the
    /// server-fetched log (the authoritative per-push history), keeping local
    /// checkpoints (revision `0`) that are newer than the oldest server
    /// entry. Server entries may arrive unordered; they are sorted ascending.
    pub fn merge_server_revisions(
        &mut self,
        collection_id: &str,
        mut server_entries: Vec<CollectionRevision>,
    ) {
        server_entries.retain(|entry| entry.revision > 0);
        server_entries.sort_by_key(|entry| entry.revision);
        let local_pending: Vec<CollectionRevision> = self
            .revisions_of(collection_id)
            .iter()
            .filter(|entry| entry.revision == 0)
            .cloned()
            .collect();
        let mut merged = server_entries;
        merged.extend(local_pending);
        merged.sort_by_key(|entry| (entry.revision == 0, entry.time.clone()));
        self.collection_revisions
            .insert(collection_id.to_string(), merged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthConfig, AuthKind, HttpMethod};

    fn request(id: &str) -> RequestDef {
        RequestDef::new(HttpMethod::Get, format!("req {id}"), "https://api.test/x")
    }

    fn stored(
        id: &str,
        sync: SyncState,
        origin: Option<ServerOrigin>,
        access: Option<AccessLevel>,
    ) -> StoredCollection {
        StoredCollection {
            collection: Collection::new(format!("collection {id}")),
            sync,
            origin,
            access,
        }
    }

    fn origin(kind: NamespaceKind) -> ServerOrigin {
        ServerOrigin {
            server_url: "https://sync.example.test".to_string(),
            collection_id: "c-server-1".to_string(),
            namespace: NamespaceRef {
                kind,
                id: "ns-1".to_string(),
                name: "alice".to_string(),
            },
        }
    }

    fn sample_doc() -> WorkspaceDoc {
        WorkspaceDoc {
            format_version: 1,
            personal: vec![
                stored("local", SyncState::Local, None, None),
                stored(
                    "synced",
                    SyncState::Synced { revision: 7 },
                    Some(origin(NamespaceKind::Personal)),
                    Some(AccessLevel::FullAccess),
                ),
            ],
            organizations: vec![
                stored(
                    "pending",
                    SyncState::PendingEdits { base_revision: 3 },
                    Some(origin(NamespaceKind::Organization)),
                    Some(AccessLevel::ReadOnly),
                ),
                stored(
                    "revoked",
                    SyncState::Revoked,
                    Some(origin(NamespaceKind::Organization)),
                    None,
                ),
            ],
            globals: vec![KeyValue::new("global_var", "gv")],
            environments: vec![Environment {
                id: "e1".to_string(),
                name: "Dev".to_string(),
                values: vec![KeyValue::new("host", "localhost:8080")],
            }],
            history: vec![HistoryEntry {
                id: "h1".to_string(),
                time: "2026-10-20T10:00:00Z".to_string(),
                request: request("r1"),
                response: Some(ResponseSummary {
                    status: 200,
                    reason: "OK".to_string(),
                    size: 512,
                    duration_ms: 15,
                    time: "2026-10-20T10:00:01Z".to_string(),
                }),
            }],
            settings: ClientSettings {
                server_url: Some("https://sync.example.test".to_string()),
                servers: Vec::new(),
                active_environment: Some("Dev".to_string()),
                theme: "dark".to_string(),
            },
            collection_revisions: BTreeMap::new(),
        }
    }

    #[test]
    fn new_is_empty_personal_local() {
        let doc = WorkspaceDoc::new();
        assert_eq!(doc.format_version, 1);
        assert!(doc.personal.is_empty());
        assert!(doc.organizations.is_empty());
        assert!(doc.globals.is_empty());
        assert!(doc.environments.is_empty());
        assert!(doc.history.is_empty());
        assert_eq!(
            doc.settings,
            ClientSettings {
                server_url: None,
                servers: Vec::new(),
                active_environment: None,
                theme: "light".to_string()
            }
        );
    }

    #[test]
    fn roundtrip_includes_all_sync_states_and_origin() {
        let doc = sample_doc();
        let sync_states: Vec<SyncState> = doc
            .personal
            .iter()
            .chain(doc.organizations.iter())
            .map(|s| s.sync.clone())
            .collect();
        assert!(sync_states.contains(&SyncState::Local));
        assert!(sync_states.contains(&SyncState::Synced { revision: 7 }));
        assert!(sync_states.contains(&SyncState::PendingEdits { base_revision: 3 }));
        assert!(sync_states.contains(&SyncState::Revoked));
        assert_eq!(
            doc.organizations[1].origin.as_ref().unwrap().namespace.kind,
            NamespaceKind::Organization
        );

        // "Save data and read it back" round-trip.
        let json = doc.to_json().unwrap();
        let back = WorkspaceDoc::from_json(&json).unwrap();
        assert_eq!(doc, back);
    }

    #[test]
    fn json_uses_camel_case_keys() {
        let json = sample_doc().to_json().unwrap();
        assert!(json.contains("\"formatVersion\": 1"));
        assert!(json.contains("\"pendingEdits\""));
        assert!(json.contains("\"baseRevision\": 3"));
        assert!(json.contains("\"serverUrl\""));
        assert!(json.contains("\"fullAccess\""));
        assert!(json.contains("\"readOnly\""));
    }

    #[test]
    fn from_json_rejects_other_versions() {
        let json = sample_doc().to_json().unwrap();

        let bumped = json.replace("\"formatVersion\": 1", "\"formatVersion\": 2");
        match WorkspaceDoc::from_json(&bumped) {
            Err(StoreError::UnsupportedVersion {
                found: 2,
                expected: 1,
            }) => {}
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }

        let missing = json.replace("\"formatVersion\": 1,\n", "");
        match WorkspaceDoc::from_json(&missing) {
            Err(StoreError::UnsupportedVersion {
                found: 0,
                expected: 1,
            }) => {}
            other => panic!("expected UnsupportedVersion for missing version, got {other:?}"),
        }
    }

    #[test]
    fn from_json_rejects_malformed_json() {
        assert!(matches!(
            WorkspaceDoc::from_json("not json at all"),
            Err(StoreError::Serialization(_))
        ));
    }

    #[test]
    fn empty_doc_roundtrip() {
        let doc = WorkspaceDoc::new();
        let back = WorkspaceDoc::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(doc, back);
    }

    #[test]
    fn push_history_keeps_newest_limit() {
        let mut doc = WorkspaceDoc::new();
        for i in 0..(HISTORY_LIMIT + 5) {
            doc.push_history(HistoryEntry {
                id: format!("h{i}"),
                time: "2026-10-20T10:00:00Z".to_string(),
                request: request(&i.to_string()),
                response: None,
            });
        }
        assert_eq!(doc.history.len(), HISTORY_LIMIT);
        // Oldest 5 were dropped; newest kept in order.
        assert_eq!(doc.history.first().unwrap().id, "h5");
        assert_eq!(
            doc.history.last().unwrap().id,
            format!("h{}", HISTORY_LIMIT + 4)
        );
    }

    #[test]
    fn add_history_generates_id_and_time() {
        let mut doc = WorkspaceDoc::new();
        let mut req = request("x");
        req.auth = Some(AuthConfig {
            kind: AuthKind::Bearer {
                token: "t".to_string(),
            },
        });
        doc.add_history(
            req.clone(),
            Some(ResponseSummary {
                status: 201,
                reason: "Created".to_string(),
                size: 0,
                duration_ms: 3,
                time: "2026-10-20T10:00:02Z".to_string(),
            }),
        );
        assert_eq!(doc.history.len(), 1);
        let entry = &doc.history[0];
        assert!(!entry.id.is_empty());
        assert!(entry.time.ends_with('Z'));
        assert_eq!(entry.request, req);
        assert_eq!(entry.response.as_ref().unwrap().status, 201);

        // Cap applies through add_history too.
        for i in 0..(HISTORY_LIMIT + 2) {
            doc.add_history(request(&format!("cap{i}")), None);
        }
        assert_eq!(doc.history.len(), HISTORY_LIMIT);
    }

    fn revision_entry(revision: u64, time: &str) -> crate::revision::CollectionRevision {
        crate::revision::CollectionRevision {
            revision,
            content_hash: revision,
            time: time.to_string(),
            author: "alice".to_string(),
            summary: "s".to_string(),
            changes: Vec::new(),
        }
    }

    #[test]
    fn push_revision_caps_and_keeps_order() {
        let mut doc = WorkspaceDoc::new();
        for i in 0..(REVISIONS_LIMIT + 5) {
            doc.push_revision("c1", revision_entry(i as u64, &format!("t{i}")));
        }
        assert_eq!(doc.revisions_of("c1").len(), REVISIONS_LIMIT);
        // Oldest dropped, newest kept.
        assert_eq!(doc.revisions_of("c1")[0].revision, 5);
        assert_eq!(
            doc.revisions_of("c1").last().unwrap().revision,
            REVISIONS_LIMIT as u64 + 4
        );
        assert!(doc.revisions_of("other").is_empty());
    }

    #[test]
    fn pending_checkpoints_are_rewritten_on_push() {
        let mut doc = WorkspaceDoc::new();
        doc.push_revision("c1", revision_entry(0, "t1"));
        doc.push_revision("c1", revision_entry(0, "t2"));
        doc.push_revision("c1", revision_entry(3, "t0"));
        doc.mark_pending_pushed("c1", 4);
        let revisions: Vec<u64> = doc
            .revisions_of("c1")
            .iter()
            .map(|entry| entry.revision)
            .collect();
        assert_eq!(revisions, [4, 4, 3]);
    }

    #[test]
    fn server_revisions_replace_pushed_entries_and_keep_local_checkpoints() {
        let mut doc = WorkspaceDoc::new();
        doc.push_revision("c1", revision_entry(1, "t1"));
        doc.push_revision("c1", revision_entry(0, "t2-local"));
        doc.merge_server_revisions("c1", vec![revision_entry(2, "t2"), revision_entry(1, "t1")]);
        let revisions: Vec<u64> = doc
            .revisions_of("c1")
            .iter()
            .map(|entry| entry.revision)
            .collect();
        // Server 1 replaced local 1; server 2 appended; the pending local
        // checkpoint (revision 0) sorts last.
        assert_eq!(revisions, [1, 2, 0]);
    }

    #[test]
    fn collection_revisions_roundtrip_and_legacy_docs_parse() {
        let mut doc = WorkspaceDoc::new();
        doc.push_revision("c1", revision_entry(1, "2026-10-24T10:00:00Z"));
        let json = doc.to_json().unwrap();
        assert!(json.contains("collectionRevisions"));
        let back = WorkspaceDoc::from_json(&json).unwrap();
        assert_eq!(back.revisions_of("c1")[0].revision, 1);

        // A document written by an older build (no revisions field) parses
        // with an empty log.
        let legacy = r#"{"formatVersion":1,"personal":[],"organizations":[],"globals":[],"environments":[],"history":[],"settings":{"serverUrl":null,"servers":[],"activeEnvironment":null,"theme":"dark"}}"#;
        let doc = WorkspaceDoc::from_json(legacy).unwrap();
        assert!(doc.collection_revisions.is_empty());
    }
}
