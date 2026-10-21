//! Request editor (ui-spec §2.5): URL row, section tabs, Params/Body/Headers/
//! Auth/Settings sections. Edits are applied directly to the model with dirty
//! tracking (clone-edit-write-back per frame).

use eframe::egui::{self, RichText, Ui};

use crate::app::RestrustApp;
use crate::state::{SectionTab, TabKind};
use crate::theme;
use crate::ui::common::{
    bulk_to_rows, busy_button, current_tokens, kv_editor, mono_field, muted, primary_button,
    rows_to_bulk,
};

use restrust_core::model::{
    ApiKeyIn, AuthConfig, AuthKind, BodyKind, HttpMethod, RequestBody, RequestDef,
};
use restrust_core::workspace_doc::SyncState;

const METHODS: [HttpMethod; 7] = [
    HttpMethod::Get,
    HttpMethod::Post,
    HttpMethod::Put,
    HttpMethod::Patch,
    HttpMethod::Delete,
    HttpMethod::Head,
    HttpMethod::Options,
];

pub fn title_row(app: &mut RestrustApp, ui: &mut Ui) {
    let Some(tab) = app.state.active_tab() else {
        return;
    };
    let name = tab.label(&app.state.doc);
    let location = match &tab.kind {
        TabKind::Scratch(_) => "Unsaved request".to_string(),
        TabKind::Saved { collection_id, .. } => app
            .state
            .find_stored(collection_id)
            .map(|s| format!("{} /", s.collection.name))
            .unwrap_or_default(),
    };
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                muted(ui, &location);
                ui.label(RichText::new(name).strong());
                if !can_edit(app) {
                    muted(ui, "Read only");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Code")
                        .on_hover_text("Generate a code snippet")
                        .clicked()
                    {
                        if let Some(tab) = app.state.active_tab_mut() {
                            tab.code_open = !tab.code_open;
                        }
                    }
                    if ui
                        .button("Save")
                        .on_hover_text("Save request  Ctrl+S")
                        .clicked()
                    {
                        app.save_active_request();
                    }
                });
            });
        });
}

fn can_edit(app: &RestrustApp) -> bool {
    match app.state.active_tab().map(|tab| &tab.kind) {
        Some(TabKind::Saved { collection_id, .. }) => app
            .state
            .find_stored(collection_id)
            .is_some_and(|s| app.state.can_edit_tree(s)),
        _ => true,
    }
}

pub fn code_panel(app: &mut RestrustApp, ui: &mut Ui) {
    use restrust_interchange::snippet::{self, Target};
    ui.horizontal(|ui| {
        ui.strong("Code snippet");
        if ui.button("×").clicked() {
            if let Some(tab) = app.state.active_tab_mut() {
                tab.code_open = false;
            }
        }
    });
    let key = ui.scope_id().with("snippet_language");
    let mut selected = ui.ctx().data(|d| d.get_temp::<usize>(key).unwrap_or(0));
    let options = [
        ("cURL", Target::Curl),
        ("Rust · reqwest", Target::RustReqwest),
        ("Python · requests", Target::PythonRequests),
        ("JavaScript · fetch", Target::JsFetch),
        ("Go · net/http", Target::GoNetHttp),
    ];
    egui::ComboBox::from_id_salt("snippet_language")
        .selected_text(options[selected].0)
        .show_ui(ui, |ui| {
            for (index, (name, _)) in options.iter().enumerate() {
                ui.selectable_value(&mut selected, index, *name);
            }
        });
    ui.ctx().data_mut(|d| d.insert_temp(key, selected));
    if let Some(mut def) = app.state.active_request_def() {
        let (auth, _) = app.state.auth_and_scopes_for(&def);
        def.auth = Some(AuthConfig { kind: auth });
        let code = snippet::generate(&def, options[selected].1);
        if ui.button("Copy snippet").clicked() {
            app.services.clipboard.set_text(&code);
            app.state.push_toast("Code snippet copied".into());
        }
        ui.separator();
        egui::ScrollArea::both()
            .id_salt("snippet_scroll")
            .show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(code).monospace()).selectable(true));
            });
    }
}

