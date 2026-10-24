//! Administration window (ui-spec §3.8): Users and Organizations tabs, calling
//! the admin APIs with verbatim server errors. Native renders a second OS
//! viewport; wasm a full-area modal (§1.5). The shared body uses only
//! `show_inside` panels so it works in both hosts.

use std::sync::{Arc, Mutex, MutexGuard};

use eframe::egui::{self, Button, RichText, Ui, Vec2};

use crate::api;
use crate::app::RestrustApp;
use crate::platform::Services;
use crate::state::{AdminOp, AdminTab, AdminUi, Banner, BannerAction, MemberRow, Severity};
use crate::theme;
use crate::ui::common::{
    current_tokens, heading, linear_alpha, muted, primary_button, primary_button_enabled,
    row_sense, section_label,
};

/// Opens the administration window on the given tab and (re)loads its lists.
pub fn open_admin(app: &mut RestrustApp, tab: AdminTab) {
    let base = app.server_url();
    let token = app.state.login.token.clone().unwrap_or_default();
    if base.is_empty() || token.is_empty() {
        return;
    }
    {
        let mut admin = app.admin.lock().unwrap();
        admin.base = base;
        admin.token = token;
        admin.tab = tab;
        admin.users_loaded = false;
        admin.orgs_loaded = false;
        admin.banners.clear();
        admin.selected_org = None;
        admin.member_draft.clear();
        admin.loading_users = false;
        admin.loading_orgs = false;
        admin.pending.clear();
    }
    app.state.admin_open = true;
    reload_lists(app);
}

fn reload_lists(app: &RestrustApp) {
    let mut admin = app.admin.lock().unwrap();
    if !admin.users_loaded && !admin.loading_users {
        admin.loading_users = true;
        let req = api::admin_users_list(&admin.base, &admin.token);
        queue_admin(&mut admin, &app.services, AdminOp::ListUsers, req);
    }
    if !admin.orgs_loaded && !admin.loading_orgs {
        admin.loading_orgs = true;
        let req = api::admin_orgs_list(&admin.base, &admin.token);
        queue_admin(&mut admin, &app.services, AdminOp::ListOrgs, req);
    }
}

/// Issues an admin request, recording which operation it was for. Use this
/// when the `AdminUi` lock is NOT held (it locks internally).
fn issue_admin(
    admin: &Arc<Mutex<AdminUi>>,
    services: &Arc<Services>,
    op: AdminOp,
    req: ehttp::Request,
) {
    queue_admin(&mut admin.lock().unwrap(), services, op, req);
}

/// Same as [`issue_admin`] for callers that already hold the lock — re-locking
/// from inside the UI closures would deadlock the window.
fn queue_admin(admin: &mut AdminUi, services: &Arc<Services>, op: AdminOp, req: ehttp::Request) {
    let id = services.http.fetch_tracked(req);
    admin.pending.insert(id, op);
}

/// Renders the administration window: native second viewport, wasm full-area
/// modal (§1.5).
pub fn draw(app: &mut RestrustApp, ctx: &egui::Context) {
    if !app.state.admin_open {
        return;
    }
    let host = RestrustApp::host_of(&app.admin.lock().unwrap().base);

    if cfg!(target_arch = "wasm32") {
        // Full-area modal with a pseudo title bar (§1.5 [default]).
        let t = if ctx.theme() == egui::Theme::Dark {
            theme::DARK
        } else {
            theme::LIGHT
        };
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("admin_modal"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(16.0, 16.0))
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(t.panel)
                    .stroke(egui::Stroke::new(1.0, t.border))
                    .corner_radius(8.0)
                    .inner_margin(theme::GAP_INNER)
                    .show(ui, |ui| {
                        ui.set_min_size(egui::vec2(
                            (screen.width() - 32.0).max(320.0),
                            (screen.height() - 32.0).max(240.0),
                        ));
                        admin_ui(ui, &app.admin, &app.services, &host);
                    });
            });
        return;
    }

    let admin = app.admin.clone();
    let services = app.services.clone();
    let title = format!("Administration — {host}");
    ctx.show_viewport_deferred(
        egui::ViewportId(egui::Id::unique("admin window")),
        egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([900.0, 560.0]),
        move |ui, _class| {
            // Honor the OS close button.
            {
                let mut admin_ref = admin.lock().unwrap();
                if ui.input(|i| {
                    i.viewport()
                        .events
                        .iter()
                        .any(|e| matches!(e, egui::ViewportEvent::Close))
                }) {
                    admin_ref.close_requested = true;
                }
            }
            // The OS window provides the title bar; render the shared body.
            let host = RestrustApp::host_of(&admin.lock().unwrap().base);
            admin_ui(ui, &admin, &services, &host);
        },
    );
}

