//! Git-style change history for collections (RestRust-native format).
//!
//! Every meaningful save of a collection can record a [`CollectionRevision`]:
//! who changed it, when, a generated summary, and the *scope* of the change —
//! a list of [`ChangeRecord`]s naming each affected request/folder/aspect
//! (e.g. "Request `Login` modified — url, headers"). This is richer than a
//! plain revision counter: the log answers "what exactly changed, and where".
//!
//! The log lives in the client workspace document (`WorkspaceDoc::
//! collection_revisions`, keyed by local collection id) and, server-side, in
//! the `collection_revisions` table fed by the same diff on every PUT.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Collection, CollectionItem, KeyValue};

/// Maximum revision entries retained per collection; older entries are
/// dropped (oldest first).
pub const REVISIONS_LIMIT: usize = 200;

/// What happened to a change target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Removed,
    Modified,
    Renamed,
}

impl ChangeKind {
    /// Single-character marker for compact history rows (`+ − ~ R`).
    pub fn marker(self) -> &'static str {
        match self {
            ChangeKind::Added => "+",
            ChangeKind::Removed => "−",
            ChangeKind::Modified => "~",
            ChangeKind::Renamed => "R",
        }
    }

    /// Lower-case word for summaries ("added", "removed", …).
    pub fn verb(self) -> &'static str {
        match self {
            ChangeKind::Added => "added",
            ChangeKind::Removed => "removed",
            ChangeKind::Modified => "modified",
            ChangeKind::Renamed => "renamed",
        }
    }
}

/// What a change record points at. Node targets carry the name *after* the
/// change (for `Removed`, the name before it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ChangeTarget {
    Collection,
    Request { id: String, name: String },
    Folder { id: String, name: String },
    Variables,
    Auth,
}

impl ChangeTarget {
    /// Human-readable noun used in summaries and rows.
    pub fn describe(&self) -> String {
        match self {
            ChangeTarget::Collection => "collection".to_string(),
            ChangeTarget::Request { name, .. } => format!("request “{name}”"),
            ChangeTarget::Folder { name, .. } => format!("folder “{name}”"),
            ChangeTarget::Variables => "collection variables".to_string(),
            ChangeTarget::Auth => "collection auth".to_string(),
        }
    }
}

/// One scoped change: a kind, its target, and the aspects (fields) that
/// changed — the "domain" of the change (e.g. `["url", "headers"]`, a moved
/// node is `["moved"]`, an added request carries `["METHOD url"]`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeRecord {
    pub kind: ChangeKind,
    pub target: ChangeTarget,
    pub fields: Vec<String>,
}

/// One entry of a collection's history: a commit-like record of a save, push
/// or pull.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionRevision {
    /// Server revision this entry belongs to; `0` marks a local checkpoint
    /// that has not been pushed (rewritten to the real revision on push).
    pub revision: u64,
    /// Hash of the full collection content after this change
    /// ([`content_hash`]); used to skip recording no-op saves.
    pub content_hash: u64,
    /// RFC3339 UTC string.
    pub time: String,
    /// Author username (client entries: signed-in user, `local` when
    /// offline; server entries: the uploading user; pulls: the namespace
    /// owner's name).
    pub author: String,
    /// Generated one-line summary, e.g. "added 2 requests, modified 1
    /// request".
    pub summary: String,
    pub changes: Vec<ChangeRecord>,
}

