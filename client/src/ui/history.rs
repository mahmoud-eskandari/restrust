//! History list (ui-spec §1.2 item 6, §2.4): recent requests with method chip,
//! host+path, relative time; context menu and inline clear confirm.

use eframe::egui::{self, RichText, Ui};

use crate::app::RestrustApp;
use crate::state::host_and_path;
use crate::theme;
use crate::ui::common::{current_tokens, method_chip, muted};

pub fn draw(app: &mut RestrustApp, ui: &mut Ui) {
    let t = current_tokens(ui);
    egui::Frame::NONE
        .fill(t.panel)
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            // Header row: `History` + hover-revealed `Clear…` (§1.2).
            ui.set_min_height(theme::HEADER_ROW_HEIGHT);
            ui.horizontal(|ui| {
                ui.label(RichText::new("History").strong().size(12.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let clear_confirm = ui.ctx().data(|d| {
                        d.get_temp::<bool>(egui::Id::unique("history_clear_confirm"))
                            .unwrap_or(false)
                    });
                    if clear_confirm {
                        // Inline confirm popover (§2.4).
                        if ui.button("Cancel").clicked() {
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(egui::Id::unique("history_clear_confirm"), false)
                            });
                        }
                        if ui
                            .add(egui::Button::new(RichText::new("Clear").color(t.danger)))
                            .clicked()
                        {
                            app.state.doc.history.clear();
                            app.state.mark_edited();
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(egui::Id::unique("history_clear_confirm"), false)
                            });
                        }
                    } else if ui
                        .add(egui::Button::new(RichText::new("Clear…").size(12.0)).small())
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::unique("history_clear_confirm"), true)
                        });
                    }
                });
            });
            ui.separator();

            if app.state.doc.history.is_empty() {
                empty(ui);
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("history_scroll")
                .auto_shrink(false)
                .show(ui, |ui| {
                    // Newest first (snapshot: rows mutate app state on click).
                    let entries: Vec<_> = app.state.doc.history.iter().rev().cloned().collect();
                    for entry in &entries {
                        let row_resp =
                            crate::ui::common::row_sense(ui, false, t.hover_row, t.selected_row);
                        ui.allocate_ui_with_layout(
                            egui::vec2(row_resp.rect.width(), theme::TREE_ROW_HEIGHT),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add_space(theme::GAP_TIGHT);
                                method_chip(ui, entry.request.method);
                                ui.label(
                                    RichText::new(format!(
                                        "{} · {}",
                                        host_and_path(&entry.request.url),
                                        relative(&entry.time)
                                    ))
                                    .size(12.0),
                                );
                            },
                        );
                        if row_resp.clicked() {
                            // Opens the request in a new tab, no cached response.
                            app.state.open_scratch(entry.request.clone());
                        }
                        entry_context_menu(app, &row_resp, entry);
                    }
                });
        });
}

fn relative(rfc3339: &str) -> String {
    crate::state::relative_time(rfc3339)
}

fn empty(ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(theme::GAP_STACK);
        ui.label(RichText::new("No history yet").strong());
        muted(ui, "Sent requests appear here.");
    });
}

fn entry_context_menu(
    app: &mut RestrustApp,
    resp: &egui::Response,
    entry: &restrust_core::model::HistoryEntry,
) {
    resp.clone().context_menu(|ui| {
        if ui.button("Open in new tab").clicked() {
            app.state.open_scratch(entry.request.clone());
        }
        if ui.button("Copy as cURL").clicked() {
            let curl = restrust_interchange::curl::export_curl(&entry.request);
            app.services.clipboard.set_text(&curl);
            app.state.push_toast("Copied".to_string());
        }
        if ui.button("Clear history…").clicked() {
            // Surfaces the inline confirm popover under the header (§2.4).
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::unique("history_clear_confirm"), true));
        }
    });
}