/// The shared administration body (usable inside any host Ui).
fn admin_ui(ui: &mut Ui, admin_arc: &Arc<Mutex<AdminUi>>, services: &Arc<Services>, host: &str) {
    let mut admin = admin_arc.lock().unwrap();
    let t = current_tokens(ui);
    let is_wasm = cfg!(target_arch = "wasm32");

    // Pseudo title bar (wasm only): raised strip with a real close button.
    if is_wasm {
        egui::Frame::NONE
            .fill(t.panel_raised)
            .stroke(egui::Stroke::new(1.0, t.border))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.set_height(theme::HEADER_ROW_HEIGHT);
                    ui.label(RichText::new(format!("Administration — {host}")).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(Button::new("✕").min_size(Vec2::new(26.0, 22.0)))
                            .on_hover_text("Close")
                            .clicked()
                        {
                            admin.close_requested = true;
                        }
                    });
                });
            });
        ui.add_space(theme::GAP_STACK);
    }

    // Tab bar: Users | Organizations.
    ui.horizontal_centered(|ui| {
        for (tab, label) in [
            (AdminTab::Users, "Users"),
            (AdminTab::Organizations, "Organizations"),
        ] {
            if ui
                .add(
                    Button::selectable(admin.tab == tab, RichText::new(label).strong())
                        .min_size(Vec2::new(120.0, theme::HEIGHT_MENU_ITEM)),
                )
                .clicked()
            {
                admin.tab = tab;
            }
        }
    });
    ui.separator();

    // Banner strip directly under the tab bar (§3.8).
    if !admin.banners.is_empty() {
        let mut dismiss: Option<usize> = None;
        for (index, banner) in admin.banners.iter().enumerate() {
            if banner_ui(ui, &t, banner) {
                dismiss = Some(index);
            }
            ui.add_space(theme::GAP_TIGHT);
        }
        if let Some(index) = dismiss {
            admin.banners.remove(index);
        }
        ui.add_space(theme::GAP_STACK);
    }

    match admin.tab {
        AdminTab::Users => users_tab(ui, &mut admin, services),
        AdminTab::Organizations => orgs_tab(ui, &mut admin, services),
    }
}

/// One banner: tinted card with a status dot, a wrapping message and a
/// right-aligned dismiss button. Returns true when dismissed.
fn banner_ui(ui: &mut Ui, t: &crate::theme::ThemeTokens, banner: &Banner) -> bool {
    let color = match banner.severity {
        Severity::Info => t.info,
        Severity::Warning => t.warning,
        Severity::Danger => t.danger,
    };
    let mut dismiss = false;
    egui::Frame::NONE
        .fill(linear_alpha(color, 0.08))
        .stroke(egui::Stroke::new(1.0, linear_alpha(color, 0.35)))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("●").size(11.0).color(color));
                ui.with_layout(
                    egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&banner.message).size(12.0).color(t.text),
                            )
                            .wrap(),
                        );
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            Button::new(RichText::new("✕").size(11.0).color(t.muted))
                                .min_size(Vec2::new(20.0, 20.0)),
                        )
                        .on_hover_text("Dismiss")
                        .clicked()
                    {
                        dismiss = true;
                    }
                });
            });
        });
    dismiss
}

