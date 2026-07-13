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

/// A bordered card with a blue [`section_label`] title, followed by a uniform
/// 8px gap — the Co-Op tab's category styling, reused across tabs.
///
/// The 8px trailing space is the *only* inter-card gap, so callers must zero
/// the container's vertical item spacing (`ui.spacing_mut().item_spacing.y = 0.0`)
/// before stacking cards; otherwise egui adds its own spacing on top. The card
/// sets its own inner spacing, independent of that outer zero.
pub fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.group(|ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 4.0; // comfortable spacing inside the card
        ui.label(section_label(title));
        ui.add_space(4.0);
        body(ui);
    });
    ui.add_space(8.0);
}

// ---- Checkbox ------------------------------------------------------------

/// Outline of an unchecked box — light enough to read on the panel.
pub const CHECK_OUTLINE: Color32 = steel(150);

/// A checkbox styled like the Ritz launcher: an 18px rounded box (accent-filled
/// with a white check when on, hairline outline when off) followed by the label,
/// the whole row a single click target with a subtle hover wash. Drop-in
/// replacement for `crate::theme::styled_checkbox(ui, &mut x, label)` — returns the row's response.
pub fn styled_checkbox(
    ui: &mut egui::Ui,
    checked: &mut bool,
    label: impl Into<String>,
) -> egui::Response {
    const BOX: f32 = 18.0;
    const GAP: f32 = 7.0;
    let on = *checked;

    let font = TextStyle::Body.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(label.into(), font, TEXT);
    let gsize = galley.size();
    let size = egui::vec2(BOX + GAP + gsize.x, BOX.max(gsize.y));
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect.expand2(egui::vec2(4.0, 2.0)), CornerRadius::same(6), HOV);
        }
        let box_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - BOX / 2.0),
            egui::Vec2::splat(BOX),
        )
        .shrink(1.0);
        let round = CornerRadius::same(5);
        if on {
            painter.rect_filled(box_rect, round, ACCENT);
            let g = painter.layout_no_wrap(
                crate::icons::CHECK.to_owned(),
                FontId::proportional(11.0),
                Color32::WHITE,
            );
            // Centre on the glyph's ink, not its advance box (Nerd glyphs are offset).
            let ink = g
                .rows
                .first()
                .and_then(|r| r.glyphs.first())
                .map(|gl| gl.pos.to_vec2() + gl.uv_rect.offset + gl.uv_rect.size * 0.5)
                .unwrap_or_else(|| g.size() * 0.5);
            painter.galley(box_rect.center() - ink, g, Color32::WHITE);
        } else {
            painter.rect_stroke(
                box_rect,
                round,
                Stroke::new(1.5, CHECK_OUTLINE),
                egui::StrokeKind::Inside,
            );
        }
        painter.galley(
            egui::pos2(box_rect.right() + GAP, rect.center().y - gsize.y / 2.0),
            galley,
            TEXT,
        );
    }

    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    resp
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
