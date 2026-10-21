//! Left sidebar (ui-spec §1.2, §2.2, §2.3): namespace switcher, search,
//! collections tree with context menus, history splitter, plus the narrow-width
//! icon rail and overlay variants (§1.4).

use eframe::egui::{self, Context, RichText, Ui};

use crate::app::RestrustApp;
use crate::state::{Access, DialogState, NamespaceSelection, SelectedNode, TabKind};
use crate::theme;
use crate::ui::common::{current_tokens, icon_button, method_chip, muted};
use crate::ui::history;

use restrust_core::model::{Collection, CollectionItem, Folder, HttpMethod, RequestDef};
use restrust_core::workspace_doc::{StoredCollection, SyncState};

pub fn draw(app: &mut RestrustApp, root: &mut egui::Ui) {
    let t = current_tokens_ctx(root.ctx());
    egui::Panel::left("sidebar")
        .resizable(true)
        .default_size(280.0)
        .size_range(220.0..=480.0)
        .frame(egui::Frame::NONE.fill(t.panel))
        .show(root, |ui| {
            contents(app, ui);
        });
}

/// The 48 px icon rail for 600–899 px widths (§1.4).
pub fn rail(app: &mut RestrustApp, root: &mut egui::Ui) {
    let t = current_tokens_ctx(root.ctx());
    egui::Panel::left("sidebar_rail")
        .resizable(false)
        .exact_size(48.0)
        .frame(egui::Frame::NONE.fill(t.panel))
        .show(root, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(theme::GAP_TIGHT);
                let is_org = matches!(app.state.namespace, NamespaceSelection::Organization(_));
                let initial = namespace_initial(app);
                if ui
                    .add(egui::Button::new(
                        RichText::new(initial).strong().size(14.0),
                    ))
                    .on_hover_text("Switch namespace")
                    .clicked()
                {
                    app.state.sidebar_overlay = true;
                }
                if is_org {
                    let rect = ui.min_rect();
                    ui.painter().circle_filled(
                        rect.right_bottom() + egui::vec2(-6.0, -2.0),
                        3.0,
                        t.accent,
                    );
                }
                ui.add_space(theme::GAP_TIGHT);
                if icon_button(ui, "🔍", "Search collections and requests  Ctrl+F").clicked() {
                    app.state.sidebar_overlay = true;
                    app.state.search_focus = true;
                }
                if icon_button(ui, "🗂", "Collections").clicked() {
                    app.state.sidebar_overlay = true;
                }
                if icon_button(ui, "🕘", "History").clicked() {
                    app.state.sidebar_overlay = true;
                }
            });
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                if icon_button(ui, "⚙", "Settings").clicked() {
                    app.state.dialog = DialogState::Settings;
                }
                ui.add_space(theme::GAP_TIGHT);
            });
        });
}

/// The overlay sidebar for narrow widths: foreground Area, closes on outside
/// click or Esc (§1.4).
pub fn overlay(app: &mut RestrustApp, ctx: &egui::Context) {
    // Click-catcher behind the overlay.
    let screen = ctx.content_rect();
    let clicked_outside = egui::Area::new(egui::Id::unique("sidebar_scrim"))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let resp = ui.allocate_rect(screen, egui::Sense::click());
            resp.clicked()
        })
        .inner;
    if clicked_outside {
        app.state.sidebar_overlay = false;
        return;
    }
    let t = current_tokens_ctx(ctx);
    egui::Area::new(egui::Id::unique("sidebar_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(0.0, theme::TOP_BAR_HEIGHT))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(t.panel)
                .stroke(egui::Stroke::new(1.0, t.border))
                .inner_margin(egui::Margin::same(4))
                .show(ui, |ui| {
                    ui.set_max_width(280.0);
                    ui.set_min_height(
                        (screen.height() - theme::TOP_BAR_HEIGHT - theme::STATUS_BAR_HEIGHT)
                            .max(100.0),
                    );
                    contents(app, ui);
                });
        });
}

fn namespace_initial(app: &RestrustApp) -> String {
    match &app.state.namespace {
        NamespaceSelection::Personal => "P".to_string(),
        NamespaceSelection::Organization(name) => name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default(),
    }
}

fn current_tokens_ctx(ctx: &Context) -> crate::theme::ThemeTokens {
    if ctx.theme() == egui::Theme::Dark {
        theme::DARK
    } else {
        theme::LIGHT
    }
}

