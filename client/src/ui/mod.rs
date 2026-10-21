//! UI orchestration: panel layout (ui-spec §1), global keyboard shortcuts
//! (§7), the main area, status bar and empty states (§2.7).

pub mod admin;
pub mod banners;
pub mod common;
pub mod dialogs;
pub mod history;
pub mod request_editor;
pub mod response_view;
pub mod sidebar;
pub mod top_bar;

use eframe::egui::{self, Context, Ui};

use crate::app::RestrustApp;
use crate::platform::IS_WASM;
use crate::state::{DialogState, TabKind};
use crate::theme;
use common::{current_tokens, heading, muted, primary_button};

use restrust_core::model::RequestDef;
use restrust_core::workspace_doc::SyncState;

/// Wide-layout threshold (§1.4).
const WIDE: f32 = 900.0;
/// Below this the rail hides too (§1.4).
const NARROW: f32 = 600.0;

/// Global shortcuts, checked before any panel painting (§7, §9).
pub fn handle_shortcuts(app: &mut RestrustApp, ctx: &egui::Context) {
    let cmd = |key: egui::Key| ctx.input(|i| i.modifiers.command && i.key_pressed(key));
    let alt = |key: egui::Key| ctx.input(|i| i.modifiers.alt && i.key_pressed(key));

    if cmd(egui::Key::S) {
        app.save_active_request();
    }
    // Browsers usually eat Ctrl+T / Ctrl+W; Alt variants are bound everywhere.
    if cmd(egui::Key::T) || alt(egui::Key::T) {
        app.state.new_tab();
    }
    if cmd(egui::Key::W) || alt(egui::Key::W) {
        if let Some(index) = app.state.active_tab {
            app.state.close_tab(index);
        }
    }
    if cmd(egui::Key::F) {
        app.state.search_focus = true;
        if ctx.content_rect().width() < WIDE {
            app.state.sidebar_overlay = true;
        }
    }
    if cmd(egui::Key::B) {
        toggle_sidebar(app, ctx.content_rect().width());
    }
    if cmd(egui::Key::Comma) {
        app.state.dialog = DialogState::Settings;
    }
    if cmd(egui::Key::Slash) {
        app.state.dialog = DialogState::Shortcuts;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::F2)) && common::global_keys_available(ctx) {
        if let Some(selected) = app.state.selected.clone() {
            if app
                .state
                .find_stored(&selected.collection_id)
                .map(|s| s.sync != SyncState::Revoked)
                .unwrap_or(false)
            {
                let buffer = app
                    .state
                    .find_stored(&selected.collection_id)
                    .and_then(|s| {
                        selected
                            .node_id
                            .as_deref()
                            .and_then(|id| {
                                s.collection
                                    .find_request(id)
                                    .map(|r| r.name.clone())
                                    .or_else(|| folder_name(&s.collection.items, id))
                            })
                            .or_else(|| {
                                selected
                                    .node_id
                                    .is_none()
                                    .then(|| s.collection.name.clone())
                            })
                    })
                    .unwrap_or_default();
                app.state.renaming = Some(crate::state::RenameState {
                    collection_id: selected.collection_id,
                    node_id: selected.node_id,
                    buffer,
                });
            }
        }
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Enter)) && common::global_keys_available(ctx) {
        if let Some(selected) = app.state.selected.clone() {
            if let Some(stored) = app.state.find_stored(&selected.collection_id) {
                if stored.sync == SyncState::Revoked {
                    return;
                }
                match selected.node_id.as_deref() {
                    Some(id) => {
                        if let Some(def) = stored.collection.find_request(id).cloned() {
                            app.state.open_request(&selected.collection_id, &def.id);
                        }
                    }
                    None => {
                        if let Some(RequestDef { id, .. }) = stored
                            .collection
                            .items
                            .iter()
                            .find_map(|item| item.as_request())
                        {
                            let id = id.clone();
                            app.state.open_request(&selected.collection_id, &id);
                        }
                    }
                }
            }
        }
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.state.sidebar_overlay = false;
        if app.state.renaming.take().is_some() {
            return;
        }
        if !app.state.dialog.in_flight() && !matches!(app.state.dialog, DialogState::None) {
            // Modals consume Esc themselves; this covers custom overlays.
            app.state.dialog = DialogState::None;
        }
    }
}

pub fn toggle_sidebar(app: &mut RestrustApp, width: f32) {
    if width < WIDE {
        app.state.sidebar_overlay = !app.state.sidebar_overlay;
    } else {
        app.state.sidebar_visible = !app.state.sidebar_visible;
        app.state.sidebar_overlay = false;
    }
}