/// The URL row (36 px; wraps into two rows under 560 px, §1.4).
pub fn url_row(app: &mut RestrustApp, ui: &mut Ui, narrow: bool) {
    let t = current_tokens(ui);
    egui::Frame::NONE
        .fill(t.panel)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            let Some(original) = app.state.active_request_def() else {
                return;
            };
            let mut def = original.clone();
            let editable = can_edit(app);
            let sending = app
                .state
                .active_tab()
                .map(|tab| app.state.sending_tabs.contains(&tab.id))
                .unwrap_or(false);
            let enter_to_send =
                ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));

            if narrow {
                ui.set_min_height(theme::URL_ROW_HEIGHT);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(editable, |ui| method_combo(ui, &mut def.method));
                        let label = if sending { "Sending…" } else { "Send" };
                        if busy_button(ui, "Send", label, sending).clicked() {
                            commit_and_send(app, &original, &def, sending);
                        }
                    });
                    ui.add_enabled(
                        editable,
                        egui::TextEdit::singleline(&mut def.url)
                            .hint_text("https://api.example.com/path")
                            .font(egui::TextStyle::Monospace)
                            .desired_width(ui.available_width()),
                    );
                });
            } else {
                ui.set_min_height(theme::URL_ROW_HEIGHT);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(editable, |ui| method_combo(ui, &mut def.method));
                    ui.add_enabled(
                        editable,
                        egui::TextEdit::singleline(&mut def.url)
                            .hint_text("https://api.example.com/path")
                            .font(egui::TextStyle::Monospace)
                            .desired_width(ui.available_width() - 96.0),
                    );
                    let label = if sending { "Sending…" } else { "Send" };
                    if busy_button(ui, "Send", label, sending).clicked() {
                        commit_and_send(app, &original, &def, sending);
                    }
                });
            }
            if enter_to_send && !sending {
                commit_and_send(app, &original, &def, sending);
            } else if def != original {
                app.state.apply_request_edit(&def);
            }
        });
}

fn commit_and_send(app: &mut RestrustApp, original: &RequestDef, def: &RequestDef, sending: bool) {
    if def != original {
        app.state.apply_request_edit(def);
    }
    if !sending {
        app.send_active_request();
    }
}

/// The current model state of the active tab's request (for change checks).
fn snapshot_of(app: &RestrustApp) -> RequestDef {
    app.state
        .active_request_def()
        .unwrap_or_else(|| RequestDef::new(HttpMethod::Get, "", ""))
}

fn method_combo(ui: &mut Ui, method: &mut HttpMethod) {
    let t = current_tokens(ui);
    let color = crate::theme::method_color(*method, t.dark);
    egui::ComboBox::from_id_salt("method_combo")
        .width(86.0)
        .selected_text(RichText::new(method.as_str()).strong().color(color))
        .show_ui(ui, |ui| {
            for candidate in METHODS {
                let color = crate::theme::method_color(candidate, t.dark);
                ui.selectable_value(
                    method,
                    candidate,
                    RichText::new(candidate.as_str()).color(color),
                );
            }
        });
}

/// Section tab bar + the active section body (§2.5).
pub fn draw(app: &mut RestrustApp, ui: &mut Ui) {
    let t = current_tokens(ui);
    egui::Frame::NONE
        .fill(t.bg)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            section_bar(app, ui);
            ui.separator();
            let Some(def) = app.state.active_request_def() else {
                return;
            };
            let mut def = def;
            let section = app
                .state
                .active_tab()
                .map(|tab| tab.section)
                .unwrap_or(SectionTab::Params);
            ui.add_enabled_ui(can_edit(app), |ui| match section {
                SectionTab::Params => params_section(ui, &mut def),
                SectionTab::Body => body_section(app, ui, &mut def),
                SectionTab::Headers => headers_section(ui, &mut def),
                SectionTab::Auth => auth_section(app, ui, &mut def),
                SectionTab::Settings => settings_section(app, ui, &mut def),
            });
            if def != snapshot_of(app) {
                let changed = def.clone();
                app.state.apply_request_edit(&changed);
            }
        });
}

