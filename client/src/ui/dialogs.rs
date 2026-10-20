//! Modal dialogs (ui-spec §3): Sign in, Share, Import, Export, Settings,
//! Shortcuts, About, confirmations (incl. the 409 conflict), environment
//! dialogs and Save-into. All use `egui::Modal` with the §3 frame; Esc closes
//! unless an operation is in flight.

use eframe::egui::{self, Context, Modal, RichText, Ui};

use crate::app::RestrustApp;
use crate::platform::IS_WASM;
use crate::state::{DialogState, ExportFormat, ImportFormat, SignInDialog};
use crate::theme;
use crate::ui::common::{
    busy_button, current_tokens, danger_button, dialog_frame, heading, kv_editor, mono_field,
    muted, primary_button, section_label,
};

use restrust_core::model::{Collection, Environment};
use restrust_core::share::{AccessLevel, ShareGrant, ShareSubject};
use restrust_core::workspace_doc::SyncState;

pub fn draw(app: &mut RestrustApp, ctx: &egui::Context) {
    match app.state.dialog.clone() {
        DialogState::None => {}
        DialogState::SignIn(dialog) => sign_in(app, ctx, *dialog),
        DialogState::Share(dialog) => share(app, ctx, *dialog),
        DialogState::Import(dialog) => import(app, ctx, *dialog),
        DialogState::Export(dialog) => export(app, ctx, *dialog),
        DialogState::Settings => settings(app, ctx),
        DialogState::Shortcuts => shortcuts(app, ctx),
        DialogState::About => about(app, ctx),
        DialogState::NewCollection { name } => new_collection(app, ctx, &name),
        DialogState::NewEnvironment(dialog) => new_environment(app, ctx, *dialog),
        DialogState::ManageEnvironments => manage_environments(app, ctx),
        DialogState::ConfirmDelete {
            collection_id,
            name,
            requests,
            on_server,
            busy,
        } => confirm_delete(app, ctx, &collection_id, &name, requests, on_server, busy),
        DialogState::ConfirmRemoveLocal {
            collection_id,
            name,
        } => confirm_remove_local(app, ctx, &collection_id, &name),
        DialogState::Conflict {
            collection_id,
            name,
            server_revision,
        } => conflict(app, ctx, &collection_id, &name, server_revision),
        DialogState::SaveScratchInto {
            tab_id,
            collection_id,
            parent_folder,
        } => save_scratch_into(app, ctx, tab_id, collection_id, parent_folder),
    }
}

/// Opens the Share dialog for a stored collection (owner-gated callers only).
pub fn open_share(app: &mut RestrustApp, collection_id: &str) {
    let Some(stored) = app.state.find_stored(collection_id) else {
        return;
    };
    let server_id = stored
        .origin
        .as_ref()
        .map(|o| o.collection_id.clone())
        .unwrap_or_default();
    let grants = app
        .state
        .grants_known
        .get(&server_id)
        .cloned()
        .unwrap_or_default();
    let slug = app.state.slug_known.get(&server_id).cloned();
    let name = stored.collection.name.clone();
    let slug_default = slug
        .clone()
        .unwrap_or_else(|| crate::platform::sanitize_slug(&name));
    app.state.dialog = DialogState::Share(Box::new(crate::state::ShareDialog {
        local_collection_id: collection_id.to_string(),
        server_collection_id: server_id,
        name,
        grants,
        add_username: String::new(),
        add_level: AccessLevel::ReadOnly,
        org_wide: false,
        org_choice: app.state.login.organizations.first().map(|o| o.id.clone()),
        public_on: slug.is_some(),
        slug: slug_default,
        live_slug: slug,
        saving: false,
        publishing: false,
        error: None,
    }));
}

/// Opens the Export dialog for a stored collection.
pub fn open_export(app: &mut RestrustApp, collection_id: &str) {
    let Some(stored) = app.state.find_stored(collection_id) else {
        return;
    };
    let name = stored.collection.name.clone();
    let output = app
        .generate_export(&stored.collection, ExportFormat::Postman21)
        .unwrap_or_default();
    let path = default_export_path(&name);
    app.state.dialog = DialogState::Export(Box::new(crate::state::ExportDialog {
        collection_id: collection_id.to_string(),
        name,
        format: ExportFormat::Postman21,
        output,
        path,
        error: None,
    }));
}

fn default_export_path(name: &str) -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::platform::default_export_path(name)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        String::new()
    }
}

// --- generic modal helpers -----------------------------------------------------------

fn centered(ctx: &Context, _width: f32) -> egui::Pos2 {
    let screen = ctx.content_rect();
    egui::pos2(screen.left(), screen.top() + screen.height() * 0.15)
}

fn current_tokens_ctx(ctx: &Context) -> crate::theme::ThemeTokens {
    if ctx.theme() == egui::Theme::Dark {
        theme::DARK
    } else {
        theme::LIGHT
    }
}

// --- 3.1 Sign in -----------------------------------------------------------------------