/// Compares two collection snapshots and returns the scoped list of changes.
///
/// Items are matched by id (stable UUIDs), so renames and moves are
/// recognized instead of showing as remove+add.
pub fn describe_changes(old: &Collection, new: &Collection) -> Vec<ChangeRecord> {
    let mut changes = Vec::new();

    if old.name != new.name {
        changes.push(ChangeRecord {
            kind: ChangeKind::Renamed,
            target: ChangeTarget::Collection,
            fields: vec!["name".to_string()],
        });
    }
    if old.description != new.description {
        changes.push(ChangeRecord {
            kind: ChangeKind::Modified,
            target: ChangeTarget::Collection,
            fields: vec!["description".to_string()],
        });
    }
    if old.events != new.events {
        changes.push(ChangeRecord {
            kind: ChangeKind::Modified,
            target: ChangeTarget::Collection,
            fields: vec!["scripts".to_string()],
        });
    }
    let variable_fields = diff_variables(&old.variables, &new.variables);
    if !variable_fields.is_empty() {
        changes.push(ChangeRecord {
            kind: ChangeKind::Modified,
            target: ChangeTarget::Variables,
            fields: variable_fields,
        });
    }
    if old.auth != new.auth {
        changes.push(ChangeRecord {
            kind: ChangeKind::Modified,
            target: ChangeTarget::Auth,
            fields: vec!["auth".to_string()],
        });
    }

    let mut old_items = BTreeMap::new();
    flatten(&old.items, String::new(), &mut old_items);
    let mut new_items = BTreeMap::new();
    flatten(&new.items, String::new(), &mut new_items);

    for (id, item) in &new_items {
        if !old_items.contains_key(id) {
            changes.push(added_record(item.item));
        }
    }
    for (id, item) in &old_items {
        if new_items.contains_key(id) {
            continue;
        }
        changes.push(ChangeRecord {
            kind: ChangeKind::Removed,
            target: target_of(item.item),
            fields: Vec::new(),
        });
    }
    for (id, new_item) in &new_items {
        let Some(old_item) = old_items.get(id) else {
            continue;
        };
        let mut fields = Vec::new();
        if old_item.path != new_item.path {
            fields.push("moved".to_string());
        }
        match (old_item.item, new_item.item) {
            (CollectionItem::Request(old_req), CollectionItem::Request(new_req)) => {
                if old_req.name != new_req.name {
                    fields.push("name".to_string());
                }
                if old_req.method != new_req.method {
                    fields.push("method".to_string());
                }
                if old_req.url != new_req.url {
                    fields.push("url".to_string());
                }
                if old_req.description != new_req.description {
                    fields.push("description".to_string());
                }
                if old_req.query != new_req.query {
                    fields.push("params".to_string());
                }
                if old_req.headers != new_req.headers {
                    fields.push("headers".to_string());
                }
                if old_req.body != new_req.body {
                    fields.push("body".to_string());
                }
                if old_req.auth != new_req.auth {
                    fields.push("auth".to_string());
                }
            }
            (CollectionItem::Folder(old_f), CollectionItem::Folder(new_f)) => {
                if old_f.name != new_f.name {
                    fields.push("name".to_string());
                }
                if old_f.description != new_f.description {
                    fields.push("description".to_string());
                }
                if old_f.auth != new_f.auth {
                    fields.push("auth".to_string());
                }
            }
            _ => {} // an id reused across kinds is treated as remove+add above
        }
        if !fields.is_empty() {
            let renamed = fields.iter().any(|f| f == "name");
            changes.push(ChangeRecord {
                kind: if renamed {
                    ChangeKind::Renamed
                } else {
                    ChangeKind::Modified
                },
                target: target_of(new_item.item),
                fields,
            });
        }
    }
    changes
}

/// Builds a revision entry from two snapshots: hashes, diffs and summarizes.
pub fn revision_for(
    revision: u64,
    author: &str,
    old: &Collection,
    new: &Collection,
) -> CollectionRevision {
    let changes = describe_changes(old, new);
    CollectionRevision {
        revision,
        content_hash: content_hash(new),
        time: crate::model::now_rfc3339(),
        author: author.to_string(),
        summary: summarize(&changes),
        changes,
    }
}