fn section_bar(app: &mut RestrustApp, ui: &mut Ui) {
    let Some(def) = app.state.active_request_def() else {
        return;
    };
    let enabled_params = def.query.iter().filter(|r| r.enabled).count();
    let enabled_headers = def.headers.iter().filter(|r| r.enabled).count();
    let body_kind = body_kind_label(&def.body.kind);
    let auth_label = auth_tab_label(app, &def);
    let counts_hide = |n: usize, has_content: bool| !has_content && n == 0;

    ui.horizontal(|ui| {
        let mut section = app
            .state
            .active_tab()
            .map(|tab| tab.section)
            .unwrap_or(SectionTab::Params);
        let active_color = current_tokens(ui).on_accent;
        let text_color = ui.visuals().text_color();
        let label = |text: String, active: bool| {
            egui::RichText::new(text)
                .strong()
                .color(if active { active_color } else { text_color })
        };
        let params_label =
            if counts_hide(enabled_params, def.query.iter().any(|r| !r.key.is_empty())) {
                "Params".to_string()
            } else {
                format!("Params ({enabled_params})")
            };
        if ui
            .selectable_label(
                section == SectionTab::Params,
                label(params_label, section == SectionTab::Params),
            )
            .clicked()
        {
            section = SectionTab::Params;
        }
        let body_label = if matches!(def.body.kind, BodyKind::None) && def.body.raw.is_none() {
            "Body".to_string()
        } else {
            format!("Body ({body_kind})")
        };
        if ui
            .selectable_label(
                section == SectionTab::Body,
                label(body_label, section == SectionTab::Body),
            )
            .clicked()
        {
            section = SectionTab::Body;
        }
        let headers_label = if counts_hide(
            enabled_headers,
            def.headers.iter().any(|r| !r.key.is_empty()),
        ) {
            "Headers".to_string()
        } else {
            format!("Headers ({enabled_headers})")
        };
        if ui
            .selectable_label(
                section == SectionTab::Headers,
                label(headers_label, section == SectionTab::Headers),
            )
            .clicked()
        {
            section = SectionTab::Headers;
        }
        if ui
            .selectable_label(
                section == SectionTab::Auth,
                label(auth_label, section == SectionTab::Auth),
            )
            .clicked()
        {
            section = SectionTab::Auth;
        }
        if ui
            .selectable_label(
                section == SectionTab::Settings,
                label("Settings".to_string(), section == SectionTab::Settings),
            )
            .clicked()
        {
            section = SectionTab::Settings;
        }
        if let Some(tab) = app.state.active_tab_mut() {
            tab.section = section;
        }
    });
}

fn body_kind_label(kind: &BodyKind) -> &'static str {
    match kind {
        BodyKind::None => "none",
        BodyKind::Raw { .. } => "raw",
        BodyKind::UrlEncoded => "urlencoded",
        BodyKind::Multipart => "multipart",
        BodyKind::Binary { .. } => "binary",
    }
}

fn auth_tab_label(app: &RestrustApp, def: &RequestDef) -> String {
    let inherited = match app.state.active_tab().map(|tab| &tab.kind) {
        Some(TabKind::Saved { collection_id, .. }) => app
            .state
            .find_stored(collection_id)
            .map(|stored| stored.collection.resolve_request_auth(&def.id))
            .filter(|kind| !matches!(kind, AuthKind::None)),
        _ => None,
    };
    match (&def.auth, inherited) {
        (None, Some(_)) => "Auth (Inherited)".to_string(),
        (None, None) => "Auth".to_string(),
        (Some(config), _) => format!("Auth ({})", auth_kind_label(&config.kind)),
    }
}