fn sign_in(app: &mut RestrustApp, ctx: &egui::Context, mut dialog: SignInDialog) {
    let t = current_tokens_ctx(ctx);
    let providers_needed = !dialog.providers_loaded && !dialog.providers_failed;
    let response = Modal::new(egui::Id::unique("sign_in_dialog"))
        .area(egui::Area::new(egui::Id::unique("sign_in_dialog")).fixed_pos(centered(ctx, 420.0)))
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(420.0);
            heading(ui, "Sign in");
            ui.add_space(theme::GAP_STACK);

            section_label(ui, "Server");
            ui.label("Server URL");
            ui.text_edit_singleline(&mut dialog.server_url);
            ui.checkbox(&mut dialog.remember, "Remember this server");
            ui.separator();

            section_label(ui, "Local account");
            ui.label("Username");
            ui.text_edit_singleline(&mut dialog.username);
            ui.label("Password");
            ui.add(egui::TextEdit::singleline(&mut dialog.password).password(true));
            if busy_button(ui, "Sign in", "Signing in…", dialog.in_flight).clicked() {
                dialog.in_flight = true;
                dialog.error = None;
                let server = normalize_server(&dialog.server_url);
                app.login_password(
                    server.clone(),
                    dialog.username.clone(),
                    dialog.password.clone(),
                    dialog.remember,
                );
            }
            ui.separator();

            section_label(ui, "Provider sign-in");
            if !dialog.providers_loaded && !dialog.providers_failed {
                ui.add(egui::Spinner::new().size(14.0));
            } else if dialog.providers.is_empty() {
                muted(ui, "No providers are configured on this server");
            } else {
                ui.horizontal(|ui| {
                    for provider in &dialog.providers {
                        if ui.button(&provider.name).clicked() {
                            dialog.provider_pending = Some(provider.id.clone());
                            let url = crate::api::oidc_start_url(
                                &normalize_server(&dialog.server_url),
                                &provider.id,
                            );
                            if IS_WASM {
                                crate::platform::navigate_current_page(&url);
                            } else {
                                app.services.browser.open_url(&url);
                            }
                        }
                    }
                });
            }
            if let Some(_provider) = &dialog.provider_pending {
                if !IS_WASM {
                    ui.separator();
                    section_label(ui, "Complete sign-in");
                    muted(
                        ui,
                        "A browser window opened. Sign in, then paste the token shown on the done page.",
                    );
                    ui.label("Session token");
                    mono_field(ui, &mut dialog.paste_token, "paste the token here");
                    if busy_button(ui, "Complete sign-in", "Signing in…", dialog.completing).clicked()
                    {
                        dialog.completing = true;
                        dialog.error = None;
                        app.complete_with_token(
                            normalize_server(&dialog.server_url),
                            dialog.paste_token.trim().to_string(),
                            dialog.remember,
                        );
                    }
                } else {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0));
                        muted(ui, "Waiting for the server to redirect back…");
                    });
                }
            }

            if let Some(error) = &dialog.error {
                error_block(ui, error);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() && !dialog.in_flight && !dialog.completing {
                    app.state.dialog = DialogState::None;
                }
            });
        });
    if response.should_close() && !dialog.in_flight && !dialog.completing {
        app.state.dialog = DialogState::None;
        return;
    }
    if providers_needed && !dialog.server_url.trim().is_empty() {
        let fetched_flag = egui::Id::unique("signin_providers_fetched");
        let already = ctx.data(|d| d.get_temp::<bool>(fetched_flag).unwrap_or(false));
        if !already {
            ctx.data_mut(|d| d.insert_temp(fetched_flag, true));
            let server = normalize_server(&dialog.server_url);
            app.fetch_providers(server);
        }
    }
    if matches!(app.state.dialog, DialogState::SignIn(_)) {
        if let DialogState::SignIn(current) = &mut app.state.dialog {
            **current = dialog;
        }
    }
}

fn normalize_server(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

fn error_block(ui: &mut Ui, message: &str) {
    let t = current_tokens(ui);
    egui::Frame::NONE
        .fill(crate::ui::common::linear_alpha(t.danger, 0.10))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!("▌! {message}"))
                    .size(12.0)
                    .color(t.text),
            );
        });
}

// --- 3.2 Share -------------------------------------------------------------------------

