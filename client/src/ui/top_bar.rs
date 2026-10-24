//! Top bar (ui-spec §2.1): sidebar toggle, app menu, environment selector,
//! sync status pill, Administration menu (role-gated), Settings, Help, account.

use eframe::egui::{self, Button, Context, IntoAtoms, RichText, Ui, Vec2};
use egui::containers::menu::MenuButton;

use crate::app::RestrustApp;
use crate::state::{DialogState, ServerStatus};
use crate::theme;
use crate::ui::common::{
    current_tokens, ghost_toggle, linear_alpha, menu_item, menu_item_enabled, menu_item_width,
    muted,
};

use restrust_core::workspace_doc::SyncState;

/// Every control in the bar gets this height so the row lines up (§1.3).
const BAR_CONTROL_HEIGHT: f32 = theme::HEIGHT_CONTROL;

/// A menu button sized to the bar height (plain `ui.menu_button` sizes to the
/// text, which made the row ragged).
fn bar_menu_button<'a, R>(
    ui: &mut Ui,
    label: impl IntoAtoms<'a>,
    content: impl FnOnce(&mut Ui) -> R,
) -> egui::Response {
    MenuButton::from_button(Button::new(label).min_size(Vec2::new(0.0, BAR_CONTROL_HEIGHT)))
        .ui(ui, content)
        .0
}

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
                ui.add_space(theme::GAP_TIGHT);
                app_menu(app, ui);
                if !crate::platform::IS_WASM {
                    server_selector(app, ui);
                }
                if ui.ctx().content_rect().width() >= 900.0 {
                    environment_selector(app, ui);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    account_button(app, ui);
                    if ui.ctx().content_rect().width() >= 900.0 {
                        ui.add_space(theme::GAP_TIGHT);
                        help_menu(app, ui);
                        settings_button(app, ui);
                    }
                    administration_menu(app, ui);
                    sync_pill(app, ui);
                });
            });
        });
}

fn server_selector(app: &mut RestrustApp, ui: &mut Ui) {
    let active = app.state.active_server.clone();
    let known = app.state.server_choices();
    let label = active
        .as_deref()
        .map(RestrustApp::host_of)
        .unwrap_or_else(|| "Local workspace".into());
    let label = if label.chars().count() > 24 {
        format!("{}…", label.chars().take(23).collect::<String>())
    } else {
        label
    };
    // The active server's identity color leads the button.
    let dot = active
        .as_deref()
        .map(|server| theme::server_color(server, &known))
        .unwrap_or(current_tokens(ui).muted);
    bar_menu_button(
        ui,
        (
            RichText::new("● ").size(11.0).color(dot),
            RichText::new(format!("{label} ▾")),
        ),
        |ui| {
            const W: f32 = 280.0;
            let w = menu_item_width(W);
            ui.set_min_width(W);
            muted(ui, "Servers");
            let ready = app.can_switch_server();
            if ui
                .add_enabled(
                    ready,
                    Button::new((
                        RichText::new("● ")
                            .size(11.0)
                            .color(current_tokens(ui).muted),
                        RichText::new("Local workspace"),
                    ))
                    .selected(active.is_none())
                    .min_size(Vec2::new(w, theme::HEIGHT_MENU_ITEM)),
                )
                .clicked()
            {
                app.switch_server(None);
                ui.close();
            }
            for server in app.state.server_choices() {
                let connected = if active.as_ref() == Some(&server) {
                    app.state.login.signed_in()
                } else {
                    app.state
                        .server_sessions
                        .get(&server)
                        .is_some_and(|s| s.login.signed_in())
                };
                let label = format!(
                    "{}{}",
                    RestrustApp::host_of(&server),
                    if connected { " · signed in" } else { "" }
                );
                let dot = theme::server_color(&server, &known);
                if ui
                    .add_enabled(
                        ready,
                        Button::new((
                            RichText::new("● ").size(11.0).color(dot),
                            RichText::new(label),
                        ))
                        .selected(active.as_ref() == Some(&server))
                        .min_size(Vec2::new(w, theme::HEIGHT_MENU_ITEM)),
                    )
                    .on_hover_text(&server)
                    .clicked()
                {
                    app.switch_server(Some(server.clone()));
                    if !app.state.login.signed_in() {
                        app.state.dialog = DialogState::SignIn(Box::new(
                            crate::state::SignInDialog::new(Some(server)),
                        ));
                    }
                    ui.close();
                }
            }
            ui.separator();
            if ui
                .add_enabled(
                    ready,
                    Button::new("Join another server…")
                        .min_size(Vec2::new(w, theme::HEIGHT_MENU_ITEM)),
                )
                .on_disabled_hover_text("Wait for the current server operation to finish")
                .clicked()
            {
                app.state.dialog =
                    DialogState::SignIn(Box::new(crate::state::SignInDialog::new(None)));
                ui.close();
            }
        },
    );
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
        super::toggle_sidebar(app, ui.ctx().content_rect().width());
    }
}