pub fn auth_kind_label(kind: &AuthKind) -> &'static str {
    match kind {
        AuthKind::None => "No Auth",
        AuthKind::Bearer { .. } => "Bearer Token",
        AuthKind::Basic { .. } => "Basic Auth",
        AuthKind::ApiKey { .. } => "API Key",
    }
}

// --- sections ---------------------------------------------------------------------

fn params_section(ui: &mut Ui, def: &mut RequestDef) {
    let tab_id = ui.scope_id().with("params");
    let mut bulk = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(tab_id).unwrap_or(false));
    if ui
        .add(egui::Button::new(RichText::new("Bulk edit").size(12.0)))
        .clicked()
    {
        bulk = !bulk;
    }
    ui.ctx().data_mut(|d| d.insert_temp(tab_id, bulk));
    if bulk {
        let text_id = tab_id.with("buffer");
        let mut text = ui
            .ctx()
            .data(|d| d.get_temp::<String>(text_id))
            .unwrap_or_else(|| rows_to_bulk(&def.query));
        let response = ui.add(
            egui::TextEdit::multiline(&mut text)
                .font(egui::TextStyle::Monospace)
                .desired_rows(6)
                .desired_width(ui.available_width()),
        );
        if response.changed() {
            def.query = bulk_to_rows(&text, &def.query);
        }
        ui.ctx().data_mut(|d| d.insert_temp(text_id, text));
    } else {
        ui.ctx()
            .data_mut(|d| d.remove::<String>(tab_id.with("buffer")));
        kv_editor(ui, "params", &mut def.query, true, &mut || {});
    }
}

fn body_section(app: &mut RestrustApp, ui: &mut Ui, def: &mut RequestDef) {
    ui.horizontal(|ui| {
        let kinds = [
            (BodyKind::None, "None"),
            (
                BodyKind::Raw {
                    mime: "application/json".into(),
                },
                "Raw",
            ),
            (BodyKind::UrlEncoded, "URL-encoded"),
            (BodyKind::Multipart, "Multipart"),
            (
                BodyKind::Binary {
                    file_path: String::new(),
                },
                "Binary",
            ),
        ];
        for (kind, label) in kinds {
            let same = body_kind_same(&def.body.kind, &kind);
            if ui
                .selectable_label(same, RichText::new(label).strong())
                .clicked()
                && !same
            {
                def.body = RequestBody {
                    kind,
                    raw: None,
                    rows: Vec::new(),
                };
            }
        }
    });
    ui.add_space(theme::GAP_TIGHT);
    match &mut def.body.kind {
        BodyKind::None => {
            muted(ui, "This request has no body");
        }
        BodyKind::Raw { mime } => {
            ui.horizontal(|ui| {
                ui.label("Mime");
                egui::ComboBox::from_id_salt("raw_mime")
                    .selected_text(pretty_mime(mime))
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            ("text/plain", "Text"),
                            ("application/json", "JSON"),
                            ("application/xml", "XML"),
                            ("text/html", "HTML"),
                            ("application/javascript", "JavaScript"),
                        ] {
                            ui.selectable_value(mime, value.to_string(), label);
                        }
                    });
            });
            let mut raw = def.body.raw.clone().unwrap_or_default();
            ui.add(
                egui::TextEdit::multiline(&mut raw)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(6)
                    .desired_width(ui.available_width())
                    .hint_text("{}"),
            );
            def.body.raw = Some(raw);
        }
        BodyKind::UrlEncoded => {
            let mut rows = std::mem::take(&mut def.body.rows);
            kv_editor(ui, "body_urlencoded", &mut rows, true, &mut || {});
            def.body.rows = rows;
        }
        BodyKind::Multipart => {
            let mut rows = std::mem::take(&mut def.body.rows);
            kv_editor(ui, "body_multipart", &mut rows, true, &mut || {});
            def.body.rows = rows;
            muted(ui, "File parts are not supported in v1 — text parts only");
        }
        BodyKind::Binary { file_path } => {
            if crate::platform::IS_WASM {
                ui.add_enabled(
                    false,
                    egui::TextEdit::singleline(&mut String::new())
                        .hint_text("Binary bodies require the desktop app"),
                );
                muted(ui, "Binary bodies require the desktop app");
            } else {
                ui.label("File path");
                mono_field(ui, file_path, "/path/to/file");
            }
        }
    }
    let _ = app;
}

