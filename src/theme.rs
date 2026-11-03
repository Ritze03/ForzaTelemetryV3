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
/// Caution / in-between state (muted pastel amber — e.g. gearbox not yet calibrated).
pub const WARN: Color32 = Color32::from_rgb(0xD8, 0xB4, 0x55);

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

/// Gray placeholder text for a text box's `hint_text`. Needed because the theme's
/// `override_text_color` otherwise paints the hint the same near-white as real text —
/// an explicit colour wins over the override.
pub fn placeholder(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(DIM)
}

// ---- Panes: columns & cards ---------------------------------------------
//
// The containment rule (docs/ui/STYLING-GUIDE.md → "Panes: everything stays in its
// container"): every page column and every card is a *pane* with a fixed width taken from
// its parent, and its contents are clipped to it. Why: egui grows a `Ui` (and every
// enclosing `Ui`) to fit a widget that is wider than the space it was given, and plain
// `ui.columns` doesn't clip — so one too-wide row (the Overlay tab's Drift Counter radios)
// used to widen its card past the column and paint over the neighbouring column. Inside a
// pane only the pane's own width counts: rows shrink, wrap or stack to it, and whatever
// still doesn't fit is cut at the pane edge instead of spilling into another pane.

/// A child `Ui` exactly `rect.width()` wide (top at `rect.top()`, growing down), painted
/// only inside `clip_x` horizontally. Its overflow never reaches the parent: the caller
/// allocates the fixed width itself.
fn pane_ui(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    clip_x: egui::Rangef,
    layout: egui::Layout,
    salt: impl std::hash::Hash,
) -> egui::Ui {
    let mut child = ui.new_child(egui::UiBuilder::new().id_salt(salt).max_rect(rect).layout(layout));
    child.set_width(rect.width()); // min == max: the pane neither shrinks nor grows
    let clip = child.clip_rect();
    child.shrink_clip_rect(egui::Rect::from_x_y_ranges(clip_x, clip.y_range()));
    child
}

/// Like `ui.columns`, but each column is a contained pane: exactly `1/n` of the width (minus
/// the `item_spacing.x` gaps), clipped to its own slot, and the row allocates exactly the
/// available width — a too-wide widget in one column can neither widen it nor paint into
/// the next. Use it for page columns *and* for the label | control halves of a row.
pub fn columns<R>(ui: &mut egui::Ui, n: usize, add: impl FnOnce(&mut [egui::Ui]) -> R) -> R {
    let n = n.max(1);
    let gap = ui.spacing().item_spacing.x;
    let total = ui.available_width().max(0.0);
    let col_w = ((total - gap * (n as f32 - 1.0)) / n as f32).max(0.0);
    let top_left = ui.cursor().min;
    let bottom = ui.max_rect().bottom().max(top_left.y);
    let salt = ui.next_auto_id();
    let mut cols: Vec<egui::Ui> = (0..n)
        .map(|i| {
            let x = top_left.x + i as f32 * (col_w + gap);
            let rect = egui::Rect::from_min_max(egui::pos2(x, top_left.y), egui::pos2(x + col_w, bottom));
            // Half the gap of slack each side: focus rings / hover washes stay visible, but
            // two neighbouring columns' clip rects never overlap.
            let clip_x = egui::Rangef::new(x - gap * 0.5, x + col_w + gap * 0.5);
            let layout = egui::Layout::top_down_justified(egui::Align::LEFT);
            pane_ui(ui, rect, clip_x, layout, (salt, "theme_col", i))
        })
        .collect();
    let r = add(&mut cols);
    let h = cols.iter().map(|c| c.min_rect().bottom() - top_left.y).fold(0.0_f32, f32::max);
    ui.advance_cursor_after_rect(egui::Rect::from_min_size(top_left, egui::vec2(total, h)));
    r
}

