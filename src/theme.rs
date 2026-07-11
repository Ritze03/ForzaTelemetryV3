//! "Graphite" theme — role-based colour tokens + egui style setup.
//! Adapted from the Ritz launcher's theme so the whole app re-themes from one place.
//! Reference colours by *role* (PANEL, ACCENT, DIM…), never hard-code hex at call sites.
//!
//! Semantic data colours (tyre temps, input bars, warning greens/reds) intentionally
//! live at their call sites — those encode meaning, not chrome.

use egui::{Color32, CornerRadius, FontId, RichText, Stroke, TextStyle};

// ---- Brand / chrome ------------------------------------------------------

/// Brand accent (indigo). Selected tabs, primary action, section labels.
pub const ACCENT: Color32 = Color32::from_rgb(0x5B, 0x8B, 0xF0);
/// Title-bar / top-panel background (darker than panels).
pub const HEAD: Color32 = Color32::from_rgb(0x14, 0x17, 0x1A);
/// Main panel / column background.
pub const PANEL: Color32 = Color32::from_rgb(0x1E, 0x21, 0x25);
/// Footer / status band background.
pub const PANEL2: Color32 = Color32::from_rgb(0x19, 0x1C, 0x1F);
/// All hairline borders / dividers.
pub const BORDER: Color32 = Color32::from_rgb(0x2C, 0x30, 0x36);
/// Primary text.
pub const TEXT: Color32 = Color32::from_rgb(0xE7, 0xE9, 0xEC);
/// Secondary text.
pub const DIM: Color32 = Color32::from_rgb(0x96, 0x9C, 0xA6);
/// Tertiary text / labels / placeholders.
pub const FAINT: Color32 = Color32::from_rgb(0x64, 0x6A, 0x73);
/// Input & code-block background.
pub const FIELD: Color32 = Color32::from_rgb(0x15, 0x18, 0x1B);
/// Secondary button background.
pub const BTN: Color32 = Color32::from_rgb(0x26, 0x2A, 0x30);
/// Button border.
pub const BTNBD: Color32 = Color32::from_rgb(0x34, 0x39, 0x41);
/// Text on a primary (accent) button.
pub const PRIMARY_TEXT: Color32 = Color32::from_rgb(0x0B, 0x12, 0x22);
/// Destructive / danger (delete, stop, disconnect).
pub const DANGER: Color32 = Color32::from_rgb(0xE1, 0x55, 0x54);
/// Positive / connected (green).
pub const GOOD: Color32 = Color32::from_rgb(0x6C, 0xC5, 0x51);

// ---- Steel scale (dashboard neutrals) -------------------------------------
// Widget chrome uses these blue-tinted neutrals instead of pure grays so the
// dashboard leans toward the indigo accent. `lum` tracks the `from_gray` value
// it replaces (green channel = lum keeps perceived brightness ~equal).

/// Blue-tinted neutral: r ≈ 0.9·lum, g = lum, b ≈ 1.17·lum (saturating).
pub const fn steel(lum: u8) -> Color32 {
    let b = lum as u16 + lum as u16 / 6;
    let b = if b > 255 { 255 } else { b };
    Color32::from_rgb(lum - lum / 10, lum, b as u8)
}

/// Secondary widget text: unit labels, legends, muted values (was gray ~140–160).
pub const TEXT_DIM: Color32 = steel(150);
/// Faint hint / caption text: placeholders, sub-labels (was gray ~90–120).
pub const TEXT_FAINT: Color32 = steel(105);
/// Hairline gridlines & crosshairs inside gauges/plots (was gray ~45–55).
pub const STROKE_DIM: Color32 = steel(50);
/// Rims & rings around gauges (was gray ~80).
pub const STROKE_MID: Color32 = steel(80);
/// Recessed track behind bars & sliders (was gray ~40).
pub const TRACK: Color32 = steel(40);
/// Dark circular gauge well (was gray ~20–28).
pub const WELL: Color32 = steel(24);

// Derived selection / hover tints (premultiplied — const-friendly).
pub const SEL: Color32 = Color32::from_rgba_premultiplied(0x0F, 0x16, 0x27, 0x29);
pub const SELBD: Color32 = Color32::from_rgba_premultiplied(0x26, 0x3A, 0x65, 0x6B);
pub const HOV: Color32 = Color32::from_rgba_premultiplied(0x0D, 0x0D, 0x0D, 0x0D);

// ---- Button variants -----------------------------------------------------

/// Primary action: solid accent fill, dark bold text.
pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).color(PRIMARY_TEXT).strong())
        .fill(ACCENT)
        .stroke(Stroke::new(1.0, ACCENT))
}

/// Destructive: transparent fill, red text + faint red border.
pub fn danger_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).color(DANGER))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xE1, 0x55, 0x54, 82)))
}

/// Secondary/neutral action: btn fill + border.
pub fn secondary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).color(TEXT))
        .fill(BTN)
        .stroke(Stroke::new(1.0, BTNBD))
}

/// An UPPERCASE section label in the accent colour.
pub fn section_label(text: &str) -> RichText {
    RichText::new(text.to_uppercase()).color(ACCENT).size(12.0).strong()
}

// ---- Apply ---------------------------------------------------------------

/// Install the Graphite visuals + type scale. Call once at startup.
pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();

    v.override_text_color = Some(TEXT);
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.extreme_bg_color = FIELD;
    v.faint_bg_color = HOV;
    v.hyperlink_color = ACCENT;

    v.selection.bg_fill = SEL;
    v.selection.stroke = Stroke::new(1.0, SELBD);

    let round = CornerRadius::same(7);

    // Non-interactive surfaces (labels, separators, group frames).
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, DIM);
    v.widgets.noninteractive.corner_radius = round;

    // Resting interactive widgets.
    v.widgets.inactive.bg_fill = BTN;
    v.widgets.inactive.weak_bg_fill = BTN;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BTNBD);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.corner_radius = round;

    // Hovered.
    v.widgets.hovered.bg_fill = BTN;
    v.widgets.hovered.weak_bg_fill = HOV;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.corner_radius = round;

    // Active / pressed.
    v.widgets.active.bg_fill = BTN;
    v.widgets.active.weak_bg_fill = HOV;
    v.widgets.active.bg_stroke = Stroke::new(1.0, SELBD);
    v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.active.corner_radius = round;

    // Open (combo boxes, menus).
    v.widgets.open.bg_fill = FIELD;
    v.widgets.open.weak_bg_fill = FIELD;
    v.widgets.open.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.open.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.open.corner_radius = round;

    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        use egui::FontFamily::{Monospace, Proportional};
        s.text_styles = [
            (TextStyle::Heading, FontId::new(18.0, Proportional)),
            (TextStyle::Body, FontId::new(13.0, Proportional)),
            (TextStyle::Button, FontId::new(13.0, Proportional)),
            (TextStyle::Small, FontId::new(11.0, Proportional)),
            (TextStyle::Monospace, FontId::new(12.0, Monospace)),
        ]
        .into();
        s.spacing.button_padding = egui::vec2(9.0, 5.0);
        s.spacing.interact_size.y = 22.0;
    });
}
