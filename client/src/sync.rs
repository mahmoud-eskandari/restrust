//! Client-side sync engine (CONTRACT §5.3) — pure, UI-free, unit-testable.
//!
//! The one hard rule (PRD §7 guardrail, ADR-02): only a *confirmed* `403
//! access_denied` (delivered here as [`SyncOutcome::Revoked`]) may move a
//! collection into [`SyncState::Revoked`]. Network errors and 401s never do.

use restrust_core::model::Collection;
use restrust_core::workspace_doc::{
    NamespaceKind, NamespaceRef, ServerOrigin, StoredCollection, SyncState, WorkspaceDoc,
};

/// What a sync exchange concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Nothing to do; local already matches the server.
    UpToDate,
    /// A push was accepted; the collection is in sync at `new_revision`.
    Pushed { new_revision: u64 },
    /// A pull replaced local content; in sync at `revision`.
    Pulled { revision: u64 },
    /// The server rejected the push (409); local edits are retained.
    Conflict { server_revision: u64 },
    /// A confirmed 403 access_denied — the only revocation trigger.
    Revoked,
    /// Transport-level failure; state is untouched (guardrail).
    NetworkError { message: String },
    /// 401 session expiry; state is untouched (never revocation).
    Unauthorized,
}

/// Plans a push for a stored collection: only `PendingEdits` produce one,
/// returning the base revision and a snapshot of the collection to send.
pub fn plan_push(local: &StoredCollection) -> Option<(u64, Collection)> {
    match local.sync {
        SyncState::PendingEdits { base_revision } => {
            Some((base_revision, local.collection.clone()))
        }
        _ => None,
    }
}

/// Base revision for a force-push (the 409 dialog's "Overwrite server copy").
pub fn plan_force_push(
    local: &StoredCollection,
    server_revision: u64,
) -> Option<(u64, Collection)> {
    match local.sync {
        SyncState::PendingEdits { .. } | SyncState::Synced { .. } => {
            Some((server_revision, local.collection.clone()))
        }
        _ => None,
    }
}

/// Marks a stored collection as locally edited. `Synced` degrades to
/// `PendingEdits` on the same base revision; every other state is unchanged
/// (local stays local; revoked stays revoked — the UI never edits revoked
/// collections).
pub fn mark_edited(stored: &mut StoredCollection) {
    if let SyncState::Synced { revision } = stored.sync {
        stored.sync = SyncState::PendingEdits {
            base_revision: revision,
        };
    }
}

/// Applies a sync outcome to the stored collection's state.
///
/// - `Pushed`/`Pulled` → `Synced`
/// - `Revoked` → `Revoked`
/// - `Conflict` → state unchanged (edits retained locally; the dialog decides
///   overwrite vs. keep-local)
/// - `NetworkError`/`Unauthorized`/`UpToDate` → state unchanged — the guardrail
///   keeps prior state on anything but a confirmed denial
pub fn apply_response(stored: &mut StoredCollection, outcome: &SyncOutcome) {
    match outcome {
        SyncOutcome::UpToDate => {}
        SyncOutcome::Pushed { new_revision } => {
            stored.sync = SyncState::Synced {
                revision: *new_revision,
            };
        }
        SyncOutcome::Pulled { revision } => {
            stored.sync = SyncState::Synced {
                revision: *revision,
            };
        }
        SyncOutcome::Conflict { .. } => {}
        SyncOutcome::Revoked => stored.sync = SyncState::Revoked,
        SyncOutcome::NetworkError { .. } | SyncOutcome::Unauthorized => {}
    }
}

/// One server-side collection as listed by `GET /collections`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerEntry {
    pub id: String,
    pub revision: u64,
}

/// A pull plan derived from the server list vs. local state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullPlan {
    /// Server collections whose content must be fetched (revision differs and
    /// no local edits pending).
    pub to_pull: Vec<String>,
    /// Local collections with pending edits that must push first.
    pub to_push: Vec<String>,
    /// Local `Revoked` collections to re-verify with one GET (a success
    /// restores them, a confirmed 403 keeps them revoked).
    pub to_verify: Vec<String>,
    /// Local pending edits whose server row disappeared (kept local; deletion
    /// propagation is a documented non-goal).
    pub orphaned_pending: Vec<String>,
}