/// A bordered card with a blue [`section_label`] title, followed by a uniform
/// 8px gap — the Co-Op tab's category styling, reused across tabs.
///
/// The card is a pane: exactly as wide as its parent's available width (its column), and
/// its body is laid out in, and clipped to, the card's inner width — a row that is too
/// wide is cut at the card edge, it never widens the card. Size the body's rows from
/// `ui.available_width()` (the row helpers below all do).
///
/// The 8px trailing space is the *only* inter-card gap, so callers must zero
/// the container's vertical item spacing (`ui.spacing_mut().item_spacing.y = 0.0`)
/// before stacking cards; otherwise egui adds its own spacing on top. The card
/// sets its own inner spacing, independent of that outer zero.
pub fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    let frame = egui::Frame::group(ui.style());
    let margin = frame.total_margin();
    let inner_w = (ui.available_width() - margin.sum().x).max(0.0);
    frame.show(ui, |ui| {
        let top_left = ui.cursor().min;
        let rect = egui::Rect::from_min_max(
            top_left,
            egui::pos2(top_left.x + inner_w, ui.max_rect().bottom().max(top_left.y)),
        );
        // Clip just inside the frame's stroke, so hover washes in the inner margin show.
        let slack = (margin.left - frame.stroke.width - 1.0).max(0.0);
        let clip_x = egui::Rangef::new(rect.left() - slack, rect.right() + slack);
        let salt = ui.auto_id_with("theme_card");
        let mut body_ui = pane_ui(ui, rect, clip_x, *ui.layout(), salt);
        body_ui.spacing_mut().item_spacing.y = 4.0; // comfortable spacing inside the card
        body_ui.label(section_label(title));
        body_ui.add_space(4.0);
        body(&mut body_ui);
        let h = (body_ui.min_rect().bottom() - top_left.y).max(0.0);
        ui.advance_cursor_after_rect(egui::Rect::from_min_size(top_left, egui::vec2(inner_w, h)));
    });
    ui.add_space(8.0);
}

// ---- Segmented control ---------------------------------------------------

/// A pill-shaped segmented control: a fully-rounded dark container holding equal-width
/// text segments, the selected one a tinted, bordered pill (`SEL` fill, `SELBD` stroke,
/// `TEXT`), the rest dim text on the container that wash `HOV` on hover. Fills the
/// available width. Returns true when the selection changed this frame.
///
/// Why SEL/SELBD: they are the theme's accent-derived selection tints (the same ones
/// the page pills and text selection use), so the control follows the accent instead of
/// hard-coding a green/blue.
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, current: &mut T, options: &[(T, &str)]) -> bool {
    const H: f32 = 32.0; // outer height
    const PAD: f32 = 3.0; // container inner margin
    let n = options.len().max(1) as f32;
    let (outer, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), H), egui::Sense::hover());
    let pill = |r: egui::Rect| CornerRadius::same((r.height() * 0.5) as u8);
    ui.painter().rect(outer, pill(outer), FIELD, Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);

    let inner = outer.shrink(PAD);
    let seg_w = inner.width() / n;
    let mut changed = false;
    for (i, (value, label)) in options.iter().enumerate() {
        let seg = egui::Rect::from_min_size(
            egui::pos2(inner.left() + seg_w * i as f32, inner.top()),
            egui::vec2(seg_w, inner.height()),
        );
        let resp = ui.interact(seg, ui.id().with(("segmented", i)), egui::Sense::click());
        let selected = *current == *value;
        let text_col = if selected {
            ui.painter().rect(seg, pill(seg), SEL, Stroke::new(1.0, SELBD), egui::StrokeKind::Inside);
            TEXT
        } else {
            if resp.hovered() && ui.is_enabled() {
                ui.painter().rect_filled(seg, pill(seg), HOV);
            }
            TEXT_DIM
        };
        if ui.is_enabled() {
            resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        ui.painter().text(seg.center(), egui::Align2::CENTER_CENTER, *label, FontId::proportional(14.0), text_col);
        if resp.clicked() && !selected {
            *current = *value;
            changed = true;
        }
    }
    changed
}