fn users_tab(ui: &mut Ui, admin: &mut MutexGuard<'_, AdminUi>, services: &Arc<Services>) {
    let t = current_tokens(ui);
    ui.horizontal_centered(|ui| {
        if ui
            .add(Button::new("New user…").min_size(Vec2::new(0.0, theme::HEIGHT_CONTROL)))
            .clicked()
        {
            admin.new_user = Some(crate::state::NewUserDialog {
                username: String::new(),
                password: String::new(),
                role: "user".to_string(),
            });
        }
        // Every user operation ends with a list refresh; show it working.
        if admin.pending.values().any(|op| {
            matches!(
                op,
                AdminOp::ListUsers
                    | AdminOp::CreateUser
                    | AdminOp::PatchUser { .. }
                    | AdminOp::ResetPassword { .. }
            )
        }) {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(RichText::new("Refreshing…").size(12.0).color(t.muted));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_sized(
                [220.0, theme::HEIGHT_CONTROL],
                egui::TextEdit::singleline(&mut admin.filter).hint_text("Filter users"),
            );
        });
    });
    ui.add_space(theme::GAP_TIGHT);

    if admin.loading_users && admin.users.is_empty() {
        ui.add(egui::Spinner::new().size(16.0));
        return;
    }
    if let Some(error) = &admin.users_error {
        muted(ui, error);
        ui.add_space(theme::GAP_TIGHT);
    }

    let filter = admin.filter.to_lowercase();
    let mut patch: Option<(String, Option<String>, Option<bool>)> = None;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            egui::Grid::new("admin_users_grid")
                .num_columns(6)
                .striped(true)
                .min_row_height(theme::HEIGHT_CONTROL)
                .spacing([theme::GAP_INNER, 4.0])
                .show(ui, |ui| {
                    for (name, width) in [
                        ("Username", 150.0),
                        ("Display name", 170.0),
                        ("Role", 110.0),
                        ("Status", 90.0),
                        ("Created", 100.0),
                        ("", 40.0),
                    ] {
                        ui.set_min_width(width);
                        ui.label(RichText::new(name).strong().size(12.0).color(t.muted));
                    }
                    ui.end_row();
                    let mut visible = 0usize;
                    for user in admin.users.clone() {
                        if !filter.is_empty()
                            && !user.username.to_lowercase().contains(&filter)
                            && !user
                                .display_name
                                .as_deref()
                                .unwrap_or_default()
                                .to_lowercase()
                                .contains(&filter)
                        {
                            continue;
                        }
                        visible += 1;
                        ui.label(RichText::new(&user.username).size(12.0));
                        ui.label(
                            RichText::new(user.display_name.clone().unwrap_or_default()).size(12.0),
                        );
                        // Role cell: inline dropdown PATCHing immediately (§3.8).
                        let busy = admin.row_busy.contains(&user.id);
                        let mut role = user.system_role.clone();
                        ui.horizontal_centered(|ui| {
                            egui::ComboBox::from_id_salt(format!("role_{}", user.id))
                                .width(90.0)
                                .selected_text(role.clone())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut role, "user".to_string(), "user");
                                    ui.selectable_value(&mut role, "admin".to_string(), "admin");
                                });
                            if busy {
                                ui.add(egui::Spinner::new().size(14.0));
                            }
                        });
                        if role != user.system_role && !busy {
                            patch = Some((user.id.clone(), Some(role), None));
                        }
                        let status = if user.disabled { "Disabled" } else { "Active" };
                        ui.label(RichText::new(status).size(12.0).color(if user.disabled {
                            t.revoked_gray
                        } else {
                            t.success
                        }));
                        ui.label(
                            RichText::new(
                                user.created_at
                                    .get(..10)
                                    .unwrap_or(&user.created_at)
                                    .to_string(),
                            )
                            .size(12.0),
                        );
                        // Row menu [⋯]: Disable/Enable + Reset password… (§3.8).
                        let menu = ui.add(
                            Button::new(RichText::new("⋯").size(14.0).color(t.muted))
                                .min_size(Vec2::new(28.0, 22.0)),
                        );
                        let mut toggle: Option<(String, bool)> = None;
                        let mut reset: Option<String> = None;
                        egui::Popup::menu(&menu).show(|ui| {
                            if menu_item_row(ui, if user.disabled { "Enable" } else { "Disable" })
                                .clicked()
                            {
                                toggle = Some((user.id.clone(), !user.disabled));
                            }
                            if menu_item_row(ui, "Reset password…").clicked() {
                                reset = Some(user.id.clone());
                            }
                        });
                        if let Some((user_id, disabled)) = toggle {
                            patch = Some((user_id, None, Some(disabled)));
                        }
                        if let Some(user_id) = reset {
                            admin.reset_password = Some(crate::state::ResetPasswordDialog {
                                user_id,
                                username: user.username.clone(),
                                password: String::new(),
                            });
                        }
                        ui.end_row();
                    }
                    if admin.users_loaded && visible == 0 {
                        ui.label(
                            RichText::new(if filter.is_empty() {
                                "No users yet."
                            } else {
                                "No users match this filter."
                            })
                            .size(12.0)
                            .color(t.muted),
                        );
                        ui.end_row();
                    }
                });
        });

    if let Some((user_id, role, disabled)) = patch {
        admin.row_busy.insert(user_id.clone());
        let req = api::admin_user_patch(
            &admin.base,
            &admin.token,
            &user_id,
            role.as_deref(),
            disabled,
            None,
        );
        queue_admin(admin, services, AdminOp::PatchUser { user_id }, req);
    }

    // New user dialog.
    if let Some(dialog) = admin.new_user.clone() {
        let mut dialog = dialog;
        let mut close = false;
        egui::Window::new("New user")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_min_width(280.0);
                section_label(ui, "Username");
                ui.add_sized(
                    [260.0, theme::HEIGHT_CONTROL],
                    egui::TextEdit::singleline(&mut dialog.username).hint_text("username"),
                );
                ui.add_space(theme::GAP_STACK);
                section_label(ui, "Password");
                ui.add_sized(
                    [260.0, theme::HEIGHT_CONTROL],
                    egui::TextEdit::singleline(&mut dialog.password)
                        .password(true)
                        .hint_text("Initial password"),
                );
                ui.add_space(theme::GAP_STACK);
                section_label(ui, "Role");
                egui::ComboBox::from_id_salt("new_user_role")
                    .width(150.0)
                    .selected_text(dialog.role.clone())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut dialog.role, "user".to_string(), "user");
                        ui.selectable_value(&mut dialog.role, "admin".to_string(), "admin");
                    });
                let valid = !dialog.username.trim().is_empty() && !dialog.password.is_empty();
                ui.add_space(theme::GAP_SECTION);
                if !valid {
                    muted(ui, "Enter a username and an initial password.");
                    ui.add_space(theme::GAP_TIGHT);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button_enabled(ui, valid, "Create", 90.0).clicked() {
                        let req = api::admin_user_create(
                            &admin.base,
                            &admin.token,
                            dialog.username.trim(),
                            &dialog.password,
                            &dialog.role,
                        );
                        queue_admin(admin, services, AdminOp::CreateUser, req);
                        close = true;
                    }
                    if ui
                        .add(Button::new("Cancel").min_size(Vec2::new(80.0, theme::HEIGHT_CONTROL)))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
        if close {
            admin.new_user = None;
        } else {
            admin.new_user = Some(dialog);
        }
    }

    // Reset password dialog.
    if let Some(dialog) = admin.reset_password.clone() {
        let mut dialog = dialog;
        let mut close = false;
        egui::Window::new("Reset password")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_min_width(280.0);
                muted(ui, &format!("New password for {}", dialog.username));
                ui.add_space(theme::GAP_STACK);
                section_label(ui, "New password");
                ui.add_sized(
                    [260.0, theme::HEIGHT_CONTROL],
                    egui::TextEdit::singleline(&mut dialog.password)
                        .password(true)
                        .hint_text("New password"),
                );
                let valid = !dialog.password.is_empty();
                ui.add_space(theme::GAP_SECTION);
                if !valid {
                    muted(ui, "Enter the new password.");
                    ui.add_space(theme::GAP_TIGHT);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button_enabled(ui, valid, "Reset", 90.0).clicked() {
                        let req = api::admin_user_patch(
                            &admin.base,
                            &admin.token,
                            &dialog.user_id,
                            None,
                            None,
                            Some(&dialog.password),
                        );
                        queue_admin(
                            admin,
                            services,
                            AdminOp::ResetPassword {
                                user_id: dialog.user_id.clone(),
                            },
                            req,
                        );
                        close = true;
                    }
                    if ui
                        .add(Button::new("Cancel").min_size(Vec2::new(80.0, theme::HEIGHT_CONTROL)))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
        if close {
            admin.reset_password = None;
        } else {
            admin.reset_password = Some(dialog);
        }
    }
}

