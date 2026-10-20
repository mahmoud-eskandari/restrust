//! Top bar (ui-spec §2.1): sidebar toggle, app menu, environment selector,
//! sync status pill, Administration menu (role-gated), Settings, Help, account.

use eframe::egui::{self, Context, RichText, Ui};

use crate::app::RestrustApp;
use crate::state::{DialogState, NamespaceSelection, ServerStatus};
use crate::theme;
use crate::ui::common::{current_tokens, ghost_toggle, linear_alpha, muted};

use restrust_core::workspace_doc::SyncState;

pub fn draw(app: &mut RestrustApp, root: &mut egui::Ui) {
    let t = current_tokens_ctx(root.ctx());
    egui::Panel::top("top_bar")
        .exact_size(theme::TOP_BAR_HEIGHT)
        .frame(egui::Frame::NONE.fill(t.panel))
        .show(root, |ui| {
            ui.horizontal_centered(|ui| {
                ui.set_height(theme::TOP_BAR_HEIGHT);
                ui.add_space(theme::GAP_INNER);
                sidebar_toggle(app, ui);
                app_menu(app, ui);
                environment_selector(app, ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    account_button(app, ui);
                    help_menu(app, ui);
                    settings_button(app, ui);
                    administration_menu(app, ui);
                    sync_pill(app, ui);
                });
            });
        });
}

fn current_tokens_ctx(ctx: &Context) -> crate::theme::ThemeTokens {
    if ctx.theme() == egui::Theme::Dark {
        theme::DARK
    } else {
        theme::LIGHT
    }
}

fn sidebar_toggle(app: &mut RestrustApp, ui: &mut Ui) {
    if ghost_toggle(ui, "☰", app.state.sidebar_visible, "Toggle sidebar  Ctrl+B").clicked() {
        app.state.sidebar_visible = !app.state.sidebar_visible;
        app.state.sidebar_overlay = false;
    }
}

/// Logo mark + wordmark; the wordmark hides under 600 px (§1.4).
fn app_menu(app: &mut RestrustApp, ui: &mut Ui) {
    let narrow = ui.ctx().content_rect().width() < 600.0;
    let label = if narrow {
        "▲".to_string()
    } else {
        "▲ restrust".to_string()
    };
    ui.menu_button(RichText::new(label).strong(), |ui| {
        ui.set_min_width(190.0);
        if ui.button("New Request\tCtrl+T").clicked() {
            app.state.new_tab();
        }
        if ui.button("New Collection").clicked() {
            app.state.dialog = DialogState::NewCollection {
                name: String::new(),
            };
        }
        if ui.button("New Environment…").clicked() {
            app.state.dialog = DialogState::NewEnvironment(Box::default());
        }
        ui.separator();
        if ui.button("Import…").clicked() {
            app.state.dialog = DialogState::Import(Box::new(crate::state::ImportDialog::new(None)));
        }
        let selected_collection = app.state.selected.as_ref().map(|s| s.collection_id.clone());
        let has_selection = selected_collection.is_some();
        if ui
            .add_enabled(has_selection, egui::Button::new("Export…"))
            .on_disabled_hover_text("Select a collection first")
            .clicked()
        {
            if let Some(collection_id) = selected_collection {
                crate::ui::dialogs::open_export(app, &collection_id);
            }
        }
        ui.separator();
        if ui.button("Save Workspace\tCtrl+S").clicked() {
            app.save_now();
        }
        let can_sync = app.state.login.signed_in();
        if ui
            .add_enabled(can_sync, egui::Button::new("Sync Now"))
            .on_disabled_hover_text("Sign in to sync")
            .clicked()
        {
            app.sync_now();
        }
        ui.separator();
        if ui.button("Settings…\tCtrl+,").clicked() {
            app.state.dialog = DialogState::Settings;
        }
        if !crate::platform::IS_WASM && ui.button("Quit").clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    });
}