/// The full frame: top bar, banner strip, status bar, sidebar, main area,
/// dialogs, toasts, administration window (§1.1).
pub const EGUI_VERSION: &str = "0.37";

pub fn draw(app: &mut RestrustApp, root: &mut egui::Ui, ctx: &egui::Context) {
    top_bar::draw(app, root);
    banners::strip(app, root);
    egui::Panel::bottom("status_bar")
        .exact_size(theme::STATUS_BAR_HEIGHT)
        .frame(egui::Frame::NONE.fill(current_tokens_ctx(ctx).panel))
        .show(root, |ui| status_bar(app, ui));

    let width = ctx.content_rect().width();
    if width >= WIDE {
        if app.state.sidebar_visible {
            sidebar::draw(app, root);
        }
    } else if width >= NARROW {
        if app.state.sidebar_visible {
            sidebar::rail(app, root);
        }
        if app.state.sidebar_overlay {
            sidebar::overlay(app, ctx);
        }
    } else if app.state.sidebar_overlay {
        sidebar::overlay(app, ctx);
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(current_tokens_ctx(ctx).bg))
        .show(root, |ui| main_area(app, ui));

    dialogs::draw(app, ctx);
    banners::toasts(app, ctx);
    admin::draw(app, ctx);
}

fn folder_name(items: &[restrust_core::model::CollectionItem], id: &str) -> Option<String> {
    for item in items {
        if let restrust_core::model::CollectionItem::Folder(folder) = item {
            if folder.id == id {
                return Some(folder.name.clone());
            }
            if let Some(found) = folder_name(&folder.items, id) {
                return Some(found);
            }
        }
    }
    None
}

fn current_tokens_ctx(ctx: &Context) -> crate::theme::ThemeTokens {
    if ctx.theme() == egui::Theme::Dark {
        theme::DARK
    } else {
        theme::LIGHT
    }
}

// --- status bar (§8.1) -----------------------------------------------------------

fn status_bar(app: &mut RestrustApp, ui: &mut Ui) {
    let t = current_tokens(ui);
    ui.horizontal_centered(|ui| {
        ui.set_height(theme::STATUS_BAR_HEIGHT);
        let saving = app
            .state
            .save_started
            .map(|s| s.elapsed().as_millis() < 350)
            .unwrap_or(false);
        let left = if saving {
            "Saving…".to_string()
        } else if let Some(hhmm) = &app.last_saved_hhmm {
            format!("All changes saved {hhmm}")
        } else {
            "All changes saved".to_string()
        };
        ui.label(egui::RichText::new(left).size(11.0).color(t.muted));
        ui.separator();
        ui.label(
            egui::RichText::new(app.services.storage_description())
                .size(11.0)
                .color(t.muted),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("restrust v{}", env!("CARGO_PKG_VERSION")))
                    .size(11.0)
                    .color(t.muted),
            );
        });
    });
}

// --- main area ---------------------------------------------------------------------

fn main_area(app: &mut RestrustApp, ui: &mut Ui) {
    // Revoked view (§6.2) takes over the main area while a node of a revoked
    // collection is selected.
    if let Some(selected) = app.state.selected.clone() {
        if app
            .state
            .find_stored(&selected.collection_id)
            .map(|s| s.sync == SyncState::Revoked)
            .unwrap_or(false)
        {
            revoked_view(app, ui, &selected.collection_id);
            return;
        }
    }

    if app.state.tabs.is_empty() {
        empty_tabs(app, ui);
        return;
    }

    tab_bar(app, ui);
    request_editor::title_row(app, ui);
    if app.state.active_tab().is_some_and(|t| t.code_open) {
        egui::Panel::right("code_snippet")
            .resizable(true)
            .default_size(300.0)
            .size_range(220.0..=500.0)
            .show(ui, |ui| request_editor::code_panel(app, ui));
    }

    let narrow = ui.available_width() < 560.0;
    request_editor::url_row(app, ui, narrow);

    let Some(tab_id) = app.state.active_tab().map(|t| t.id) else {
        return;
    };
    let collapsed = app
        .state
        .active_tab()
        .map(|t| t.response_collapsed)
        .unwrap_or(false);

    if collapsed {
        response_view::collapsed_stub(app, ui);
        request_editor::draw(app, ui);
    } else {
        egui::Panel::top("request_pane")
            .resizable(true)
            .show_separator_line(false)
            .default_size((ui.available_height() * 0.55).max(120.0))
            .min_size(120.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("request_editor_scroll")
                    .show(ui, |ui| request_editor::draw(app, ui));
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                response_view::draw(app, ui, tab_id);
            });
    }
}