/// Plans a pull pass. Local revoked collections are skipped for normal
/// exchange (only re-verified); pending edits push before pulling.
pub fn plan_pull(local: &[StoredCollection], server: &[ServerEntry]) -> PullPlan {
    let mut plan = PullPlan::default();
    let server_revs: Vec<(&ServerEntry, bool)> = server
        .iter()
        .map(|entry| {
            let exists_locally = local.iter().any(|stored| {
                stored
                    .origin
                    .as_ref()
                    .map(|o| o.collection_id == entry.id)
                    .unwrap_or(false)
            });
            (entry, exists_locally)
        })
        .collect();

    for stored in local {
        let Some(origin) = stored.origin.as_ref() else {
            continue; // local-only collections never appear in the exchange
        };
        let Some((entry, _)) = server_revs
            .iter()
            .find(|(entry, _)| entry.id == origin.collection_id)
            .copied()
        else {
            if matches!(stored.sync, SyncState::PendingEdits { .. }) {
                plan.orphaned_pending.push(stored.collection.id.clone());
            }
            continue;
        };
        match stored.sync {
            SyncState::Revoked => plan.to_verify.push(stored.collection.id.clone()),
            SyncState::PendingEdits { .. } => plan.to_push.push(stored.collection.id.clone()),
            SyncState::Synced { revision } => {
                if entry.revision != revision {
                    plan.to_pull.push(stored.collection.id.clone());
                }
            }
            SyncState::Local => {} // has an origin but Local: treat as needing a pull
        }
    }

    // Server collections with no local counterpart at all.
    for (entry, exists_locally) in server_revs {
        if !exists_locally {
            plan.to_pull.push(entry.id.clone());
        }
    }
    plan
}

/// Marks a local-only collection as synced after `POST /collections` + first
/// successful PUT: records the origin and the synced revision.
pub fn mark_enabled(
    stored: &mut StoredCollection,
    server_url: &str,
    server_id: &str,
    namespace: NamespaceRef,
    revision: u64,
) {
    stored.sync = SyncState::Synced { revision };
    stored.origin = Some(ServerOrigin {
        server_url: server_url.to_string(),
        collection_id: server_id.to_string(),
        namespace,
    });
    stored.access = None;
}

/// Revocation resolution "Save as personal local copy" (AC-05/06, R-09):
/// moves the retained collection into the personal namespace, clears its
/// origin and sets `sync = Local`. Returns the moved collection's name.
pub fn save_revoked_as_personal(doc: &mut WorkspaceDoc, collection_id: &str) -> Option<String> {
    let stored = remove_stored(doc, collection_id)?;
    let mut stored = stored;
    stored.sync = SyncState::Local;
    stored.origin = None;
    stored.access = None;
    let name = stored.collection.name.clone();
    doc.personal.push(stored);
    Some(name)
}

/// Revocation resolution "Remove local data" (R-09): explicitly deletes the
/// retained local copy. Nothing else in the codebase removes retained data.
pub fn remove_local_data(doc: &mut WorkspaceDoc, collection_id: &str) -> Option<String> {
    remove_stored(doc, collection_id).map(|s| s.collection.name)
}

fn remove_stored(doc: &mut WorkspaceDoc, collection_id: &str) -> Option<StoredCollection> {
    if let Some(pos) = doc
        .personal
        .iter()
        .position(|s| s.collection.id == collection_id)
    {
        return Some(doc.personal.remove(pos));
    }
    if let Some(pos) = doc
        .organizations
        .iter()
        .position(|s| s.collection.id == collection_id)
    {
        return Some(doc.organizations.remove(pos));
    }
    None
}

