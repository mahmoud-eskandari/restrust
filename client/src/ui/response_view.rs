//! Response area (ui-spec §2.6): status pill, time/size, recent dropdown,
//! Body (Pretty/Raw/Preview) and Headers tabs, copy/save actions.

use eframe::egui::{self, RichText, Ui};

use crate::app::RestrustApp;
use crate::state::{BodyView, CachedResponse, ResponseTab};
use crate::theme::{self, human_size};
use crate::ui::common::{current_tokens, icon_button, status_pill};

pub fn draw(app: &mut RestrustApp, ui: &mut Ui, _tab_id: u64) {
    let t = current_tokens(ui);
    egui::Frame::NONE
        .fill(t.bg)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            let Some(tab) = app.state.active_tab() else {
                return;
            };
            if app.state.sending_tabs.contains(&tab.id) {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Sending request…");
                });
                return;
            }
            if let Some(error) = &tab.send_error {
                ui.colored_label(t.danger, "Request failed");
                ui.label(error);
                return;
            }
            let Some(cached) = tab.displayed_response().cloned() else {
                empty(ui);
                return;
            };
            meta_row(app, ui, &cached);
            tabs_row(app, ui);
            egui::ScrollArea::both()
                .id_salt("response_scroll")
                .auto_shrink(false)
                .show(ui, |ui| body_or_headers(app, ui, &cached));
        });
}

/// The 28 px collapsed stub: meta row only (§1.1 response collapse).
pub fn collapsed_stub(app: &mut RestrustApp, ui: &mut Ui) {
    let t = current_tokens(ui);
    egui::Frame::NONE
        .fill(t.bg)
        .inner_margin(egui::Margin::symmetric(8, 0))
        .show(ui, |ui| {
            ui.set_min_height(theme::RESPONSE_META_HEIGHT);
            let Some(tab) = app.state.active_tab() else {
                return;
            };
            match tab.displayed_response().cloned() {
                Some(cached) => meta_row(app, ui, &cached),
                None => empty(ui),
            }
        });
}

fn empty(ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(theme::GAP_SECTION);
        ui.label(RichText::new("No responses yet").strong());
        ui.add_space(theme::GAP_TIGHT);
        crate::ui::common::muted(ui, "Send a request to see the response here (Ctrl+Enter).");
    });
}

#[allow(clippy::too_many_lines)]
fn meta_row(app: &mut RestrustApp, ui: &mut Ui, cached: &CachedResponse) {
    let t = current_tokens(ui);
    ui.horizontal(|ui| {
        ui.set_min_height(theme::RESPONSE_META_HEIGHT);
        let color = theme::status_class_color(cached.meta.status, &t);
        status_pill(
            ui,
            &format!("{} {}", cached.meta.status, cached.meta.reason),
            color,
        );
        ui.label(RichText::new(format!("{} ms", cached.meta.duration_ms)).size(12.0));
        ui.separator();
        ui.label(RichText::new(human_size(cached.meta.size)).size(12.0));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let collapsed = app
                .state
                .active_tab()
                .map(|tab| tab.response_collapsed)
                .unwrap_or(false);
            if icon_button(ui, if collapsed { "▸" } else { "▾" }, "Collapse response").clicked()
            {
                if let Some(tab) = app.state.active_tab_mut() {
                    tab.response_collapsed = !tab.response_collapsed;
                }
            }
            if ui.button("Save response").clicked() {
                app.state.dialog =
                    crate::state::DialogState::Export(Box::new(crate::state::ExportDialog {
                        collection_id: String::new(),
                        name: "response".to_string(),
                        format: crate::state::ExportFormat::RestrustJson,
                        output: String::from_utf8_lossy(&cached.body).to_string(),
                        path: default_response_path(),
                        error: None,
                    }));
            }
            if ui.button("Copy response").clicked() {
                let text = String::from_utf8_lossy(&cached.body).to_string();
                app.services.clipboard.set_text(&text);
                app.state.push_toast("Response copied".to_string());
            }
            // Recent ▾ (cache depth 10, §2.6).
            let count = app
                .state
                .active_tab()
                .map(|tab| tab.responses.len())
                .unwrap_or(0);
            if count > 1 {
                let entries: Vec<CachedResponse> = app
                    .state
                    .active_tab()
                    .map(|tab| tab.responses.iter().rev().cloned().collect())
                    .unwrap_or_default();
                let resp = ui.button("Recent ▾");
                egui::Popup::menu(&resp).show(|ui| {
                    ui.set_min_width(200.0);
                    for (index, entry) in entries.iter().enumerate() {
                        let time = entry
                            .meta
                            .time
                            .split('T')
                            .nth(1)
                            .unwrap_or("")
                            .trim_end_matches('Z')
                            .to_string();
                        if ui
                            .button(format!(
                                "{time} · {} · {} ms",
                                entry.meta.status, entry.meta.duration_ms
                            ))
                            .clicked()
                        {
                            if let Some(tab) = app.state.active_tab_mut() {
                                tab.response_index = Some(tab.responses.len() - 1 - index);
                            }
                            ui.close();
                        }
                    }
                });
            }
        });
    });
}