fn tab_bar(app: &mut RestrustApp, ui: &mut Ui) {
    let t = current_tokens(ui);
    egui::Frame::NONE
        .fill(t.panel)
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.set_height(theme::TAB_BAR_HEIGHT);
            ui.horizontal_centered(|ui| {
                let mut close: Option<usize> = None;
                let labels: Vec<(u64, String)> = app
                    .state
                    .tabs
                    .iter()
                    .map(|tab| (tab.id, tab.label(&app.state.doc)))
                    .collect();
                egui::ScrollArea::horizontal()
                    .id_salt("tab_bar_scroll")
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            for (index, (tab_id, label)) in labels.iter().enumerate() {
                                let active = app.state.active_tab == Some(index);
                                let resp = ui.selectable_label(
                                    active,
                                    egui::RichText::new(format!(" {label} "))
                                        .strong()
                                        .color(if active { t.on_accent } else { t.text }),
                                );
                                if resp.clicked() {
                                    app.state.active_tab = Some(index);
                                }
                                if resp.middle_clicked() {
                                    close = Some(index);
                                }
                                let close_resp = ui.add(
                                    egui::Button::new(
                                        egui::RichText::new("×").small().color(t.muted),
                                    )
                                    .small(),
                                );
                                if close_resp.clicked() {
                                    close = Some(index);
                                }
                                let _ = tab_id;
                                ui.separator();
                            }
                        });
                    });
                if ui
                    .add(egui::Button::new(egui::RichText::new("＋").size(14.0)))
                    .on_hover_text("New request tab (Ctrl+T)")
                    .clicked()
                {
                    app.state.new_tab();
                }
                if let Some(index) = close {
                    app.state.close_tab(index);
                }
            });
        });
}

// --- empty states (§2.7) -------------------------------------------------------------

fn empty_tabs(app: &mut RestrustApp, ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(theme::GAP_SECTION * 2.0);
        heading(ui, "No open requests");
        ui.add_space(theme::GAP_TIGHT);
        muted(ui, "Create a request from the sidebar, or press Ctrl+T.");
        ui.add_space(theme::GAP_STACK);
        if primary_button(ui, "New Request", 120.0).clicked() {
            app.state.new_tab();
        }
    });
}

/// The revocation banner view (§6.2, exact texts).
pub fn revoked_view(app: &mut RestrustApp, ui: &mut Ui, collection_id: &str) {
    let t = current_tokens(ui);
    let Some(stored) = app.state.find_stored(collection_id).cloned() else {
        return;
    };
    let requests = stored.collection.count_requests();
    let name = stored.collection.name.clone();
    ui.vertical_centered(|ui| {
        ui.add_space(theme::GAP_SECTION * 2.0);
        egui::Frame::NONE
            .fill(crate::ui::common::linear_alpha(t.danger, 0.10))
            .stroke(egui::Stroke::NONE)
            .corner_radius(8.0)
            .inner_margin(theme::GAP_SECTION)
            .show(ui, |ui| {
                ui.set_width(ui.available_width().min(560.0));
                egui::Frame::NONE
                    .fill(t.danger)
                    .corner_radius(2.0)
                    .show(ui, |ui| {
                        ui.set_width(4.0);
                        ui.set_height(2.0);
                    });
                ui.add_space(theme::GAP_TIGHT);
                ui.label(
                    egui::RichText::new(
                        "⚠ Access to this collection was revoked. Synchronization is disabled.",
                    )
                    .strong()
                    .color(t.text),
                );
                ui.add_space(theme::GAP_STACK);
                ui.horizontal(|ui| {
                    if primary_button(ui, "Save as personal local copy", 0.0).clicked() {
                        app.save_revoked_as_personal(collection_id);
                    }
                    if common::danger_button(ui, "Delete local data").clicked() {
                        app.state.dialog = DialogState::ConfirmRemoveLocal {
                            collection_id: collection_id.to_string(),
                            name: name.clone(),
                        };
                    }
                });
                ui.add_space(theme::GAP_STACK);
                muted(
                    ui,
                    &format!(
                        "{requests} requests are retained on this device. Nothing was deleted automatically."
                    ),
                );
            });
    });
}

/// True when the active tab points into the given collection.
#[allow(dead_code)]
fn tab_points_into(app: &RestrustApp, collection_id: &str) -> bool {
    app.state
        .active_tab()
        .map(|tab| {
            matches!(&tab.kind, TabKind::Saved { collection_id: c, .. } if c == collection_id)
        })
        .unwrap_or(false)
}

/// Platform note used in send-failure hints (§1.5).
pub fn platform_hint() -> &'static str {
    if IS_WASM {
        "Check the URL, network, and CORS — the browser blocks cross-origin requests without server opt-in."
    } else {
        "Check the URL and network."
    }
}
