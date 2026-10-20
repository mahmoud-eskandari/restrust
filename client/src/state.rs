//! Runtime UI state (CONTRACT §5.2): open tabs, dialogs, banners, toasts,
//! login/session, admin window state, autosave bookkeeping. The persisted data
//! itself lives in [`WorkspaceDoc`]; edits are applied directly to the model
//! and tracked via dirty flags.

use crate::time::Instant;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use restrust_core::model::{CollectionItem, KeyValue, RequestDef, ResponseSummary};
use restrust_core::share::{AccessLevel, ShareGrant};
use restrust_core::workspace_doc::{StoredCollection, SyncState, WorkspaceDoc};

use crate::api::{AdminOrgDto, AdminUserDto, ProviderDto, SessionOrg, SessionUser};

/// Cached responses kept per tab (CONTRACT §5.2: last 10).
pub const RESPONSE_CACHE_LIMIT: usize = 10;

// --- banners / toasts ----------------------------------------------------------

/// Banner severity (ui-spec §5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Danger,
}

/// What a banner's action buttons do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BannerAction {
    /// Retry a failed request send.
    RetrySend,
    /// Retry the server connection / sync pass.
    RetrySync,
    Dismiss,
    SignIn,
    Refresh,
    /// Reveal a revoked collection in the tree (sync pill flow).
    ShowCollection(String),
}

/// A stacked banner in the strip below the top bar.
#[derive(Clone, Debug)]
pub struct Banner {
    pub severity: Severity,
    pub message: String,
    pub actions: Vec<BannerAction>,
}

/// A transient bottom-right toast (auto-fades after 2.5 s).
#[derive(Clone, Debug)]
pub struct Toast {
    pub message: String,
    pub success: bool,
    pub born: Instant,
}

// --- tabs ------------------------------------------------------------------------

/// Which request a tab shows.
#[derive(Clone, Debug)]
pub enum TabKind {
    /// An unsaved scratch request (`Untitled request`).
    Scratch(Box<RequestDef>),
    /// A request inside a stored collection.
    Saved {
        collection_id: String,
        request_id: String,
    },
}

/// Request editor section tabs (ui-spec §2.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionTab {
    Params,
    Body,
    Headers,
    Auth,
    Settings,
}

/// Response area tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseTab {
    Body,
    Headers,
}

/// Response body sub-views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyView {
    Pretty,
    Raw,
    Preview,
}

/// One cached response of a tab.
#[derive(Clone, Debug)]
pub struct CachedResponse {
    pub meta: ResponseSummary,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

/// An open request tab.
#[derive(Clone, Debug)]
pub struct Tab {
    pub id: u64,
    pub kind: TabKind,
    pub section: SectionTab,
    pub response_tab: ResponseTab,
    pub body_view: BodyView,
    pub wrap: bool,
    pub responses: VecDeque<CachedResponse>,
    pub response_collapsed: bool,
    /// Params bulk-edit textarea toggle.
    pub bulk_edit: bool,
}

impl Tab {
    pub fn new(id: u64, kind: TabKind) -> Self {
        Tab {
            id,
            kind,
            section: SectionTab::Params,
            response_tab: ResponseTab::Body,
            body_view: BodyView::Pretty,
            wrap: true,
            responses: VecDeque::new(),
            response_collapsed: false,
            bulk_edit: false,
        }
    }

    pub fn label(&self, doc: &WorkspaceDoc) -> String {
        match &self.kind {
            TabKind::Scratch(def) => {
                if def.name.trim().is_empty() {
                    "Untitled request".to_string()
                } else {
                    def.name.clone()
                }
            }
            TabKind::Saved {
                collection_id,
                request_id,
            } => doc
                .personal
                .iter()
                .chain(doc.organizations.iter())
                .find(|s| s.collection.id == *collection_id)
                .and_then(|s| s.collection.find_request(request_id))
                .map(|r| r.name.clone())
                .unwrap_or_else(|| "(deleted request)".to_string()),
        }
    }
}

// --- login / server ---------------------------------------------------------------

/// Sync status pill states (ui-spec §2.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ServerStatus {
    #[default]
    Offline,
    Connected,
    Syncing,
    Failed(String),
    /// A collection got a confirmed 403.
    Revoked(String),
}