fn tabs_row(app: &mut RestrustApp, ui: &mut Ui) {
    let t = current_tokens(ui);
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let mut response_tab = app
            .state
            .active_tab()
            .map(|tab| tab.response_tab)
            .unwrap_or(ResponseTab::Body);
        let mut body_view = app
            .state
            .active_tab()
            .map(|tab| tab.body_view)
            .unwrap_or(BodyView::Pretty);
        let wrap = app.state.active_tab().map(|tab| tab.wrap).unwrap_or(true);

        if ui
            .selectable_label(
                response_tab == ResponseTab::Body,
                RichText::new("Body").strong(),
            )
            .clicked()
        {
            response_tab = ResponseTab::Body;
        }
        if ui
            .selectable_label(
                response_tab == ResponseTab::Headers,
                RichText::new("Headers").strong(),
            )
            .clicked()
        {
            response_tab = ResponseTab::Headers;
        }

        if response_tab == ResponseTab::Body {
            ui.separator();
            for (view, label) in [
                (BodyView::Pretty, "Pretty"),
                (BodyView::Raw, "Raw"),
                (BodyView::Preview, "Preview"),
            ] {
                if ui
                    .selectable_label(body_view == view, RichText::new(label).strong())
                    .clicked()
                {
                    body_view = view;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(RichText::new("Wrap").size(12.0)).selected(wrap))
                    .clicked()
                {
                    if let Some(tab) = app.state.active_tab_mut() {
                        tab.wrap = !tab.wrap;
                    }
                }
            });
        }
        let _ = t;
        if let Some(tab) = app.state.active_tab_mut() {
            tab.response_tab = response_tab;
            tab.body_view = body_view;
        }
    });
    ui.add_space(2.0);
}

fn body_or_headers(app: &mut RestrustApp, ui: &mut Ui, cached: &CachedResponse) {
    let response_tab = app
        .state
        .active_tab()
        .map(|tab| tab.response_tab)
        .unwrap_or(ResponseTab::Body);
    let body_view = app
        .state
        .active_tab()
        .map(|tab| tab.body_view)
        .unwrap_or(BodyView::Pretty);
    let wrap = app.state.active_tab().map(|tab| tab.wrap).unwrap_or(true);
    match response_tab {
        ResponseTab::Headers => {
            if cached.headers.is_empty() {
                crate::ui::common::muted(ui, "No response yet.");
                return;
            }
            egui::Grid::new("response_headers")
                .num_columns(2)
                .striped(true)
                .spacing([theme::GAP_INNER, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").strong());
                    ui.label(RichText::new("Value").strong());
                    ui.end_row();
                    for (name, value) in &cached.headers {
                        ui.label(RichText::new(name).size(12.0));
                        ui.label(
                            RichText::new(value)
                                .size(12.0)
                                .family(egui::FontFamily::Monospace),
                        );
                        ui.end_row();
                    }
                });
        }
        ResponseTab::Body => match body_view {
            BodyView::Pretty => {
                let text = String::from_utf8_lossy(&cached.body);
                match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(value) => pretty_json(ui, &value),
                    Err(_) => {
                        crate::ui::common::muted(ui, "Pretty view is for JSON — showing raw.");
                        raw(ui, &text, wrap);
                    }
                }
            }
            BodyView::Raw => {
                let text = String::from_utf8_lossy(&cached.body);
                raw(ui, &text, wrap);
            }
            BodyView::Preview => {
                crate::ui::common::muted(
                    ui,
                    "Preview is not available for this content type — showing raw.",
                );
                let text = String::from_utf8_lossy(&cached.body);
                raw(ui, &text, wrap);
            }
        },
    }
}

