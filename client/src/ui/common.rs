//! Shared UI widgets: method chips, status pills, badges, the key/value grid
//! editor, primary/danger buttons (ui-spec §1.3, §2.5, §4.3).

use egui::{Button, Color32, Response, RichText, Sense, Stroke, TextEdit, Ui, Vec2};

use crate::theme::{self, ThemeTokens};
use restrust_core::model::{HttpMethod, KeyValue};

/// Theme tokens matching the egui style's dark-mode flag (kept in sync by
/// `theme::apply_theme`).
pub fn current_tokens(ui: &Ui) -> ThemeTokens {
    if ui.visuals().dark_mode {
        theme::DARK
    } else {
        theme::LIGHT
    }
}

/// Alpha-modulated color (`Color32::gamma_multiply` is the visually-correct
/// multiplier).
pub fn linear_alpha(color: Color32, a: f32) -> Color32 {
    color.gamma_multiply(a.clamp(0.0, 1.0))
}

/// Method chip: colored text on a transparent bg with a 40%-alpha border
/// (30×16 logical px, §1.3).
pub fn method_chip(ui: &mut Ui, method: HttpMethod) -> Response {
    let t = current_tokens(ui);
    let color = theme::method_color(method, t.dark);
    let text = RichText::new(method.as_str())
        .size(10.0)
        .color(color)
        .strong();
    let resp = ui.add_sized([30.0, 16.0], egui::Label::new(text).selectable(false));
    ui.painter().rect_stroke(
        resp.rect,
        4.0,
        Stroke::new(1.0, linear_alpha(color, 0.4)),
        egui::StrokeKind::Middle,
    );
    resp
}

/// Filled status pill (`200 OK`, `Network error`): colored fill + white text,
/// fully rounded (§2.6).
pub fn status_pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let t = current_tokens(ui);
    let text = RichText::new(text).size(11.0).color(t.on_accent).strong();
    ui.add(
        Button::new(text)
            .fill(color)
            .corner_radius(8)
            .min_size(Vec2::new(0.0, 18.0)),
    )
}

/// Small outlined gray badge, e.g. the `revoked` pill (§6.2).
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    let text = RichText::new(text).size(10.0).color(color);
    let resp = ui.add(
        Button::new(text)
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, linear_alpha(color, 0.5)))
            .corner_radius(8)
            .min_size(Vec2::new(0.0, 14.0)),
    );
    let _ = resp;
}

/// Section heading (16 px, bold).
pub fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(16.0).strong());
}

/// Dialog section label (12 px, semi-bold muted).
pub fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(12.0)
            .strong()
            .color(current_tokens(ui).muted),
    );
}

/// Muted secondary text.
pub fn muted(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(12.0)
            .color(current_tokens(ui).muted),
    );
}

/// The primary (accent-filled) button, e.g. Send / Import / Save.
pub fn primary_button(ui: &mut Ui, label: &str, min_width: f32) -> Response {
    let t = current_tokens(ui);
    ui.add(
        Button::new(RichText::new(label).color(t.on_accent).strong())
            .fill(t.accent)
            .stroke(Stroke::NONE)
            .min_size(Vec2::new(min_width, theme::HEIGHT_CONTROL))
            .corner_radius(4),
    )
}

/// Danger (destructive action) button.
pub fn danger_button(ui: &mut Ui, label: &str) -> Response {
    let t = current_tokens(ui);
    ui.add(
        Button::new(RichText::new(label).color(t.on_accent).strong())
            .fill(t.danger)
            .stroke(Stroke::NONE)
            .min_size(Vec2::new(0.0, theme::HEIGHT_CONTROL))
            .corner_radius(4),
    )
}

/// A button that disables and shows an inline 14 px spinner while `busy`
/// (§5.1). `busy_label` is the gerund form (`Sending…`).
pub fn busy_button(ui: &mut Ui, label: &str, busy_label: &str, busy: bool) -> Response {
    ui.horizontal(|ui| {
        let resp = ui.add_enabled(
            !busy,
            Button::new(if busy { busy_label } else { label })
                .min_size(Vec2::new(0.0, theme::HEIGHT_CONTROL)),
        );
        if busy {
            ui.add(egui::Spinner::new().size(14.0));
        }
        resp
    })
    .inner
}

/// Dialog frame: `panel_raised` bg, radius 8, padding 16 (ui-spec §3).
pub fn dialog_frame(t: &ThemeTokens) -> egui::Frame {
    egui::Frame::NONE
        .fill(t.panel_raised)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(16))
}

/// Monospace single-line text edit filling the remaining width.
pub fn mono_field(ui: &mut Ui, value: &mut String, hint: &str) -> Response {
    ui.add(
        TextEdit::singleline(value)
            .hint_text(hint)
            .font(egui::TextStyle::Monospace)
            .desired_width(ui.available_width()),
    )
}