/// Login/session runtime state.
#[derive(Clone, Debug, Default)]
pub struct LoginState {
    pub token: Option<String>,
    pub user: Option<SessionUser>,
    pub organizations: Vec<SessionOrg>,
    pub status: ServerStatus,
    pub last_synced: Option<String>,
}

impl LoginState {
    pub fn signed_in(&self) -> bool {
        self.token.is_some() && self.user.is_some()
    }

    pub fn is_admin(&self) -> bool {
        self.user
            .as_ref()
            .map(|u| u.system_role == "admin")
            .unwrap_or(false)
    }
}

/// Effective access classification used for gating (ui-spec §2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Local-only collection or server-reported owner.
    Owner,
    Full,
    Read,
}

/// Last-known server access per collection id (from `GET /collections`).
#[derive(Clone, Debug, Default)]
pub struct SyncInfo {
    pub access: String,
}

// --- dialogs -----------------------------------------------------------------------

/// Sign-in dialog state (ui-spec §3.1).
#[derive(Clone, Debug)]
pub struct SignInDialog {
    pub server_url: String,
    pub remember: bool,
    pub username: String,
    pub password: String,
    pub in_flight: bool,
    pub error: Option<String>,
    pub providers: Vec<ProviderDto>,
    pub providers_loaded: bool,
    /// Set when fetching providers failed (section is skipped).
    pub providers_failed: bool,
    /// Provider whose browser flow is in progress (desktop paste-token flow).
    pub provider_pending: Option<String>,
    pub paste_token: String,
    pub completing: bool,
}

impl SignInDialog {
    pub fn new(server_url: Option<String>) -> Self {
        SignInDialog {
            server_url: server_url.unwrap_or_default(),
            remember: true,
            username: String::new(),
            password: String::new(),
            in_flight: false,
            error: None,
            providers: Vec::new(),
            providers_loaded: false,
            providers_failed: false,
            provider_pending: None,
            paste_token: String::new(),
            completing: false,
        }
    }
}

/// Share dialog state (ui-spec §3.2).
#[derive(Clone, Debug)]
pub struct ShareDialog {
    pub local_collection_id: String,
    pub server_collection_id: String,
    pub name: String,
    pub grants: Vec<ShareGrant>,
    pub add_username: String,
    pub add_level: AccessLevel,
    pub org_wide: bool,
    pub org_choice: Option<String>,
    pub public_on: bool,
    pub slug: String,
    /// Known live slug (from a publish in this session).
    pub live_slug: Option<String>,
    pub saving: bool,
    pub publishing: bool,
    pub error: Option<String>,
}

/// Detected import format (ui-spec §3.3 chip).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportFormat {
    Unknown,
    Postman21,
    Postman30,
    OpenApi,
    Curl,
}

impl ImportFormat {
    pub fn label(self) -> &'static str {
        match self {
            ImportFormat::Unknown => "Unrecognized format",
            ImportFormat::Postman21 => "Postman v2.1",
            ImportFormat::Postman30 => "Postman v3.0",
            ImportFormat::OpenApi => "OpenAPI 3",
            ImportFormat::Curl => "cURL",
        }
    }
}

/// Import dialog state (ui-spec §3.3).
#[derive(Clone, Debug)]
pub struct ImportDialog {
    pub text: String,
    /// Preset destination (opened via `Import into…`): a local collection id.
    pub preset_destination: Option<String>,
    /// `None` = "New collection".
    pub destination: Option<String>,
    pub error: Option<String>,
    /// Report view shown after a successful import.
    pub done: Option<ImportDone>,
}

impl ImportDialog {
    pub fn new(preset_destination: Option<String>) -> Self {
        ImportDialog {
            text: String::new(),
            preset_destination: preset_destination.clone(),
            destination: preset_destination,
            error: None,
            done: None,
        }
    }
}

/// The post-import report view.
#[derive(Clone, Debug)]
pub struct ImportDone {
    pub collections: usize,
    pub environments: usize,
    /// For cURL imports: the destination collection name.
    pub request_destination: Option<String>,
    pub preserved: Vec<String>,
    pub lossy: Vec<String>,
    pub unsupported: Vec<String>,
    pub expanded: [bool; 3],
}