/// Environment selector (§2.1): active environment or `No environment`.
fn environment_selector(app: &mut RestrustApp, ui: &mut Ui) {
    ui.add_space(theme::GAP_STACK);
    let active = app
        .state
        .doc
        .settings
        .active_environment
        .clone()
        .unwrap_or_default();
    let label = if active.is_empty() {
        "No environment ▾".to_string()
    } else {
        format!("{active} ▾")
    };
    ui.menu_button(label, |ui| {
        ui.set_min_width(190.0);
        if ui
            .button(format!("{} No environment", check(&active, "")))
            .clicked()
        {
            app.state.doc.settings.active_environment = None;
            app.state.mark_edited();
        }
        for env in app.state.doc.environments.clone() {
            if ui
                .button(format!("{} {}", check(&active, &env.name), env.name))
                .clicked()
            {
                app.state.doc.settings.active_environment = Some(env.name.clone());
                app.state.mark_edited();
            }
        }
        ui.separator();
        if ui.button("New environment…").clicked() {
            app.state.dialog = DialogState::NewEnvironment(Box::default());
        }
        if ui.button("Manage environments…").clicked() {
            app.state.dialog = DialogState::ManageEnvironments;
        }
    });
}

fn check(active: &str, candidate: &str) -> &'static str {
    if active == candidate {
        "✓"
    } else {
        " "
    }
}

/// The sync status pill + its state-dependent dropdown (§2.1).
fn sync_pill(app: &mut RestrustApp, ui: &mut Ui) {
    let t = current_tokens(ui);
    let narrow = ui.ctx().content_rect().width() < 600.0;
    let status = app.state.login.status.clone();
    let username = app
        .state
        .login
        .user
        .as_ref()
        .map(|u| u.username.clone())
        .unwrap_or_default();
    let host = RestrustApp::host_of(&app.server_url());

    let (dot, text, tooltip) = match &status {
        ServerStatus::Offline => (
            t.muted,
            "Offline local".to_string(),
            "Work locally — nothing leaves this device".to_string(),
        ),
        ServerStatus::Connected => {
            let synced = app
                .state
                .login
                .last_synced
                .clone()
                .unwrap_or_else(|| "not synced yet".to_string());
            (
                t.success,
                format!("Connected as {username}"),
                format!("Connected to {host} · last synced {synced}"),
            )
        }
        ServerStatus::Syncing => (
            t.accent,
            "Syncing…".to_string(),
            format!("Exchanging changes with {host}"),
        ),
        ServerStatus::Failed(err) => (
            t.danger,
            "Connection failed".to_string(),
            format!("Cannot reach {host} — {err}"),
        ),
        ServerStatus::Revoked(name) => (
            t.danger,
            format!("Access revoked on {name}"),
            "Click to view the revoked collection".to_string(),
        ),
    };
    if app.state.sync_in_flight {
        ui.add(egui::Spinner::new().size(12.0));
    }
    let display = if narrow {
        // Truncate to a status dot + one word (§1.4).
        let word = match &status {
            ServerStatus::Offline => "Offline",
            ServerStatus::Connected => "Connected",
            ServerStatus::Syncing => "Syncing…",
            ServerStatus::Failed(_) => "Failed",
            ServerStatus::Revoked(_) => "Revoked",
        };
        word.to_string()
    } else {
        text
    };
    let pill = egui::Button::new(
        RichText::new(format!("● {display}"))
            .size(12.0)
            .color(t.text),
    )
    .fill(linear_alpha(dot, 0.15));
    let resp = ui.add(pill).on_hover_text(tooltip);

    let popup = egui::Popup::menu(&resp);
    let popup_resp = popup.show(|ui| {
        ui.set_min_width(260.0);
        match &status {
            ServerStatus::Offline => {
                muted(ui, "Not connected to a server");
                if ui.button("Sign in…").clicked() {
                    let server = app.state.doc.settings.server_url.clone();
                    app.state.dialog =
                        DialogState::SignIn(Box::new(crate::state::SignInDialog::new(server)));
                }
            }
            ServerStatus::Connected => {
                muted(ui, &format!("Signed in to {host}"));
                let synced = app
                    .state
                    .login
                    .last_synced
                    .clone()
                    .unwrap_or_else(|| "not yet".to_string());
                muted(ui, &format!("Last synced {synced}"));
                if ui.button("Sync now").clicked() {
                    app.sync_now();
                }
                if ui.button("Sign out").clicked() {
                    app.logout();
                }
            }
            ServerStatus::Syncing => {
                muted(ui, &format!("Exchanging changes with {host}"));
            }
            ServerStatus::Failed(err) => {
                muted(ui, &format!("Cannot reach {host}: {err}"));
                muted(
                    ui,
                    "Your edits are saved locally and will sync when the server is reachable",
                );
                if ui.button("Retry connection").clicked() {
                    app.retry_connection();
                }
                if ui.button("Sign out").clicked() {
                    app.logout();
                }
            }
            ServerStatus::Revoked(name) => {
                muted(
                    ui,
                    "Access to this collection was revoked. Synchronization is disabled.",
                );
                let _ = name;
                if ui.button("Show collection").clicked() {
                    if let Some(stored) = app
                        .state
                        .doc
                        .personal
                        .iter()
                        .chain(app.state.doc.organizations.iter())
                        .find(|s| s.sync == SyncState::Revoked)
                    {
                        let id = stored.collection.id.clone();
                        app.state.selected = Some(crate::state::SelectedNode {
                            collection_id: id,
                            node_id: None,
                        });
                        app.state.tabs.clear();
                        app.state.active_tab = None;
                    }
                }
            }
        }
    });
    if popup_resp.is_some() {
        resp.surrender_focus();
    }
}

