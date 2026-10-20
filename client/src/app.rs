//! The eframe application: owns [`AppState`] + [`Services`], drains the HTTP
//! inbox, applies sync outcomes, drives autosave, and hands `&mut self` to the
//! UI modules (ui-spec + CONTRACT §5.2).

use crate::time::Instant;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui;
use restrust_core::model::{Collection, CollectionItem, RequestDef};
use restrust_core::share::ShareGrant;
use restrust_core::workspace_doc::{
    NamespaceKind, NamespaceRef, ServerOrigin, StoredCollection, SyncState, WorkspaceDoc,
};

use crate::api::{self, ApiError};
use crate::platform::{HttpService, Services, IS_WASM};
use crate::send;
use crate::state::{
    self, AdminOp, AdminUi, BannerAction, CachedResponse, DialogState, ImportFormat,
    NamespaceSelection, ServerStatus, Severity,
};
use crate::sync::{self, SyncOutcome};
use crate::theme;
use crate::ui;

/// What an in-flight HTTP request was for.
#[derive(Debug)]
pub enum InflightOp {
    Send {
        tab_id: u64,
        def: Box<RequestDef>,
        started: Instant,
    },
    Providers,
    Login {
        server: String,
        remember: bool,
    },
    Session {
        server: String,
        token: String,
        remember: bool,
    },
    CollectionsList {
        server: String,
    },
    CollectionGet {
        server: String,
        /// Server collection id (the GET target).
        server_id: String,
        /// Local collection id; empty when the server row is new locally.
        local_id: String,
    },
    CollectionPut {
        server: String,
        local_id: String,
        force: bool,
        /// Enable-sync final push: the fresh server id to claim.
        enable: Option<String>,
    },
    CollectionCreate {
        server: String,
        local_id: String,
    },
    CollectionDelete {
        server: String,
        local_id: String,
    },
    SharesPut {
        local_id: String,
        server_id: String,
        grants: Vec<ShareGrant>,
    },
    Publish {
        local_id: String,
        server_id: String,
        slug: Option<String>,
    },
    Unpublish {
        local_id: String,
        server_id: String,
        slug: String,
    },
    Admin(AdminOp),
}

pub struct RestrustApp {
    pub state: state::AppState,
    pub services: Arc<Services>,
    /// Admin window state shared with the native viewport callback.
    pub admin: Arc<Mutex<AdminUi>>,
    pub inflight: HashMap<u64, InflightOp>,
    /// Outstanding requests in the current sync pass.
    pub sync_pending: usize,
    pub sync_errors: Vec<String>,
    pub sync_worked: bool,
    pub applied_theme: Option<String>,
    pub last_saved_hhmm: Option<String>,
    /// Most recent `GET /collections` response (namespace metadata).
    last_list_entries: Vec<api::CollectionListEntry>,
}

