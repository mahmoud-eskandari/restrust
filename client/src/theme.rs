//! Theme tokens (mydocs/design/ui-spec.md §4), spacing helpers and font setup.
//!
//! The dark palette is the default. `Vazirmatn` is installed as a *fallback*
//! glyph source in both the proportional and monospace families so Persian
//! content (URLs, bodies, responses) renders instead of showing tofu, while
//! the egui default fonts stay primary (ui-spec §4.5).

use egui::{Color32, Context, FontData, FontDefinitions, FontFamily, Visuals};
use restrust_core::model::HttpMethod;

/// The bundled Persian font, registered under this name.
const VAZIRMATN: &[u8] = include_bytes!("../assets/fonts/Vazirmatn-Regular.ttf");

/// All color tokens of one theme (ui-spec §4.1 / §4.2).
#[derive(Clone, Copy, Debug)]
pub struct ThemeTokens {
    pub bg: Color32,
    pub panel: Color32,
    pub panel_raised: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub info: Color32,
    pub revoked_gray: Color32,
    pub selected_row: Color32,
    pub hover_row: Color32,
    /// True when this is the dark palette (method colors differ, §4.3).
    pub dark: bool,
}

/// The dark palette (default).
pub const DARK: ThemeTokens = ThemeTokens {
    bg: Color32::from_rgb(0x12, 0x14, 0x18),
    panel: Color32::from_rgb(0x1B, 0x1E, 0x23),
    panel_raised: Color32::from_rgb(0x22, 0x26, 0x2C),
    border: Color32::from_rgb(0x30, 0x34, 0x3B),
    text: Color32::from_rgb(0xE8, 0xEA, 0xED),
    muted: Color32::from_rgb(0x9A, 0xA1, 0xAB),
    accent: Color32::from_rgb(0xFF, 0x6C, 0x37),
    accent_hover: Color32::from_rgb(0xFF, 0x85, 0x57),
    on_accent: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    success: Color32::from_rgb(0x3F, 0xB9, 0x50),
    warning: Color32::from_rgb(0xD2, 0x99, 0x22),
    danger: Color32::from_rgb(0xF8, 0x51, 0x49),
    info: Color32::from_rgb(0x58, 0xA6, 0xFF),
    revoked_gray: Color32::from_rgb(0x6E, 0x74, 0x80),
    selected_row: Color32::from_rgb(0x26, 0x2A, 0x31),
    hover_row: Color32::from_rgb(0x21, 0x24, 0x2A),
    dark: true,
};

/// The light palette.
pub const LIGHT: ThemeTokens = ThemeTokens {
    bg: Color32::from_rgb(0xF4, 0xF5, 0xF7),
    panel: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    panel_raised: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    border: Color32::from_rgb(0xD8, 0xDC, 0xE1),
    text: Color32::from_rgb(0x1F, 0x23, 0x28),
    muted: Color32::from_rgb(0x6A, 0x73, 0x7D),
    accent: Color32::from_rgb(0xD9, 0x48, 0x0F),
    accent_hover: Color32::from_rgb(0xB5, 0x38, 0x0D),
    on_accent: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    success: Color32::from_rgb(0x1A, 0x7F, 0x37),
    warning: Color32::from_rgb(0x9A, 0x67, 0x00),
    danger: Color32::from_rgb(0xCF, 0x22, 0x2E),
    info: Color32::from_rgb(0x09, 0x69, 0xDA),
    revoked_gray: Color32::from_rgb(0x8B, 0x94, 0x9E),
    selected_row: Color32::from_rgb(0xEA, 0xEE, 0xF2),
    hover_row: Color32::from_rgb(0xF0, 0xF2, 0xF5),
    dark: false,
};

/// Tokens for a theme name (`"dark"` / `"light"`); anything else falls back to
/// the default dark palette.
pub fn tokens(theme_name: &str) -> ThemeTokens {
    match theme_name {
        "light" => LIGHT,
        _ => DARK,
    }
}