fn share(app: &mut RestrustApp, ctx: &egui::Context, mut dialog: crate::state::ShareDialog) {
    let t = current_tokens_ctx(ctx);
    let response = Modal::new(egui::Id::unique("share_dialog"))
        .area(egui::Area::new(egui::Id::unique("share_dialog")).fixed_pos(centered(ctx, 520.0)))
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(520.0);
            heading(ui, &format!("Share “{}”", dialog.name));
            ui.add_space(theme::GAP_STACK);

            section_label(ui, "People with access");
            let mut remove: Option<usize> = None;
            let grant_snapshot = dialog.grants.clone();
            for (index, grant) in grant_snapshot.iter().enumerate() {
                ui.horizontal(|ui| {
                    match &grant.subject {
                        ShareSubject::User { username, .. } => {
                            let initials: String = username
                                .split_whitespace()
                                .map(|w| {
                                    w.chars()
                                        .next()
                                        .map(|c| c.to_ascii_uppercase())
                                        .unwrap_or_default()
                                })
                                .take(2)
                                .collect();
                            crate::ui::common::badge(ui, &initials, t.muted);
                            ui.label(username.clone());
                            muted(ui, "User");
                        }
                        ShareSubject::Organization { name, .. } => {
                            crate::ui::common::badge(ui, "O", t.muted);
                            ui.label(name.clone());
                            muted(ui, "Organization");
                        }
                        ShareSubject::Public => {
                            crate::ui::common::badge(ui, "P", t.muted);
                            ui.label("Public");
                            muted(ui, "Public");
                        }
                    }
                    let mut level = grant.level;
                    egui::ComboBox::from_id_salt(format!("grant_level_{index}"))
                        .selected_text(match level {
                            AccessLevel::ReadOnly => "Read only",
                            AccessLevel::FullAccess => "Full Access",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut level, AccessLevel::ReadOnly, "Read only");
                            ui.selectable_value(&mut level, AccessLevel::FullAccess, "Full Access");
                        });
                    if let Some(grant) = dialog.grants.get_mut(index) {
                        grant.level = level;
                    }
                    if ui.button("✕").clicked() {
                        remove = Some(index);
                    }
                });
            }
            if let Some(index) = remove {
                dialog.grants.remove(index);
            }
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut dialog.add_username)
                        .hint_text("Add user…")
                        .desired_width(160.0),
                );
                let mut add_level = dialog.add_level;
                egui::ComboBox::from_id_salt("add_level")
                    .selected_text(match add_level {
                        AccessLevel::ReadOnly => "Read only",
                        AccessLevel::FullAccess => "Full Access",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut add_level, AccessLevel::ReadOnly, "Read only");
                        ui.selectable_value(&mut add_level, AccessLevel::FullAccess, "Full Access");
                    });
                dialog.add_level = add_level;
                if ui.button("Add").clicked() {
                    let name = dialog.add_username.trim().to_string();
                    if !name.is_empty() {
                        dialog.grants.push(ShareGrant {
                            subject: ShareSubject::User {
                                id: String::new(),
                                username: name,
                            },
                            level: dialog.add_level,
                        });
                        dialog.add_username.clear();
                        dialog.error = None;
                    }
                }
            });
            ui.add_space(theme::GAP_STACK);

            if !app.state.login.organizations.is_empty() {
                section_label(ui, "Organization");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut dialog.org_wide, "Share with entire organization");
                    if dialog.org_wide && app.state.login.organizations.len() > 1 {
                        egui::ComboBox::from_id_salt("org_choice")
                            .selected_text(
                                app.state
                                    .login
                                    .organizations
                                    .iter()
                                    .find(|o| Some(&o.id) == dialog.org_choice.as_ref())
                                    .map(|o| o.name.clone())
                                    .unwrap_or_default(),
                            )
                            .show_ui(ui, |ui| {
                                for org in app.state.login.organizations.clone() {
                                    let selected = dialog.org_choice == Some(org.id.clone());
                                    if ui.selectable_label(selected, &org.name).clicked() {
                                        dialog.org_choice = Some(org.id);
                                    }
                                }
                            });
                    }
                });
                ui.add_space(theme::GAP_STACK);
            }

            section_label(ui, "Public");
            ui.checkbox(&mut dialog.public_on, "Publish to web (Public)");
            if dialog.public_on {
                ui.horizontal(|ui| {
                    ui.label("URL");
                    let host = RestrustApp::host_of(&app.server_url());
                    ui.add(
                        egui::TextEdit::singleline(&mut dialog.slug)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(ui.available_width() - 120.0),
                    );
                    let _ = host;
                });
                if let Some(slug) = &dialog.live_slug {
                    let url = crate::api::public_url(&app.server_url(), slug);
                    ui.horizontal(|ui| {
                        muted(ui, &format!("Published · {url}"));
                        if busy_button(ui, "Unpublish", "Working…", dialog.publishing).clicked() {
                            dialog.publishing = true;
                            app.unpublish(
                                &dialog.local_collection_id,
                                &dialog.server_collection_id,
                                slug.clone(),
                            );
                        }
                    });
                } else if ui.button("Publish").clicked() {
                    dialog.publishing = true;
                    let slug = {
                        let s = dialog.slug.trim().to_string();
                        if s.is_empty() {
                            None
                        } else {
                            Some(s)
                        }
                    };
                    app.publish(
                        &dialog.local_collection_id,
                        &dialog.server_collection_id,
                        slug,
                    );
                }
            }
            ui.add_space(theme::GAP_STACK);

            if let Some(error) = &dialog.error {
                error_block(ui, error);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if busy_button(ui, "Save shares", "Saving…", dialog.saving).clicked() {
                    let mut grants = dialog.grants.clone();
                    if dialog.org_wide {
                        if let Some(org) = app
                            .state
                            .login
                            .organizations
                            .iter()
                            .find(|o| Some(&o.id) == dialog.org_choice.as_ref())
                        {
                            grants.push(ShareGrant {
                                subject: ShareSubject::Organization {
                                    id: org.id.clone(),
                                    name: org.name.clone(),
                                },
                                level: AccessLevel::ReadOnly,
                            });
                        }
                    }
                    if dialog.public_on {
                        grants.push(ShareGrant {
                            subject: ShareSubject::Public,
                            level: AccessLevel::ReadOnly,
                        });
                    }
                    dialog.saving = true;
                    dialog.error = None;
                    app.shares_save(
                        &dialog.local_collection_id,
                        &dialog.server_collection_id,
                        grants,
                    );
                }
                if ui.button("Cancel").clicked() && !dialog.saving && !dialog.publishing {
                    app.state.dialog = DialogState::None;
                }
            });
        });
    if response.should_close() && !dialog.saving && !dialog.publishing {
        app.state.dialog = DialogState::None;
        return;
    }
    if matches!(app.state.dialog, DialogState::Share(_)) {
        if let DialogState::Share(current) = &mut app.state.dialog {
            **current = dialog;
        }
    }
}