/// Administration menu — only rendered for signed-in sysadmins (R-15/AC-14).
fn administration_menu(app: &mut RestrustApp, ui: &mut Ui) {
    if !app.state.login.is_admin() {
        return;
    }
    ui.add_space(theme::GAP_TIGHT);
    ui.menu_button("Administration ▾", |ui| {
        ui.set_min_width(160.0);
        if ui.button("Users").clicked() {
            crate::ui::admin::open_admin(app, crate::state::AdminTab::Users);
        }
        if ui.button("Organizations").clicked() {
            crate::ui::admin::open_admin(app, crate::state::AdminTab::Organizations);
        }
    });
}

fn settings_button(app: &mut RestrustApp, ui: &mut Ui) {
    if ui
        .add(egui::Button::new(RichText::new("⚙").size(15.0)))
        .on_hover_text("Settings")
        .clicked()
    {
        app.state.dialog = DialogState::Settings;
    }
}

fn help_menu(app: &mut RestrustApp, ui: &mut Ui) {
    ui.menu_button("Help ▾", |ui| {
        ui.set_min_width(210.0);
        if ui.button("Keyboard shortcuts\tCtrl+/").clicked() {
            app.state.dialog = DialogState::Shortcuts;
        }
        if ui.button("About").clicked() {
            app.state.dialog = DialogState::About;
        }
    });
}

fn account_button(app: &mut RestrustApp, ui: &mut Ui) {
    let t = current_tokens(ui);
    let Some(user) = app.state.login.user.clone() else {
        if ui.button("Sign in…").clicked() {
            let server = app.state.doc.settings.server_url.clone();
            app.state.dialog =
                DialogState::SignIn(Box::new(crate::state::SignInDialog::new(server)));
        }
        return;
    };
    ui.menu_button(format!("{} ▾", user.username), |ui| {
        ui.set_min_width(210.0);
        ui.label(RichText::new(&user.username).strong());
        if let Some(display) = user.display_name.as_ref() {
            if display != &user.username {
                muted(ui, display);
            }
        }
        let role = if user.system_role == "admin" {
            "System administrator"
        } else {
            "User"
        };
        muted(ui, &format!("Role: {role}"));
        ui.separator();
        if ui.button("Sign out").clicked() {
            app.logout();
        }
    });
    let _ = t;
    let _ = NamespaceSelection::Personal;
}