/// Export dialog state (ui-spec §3.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Postman21,
    OpenApi30,
    RestrustJson,
}

/// Export dialog state.
#[derive(Clone, Debug)]
pub struct ExportDialog {
    pub collection_id: String,
    pub name: String,
    pub format: ExportFormat,
    pub output: String,
    pub path: String,
    pub error: Option<String>,
}

/// "New environment…" dialog (name + rows).
#[derive(Clone, Debug, Default)]
pub struct NewEnvironmentDialog {
    pub name: String,
    pub values: Vec<KeyValue>,
}

/// All modal dialogs (ui-spec §3). At most one is open.
#[derive(Clone, Debug, Default)]
pub enum DialogState {
    #[default]
    None,
    SignIn(Box<SignInDialog>),
    Share(Box<ShareDialog>),
    Import(Box<ImportDialog>),
    Export(Box<ExportDialog>),
    Settings,
    Shortcuts,
    About,
    NewCollection {
        name: String,
    },
    NewEnvironment(Box<NewEnvironmentDialog>),
    ManageEnvironments,
    /// Delete collection (local-only = §3.5a, synced owner = §3.5b).
    ConfirmDelete {
        collection_id: String,
        name: String,
        requests: usize,
        on_server: bool,
        busy: bool,
    },
    /// Remove retained local data after revocation (§3.5c).
    ConfirmRemoveLocal {
        collection_id: String,
        name: String,
    },
    /// Sync conflict (§3.5d).
    Conflict {
        collection_id: String,
        name: String,
        server_revision: u64,
    },
    /// "Save into…" for a scratch tab (Settings section, §2.5).
    SaveScratchInto {
        tab_id: u64,
        collection_id: Option<String>,
        parent_folder: Option<String>,
    },
}

impl DialogState {
    pub fn is_none(&self) -> bool {
        matches!(self, DialogState::None)
    }

    /// True while any dialog operation is in flight (Esc must not close).
    pub fn in_flight(&self) -> bool {
        match self {
            DialogState::SignIn(d) => d.in_flight || d.completing,
            DialogState::Share(d) => d.saving || d.publishing,
            DialogState::ConfirmDelete { busy, .. } => *busy,
            _ => false,
        }
    }
}

// --- administration (§3.8) -----------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AdminTab {
    #[default]
    Users,
    Organizations,
}

/// New-user dialog fields.
#[derive(Clone, Debug)]
pub struct NewUserDialog {
    pub username: String,
    pub password: String,
    pub role: String,
}

/// Reset-password dialog fields.
#[derive(Clone, Debug)]
pub struct ResetPasswordDialog {
    pub user_id: String,
    pub username: String,
    pub password: String,
}

/// A member row of the organization draft list.
#[derive(Clone, Debug)]
pub struct MemberRow {
    pub username: String,
    pub role: String,
    /// Marked for removal (struck-through until saved).
    pub removed: bool,
}

/// Administration window state — shared between the main window (which drains
/// HTTP results into it) and the native viewport / wasm modal that renders it.
#[derive(Clone, Debug, Default)]
pub struct AdminUi {
    pub base: String,
    pub token: String,
    pub tab: AdminTab,
    pub users: Vec<AdminUserDto>,
    pub users_loaded: bool,
    pub loading_users: bool,
    pub orgs: Vec<AdminOrgDto>,
    pub orgs_loaded: bool,
    pub loading_orgs: bool,
    pub selected_org: Option<usize>,
    /// Edited member list for the selected org.
    pub member_draft: Vec<MemberRow>,
    pub add_member_username: String,
    pub add_member_role: String,
    pub filter: String,
    /// Row-level in-flight markers (user/org ids).
    pub row_busy: HashSet<String>,
    pub banners: Vec<Banner>,
    pub new_user: Option<NewUserDialog>,
    pub reset_password: Option<ResetPasswordDialog>,
    pub new_org_name: String,
    pub new_org_open: bool,
    /// HTTP ids this window is waiting on, mapped to their purpose.
    pub pending: HashMap<u64, AdminOp>,
    pub users_error: Option<String>,
    /// Set by the native viewport callback when the OS close button is used.
    pub close_requested: bool,
}

