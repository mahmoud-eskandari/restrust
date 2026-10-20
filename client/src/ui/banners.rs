//! Banner strip (below the top bar) and bottom-right toasts (ui-spec §5).

use eframe::egui::{self, Context};

use crate::app::RestrustApp;
use crate::state::{BannerAction, Severity};
use crate::theme;
use crate::ui::common::linear_alpha;

/// The stacked banner strip; the panel only exists when banners are present
/// (§9: no reserved space).
pub fn strip(app: &mut RestrustApp, root: &mut egui::Ui) {
    if app.state.banners.is_empty() {
        return;
    }
    let banners = app.state.banners.clone();
    let t = current_tokens_ctx(root.ctx());
    egui::Panel::top("banner_strip")
        .frame(egui::Frame::NONE.fill(t.panel))
        .show(root, |ui| {
            let mut dismiss: Option<usize> = None;
            for (index, banner) in banners.iter().enumerate() {
                let color = match banner.severity {
                    Severity::Info => t.info,
                    Severity::Warning => t.warning,
                    Severity::Danger => t.danger,
                };
                let icon = match banner.severity {
                    Severity::Info => "i",
                    Severity::Warning => "⚠",
                    Severity::Danger => "!",
                };
                egui::Frame::NONE
                    .fill(linear_alpha(color, 0.10))
                    .corner_radius(0.0)
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            egui::Frame::NONE
                                .fill(color)
                                .corner_radius(2.0)
                                .show(ui, |ui| {
                                    ui.set_width(4.0);
                                    ui.set_height(18.0);
                                });
                            ui.label(egui::RichText::new(icon).strong().color(color));
                            let text = egui::RichText::new(&banner.message).size(12.0);
                            ui.add(egui::Label::new(text).wrap().truncate())
                                .on_hover_text(&banner.message);
                            for action in &banner.actions {
                                match action {
                                    BannerAction::RetrySend => {
                                        if ui.button("Retry").clicked() {
                                            app.send_active_request();
                                            dismiss = Some(index);
                                        }
                                    }
                                    BannerAction::RetrySync => {
                                        if ui.button("Retry").clicked() {
                                            app.retry_connection();
                                            dismiss = Some(index);
                                        }
                                    }
                                    BannerAction::Dismiss => {
                                        if ui.button("Dismiss").clicked() {
                                            dismiss = Some(index);
                                        }
                                    }
                                    BannerAction::SignIn => {
                                        if ui.button("Sign in…").clicked() {
                                            let server = app.state.doc.settings.server_url.clone();
                                            app.state.dialog = crate::state::DialogState::SignIn(
                                                Box::new(crate::state::SignInDialog::new(server)),
                                            );
                                            dismiss = Some(index);
                                        }
                                    }
                                    BannerAction::Refresh => {
                                        if ui.button("Refresh").clicked() {
                                            app.sync_now();
                                            dismiss = Some(index);
                                        }
                                    }
                                    BannerAction::ShowCollection(collection_id) => {
                                        if ui.button("Show collection").clicked() {
                                            app.state.selected = Some(crate::state::SelectedNode {
                                                collection_id: collection_id.clone(),
                                                node_id: None,
                                            });
                                            app.state.tabs.clear();
                                            app.state.active_tab = None;
                                            dismiss = Some(index);
                                        }
                                    }
                                }
                            }
                        });
                    });
            }
            if let Some(index) = dismiss {
                app.state.banners.remove(index);
            }
        });
}

/// Transient success toasts, bottom-right, 2.5 s fade, max 3 (§5.3).
pub fn toasts(app: &mut RestrustApp, ctx: &egui::Context) {
    if app.state.toasts.is_empty() {
        return;
    }
    let t = current_tokens_ctx(ctx);
    egui::Area::new(egui::Id::unique("toasts"))
        .order(egui::Order::Foreground)
        .anchor(
            egui::Align2::RIGHT_BOTTOM,
            [-theme::GAP_SECTION, -theme::GAP_SECTION],
        )
        .show(ctx, |ui| {
            let mut expired = 0usize;
            for toast in &app.state.toasts {
                if toast.born.elapsed().as_millis() >= 2500 {
                    expired += 1;
                    continue;
                }
                let border = if toast.success { t.success } else { t.accent };
                egui::Frame::NONE
                    .fill(t.panel_raised)
                    .stroke(egui::Stroke::new(1.0, t.border))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            egui::Frame::NONE
                                .fill(border)
                                .corner_radius(2.0)
                                .show(ui, |ui| {
                                    ui.set_width(4.0);
                                    ui.set_height(14.0);
                                });
                            ui.label(egui::RichText::new(&toast.message).size(12.0).strong());
                        });
                    });
            }
            // Hover pauses the fade by resetting the clock of visible toasts.
            if ui.rect_contains_pointer(ui.min_rect()) {
                for toast in &mut app.state.toasts {
                    toast.born = crate::time::Instant::now();
                }
            }
            if expired > 0 {
                app.state.toasts.drain(..expired);
            }
        });
}

fn current_tokens_ctx(ctx: &Context) -> crate::theme::ThemeTokens {
    if ctx.theme() == egui::Theme::Dark {
        theme::DARK
    } else {
        theme::LIGHT
    }
}