fn body_kind_same(a: &BodyKind, b: &BodyKind) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

fn pretty_mime(mime: &str) -> String {
    match mime {
        "text/plain" => "Text".to_string(),
        "application/json" => "JSON".to_string(),
        "application/xml" => "XML".to_string(),
        "text/html" => "HTML".to_string(),
        "application/javascript" => "JavaScript".to_string(),
        other => other.to_string(),
    }
}

fn headers_section(ui: &mut Ui, def: &mut RequestDef) {
    kv_editor(ui, "headers", &mut def.headers, true, &mut || {});
    ui.add_space(theme::GAP_TIGHT);
    muted(
        ui,
        "Transport headers (Host, Content-Length, …) are added automatically",
    );
}

fn auth_section(app: &mut RestrustApp, ui: &mut Ui, def: &mut RequestDef) {
    // Inherited summary when the request defines no auth of its own (§2.5).
    let inherited = match app.state.active_tab().map(|tab| tab.kind.clone()) {
        Some(TabKind::Saved { collection_id, .. }) => {
            let resolved = app
                .state
                .find_stored(&collection_id)
                .map(|stored| stored.collection.resolve_request_auth(&def.id))
                .unwrap_or(AuthKind::None);
            if matches!(resolved, AuthKind::None) {
                None
            } else {
                Some((resolved, collection_id))
            }
        }
        _ => None,
    };

    if def.auth.is_none() {
        if let Some((resolved, collection_id)) = &inherited {
            let name = app
                .state
                .find_stored(collection_id)
                .map(|s| s.collection.name.clone())
                .unwrap_or_default();
            muted(
                ui,
                &format!(
                    "Inherited from collection “{name}” ({})",
                    auth_kind_label(resolved)
                ),
            );
            ui.add_space(theme::GAP_TIGHT);
            if primary_button(ui, "Override", 0.0).clicked() {
                def.auth = Some(AuthConfig {
                    kind: resolved.clone(),
                });
            }
            return;
        }
    }

    ui.horizontal(|ui| {
        ui.label("Auth type");
        let current = def
            .auth
            .as_ref()
            .map(|a| auth_kind_label(&a.kind))
            .unwrap_or("No Auth");
        egui::ComboBox::from_id_salt("auth_kind")
            .selected_text(current)
            .show_ui(ui, |ui| {
                let mut pick = |kind: AuthKind| {
                    ui.selectable_value(
                        &mut def.auth,
                        Some(AuthConfig { kind: kind.clone() }),
                        auth_kind_label(&kind),
                    );
                };
                pick(AuthKind::None);
                pick(AuthKind::Bearer {
                    token: String::new(),
                });
                pick(AuthKind::Basic {
                    username: String::new(),
                    password: String::new(),
                });
                pick(AuthKind::ApiKey {
                    key: String::new(),
                    value: String::new(),
                    add_to: ApiKeyIn::Header,
                });
            });
    });
    ui.add_space(theme::GAP_TIGHT);

    let Some(config) = def.auth.as_mut() else {
        muted(ui, "No auth is attached to this request.");
        return;
    };
    match &mut config.kind {
        AuthKind::None => {
            muted(ui, "No auth is attached to this request.");
        }
        AuthKind::Bearer { token } => {
            ui.label("Token");
            mono_field(ui, token, "token");
        }
        AuthKind::Basic { username, password } => {
            ui.label("Username");
            ui.text_edit_singleline(username);
            ui.label("Password");
            ui.add(egui::TextEdit::singleline(password).password(true));
        }
        AuthKind::ApiKey { key, value, add_to } => {
            ui.label("Key");
            ui.text_edit_singleline(key);
            ui.label("Value");
            mono_field(ui, value, "value");
            ui.label("Add to");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("api_key_in")
                    .selected_text(match add_to {
                        ApiKeyIn::Header => "Header",
                        ApiKeyIn::Query => "Query params",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(add_to, ApiKeyIn::Header, "Header");
                        ui.selectable_value(add_to, ApiKeyIn::Query, "Query params");
                    });
            });
        }
    }
    if inherited.is_some() {
        ui.add_space(theme::GAP_TIGHT);
        if ui.button("Use inherited").clicked() {
            def.auth = None;
        }
    }
}