/// What an in-flight admin request was for.
#[derive(Clone, Debug)]
pub enum AdminOp {
    ListUsers,
    CreateUser,
    PatchUser { user_id: String },
    ResetPassword { user_id: String },
    ListOrgs,
    CreateOrg,
    PutMembers { org_id: String },
}

impl AdminUi {
    pub fn selected_org_data(&self) -> Option<&AdminOrgDto> {
        self.selected_org.and_then(|i| self.orgs.get(i))
    }
}

// --- app state -------------------------------------------------------------------------

/// Which tree node is selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedNode {
    pub collection_id: String,
    /// `None` = the collection root row itself.
    pub node_id: Option<String>,
}

/// Inline rename state.
#[derive(Clone, Debug)]
pub struct RenameState {
    pub collection_id: String,
    pub node_id: Option<String>,
    pub buffer: String,
}

/// Which namespace the sidebar is scoped to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamespaceSelection {
    Personal,
    Organization(String),
}

/// The whole runtime state alongside the persisted document.
pub struct AppState {
    pub doc: WorkspaceDoc,
    pub tabs: Vec<Tab>,
    pub active_tab: Option<usize>,
    pub next_tab_id: u64,
    pub selected: Option<SelectedNode>,
    pub renaming: Option<RenameState>,
    /// Tree row whose `⋯` menu is open: (collection id, node id).
    pub context_menu_node: Option<(String, Option<String>)>,
    pub search: String,
    pub search_focus: bool,
    pub sidebar_visible: bool,
    pub sidebar_overlay: bool,
    pub namespace: NamespaceSelection,
    pub expanded: BTreeSet<String>,
    pub banners: Vec<Banner>,
    pub toasts: Vec<Toast>,
    pub login: LoginState,
    pub dialog: DialogState,
    pub dirty: bool,
    pub last_edit: Option<Instant>,
    pub save_started: Option<Instant>,
    pub sending_tabs: HashSet<u64>,
    pub sync_in_flight: bool,
    pub sync_info: HashMap<String, SyncInfo>,
    /// Last-saved share grants per server collection id (v1 servers expose no
    /// GET for grants; this is the client's best knowledge).
    pub grants_known: HashMap<String, Vec<ShareGrant>>,
    /// Known publication slug per server collection id.
    pub slug_known: HashMap<String, String>,
    pub admin_open: bool,
}

impl AppState {
    pub fn new(doc: WorkspaceDoc) -> Self {
        AppState {
            doc,
            tabs: Vec::new(),
            active_tab: None,
            next_tab_id: 1,
            selected: None,
            renaming: None,
            context_menu_node: None,
            search: String::new(),
            search_focus: false,
            sidebar_visible: true,
            sidebar_overlay: false,
            namespace: NamespaceSelection::Personal,
            expanded: BTreeSet::new(),
            banners: Vec::new(),
            toasts: Vec::new(),
            login: LoginState::default(),
            dialog: DialogState::None,
            dirty: false,
            last_edit: None,
            save_started: None,
            sending_tabs: HashSet::new(),
            sync_in_flight: false,
            sync_info: HashMap::new(),
            grants_known: HashMap::new(),
            slug_known: HashMap::new(),
            admin_open: false,
        }
    }

    // --- dirty / banners / toasts ------------------------------------------------

    /// Records an edit: workspace dirty + collection PendingEdits.
    pub fn mark_edited(&mut self) {
        self.dirty = true;
        self.last_edit = Some(Instant::now());
    }

    /// Marks the stored collection as edited (sync PendingEdits) and the
    /// workspace dirty.
    pub fn mark_collection_edited(&mut self, collection_id: &str) {
        if let Some(stored) = find_stored_mut(&mut self.doc, collection_id) {
            crate::sync::mark_edited(stored);
        }
        self.mark_edited();
    }

    pub fn push_banner(&mut self, severity: Severity, message: String, actions: Vec<BannerAction>) {
        // Stacked max 3, oldest dropped (ui-spec §5.2).
        self.banners.push(Banner {
            severity,
            message,
            actions,
        });
        let overflow = self.banners.len().saturating_sub(3);
        if overflow > 0 {
            self.banners.drain(..overflow);
        }
    }

    pub fn push_toast(&mut self, message: String) {
        self.toasts.push(Toast {
            message,
            success: true,
            born: Instant::now(),
        });
        let overflow = self.toasts.len().saturating_sub(3);
        if overflow > 0 {
            self.toasts.drain(..overflow);
        }
    }