// ---- Checkbox & radio ----------------------------------------------------

/// Outline of an unchecked box/circle — light enough to read on the panel.
pub const CHECK_OUTLINE: Color32 = steel(150);

/// Whether [`mark_ui`] draws a rounded square (checkbox) or a circle (radio).
#[derive(Clone, Copy, PartialEq)]
enum MarkShape {
    Check,
    Radio,
}

/// A checkbox styled like the Ritz launcher: an 18px rounded box (accent-filled
/// with a white check when on, hairline outline when off) followed by the label,
/// the whole row a single click target with a subtle hover wash. Drop-in
/// replacement for `crate::theme::styled_checkbox(ui, &mut x, label)` — returns the row's response.
pub fn styled_checkbox(
    ui: &mut egui::Ui,
    checked: &mut bool,
    label: impl Into<String>,
) -> egui::Response {
    checkbox_ui(ui, checked, label.into(), 0.0, f32::INFINITY, true)
}

/// Content-sized styled checkbox with an explicit enabled flag: when `enabled` is
/// false the row renders dimmed and ignores clicks (`*checked` is left untouched).
/// Used by the export/import tree to grey out groups absent from a pasted JSON.
pub fn styled_checkbox_enabled(
    ui: &mut egui::Ui,
    checked: &mut bool,
    label: impl Into<String>,
    enabled: bool,
) -> egui::Response {
    checkbox_ui(ui, checked, label.into(), 0.0, f32::INFINITY, enabled)
}

/// A radio button matching the styled checkbox — same 18px mark, accent fill, hover
/// wash and whole-row click target, but drawn as a circle with a white centre dot
/// when selected. `*current` is set to `value` on click. Content-sized; group them
/// in a `ui.horizontal` (or stack them) like `ui.radio_value`.
pub fn styled_radio<T: PartialEq>(
    ui: &mut egui::Ui,
    current: &mut T,
    value: T,
    label: impl Into<String>,
) -> egui::Response {
    styled_radio_w(ui, current, value, label, 0.0)
}

/// Same as [`styled_radio`] but with an explicit minimum width for the mark + label.
/// Use on the *first* button of a row of radio pairs (e.g. the unit toggles in
/// Settings → Display) so every row's second button starts at the same x — a shared
/// `col_w` across rows makes them read as columns instead of each row hugging its own
/// label width.
pub fn styled_radio_w<T: PartialEq>(
    ui: &mut egui::Ui,
    current: &mut T,
    value: T,
    label: impl Into<String>,
    col_w: f32,
) -> egui::Response {
    let selected = *current == value;
    let mut resp = mark_ui(ui, selected, label.into(), col_w, f32::INFINITY, true, MarkShape::Radio);
    if resp.clicked() && !selected {
        *current = value;
        resp.mark_changed();
    }
    resp
}

/// A one-of-N choice as a row of [`styled_radio`]s that **wraps**: the options sit side by
/// side while they fit the available width and stack onto further lines when they don't
/// (a narrow column), so the group never pokes out of its pane. Returns true on change.
pub fn radio_group<T: PartialEq + Copy>(ui: &mut egui::Ui, current: &mut T, options: &[(T, &str)]) -> bool {
    ui.horizontal_wrapped(|ui| {
        let mut changed = false;
        for &(value, label) in options {
            changed |= styled_radio(ui, current, value, label).changed();
        }
        changed
    })
    .inner
}

/// Checkbox behaviour on top of [`mark_ui`]: toggles `*checked` on click.
fn checkbox_ui(
    ui: &mut egui::Ui,
    checked: &mut bool,
    label: String,
    min_w: f32,
    max_w: f32,
    enabled: bool,
) -> egui::Response {
    let mut resp = mark_ui(ui, *checked, label, min_w, max_w, enabled, MarkShape::Check);
    if enabled && resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    resp
}