/// Sidebar internals: namespace switcher → search → collections header →
/// tree/history split (§1.2).
fn contents(app: &mut RestrustApp, ui: &mut Ui) {
    namespace_switcher(app, ui);
    search_box(app, ui);
    collections_header(app, ui);

    // History splitter: bottom panel at 35% default, min 96 each (§1.2).
    egui::Panel::bottom("sidebar_history")
        .resizable(true)
        .show_separator_line(false)
        .default_size((ui.available_height() * 0.35).max(96.0))
        .min_size(96.0)
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            history::draw(app, ui);
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            tree(app, ui);
        });
}

fn namespace_switcher(app: &mut RestrustApp, ui: &mut Ui) {
    ui.add_space(theme::GAP_TIGHT);
    let label = match &app.state.namespace {
        NamespaceSelection::Personal => "Personal".to_string(),
        NamespaceSelection::Organization(id) => app
            .state
            .login
            .organizations
            .iter()
            .find(|o| o.id == *id)
            .map(|o| o.name.clone())
            .unwrap_or_else(|| "Organization".into()),
    };
    let active_server = app.state.active_server.clone();
    let count_for = |org_id: &str| {
        app.state
            .doc
            .organizations
            .iter()
            .filter(|s| {
                s.origin
                    .as_ref()
                    .map(|o| {
                        o.namespace.id == org_id
                            && Some(o.server_url.as_str()) == active_server.as_deref()
                    })
                    .unwrap_or(false)
            })
            .count()
    };
    ui.menu_button(format!("{label} ▾"), |ui| {
        ui.set_min_width(240.0);
        let mut selected = app.state.namespace.clone();
        if ui.button("Personal").clicked() {
            selected = NamespaceSelection::Personal;
            ui.close();
        }
        for org in app.state.login.organizations.clone() {
            let n = count_for(&org.id);
            if ui
                .button(format!("{}   ({n} collections)", org.name))
                .clicked()
            {
                selected = NamespaceSelection::Organization(org.id);
                ui.close();
            }
        }
        app.state.namespace = selected;
    });
    ui.add_space(theme::GAP_TIGHT);
}

fn search_box(app: &mut RestrustApp, ui: &mut Ui) {
    let mut focus = app.state.search_focus;
    let resp = ui.add(
        egui::TextEdit::singleline(&mut app.state.search)
            .hint_text("Search collections and requests")
            .desired_width(ui.available_width()),
    );
    if focus {
        resp.request_focus();
        focus = false;
    }
    app.state.search_focus = focus;
}

fn collections_header(app: &mut RestrustApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Collections").strong().size(12.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(RichText::new("⇩").size(13.0)).small())
                .on_hover_text("Import…")
                .clicked()
            {
                app.state.dialog =
                    DialogState::Import(Box::new(crate::state::ImportDialog::new(None)));
            }
            if ui
                .add(egui::Button::new(RichText::new("＋").size(14.0)).small())
                .on_hover_text("New collection")
                .clicked()
            {
                app.state.dialog = DialogState::NewCollection {
                    name: String::new(),
                };
            }
        });
    });
    ui.add_space(theme::GAP_TIGHT);
}

// --- tree -------------------------------------------------------------------------

fn tree(app: &mut RestrustApp, ui: &mut Ui) {
    let ids: Vec<String> = app
        .state
        .visible_collections()
        .iter()
        .map(|s| s.collection.id.clone())
        .collect();
    if app.state.doc.personal.is_empty() && app.state.doc.organizations.is_empty() {
        empty_collections(app, ui);
        return;
    }
    if ids.is_empty() {
        // The active namespace has no collections.
        if app.state.search.trim().is_empty() {
            muted(ui, "No collections in this namespace yet.");
            if ui.button("New Collection").clicked() {
                app.state.dialog = DialogState::NewCollection {
                    name: String::new(),
                };
            }
        } else {
            no_matches(app, ui);
        }
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("tree_scroll")
        .auto_shrink(false)
        .show(ui, |ui| {
            let mut remove: Option<String> = None;
            for id in ids {
                if let Some(stored) = app.state.find_stored(&id).cloned() {
                    if !search_matches(&app.state.search, &stored.collection) {
                        continue;
                    }
                    collection_row(app, ui, &stored, &mut remove);
                }
            }
            if let Some(id) = remove {
                app.remove_collection_local(&id);
            }
        });
}

fn empty_collections(app: &mut RestrustApp, ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(theme::GAP_SECTION);
        ui.label(RichText::new("No collections yet").strong());
        ui.add_space(theme::GAP_TIGHT);
        muted(ui, "Create or import your first collection.");
        ui.add_space(theme::GAP_STACK);
        ui.horizontal(|ui| {
            if crate::ui::common::primary_button(ui, "New Collection", 0.0).clicked() {
                app.state.dialog = DialogState::NewCollection {
                    name: String::new(),
                };
            }
            if ui.button("Import…").clicked() {
                app.state.dialog =
                    DialogState::Import(Box::new(crate::state::ImportDialog::new(None)));
            }
        });
    });
}