// --- 3.3 Import ------------------------------------------------------------------------

fn import(app: &mut RestrustApp, ctx: &egui::Context, mut dialog: crate::state::ImportDialog) {
    let t = current_tokens_ctx(ctx);
    let response = Modal::new(egui::Id::unique("import_dialog"))
        .area(egui::Area::new(egui::Id::unique("import_dialog")).fixed_pos(centered(ctx, 640.0)))
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(640.0);
            heading(ui, "Import");
            ui.add_space(theme::GAP_STACK);
            if let Some(done) = &mut dialog.done {
                // Post-import report view.
                if done.collections > 0 || done.environments > 0 {
                    ui.label(RichText::new(format!(
                        "Imported {} collection{}, {} environment{}.",
                        done.collections,
                        if done.collections == 1 { "" } else { "s" },
                        done.environments,
                        if done.environments == 1 { "" } else { "s" }
                    )));
                }
                if let Some(destination) = &done.request_destination {
                    ui.label(RichText::new(format!(
                        "Imported 1 request into “{destination}”."
                    )));
                }
                ui.add_space(theme::GAP_STACK);
                let sections = [
                    ("Preserved", &done.preserved, 0),
                    ("Lossy", &done.lossy, 1),
                    ("Unsupported", &done.unsupported, 2),
                ];
                for (label, items, slot) in sections {
                    if items.is_empty() {
                        continue;
                    }
                    let open = done.expanded[slot];
                    if ui
                        .button(format!("{} {} ({})", if open { "▾" } else { "▶" }, label, items.len()))
                        .clicked()
                    {
                        done.expanded[slot] = !open;
                    }
                    if open {
                        for item in items {
                            ui.label(RichText::new(format!("• {item}")).size(12.0));
                        }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Done").clicked() {
                        app.state.dialog = DialogState::None;
                    }
                });
                return;
            }
            muted(
                ui,
                "Paste Postman v2.1 / v3.0 JSON, OpenAPI 3.0 / 3.1 YAML or JSON, or a cURL command.",
            );
            ui.add_space(theme::GAP_TIGHT);
            ui.add(
                egui::TextEdit::multiline(&mut dialog.text)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(10)
                    .desired_width(ui.available_width()),
            );
            ui.add_space(theme::GAP_TIGHT);
            let format = RestrustApp::detect_import_format(&dialog.text);
            match format {
                ImportFormat::Unknown if dialog.text.trim().is_empty() => {
                    ui.label(RichText::new("Detected: ?").size(12.0));
                }
                ImportFormat::Unknown => {
                    ui.label(
                        RichText::new(format!("Detected: {}", format.label()))
                            .size(12.0)
                            .color(t.danger),
                    );
                }
                _ => {
                    crate::ui::common::badge(ui, format.label(), t.success);
                }
            }
            ui.add_space(theme::GAP_TIGHT);
            ui.horizontal(|ui| {
                ui.label("Import into:");
                let mut destination = dialog.destination.clone();
                egui::ComboBox::from_id_salt("import_destination")
                    .selected_text(match &destination {
                        Some(id) => app
                            .state
                            .find_stored(id)
                            .map(|s| s.collection.name.clone())
                            .unwrap_or_else(|| "New collection".to_string()),
                        None => "New collection".to_string(),
                    })
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(destination.is_none(), "New collection").clicked() {
                            destination = None;
                        }
                        for stored in app.state.doc.personal.iter().chain(app.state.doc.organizations.iter()) {
                            let id = stored.collection.id.clone();
                            let name = stored.collection.name.clone();
                            if ui
                                .selectable_label(destination == Some(id.clone()), name)
                                .clicked()
                            {
                                destination = Some(id);
                            }
                        }
                    });
                dialog.destination = destination;
            });
            if let Some(error) = &dialog.error {
                ui.add_space(theme::GAP_TIGHT);
                error_block(ui, error);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if primary_button(ui, "Import", 96.0).clicked() {
                    match app.run_import(&dialog.text.clone(), dialog.destination.clone()) {
                        Ok(done) => dialog.done = Some(done),
                        Err(err) => dialog.error = Some(err),
                    }
                }
                if ui.button("Cancel").clicked() {
                    app.state.dialog = DialogState::None;
                }
            });
        });
    if response.should_close() {
        app.state.dialog = DialogState::None;
        return;
    }
    if matches!(app.state.dialog, DialogState::Import(_)) {
        if let DialogState::Import(current) = &mut app.state.dialog {
            **current = dialog;
        }
    }
}

// --- 3.4 Export ------------------------------------------------------------------------