fn settings_section(app: &mut RestrustApp, ui: &mut Ui, def: &mut RequestDef) {
    ui.label("Request name");
    ui.text_edit_singleline(&mut def.name);
    ui.add_space(theme::GAP_TIGHT);
    ui.label("Description");
    let mut description = def.description.clone().unwrap_or_default();
    ui.add(
        egui::TextEdit::multiline(&mut description)
            .desired_rows(3)
            .desired_width(ui.available_width()),
    );
    def.description = if description.trim().is_empty() {
        None
    } else {
        Some(description)
    };
    ui.add_space(theme::GAP_STACK);
    muted(ui, &format!("ID: {}", def.id));

    let (location, sync_text) = app
        .state
        .active_tab()
        .map(|tab| &tab.kind)
        .and_then(|kind| match kind {
            TabKind::Saved { collection_id, .. } => {
                app.state.find_stored(collection_id).map(|stored| {
                    let location = match stored.origin.as_ref().map(|o| o.namespace.kind) {
                        Some(restrust_core::workspace_doc::NamespaceKind::Organization) => format!(
                            "{} · {}",
                            stored
                                .origin
                                .as_ref()
                                .map(|o| o.namespace.name.clone())
                                .unwrap_or_default(),
                            stored.collection.name
                        ),
                        _ => format!("Personal · {}", stored.collection.name),
                    };
                    let sync_text = match &stored.sync {
                        SyncState::Local => "Local".to_string(),
                        SyncState::Synced { revision } => format!("Synced (rev {revision})"),
                        SyncState::PendingEdits { .. } => "Unsynced edits".to_string(),
                        SyncState::Revoked => "Revoked".to_string(),
                    };
                    (location, sync_text)
                })
            }
            TabKind::Scratch(_) => None,
        })
        .unwrap_or_else(|| {
            (
                "Scratch tab — not saved yet".to_string(),
                "Local".to_string(),
            )
        });
    muted(ui, &format!("Location: {location}"));
    muted(ui, &format!("Sync state: {sync_text}"));

    if let Some(tab_id) = app.state.active_tab().map(|t| t.id) {
        if matches!(
            app.state.active_tab().map(|t| t.kind.clone()),
            Some(TabKind::Scratch(_))
        ) {
            ui.add_space(theme::GAP_SECTION);
            crate::ui::common::section_label(ui, "Save into…");
            save_scratch_into(app, ui, tab_id, def);
        }
    }
}

fn save_scratch_into(app: &mut RestrustApp, ui: &mut Ui, tab_id: u64, def: &RequestDef) {
    let dialog_open = matches!(
        &app.state.dialog,
        crate::state::DialogState::SaveScratchInto { tab_id: t, .. } if *t == tab_id
    );
    if !dialog_open {
        if primary_button(ui, "Save into…", 0.0).clicked() {
            app.state.dialog = crate::state::DialogState::SaveScratchInto {
                tab_id,
                collection_id: None,
                parent_folder: None,
            };
        }
        return;
    }
    // The dialog itself lives in dialogs.rs; keep this section quiet while open.
    let _ = def;
    muted(ui, "Choose a destination in the dialog.");
}