/// Key/value grid editor (Params/Headers/Body rows, §2.5): columns
/// `Key | Value | Description | ✓` plus a remove `✕`, footer `＋ Add row`.
/// Disabled rows render struck-through gray. `on_edit` fires whenever rows
/// changed (for dirty tracking).
pub fn kv_editor(
    ui: &mut Ui,
    id: &str,
    rows: &mut Vec<KeyValue>,
    with_description: bool,
    on_edit: &mut impl FnMut(),
) {
    let t = current_tokens(ui);
    egui::Grid::new(format!("kv_{id}"))
        .num_columns(5)
        .spacing([theme::GAP_INNER, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Key").size(12.0).color(t.muted));
            ui.label(RichText::new("Value").size(12.0).color(t.muted));
            if with_description {
                ui.label(RichText::new("Description").size(12.0).color(t.muted));
            } else {
                ui.label("");
            }
            ui.label(RichText::new("✓").size(12.0).color(t.muted));
            ui.label("");
            ui.end_row();

            let mut remove: Option<usize> = None;
            for (index, row) in rows.iter_mut().enumerate() {
                let mut key = row.key.clone();
                let mut value = row.value.clone();
                let mut description = row.description.clone().unwrap_or_default();
                let color = if row.enabled { t.text } else { t.revoked_gray };

                let key_resp = ui.add(
                    TextEdit::singleline(&mut key)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .text_color(color),
                );
                let value_resp = ui.add(
                    TextEdit::singleline(&mut value)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .text_color(color),
                );
                if with_description {
                    ui.add(
                        TextEdit::singleline(&mut description)
                            .desired_width(f32::INFINITY)
                            .text_color(color),
                    );
                } else {
                    ui.label("");
                }
                if ui.checkbox(&mut row.enabled, "").changed() {
                    on_edit();
                }
                if ui
                    .add(Button::new(RichText::new("✕").size(12.0).color(t.muted)))
                    .clicked()
                {
                    remove = Some(index);
                }
                ui.end_row();

                if !row.enabled {
                    for rect in [key_resp.rect, value_resp.rect] {
                        ui.painter().line_segment(
                            [rect.left_center(), rect.right_center()],
                            Stroke::new(1.0, t.revoked_gray),
                        );
                    }
                }
                if key_resp.changed() || value_resp.changed() {
                    row.key = key;
                    row.value = value;
                    on_edit();
                }
                if with_description {
                    row.description = if description.trim().is_empty() {
                        None
                    } else {
                        Some(description)
                    };
                }
            }
            if let Some(index) = remove {
                rows.remove(index);
                on_edit();
            }
        });
    if ui
        .add(Button::new(RichText::new("＋ Add row").size(12.0)))
        .clicked()
    {
        rows.push(KeyValue::new("", ""));
        on_edit();
    }
}

/// Bulk-edit textarea representation (`key:value` per line, blank line = row
/// separator, §2.5).
pub fn rows_to_bulk(rows: &[KeyValue]) -> String {
    rows.iter()
        .map(|r| format!("{}:{}", r.key, r.value))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parses the bulk textarea back into rows, preserving the enable flag and
/// description of rows whose key still matches.
pub fn bulk_to_rows(bulk: &str, template: &[KeyValue]) -> Vec<KeyValue> {
    bulk.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (key, value) = line.split_once(':').unwrap_or((line, ""));
            let key = key.trim().to_string();
            let mut row = KeyValue {
                value: value.trim().to_string(),
                enabled: true,
                description: None,
                key,
            };
            if let Some(old) = template.iter().find(|r| r.key == row.key) {
                row.enabled = old.enabled;
                row.description.clone_from(&old.description);
            }
            row
        })
        .collect()
}

/// True when no popup/menu is open and no text field has keyboard focus —
/// gates global shortcuts like Enter/F2.
pub fn global_keys_available(ctx: &egui::Context) -> bool {
    !ctx.egui_wants_keyboard_input() && !egui::Popup::is_any_open(ctx)
}

/// Small icon-only button with a tooltip.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    ui.add(Button::new(RichText::new(icon).size(14.0)))
        .on_hover_text(tooltip)
}

/// Ghost toggle button that shows a selected background when active.
pub fn ghost_toggle(ui: &mut Ui, icon: &str, active: bool, tooltip: &str) -> Response {
    ui.add(Button::new(RichText::new(icon).size(14.0)).selected(active))
        .on_hover_text(tooltip)
}

/// A full-row clickable area with hover/selected fills, for tree and history
/// rows.
pub fn row_sense(
    ui: &mut Ui,
    selected: bool,
    hover_fill: Color32,
    selected_fill: Color32,
) -> Response {
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click());
    let fill = if selected {
        selected_fill
    } else if resp.hovered() {
        hover_fill
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(resp.rect, 0.0, fill);
    }
    resp
}