fn export(app: &mut RestrustApp, ctx: &egui::Context, mut dialog: crate::state::ExportDialog) {
    let t = current_tokens_ctx(ctx);
    let is_response_save = dialog.collection_id.is_empty();
    let response = Modal::new(egui::Id::unique("export_dialog"))
        .area(egui::Area::new(egui::Id::unique("export_dialog")).fixed_pos(centered(ctx, 640.0)))
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(640.0);
            let title = if is_response_save {
                "Save response".to_string()
            } else {
                format!("Export “{}”", dialog.name)
            };
            heading(ui, &title);
            ui.add_space(theme::GAP_STACK);
            if !is_response_save {
                ui.horizontal(|ui| {
                    for (format, label) in [
                        (ExportFormat::Postman21, "Postman v2.1"),
                        (ExportFormat::OpenApi30, "OpenAPI 3.0"),
                        (ExportFormat::RestrustJson, "restrust JSON"),
                    ] {
                        if ui
                            .radio(dialog.format == format, label)
                            .clicked()
                        {
                            dialog.format = format;
                            if let Some(stored) = app.state.find_stored(&dialog.collection_id) {
                                match app.generate_export(&stored.collection, format) {
                                    Ok(output) => dialog.output = output,
                                    Err(err) => dialog.error = Some(err),
                                }
                            }
                        }
                    }
                });
                if dialog.format == ExportFormat::OpenApi30 {
                    muted(
                        ui,
                        "OpenAPI export is lossy: scripts, auth details and folder nesting are reduced.",
                    );
                }
                ui.add_space(theme::GAP_TIGHT);
            }
            ui.add(
                egui::TextEdit::multiline(&mut dialog.output)
                    .font(egui::TextStyle::Monospace)
                    .interactive(false)
                    .desired_rows(12)
                    .desired_width(ui.available_width()),
            );
            ui.add_space(theme::GAP_TIGHT);
            ui.horizontal(|ui| {
                if ui.button("Copy").clicked() {
                    app.services.clipboard.set_text(&dialog.output);
                    app.state.push_toast("Copied".to_string());
                }
                if !IS_WASM && ui.button("Write file").clicked() {
                    match std::fs::write(&dialog.path, &dialog.output) {
                        Ok(()) => {
                            app.state.push_toast("Saved".to_string());
                        }
                        Err(err) => {
                            dialog.error = Some(format!(
                                "Could not write {}: {err}. Choose another location and try again.",
                                dialog.path
                            ));
                        }
                    }
                }
            });
            if !IS_WASM {
                ui.horizontal(|ui| {
                    ui.label("File path");
                    mono_field(ui, &mut dialog.path, "choose a path");
                });
            }
            if let Some(error) = &dialog.error {
                ui.add_space(theme::GAP_TIGHT);
                error_block(ui, error);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Done").clicked() {
                    app.state.dialog = DialogState::None;
                }
            });
        });
    if response.should_close() {
        app.state.dialog = DialogState::None;
        return;
    }
    if matches!(app.state.dialog, DialogState::Export(_)) {
        if let DialogState::Export(current) = &mut app.state.dialog {
            **current = dialog;
        }
    }
}

// --- 3.6 Settings ----------------------------------------------------------------------

fn settings(app: &mut RestrustApp, ctx: &egui::Context) {
    let t = current_tokens_ctx(ctx);
    let mut close = false;
    let mut theme_pick = app.state.doc.settings.theme.clone();
    let mut server_url = app
        .state
        .doc
        .settings
        .server_url
        .clone()
        .unwrap_or_default();
    Modal::new(egui::Id::unique("settings_dialog"))
        .area(egui::Area::new(egui::Id::unique("settings_dialog")).fixed_pos(centered(ctx, 480.0)))
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(480.0);
            heading(ui, "Settings");
            ui.add_space(theme::GAP_SECTION);

            section_label(ui, "Appearance");
            ui.horizontal(|ui| {
                let _ = ui.radio(theme_pick == "light", "Light");
                let _ = ui.radio(theme_pick == "dark", "Dark");
            });
            ui.add_space(theme::GAP_SECTION);

            section_label(ui, "Server");
            ui.label("Server URL");
            ui.text_edit_singleline(&mut server_url);
            if app.state.login.signed_in() {
                let username = app
                    .state
                    .login
                    .user
                    .as_ref()
                    .map(|u| u.username.clone())
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    muted(ui, &format!("Signed in as {username}"));
                    if ui.button("Sign out").clicked() {
                        app.logout();
                    }
                });
            } else {
                muted(ui, "Not signed in — sync and sharing are optional.");
            }
            ui.add_space(theme::GAP_SECTION);

            section_label(ui, "Data");
            ui.horizontal(|ui| {
                ui.label("Storage");
                ui.label(app.services.storage_description());
            });
            muted(
                ui,
                "Changes save automatically shortly after you stop typing, on Ctrl+S, and on exit.",
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Done").clicked() {
                    close = true;
                }
            });
        });
    if close {
        app.state.dialog = DialogState::None;
    }
    if theme_pick != "dark" && theme_pick != "light" {
        theme_pick = "dark".to_string();
    }
    let trimmed_server = server_url.trim().to_string();
    if app.state.doc.settings.theme != theme_pick
        || app.state.doc.settings.server_url.as_deref() != Some(trimmed_server.as_str())
    {
        app.state.doc.settings.theme = theme_pick.clone();
        app.state.doc.settings.server_url = if trimmed_server.is_empty() {
            None
        } else {
            Some(trimmed_server)
        };
        app.state.mark_edited();
    }
}

// --- 3.7 Shortcuts ---------------------------------------------------------------------