/// Shared renderer for the styled checkbox and radio: lays out an 18px mark + label
/// as one click-target row with a hover wash, and paints the mark per `shape`. Pure
/// rendering — returns the row response; callers apply the state change on click.
fn mark_ui(
    ui: &mut egui::Ui,
    on: bool,
    label: String,
    min_w: f32,
    max_w: f32,
    enabled: bool,
    shape: MarkShape,
) -> egui::Response {
    const BOX: f32 = 18.0;
    const GAP: f32 = 7.0;

    let font = TextStyle::Body.resolve(ui.style());
    // Width is the label content clamped to [min_w, max_w]: category checkboxes
    // pass a min of half the card (so short ones read a uniform width) and a max
    // of the card (so a long label never forces the card wider — it wraps first).
    // The content-sized entry points pass [0, ∞] to stay content-sized.
    // Never wider than the space the row has (the pane rule): a content-sized mark in a
    // narrow pane wraps its label instead of growing past the edge. In a wrapping layout the
    // limit is the whole line, so an option that doesn't fit moves to the next line first.
    let limit = if ui.layout().main_wrap { ui.max_rect().width() } else { ui.available_size_before_wrap().x };
    let max_w = max_w.min(limit).max(BOX);
    let min_w = min_w.min(max_w);
    let no_wrap = ui.painter().layout_no_wrap(label.clone(), font.clone(), TEXT);
    let content_w = BOX + GAP + no_wrap.size().x;
    let w = content_w.clamp(min_w, max_w);
    let galley = if content_w <= w + 0.5 {
        no_wrap
    } else {
        ui.painter().layout(label, font, TEXT, (w - BOX - GAP).max(0.0))
    };
    let stretched = min_w > 0.0; // the half-width category rows (checkbox_row)
    let gsize = galley.size();
    // Occupy the standard control row height so it lines up with sliders / comboboxes
    // sharing its row (the mark + label stay centered within it).
    let size = egui::vec2(w, ui.spacing().interact_size.y.max(BOX).max(gsize.y));
    // Disabled rows don't sense clicks, so they can't toggle or show a hover wash.
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);

    // Dim everything one step when disabled: the fill, the mark, the outline, the text.
    let fill_col = if enabled { ACCENT } else { steel(70) };
    let mark_col = if enabled { Color32::WHITE } else { steel(150) };
    let outline_col = if enabled { CHECK_OUTLINE } else { steel(90) };
    let text_col = if enabled { TEXT } else { TEXT_DIM };

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if enabled && resp.hovered() {
            let hov = if stretched { rect } else { rect.expand2(egui::vec2(4.0, 2.0)) };
            painter.rect_filled(hov, CornerRadius::same(6), HOV);
        }
        let box_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - BOX / 2.0),
            egui::Vec2::splat(BOX),
        )
        .shrink(1.0);
        match shape {
            MarkShape::Check => {
                let round = CornerRadius::same(5);
                if on {
                    painter.rect_filled(box_rect, round, fill_col);
                    let g = painter.layout_no_wrap(
                        crate::icons::CHECK.to_owned(),
                        FontId::proportional(11.0),
                        mark_col,
                    );
                    // Centre on the glyph's ink, not its advance box (Nerd glyphs are offset).
                    let ink = g
                        .rows
                        .first()
                        .and_then(|r| r.glyphs.first())
                        .map(|gl| gl.pos.to_vec2() + gl.uv_rect.offset + gl.uv_rect.size * 0.5)
                        .unwrap_or_else(|| g.size() * 0.5);
                    painter.galley(box_rect.center() - ink, g, mark_col);
                } else {
                    painter.rect_stroke(box_rect, round, Stroke::new(1.5, outline_col), egui::StrokeKind::Inside);
                }
            }
            MarkShape::Radio => {
                let c = box_rect.center();
                let r = box_rect.width() / 2.0;
                if on {
                    painter.circle_filled(c, r, fill_col);
                    painter.circle_filled(c, r * 0.4, mark_col); // white centre dot
                } else {
                    painter.circle_stroke(c, r - 0.75, Stroke::new(1.5, outline_col));
                }
            }
        }
        painter.galley(
            egui::pos2(box_rect.right() + GAP, rect.center().y - gsize.y / 2.0),
            galley,
            text_col,
        );
    }

    resp
}