fn no_matches(app: &mut RestrustApp, ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(theme::GAP_SECTION);
        ui.label(RichText::new(format!("No matches for “{}”", app.state.search)).strong());
        if ui.button("Clear search").clicked() {
            app.state.search.clear();
        }
    });
}

fn search_matches(query: &str, collection: &Collection) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    fn item_matches(item: &CollectionItem, query: &str) -> bool {
        if item.name().to_lowercase().contains(query) {
            return true;
        }
        match item {
            CollectionItem::Folder(folder) => folder.items.iter().any(|i| item_matches(i, query)),
            CollectionItem::Request(_) => false,
        }
    }
    collection.name.to_lowercase().contains(&query)
        || collection.items.iter().any(|i| item_matches(i, &query))
}

fn sync_badge(ui: &mut Ui, sync: &SyncState) {
    let t = current_tokens(ui);
    match sync {
        SyncState::Local => {}
        SyncState::Synced { revision } => {
            let resp = ui.label(RichText::new("✓").size(12.0).color(t.muted).weak());
            resp.on_hover_text(format!("Synced · revision {revision}"));
        }
        SyncState::PendingEdits { .. } => {
            let resp = ui.label(RichText::new("●").size(9.0).color(t.warning));
            resp.on_hover_text("Unsynced edits");
        }
        SyncState::Revoked => crate::ui::common::badge(ui, "revoked", t.revoked_gray),
    }
}

#[allow(clippy::too_many_lines)]
fn collection_row(
    app: &mut RestrustApp,
    ui: &mut Ui,
    stored: &StoredCollection,
    _remove: &mut Option<String>,
) {
    let t = current_tokens(ui);
    let collection_id = stored.collection.id.clone();
    let revoked = stored.sync == SyncState::Revoked;
    let expanded =
        app.state.expanded.contains(&collection_id) || !app.state.search.trim().is_empty();
    let selected = app
        .state
        .selected
        .as_ref()
        .map(|s| s.collection_id == collection_id && s.node_id.is_none())
        .unwrap_or(false);

    let rename_here = app
        .state
        .renaming
        .as_ref()
        .map(|r| r.collection_id == collection_id && r.node_id.is_none())
        .unwrap_or(false);

    let height = theme::TREE_ROW_HEIGHT;
    let resp = crate::ui::common::row_sense(ui, selected, t.hover_row, t.selected_row);
    ui.allocate_ui_with_layout(
        egui::vec2(resp.rect.width(), height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_space(theme::GAP_TIGHT);
            let chevron = if expanded { "▾" } else { "▸" };
            if ui
                .add(egui::Button::new(RichText::new(chevron).size(10.0)).frame(false))
                .clicked()
            {
                if expanded {
                    app.state.expanded.remove(&collection_id);
                } else {
                    app.state.expanded.insert(collection_id.clone());
                }
            }
            let name_color = if revoked { t.revoked_gray } else { t.text };
            if rename_here {
                rename_edit(app, ui, &collection_id, None);
            } else {
                ui.add(
                    egui::Label::new(RichText::new(&stored.collection.name).color(name_color))
                        .selectable(false),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(RichText::new("⋯").size(13.0)).frame(false))
                    .clicked()
                {
                    app.state.context_menu_node = Some((collection_id.clone(), None));
                }
                if !revoked {
                    sync_badge(ui, &stored.sync);
                }
            });
        },
    );
    if resp.clicked() {
        app.state.selected = Some(SelectedNode {
            collection_id: collection_id.clone(),
            node_id: None,
        });
    }
    collection_context_menu(app, ui, &resp, stored);

    if expanded {
        let items_snapshot: Vec<CollectionItem> = stored.collection.items.clone();
        for item in items_snapshot {
            item_row(app, ui, &collection_id, &item, 1, revoked);
        }
    }
}