/// A personal namespace ref for the signed-in user.
pub fn personal_namespace(user_id: &str, username: &str) -> NamespaceRef {
    NamespaceRef {
        kind: NamespaceKind::Personal,
        id: user_id.to_string(),
        name: username.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use restrust_core::model::{Collection, HttpMethod, RequestDef};

    fn stored(id: &str, sync: SyncState, origin: Option<ServerOrigin>) -> StoredCollection {
        StoredCollection {
            collection: Collection {
                id: id.to_string(),
                name: format!("collection {id}"),
                description: None,
                items: vec![restrust_core::model::CollectionItem::Request(
                    RequestDef::new(HttpMethod::Get, "list", "https://api.test/x"),
                )],
                variables: Vec::new(),
                auth: None,
                events: Vec::new(),
            },
            sync,
            origin,
            access: None,
        }
    }

    fn origin(collection_id: &str, kind: NamespaceKind) -> ServerOrigin {
        ServerOrigin {
            server_url: "https://sync.test".to_string(),
            collection_id: collection_id.to_string(),
            namespace: NamespaceRef {
                kind,
                id: "ns-1".to_string(),
                name: "acme".to_string(),
            },
        }
    }

    #[test]
    fn ac02_pending_edits_survive_and_plan_a_push() {
        // Synced org collection, user edits offline, restarts: data + edits stay.
        let mut s = stored(
            "c1",
            SyncState::Synced { revision: 5 },
            Some(origin("s1", NamespaceKind::Organization)),
        );
        mark_edited(&mut s);
        assert_eq!(s.sync, SyncState::PendingEdits { base_revision: 5 });
        // The content itself is still there:
        assert_eq!(s.collection.count_requests(), 1);

        let (base, collection) = plan_push(&s).expect("pending edits must push");
        assert_eq!(base, 5);
        assert_eq!(collection.name, "collection c1");
    }

    #[test]
    fn non_pending_states_do_not_push() {
        for sync in [
            SyncState::Local,
            SyncState::Synced { revision: 3 },
            SyncState::Revoked,
        ] {
            let s = stored(
                "c",
                sync.clone(),
                Some(origin("s", NamespaceKind::Personal)),
            );
            assert!(plan_push(&s).is_none(), "no push from {sync:?}");
        }
    }

    #[test]
    fn push_accepted_moves_to_synced() {
        let mut s = stored("c1", SyncState::PendingEdits { base_revision: 2 }, None);
        apply_response(&mut s, &SyncOutcome::Pushed { new_revision: 3 });
        assert_eq!(s.sync, SyncState::Synced { revision: 3 });
    }

    #[test]
    fn ac03_conflict_retains_local_edits() {
        let mut s = stored("c1", SyncState::PendingEdits { base_revision: 2 }, None);
        apply_response(&mut s, &SyncOutcome::Conflict { server_revision: 9 });
        // Edits are retained locally; only the dialog resolves it.
        assert_eq!(s.sync, SyncState::PendingEdits { base_revision: 2 });
        let (base, _) = plan_push(&s).unwrap();
        assert_eq!(base, 2); // still pushing from the stale base
        let (force_base, _) = plan_force_push(&s, 9).unwrap();
        assert_eq!(force_base, 9); // overwrite uses the server revision
    }

    #[test]
    fn ac04_confirmed_denial_revokes_but_transport_never_does() {
        let mut s = stored(
            "c1",
            SyncState::Synced { revision: 4 },
            Some(origin("s1", NamespaceKind::Organization)),
        );
        apply_response(&mut s, &SyncOutcome::Revoked);
        assert_eq!(s.sync, SyncState::Revoked);

        // Guardrail: network errors keep the previous state.
        let mut s = stored(
            "c2",
            SyncState::Synced { revision: 4 },
            Some(origin("s2", NamespaceKind::Organization)),
        );
        apply_response(
            &mut s,
            &SyncOutcome::NetworkError {
                message: "connection refused".into(),
            },
        );
        assert_eq!(s.sync, SyncState::Synced { revision: 4 });

        // 401 is session expiry, not revocation.
        apply_response(&mut s, &SyncOutcome::Unauthorized);
        assert_eq!(s.sync, SyncState::Synced { revision: 4 });
    }

    #[test]
    fn pull_and_uptodate_transitions() {
        let mut s = stored(
            "c1",
            SyncState::Local,
            Some(origin("s1", NamespaceKind::Personal)),
        );
        apply_response(&mut s, &SyncOutcome::Pulled { revision: 7 });
        assert_eq!(s.sync, SyncState::Synced { revision: 7 });

        apply_response(&mut s, &SyncOutcome::UpToDate);
        assert_eq!(s.sync, SyncState::Synced { revision: 7 });
    }

    #[test]
    fn ac06_save_revoked_as_personal_moves_out_of_org() {
        let mut doc = WorkspaceDoc::new();
        doc.organizations.push(stored(
            "c1",
            SyncState::Revoked,
            Some(origin("s1", NamespaceKind::Organization)),
        ));

        let name = save_revoked_as_personal(&mut doc, "c1").unwrap();
        assert_eq!(name, "collection c1");
        assert!(doc.organizations.is_empty());
        assert_eq!(doc.personal.len(), 1);
        let moved = &doc.personal[0];
        assert_eq!(moved.sync, SyncState::Local);
        assert!(moved.origin.is_none());
        // Content is retained.
        assert_eq!(moved.collection.count_requests(), 1);
        // It no longer syncs to the revoked source:
        assert!(plan_push(moved).is_none());
    }

    #[test]
    fn ac05_remove_local_data_is_explicit_only() {
        let mut doc = WorkspaceDoc::new();
        doc.organizations.push(stored(
            "c1",
            SyncState::Revoked,
            Some(origin("s1", NamespaceKind::Organization)),
        ));
        // No sync outcome removes it:
        let mut s = doc.organizations.clone();
        for outcome in [
            SyncOutcome::UpToDate,
            SyncOutcome::Pushed { new_revision: 1 },
            SyncOutcome::Pulled { revision: 1 },
            SyncOutcome::Conflict { server_revision: 1 },
            SyncOutcome::Revoked,
            SyncOutcome::NetworkError {
                message: "x".into(),
            },
            SyncOutcome::Unauthorized,
        ] {
            apply_response(&mut s[0], &outcome);
        }
        assert_eq!(doc.organizations.len(), 1);
        // Explicit removal works and reports the name.
        assert_eq!(remove_local_data(&mut doc, "c1").unwrap(), "collection c1");
        assert!(doc.organizations.is_empty());
        assert!(remove_local_data(&mut doc, "missing").is_none());
    }

    #[test]
    fn revoked_personal_share_can_also_be_moved() {
        // Revoked personal shares live in the personal list too.
        let mut doc = WorkspaceDoc::new();
        doc.personal.push(stored(
            "p1",
            SyncState::Revoked,
            Some(origin("s9", NamespaceKind::Personal)),
        ));
        assert!(save_revoked_as_personal(&mut doc, "p1").is_some());
        assert_eq!(doc.personal.len(), 1);
        assert!(doc.personal[0].origin.is_none());
    }

    #[test]
    fn plan_pull_categories() {
        let local = vec![
            // synced, same revision -> nothing
            stored(
                "a",
                SyncState::Synced { revision: 3 },
                Some(origin("sa", NamespaceKind::Personal)),
            ),
            // synced, server moved ahead -> pull
            stored(
                "b",
                SyncState::Synced { revision: 1 },
                Some(origin("sb", NamespaceKind::Personal)),
            ),
            // pending edits -> push first
            stored(
                "c",
                SyncState::PendingEdits { base_revision: 2 },
                Some(origin("sc", NamespaceKind::Personal)),
            ),
            // revoked -> verify only
            stored(
                "d",
                SyncState::Revoked,
                Some(origin("sd", NamespaceKind::Organization)),
            ),
            // local-only -> never exchanged
            stored("e", SyncState::Local, None),
            // pending edits whose server row is gone -> orphaned
            stored(
                "f",
                SyncState::PendingEdits { base_revision: 8 },
                Some(origin("sf", NamespaceKind::Organization)),
            ),
        ];
        let server = vec![
            ServerEntry {
                id: "sa".into(),
                revision: 3,
            },
            ServerEntry {
                id: "sb".into(),
                revision: 4,
            },
            ServerEntry {
                id: "sc".into(),
                revision: 2,
            },
            ServerEntry {
                id: "sd".into(),
                revision: 6,
            },
            ServerEntry {
                id: "new".into(),
                revision: 1,
            }, // not local at all
        ];
        let plan = plan_pull(&local, &server);
        assert_eq!(plan.to_pull, vec!["b".to_string(), "new".to_string()]);
        assert_eq!(plan.to_push, vec!["c".to_string()]);
        assert_eq!(plan.to_verify, vec!["d".to_string()]);
        assert_eq!(plan.orphaned_pending, vec!["f".to_string()]);
    }

    #[test]
    fn mark_edited_only_degrades_synced() {
        let mut s = stored("c", SyncState::Local, None);
        mark_edited(&mut s);
        assert_eq!(s.sync, SyncState::Local);

        let mut s = stored("c", SyncState::Revoked, None);
        mark_edited(&mut s);
        assert_eq!(s.sync, SyncState::Revoked);

        let mut s = stored("c", SyncState::PendingEdits { base_revision: 4 }, None);
        mark_edited(&mut s);
        assert_eq!(s.sync, SyncState::PendingEdits { base_revision: 4 });
    }

    #[test]
    fn mark_enabled_records_origin_and_sync() {
        let mut s = stored("c", SyncState::Local, None);
        mark_enabled(
            &mut s,
            "https://sync.test",
            "server-id",
            personal_namespace("u1", "alice"),
            2,
        );
        assert_eq!(s.sync, SyncState::Synced { revision: 2 });
        let origin = s.origin.unwrap();
        assert_eq!(origin.collection_id, "server-id");
        assert_eq!(origin.namespace.kind, NamespaceKind::Personal);
    }
}