    // --- collection lookup --------------------------------------------------------

    pub fn find_stored(&self, collection_id: &str) -> Option<&StoredCollection> {
        find_stored(&self.doc, collection_id)
    }

    pub fn stored_mut(&mut self, collection_id: &str) -> Option<&mut StoredCollection> {
        find_stored_mut(&mut self.doc, collection_id)
    }

    /// Effective access classification for gating.
    pub fn access_of(&self, stored: &StoredCollection) -> Access {
        if stored.sync == SyncState::Local && stored.origin.is_none() {
            return Access::Owner;
        }
        match self
            .sync_info
            .get(
                stored
                    .origin
                    .as_ref()
                    .map(|o| o.collection_id.as_str())
                    .unwrap_or(stored.collection.id.as_str()),
            )
            .map(|i| i.access.as_str())
        {
            Some("read") => Access::Read,
            Some("full") => Access::Full,
            Some("owner") => Access::Owner,
            _ => match stored.access {
                Some(AccessLevel::ReadOnly) => Access::Read,
                Some(AccessLevel::FullAccess) => Access::Full,
                None => Access::Owner,
            },
        }
    }

    pub fn can_edit_tree(&self, stored: &StoredCollection) -> bool {
        stored.sync != SyncState::Revoked && self.access_of(stored) != Access::Read
    }

    pub fn is_owner(&self, stored: &StoredCollection) -> bool {
        self.access_of(stored) == Access::Owner
    }

    /// Name of a revoked collection, if any (for the sync pill).
    pub fn revoked_collection(&self) -> Option<String> {
        self.doc
            .personal
            .iter()
            .chain(self.doc.organizations.iter())
            .find(|s| s.sync == SyncState::Revoked)
            .map(|s| s.collection.name.clone())
    }

    /// Collections visible in the sidebar for the active namespace.
    pub fn visible_collections(&self) -> Vec<&StoredCollection> {
        match &self.namespace {
            NamespaceSelection::Personal => self.doc.personal.iter().collect(),
            NamespaceSelection::Organization(org_id) => self
                .doc
                .organizations
                .iter()
                .filter(|s| {
                    s.origin
                        .as_ref()
                        .map(|o| o.namespace.id == *org_id)
                        .unwrap_or(false)
                })
                .collect(),
        }
    }