fn raw(ui: &mut Ui, text: &str, wrap: bool) {
    let mut buffer = text;
    let edit = egui::TextEdit::multiline(&mut buffer)
        .font(egui::TextStyle::Monospace)
        .desired_width(if wrap {
            ui.available_width()
        } else {
            f32::INFINITY
        });
    ui.add(edit);
}

/// Pretty JSON with minimal syntax coloring: keys `info`, strings `success`,
/// numbers `accent`, punctuation muted (§2.6). Rendered via a `LayoutJob`.
fn pretty_json(ui: &mut Ui, value: &serde_json::Value) {
    let t = current_tokens(ui);
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    emit_json(value, &mut job, &t, &font, 0);
    ui.add(egui::Label::new(job).selectable(true));
}

fn emit_json(
    value: &serde_json::Value,
    job: &mut egui::text::LayoutJob,
    t: &crate::theme::ThemeTokens,
    font: &egui::FontId,
    depth: usize,
) {
    let muted = |job: &mut egui::text::LayoutJob, text: &str| {
        job.append(text, 0.0, egui::TextFormat::simple(font.clone(), t.muted));
    };
    match value {
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                muted(job, "{}");
                return;
            }
            muted(job, "{\n");
            let last = map.len() - 1;
            for (index, (key, child)) in map.iter().enumerate() {
                job.append(
                    &"  ".repeat(depth + 1),
                    0.0,
                    egui::TextFormat::simple(font.clone(), t.muted),
                );
                job.append(
                    &serde_json::to_string(key).unwrap_or_default(),
                    0.0,
                    egui::TextFormat::simple(font.clone(), t.info),
                );
                muted(job, ": ");
                emit_json(child, job, t, font, depth + 1);
                muted(job, if index == last { "\n" } else { ",\n" });
            }
            job.append(
                &"  ".repeat(depth),
                0.0,
                egui::TextFormat::simple(font.clone(), t.muted),
            );
            muted(job, "}");
        }
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                muted(job, "[]");
                return;
            }
            muted(job, "[\n");
            let last = items.len() - 1;
            for (index, child) in items.iter().enumerate() {
                job.append(
                    &"  ".repeat(depth + 1),
                    0.0,
                    egui::TextFormat::simple(font.clone(), t.muted),
                );
                emit_json(child, job, t, font, depth + 1);
                muted(job, if index == last { "\n" } else { ",\n" });
            }
            job.append(
                &"  ".repeat(depth),
                0.0,
                egui::TextFormat::simple(font.clone(), t.muted),
            );
            muted(job, "]");
        }
        serde_json::Value::String(text) => job.append(
            &serde_json::to_string(text).unwrap_or_default(),
            0.0,
            egui::TextFormat::simple(font.clone(), t.success),
        ),
        serde_json::Value::Number(number) => job.append(
            &number.to_string(),
            0.0,
            egui::TextFormat::simple(font.clone(), t.accent),
        ),
        serde_json::Value::Bool(flag) => job.append(
            &flag.to_string(),
            0.0,
            egui::TextFormat::simple(font.clone(), t.accent),
        ),
        serde_json::Value::Null => muted(job, "null"),
    }
}

fn default_response_path() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::platform::default_export_path("response")
    }
    #[cfg(target_arch = "wasm32")]
    {
        String::new()
    }
}