impl RestrustApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let services = Arc::new(Services::new(&cc.egui_ctx));

        // Load the workspace; a corrupt store is backed up and we start fresh.
        let (mut doc, corrupt) = match services.storage.read_raw() {
            Some(raw) => match WorkspaceDoc::from_json(&raw) {
                Ok(doc) => {
                    log::info!("boot: loaded {} personal collection(s)", doc.personal.len());
                    (doc, None)
                }
                Err(err) => {
                    services.storage.backup_corrupt(raw);
                    (WorkspaceDoc::new(), Some(err.to_string()))
                }
            },
            None => (WorkspaceDoc::new(), None),
        };
        // Fresh workspaces default to dark (ui-spec §4.4 [default]).
        if doc.settings.theme != "dark" && doc.settings.theme != "light" {
            doc.settings.theme = "dark".to_string();
        }

        let mut app_state = state::AppState::new(doc);
        if let Some(reason) = corrupt {
            app_state.push_banner(
                Severity::Warning,
                format!(
                    "Could not load the saved workspace: {reason}. A backup copy was kept next to the workspace file — nothing was lost. Starting fresh."
                ),
                vec![BannerAction::Dismiss],
            );
        }

        // `mut` is only needed on wasm, where the OIDC pickup below mutates it.
        #[cfg_attr(not(target_arch = "wasm32"), expect(unused_mut))]
        let mut app = RestrustApp {
            state: app_state,
            services,
            admin: Arc::new(Mutex::new(AdminUi::default())),
            inflight: HashMap::new(),
            sync_pending: 0,
            sync_errors: Vec::new(),
            sync_worked: false,
            applied_theme: None,
            last_saved_hhmm: None,
            last_list_entries: Vec::new(),
        };

        // wasm: pick up a `#token=` left by the same-origin OIDC redirect.
        #[cfg(target_arch = "wasm32")]
        if let Some(token) = crate::platform::take_location_hash_token() {
            let base = wasm_origin();
            if !base.is_empty() {
                let id = app.services.http.fetch_tracked(api::session(&base, &token));
                app.inflight.insert(
                    id,
                    InflightOp::Session {
                        server: base,
                        token,
                        remember: true,
                    },
                );
            }
        }
        app
    }

    // === plumbing =================================================================

    fn api_fetch(&mut self, op: InflightOp, req: ehttp::Request) {
        let id = self.services.http.fetch_tracked(req);
        self.inflight.insert(id, op);
    }

    /// Issues an admin request; its result is routed to the admin window.
    pub fn admin_fetch(&self, op: AdminOp, req: ehttp::Request) {
        let id = self.services.http.fetch_tracked(req);
        self.admin.lock().unwrap().pending.insert(id, op);
    }

    fn server_base(&self) -> Option<String> {
        let token = self.state.login.token.as_ref()?;
        let base = self.server_url();
        if token.is_empty() || base.is_empty() {
            None
        } else {
            Some(base)
        }
    }

    pub fn server_url(&self) -> String {
        self.state
            .doc
            .settings
            .server_url
            .clone()
            .unwrap_or_default()
    }

    pub fn host_of(url: &str) -> String {
        url.trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string()
    }

    pub fn save_now(&mut self) {
        self.state.save_started = Some(Instant::now());
        self.services.storage.save(&self.state.doc);
        self.state.dirty = false;
        self.last_saved_hhmm = Some(state::hhmm_now());
    }

    fn autosave_tick(&mut self) {
        let due = self.state.dirty
            && self
                .state
                .last_edit
                .map(|t| t.elapsed().as_millis() >= 1000)
                .unwrap_or(false);
        if due {
            self.save_now();
        }
    }

    fn tick_toasts(&mut self) {
        self.state
            .toasts
            .retain(|toast| toast.born.elapsed().as_millis() < 2500);
    }

    fn apply_theme_if_needed(&mut self, ctx: &egui::Context) {
        let name = self.state.doc.settings.theme.clone();
        if self.applied_theme.as_deref() != Some(name.as_str()) {
            theme::apply_theme(ctx, &name);
            self.applied_theme = Some(name);
        }
    }

    // === send =====================================================================

    pub fn send_active_request(&mut self) {
        let Some(tab_id) = self.state.active_tab().map(|t| t.id) else {
            return;
        };
        if self.state.sending_tabs.contains(&tab_id) {
            return;
        }
        let Some(def) = self.state.active_request_def() else {
            return;
        };
        let (auth, scopes) = self.state.auth_and_scopes_for(&def);
        match send::build_request(&def, &auth, &scopes) {
            Ok(req) => {
                self.state.sending_tabs.insert(tab_id);
                self.api_fetch(
                    InflightOp::Send {
                        tab_id,
                        def: Box::new(def),
                        started: Instant::now(),
                    },
                    req,
                );
            }
            Err(err) => {
                self.send_failed_banner(&err);
                self.state.doc.add_history(def, None);
                self.state.mark_edited();
            }
        }
    }

    fn send_failed_banner(&mut self, err: &str) {
        let message = if IS_WASM {
            format!("Request failed: {err}. Check the URL, network, and CORS — the browser blocks cross-origin requests without server opt-in.")
        } else {
            format!("Request failed: {err}. Check the URL and network, then send again.")
        };
        self.state
            .push_banner(Severity::Danger, message, vec![BannerAction::RetrySend]);
    }

    // === session / login ==============================================================

    pub fn login_password(
        &mut self,
        server: String,
        username: String,
        password: String,
        remember: bool,
    ) {
        self.api_fetch(
            InflightOp::Login {
                server: server.clone(),
                remember,
            },
            api::login(&server, &username, &password),
        );
    }

    pub fn complete_with_token(&mut self, server: String, token: String, remember: bool) {
        let id = self
            .services
            .http
            .fetch_tracked(api::session(&server, &token));
        self.inflight.insert(
            id,
            InflightOp::Session {
                server,
                token,
                remember,
            },
        );
    }

    pub fn fetch_providers(&mut self, server: String) {
        self.api_fetch(InflightOp::Providers, api::providers(&server));
    }

    pub fn logout(&mut self) {
        let token = self.state.login.token.take().unwrap_or_default();
        self.state.login.user = None;
        self.state.login.organizations.clear();
        self.state.login.status = ServerStatus::Offline;
        self.state.namespace = NamespaceSelection::Personal;
        self.state.admin_open = false;
        self.state.sync_in_flight = false;
        self.sync_pending = 0;
        // Best-effort server logout; the token is discarded regardless.
        let base = self.server_url();
        if !token.is_empty() && !base.is_empty() {
            self.services
                .http
                .fetch(api::logout(&base, &token), Box::new(|_| {}));
        }
    }

    pub fn session_established(&mut self, server: String, token: String, remember: bool) {
        if remember || self.state.doc.settings.server_url.is_none() {
            self.state.doc.settings.server_url = Some(server.clone());
        }
        self.state.login.token = Some(token);
        self.state.login.status = ServerStatus::Connected;
        self.state.dialog = DialogState::None;
        self.state.mark_edited();
        self.sync_now();
    }

    // === sync ========================================================================

    pub fn sync_now(&mut self) {
        let (Some(token), base) = (
            self.state.login.token.clone(),
            self.server_base().unwrap_or_default(),
        ) else {
            return;
        };
        if self.state.sync_in_flight || base.is_empty() {
            return;
        }
        self.state.sync_in_flight = true;
        self.state.login.status = ServerStatus::Syncing;
        self.sync_pending = 0;
        self.sync_errors.clear();
        self.sync_worked = false;
        let id = self
            .services
            .http
            .fetch_tracked(api::collections_list(&base, &token));
        self.inflight
            .insert(id, InflightOp::CollectionsList { server: base });
    }

    /// Retry-connection action from the failed pill state.
    pub fn retry_connection(&mut self) {
        if self.state.login.token.is_some() {
            self.sync_now();
        }
    }

    fn sync_step_done(&mut self) {
        self.sync_pending = self.sync_pending.saturating_sub(1);
        if self.sync_pending == 0 && self.state.sync_in_flight {
            self.state.sync_in_flight = false;
            if let Some(name) = self.state.revoked_collection() {
                self.state.login.status = ServerStatus::Revoked(name);
            } else if let Some(err) = self.sync_errors.first() {
                self.state.login.status = ServerStatus::Failed(err.clone());
            } else {
                self.state.login.status = ServerStatus::Connected;
                self.state.login.last_synced = Some(state::hhmm_now());
                if self.sync_worked {
                    self.state.push_toast("Synced all collections".to_string());
                }
            }
            self.state.mark_edited();
        }
    }

    fn sync_transport_error(&mut self, host: &str, err: &str) {
        let message = format!(
            "Could not reach {host}: {err}. Your edits are saved locally and will sync when the server is reachable."
        );
        self.state
            .push_banner(Severity::Warning, message, vec![BannerAction::RetrySync]);
        self.sync_errors.push(err.to_string());
    }

    pub fn handle_unauthorized(&mut self) {
        self.state.login.token = None;
        self.state.login.user = None;
        self.state.login.organizations.clear();
        self.state.login.status = ServerStatus::Offline;
        self.state.sync_in_flight = false;
        self.state.push_banner(
            Severity::Warning,
            "Your session expired. Sign in again to continue syncing — local work is unaffected."
                .to_string(),
            vec![BannerAction::SignIn, BannerAction::Dismiss],
        );
    }

    fn collection_get_request(
        &mut self,
        base: &str,
        token: &str,
        local_id: String,
        server_id: &str,
    ) {
        let id = self
            .services
            .http
            .fetch_tracked(api::collection_get(base, token, server_id));
        self.inflight.insert(
            id,
            InflightOp::CollectionGet {
                server: base.to_string(),
                server_id: server_id.to_string(),
                local_id,
            },
        );
        self.sync_pending += 1;
    }

    #[allow(clippy::too_many_arguments)]
    fn collection_put_request(
        &mut self,
        base: &str,
        token: &str,
        local_id: String,
        server_id: &str,
        base_revision: u64,
        collection: &Collection,
        force: bool,
        enable: Option<String>,
    ) {
        let id = self.services.http.fetch_tracked(api::collection_put(
            base,
            token,
            server_id,
            base_revision,
            collection,
        ));
        self.inflight.insert(
            id,
            InflightOp::CollectionPut {
                server: base.to_string(),
                local_id,
                force,
                enable,
            },
        );
        self.sync_pending += 1;
    }

    pub fn enable_sync(&mut self, collection_id: &str) {
        let (Some(token), base, Some(stored)) = (
            self.state.login.token.clone(),
            self.server_base().unwrap_or_default(),
            self.state.find_stored(collection_id).cloned(),
        ) else {
            return;
        };
        let id = self.services.http.fetch_tracked(api::collection_create(
            &base,
            &token,
            &stored.collection.name,
        ));
        self.inflight.insert(
            id,
            InflightOp::CollectionCreate {
                server: base,
                local_id: collection_id.to_string(),
            },
        );
    }

    /// 409 dialog choice: force PUT with the server revision.
    pub fn overwrite_server_copy(&mut self, collection_id: &str, server_revision: u64) {
        let (Some(token), base, Some(stored)) = (
            self.state.login.token.clone(),
            self.server_base().unwrap_or_default(),
            self.state.find_stored(collection_id).cloned(),
        ) else {
            return;
        };
        let Some(server_id) = stored.origin.as_ref().map(|o| o.collection_id.clone()) else {
            return;
        };
        self.state.dialog = DialogState::None;
        self.state.sync_in_flight = true;
        self.state.login.status = ServerStatus::Syncing;
        self.sync_pending = 1;
        self.sync_errors.clear();
        self.collection_put_request(
            &base,
            &token,
            collection_id.to_string(),
            &server_id,
            server_revision,
            &stored.collection,
            true,
            None,
        );
    }

    /// 409 dialog choice: keep local — nothing is sent.
    pub fn keep_local_copy(&mut self) {
        self.state.dialog = DialogState::None;
    }

    // === share / publish ================================================================

    pub fn shares_save(&mut self, local_id: &str, server_id: &str, grants: Vec<ShareGrant>) {
        let (Some(token), base) = (
            self.state.login.token.clone(),
            self.server_base().unwrap_or_default(),
        ) else {
            return;
        };
        let id = self
            .services
            .http
            .fetch_tracked(api::shares_put(&base, &token, server_id, &grants));
        self.inflight.insert(
            id,
            InflightOp::SharesPut {
                local_id: local_id.to_string(),
                server_id: server_id.to_string(),
                grants,
            },
        );
    }

    pub fn publish(&mut self, local_id: &str, server_id: &str, slug: Option<String>) {
        let (Some(token), base) = (
            self.state.login.token.clone(),
            self.server_base().unwrap_or_default(),
        ) else {
            return;
        };
        let id = self.services.http.fetch_tracked(api::publish(
            &base,
            &token,
            server_id,
            slug.as_deref(),
        ));
        self.inflight.insert(
            id,
            InflightOp::Publish {
                local_id: local_id.to_string(),
                server_id: server_id.to_string(),
                slug,
            },
        );
    }

    pub fn unpublish(&mut self, local_id: &str, server_id: &str, slug: String) {
        let (Some(token), base) = (
            self.state.login.token.clone(),
            self.server_base().unwrap_or_default(),
        ) else {
            return;
        };
        let id = self
            .services
            .http
            .fetch_tracked(api::unpublish(&base, &token, server_id));
        self.inflight.insert(
            id,
            InflightOp::Unpublish {
                local_id: local_id.to_string(),
                server_id: server_id.to_string(),
                slug,
            },
        );
    }

    pub fn delete_collection(&mut self, collection_id: &str) {
        let server_id = self
            .state
            .find_stored(collection_id)
            .and_then(|s| s.origin.as_ref().map(|o| o.collection_id.clone()));
        match server_id {
            None => self.remove_collection_local(collection_id),
            Some(server_id) => {
                let (Some(token), base) = (
                    self.state.login.token.clone(),
                    self.server_base().unwrap_or_default(),
                ) else {
                    // Signed out: only local removal is possible.
                    self.remove_collection_local(collection_id);
                    return;
                };
                let id = self
                    .services
                    .http
                    .fetch_tracked(api::collection_delete(&base, &token, &server_id));
                self.inflight.insert(
                    id,
                    InflightOp::CollectionDelete {
                        server: base,
                        local_id: collection_id.to_string(),
                    },
                );
            }
        }
    }

    pub fn remove_collection_local(&mut self, collection_id: &str) {
        sync::remove_local_data(&mut self.state.doc, collection_id);
        self.state.tabs.retain(|tab| {
            !matches!(&tab.kind, state::TabKind::Saved { collection_id: c, .. } if c == collection_id)
        });
        if self.state.tabs.is_empty() {
            self.state.active_tab = None;
        }
        if self
            .state
            .selected
            .as_ref()
            .map(|s| s.collection_id == collection_id)
            .unwrap_or(false)
        {
            self.state.selected = None;
        }
        self.state.mark_edited();
    }

    /// Revocation resolution: move the retained copy into Personal (AC-05/06).
    pub fn save_revoked_as_personal(&mut self, collection_id: &str) {
        if let Some(name) = sync::save_revoked_as_personal(&mut self.state.doc, collection_id) {
            self.state
                .push_toast(format!("Saved \"{name}\" to Personal"));
            self.normalize_revoked_pill();
            self.state.namespace = NamespaceSelection::Personal;
            self.state.mark_edited();
        }
    }

    pub fn normalize_revoked_pill(&mut self) {
        match self.state.revoked_collection() {
            Some(name) => self.state.login.status = ServerStatus::Revoked(name),
            None => {
                if self.state.login.signed_in() {
                    self.state.login.status = ServerStatus::Connected;
                } else {
                    self.state.login.status = ServerStatus::Offline;
                }
            }
        }
    }

    // === import / export ==================================================================

    /// Local format detection for the import chip (ui-spec §3.3).
    pub fn detect_import_format(text: &str) -> ImportFormat {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return ImportFormat::Unknown;
        }
        if trimmed.starts_with("curl ") || trimmed.starts_with("curl\t") {
            return ImportFormat::Curl;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(schema) = value.pointer("/info/schema").and_then(|v| v.as_str()) {
                if schema.contains("v2.1") {
                    return ImportFormat::Postman21;
                }
                if schema.contains("v3.0") {
                    return ImportFormat::Postman30;
                }
            }
            if value.get("openapi").and_then(|v| v.as_str()).is_some() {
                return ImportFormat::OpenApi;
            }
        }
        // YAML OpenAPI: look for a top-level `openapi:` key before other keys.
        for line in trimmed.lines() {
            let line = line.trim();
            if line.starts_with("openapi:") {
                return ImportFormat::OpenApi;
            }
            if !line.starts_with('#') && !line.starts_with("---") && !line.is_empty() {
                break;
            }
        }
        ImportFormat::Unknown
    }

    pub fn run_import(
        &mut self,
        text: &str,
        destination: Option<String>,
    ) -> Result<state::ImportDone, String> {
        use restrust_interchange as ic;
        match Self::detect_import_format(text) {
            ImportFormat::Curl => {
                let mut def = ic::curl::import_curl(text)
                    .map_err(|e| format!("Import failed: {e}. Check the format and try again."))?;
                if def.name.trim().is_empty() {
                    def.name = "Imported request".to_string();
                }
                let dest_name = match &destination {
                    Some(collection_id) => {
                        let Some(stored) = self.state.stored_mut(collection_id) else {
                            return Err(
                                "Import failed: destination collection not found.".to_string()
                            );
                        };
                        let name = stored.collection.name.clone();
                        stored.collection.items.push(CollectionItem::Request(def));
                        sync::mark_edited(stored);
                        name
                    }
                    None => {
                        let name = def.name.clone();
                        let mut stored = StoredCollection {
                            collection: Collection::new(name.clone()),
                            sync: SyncState::Local,
                            origin: None,
                            access: None,
                        };
                        stored.collection.items.push(CollectionItem::Request(def));
                        self.state.doc.personal.push(stored);
                        name
                    }
                };
                self.state.mark_edited();
                Ok(state::ImportDone {
                    collections: 0,
                    environments: 0,
                    request_destination: Some(dest_name),
                    preserved: Vec::new(),
                    lossy: Vec::new(),
                    unsupported: Vec::new(),
                    expanded: [true, false, false],
                })
            }
            ImportFormat::Postman21 | ImportFormat::Postman30 => {
                let imported = ic::postman::detect_and_import(text)
                    .map_err(|e| format!("Import failed: {e}. Check the format and try again."))?;
                self.apply_imported(imported)
            }
            ImportFormat::OpenApi => {
                let imported = ic::openapi::detect_and_import(text)
                    .map_err(|e| format!("Import failed: {e}. Check the format and try again."))?;
                self.apply_imported(imported)
            }
            ImportFormat::Unknown => Err(
                "Import failed: Unrecognized format. Paste Postman v2.1 / v3.0 JSON, OpenAPI 3.0 / 3.1 YAML or JSON, or a cURL command."
                    .to_string(),
            ),
        }
    }

    fn apply_imported(
        &mut self,
        imported: restrust_interchange::Imported,
    ) -> Result<state::ImportDone, String> {
        let collections = imported.collections.len();
        let environments = imported.environments.len();
        for collection in imported.collections {
            self.state.doc.personal.push(StoredCollection {
                collection,
                sync: SyncState::Local,
                origin: None,
                access: None,
            });
        }
        for environment in imported.environments {
            self.state.doc.environments.push(environment);
        }
        self.state
            .push_toast(format!("Imported {collections} collection(s)"));
        self.state.mark_edited();
        Ok(state::ImportDone {
            collections,
            environments,
            request_destination: None,
            preserved: imported.report.preserved,
            lossy: imported.report.lossy,
            unsupported: imported.report.unsupported,
            expanded: [true, false, false],
        })
    }

    pub fn generate_export(
        &self,
        collection: &Collection,
        format: state::ExportFormat,
    ) -> Result<String, String> {
        use restrust_interchange as ic;
        match format {
            state::ExportFormat::Postman21 => {
                ic::postman::export_v2_1(collection).map_err(|e| format!("Export failed: {e}."))
            }
            state::ExportFormat::OpenApi30 => {
                ic::openapi::export_3_0(collection).map_err(|e| format!("Export failed: {e}."))
            }
            state::ExportFormat::RestrustJson => {
                serde_json::to_string_pretty(collection).map_err(|e| format!("Export failed: {e}."))
            }
        }
    }

    // === results ============================================================================

    fn drain_http(&mut self) {
        for (id, result) in self.services.http.drain() {
            if let Some(op) = self.inflight.remove(&id) {
                self.handle_result(op, result);
            } else {
                let pending_op = {
                    let mut admin = self.admin.lock().unwrap();
                    admin.pending.remove(&id)
                };
                if let Some(op) = pending_op {
                    ui::admin::handle_admin_result(self, op, result);
                }
            }
        }
    }

    fn handle_result(&mut self, op: InflightOp, result: Result<ehttp::Response, String>) {
        match op {
            InflightOp::Send {
                tab_id,
                def,
                started,
            } => {
                self.state.sending_tabs.remove(&tab_id);
                let def = *def;
                match &result {
                    Ok(resp) => {
                        let summary =
                            send::summary_from_response(resp, started.elapsed().as_millis() as u64);
                        let cached = CachedResponse {
                            meta: summary.clone(),
                            body: resp.bytes.clone(),
                            headers: resp.headers.headers.clone(),
                        };
                        self.state.cache_response(tab_id, cached);
                        self.state.doc.add_history(def, Some(summary));
                        self.state.mark_edited();
                    }
                    Err(err) => {
                        self.send_failed_banner(err);
                        self.state.doc.add_history(def, None);
                        self.state.mark_edited();
                    }
                }
            }
            InflightOp::Providers => {
                if let DialogState::SignIn(dialog) = &mut self.state.dialog {
                    dialog.providers_loaded = true;
                    match api::parse_ok(&result, |r| api::parse_providers(r.text().unwrap_or(""))) {
                        Ok(providers) => dialog.providers = providers,
                        Err(_) => dialog.providers_failed = true,
                    }
                }
            }
            InflightOp::Login { server, remember } => {
                match api::parse_ok(&result, |r| api::parse_login(r.text().unwrap_or(""))) {
                    Ok(login) => {
                        if let DialogState::SignIn(dialog) = &mut self.state.dialog {
                            dialog.in_flight = false;
                        }
                        let id = self
                            .services
                            .http
                            .fetch_tracked(api::session(&server, &login.token));
                        self.state.login.user = Some(login.user);
                        self.inflight.insert(
                            id,
                            InflightOp::Session {
                                server,
                                token: login.token,
                                remember,
                            },
                        );
                    }
                    Err(err) => {
                        if let DialogState::SignIn(dialog) = &mut self.state.dialog {
                            dialog.in_flight = false;
                            dialog.error = Some(format!(
                                "Sign-in failed: {}. Check the server URL and your credentials, then try again.",
                                err.server_message()
                            ));
                        }
                    }
                }
            }
            InflightOp::Session {
                server,
                token,
                remember,
            } => match api::parse_ok(&result, |r| api::parse_session(r.text().unwrap_or(""))) {
                Ok(session) => {
                    self.state.login.user = Some(session.user);
                    self.state.login.organizations = session.organizations;
                    self.session_established(server, token, remember);
                }
                Err(err) => {
                    let message = format!(
                        "Sign-in failed: {}. Check the server URL and your credentials, then try again.",
                        err.server_message()
                    );
                    if let DialogState::SignIn(dialog) = &mut self.state.dialog {
                        dialog.completing = false;
                        dialog.error = Some(message);
                    } else if matches!(err, ApiError::Unauthorized) {
                        self.handle_unauthorized();
                    }
                }
            },
            InflightOp::CollectionsList { server } => {
                let host = Self::host_of(&server);
                match api::parse_ok(&result, |r| {
                    api::parse_collection_list(r.text().unwrap_or(""))
                }) {
                    Ok(entries) => self.start_sync_exchange(&server, entries),
                    Err(ApiError::Unauthorized) => {
                        self.handle_unauthorized();
                        self.sync_step_done();
                    }
                    Err(ApiError::Transport(err)) => {
                        self.sync_transport_error(&host, &err);
                        self.sync_step_done();
                    }
                    Err(err) => {
                        self.sync_errors.push(err.server_message());
                        self.sync_step_done();
                    }
                }
            }
            InflightOp::CollectionGet {
                server,
                server_id,
                local_id,
            } => {
                let host = Self::host_of(&server);
                match api::parse_ok(&result, |r| {
                    api::parse_collection_get(r.text().unwrap_or(""))
                }) {
                    Ok(get) => self.apply_pulled(&server, &server_id, &local_id, get),
                    Err(ApiError::Unauthorized) => {
                        self.handle_unauthorized();
                        self.sync_step_done();
                    }
                    Err(ApiError::AccessDenied) => {
                        // Confirmed 403 access_denied — the only revocation
                        // trigger (guardrail, PRD §7).
                        if !local_id.is_empty() {
                            if let Some(stored) = self.state.stored_mut(&local_id) {
                                sync::apply_response(stored, &SyncOutcome::Revoked);
                                let name = stored.collection.name.clone();
                                self.state.login.status = ServerStatus::Revoked(name);
                                self.state.mark_edited();
                            }
                        }
                        self.sync_step_done();
                    }
                    Err(ApiError::Transport(err)) => {
                        self.sync_transport_error(&host, &err);
                        self.sync_step_done();
                    }
                    Err(err) => {
                        self.sync_errors.push(err.server_message());
                        self.sync_step_done();
                    }
                }
            }
            InflightOp::CollectionPut {
                server,
                local_id,
                force,
                enable,
            } => {
                let host = Self::host_of(&server);
                match api::parse_ok(&result, |r| api::parse_revision(r.text().unwrap_or(""))) {
                    Ok(new_revision) => {
                        if let Some(server_id) = enable {
                            let user = self.state.login.user.clone().unwrap_or_default();
                            if let Some(stored) = self.state.stored_mut(&local_id) {
                                let name = stored.collection.name.clone();
                                sync::mark_enabled(
                                    stored,
                                    &server,
                                    &server_id,
                                    sync::personal_namespace(&user.id, &user.username),
                                    new_revision,
                                );
                                self.state.push_toast(format!("Synced \"{name}\""));
                                self.state.mark_edited();
                            }
                        } else if let Some(stored) = self.state.stored_mut(&local_id) {
                            let name = stored.collection.name.clone();
                            sync::apply_response(stored, &SyncOutcome::Pushed { new_revision });
                            self.sync_worked = true;
                            if force {
                                self.state
                                    .push_toast(format!("Overwrote server copy of \"{name}\""));
                            } else {
                                self.state.push_toast(format!("Synced \"{name}\""));
                            }
                            self.state.mark_edited();
                        }
                        self.sync_step_done();
                    }
                    Err(ApiError::Unauthorized) => {
                        self.handle_unauthorized();
                        self.sync_step_done();
                    }
                    Err(ApiError::Conflict { server_revision }) => {
                        // Edits retained; dialog decides (§3.5d).
                        let name = self
                            .state
                            .find_stored(&local_id)
                            .map(|s| s.collection.name.clone())
                            .unwrap_or_default();
                        self.sync_errors.push("sync conflict".to_string());
                        self.state.dialog = DialogState::Conflict {
                            collection_id: local_id,
                            name,
                            server_revision,
                        };
                        self.sync_step_done();
                    }
                    Err(ApiError::AccessDenied) => {
                        if let Some(stored) = self.state.stored_mut(&local_id) {
                            sync::apply_response(stored, &SyncOutcome::Revoked);
                            let name = stored.collection.name.clone();
                            self.state.login.status = ServerStatus::Revoked(name);
                            self.state.mark_edited();
                        }
                        self.sync_step_done();
                    }
                    Err(ApiError::Transport(err)) => {
                        self.sync_transport_error(&host, &err);
                        self.sync_step_done();
                    }
                    Err(err) => {
                        self.sync_errors.push(err.server_message());
                        self.sync_step_done();
                    }
                }
            }
            InflightOp::CollectionCreate { server, local_id } => {
                match api::parse_ok(&result, |r| api::parse_create(r.text().unwrap_or(""))) {
                    Ok((server_id, revision)) => {
                        let token = self.state.login.token.clone().unwrap_or_default();
                        if let Some(stored) = self.state.find_stored(&local_id).cloned() {
                            self.collection_put_request(
                                &server,
                                &token,
                                local_id,
                                &server_id,
                                revision,
                                &stored.collection,
                                false,
                                Some(server_id.clone()),
                            );
                        }
                    }
                    Err(err) => {
                        let message = format!(
                            "Could not enable sync: {}. Your collection stays local — nothing is lost.",
                            err.server_message()
                        );
                        self.state.push_banner(
                            Severity::Warning,
                            message,
                            vec![BannerAction::Dismiss],
                        );
                    }
                }
            }
            InflightOp::CollectionDelete { local_id, .. } => {
                match api::parse_ok(&result, |r| Ok(r.text().unwrap_or("").to_string())) {
                    Ok(_) => {
                        let name = self
                            .state
                            .find_stored(&local_id)
                            .map(|s| s.collection.name.clone());
                        self.remove_collection_local(&local_id);
                        if let Some(name) = name {
                            self.state.push_toast(format!("Deleted \"{name}\""));
                        }
                    }
                    Err(err) => {
                        let message = format!(
                            "The server rejected this action: {}. Refresh and try again — the list may be out of date.",
                            err.server_message()
                        );
                        self.state.push_banner(
                            Severity::Danger,
                            message,
                            vec![BannerAction::Refresh],
                        );
                    }
                }
            }
            InflightOp::SharesPut {
                local_id,
                server_id,
                grants,
            } => {
                if let DialogState::Share(dialog) = &mut self.state.dialog {
                    dialog.saving = false;
                }
                match api::parse_ok(&result, |r| Ok(r.text().unwrap_or("").to_string())) {
                    Ok(_) => {
                        self.state.grants_known.insert(server_id, grants);
                        self.state.dialog = DialogState::None;
                    }
                    Err(ApiError::Unauthorized) => self.handle_unauthorized(),
                    Err(err) => {
                        if let DialogState::Share(dialog) = &mut self.state.dialog {
                            dialog.error = Some(err.server_message());
                        }
                    }
                }
                let _ = local_id;
            }
            InflightOp::Publish {
                local_id,
                server_id,
                ..
            } => {
                if let DialogState::Share(dialog) = &mut self.state.dialog {
                    dialog.publishing = false;
                }
                match api::parse_ok(&result, |r| api::parse_publish(r.text().unwrap_or(""))) {
                    Ok(slug) => {
                        self.state
                            .slug_known
                            .insert(server_id.clone(), slug.clone());
                        if let DialogState::Share(dialog) = &mut self.state.dialog {
                            dialog.live_slug = Some(slug.clone());
                        }
                        let url = api::public_url(&self.server_url(), &slug);
                        self.state.push_toast(format!("Published at {url}"));
                    }
                    Err(ApiError::Unauthorized) => self.handle_unauthorized(),
                    Err(err) => {
                        if let DialogState::Share(dialog) = &mut self.state.dialog {
                            dialog.error = Some(err.server_message());
                        }
                    }
                }
                let _ = local_id;
            }
            InflightOp::Unpublish {
                local_id,
                server_id,
                slug,
            } => {
                if let DialogState::Share(dialog) = &mut self.state.dialog {
                    dialog.publishing = false;
                }
                match api::parse_ok(&result, |r| Ok(r.text().unwrap_or("").to_string())) {
                    Ok(_) => {
                        self.state.slug_known.remove(&server_id);
                        if let DialogState::Share(dialog) = &mut self.state.dialog {
                            dialog.live_slug = None;
                            dialog.public_on = false;
                        }
                        self.state.push_toast(format!("Unpublished {slug}"));
                    }
                    Err(ApiError::Unauthorized) => self.handle_unauthorized(),
                    Err(err) => {
                        if let DialogState::Share(dialog) = &mut self.state.dialog {
                            dialog.error = Some(err.server_message());
                        }
                    }
                }
                let _ = local_id;
            }
            InflightOp::Admin(op) => ui::admin::handle_admin_result(self, op, result),
        }
    }

    // === sync exchange helpers ==========================================================

    fn start_sync_exchange(&mut self, server: &str, entries: Vec<api::CollectionListEntry>) {
        for entry in &entries {
            self.state.sync_info.insert(
                entry.id.clone(),
                state::SyncInfo {
                    access: entry.access.clone(),
                },
            );
        }
        let all: Vec<StoredCollection> = self
            .state
            .doc
            .personal
            .iter()
            .chain(self.state.doc.organizations.iter())
            .cloned()
            .collect();
        let server_entries: Vec<sync::ServerEntry> = entries
            .iter()
            .map(|e| sync::ServerEntry {
                id: e.id.clone(),
                revision: e.revision,
            })
            .collect();
        let plan = sync::plan_pull(&all, &server_entries);
        let token = self.state.login.token.clone().unwrap_or_default();
        let by_local_id: HashMap<String, String> = all
            .iter()
            .filter_map(|s| {
                s.origin
                    .as_ref()
                    .map(|o| (s.collection.id.clone(), o.collection_id.clone()))
            })
            .collect();

        self.sync_pending = 0;
        for local_id in plan.to_push {
            if let Some((base, collection)) = all
                .iter()
                .find(|s| s.collection.id == local_id)
                .and_then(sync::plan_push)
            {
                if let Some(server_id) = by_local_id.get(&local_id) {
                    self.collection_put_request(
                        server,
                        &token,
                        local_id,
                        server_id,
                        base,
                        &collection,
                        false,
                        None,
                    );
                }
            }
        }
        for server_id in plan.to_pull {
            let local_id = by_local_id
                .iter()
                .find(|(_, sid)| **sid == server_id)
                .map(|(local, _)| local.clone())
                .unwrap_or_default();
            self.collection_get_request(server, &token, local_id, &server_id);
        }
        for server_id in plan.to_verify {
            if let Some((local_id, _)) = by_local_id.iter().find(|(_, sid)| **sid == server_id) {
                let local_id = local_id.clone();
                self.collection_get_request(server, &token, local_id, &server_id);
            }
        }

        self.last_list_entries = entries;
        if self.sync_pending == 0 {
            // Nothing to exchange — finish the pass immediately.
            self.sync_worked = false;
            self.sync_step_done();
        }
    }

    fn apply_pulled(
        &mut self,
        server: &str,
        server_id: &str,
        local_id: &str,
        get: api::CollectionGetResponse,
    ) {
        let namespace = self.list_entry_namespace(server_id).unwrap_or_else(|| {
            let user = self.state.login.user.clone().unwrap_or_default();
            NamespaceRef {
                kind: NamespaceKind::Personal,
                id: user.id,
                name: user.username,
            }
        });
        let was_revoked = self
            .state
            .find_stored(local_id)
            .map(|s| s.sync == SyncState::Revoked)
            .unwrap_or(false);
        let name = get.collection.name.clone();
        let revision = get.revision;
        let access = match get.access.as_str() {
            "full" => Some(restrust_core::share::AccessLevel::FullAccess),
            "read" => Some(restrust_core::share::AccessLevel::ReadOnly),
            _ => None,
        };
        if local_id.is_empty() {
            let stored = StoredCollection {
                collection: get.collection,
                sync: SyncState::Synced { revision },
                origin: Some(ServerOrigin {
                    server_url: server.to_string(),
                    collection_id: server_id.to_string(),
                    namespace,
                }),
                access,
            };
            match stored.origin.as_ref().map(|o| o.namespace.kind) {
                Some(NamespaceKind::Organization) => self.state.doc.organizations.push(stored),
                _ => self.state.doc.personal.push(stored),
            }
        } else if let Some(stored) = self.state.stored_mut(local_id) {
            stored.collection = get.collection;
            stored.access = access;
            if let Some(origin) = stored.origin.as_mut() {
                origin.server_url = server.to_string();
                origin.collection_id = server_id.to_string();
            }
            sync::apply_response(stored, &SyncOutcome::Pulled { revision });
        }
        self.sync_worked = true;
        if was_revoked {
            self.state
                .push_toast(format!("Access to \"{name}\" was restored"));
            self.normalize_revoked_pill();
        } else {
            self.state.push_toast(format!("Synced \"{name}\""));
        }
        self.state.mark_edited();
        self.sync_step_done();
    }

    fn list_entry_namespace(&self, server_id: &str) -> Option<NamespaceRef> {
        self.last_list_entries
            .iter()
            .find(|e| e.id == server_id)
            .map(|e| NamespaceRef {
                kind: match e.namespace.kind.as_str() {
                    "organization" => NamespaceKind::Organization,
                    _ => NamespaceKind::Personal,
                },
                id: e.namespace.id.clone(),
                name: e.namespace.name.clone(),
            })
    }
}

#[cfg(target_arch = "wasm32")]
fn wasm_origin() -> String {
    eframe::web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default()
}

impl eframe::App for RestrustApp {
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        if self.state.dirty {
            self.save_now();
        }
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.apply_theme_if_needed(&ctx);
        ui::handle_shortcuts(self, &ctx);
        self.drain_http();
        self.autosave_tick();
        self.tick_toasts();

        // egui only repaints on input; schedule frames while a timer (autosave,
        // toast expiry) has a pending deadline so it actually fires when idle.
        if self.state.dirty {
            ctx.request_repaint_after(std::time::Duration::from_millis(1100));
        }
        if !self.state.toasts.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }

        // Admin window close (native viewport close button).
        {
            let mut admin = self.admin.lock().unwrap();
            if admin.close_requested {
                admin.close_requested = false;
                self.state.admin_open = false;
            }
        }

        ui::draw(self, root, &ctx);
    }
}

impl Drop for RestrustApp {
    fn drop(&mut self) {
        if self.state.dirty {
            self.save_now();
        }
    }
}