/// Full-width item inside the row `⋯` popup menu.
fn menu_item_row(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(Button::new(label).min_size(Vec2::new(
        ui.available_width().min(180.0),
        theme::HEIGHT_MENU_ITEM,
    )))
}

fn orgs_tab(ui: &mut Ui, admin: &mut MutexGuard<'_, AdminUi>, services: &Arc<Services>) {
    let t = current_tokens(ui);
    if admin.add_member_role.is_empty() {
        admin.add_member_role = "viewer".to_string();
    }
    ui.horizontal_centered(|ui| {
        if ui
            .add(Button::new("New organization…").min_size(Vec2::new(0.0, theme::HEIGHT_CONTROL)))
            .clicked()
        {
            admin.new_org_open = true;
            admin.new_org_name.clear();
        }
        // Creating an org or saving members ends with a list refresh.
        if admin.pending.values().any(|op| {
            matches!(
                op,
                AdminOp::ListOrgs | AdminOp::CreateOrg | AdminOp::PutMembers { .. }
            )
        }) {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(RichText::new("Refreshing…").size(12.0).color(t.muted));
        }
    });
    ui.add_space(theme::GAP_TIGHT);
    if admin.loading_orgs && admin.orgs.is_empty() {
        ui.add(egui::Spinner::new().size(16.0));
        return;
    }

    egui::Panel::left("admin_orgs_list")
        .resizable(false)
        .exact_size(240.0)
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            if admin.orgs.is_empty() && admin.orgs_loaded {
                muted(ui, "No organizations yet.");
            }
            let mut select: Option<usize> = None;
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    for (index, org) in admin.orgs.clone().iter().enumerate() {
                        let selected = admin.selected_org == Some(index);
                        let resp = egui::Frame::NONE
                            .inner_margin(egui::Margin::symmetric(8, 4))
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                let r = row_sense(
                                    ui,
                                    selected,
                                    current_tokens(ui).hover_row,
                                    current_tokens(ui).selected_row,
                                );
                                ui.horizontal_centered(|ui| {
                                    ui.label(RichText::new(&org.name).size(12.0));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(format!("{}", org.members.len()))
                                                    .size(11.0)
                                                    .weak(),
                                            );
                                        },
                                    );
                                });
                                r
                            })
                            .inner;
                        if resp.clicked() {
                            select = Some(index);
                        }
                    }
                });
            if let Some(index) = select {
                admin.selected_org = Some(index);
                if let Some(org) = admin.orgs.get(index) {
                    admin.member_draft = org
                        .members
                        .iter()
                        .map(|m| MemberRow {
                            username: m.username.clone(),
                            role: m.role.clone(),
                            removed: false,
                        })
                        .collect();
                }
            }
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            let Some(org) = admin.selected_org_data().cloned() else {
                ui.vertical_centered(|ui| {
                    ui.add_space(theme::GAP_SECTION * 2.0);
                    muted(ui, "Select an organization to manage its members.");
                });
                return;
            };
            heading(ui, &format!("Members of “{}”", org.name));
            ui.add_space(theme::GAP_TIGHT);
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    egui::Grid::new("admin_members_grid")
                        .num_columns(3)
                        .striped(true)
                        .min_row_height(theme::HEIGHT_CONTROL)
                        .spacing([theme::GAP_INNER, 4.0])
                        .show(ui, |ui| {
                            ui.set_min_width(180.0);
                            ui.label(RichText::new("Username").strong().size(12.0).color(t.muted));
                            ui.set_min_width(120.0);
                            ui.label(RichText::new("Role").strong().size(12.0).color(t.muted));
                            ui.set_min_width(40.0);
                            ui.label("");
                            ui.end_row();
                            for row in admin.member_draft.iter_mut() {
                                let text = if row.removed {
                                    RichText::new(&row.username)
                                        .size(12.0)
                                        .color(current_tokens(ui).revoked_gray)
                                        .strikethrough()
                                } else {
                                    RichText::new(&row.username).size(12.0)
                                };
                                ui.label(text);
                                let mut role = row.role.clone();
                                egui::ComboBox::from_id_salt(format!(
                                    "member_role_{}",
                                    row.username
                                ))
                                .width(110.0)
                                .selected_text(role.clone())
                                .show_ui(ui, |ui| {
                                    for option in ["admin", "editor", "viewer"] {
                                        ui.selectable_value(&mut role, option.to_string(), option);
                                    }
                                });
                                row.role = role;
                                ui.add_enabled(
                                    !row.removed,
                                    Button::new(RichText::new("✕").size(12.0).color(t.muted))
                                        .min_size(Vec2::new(24.0, 22.0)),
                                )
                                .on_hover_text(if row.removed {
                                    "Marked for removal — save to apply"
                                } else {
                                    "Remove member"
                                });
                                ui.end_row();
                            }
                            if admin.member_draft.is_empty() {
                                ui.label(RichText::new("No members.").size(12.0).color(t.muted));
                                ui.end_row();
                            }
                        });
                });
            ui.add_space(theme::GAP_STACK);
            ui.horizontal_centered(|ui| {
                ui.add_sized(
                    [200.0, theme::HEIGHT_CONTROL],
                    egui::TextEdit::singleline(&mut admin.add_member_username)
                        .hint_text("Add member by username…"),
                );
                egui::ComboBox::from_id_salt("add_member_role")
                    .width(100.0)
                    .selected_text(admin.add_member_role.clone())
                    .show_ui(ui, |ui| {
                        for option in ["admin", "editor", "viewer"] {
                            ui.selectable_value(
                                &mut admin.add_member_role,
                                option.to_string(),
                                option,
                            );
                        }
                    });
                if ui
                    .add(Button::new("Add").min_size(Vec2::new(64.0, theme::HEIGHT_CONTROL)))
                    .clicked()
                {
                    let username = admin.add_member_username.trim().to_string();
                    if !username.is_empty() {
                        let role = admin.add_member_role.clone();
                        admin.member_draft.push(MemberRow {
                            username,
                            role,
                            removed: false,
                        });
                        admin.add_member_username.clear();
                    }
                }
            });
            ui.add_space(theme::GAP_STACK);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if primary_button(ui, "Save members", 110.0).clicked() {
                    let members: Vec<(String, String)> = admin
                        .member_draft
                        .iter()
                        .filter(|row| !row.removed)
                        .map(|row| (row.username.clone(), row.role.clone()))
                        .collect();
                    let req =
                        api::admin_org_members_put(&admin.base, &admin.token, &org.id, &members);
                    queue_admin(
                        admin,
                        services,
                        AdminOp::PutMembers {
                            org_id: org.id.clone(),
                        },
                        req,
                    );
                }
            });
        });

    if admin.new_org_open {
        let mut close = false;
        egui::Window::new("New organization")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_min_width(280.0);
                section_label(ui, "Name");
                ui.add_sized(
                    [260.0, theme::HEIGHT_CONTROL],
                    egui::TextEdit::singleline(&mut admin.new_org_name)
                        .hint_text("Organization name"),
                );
                let valid = !admin.new_org_name.trim().is_empty();
                ui.add_space(theme::GAP_SECTION);
                if !valid {
                    muted(ui, "Enter a name for the organization.");
                    ui.add_space(theme::GAP_TIGHT);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button_enabled(ui, valid, "Create", 90.0).clicked() {
                        let req = api::admin_org_create(
                            &admin.base,
                            &admin.token,
                            admin.new_org_name.trim(),
                        );
                        queue_admin(admin, services, AdminOp::CreateOrg, req);
                        close = true;
                    }
                    if ui
                        .add(Button::new("Cancel").min_size(Vec2::new(80.0, theme::HEIGHT_CONTROL)))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
        if close {
            admin.new_org_open = false;
        }
    }
}