    // --- tabs ----------------------------------------------------------------------

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active_tab.and_then(|i| self.tabs.get(i))
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.active_tab.and_then(|i| self.tabs.get_mut(i))
    }

    /// Opens a saved request in a tab (activating an existing tab if present).
    pub fn open_request(&mut self, collection_id: &str, request_id: &str) {
        if let Some(index) = self.tabs.iter().position(|t| {
            matches!(
                &t.kind,
                TabKind::Saved { collection_id: c, request_id: r }
                    if c == collection_id && r == request_id
            )
        }) {
            self.active_tab = Some(index);
            return;
        }
        self.tabs.push(Tab::new(
            self.next_tab_id,
            TabKind::Saved {
                collection_id: collection_id.to_string(),
                request_id: request_id.to_string(),
            },
        ));
        self.next_tab_id += 1;
        self.active_tab = Some(self.tabs.len() - 1);
    }

    /// Opens a scratch tab seeded with a request definition (history, cURL import).
    pub fn open_scratch(&mut self, def: RequestDef) {
        self.tabs
            .push(Tab::new(self.next_tab_id, TabKind::Scratch(Box::new(def))));
        self.next_tab_id += 1;
        self.active_tab = Some(self.tabs.len() - 1);
    }

    pub fn new_tab(&mut self) {
        self.open_scratch(RequestDef::new(
            restrust_core::model::HttpMethod::Get,
            "",
            "",
        ));
    }

    pub fn close_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            let tab_id = self.tabs[index].id;
            self.tabs.remove(index);
            self.sending_tabs.remove(&tab_id);
            self.active_tab = match self.active_tab {
                Some(active) if active == index => {
                    if self.tabs.is_empty() {
                        None
                    } else {
                        Some(active.min(self.tabs.len() - 1))
                    }
                }
                Some(active) if active > index => Some(active - 1),
                other => other,
            };
        }
    }

    /// Caches a response on its tab (keeping the newest 10).
    pub fn cache_response(&mut self, tab_id: u64, cached: CachedResponse) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.responses.push_back(cached);
            while tab.responses.len() > RESPONSE_CACHE_LIMIT {
                tab.responses.pop_front();
            }
        }
    }

    // --- active request access -------------------------------------------------------

    /// A snapshot of the active tab's request definition (edits are written
    /// back through [`AppState::apply_request_edit`]).
    pub fn active_request_def(&self) -> Option<RequestDef> {
        let tab = self.active_tab()?;
        match &tab.kind {
            TabKind::Scratch(def) => Some((**def).clone()),
            TabKind::Saved {
                collection_id,
                request_id,
            } => find_stored(&self.doc, collection_id)?
                .collection
                .find_request(request_id)
                .cloned(),
        }
    }

    /// Writes an edited request definition back into the model, marking the
    /// collection PendingEdits and the workspace dirty. Returns false when the
    /// request vanished (e.g. deleted elsewhere).
    pub fn apply_request_edit(&mut self, def: &RequestDef) -> bool {
        let Some(index) = self.active_tab else {
            return false;
        };
        let Some(tab) = self.tabs.get(index) else {
            return false;
        };
        match &tab.kind {
            TabKind::Scratch(_) => {
                if let Some(tab) = self.tabs.get_mut(index) {
                    if let TabKind::Scratch(scratch) = &mut tab.kind {
                        **scratch = def.clone();
                    }
                }
                self.mark_edited();
                true
            }
            TabKind::Saved {
                collection_id,
                request_id,
            } => {
                let collection_id = collection_id.clone();
                let request_id = request_id.clone();
                let found = find_stored_mut(&mut self.doc, &collection_id)
                    .and_then(|stored| stored.collection.find_request_mut(&request_id))
                    .map(|target| *target = def.clone())
                    .is_some();
                if found {
                    self.mark_collection_edited(&collection_id);
                }
                found
            }
        }
    }

    /// Resolved auth + interpolation scopes for the active tab's request.
    pub fn auth_and_scopes_for(
        &self,
        def: &RequestDef,
    ) -> (restrust_core::model::AuthKind, Vec<&[KeyValue]>) {
        let Some(tab) = self.active_tab() else {
            return (restrust_core::model::AuthKind::None, Vec::new());
        };
        match &tab.kind {
            TabKind::Scratch(_) => (
                def.auth
                    .as_ref()
                    .map(|a| a.kind.clone())
                    .unwrap_or(restrust_core::model::AuthKind::None),
                crate::send::resolved_scopes(&self.doc, None),
            ),
            TabKind::Saved { collection_id, .. } => {
                let stored = find_stored(&self.doc, collection_id);
                let auth = stored
                    .map(|s| s.collection.resolve_request_auth(&def.id))
                    .unwrap_or(restrust_core::model::AuthKind::None);
                let scopes = crate::send::resolved_scopes(&self.doc, stored.map(|s| &s.collection));
                (auth, scopes)
            }
        }
    }
}

pub fn find_stored<'a>(doc: &'a WorkspaceDoc, collection_id: &str) -> Option<&'a StoredCollection> {
    doc.personal
        .iter()
        .chain(doc.organizations.iter())
        .find(|s| s.collection.id == collection_id)
}

pub fn find_stored_mut<'a>(
    doc: &'a mut WorkspaceDoc,
    collection_id: &str,
) -> Option<&'a mut StoredCollection> {
    if let Some(pos) = doc
        .personal
        .iter()
        .position(|s| s.collection.id == collection_id)
    {
        return Some(&mut doc.personal[pos]);
    }
    doc.organizations
        .iter()
        .position(|s| s.collection.id == collection_id)
        .map(|pos| &mut doc.organizations[pos])
}

/// Deletes a request or folder node from its collection.
pub fn delete_tree_node(collection: &mut restrust_core::model::Collection, node_id: &str) -> bool {
    collection.remove_item(node_id).is_some()
}