fn shortcuts(app: &mut RestrustApp, ctx: &egui::Context) {
    let t = current_tokens_ctx(ctx);
    let mut close = false;
    Modal::new(egui::Id::unique("shortcuts_dialog"))
        .area(egui::Area::new(egui::Id::unique("shortcuts_dialog")).fixed_pos(centered(ctx, 460.0)))
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(460.0);
            heading(ui, "Keyboard shortcuts");
            ui.add_space(theme::GAP_STACK);
            egui::Grid::new("shortcuts_grid")
                .num_columns(3)
                .striped(true)
                .spacing([theme::GAP_INNER, 4.0])
                .show(ui, |ui| {
                    for (name, win, mac) in [
                        ("Send request", "Ctrl+Enter", "Cmd+Enter"),
                        ("Save workspace", "Ctrl+S", "Cmd+S"),
                        ("New request tab", "Ctrl+T", "Cmd+T"),
                        ("Close tab", "Ctrl+W", "Cmd+W"),
                        ("Focus search", "Ctrl+F", "Cmd+F"),
                        ("Toggle sidebar", "Ctrl+B", "Cmd+B"),
                        ("Settings", "Ctrl+,", "Cmd+,"),
                        ("Keyboard shortcuts modal", "Ctrl+/", "Cmd+/"),
                        ("Rename selected tree node", "F2", "F2"),
                        ("Open selected tree node", "Enter", "Enter"),
                        ("Close dialog / menu / overlay", "Esc", "Esc"),
                    ] {
                        ui.label(RichText::new(name).size(12.0));
                        ui.label(RichText::new(win).size(12.0).monospace());
                        ui.label(RichText::new(mac).size(12.0).monospace());
                        ui.end_row();
                    }
                });
            if IS_WASM {
                ui.add_space(theme::GAP_TIGHT);
                muted(ui, "In the browser, Alt+T / Alt+W replace Ctrl+T / Ctrl+W.");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
    if close {
        app.state.dialog = DialogState::None;
    }
}

// --- 3.9 About -------------------------------------------------------------------------

fn about(app: &mut RestrustApp, ctx: &egui::Context) {
    let t = current_tokens_ctx(ctx);
    let mut close = false;
    let server_line = if app.state.login.signed_in() {
        format!("Server: {} · connected", app.server_url())
    } else {
        String::new()
    };
    Modal::new(egui::Id::unique("about_dialog"))
        .area(egui::Area::new(egui::Id::unique("about_dialog")).fixed_pos(centered(ctx, 360.0)))
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(360.0);
            ui.vertical_centered(|ui| {
                ui.add_space(theme::GAP_TIGHT);
                ui.label(RichText::new("▲").size(28.0));
                ui.label(RichText::new("restrust").size(16.0).strong());
                ui.add_space(theme::GAP_TIGHT);
                muted(
                    ui,
                    &format!(
                        "Version {} (egui {})",
                        env!("CARGO_PKG_VERSION"),
                        crate::ui::EGUI_VERSION
                    ),
                );
                muted(ui, "A local-first API client for REST development.");
                if !server_line.is_empty() {
                    muted(ui, &server_line);
                }
                muted(ui, "UI language: English");
                ui.add_space(theme::GAP_TIGHT);
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
    if close {
        app.state.dialog = DialogState::None;
    }
}

// --- environments (§2.1) ------------------------------------------------------------------

fn new_collection(app: &mut RestrustApp, ctx: &egui::Context, initial_name: &str) {
    let t = current_tokens_ctx(ctx);
    let mut close = false;
    let mut name = initial_name.to_string();
    Modal::new(egui::Id::unique("new_collection_dialog"))
        .area(
            egui::Area::new(egui::Id::unique("new_collection_dialog"))
                .fixed_pos(centered(ctx, 380.0)),
        )
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(380.0);
            heading(ui, "New Collection");
            ui.add_space(theme::GAP_STACK);
            ui.label("Name");
            ui.text_edit_singleline(&mut name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if primary_button(ui, "Create", 72.0).clicked() && !name.trim().is_empty() {
                    let stored = restrust_core::workspace_doc::StoredCollection {
                        collection: Collection::new(name.trim().to_string()),
                        sync: SyncState::Local,
                        origin: None,
                        access: None,
                    };
                    app.state.doc.personal.push(stored);
                    app.state.mark_edited();
                    close = true;
                }
            });
        });
    if close {
        app.state.dialog = DialogState::None;
        return;
    }
    if matches!(app.state.dialog, DialogState::NewCollection { .. }) {
        if let DialogState::NewCollection { name: current } = &mut app.state.dialog {
            *current = name;
        }
    }
}

fn new_environment(
    app: &mut RestrustApp,
    ctx: &egui::Context,
    dialog: crate::state::NewEnvironmentDialog,
) {
    let t = current_tokens_ctx(ctx);
    let mut d = dialog;
    let mut close = false;
    Modal::new(egui::Id::unique("new_environment_dialog"))
        .area(
            egui::Area::new(egui::Id::unique("new_environment_dialog"))
                .fixed_pos(centered(ctx, 460.0)),
        )
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(460.0);
            heading(ui, "New environment");
            ui.add_space(theme::GAP_STACK);
            ui.label("Name");
            ui.text_edit_singleline(&mut d.name);
            ui.add_space(theme::GAP_TIGHT);
            kv_editor(ui, "new_env_values", &mut d.values, false, &mut || {});
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if primary_button(ui, "Create", 72.0).clicked() && !d.name.trim().is_empty() {
                    let env = Environment {
                        id: restrust_core::model::new_id(),
                        name: d.name.trim().to_string(),
                        values: d.values.clone(),
                    };
                    app.state.doc.environments.push(env);
                    app.state.mark_edited();
                    close = true;
                }
            });
        });
    if close {
        app.state.dialog = DialogState::None;
        return;
    }
    if matches!(app.state.dialog, DialogState::NewEnvironment(_)) {
        if let DialogState::NewEnvironment(current) = &mut app.state.dialog {
            **current = d;
        }
    }
}