/// Applies the palette named by `theme_name` to the egui context.
pub fn apply_theme(ctx: &Context, theme_name: &str) {
    let t = tokens(theme_name);
    // Switch the active theme first so `set_visuals` targets the right style
    // and `Context::theme()` reports the mode our tokens assume.
    ctx.set_theme(if t.dark {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    });
    let mut visuals = if t.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    visuals.panel_fill = t.panel;
    visuals.window_fill = t.panel_raised;
    visuals.extreme_bg_color = t.bg;
    visuals.faint_bg_color = t.bg;
    visuals.code_bg_color = t.bg;
    visuals.hyperlink_color = t.accent;
    // Selection + widget accenting (ui-spec: accent drives Send/selection/focus).
    visuals.selection.bg_fill = t.accent;
    visuals.selection.stroke = egui::Stroke::new(1.0, t.on_accent);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, t.muted);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, t.text);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, t.text);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, t.text);
    visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0, t.accent);
    ctx.set_visuals(visuals);
}

/// Installs the Vazirmatn fallback font (call once at startup).
pub fn install_fonts(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("vazirmatn".into(), FontData::from_static(VAZIRMATN).into());
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("vazirmatn".into());
    }
    ctx.set_fonts(fonts);
}

/// Method-chip color per §4.3 (colored text on transparent bg with a 40%-alpha
/// border is drawn by the chip widget itself).
pub fn method_color(method: HttpMethod, dark: bool) -> Color32 {
    match method {
        HttpMethod::Get => Color32::from_rgb(0x58, 0xA6, 0xFF),
        HttpMethod::Post => Color32::from_rgb(0x3F, 0xB9, 0x50),
        HttpMethod::Put => Color32::from_rgb(0xD2, 0x99, 0x22),
        HttpMethod::Patch => {
            if dark {
                Color32::from_rgb(0xA3, 0x71, 0xF7)
            } else {
                Color32::from_rgb(0x82, 0x50, 0xDF)
            }
        }
        HttpMethod::Delete => {
            if dark {
                Color32::from_rgb(0xF8, 0x51, 0x49)
            } else {
                Color32::from_rgb(0xCF, 0x22, 0x2E)
            }
        }
        HttpMethod::Head | HttpMethod::Options => {
            if dark {
                Color32::from_rgb(0x9A, 0xA1, 0xAB)
            } else {
                Color32::from_rgb(0x6A, 0x73, 0x7D)
            }
        }
    }
}

/// Status-class color for a response status code (§4.3): 1xx muted, 2xx
/// success, 3xx info, 4xx warning, 5xx danger.
pub fn status_class_color(status: u16, t: &ThemeTokens) -> Color32 {
    match status {
        100..=199 => t.muted,
        200..=299 => t.success,
        300..=399 => t.info,
        400..=499 => t.warning,
        _ => t.danger,
    }
}

// --- spacing / sizing helpers (§4.4) -----------------------------------------

/// Tight gap between an icon and its label.
pub const GAP_TIGHT: f32 = 4.0;
/// Standard inner padding-x of controls / table cell padding.
pub const GAP_INNER: f32 = 8.0;
/// Related control stacks.
pub const GAP_STACK: f32 = 8.0;
/// Dialog padding, section gaps, group separation.
pub const GAP_SECTION: f32 = 16.0;

/// Small control height (chips, table toggles).
pub const HEIGHT_SMALL: f32 = 22.0;
/// Standard control height (buttons, text fields, combos).
pub const HEIGHT_CONTROL: f32 = 28.0;
/// Primary/large (Send) control height.
pub const HEIGHT_PRIMARY: f32 = 32.0;

/// Fixed top bar height.
pub const TOP_BAR_HEIGHT: f32 = 40.0;
/// Fixed status bar height.
pub const STATUS_BAR_HEIGHT: f32 = 24.0;
/// Request tab bar height.
pub const TAB_BAR_HEIGHT: f32 = 32.0;
/// Section/response tab bar height.
pub const SECTION_BAR_HEIGHT: f32 = 30.0;
/// URL row height.
pub const URL_ROW_HEIGHT: f32 = 36.0;
/// Tree / history row height.
pub const TREE_ROW_HEIGHT: f32 = 26.0;
/// Sidebar header row height.
pub const HEADER_ROW_HEIGHT: f32 = 28.0;
/// Response meta row height.
pub const RESPONSE_META_HEIGHT: f32 = 28.0;

/// Human-readable byte size (`412 B`, `1.4 KB`, `2.1 MB`).
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < MB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / MB)
    }
}