/// Flattened (label, collection_id, parent) list of every collection + folder
/// for destination pickers.
pub fn destination_targets(doc: &WorkspaceDoc) -> Vec<(String, String, Option<String>)> {
    let mut out = Vec::new();
    for stored in doc.personal.iter().chain(doc.organizations.iter()) {
        out.push((
            stored.collection.name.clone(),
            stored.collection.id.clone(),
            None,
        ));
        collect_folder_targets(
            &stored.collection.items,
            &stored.collection.id,
            "",
            &mut out,
        );
    }
    out
}

fn collect_folder_targets(
    items: &[CollectionItem],
    collection_id: &str,
    prefix: &str,
    out: &mut Vec<(String, String, Option<String>)>,
) {
    for item in items {
        if let CollectionItem::Folder(folder) = item {
            let label = format!("{prefix}{}", folder.name);
            out.push((
                label.clone(),
                collection_id.to_string(),
                Some(folder.id.clone()),
            ));
            collect_folder_targets(&folder.items, collection_id, &format!("{label}/"), out);
        }
    }
}

// --- display helpers --------------------------------------------------------------------

/// `{host}{path}` for history rows, e.g. `api.example.com/users`.
pub fn host_and_path(url: &str) -> String {
    let no_scheme = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    no_scheme.split('?').next().unwrap_or(no_scheme).to_string()
}

/// Relative time for history rows (`3m ago`).
pub fn relative_time(rfc3339: &str) -> String {
    let Ok(time) = chrono::DateTime::parse_from_rfc3339(rfc3339) else {
        return String::new();
    };
    let elapsed = chrono::Utc::now().signed_duration_since(time);
    let minutes = elapsed.num_minutes();
    match minutes {
        ..=0 => "just now".to_string(),
        1..=59 => format!("{minutes}m ago"),
        60..=1439 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1440),
    }
}