// --- result routing ------------------------------------------------------------------

/// Applies an admin HTTP result (called from the main update loop).
pub fn handle_admin_result(
    app: &mut RestrustApp,
    op: AdminOp,
    result: Result<ehttp::Response, String>,
) {
    let mut toast: Option<String> = None;
    let mut unauthorized = false;
    // Follow-up list fetches are queued here and issued after the lock is
    // released (fetching while holding it would deadlock).
    let mut follow_ups: Vec<(AdminOp, ehttp::Request)> = Vec::new();
    {
        let mut admin = app.admin.lock().unwrap();
        let rejection = |err: &api::ApiError| {
            Banner {
                severity: Severity::Danger,
                message: format!(
                    "The server rejected this action: {}. Refresh and try again — the list may be out of date.",
                    err.server_message()
                ),
                actions: vec![BannerAction::Refresh],
            }
        };
        let ok_body = |result: &Result<ehttp::Response, String>| {
            api::parse_ok(result, |r| Ok(r.text().unwrap_or("").to_string()))
        };
        match op {
            AdminOp::ListUsers => {
                admin.loading_users = false;
                match api::parse_ok(&result, |r| api::parse_admin_users(r.text().unwrap_or(""))) {
                    Ok(users) => {
                        admin.users = users;
                        admin.users_loaded = true;
                        admin.users_error = None;
                    }
                    Err(api::ApiError::Unauthorized) => unauthorized = true,
                    Err(err) => {
                        admin.users_error = Some(err.server_message());
                        admin.banners.push(rejection(&err));
                    }
                }
            }
            AdminOp::CreateUser => match ok_body(&result) {
                Ok(_) => {
                    toast = Some("User created".to_string());
                    follow_ups.push((
                        AdminOp::ListUsers,
                        api::admin_users_list(&admin.base, &admin.token),
                    ));
                }
                Err(api::ApiError::Unauthorized) => unauthorized = true,
                Err(err) => admin.banners.push(rejection(&err)),
            },
            AdminOp::PatchUser { ref user_id } | AdminOp::ResetPassword { ref user_id } => {
                admin.row_busy.remove(user_id);
                match ok_body(&result) {
                    Ok(_) => {
                        if matches!(op, AdminOp::ResetPassword { .. }) {
                            toast = Some("Password reset".to_string());
                        }
                        follow_ups.push((
                            AdminOp::ListUsers,
                            api::admin_users_list(&admin.base, &admin.token),
                        ));
                    }
                    Err(api::ApiError::Unauthorized) => unauthorized = true,
                    Err(err) => admin.banners.push(rejection(&err)),
                }
            }
            AdminOp::ListOrgs => {
                admin.loading_orgs = false;
                match api::parse_ok(&result, |r| api::parse_admin_orgs(r.text().unwrap_or(""))) {
                    Ok(orgs) => {
                        admin.orgs = orgs;
                        admin.orgs_loaded = true;
                        admin.selected_org = admin.selected_org.filter(|i| *i < admin.orgs.len());
                        if let Some(index) = admin.selected_org {
                            let members = admin.orgs[index]
                                .members
                                .iter()
                                .map(|m| MemberRow {
                                    username: m.username.clone(),
                                    role: m.role.clone(),
                                    removed: false,
                                })
                                .collect();
                            admin.member_draft = members;
                        }
                    }
                    Err(api::ApiError::Unauthorized) => unauthorized = true,
                    Err(err) => admin.banners.push(rejection(&err)),
                }
            }
            AdminOp::CreateOrg => match ok_body(&result) {
                Ok(_) => {
                    toast = Some("Organization created".to_string());
                    follow_ups.push((
                        AdminOp::ListOrgs,
                        api::admin_orgs_list(&admin.base, &admin.token),
                    ));
                }
                Err(api::ApiError::Unauthorized) => unauthorized = true,
                Err(err) => admin.banners.push(rejection(&err)),
            },
            AdminOp::PutMembers { .. } => match ok_body(&result) {
                Ok(_) => {
                    toast = Some("Members saved".to_string());
                    follow_ups.push((
                        AdminOp::ListOrgs,
                        api::admin_orgs_list(&admin.base, &admin.token),
                    ));
                }
                Err(api::ApiError::Unauthorized) => unauthorized = true,
                Err(err) => admin.banners.push(rejection(&err)),
            },
        }
    }
    for (op, req) in follow_ups {
        issue_admin(&app.admin, &app.services, op, req);
    }
    if unauthorized {
        app.handle_unauthorized();
    }
    if let Some(toast) = toast {
        app.state.push_toast(toast);
    }
}