fn item_row(
    app: &mut RestrustApp,
    ui: &mut Ui,
    collection_id: &str,
    item: &CollectionItem,
    depth: usize,
    revoked: bool,
) {
    let t = current_tokens(ui);
    let node_id = item.id().to_string();
    let selected = app
        .state
        .selected
        .as_ref()
        .map(|s| s.collection_id == collection_id && s.node_id.as_deref() == Some(node_id.as_str()))
        .unwrap_or(false);
    let expanded_child =
        app.state.expanded.contains(&node_id) || !app.state.search.trim().is_empty();

    let mut open_tab = false;
    let resp = crate::ui::common::row_sense(ui, selected, t.hover_row, t.selected_row);
    ui.allocate_ui_with_layout(
        egui::vec2(resp.rect.width(), theme::TREE_ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_space(theme::GAP_TIGHT + depth as f32 * 14.0);
            match item {
                CollectionItem::Request(def) => {
                    method_chip(ui, def.method);
                    let rename_here = app
                        .state
                        .renaming
                        .as_ref()
                        .map(|r| {
                            r.collection_id == collection_id
                                && r.node_id.as_deref() == Some(node_id.as_str())
                        })
                        .unwrap_or(false);
                    if rename_here {
                        rename_edit(app, ui, collection_id, Some(&node_id));
                    } else {
                        ui.add(
                            egui::Label::new(RichText::new(&def.name).color(if revoked {
                                t.revoked_gray
                            } else {
                                t.text
                            }))
                            .selectable(false),
                        );
                    }
                }
                CollectionItem::Folder(folder) => {
                    let chevron = if expanded_child { "▾" } else { "▸" };
                    if ui
                        .add(egui::Button::new(RichText::new(chevron).size(10.0)).frame(false))
                        .clicked()
                    {
                        if expanded_child {
                            app.state.expanded.remove(&node_id);
                        } else {
                            app.state.expanded.insert(node_id.clone());
                        }
                    }
                    let rename_here = app
                        .state
                        .renaming
                        .as_ref()
                        .map(|r| {
                            r.collection_id == collection_id
                                && r.node_id.as_deref() == Some(node_id.as_str())
                        })
                        .unwrap_or(false);
                    if rename_here {
                        rename_edit(app, ui, collection_id, Some(&node_id));
                    } else {
                        ui.add(
                            egui::Label::new(RichText::new(&folder.name).color(if revoked {
                                t.revoked_gray
                            } else {
                                t.text
                            }))
                            .selectable(false),
                        );
                    }
                }
            }
        },
    );
    if resp.clicked() {
        app.state.selected = Some(SelectedNode {
            collection_id: collection_id.to_string(),
            node_id: Some(node_id.clone()),
        });
        // Single click on a request opens it in a new tab (Postman behavior).
        if matches!(item, CollectionItem::Request(_)) && !revoked {
            open_tab = true;
        }
    }
    if resp.double_clicked() && !revoked {
        open_tab = true;
    }
    if open_tab {
        if let CollectionItem::Request(def) = item {
            app.state.open_request(collection_id, &def.id);
        }
    }
    item_context_menu(app, ui, &resp, collection_id, item, revoked);

    if let CollectionItem::Folder(folder) = item {
        if expanded_child {
            for child in &folder.items {
                item_row(app, ui, collection_id, child, depth + 1, revoked);
            }
        }
    }
}