/// Current local time as `HH:MM` (for "last synced"/"changes saved").
pub fn hhmm_now() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use restrust_core::model::{Collection, HttpMethod};
    use restrust_core::workspace_doc::ServerOrigin;

    fn stored(id: &str) -> StoredCollection {
        StoredCollection {
            collection: Collection::new(format!("col {id}")),
            sync: SyncState::Local,
            origin: None,
            access: None,
        }
    }

    fn response(status: u16) -> CachedResponse {
        CachedResponse {
            meta: ResponseSummary {
                status,
                reason: "OK".into(),
                size: 1,
                duration_ms: 1,
                time: "2026-10-20T10:00:00Z".into(),
            },
            body: Vec::new(),
            headers: Vec::new(),
        }
    }

    #[test]
    fn response_cache_keeps_last_ten() {
        let mut state = AppState::new(WorkspaceDoc::new());
        state.open_scratch(RequestDef::new(HttpMethod::Get, "x", "https://t"));
        let tab_id = state.tabs[0].id;
        for i in 0..15 {
            state.cache_response(tab_id, response(200 + (i % 10) as u16));
        }
        assert_eq!(state.tabs[0].responses.len(), RESPONSE_CACHE_LIMIT);
        // Entries 5..=14 are kept; the newest is i=14 -> status 204.
        assert_eq!(state.tabs[0].responses.back().unwrap().meta.status, 204);
        assert_eq!(state.tabs[0].responses.front().unwrap().meta.status, 205);
    }

    #[test]
    fn open_request_deduplicates_tabs() {
        let mut state = AppState::new(WorkspaceDoc::new());
        state.doc.personal.push(stored("c1"));
        state.doc.personal[0]
            .collection
            .items
            .push(CollectionItem::Request(RequestDef::new(
                HttpMethod::Get,
                "list",
                "https://t",
            )));
        let collection_id = state.doc.personal[0].collection.id.clone();
        let request_id = state.doc.personal[0].collection.items[0].id().to_string();
        state.open_request(&collection_id, &request_id);
        state.open_request(&collection_id, &request_id);
        state.open_request(&collection_id, &request_id);
        assert_eq!(state.tabs.len(), 1);
        state.open_scratch(RequestDef::new(HttpMethod::Get, "y", "https://t"));
        state.open_request(&collection_id, &request_id);
        assert_eq!(state.tabs.len(), 2);
        // The existing request tab was re-activated.
        assert_eq!(state.active_tab, Some(0));
    }

    #[test]
    fn close_tab_adjusts_active_index() {
        let mut state = AppState::new(WorkspaceDoc::new());
        for name in ["a", "b", "c"] {
            state.open_scratch(RequestDef::new(HttpMethod::Get, name, "https://t"));
        }
        // Active is Some(2) ("c"); closing index 0 shifts it to Some(1).
        state.close_tab(0);
        assert_eq!(state.active_tab, Some(1));
        state.active_tab = Some(1);
        state.close_tab(0);
        assert_eq!(state.active_tab, Some(0));
        state.close_tab(0);
        assert_eq!(state.active_tab, None);
        assert!(state.tabs.is_empty());
    }

    #[test]
    fn mark_collection_edited_sets_pending() {
        let mut state = AppState::new(WorkspaceDoc::new());
        state.doc.personal.push(stored("c1"));
        state.doc.personal[0].sync = SyncState::Synced { revision: 4 };
        let collection_id = state.doc.personal[0].collection.id.clone();
        state.mark_collection_edited(&collection_id);
        assert_eq!(
            state.doc.personal[0].sync,
            SyncState::PendingEdits { base_revision: 4 }
        );
        assert!(state.dirty);
    }

    #[test]
    fn access_classification() {
        let mut state = AppState::new(WorkspaceDoc::new());
        let mut local = stored("l1");
        local.sync = SyncState::Local;
        assert_eq!(state.access_of(&local), Access::Owner);

        let mut synced = stored("s1");
        synced.sync = SyncState::Synced { revision: 1 };
        synced.origin = Some(ServerOrigin {
            server_url: "https://s".into(),
            collection_id: "srv1".into(),
            namespace: restrust_core::workspace_doc::NamespaceRef {
                kind: restrust_core::workspace_doc::NamespaceKind::Personal,
                id: "u1".into(),
                name: "alice".into(),
            },
        });
        // No sync_info yet: fall back to stored.access (None -> owner).
        assert_eq!(state.access_of(&synced), Access::Owner);

        state.sync_info.insert(
            "srv1".into(),
            SyncInfo {
                access: "read".into(),
            },
        );
        assert_eq!(state.access_of(&synced), Access::Read);
        assert!(!state.can_edit_tree(&synced));
        state.sync_info.insert(
            "srv1".into(),
            SyncInfo {
                access: "full".into(),
            },
        );
        assert_eq!(state.access_of(&synced), Access::Full);
        assert!(state.can_edit_tree(&synced));
    }

    #[test]
    fn revoked_collection_found_for_pill() {
        let mut state = AppState::new(WorkspaceDoc::new());
        assert!(state.revoked_collection().is_none());
        let mut revoked = stored("r1");
        revoked.sync = SyncState::Revoked;
        state.doc.organizations.push(revoked);
        assert_eq!(state.revoked_collection().unwrap(), "col r1");
    }

    #[test]
    fn host_and_path_strips_scheme_and_query() {
        assert_eq!(
            host_and_path("https://api.example.com/users?page=1"),
            "api.example.com/users"
        );
        assert_eq!(host_and_path("api.example.com"), "api.example.com");
        assert_eq!(host_and_path("{{base_url}}/x"), "{{base_url}}/x");
    }

    #[test]
    fn relative_time_buckets() {
        let now = chrono::Utc::now();
        let fmt = |d: chrono::Duration| now.checked_sub_signed(d).unwrap().to_rfc3339();
        assert_eq!(
            relative_time(&fmt(chrono::Duration::seconds(5))),
            "just now"
        );
        assert_eq!(relative_time(&fmt(chrono::Duration::minutes(3))), "3m ago");
        assert_eq!(relative_time(&fmt(chrono::Duration::hours(5))), "5h ago");
        assert_eq!(relative_time(&fmt(chrono::Duration::days(2))), "2d ago");
        assert_eq!(relative_time("not a date"), "");
    }

    #[test]
    fn banners_and_toasts_cap_at_three() {
        let mut state = AppState::new(WorkspaceDoc::new());
        for i in 0..5 {
            state.push_banner(Severity::Danger, format!("b{i}"), vec![]);
            state.push_toast(format!("t{i}"));
        }
        assert_eq!(state.banners.len(), 3);
        assert_eq!(state.banners[0].message, "b2");
        assert_eq!(state.toasts.len(), 3);
    }
}