fn manage_environments(app: &mut RestrustApp, ctx: &egui::Context) {
    let t = current_tokens_ctx(ctx);
    let mut close = false;
    let names: Vec<String> = app
        .state
        .doc
        .environments
        .iter()
        .map(|e| e.name.clone())
        .collect();
    let active = app.state.doc.settings.active_environment.clone();
    Modal::new(egui::Id::unique("manage_environments_dialog"))
        .area(
            egui::Area::new(egui::Id::unique("manage_environments_dialog"))
                .fixed_pos(centered(ctx, 520.0)),
        )
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(520.0);
            heading(ui, "Manage environments");
            ui.add_space(theme::GAP_STACK);
            if names.is_empty() {
                muted(ui, "No environments yet.");
            }
            let mut deleted: Option<String> = None;
            for name in &names {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).strong());
                    if Some(name.clone()) == active {
                        crate::ui::common::badge(ui, "active", t.success);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Delete").clicked() {
                            deleted = Some(name.clone());
                        }
                        if ui.button("Duplicate").clicked() {
                            if let Some(env) = app
                                .state
                                .doc
                                .environments
                                .iter()
                                .find(|e| &e.name == name)
                                .cloned()
                            {
                                let copy = Environment {
                                    id: restrust_core::model::new_id(),
                                    name: format!("{} (copy)", env.name),
                                    values: env.values,
                                };
                                app.state.doc.environments.push(copy);
                                app.state.mark_edited();
                            }
                        }
                        if ui.button("Rename…").clicked() {
                            if let Some(env) = app
                                .state
                                .doc
                                .environments
                                .iter_mut()
                                .find(|e| &e.name == name)
                            {
                                let mut buffer = env.name.clone();
                                let resp = ui.text_edit_singleline(&mut buffer);
                                if resp.changed() {
                                    env.name = buffer;
                                    app.state.mark_edited();
                                }
                            }
                        }
                    });
                });
                if let Some(env) = app
                    .state
                    .doc
                    .environments
                    .iter_mut()
                    .find(|e| &e.name == name)
                {
                    let mut edited = false;
                    let mut values = std::mem::take(&mut env.values);
                    kv_editor(
                        ui,
                        &format!("env_{}", env.id),
                        &mut values,
                        false,
                        &mut || edited = true,
                    );
                    env.values = values;
                    if edited {
                        app.state.mark_edited();
                    }
                }
            }
            if let Some(name) = deleted {
                app.state.doc.environments.retain(|e| e.name != name);
                if app.state.doc.settings.active_environment.as_deref() == Some(&name) {
                    app.state.doc.settings.active_environment = None;
                }
                app.state.mark_edited();
            }
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Done").clicked() {
                    close = true;
                }
            });
        });
    if close {
        app.state.dialog = DialogState::None;
    }
}

// --- 3.5 confirmations ---------------------------------------------------------------------

fn confirm_delete(
    app: &mut RestrustApp,
    ctx: &egui::Context,
    collection_id: &str,
    name: &str,
    requests: usize,
    on_server: bool,
    _busy: bool,
) {
    let t = current_tokens_ctx(ctx);
    let mut close = false;
    let mut do_delete = false;
    Modal::new(egui::Id::unique("confirm_delete_dialog"))
        .area(egui::Area::new(egui::Id::unique("confirm_delete_dialog")).fixed_pos(centered(ctx, 420.0)))
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(420.0);
            heading(ui, "Delete collection?");
            ui.add_space(theme::GAP_STACK);
            if on_server {
                ui.label(format!(
                    "This deletes “{name}” on the server and for everyone it is shared with."
                ));
            } else {
                ui.label(format!(
                    "Delete “{name}” and its {requests} requests from this device? This cannot be undone."
                ));
            }
            ui.add_space(theme::GAP_STACK);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if on_server {
                    if danger_button(ui, "Delete everywhere").clicked() {
                        do_delete = true;
                    }
                } else if danger_button(ui, "Delete").clicked() {
                    do_delete = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if do_delete {
        app.state.dialog = DialogState::None;
        app.delete_collection(collection_id);
        return;
    }
    if close {
        app.state.dialog = DialogState::None;
    }
}

fn confirm_remove_local(
    app: &mut RestrustApp,
    ctx: &egui::Context,
    collection_id: &str,
    name: &str,
) {
    let t = current_tokens_ctx(ctx);
    let mut close = false;
    let mut do_remove = false;
    Modal::new(egui::Id::unique("confirm_remove_local_dialog"))
        .area(egui::Area::new(egui::Id::unique("confirm_remove_local_dialog")).fixed_pos(centered(ctx, 420.0)))
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(420.0);
            heading(ui, "Delete local data?");
            ui.add_space(theme::GAP_STACK);
            ui.label(format!(
                "Remove the retained local copy of “{name}”? Access was revoked, so this data exists only on this device. This cannot be undone."
            ));
            ui.add_space(theme::GAP_STACK);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if danger_button(ui, "Delete local data").clicked() {
                    do_remove = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if do_remove {
        app.remove_collection_local(collection_id);
        app.state.push_toast("Removed local data".to_string());
        app.normalize_revoked_pill();
        app.state.dialog = DialogState::None;
        return;
    }
    if close {
        app.state.dialog = DialogState::None;
    }
}

fn conflict(
    app: &mut RestrustApp,
    ctx: &egui::Context,
    collection_id: &str,
    name: &str,
    server_revision: u64,
) {
    let t = current_tokens_ctx(ctx);
    let mut overwrite = false;
    let mut keep = false;
    Modal::new(egui::Id::unique("conflict_dialog"))
        .area(egui::Area::new(egui::Id::unique("conflict_dialog")).fixed_pos(centered(ctx, 460.0)))
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(460.0);
            heading(ui, "Sync conflict");
            ui.add_space(theme::GAP_STACK);
            ui.label(format!(
                "The server copy of “{name}” was changed by someone else (server revision {server_revision}). Choose which version to keep."
            ));
            ui.add_space(theme::GAP_STACK);
            ui.horizontal(|ui| {
                if primary_button(ui, "Overwrite server copy", 0.0).clicked() {
                    overwrite = true;
                }
                if ui.button("Keep my local copy").clicked() {
                    keep = true;
                }
            });
            ui.add_space(theme::GAP_TIGHT);
            muted(
                ui,
                "Your edits stay local and unsynced. Resolve later by exporting or overwriting.",
            );
        });
    if overwrite {
        app.overwrite_server_copy(collection_id, server_revision);
        return;
    }
    if keep {
        app.keep_local_copy();
    }
}