/// Logo mark + wordmark; the wordmark hides under 600 px (§1.4).
fn app_menu(app: &mut RestrustApp, ui: &mut Ui) {
    let narrow = ui.ctx().content_rect().width() < 600.0;
    let logo: egui::ImageSource = egui::include_image!("../../assets/logo.png");
    let label = if narrow {
        RichText::new("").strong()
    } else {
        RichText::new(" restrust").strong()
    };
    MenuButton::from_button(
        Button::image_and_text(
            egui::Image::new(logo).max_size(egui::vec2(18.0, 18.0)),
            label,
        )
        .min_size(Vec2::new(0.0, BAR_CONTROL_HEIGHT)),
    )
    .ui(ui, |ui| {
        const W: f32 = 230.0;
        let w = menu_item_width(W);
        ui.set_min_width(W);
        if menu_item(ui, "New Request", Some("Ctrl+T"), w).clicked() {
            app.state.new_tab();
        }
        if menu_item(ui, "New Collection", None, w).clicked() {
            app.state.dialog = DialogState::NewCollection {
                name: String::new(),
            };
        }
        if menu_item(ui, "New Environment…", None, w).clicked() {
            app.state.dialog = DialogState::NewEnvironment(Box::default());
        }
        ui.separator();
        if menu_item(ui, "Import…", None, w).clicked() {
            app.state.dialog = DialogState::Import(Box::new(crate::state::ImportDialog::new(None)));
        }
        let selected_collection = app.state.selected.as_ref().map(|s| s.collection_id.clone());
        if menu_item_enabled(
            ui,
            selected_collection.is_some(),
            "Export…",
            None,
            "Select a collection first",
            w,
        )
        .clicked()
        {
            if let Some(collection_id) = selected_collection {
                crate::ui::dialogs::open_export(app, &collection_id);
            }
        }
        ui.separator();
        if menu_item(ui, "Save Workspace", Some("Ctrl+S"), w).clicked() {
            app.save_and_record();
        }
        if menu_item_enabled(
            ui,
            app.state.login.signed_in(),
            "Sync Now",
            None,
            "Sign in to sync",
            w,
        )
        .clicked()
        {
            app.sync_now();
        }
        ui.separator();
        if menu_item(ui, "Settings…", Some("Ctrl+,"), w).clicked() {
            app.state.dialog = DialogState::Settings;
        }
        if !crate::platform::IS_WASM && menu_item(ui, "Quit", None, w).clicked() {
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
    bar_menu_button(ui, label, |ui| {
        const W: f32 = 220.0;
        let w = menu_item_width(W);
        ui.set_min_width(W);
        if menu_item(
            ui,
            &format!("{} No environment", check(&active, "")),
            None,
            w,
        )
        .clicked()
        {
            app.state.doc.settings.active_environment = None;
            app.state.mark_edited();
        }
        for env in app.state.doc.environments.clone() {
            if menu_item(
                ui,
                &format!("{} {}", check(&active, &env.name), env.name),
                None,
                w,
            )
            .clicked()
            {
                app.state.doc.settings.active_environment = Some(env.name.clone());
                app.state.mark_edited();
            }
        }
        ui.separator();
        if menu_item(ui, "New environment…", None, w).clicked() {
            app.state.dialog = DialogState::NewEnvironment(Box::default());
        }
        if menu_item(ui, "Manage environments…", None, w).clicked() {
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
        ui.add_space(theme::GAP_TIGHT);
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
    // When connected, the pill's dot wears the server's identity color;
    // failure states keep their status color.
    let dot_color = if matches!(&status, ServerStatus::Connected | ServerStatus::Syncing) {
        theme::server_color(&app.server_url(), &app.state.server_choices())
    } else {
        dot
    };
    let pill = Button::new((
        RichText::new("● ").size(12.0).color(dot_color),
        RichText::new(display).size(12.0).color(t.text),
    ))
    .fill(linear_alpha(dot, 0.15))
    .min_size(Vec2::new(0.0, BAR_CONTROL_HEIGHT));
    let resp = ui.add(pill).on_hover_text(tooltip);

    let popup = egui::Popup::menu(&resp);
    let popup_resp = popup.show(|ui| {
        const W: f32 = 280.0;
        let w = menu_item_width(W);
        ui.set_min_width(W);
        match &status {
            ServerStatus::Offline => {
                muted(ui, "Not connected to a server");
                if menu_item(ui, "Sign in…", None, w).clicked() {
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
                if menu_item(ui, "Sync now", None, w).clicked() {
                    app.sync_now();
                }
                if menu_item(ui, "Sign out", None, w).clicked() {
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
                if menu_item(ui, "Retry connection", None, w).clicked() {
                    app.retry_connection();
                }
                if menu_item(ui, "Sign out", None, w).clicked() {
                    app.logout();
                }
            }
            ServerStatus::Revoked(name) => {
                muted(
                    ui,
                    "Access to this collection was revoked. Synchronization is disabled.",
                );
                let _ = name;
                if menu_item(ui, "Show collection", None, w).clicked() {
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
    bar_menu_button(ui, "Administration ▾", |ui| {
        const W: f32 = 180.0;
        let w = menu_item_width(W);
        ui.set_min_width(W);
        if menu_item(ui, "Users", None, w).clicked() {
            crate::ui::admin::open_admin(app, crate::state::AdminTab::Users);
        }
        if menu_item(ui, "Organizations", None, w).clicked() {
            crate::ui::admin::open_admin(app, crate::state::AdminTab::Organizations);
        }
    });
}

fn settings_button(app: &mut RestrustApp, ui: &mut Ui) {
    if ui
        .add(
            Button::new(RichText::new("⚙").size(14.0))
                .min_size(Vec2::new(28.0, BAR_CONTROL_HEIGHT)),
        )
        .on_hover_text("Settings")
        .clicked()
    {
        app.state.dialog = DialogState::Settings;
    }
}

fn help_menu(app: &mut RestrustApp, ui: &mut Ui) {
    bar_menu_button(ui, "Help ▾", |ui| {
        const W: f32 = 230.0;
        let w = menu_item_width(W);
        ui.set_min_width(W);
        if menu_item(ui, "Keyboard shortcuts", Some("Ctrl+/"), w).clicked() {
            app.state.dialog = DialogState::Shortcuts;
        }
        if menu_item(ui, "About", None, w).clicked() {
            app.state.dialog = DialogState::About;
        }
    });
}

fn account_button(app: &mut RestrustApp, ui: &mut Ui) {
    let Some(user) = app.state.login.user.clone() else {
        if ui
            .add(Button::new("Sign in…").min_size(Vec2::new(0.0, BAR_CONTROL_HEIGHT)))
            .clicked()
        {
            let server = app.state.doc.settings.server_url.clone();
            app.state.dialog =
                DialogState::SignIn(Box::new(crate::state::SignInDialog::new(server)));
        }
        return;
    };
    bar_menu_button(ui, format!("{} ▾", user.username), |ui| {
        const W: f32 = 230.0;
        let w = menu_item_width(W);
        ui.set_min_width(W);
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
        if menu_item(ui, "Sign out", None, w).clicked() {
            app.logout();
        }
    });
}