/// Inline rename: Enter commits, Esc cancels, focus loss commits (§1.3).
fn rename_edit(app: &mut RestrustApp, ui: &mut Ui, collection_id: &str, node_id: Option<&str>) {
    // ui is used for focus handling below.
    let Some(rename) = app.state.renaming.as_mut() else {
        return;
    };
    let resp = ui.add(
        egui::TextEdit::singleline(&mut rename.buffer)
            .desired_width(140.0)
            .clip_text(true),
    );
    if !resp.has_focus() && !resp.gained_focus() {
        resp.request_focus();
    }
    let commit = resp.lost_focus() && ui.input(|i| !i.key_pressed(egui::Key::Escape));
    if ui.input(|i| i.key_pressed(egui::Key::Enter)) || commit {
        let buffer = rename.buffer.trim().to_string();
        let node = node_id.map(|s| s.to_string());
        app.state.renaming = None;
        if !buffer.is_empty() {
            if let Some(stored) = app.state.stored_mut(collection_id) {
                match &node {
                    None => stored.collection.name = buffer,
                    Some(id) => {
                        if let Some(item) = stored.collection.find_item_mut(id) {
                            item.rename(buffer);
                        }
                    }
                }
                crate::sync::mark_edited(stored);
            }
            app.state.mark_edited();
        }
    }
}

// --- context menus (§2.3) -----------------------------------------------------------

fn collection_context_menu(
    app: &mut RestrustApp,
    _ui: &mut Ui,
    resp: &egui::Response,
    stored: &StoredCollection,
) {
    let collection_id = stored.collection.id.clone();
    let revoked = stored.sync == SyncState::Revoked;
    let open = matches!(&app.state.context_menu_node, Some((cid, None)) if *cid == collection_id);
    let mut popup = egui::Popup::menu(resp);
    if open {
        popup = popup.open(true);
    }
    popup.show(|ui| {
        app.state.context_menu_node = None;
        if revoked {
            // Inside a revoked collection the menu is exactly these two items.
            if ui.button("Save as personal local copy").clicked() {
                app.save_revoked_as_personal(&collection_id);
            }
            if ui.button("Remove local data…").clicked() {
                app.state.dialog = DialogState::ConfirmRemoveLocal {
                    collection_id: collection_id.clone(),
                    name: stored.collection.name.clone(),
                };
            }
            return;
        }
        let access = app.state.access_of(stored);
        let can_edit = access != Access::Read;
        let owner = access == Access::Owner;
        let read_tooltip = "Read only — you can send and export, but not edit";

        if ui
            .add_enabled(can_edit, egui::Button::new("New Request"))
            .on_disabled_hover_text(read_tooltip)
            .clicked()
        {
            new_request(app, &collection_id, None);
        }
        if ui
            .add_enabled(can_edit, egui::Button::new("New Folder"))
            .on_disabled_hover_text(read_tooltip)
            .clicked()
        {
            new_folder(app, &collection_id, None);
        }
        if ui.button("Rename…\tF2").clicked() {
            start_rename(app, &collection_id, None);
        }
        if ui.button("Duplicate").clicked() {
            duplicate_collection(app, &collection_id);
        }
        ui.separator();

        let synced = stored.origin.is_some() && stored.sync != SyncState::Local;
        if synced && owner {
            if ui.button("Share…").clicked() {
                crate::ui::dialogs::open_share(app, &collection_id);
            }
            if app.state.slug_known.contains_key(
                &stored
                    .origin
                    .as_ref()
                    .map(|o| o.collection_id.clone())
                    .unwrap_or_default(),
            ) {
                if ui.button("Unpublish").clicked() {
                    if let Some(server_id) = stored.origin.as_ref().map(|o| o.collection_id.clone())
                    {
                        if let Some(slug) = app.state.slug_known.get(&server_id).cloned() {
                            app.unpublish(&collection_id, &server_id, slug);
                        }
                    }
                }
            } else if ui.button("Publish to web…").clicked() {
                crate::ui::dialogs::open_share(app, &collection_id);
            }
        }
        if stored.sync == SyncState::Local && stored.origin.is_none() && app.state.login.signed_in()
        {
            if ui.button("Enable sync").clicked() {
                app.enable_sync(&collection_id);
            }
        } else if synced && ui.button("Sync now").clicked() {
            app.sync_now();
        }
        ui.separator();

        if ui.button("Import into…").clicked() {
            app.state.dialog = DialogState::Import(Box::new(crate::state::ImportDialog::new(
                Some(collection_id.clone()),
            )));
        }
        if ui.button("Export…").clicked() {
            crate::ui::dialogs::open_export(app, &collection_id);
        }
        ui.separator();

        // Delete gating (§2.3).
        let is_local = stored.sync == SyncState::Local && stored.origin.is_none();
        let org_owned = stored
            .origin
            .as_ref()
            .map(|o| o.namespace.kind == restrust_core::workspace_doc::NamespaceKind::Organization)
            .unwrap_or(false);
        let requests = stored.collection.count_requests();
        let name = stored.collection.name.clone();
        let enabled = is_local || (owner && !org_owned) || (org_owned && owner);
        let delete_button = egui::Button::new(
            RichText::new("Delete").color(crate::ui::common::current_tokens(ui).danger),
        );
        let resp = if org_owned && !owner {
            ui.add_enabled(false, delete_button).on_disabled_hover_text(
                "Only an organization administrator can delete this collection",
            )
        } else {
            ui.add_enabled(enabled, delete_button)
                .on_disabled_hover_text(read_tooltip)
        };
        if resp.clicked() {
            app.state.dialog = DialogState::ConfirmDelete {
                collection_id: collection_id.clone(),
                name,
                requests,
                on_server: !is_local,
                busy: false,
            };
        }
    });
}