// --- save scratch into (§2.5 Settings) ---------------------------------------------------------

fn save_scratch_into(
    app: &mut RestrustApp,
    ctx: &egui::Context,
    tab_id: u64,
    collection_id: Option<String>,
    parent_folder: Option<String>,
) {
    let t = current_tokens_ctx(ctx);
    let mut collection_id = collection_id;
    let mut parent_folder = parent_folder;
    let mut close = false;
    let mut do_save = false;
    let targets = crate::state::destination_targets(&app.state.doc);
    Modal::new(egui::Id::unique("save_into_dialog"))
        .area(egui::Area::new(egui::Id::unique("save_into_dialog")).fixed_pos(centered(ctx, 420.0)))
        .frame(dialog_frame(&t))
        .show(ctx, |ui| {
            ui.set_width(420.0);
            heading(ui, "Save into…");
            ui.add_space(theme::GAP_STACK);
            egui::ComboBox::from_id_salt("save_into_collection")
                .selected_text(match &collection_id {
                    Some(id) => targets
                        .iter()
                        .find(|(_, cid, _)| cid == id)
                        .map(|(label, _, _)| label.clone())
                        .unwrap_or_else(|| "Choose a collection".to_string()),
                    None => "Choose a collection".to_string(),
                })
                .show_ui(ui, |ui| {
                    for (label, cid, _) in &targets {
                        if ui
                            .selectable_label(collection_id == Some(cid.clone()), label)
                            .clicked()
                        {
                            collection_id = Some(cid.clone());
                            parent_folder = None;
                        }
                    }
                });
            // Folder dropdown scoped to the chosen collection.
            let folder_targets: Vec<(String, Option<String>)> = targets
                .iter()
                .filter(|(_, cid, _)| Some(cid) == collection_id.as_ref())
                .map(|(label, _, folder)| (label.clone(), folder.clone()))
                .collect();
            if !folder_targets.is_empty() {
                egui::ComboBox::from_id_salt("save_into_folder")
                    .selected_text(match &parent_folder {
                        Some(_) => folder_targets
                            .iter()
                            .find(|(_, f)| *f == parent_folder)
                            .map(|(label, _)| label.clone())
                            .unwrap_or_else(|| "(collection root)".to_string()),
                        None => "(collection root)".to_string(),
                    })
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(parent_folder.is_none(), "(collection root)")
                            .clicked()
                        {
                            parent_folder = None;
                        }
                        for (label, folder) in &folder_targets {
                            if ui
                                .selectable_label(parent_folder == *folder, label)
                                .clicked()
                            {
                                parent_folder = folder.clone();
                            }
                        }
                    });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if primary_button(ui, "Save", 72.0).clicked() && collection_id.is_some() {
                    do_save = true;
                }
            });
        });
    if do_save {
        if let Some(cid) = collection_id.clone() {
            let scratch = app
                .state
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .and_then(|tab| match &tab.kind {
                    crate::state::TabKind::Scratch(def) => Some((**def).clone()),
                    _ => None,
                });
            if let Some(def) = scratch {
                let request_id = def.id.clone();
                if let Some(stored) = app.state.stored_mut(&cid) {
                    stored.collection.insert_item(
                        parent_folder.as_deref(),
                        restrust_core::model::CollectionItem::Request(def),
                    );
                    crate::sync::mark_edited(stored);
                }
                app.state.mark_edited();
                if let Some(tab) = app.state.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                    tab.kind = crate::state::TabKind::Saved {
                        collection_id: cid,
                        request_id,
                    };
                }
            }
        }
        app.state.dialog = DialogState::None;
        return;
    }
    if close {
        app.state.dialog = DialogState::None;
        return;
    }
    if let DialogState::SaveScratchInto {
        tab_id: t,
        collection_id: c,
        parent_folder: f,
    } = &mut app.state.dialog
    {
        *t = tab_id;
        *c = collection_id;
        *f = parent_folder;
    }
}