/// Stable content hash of a collection (FNV-1a over its canonical JSON) —
/// used to skip no-op revisions without keeping full snapshots.
pub fn content_hash(collection: &Collection) -> u64 {
    let json = serde_json::to_string(collection).unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in json.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// One-line summary of a change list, e.g.
/// `"added 2 requests, removed 1 folder, modified “Auth” collection aspects"`.
/// Empty when nothing changed.
pub fn summarize(changes: &[ChangeRecord]) -> String {
    if changes.is_empty() {
        return String::new();
    }
    // Count by (kind, noun) so summaries stay short ("added 3 requests"
    // instead of three lines); collection/variables/auth targets are named.
    let mut counts: Vec<(String, usize)> = Vec::new();
    for change in changes {
        let noun = match &change.target {
            ChangeTarget::Request { .. } => "requests".to_string(),
            ChangeTarget::Folder { .. } => "folders".to_string(),
            other => other.describe(),
        };
        let key = format!("{} {noun}", change.kind.verb());
        match counts.iter_mut().find(|(existing, _)| *existing == key) {
            Some((_, count)) => *count += 1,
            None => counts.push((key, 1)),
        }
    }
    counts
        .iter()
        .map(|(key, count)| match key.split_once(' ') {
            Some((verb, noun)) if noun.ends_with('s') && !noun.ends_with("ss") => {
                if *count == 1 {
                    format!("{verb} 1 {}", noun.trim_end_matches('s'))
                } else {
                    format!("{verb} {count} {noun}")
                }
            }
            _ => {
                if *count == 1 {
                    key.clone()
                } else {
                    format!("{key} ×{count}")
                }
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// --- internals -----------------------------------------------------------------

struct FlatItem<'a> {
    item: &'a CollectionItem,
    /// Slash-joined parent folder names ("" = collection root).
    path: String,
}

fn flatten<'a>(
    items: &'a [CollectionItem],
    parent: String,
    out: &mut BTreeMap<String, FlatItem<'a>>,
) {
    for item in items {
        let path = parent.clone();
        out.insert(item.id().to_string(), FlatItem { item, path });
        if let CollectionItem::Folder(folder) = item {
            let child_parent = if parent.is_empty() {
                folder.name.clone()
            } else {
                format!("{parent}/{}", folder.name)
            };
            flatten(&folder.items, child_parent, out);
        }
    }
}

fn target_of(item: &CollectionItem) -> ChangeTarget {
    match item {
        CollectionItem::Request(request) => ChangeTarget::Request {
            id: request.id.clone(),
            name: request.name.clone(),
        },
        CollectionItem::Folder(folder) => ChangeTarget::Folder {
            id: folder.id.clone(),
            name: folder.name.clone(),
        },
    }
}

fn added_record(item: &CollectionItem) -> ChangeRecord {
    // An added request shows its method + URL as the change scope.
    let fields = match item {
        CollectionItem::Request(request) => {
            vec![format!("{} {}", request.method, request.url)]
        }
        CollectionItem::Folder(_) => Vec::new(),
    };
    ChangeRecord {
        kind: ChangeKind::Added,
        target: target_of(item),
        fields,
    }
}

/// Keys of variable rows that were added, removed or changed (key, value,
/// enabled).
fn diff_variables(old: &[KeyValue], new: &[KeyValue]) -> Vec<String> {
    let mut fields = Vec::new();
    for row in old {
        if !new.iter().any(|r| r.key == row.key) {
            fields.push(format!("-{}", row.key));
        }
    }
    for row in new {
        match old.iter().find(|r| r.key == row.key) {
            None => fields.push(format!("+{}", row.key)),
            Some(old_row) if old_row != row => fields.push(format!("~{}", row.key)),
            Some(_) => {}
        }
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BodyKind, HttpMethod, RequestBody, RequestDef};

    fn collection(name: &str) -> Collection {
        Collection::new(name)
    }

    fn request(id: &str, name: &str, url: &str) -> CollectionItem {
        let mut def = RequestDef::new(HttpMethod::Get, name, url);
        def.id = id.to_string();
        CollectionItem::Request(def)
    }

    #[test]
    fn identical_collections_have_no_changes() {
        let mut a = collection("API");
        a.items.push(request("r1", "Login", "https://x/login"));
        let b = a.clone();
        assert!(describe_changes(&a, &b).is_empty());
        assert_eq!(summarize(&describe_changes(&a, &b)), "");
    }

    #[test]
    fn rename_is_recognized_by_id() {
        let mut old = collection("API");
        old.items.push(request("r1", "Login", "https://x/login"));
        let mut new = old.clone();
        if let CollectionItem::Request(req) = &mut new.items[0] {
            req.name = "Sign in".to_string();
        }
        let changes = describe_changes(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Renamed);
        assert_eq!(
            changes[0].target,
            ChangeTarget::Request {
                id: "r1".to_string(),
                name: "Sign in".to_string()
            }
        );
        assert_eq!(changes[0].fields, ["name"]);
        assert_eq!(summarize(&changes), "renamed 1 request");
    }

    #[test]
    fn add_remove_and_field_scope() {
        let mut old = collection("API");
        old.items.push(request("r1", "Login", "https://x/login"));
        let mut new = collection("API");
        new.items.push(request("r1", "Login", "https://x/signin"));
        new.items.push(request("r2", "List", "https://x/list"));

        let changes = describe_changes(&old, &new);
        assert_eq!(changes.len(), 2);
        let added = changes
            .iter()
            .find(|c| c.kind == ChangeKind::Added)
            .expect("added request");
        assert_eq!(
            added.target,
            ChangeTarget::Request {
                id: "r2".to_string(),
                name: "List".to_string()
            }
        );
        assert_eq!(added.fields, ["GET https://x/list"]);
        let modified = changes
            .iter()
            .find(|c| c.kind == ChangeKind::Modified)
            .expect("modified request");
        assert_eq!(modified.fields, ["url"]);
        assert_eq!(summarize(&changes), "added 1 request, modified 1 request");
    }

    #[test]
    fn removed_request_is_reported_with_its_old_name() {
        let mut old = collection("API");
        old.items.push(request("r1", "Login", "https://x/login"));
        let new = collection("API");
        let changes = describe_changes(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Removed);
        assert_eq!(
            changes[0].target,
            ChangeTarget::Request {
                id: "r1".to_string(),
                name: "Login".to_string()
            }
        );
    }

    #[test]
    fn move_is_detected_through_the_parent_path() {
        let mut old = collection("API");
        old.items.push(request("r1", "Login", "https://x/login"));
        let mut new = collection("API");
        new.items.push(CollectionItem::Folder(crate::model::Folder {
            id: "f1".to_string(),
            name: "Auth".to_string(),
            description: None,
            items: vec![request("r1", "Login", "https://x/login")],
            auth: None,
        }));
        let changes = describe_changes(&old, &new);
        let moved = changes
            .iter()
            .find(|c| matches!(c.target, ChangeTarget::Request { .. }))
            .expect("moved request");
        assert_eq!(moved.fields, ["moved"]);
    }

    #[test]
    fn collection_level_changes_are_scoped() {
        let mut old = collection("Old name");
        old.variables.push(KeyValue::new("a", "1"));
        old.variables.push(KeyValue::new("b", "2"));
        let mut new = collection("New name");
        new.variables.push(KeyValue::new("a", "1"));
        new.variables.push(KeyValue::new("b", "3"));
        new.variables.push(KeyValue::new("c", "4"));
        let changes = describe_changes(&old, &new);
        let renamed = changes.iter().find(|c| c.kind == ChangeKind::Renamed);
        assert!(renamed.is_some_and(|c| matches!(c.target, ChangeTarget::Collection)));
        let vars = changes
            .iter()
            .find(|c| c.target == ChangeTarget::Variables)
            .expect("variables change");
        assert_eq!(vars.fields, ["~b", "+c"]);
    }

    #[test]
    fn body_changes_show_as_body_scope() {
        let mut old = collection("API");
        old.items.push(request("r1", "Login", "https://x/login"));
        let mut new = old.clone();
        if let CollectionItem::Request(req) = &mut new.items[0] {
            req.body = RequestBody {
                kind: BodyKind::Raw {
                    mime: "text/plain".into(),
                },
                raw: Some("hello".into()),
                rows: Vec::new(),
            };
        }
        let changes = describe_changes(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].fields, ["body"]);
    }

    #[test]
    fn revision_for_carries_summary_hash_and_author() {
        let old = collection("API");
        let mut new = collection("Renamed");
        new.items.push(request("r1", "Login", "https://x/login"));
        let rev = revision_for(7, "alice", &old, &new);
        assert_eq!(rev.revision, 7);
        assert_eq!(rev.author, "alice");
        assert_eq!(rev.content_hash, content_hash(&new));
        assert!(rev.summary.contains("renamed"));
        assert!(!rev.changes.is_empty());
        assert!(rev.time.ends_with('Z'));
    }

    #[test]
    fn content_hash_distinguishes_content_and_ignores_nothing() {
        let mut a = collection("API");
        a.items.push(request("r1", "Login", "https://x/login"));
        assert_eq!(content_hash(&a), content_hash(&a.clone()));
        let mut b = a.clone();
        if let CollectionItem::Request(req) = &mut b.items[0] {
            req.url = "https://x/other".to_string();
        }
        assert_ne!(content_hash(&a), content_hash(&b));
    }
}