fn item_context_menu(
    app: &mut RestrustApp,
    _ui: &mut Ui,
    resp: &egui::Response,
    collection_id: &str,
    item: &CollectionItem,
    revoked: bool,
) {
    let node_id = item.id().to_string();
    if revoked {
        // No mutations inside revoked collections (§6.2).
        return;
    }
    let can_edit = app
        .state
        .find_stored(collection_id)
        .map(|s| app.state.can_edit_tree(s))
        .unwrap_or(false);
    let read_tooltip = "Read only — you can send and export, but not edit";
    let open = matches!(&app.state.context_menu_node, Some((cid, Some(node))) if *cid == collection_id && *node == node_id);
    let mut popup = egui::Popup::menu(resp);
    if open {
        popup = popup.open(true);
    }
    popup.show(|ui| {
        app.state.context_menu_node = None;
        match item {
            CollectionItem::Folder(_) => {
                if ui
                    .add_enabled(can_edit, egui::Button::new("New Request"))
                    .on_disabled_hover_text(read_tooltip)
                    .clicked()
                {
                    new_request(app, collection_id, Some(&node_id));
                }
                if ui
                    .add_enabled(can_edit, egui::Button::new("New Folder"))
                    .on_disabled_hover_text(read_tooltip)
                    .clicked()
                {
                    new_folder(app, collection_id, Some(&node_id));
                }
                if ui.button("Rename…\tF2").clicked() {
                    start_rename(app, collection_id, Some(&node_id));
                }
                if ui
                    .add_enabled(can_edit, egui::Button::new("Duplicate"))
                    .on_disabled_hover_text(read_tooltip)
                    .clicked()
                {
                    duplicate_item(app, collection_id, item);
                }
                if ui.button("Export…").clicked() {
                    crate::ui::dialogs::open_export(app, collection_id);
                }
                if ui
                    .add_enabled(
                        can_edit,
                        egui::Button::new(
                            RichText::new("Delete")
                                .color(crate::ui::common::current_tokens(ui).danger),
                        ),
                    )
                    .on_disabled_hover_text(read_tooltip)
                    .clicked()
                {
                    delete_item(app, collection_id, &node_id);
                }
            }
            CollectionItem::Request(def) => {
                if ui.button("Open in new tab").clicked() {
                    let id = def.id.clone();
                    app.state.open_request(collection_id, &id);
                }
                if ui.button("Rename…\tF2").clicked() {
                    start_rename(app, collection_id, Some(&node_id));
                }
                if ui.button("Duplicate").clicked() {
                    duplicate_item(app, collection_id, item);
                }
                if ui.button("Copy as cURL").clicked() {
                    let curl = restrust_interchange::curl::export_curl(def);
                    app.services.clipboard.set_text(&curl);
                    app.state.push_toast("Copied".to_string());
                }
                if ui.button("Export…").clicked() {
                    crate::ui::dialogs::open_export(app, collection_id);
                }
                if ui
                    .add_enabled(
                        can_edit,
                        egui::Button::new(
                            RichText::new("Delete")
                                .color(crate::ui::common::current_tokens(ui).danger),
                        ),
                    )
                    .on_disabled_hover_text(read_tooltip)
                    .clicked()
                {
                    delete_item(app, collection_id, &node_id);
                }
            }
        }
    });
}

// --- tree mutations -------------------------------------------------------------------