/// A half-width checkbox row for use inside a category card: the checkbox fills
/// the left column so all checkboxes read the same width and a slider/control
/// would begin at the midpoint. Returns the checkbox response.
pub fn checkbox_row(ui: &mut egui::Ui, checked: &mut bool, label: impl Into<String>) -> egui::Response {
    // At least half the card wide (uniform), at most the full card (a long label
    // stays on one line but never widens the card).
    let avail = ui.available_width();
    checkbox_ui(ui, checked, label.into(), avail * 0.5, avail, true)
}

/// Like [`checkbox_row`] but with a control (e.g. a combobox) in the right half,
/// aligned where a slider's rail would start. Returns the checkbox response.
pub fn checkbox_row_with(
    ui: &mut egui::Ui,
    checked: &mut bool,
    label: impl Into<String>,
    right: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let label = label.into();
    columns(ui, 2, |c| {
        let half = c[0].available_width();
        let resp = checkbox_ui(&mut c[0], checked, label, half, half, true);
        right(&mut c[1]);
        resp
    })
}

// ---- Settings rows -------------------------------------------------------

/// Width reserved for a row's right-hand value spinner — fits the widest value
/// ("100.0%"), so the spinner never grows and pushes the row as digits change.
pub const VALUE_W: f32 = 72.0;

/// A left-column row label, laid out to line up with the control in the right
/// column of a two-`columns` settings row. Returns the label response (for
/// hover tooltips).
///
/// Why not a plain `ui.label`: `ui.columns` top-aligns each column, so a bare
/// label sits at the row's top edge while the control beside it (a slider /
/// combobox / spinner, ~`interact_size.y` tall) is vertically centered — the
/// label then reads as floating above the control. This allocates the label a
/// row of the standard control height and vertically centers it, so the two
/// line up. `left_to_right` (not the columns' justified layout) also stops a
/// wrapping label from spreading its letters across the line.
///
/// ponytail: fixed height = one control row; a label long enough to wrap to two
/// lines overflows downward. Fine for the short settings labels in use; give it
/// its own min-height growth if multi-line labels ever appear.
pub fn row_label(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let h = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), h),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| ui.add(egui::Label::new(label).wrap()),
    )
    .inner
}

/// A settings row in the "advanced" style shared across category cards: the
/// label in the left half, a slider filling the right half with a fixed-width
/// [`VALUE_W`] value spinner pinned to the far right. Returns the combined
/// slider/spinner response (use `.changed()`).
pub fn slider_row<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: f64,
    decimals: usize,
    suffix: &str,
) -> egui::Response {
    columns(ui, 2, |c| {
        row_label(&mut c[0], label);
        c[1].horizontal(|ui| {
            // Pin the fixed-width spinner to the right and let the slider fill the
            // rest, so the spinner is never the thing that clips when space is tight.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let d = ui.add_sized(
                    [VALUE_W, ui.spacing().interact_size.y],
                    egui::DragValue::new(&mut *value)
                        .range(range.clone())
                        .speed(step.max(0.01))
                        .fixed_decimals(decimals)
                        .suffix(suffix),
                );
                let rail = (ui.available_width() - 2.0).max(40.0);
                ui.spacing_mut().slider_width = rail;
                let s = ui.add(egui::Slider::new(&mut *value, range).step_by(step).show_value(false));
                s | d
            })
            .inner
        })
        .inner
    })
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