fn new_request(app: &mut RestrustApp, collection_id: &str, parent: Option<&str>) {
    let def = RequestDef::new(HttpMethod::Get, "New Request", "");
    let id = def.id.clone();
    if let Some(stored) = app.state.stored_mut(collection_id) {
        stored
            .collection
            .insert_item(parent, CollectionItem::Request(def));
        crate::sync::mark_edited(stored);
    }
    app.state.mark_edited();
    app.state.open_request(collection_id, &id);
}

fn new_folder(app: &mut RestrustApp, collection_id: &str, parent: Option<&str>) {
    let folder = Folder {
        id: restrust_core::model::new_id(),
        name: "New Folder".to_string(),
        description: None,
        items: Vec::new(),
        auth: None,
    };
    let id = folder.id.clone();
    if let Some(stored) = app.state.stored_mut(collection_id) {
        stored
            .collection
            .insert_item(parent, CollectionItem::Folder(folder));
        crate::sync::mark_edited(stored);
    }
    app.state.mark_edited();
    app.state.expanded.insert(id);
}

fn start_rename(app: &mut RestrustApp, collection_id: &str, node_id: Option<&str>) {
    let buffer = app
        .state
        .find_stored(collection_id)
        .and_then(|stored| match node_id {
            Some(id) => stored
                .collection
                .find_request(id)
                .map(|r| r.name.clone())
                .or_else(|| find_folder_name(&stored.collection.items, id)),
            None => Some(stored.collection.name.clone()),
        })
        .unwrap_or_default();
    app.state.renaming = Some(crate::state::RenameState {
        collection_id: collection_id.to_string(),
        node_id: node_id.map(|s| s.to_string()),
        buffer,
    });
}

/// Deep clone with fresh ids for every node in the subtree.
fn reid_item(item: &mut CollectionItem) {
    match item {
        CollectionItem::Request(def) => def.id = restrust_core::model::new_id(),
        CollectionItem::Folder(folder) => {
            folder.id = restrust_core::model::new_id();
            for child in &mut folder.items {
                reid_item(child);
            }
        }
    }
}

fn find_folder_name(items: &[CollectionItem], id: &str) -> Option<String> {
    for item in items {
        if let CollectionItem::Folder(folder) = item {
            if folder.id == id {
                return Some(folder.name.clone());
            }
            if let Some(found) = find_folder_name(&folder.items, id) {
                return Some(found);
            }
        }
    }
    None
}

fn duplicate_collection(app: &mut RestrustApp, collection_id: &str) {
    let Some(stored) = app.state.find_stored(collection_id).cloned() else {
        return;
    };
    let is_org = stored
        .origin
        .as_ref()
        .map(|o| o.namespace.kind == restrust_core::workspace_doc::NamespaceKind::Organization)
        .unwrap_or(false);
    let mut copy = stored;
    copy.collection.id = restrust_core::model::new_id();
    copy.collection.name = format!("{} (copy)", copy.collection.name);
    for item in &mut copy.collection.items {
        reid_item(item);
    }
    // A duplicate is local-first; if the source was synced it stays pending
    // only when the user edits it.
    copy.sync = SyncState::Local;
    copy.origin = None;
    copy.access = None;
    if is_org {
        app.state.doc.organizations.push(copy);
    } else {
        app.state.doc.personal.push(copy);
    }
    app.state.mark_edited();
}

fn duplicate_item(app: &mut RestrustApp, collection_id: &str, item: &CollectionItem) {
    let mut copy = item.clone();
    reid_item(&mut copy);
    if let Some(stored) = app.state.stored_mut(collection_id) {
        stored.collection.items.push(copy);
        crate::sync::mark_edited(stored);
    }
    app.state.mark_edited();
}

fn delete_item(app: &mut RestrustApp, collection_id: &str, node_id: &str) {
    if let Some(stored) = app.state.stored_mut(collection_id) {
        stored.collection.remove_item(node_id);
        crate::sync::mark_edited(stored);
    }
    // Close tabs pointing at the removed request.
    app.state.tabs.retain(
        |tab| !matches!(&tab.kind, TabKind::Saved { request_id, .. } if request_id == node_id),
    );
    if app.state.tabs.is_empty() {
        app.state.active_tab = None;
    }
    app.state.mark_edited();
}
