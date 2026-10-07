use std::collections::HashSet;
use std::time::Duration;

use egui::{
    Align, Color32, Layout, Pos2, Rect, RichText, Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};
use egui_plot::{AxisHints, Bar, BarChart, HPlacement, Legend, Line, Plot, PlotPoints};

use crate::app::{
    DashboardDragState, DashboardResizeState, ForzaApp, GForceStats, ResizeEdge,
};
use crate::config::{
    SprintType, TextAlign, TireDisplayStyle, WidgetKind, WidgetLayout,
};
use crate::i18n::tr;
use crate::packet::ForzaPacket;

const RESIZE_STRIP: f32 = 8.0;

/// Placeholder shown until the first packet arrives: how to switch Data Out on,
/// and the exact values to enter. `listen_port` is the app's configured port.
fn show_waiting_screen(ui: &mut Ui, listen_port: u16) {
    use crate::theme;
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.24).max(24.0));

        ui.label(
            RichText::new(tr("Waiting for telemetry…"))
                .size(26.0)
                .strong()
                .color(theme::TEXT),
        );
        ui.add_space(12.0);
        ui.label(
            RichText::new(tr("Enable Data Out in Forza — scroll all the way down"))
                .size(15.0)
                .color(theme::DIM),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(tr("SETTINGS → HUD AND GAMEPLAY → DATA OUT"))
                .size(14.0)
                .strong()
                .color(theme::ACCENT),
        );
        ui.add_space(18.0);

        // Compact card listing the exact values to enter.
        let card_w = 340.0_f32.min((ui.available_width() - 32.0).max(200.0));
        ui.allocate_ui_with_layout(vec2(card_w, 0.0), Layout::top_down(Align::Min), |ui| {
            egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .inner_margin(egui::Margin::symmetric(18, 14))
                .corner_radius(8.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    waiting_row(ui, "Data Out", "On");
                    ui.add_space(9.0);
                    waiting_row(ui, "Data Out IP Address", "127.0.0.1");
                    ui.add_space(9.0);
                    waiting_row(ui, "Data Out IP Port", &listen_port.to_string());
                });
        });
    });
}

/// One "in-game setting = value" row inside the waiting-screen card.
fn waiting_row(ui: &mut Ui, key: &str, value: &str) {
    use crate::theme;
    ui.horizontal(|ui| {
        ui.label(RichText::new(key).size(14.0).color(theme::DIM));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(value)
                    .size(14.0)
                    .strong()
                    .monospace()
                    .color(theme::GOOD),
            );
        });
    });
}

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    let Some(pkt) = app.telemetry.latest.clone() else {
        show_waiting_screen(ui, app.config.listen_port);
        return;
    };

    let edit = app.config.dashboard_edit_mode;
    let show_grid     = edit || app.config.dashboard_show_grid;
    let show_outlines = edit || app.config.dashboard_show_outlines;
    let grid_cols = app.config.grid_cols.max(1);
    let avail_w = ui.available_width();
    let avail_h = ui.available_height();
    let cell_w = avail_w / grid_cols as f32;

    // Snapshot layout for this frame (avoids borrow conflicts while we mutate app later)
    let widgets: Vec<WidgetLayout> = app.config.dashboard_widgets.clone();

    let num_rows = widgets
        .iter()
        .filter(|w| !app.config.disabled_modules.contains(&w.kind))
        .map(|w| w.row + w.row_span)
        .max()
        .unwrap_or(1)
        .max(app.config.grid_rows);
    let cell_h = avail_h / num_rows as f32;

    let origin = ui.cursor().min;

    // Allocate the full grid area so the parent cursor advances past it
    ui.allocate_exact_size(Vec2::new(avail_w, avail_h), egui::Sense::hover());

    // ── Commit drag / resize when mouse button is released ─────────
    if edit {
        let mouse_released = ui
            .ctx()
            .input(|i| i.pointer.button_released(egui::PointerButton::Primary));

        if mouse_released {
            if let Some(drag) = app.dashboard_drag.take() {
                if let Some(ptr) = ui.ctx().input(|i| i.pointer.latest_pos()) {
                    commit_drag(app, &drag, &widgets, ptr, cell_w, cell_h, origin);
                }
            }
            if let Some(resize) = app.dashboard_resize.take() {
                if let Some(ptr) = ui.ctx().input(|i| i.pointer.latest_pos()) {
                    let delta = ptr - resize.origin_ptr;
                    let (nc, nr, cs, rs) = compute_resize_result(&resize, delta, cell_w, cell_h, grid_cols);
                    let w = &mut app.config.dashboard_widgets[resize.widget_idx];
                    w.col = nc;
                    w.row = nr;
                    w.col_span = cs;
                    w.row_span = rs;
                }
                app.config.save();
            }
        }
    }

    // ── Empty-cell background ─────────────────────────────────────
    if show_grid {
        let active_widgets: Vec<WidgetLayout> = widgets
            .iter()
            .filter(|w| !app.config.disabled_modules.contains(&w.kind))
            .cloned()
            .collect();
        let occupied = compute_occupied(&active_widgets);
        let empty_stroke = Stroke::new(1.0, crate::theme::steel(38));
        for row in 0..num_rows {
            for col in 0..grid_cols {
                if !occupied.contains(&(col, row)) {
                    let r = cell_rect(col, row, 1, 1, cell_w, cell_h, origin);
                    ui.painter().rect_stroke(r, 0.0, empty_stroke, egui::StrokeKind::Middle);
                }
            }
        }
    }

    let border_color = ui
        .visuals()
        .widgets
        .noninteractive
        .bg_stroke
        .color;

    // ── Render each widget ─────────────────────────────────────────
    for (idx, widget) in widgets.iter().enumerate() {
        if widget.kind == WidgetKind::Empty
            || app.config.disabled_modules.contains(&widget.kind)
        {
            continue;
        }

        let wrect = cell_rect(
            widget.col,
            widget.row,
            widget.col_span,
            widget.row_span,
            cell_w,
            cell_h,
            origin,
        );

        // Widget border — visible when outlines are shown
        if show_outlines {
            let active = app
                .dashboard_drag
                .as_ref()
                .map_or(false, |d| d.widget_idx == idx)
                || app
                    .dashboard_resize
                    .as_ref()
                    .map_or(false, |r| r.widget_idx == idx);
            let stroke_color = if active {
                Color32::from_rgb(255, 200, 60)
            } else {
                border_color
            };
            ui.painter()
                .rect_stroke(wrect, 2.0, Stroke::new(1.5, stroke_color), egui::StrokeKind::Middle);
        }

        let content_rect = wrect.shrink(2.0);

        let kind = widget.kind.clone();
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(content_rect)
                .layout(Layout::top_down(Align::LEFT)),
            |ui| {
                ui.set_clip_rect(content_rect);
                if edit {
                    ui.set_enabled(false);
                }
                render_widget(ui, app, &pkt, &kind);
            },
        );

        if edit {
            let p = ui.painter();
            const BASE_A: u8   = 51;
            const HOVER_A: u8  = 85;
            const ACTIVE_A: u8 = 120;

            // 4 edge strips: full-width rect for grabbing, half-width rect for painting
            const VISUAL_STRIP: f32 = RESIZE_STRIP * 0.5;
            let edge_defs: [(ResizeEdge, Rect, Rect, egui::CursorIcon); 4] = [
                (ResizeEdge::Left,
                 Rect::from_min_max(wrect.min, pos2(wrect.left() + RESIZE_STRIP, wrect.bottom())),
                 Rect::from_min_max(wrect.min, pos2(wrect.left() + VISUAL_STRIP, wrect.bottom())),
                 egui::CursorIcon::ResizeWest),
                (ResizeEdge::Right,
                 Rect::from_min_max(pos2(wrect.right() - RESIZE_STRIP, wrect.top()), wrect.max),
                 Rect::from_min_max(pos2(wrect.right() - VISUAL_STRIP, wrect.top()), wrect.max),
                 egui::CursorIcon::ResizeEast),
                (ResizeEdge::Top,
                 Rect::from_min_max(wrect.min, pos2(wrect.right(), wrect.top() + RESIZE_STRIP)),
                 Rect::from_min_max(wrect.min, pos2(wrect.right(), wrect.top() + VISUAL_STRIP)),
                 egui::CursorIcon::ResizeNorth),
                (ResizeEdge::Bottom,
                 Rect::from_min_max(pos2(wrect.left(), wrect.bottom() - RESIZE_STRIP), wrect.max),
                 Rect::from_min_max(pos2(wrect.left(), wrect.bottom() - VISUAL_STRIP), wrect.max),
                 egui::CursorIcon::ResizeSouth),
            ];
            for (edge_i, (edge, strip_rect, visual_rect, cursor)) in edge_defs.into_iter().enumerate() {
                let is_active = app.dashboard_resize.as_ref()
                    .map_or(false, |r| r.widget_idx == idx && r.edge == edge);
                let strip_resp = ui.interact(
                    strip_rect,
                    egui::Id::new("wresize").with(idx).with(edge_i),
                    egui::Sense::drag(),
                );
                let alpha = if is_active { ACTIVE_A } else if strip_resp.hovered() { HOVER_A } else { BASE_A };
                p.rect_filled(visual_rect, 0.0, Color32::from_rgba_premultiplied(200, 200, 200, alpha));
                if strip_resp.hovered() || is_active {
                    ui.ctx().set_cursor_icon(cursor);
                }
                if strip_resp.drag_started() && app.dashboard_resize.is_none() {
                    app.dashboard_resize = Some(DashboardResizeState {
                        widget_idx: idx,
                        edge,
                        origin_col: widget.col,
                        origin_row: widget.row,
                        origin_span: (widget.col_span, widget.row_span),
                        origin_ptr: strip_resp.interact_pointer_pos().unwrap_or_default(),
                    });
                }
            }

            // Center move square
            let handle_size = (wrect.width().min(wrect.height()) * 0.25).clamp(24.0, 80.0);
            let move_rect = Rect::from_center_size(wrect.center(), vec2(handle_size, handle_size));
            let move_resp = ui.interact(
                move_rect,
                egui::Id::new("wmove").with(idx),
                egui::Sense::drag(),
            );
            let ma = if move_resp.is_pointer_button_down_on() { ACTIVE_A }
                     else if move_resp.hovered() { HOVER_A }
                     else { BASE_A };
            p.rect_filled(move_rect, 6.0, Color32::from_rgba_premultiplied(180, 180, 180, ma));
            p.rect_stroke(
                move_rect,
                6.0,
                Stroke::new(1.5, Color32::from_rgba_premultiplied(180, 180, 180, ma.saturating_add(40))),
                egui::StrokeKind::Middle,
            );
            if move_resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            if move_resp.is_pointer_button_down_on() && !move_resp.drag_started() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            if move_resp.drag_started() && app.dashboard_drag.is_none() {
                let ptr = move_resp.interact_pointer_pos().unwrap_or(wrect.min);
                app.dashboard_drag = Some(DashboardDragState {
                    widget_idx: idx,
                    pointer_offset: ptr - wrect.min,
                });
            }
        }
    }

    // ── Drag ghost overlay ─────────────────────────────────────────
    if edit { if let Some(drag) = &app.dashboard_drag {
        if let Some(ptr) = ui.ctx().pointer_latest_pos() {
            let widget = &widgets[drag.widget_idx];
            let tl = ptr - drag.pointer_offset;
            let raw_col = ((tl.x - origin.x) / cell_w).round() as i32;
            let raw_row = ((tl.y - origin.y) / cell_h).round() as i32;
            let snap_col = raw_col
                .max(0)
                .min(grid_cols as i32 - widget.col_span as i32)
                .max(0) as usize;
            let snap_row = raw_row.max(0) as usize;

            let ghost = cell_rect(
                snap_col,
                snap_row,
                widget.col_span,
                widget.row_span,
                cell_w,
                cell_h,
                origin,
            );
            let gp = ui
                .ctx()
                .layer_painter(egui::LayerId::new(egui::Order::Tooltip, "drag_ghost".into()));
            gp.rect_filled(ghost, 2.0, Color32::from_rgba_premultiplied(80, 130, 255, 55));
            gp.rect_stroke(ghost, 2.0, Stroke::new(2.0, Color32::from_rgb(80, 130, 255)), egui::StrokeKind::Middle);
            gp.text(
                ghost.center(),
                egui::Align2::CENTER_CENTER,
                widget.kind.label(),
                egui::FontId::proportional(13.0),
                Color32::WHITE,
            );
        }
    } }

    // ── Resize preview overlay ─────────────────────────────────────
    if edit { if let Some(resize) = &app.dashboard_resize {
        if let Some(ptr) = ui.ctx().pointer_latest_pos() {
            let delta = ptr - resize.origin_ptr;
            let (nc, nr, cs, rs) = compute_resize_result(resize, delta, cell_w, cell_h, grid_cols);
            let preview = cell_rect(nc, nr, cs, rs, cell_w, cell_h, origin);
            let rp = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                "resize_preview".into(),
            ));
            rp.rect_filled(
                preview,
                2.0,
                Color32::from_rgba_premultiplied(255, 140, 40, 45),
            );
            rp.rect_stroke(
                preview,
                2.0,
                Stroke::new(2.0, Color32::from_rgb(255, 140, 40)),
                egui::StrokeKind::Middle,
            );
            rp.text(
                preview.center(),
                egui::Align2::CENTER_CENTER,
                format!("{}×{}", cs, rs),
                egui::FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
    } }
}

// ── Grid geometry helpers ──────────────────────────────────────────

fn cell_rect(
    col: usize,
    row: usize,
    col_span: usize,
    row_span: usize,
    cell_w: f32,
    cell_h: f32,
    origin: Pos2,
) -> Rect {
    Rect::from_min_size(
        pos2(
            origin.x + col as f32 * cell_w,
            origin.y + row as f32 * cell_h,
        ),
        Vec2::new(col_span as f32 * cell_w, row_span as f32 * cell_h),
    )
}

fn compute_occupied(widgets: &[WidgetLayout]) -> HashSet<(usize, usize)> {
    let mut set = HashSet::new();
    for w in widgets {
        if w.kind == WidgetKind::Empty {
            continue;
        }
        for r in w.row..w.row + w.row_span {
            for c in w.col..w.col + w.col_span {
                set.insert((c, r));
            }
        }
    }
    set
}

fn compute_resize_result(
    resize: &DashboardResizeState,
    delta: Vec2,
    cell_w: f32,
    cell_h: f32,
    grid_cols: usize,
) -> (usize, usize, usize, usize) {
    let (oc, or_) = (resize.origin_col, resize.origin_row);
    let (ocs, ors) = resize.origin_span;
    match resize.edge {
        ResizeEdge::Right => {
            let dc = (delta.x / cell_w).round() as i32;
            let cs = ((ocs as i32 + dc).max(1) as usize).min(grid_cols.saturating_sub(oc).max(1));
            (oc, or_, cs, ors)
        }
        ResizeEdge::Bottom => {
            let dr = (delta.y / cell_h).round() as i32;
            (oc, or_, ocs, (ors as i32 + dr).max(1) as usize)
        }
        ResizeEdge::Left => {
            let dc = ((-delta.x) / cell_w).round() as i32;
            let new_col = (oc as i32 - dc).max(0) as usize;
            let taken = oc as i32 - new_col as i32;
            let cs = ((ocs as i32 + taken).max(1) as usize).min(oc + ocs - new_col);
            (new_col, or_, cs, ors)
        }
        ResizeEdge::Top => {
            let dr = ((-delta.y) / cell_h).round() as i32;
            let new_row = (or_ as i32 - dr).max(0) as usize;
            let taken = or_ as i32 - new_row as i32;
            (oc, new_row, ocs, (ors as i32 + taken).max(1) as usize)
        }
    }
}

fn commit_drag(
    app: &mut ForzaApp,
    drag: &DashboardDragState,
    widgets: &[WidgetLayout],
    ptr: Pos2,
    cell_w: f32,
    cell_h: f32,
    origin: Pos2,
) {
    let dragged = &widgets[drag.widget_idx];
    let grid_cols = app.config.grid_cols.max(1) as i32;
    let tl = ptr - drag.pointer_offset;
    let raw_col = ((tl.x - origin.x) / cell_w).round() as i32;
    let raw_row = ((tl.y - origin.y) / cell_h).round() as i32;
    let new_col = raw_col
        .max(0)
        .min(grid_cols - dragged.col_span as i32)
        .max(0) as usize;
    let new_row = raw_row.max(0) as usize;

    let old_col = dragged.col;
    let old_row = dragged.row;

    // Find first widget whose cells overlap the target area
    let collision = widgets
        .iter()
        .enumerate()
        .find(|(i, w)| {
            *i != drag.widget_idx
                && w.kind != WidgetKind::Empty
                && !app.config.disabled_modules.contains(&w.kind)
                && new_col < w.col + w.col_span
                && new_col + dragged.col_span > w.col
                && new_row < w.row + w.row_span
                && new_row + dragged.row_span > w.row
        })
        .map(|(i, _)| i);

    app.config.dashboard_widgets[drag.widget_idx].col = new_col;
    app.config.dashboard_widgets[drag.widget_idx].row = new_row;

    if let Some(ci) = collision {
        app.config.dashboard_widgets[ci].col = old_col;
        app.config.dashboard_widgets[ci].row = old_row;
    }

    app.config.save();
}

// ── Widget dispatcher ──────────────────────────────────────────────

/// Draw a dashboard widget's title row, unless the master "hide widget titles"
/// mini-setting is on. The reclaimed space flows to the widget content, which
/// sizes to `available_rect_before_wrap`.
fn widget_title(ui: &mut Ui, app: &ForzaApp, text: &str) {
    if !app.config.hide_widget_titles {
        // Truncate, never wrap: a wrapping title in a narrow cell broke letter-by-letter
        // ("BOO / ST") and ate the content's height.
        ui.add(egui::Label::new(crate::theme::section_label(text)).truncate());
    }
}

// ── Fit-to-pane helpers (the module sizing standard) ───────────────
//
// A module draws inside its own pane (the cell below its title), paints its text with
// the painter (so nothing wraps), and scales every font uniformly so the content fits the
// pane in both axes — shrinking as far as the pane demands, never growing past the base
// size. There is no minimum module size: a tiny cell just gets tiny text.
// Why: the older modules laid text out with labels/columns at fixed sizes, which forced a
// de-facto minimum cell size and overflowed or wrapped below it.

/// Inset between a module's content and its pane edge (matches `SPRINT_EDGE`).
const PANE_EDGE: f32 = 3.0;
/// A fitted font below this many points isn't worth painting — the pane is too small.
const MIN_PAINT_FONT: f32 = 4.0;

/// Claim everything left in the cell (below the title) as the module's pane and return it
/// inset by [`PANE_EDGE`]. `None` when nothing usable is left.
fn module_pane(ui: &mut Ui) -> Option<Rect> {
    let full = ui.available_rect_before_wrap();
    if full.width() <= 0.0 || full.height() <= 0.0 {
        return None;
    }
    ui.allocate_rect(full, egui::Sense::hover());
    let pane = full.shrink(PANE_EDGE);
    (pane.width() > 0.0 && pane.height() > 0.0).then_some(pane)
}

/// Uniform scale that fits `natural` into `avail`, capped at 1 (no floor).
fn fit_scale(natural: Vec2, avail: Vec2) -> f32 {
    if natural.x <= 0.0 || natural.y <= 0.0 {
        return 1.0;
    }
    // 2% slack: glyph advances don't scale perfectly linearly with the font size.
    ((avail.x / natural.x).min(avail.y / natural.y) * 0.98).clamp(0.0, 1.0)
}

/// Unwrapped text width at `font`.
fn text_w(painter: &egui::Painter, text: &str, font: &egui::FontId) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    painter.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x
}

/// One row of a fitted two-column text block. Either side may be empty.
struct FitRow {
    left: String,
    left_col: Color32,
    right: String,
    right_col: Color32,
}

impl FitRow {
    fn new(left: impl Into<String>, left_col: Color32, right: impl Into<String>, right_col: Color32) -> Self {
        Self { left: left.into(), left_col, right: right.into(), right_col }
    }
}

/// Where a [`FitRow`]'s right text goes.
#[derive(Clone, Copy, PartialEq)]
enum RightCol {
    /// Right-aligned to the pane's right edge (label … value).
    Edge,
    /// Left-aligned in a second column (at the pane's middle when there's room).
    Column,
}

/// Natural (scale 1) geometry of a row block at `base` pt.
struct RowsMetrics {
    size: Vec2,
    line_h: f32,
    row_gap: f32,
    col_gap: f32,
    left_w: f32,
    right_w: f32,
}

fn fit_rows_metrics(painter: &egui::Painter, rows: &[FitRow], base: f32, right: RightCol) -> RowsMetrics {
    let font = egui::FontId::proportional(base);
    let line_h = painter.layout_no_wrap("0".to_owned(), font.clone(), Color32::WHITE).size().y;
    let row_gap = base * 0.3;
    let col_gap = base * 0.8;
    let left_w = rows.iter().map(|r| text_w(painter, &r.left, &font)).fold(0.0, f32::max);
    let right_w = rows.iter().map(|r| text_w(painter, &r.right, &font)).fold(0.0, f32::max);
    let w = match right {
        RightCol::Edge => rows.iter().map(|r| {
            let (a, b) = (text_w(painter, &r.left, &font), text_w(painter, &r.right, &font));
            if a > 0.0 && b > 0.0 { a + col_gap + b } else { a + b }
        }).fold(0.0, f32::max),
        RightCol::Column => {
            if left_w > 0.0 && right_w > 0.0 { left_w + col_gap + right_w } else { left_w + right_w }
        }
    };
    let n = rows.len() as f32;
    let h = n * line_h + (n - 1.0).max(0.0) * row_gap;
    RowsMetrics { size: vec2(w, h), line_h, row_gap, col_gap, left_w, right_w }
}

/// Largest font size ≤ `base` at which the block `measure(size)` fits `avail`.
/// Re-measured at each candidate size: glyph advances snap to pixels and don't scale
/// linearly, so a single proportional shrink can still overflow by a pixel.
fn fit_font(base: f32, avail: Vec2, measure: impl Fn(f32) -> Vec2) -> f32 {
    if avail.x <= 0.0 || avail.y <= 0.0 {
        return 0.0;
    }
    let mut size = base;
    for _ in 0..6 {
        let k = fit_scale(measure(size), avail);
        if k >= 0.999 || size < MIN_PAINT_FONT {
            break;
        }
        size *= k;
    }
    size
}

/// A way to lay a row list out: the rows, where their right text goes, and how many
/// side-by-side columns the list flows into (top-to-bottom, then the next column).
#[derive(Clone, Copy)]
struct FitLayout<'a> {
    rows: &'a [FitRow],
    right: RightCol,
    cols: usize,
}

impl<'a> FitLayout<'a> {
    fn new(rows: &'a [FitRow], right: RightCol, cols: usize) -> Self {
        Self { rows, right, cols: cols.max(1) }
    }

    fn chunks(&self) -> std::slice::Chunks<'a, FitRow> {
        self.rows.chunks(self.rows.len().div_ceil(self.cols).max(1))
    }

    /// Gutter between flowed columns at font size `sz`.
    fn gutter(sz: f32) -> f32 {
        sz * 1.5
    }

    /// Natural block size at font size `sz`.
    fn measure(&self, painter: &egui::Painter, sz: f32) -> Vec2 {
        let ms: Vec<RowsMetrics> = self.chunks().map(|c| fit_rows_metrics(painter, c, sz, self.right)).collect();
        let n = ms.len() as f32;
        let w = ms.iter().map(|m| m.size.x).fold(0.0, f32::max);
        let h = ms.iter().map(|m| m.size.y).fold(0.0, f32::max);
        vec2(n * w + (n - 1.0) * Self::gutter(sz), h)
    }

    /// Font size this layout fits `rect` at.
    fn size(&self, painter: &egui::Painter, rect: Rect, base: f32) -> f32 {
        if self.rows.is_empty() {
            return 0.0;
        }
        fit_font(base, rect.size(), |sz| self.measure(painter, sz))
    }

    /// Paint at the size that fits `rect`, the block vertically centred.
    fn paint(&self, painter: &egui::Painter, rect: Rect, base: f32) {
        let size = self.size(painter, rect, base);
        if size < MIN_PAINT_FONT {
            return;
        }
        let block_h = self.measure(painter, size).y;
        let top = rect.top() + (rect.height() - block_h) * 0.5;
        let chunks: Vec<&[FitRow]> = self.chunks().collect();
        let n = chunks.len() as f32;
        let col_w = (rect.width() - (n - 1.0) * Self::gutter(size)) / n;
        for (i, chunk) in chunks.into_iter().enumerate() {
            let x = rect.left() + i as f32 * (col_w + Self::gutter(size));
            let col = Rect::from_min_size(pos2(x, top), vec2(col_w, block_h));
            paint_rows_sized(painter, col, chunk, size, self.right);
        }
    }
}

/// Paint `rows` at font `size` from the top of `rect` (which they are known to fit).
fn paint_rows_sized(painter: &egui::Painter, rect: Rect, rows: &[FitRow], size: f32, right: RightCol) {
    let m = fit_rows_metrics(painter, rows, size, right);
    let font = egui::FontId::proportional(size);
    // Second column: at the middle when there's room, else right after the widest left.
    let x2 = rect.left()
        + (m.left_w + m.col_gap)
            .max(rect.width() * 0.5)
            .min(rect.width() - m.right_w)
            .max(0.0);
    for (i, r) in rows.iter().enumerate() {
        let y = rect.top() + i as f32 * (m.line_h + m.row_gap);
        if !r.left.is_empty() {
            painter.text(pos2(rect.left(), y), egui::Align2::LEFT_TOP, &r.left, font.clone(), r.left_col);
        }
        if !r.right.is_empty() {
            let (x, align) = match right {
                RightCol::Edge => (rect.right(), egui::Align2::RIGHT_TOP),
                RightCol::Column => (x2, egui::Align2::LEFT_TOP),
            };
            painter.text(pos2(x, y), align, &r.right, font.clone(), r.right_col);
        }
    }
}

/// Paint `rows` as one uniformly sized block that fits `rect`, vertically centred.
fn paint_fit_rows(painter: &egui::Painter, rect: Rect, rows: &[FitRow], base: f32, right: RightCol) {
    FitLayout::new(rows, right, 1).paint(painter, rect, base);
}

/// Paint whichever candidate layout fits `rect` at the largest font — e.g. side-by-side
/// columns in a wide pane, stacked in a narrow one, flowed into several columns in a
/// wide, short one. Candidates are in order of preference: a later one must beat the
/// best so far by 15% to be picked, so the layout doesn't flip on a pixel of difference.
fn paint_best_fit(painter: &egui::Painter, rect: Rect, base: f32, candidates: &[FitLayout]) {
    let mut best: Option<(f32, usize)> = None;
    for (i, c) in candidates.iter().enumerate() {
        let sz = c.size(painter, rect, base);
        if best.is_none_or(|(b, _)| sz > b * 1.15) {
            best = Some((sz, i));
        }
    }
    if let Some((_, i)) = best {
        candidates[i].paint(painter, rect, base);
    }
}

fn render_widget(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket, kind: &WidgetKind) {
    match kind {
        WidgetKind::Empty      => {}
        WidgetKind::Speed      => show_speed_widget(ui, app, pkt),
        WidgetKind::Gear       => show_gear_widget(ui, app, pkt),
        WidgetKind::Rpm        => show_rpm_widget(ui, app, pkt),
        WidgetKind::Inputs     => show_inputs_block(ui, app, pkt),
        WidgetKind::Car        => show_car_block(ui, app, pkt),
        WidgetKind::Engine     => show_engine_block(ui, app, pkt),
        WidgetKind::Position   => show_position_block(ui, app, pkt),
        WidgetKind::Race       => show_race_block(ui, app, pkt),
        WidgetKind::Tires      => show_tires_block(ui, app, pkt),
        WidgetKind::GForce     => show_gforce_block(ui, app, pkt),
        WidgetKind::Suspension => show_suspension_block(ui, app, pkt),
        WidgetKind::MiniMap    => show_minimap_widget(ui, app),
        WidgetKind::CoopPlayers => show_coop_players(ui, app, pkt),
        WidgetKind::Trace      => show_trace_widget(ui, app, pkt),
        WidgetKind::Boost      => show_boost_widget(ui, app, pkt),
        WidgetKind::SessionStats => show_session_stats(ui, app, pkt),
        WidgetKind::PowerGraph => show_power_graph_widget(ui, app),
        WidgetKind::BoostGraph => show_boost_graph_widget(ui, app),
    }
}

/// Per-car session maxima (reset on car change) — a quick run-review summary.
/// Label … value rows fitted to the pane; a narrow pane stacks each value under its label.
fn show_session_stats(ui: &mut Ui, app: &ForzaApp, _pkt: &ForzaPacket) {
    widget_title(ui, app, tr("Session Stats"));
    let Some(pane) = module_pane(ui) else { return };

    let use_mph = app.config.use_mph;
    let use_bar = app.config.use_bar;
    let (spd, spd_u) = if use_mph {
        (app.max_speed_kmh / 1.609_34, "mph")
    } else {
        (app.max_speed_kmh, "km/h")
    };
    let (boost, boost_u) = if use_bar {
        (app.max_boost_psi * 0.068_947_6, "bar")
    } else {
        (app.max_boost_psi, "PSI")
    };
    // Cached max, not pkt.engine_max_rpm — the packet field zeroes while paused.
    let max_rpm = app.dynamic_max_rpm.max(app.cached_engine_max_rpm as f32);

    let stats: [(&str, String); 7] = [
        (tr("Top Speed"), format!("{spd:.0} {spd_u}")),
        (tr("Peak Power"), format!("{:.0} PS", app.max_power_ps)),
        (tr("Peak Torque"), format!("{:.0} Nm", app.max_torque_nm)),
        (tr("Peak Boost"), format!("{boost:.2} {boost_u}")),
        (tr("Peak Lat G"), format!("{:.2} g", app.gforce_stats.max_lateral)),
        (tr("Peak Long G"), format!("{:.2} g", app.gforce_stats.max_longitudinal)),
        (tr("Max RPM"), format!("{max_rpm:.0}")),
    ];
    let base = egui::TextStyle::Body.resolve(ui.style()).size;
    paint_session_stats(ui.painter(), pane, &stats, base);
}

/// Session Stats body: label … value rows; each value stacked under its label in a
/// narrow, tall pane; or the rows flowed into 2-4 columns in a wide, short one —
/// whichever fits the pane biggest.
fn paint_session_stats(painter: &egui::Painter, pane: Rect, stats: &[(&str, String)], base: f32) {
    let dim = crate::theme::TEXT_DIM;
    let val_col = Color32::from_rgb(230, 200, 90);
    let side: Vec<FitRow> = stats.iter()
        .map(|(l, v)| FitRow::new(*l, dim, v.clone(), val_col))
        .collect();
    let stacked: Vec<FitRow> = stats.iter()
        .flat_map(|(l, v)| [FitRow::new(*l, dim, "", val_col), FitRow::new("", dim, v.clone(), val_col)])
        .collect();
    paint_best_fit(painter, pane, base, &[
        FitLayout::new(&side, RightCol::Edge, 1),
        FitLayout::new(&stacked, RightCol::Edge, 1),
        // Wide, short panes: flow the rows into 2-4 columns.
        FitLayout::new(&side, RightCol::Edge, 2),
        FitLayout::new(&side, RightCol::Edge, 3),
        FitLayout::new(&side, RightCol::Edge, 4),
    ]);
}

/// Turbo/supercharger boost gauge — current value + a bar with the session-peak tick.
/// Adapts to its cell: taller-than-wide renders a vertical (bottom-up) gauge,
/// square or wider keeps the default horizontal bar. Every text is fitted to the pane
/// (no fixed minimum bar/text size), see [`fit_scale`].
fn show_boost_widget(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let full = ui.available_rect_before_wrap();
    let vertical = full.height() > full.width();
    widget_title(ui, app, tr("Boost"));
    let Some(pane) = module_pane(ui) else { return };

    let use_bar = app.config.use_bar;
    let conv = |psi: f32| if use_bar { psi * 0.068_947_6 } else { psi };
    let cur = conv(pkt.boost);
    let peak = conv(app.max_boost_psi);
    // Colour ramps green→orange→red with boost pressure.
    let level = (pkt.boost / 20.0).clamp(0.0, 1.0);
    let g = BoostGauge {
        cur,
        peak,
        scale: peak.max(cur).max(conv(7.0)) * 1.15,
        unit: if use_bar { "bar" } else { "PSI" },
        color: Color32::from_rgb((70.0 + 160.0 * level) as u8, (200.0 - 120.0 * level) as u8, 70),
    };
    let painter = ui.painter();
    if app.config.boost_in_bar {
        paint_boost_in_bar(painter, pane, &g);
    } else if vertical {
        paint_boost_vertical(painter, pane, &g);
    } else {
        paint_boost_horizontal(painter, pane, &g);
    }
}

/// Values the Boost module draws, already in the display unit.
struct BoostGauge {
    cur: f32,
    peak: f32,
    /// Bar full-scale value.
    scale: f32,
    unit: &'static str,
    color: Color32,
}

/// Bar track + fill + session-peak tick. `vertical` fills bottom-up, else left-to-right.
fn paint_boost_bar(painter: &egui::Painter, bar: Rect, g: &BoostGauge, vertical: bool) {
    if bar.width() <= 0.0 || bar.height() <= 0.0 {
        return;
    }
    let round = 4.0_f32.min(bar.width() * 0.5).min(bar.height() * 0.5);
    painter.rect_filled(bar, round, Color32::from_rgb(22, 24, 27));
    if g.scale <= 0.0 {
        return;
    }
    let tick = Stroke::new(2.0, Color32::from_rgb(240, 220, 90));
    let frac = (g.cur / g.scale).clamp(0.0, 1.0);
    let pf = (g.peak / g.scale).clamp(0.0, 1.0);
    let inset = 2.0_f32.min(bar.width().min(bar.height()) * 0.25);
    if vertical {
        if frac > 0.001 {
            let h = bar.height() * frac;
            painter.rect_filled(Rect::from_min_max(pos2(bar.left(), bar.bottom() - h), bar.max), round, g.color);
        }
        if pf > 0.001 {
            let y = bar.bottom() - bar.height() * pf;
            painter.line_segment([pos2(bar.left() + inset, y), pos2(bar.right() - inset, y)], tick);
        }
    } else {
        if frac > 0.001 {
            painter.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height())), round, g.color);
        }
        if pf > 0.001 {
            let x = bar.left() + bar.width() * pf;
            painter.line_segment([pos2(x, bar.top() + inset), pos2(x, bar.bottom() - inset)], tick);
        }
    }
}

/// Compact: the peak in parens on top, then a bottom-up bar filling the rest of the pane
/// with the value inside it. The unit is dropped (it's a global setting).
fn paint_boost_in_bar(painter: &egui::Painter, pane: Rect, g: &BoostGauge) {
    let galley_size = |text: &str, size: f32| {
        painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(size), Color32::WHITE).size()
    };
    let peak_text = format!("({:.2})", g.peak);
    // The peak line may take at most a quarter of the pane's height.
    let psize = fit_font(12.0, vec2(pane.width(), pane.height() * 0.25), |sz| galley_size(&peak_text, sz));
    let show_peak = psize >= MIN_PAINT_FONT;
    let peak_h = if show_peak { galley_size(&peak_text, psize).y } else { 0.0 };
    let sp = 3.0 * psize / 12.0;
    let bar = Rect::from_min_max(pos2(pane.left(), pane.top() + peak_h + sp), pane.max);
    paint_boost_bar(painter, bar, g, true);

    // Value inside the bar: at most half the bar's height, shrunk to its width.
    let val_text = format!("{:+.2}", g.cur);
    let vsize = fit_font(24.0, vec2(bar.width() * 0.9, bar.height() * 0.5), |sz| galley_size(&val_text, sz));
    if vsize >= MIN_PAINT_FONT {
        painter.text(bar.center(), egui::Align2::CENTER_CENTER, val_text,
            egui::FontId::proportional(vsize), Color32::WHITE);
    }
    if show_peak {
        painter.text(pos2(pane.center().x, pane.top() + peak_h * 0.5), egui::Align2::CENTER_CENTER,
            peak_text, egui::FontId::proportional(psize), crate::theme::TEXT_DIM);
    }
}

/// Tall pane: a bottom-up bar (≤ 26 px wide) with the value and a "unit · peak" line
/// centred under it. The text block takes at most ~45% of the height; the small line is
/// dropped when keeping it would shrink the value too much.
fn paint_boost_vertical(painter: &egui::Painter, pane: Rect, g: &BoostGauge) {
    let galley_size = |text: &str, size: f32| {
        painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(size), Color32::WHITE).size()
    };
    let value_text = format!("{:+.2}", g.cur);
    let small_text = format!("{} · {} {:.2}", g.unit, tr("peak"), g.peak);
    // Sizes are driven by the value font `v` (base 22); the small line is v/2, gap v/5.5.
    let block = |v: f32, small: bool| {
        let a = galley_size(&value_text, v);
        if small {
            let b = galley_size(&small_text, v * 0.5);
            vec2(a.x.max(b.x), a.y + v / 5.5 + b.y)
        } else {
            a
        }
    };
    let text_box = vec2(pane.width(), pane.height() * 0.45);
    let v_both = fit_font(22.0, text_box, |v| block(v, true));
    let v_value = fit_font(22.0, text_box, |v| block(v, false));
    let show_small = v_both >= v_value * 0.7;
    let v = if show_small { v_both } else { v_value };
    let text_h = if v >= MIN_PAINT_FONT { block(v, show_small).y } else { 0.0 };
    let sp = v / 5.5;

    let w = pane.width().min(26.0);
    let bar = Rect::from_min_size(
        pos2(pane.center().x - w * 0.5, pane.top()),
        vec2(w, (pane.height() - text_h - sp).max(0.0)),
    );
    paint_boost_bar(painter, bar, g, true);

    if v >= MIN_PAINT_FONT {
        let text_top = bar.bottom() + sp;
        let vh = galley_size(&value_text, v).y;
        painter.text(pos2(pane.center().x, text_top), egui::Align2::CENTER_TOP, value_text,
            egui::FontId::proportional(v), g.color);
        if show_small && v * 0.5 >= MIN_PAINT_FONT {
            painter.text(pos2(pane.center().x, text_top + vh + sp), egui::Align2::CENTER_TOP,
                small_text, egui::FontId::proportional(v * 0.5), crate::theme::TEXT_DIM);
        }
    }
}

/// Wide/square pane: "value unit … peak X" readout over a horizontal bar (≤ 26 px tall),
/// the pair centred vertically. The readout takes at most ~55% of the height.
fn paint_boost_horizontal(painter: &egui::Painter, pane: Rect, g: &BoostGauge) {
    let galley_size = |text: &str, size: f32| {
        painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(size), Color32::WHITE).size()
    };
    let value_text = format!("{:+.2}", g.cur);
    let peak_text = format!("{} {:.2}", tr("peak"), g.peak);
    // Driven by the value font `v` (base 22): unit 12/22·v, peak 11/22·v, gaps scale too.
    let k = |v: f32| v / 22.0;
    let readout = |v: f32| {
        let a = galley_size(&value_text, v);
        let u = galley_size(g.unit, 12.0 * k(v)).x;
        let p = galley_size(&peak_text, 11.0 * k(v)).x;
        vec2(a.x + 4.0 * k(v) + u + 10.0 * k(v) + p, a.y)
    };
    let v = fit_font(22.0, vec2(pane.width(), pane.height() * 0.55), readout);
    let show_text = v >= MIN_PAINT_FONT;
    let read_h = if show_text { readout(v).y } else { 0.0 };
    let sp = 4.0 * k(v);
    let bar_h = (pane.height() - read_h - sp).clamp(0.0, 26.0);
    let top = pane.top() + (pane.height() - (read_h + sp + bar_h)) * 0.5;

    if show_text {
        let cy = top + read_h * 0.5;
        let vx = pane.left();
        let vw = galley_size(&value_text, v).x;
        painter.text(pos2(vx, cy), egui::Align2::LEFT_CENTER, value_text,
            egui::FontId::proportional(v), g.color);
        painter.text(pos2(vx + vw + 4.0 * k(v), cy), egui::Align2::LEFT_CENTER, g.unit,
            egui::FontId::proportional(12.0 * k(v)), crate::theme::TEXT_DIM);
        painter.text(pos2(pane.right(), cy), egui::Align2::RIGHT_CENTER, peak_text,
            egui::FontId::proportional(11.0 * k(v)), crate::theme::TEXT_DIM);
    }
    let bar = Rect::from_min_size(pos2(pane.left(), top + read_h + sp), vec2(pane.width(), bar_h));
    paint_boost_bar(painter, bar, g, false);
}

/// Rolling speed (km/h) + RPM sparkline over the last ~30 s, hand-drawn to match
/// the lightweight dashboard widgets.
fn show_trace_widget(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    // Tighten the inter-row spacing BEFORE the title. egui bakes the gap that
    // follows a widget from item_spacing.y as read *when that widget is drawn*, so
    // setting it after the title (as before) never actually moved the legend — it
    // stayed a full default row-gap below. Zeroing it here pulls the legend right up
    // under the title (matching the Boost widget) and keeps the graph flush under the
    // legend too.
    ui.spacing_mut().item_spacing.y = 0.0;
    widget_title(ui, app, tr("Speed Trace"));

    let use_mph = app.config.use_mph;
    let unit = if use_mph { "mph" } else { "km/h" };
    let speed_disp = if use_mph { pkt.speed_mph() } else { pkt.speed_kmh() };

    // Legend / current values — size 12 to match the Boost widget's readout font.
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{speed_disp:.0} {unit}")).size(12.0).color(Color32::from_rgb(80, 200, 110)));
        ui.label(RichText::new(format!("{:.0} rpm", pkt.current_engine_rpm)).size(12.0).color(Color32::from_rgb(230, 160, 40)));
    });

    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, egui::Sense::hover());
    // 3px margin on the left, right and bottom to match the Boost widget's spacing.
    let rect = egui::Rect::from_min_max(
        pos2(full.left() + 3.0, full.top()),
        pos2(full.right() - 3.0, full.bottom() - 3.0),
    );
    if rect.height() < 10.0 || rect.width() < 10.0 {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(22, 24, 27));

    let hist = &app.trace_history;
    if hist.len() < 2 {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER,
            tr("Collecting…"), egui::FontId::proportional(12.0), crate::theme::TEXT_FAINT);
        return;
    }

    let window = crate::app::TRACE_WINDOW_SECS;
    // Active-time axis: the newest sample sits at the right edge. Paused
    // packets never enter the history, so a pause costs no plot width.
    let t_now = hist.back().map_or(0.0, |&(t, ..)| t);
    let speed_max = if use_mph { 200.0 } else { 320.0 };
    let rpm_max = effective_max_rpm(app, pkt).max(1000.0);

    // Horizontal gridlines
    for f in [0.25_f32, 0.5, 0.75] {
        let y = rect.bottom() - f * rect.height();
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(0.5, crate::theme::STROKE_DIM));
    }

    let x_of = |t: f32| {
        let age = (t_now - t).min(window);
        rect.right() - (age / window) * rect.width()
    };
    let y_of = |v: f32, vmax: f32| rect.bottom() - (v / vmax).clamp(0.0, 1.0) * (rect.height() - 2.0);

    let mut speed_pts = Vec::with_capacity(hist.len());
    let mut rpm_pts = Vec::with_capacity(hist.len());
    for &(t, spd_kmh, rpm) in hist.iter() {
        let x = x_of(t);
        let spd = if use_mph { spd_kmh / 1.609_34 } else { spd_kmh };
        speed_pts.push(pos2(x, y_of(spd, speed_max)));
        rpm_pts.push(pos2(x, y_of(rpm, rpm_max)));
    }
    painter.add(egui::Shape::line(rpm_pts, Stroke::new(1.3, Color32::from_rgb(200, 140, 35))));
    painter.add(egui::Shape::line(speed_pts, Stroke::new(1.8, Color32::from_rgb(80, 200, 110))));
}

/// Live co-op standings: one row per player (self + remotes) with a hue-coloured
/// speed bar, current speed and gear. Sorted fastest-first.
fn show_coop_players(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    use crate::coop::Role;
    widget_title(ui, app, tr("Co-Op"));
    let Some(pane) = module_pane(ui) else { return };
    let base = egui::TextStyle::Monospace.resolve(ui.style()).size;

    if app.coop.role() == Role::Off {
        let rows = [
            FitRow::new(tr("Not in a session."), crate::theme::TEXT_DIM, "", Color32::WHITE),
            FitRow::new(tr("Host or join from the Co-Op tab."), crate::theme::TEXT_FAINT, "", Color32::WHITE),
        ];
        paint_fit_rows(ui.painter(), pane, &rows, base, RightCol::Edge);
        return;
    }

    let use_mph = app.config.use_mph;
    let mut rows: Vec<CoopRow> = Vec::new();
    rows.push(CoopRow {
        name: app.config.coop_name.clone(),
        hue: app.config.coop_hue,
        speed_ms: pkt.speed,
        gear: pkt.gear,
        is_self: true,
        dist_m: 0.0,
    });
    for (info, rp) in app.coop.remote_players() {
        let dist = ((rp.position_x - pkt.position_x).powi(2)
            + (rp.position_z - pkt.position_z).powi(2)).sqrt();
        rows.push(CoopRow {
            name: info.name.clone(),
            hue: info.hue,
            speed_ms: rp.speed,
            gear: rp.gear,
            is_self: false,
            dist_m: dist,
        });
    }
    rows.sort_by(|a, b| b.speed_ms.partial_cmp(&a.speed_ms).unwrap_or(std::cmp::Ordering::Equal));
    paint_coop_rows(ui.painter(), pane, &rows, use_mph, base);
}

/// One player in the Co-Op module.
struct CoopRow {
    name: String,
    hue: f32,
    speed_ms: f32,
    gear: u8,
    is_self: bool,
    dist_m: f32,
}

/// Co-Op module body. Columns in monospace character cells — dot, rank + name (15),
/// distance (6), speed bar (flexible, ≥ 6), speed (8), gear (3) — so the layout never
/// shifts as values change. The whole table scales to fit the pane; when the width is the
/// limit, the distance and then the gear column are dropped first (if that lets the text
/// grow noticeably), and a narrow, tall pane gives each player two lines (dot + name, then
/// bar + speed), so names and speeds stay readable in a narrow pane.
fn paint_coop_rows(painter: &egui::Painter, pane: Rect, rows: &[CoopRow], use_mph: bool, base: f32) {
    if rows.is_empty() {
        return;
    }
    let unit = if use_mph { "mph" } else { "km/h" };
    // Bar scale: fixed reference top speed so bars are comparable frame-to-frame.
    let max_kmh = 320.0_f32;
    let (name_c, dist_c, bar_c, speed_c, gear_c) = (15.0, 6.0, 6.0, 8.0, 3.0);
    let n = rows.len() as f32;

    // Geometry at font size `sz`: (char width, row height, row gap, column gap, dot).
    let geom = |sz: f32| {
        let font = egui::FontId::monospace(sz);
        let ch = text_w(painter, "0", &font);
        let line_h = painter.layout_no_wrap("0".to_owned(), font, Color32::WHITE).size().y;
        (ch, line_h * 1.2, sz * 0.25, ch * 0.6, ch * 1.4)
    };
    let natural = |sz: f32, dist: bool, gear: bool, stacked: bool| {
        let (ch, row_h, row_gap, gap, dot) = geom(sz);
        if stacked {
            let w = (dot + gap + name_c * ch).max(bar_c * ch + gap + speed_c * ch);
            return vec2(w, n * 2.0 * row_h + (n - 1.0) * row_gap);
        }
        let mut w = dot + gap + name_c * ch + gap + bar_c * ch + gap + speed_c * ch;
        if dist { w += gap + dist_c * ch; }
        if gear { w += gap + gear_c * ch; }
        vec2(w, n * row_h + (n - 1.0) * row_gap)
    };

    // (show distance, show gear, two lines per player) — fullest first.
    let variants = [(true, true, false), (false, true, false), (false, false, false), (false, false, true)];
    let sizes: Vec<f32> = variants
        .iter()
        .map(|&(d, g, st)| fit_font(base, pane.size(), |sz| natural(sz, d, g, st)))
        .collect();
    let best = sizes.iter().copied().fold(0.0, f32::max);
    // Fullest variant within 85% of the best size: drop columns only when it pays.
    let vi = sizes.iter().position(|&s| s >= best * 0.85).unwrap_or(0);
    let (show_dist, show_gear, stacked) = variants[vi];
    let size = sizes[vi];
    if size < MIN_PAINT_FONT {
        return;
    }

    let font = egui::FontId::monospace(size);
    let (ch, row_h, row_gap, gap, dot) = geom(size);
    let top = pane.top() + (pane.height() - natural(size, show_dist, show_gear, stacked).y) * 0.5;
    // Right-hand fixed cells, measured from the pane's right edge.
    let right_w = speed_c * ch + if show_gear { gap + gear_c * ch } else { 0.0 };
    let left_w = dot + gap + name_c * ch + if show_dist { gap + dist_c * ch } else { 0.0 };
    let bar_w = (pane.width() - left_w - right_w - 2.0 * gap).max(0.0);
    let speed_bar = |left: f32, w: f32, cy: f32, kmh: f32, col: Color32| {
        if w > 0.0 {
            let bar = Rect::from_center_size(pos2(left + w * 0.5, cy), vec2(w, row_h * 0.6));
            let rounding = bar.height() * 0.5;
            painter.rect_filled(bar, rounding, crate::theme::TRACK);
            let frac = (kmh / max_kmh).clamp(0.0, 1.0);
            if frac > 0.0 {
                painter.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height())), rounding, col);
            }
        }
    };

    // Truncate to n chars with an ellipsis (monospace ⇒ n cells wide).
    let fit = |s: &str, n: usize| -> String {
        let c: Vec<char> = s.chars().collect();
        if c.len() > n {
            c[..n.saturating_sub(1)].iter().collect::<String>() + "…"
        } else {
            s.to_string()
        }
    };

    for (rank, r) in rows.iter().enumerate() {
        let col = crate::ui::coop::hue_color(r.hue);
        let kmh = r.speed_ms * 3.6;
        let disp = if use_mph { r.speed_ms * 2.236_94 } else { kmh };
        let gear_str = match r.gear { 0 => "N".to_string(), g => g.to_string() };
        let name_col = if r.is_self { Color32::WHITE } else { crate::theme::TEXT };
        let name = fit(&format!("{}. {}", rank + 1, r.name), name_c as usize);
        let speed_txt = format!("{disp:>3.0} {unit}");
        if stacked {
            let cy1 = top + rank as f32 * (2.0 * row_h + row_gap) + row_h * 0.5;
            let cy2 = cy1 + row_h;
            painter.circle_filled(pos2(pane.left() + dot * 0.5, cy1), dot * 0.35, col);
            painter.text(pos2(pane.left() + dot + gap, cy1), egui::Align2::LEFT_CENTER, name, font.clone(), name_col);
            speed_bar(pane.left(), pane.width() - speed_c * ch - gap, cy2, kmh, col);
            painter.text(pos2(pane.right(), cy2), egui::Align2::RIGHT_CENTER, speed_txt, font.clone(), crate::theme::TEXT);
            continue;
        }
        let cy = top + rank as f32 * (row_h + row_gap) + row_h * 0.5;
        let mut x = pane.left();

        painter.circle_filled(pos2(x + dot * 0.5, cy), dot * 0.35, col);
        x += dot + gap;

        painter.text(pos2(x, cy), egui::Align2::LEFT_CENTER, name, font.clone(), name_col);
        x += name_c * ch + gap;

        if show_dist {
            let dtxt = if r.is_self {
                String::new()
            } else if r.dist_m >= 1000.0 {
                format!("{:.1}km", r.dist_m / 1000.0)
            } else {
                format!("{:.0}m", r.dist_m)
            };
            painter.text(pos2(x + dist_c * ch, cy), egui::Align2::RIGHT_CENTER, dtxt,
                egui::FontId::monospace(size * 0.9), crate::theme::TEXT_FAINT);
            x += dist_c * ch + gap;
        }

        // The one flexible element: the speed bar fills what the fixed cells leave.
        speed_bar(x, bar_w, cy, kmh, col);

        let speed_right = if show_gear { pane.right() - gear_c * ch - gap } else { pane.right() };
        painter.text(pos2(speed_right, cy), egui::Align2::RIGHT_CENTER, speed_txt, font.clone(), crate::theme::TEXT);
        if show_gear {
            painter.text(pos2(pane.right(), cy), egui::Align2::RIGHT_CENTER,
                format!("G{gear_str}"), font.clone(), crate::theme::TEXT_DIM);
        }
    }
}

// ── New top-row widget renderers ───────────────────────────────────

fn show_speed_widget(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let speed = if app.config.use_mph { pkt.speed_mph() } else { pkt.speed_kmh() };
    let unit_str = if app.config.use_mph { "Mph" } else { "Km/h" };
    let legend_color = crate::theme::TEXT_DIM;

    let avail = ui.available_rect_before_wrap();

    // Bottom strip holds the "Km/h" label (and optional delta).
    let label_h = 20.0_f32;
    let main_h  = (avail.height() - label_h).max(label_h);

    // Digits are ~55% of font_size wide; 3 digits → ≈1.65× font_size.
    // Use main area height and width to derive the largest fitting size.
    let font_size = (main_h * 0.90)
        .min(avail.width() / 1.8)
        .max(16.0);

    // Center the number vertically in the main area (above the label strip).
    let center = pos2(avail.center().x, avail.top() + main_h * 0.5);
    let p = ui.painter();
    let fid = egui::FontId::proportional(font_size);

    match app.config.speed_align {
        TextAlign::Right => {
            p.text(center, egui::Align2::CENTER_CENTER,
                format!("{:>3.0}", speed), fid, Color32::WHITE);
        }
        TextAlign::Center => {
            p.text(center, egui::Align2::CENTER_CENTER,
                format!("{:.0}", speed), fid, Color32::WHITE);
        }
        TextAlign::RightPlaceholder => {
            let digits = format!("{:.0}", speed).len().min(3);
            let gray_str = "0".repeat(3 - digits) + &" ".repeat(digits);
            let white_str = format!("{:>3.0}", speed);
            p.text(center, egui::Align2::CENTER_CENTER,
                gray_str, fid.clone(), crate::theme::steel(70));
            p.text(center, egui::Align2::CENTER_CENTER,
                white_str, fid, Color32::WHITE);
        }
    }

    p.text(
        pos2(avail.left() + 4.0, avail.bottom() - 4.0),
        egui::Align2::LEFT_BOTTOM,
        unit_str,
        egui::FontId::proportional(12.0),
        legend_color,
    );

    if app.config.show_speed_delta {
        let sign = if app.speed_delta_kmh >= 0.0 { "+" } else { "" };
        p.text(
            pos2(avail.right() - 4.0, avail.bottom() - 4.0),
            egui::Align2::RIGHT_BOTTOM,
            format!("{sign}{:.1}", app.speed_delta_kmh),
            egui::FontId::proportional(11.0),
            legend_color,
        );
    }

    ui.allocate_space(avail.size());
}

fn show_gear_widget(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let gear_str = match pkt.gear {
        0 => "R".to_string(),
        1..=9 => pkt.gear.to_string(),
        _ => "N".to_string(),
    };
    let legend_color = crate::theme::TEXT_DIM;

    let avail = ui.available_rect_before_wrap();

    // Bottom strip holds the "Gear" label.
    let label_h = 20.0_f32;
    let main_h  = (avail.height() - label_h).max(label_h);

    // Gear is a single character; ~55% of font_size wide → divisor 0.7 gives breathing room.
    let font_size = (main_h * 0.90)
        .min(avail.width() / 0.7)
        .max(16.0);

    let center = pos2(avail.center().x, avail.top() + main_h * 0.5);
    let p = ui.painter();

    let gear_fmt = match app.config.gear_align {
        TextAlign::Right | TextAlign::RightPlaceholder => format!("{:>2}", gear_str),
        TextAlign::Center => gear_str,
    };

    p.text(center, egui::Align2::CENTER_CENTER,
        gear_fmt, egui::FontId::proportional(font_size), Color32::YELLOW);

    p.text(
        pos2(avail.left() + 4.0, avail.bottom() - 4.0),
        egui::Align2::LEFT_BOTTOM,
        tr("Gear"),
        egui::FontId::proportional(12.0),
        legend_color,
    );

    ui.allocate_space(avail.size());
}

/// Max RPM the dashboard uses (game-provided or dynamically detected redline).
/// A paused game zeroes `engine_max_rpm`, so the game-provided value comes from
/// the app's cache (which persists through pauses) and only falls back to the
/// live packet before the cache is first filled.
fn effective_max_rpm(app: &ForzaApp, pkt: &ForzaPacket) -> f32 {
    let game_max = if app.cached_engine_max_rpm > 0.0 {
        app.cached_engine_max_rpm as f32
    } else {
        pkt.engine_max_rpm
    };
    match app.config.max_rpm_mode {
        crate::config::MaxRpmSource::GameProvided => game_max,
        crate::config::MaxRpmSource::DetectDynamically => {
            if app.dynamic_max_rpm > 0.0 { app.dynamic_max_rpm } else { game_max }
        }
    }
    .max(1.0)
}

fn show_rpm_widget(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let max_rpm = effective_max_rpm(app, pkt);

    // The current + max RPM values live on the bar itself (see draw_shift_bar),
    // so the bar fills the whole widget height with just a tiny cell-edge margin.
    // Allocate the full height and inset equally (4 px) on all four sides so the
    // bar is centered with matching top and bottom gaps that mirror left/right.
    let bar_h = ui.available_height().max(4.0);
    let bar_size = Vec2::new(ui.available_width(), bar_h);
    let (rect, _) = ui.allocate_exact_size(bar_size, egui::Sense::hover());
    draw_shift_bar(
        ui,
        rect.shrink2(vec2(4.0, 4.0)),
        pkt,
        app.config.shift_low_pct,
        app.config.shift_high_pct,
        max_rpm,
    );
}

// ── Block renderers (unchanged) ────────────────────────────────────

fn show_inputs_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    widget_title(ui, app, tr("Inputs"));
    ui.add_space(4.0);

    // Backfire briefly injects a synthetic 'W' keypress, which the game reports back as a real
    // (but fake) pkt.accel for a few frames — hide that blip from the Accel bar when opted in.
    let suppress = app.config.inputs_filter_backfire_accel && app.backfire_echo_active();
    let accel = if suppress { 0 } else { pkt.accel };
    let rows = [
        (tr("Accel"),     accel,          Color32::from_rgb(60, 200, 90)),
        (tr("Brake"),     pkt.brake,      Color32::from_rgb(220, 60, 60)),
        (tr("Clutch"),    pkt.clutch,     Color32::from_rgb(80, 140, 220)),
        (tr("HandBrake"), pkt.hand_brake, Color32::from_rgb(230, 150, 40)),
    ];
    for (lbl, val, color) in rows {
        if app.config.input_bars_full_width {
            input_bar_full(ui, lbl, val, color);
        } else {
            input_bar(ui, lbl, val, color);
        }
    }

    ui.add_space(6.0);
    if !app.config.input_steer_compact {
        ui.label(tr("Steer"));
    }
    draw_steering(ui, pkt.steer);
}

fn show_car_block(ui: &mut Ui, app: &ForzaApp, _pkt: &ForzaPacket) {
    // Capture the full widget rect before the heading consumes any space.
    let full_rect = ui.available_rect_before_wrap();

    widget_title(ui, app, tr("Car"));

    let no_data = app.cached_car_class_str.is_empty();
    let class = if no_data { -1 } else { app.cached_car_class };
    let dt = if no_data { -1 } else { app.cached_drivetrain };
    let pi = if no_data { 0 } else { app.cached_car_pi };

    // Gap floors (also reserved in the scale budget below, so the drivetrain
    // label can never be pushed past the cell's bottom edge). Gaps here mean
    // the TOTAL visual gap — egui's implicit item_spacing.y after each widget
    // is part of it, so the add_space calls below subtract it back out.
    let gap = 4.0_f32;
    let spacing = ui.spacing().item_spacing.y;
    let floor_top = 4.0;
    let floor_mid = gap.max(spacing);
    let floor_bottom = spacing;
    let n_gaps = 3.0;
    let floor_total = floor_top + floor_mid + floor_bottom;

    // Scale the labels to the space that truly remains below the heading, minus
    // the gap floors.
    let avail_w = full_rect.width();
    let used_h = ui.next_widget_position().y - full_rect.min.y;
    let avail_h = (full_rect.height() - used_h).max(0.0);
    let cnative = app.labels.class_size(class, 1.0);
    let dnative = app.labels.drivetrain_size(dt, 1.0);
    let scale = (avail_w * 0.92 / cnative.x.max(dnative.x))
        .min((avail_h - floor_total).max(0.0) / (cnative.y + dnative.y))
        .clamp(0.2, 1.4);
    let csize = cnative * scale;
    let dsize = dnative * scale;

    // In a narrow/tall cell the label images are width-bound, leaving vertical
    // slack. Spread it across every gap (top margin, mid gap, bottom margin)
    // instead of one lump. Each gap keeps its floor, so tight cells fall back to
    // the original layout.
    let slack = (avail_h - floor_total - csize.y - dsize.y).max(0.0);
    let extra = slack / n_gaps;
    let top_margin = floor_top + extra;
    let mid_gap = floor_mid + extra;
    let bottom_margin = floor_bottom + extra;

    ui.add_space(top_margin);

    // Class label (centred), rating stamped into its box.
    let (crow, _) = ui.allocate_exact_size(egui::vec2(avail_w, csize.y), egui::Sense::hover());
    app.labels.paint_class(ui.painter(), class, pi,
        egui::pos2(crow.center().x - csize.x * 0.5, crow.min.y), scale);
    // Same here: the class row's implicit item_spacing.y counts toward mid_gap.
    ui.add_space((mid_gap - spacing).max(0.0));
    // Drivetrain label (centred).
    let (drow, _) = ui.allocate_exact_size(egui::vec2(avail_w, dsize.y), egui::Sense::hover());
    app.labels.paint_drivetrain(ui.painter(), dt,
        egui::pos2(drow.center().x - dsize.x * 0.5, drow.min.y), scale);
    ui.add_space((bottom_margin - spacing).max(0.0));
}

fn show_engine_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let full_rect = ui.available_rect_before_wrap();
    widget_title(ui, app, tr("Engine"));
    ui.add_space(4.0);

    let (boost_cur, boost_max, boost_unit) = if app.config.use_bar {
        (pkt.boost * 0.0689476, app.max_boost_psi * 0.0689476, "bar")
    } else {
        (pkt.boost, app.max_boost_psi, "PSI")
    };
    let power_cur = pkt.power_ps().max(0.0);
    let torque_cur = pkt.torque_nm().max(0.0);
    let boost_cur = boost_cur.max(0.0);
    let boost_max = boost_max.max(0.0);

    // Per line, the current value, the max value, and their units — the display mode
    // decides which are shown.
    use crate::config::EngineDisplayMode as EDM;
    let rows: [(f32, f32, &str, usize); 3] = [
        (power_cur,  app.max_power_ps,  "PS", 0),
        (torque_cur, app.max_torque_nm, "Nm", 0),
        (boost_cur,  boost_max,         boost_unit, 2),
    ];
    let labels = [tr("Power"), tr("Torque"), tr("Boost")];

    // Pad the label to the widest one (labels differ per language) so the value column
    // lines up; {unit:<3} pads "PS"/"Nm" so the "(" column matches "bar".
    let lw = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    // Full lines carry the "Power:/Torque:/Boost:" label; Both also gets a "(max …)" tail.
    let full: Vec<String> = rows.iter().zip(labels).map(|(&(cur, max, unit, dec), lbl)| {
        match app.config.engine_display_mode {
            EDM::Current => format!("{lbl:<lw$}  {cur:>6.dec$} {unit}"),
            EDM::Max     => format!("{lbl:<lw$}  {max:>6.dec$} {unit}"),
            EDM::Both    => format!("{lbl:<lw$}  {cur:>6.dec$} {unit:<3}  ({} {max:>6.dec$})", tr("max")),
        }
    }).collect();
    // Compact lines drop the label (and, for Both, the "max" word).
    let compact: Vec<String> = rows.iter().map(|&(cur, max, unit, dec)| {
        match app.config.engine_display_mode {
            EDM::Current => format!("{cur:>6.dec$} {unit}"),
            EDM::Max     => format!("{max:>6.dec$} {unit}"),
            EDM::Both    => format!("{cur:>6.dec$} {unit:<3}  ({max:>6.dec$})"),
        }
    }).collect();

    let body_font = egui::TextStyle::Body.resolve(ui.style());
    let avail = ui.available_width() - 2.0;
    let widest = |lines: &[String]| lines.iter().fold(0.0_f32, |w, s| {
        w.max(ui.painter().layout_no_wrap(s.clone(), body_font.clone(), Color32::WHITE).rect.width())
    });

    // Width scale: full lines if they fit at full size, else compact scaled to width.
    let (lines, w_scale) = if widest(&full) <= avail {
        (full, 1.0)
    } else {
        let cw = widest(&compact);
        (compact, if cw > avail { (avail / cw).max(0.5) } else { 1.0 })
    };

    // Height scale: all rows — the 3 value lines plus the optional type caption —
    // are one uniform size, evenly spaced, sized to fill the space left in the cell
    // below the heading. Shrink to fit, but never grow past the default size.
    let cap = app.config.engine_show_type;
    let n = if cap { 4.0 } else { 3.0 };
    let used_h = ui.next_widget_position().y - full_rect.min.y;
    let avail_h = (full_rect.height() - used_h).max(0.0);
    let lh = ui.painter()
        .layout_no_wrap("0".to_owned(), body_font.clone(), Color32::WHITE).rect.height();
    let sp = ui.spacing().item_spacing.y;
    let h_scale = (((avail_h - (n - 1.0) * sp) / (n * lh)).min(1.0)).max(0.5);

    let scale = w_scale.min(h_scale);
    let size = body_font.size * scale;

    // Center the value block (+ optional type caption) vertically in the space
    // below the heading, mirroring show_gforce_block: shift down by half the
    // slack between the available height and the scaled content height.
    let content_h = n * lh * scale + (n - 1.0) * sp * scale;
    ui.add_space(((avail_h - content_h) * 0.5).max(0.0));

    for line in lines { ui.label(egui::RichText::new(line).size(size)); }

    if cap {
        let type_text = if app.cached_num_cylinders == 0 {
            tr("Electric").to_string()
        } else {
            format!("{} {}", app.cached_num_cylinders, tr("Cylinders"))
        };
        // Same size as the value rows, centered; shrunk only if it would overflow width.
        let mut cap_size = size;
        let tw = ui.painter()
            .layout_no_wrap(type_text.clone(), egui::FontId::proportional(cap_size), Color32::WHITE)
            .rect.width();
        if tw > avail { cap_size = (cap_size * avail / tw).max(8.0); }
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(type_text).size(cap_size).color(crate::theme::TEXT_DIM));
        });
    }

}

fn show_position_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    widget_title(ui, app, tr("Position"));
    let Some(pane) = module_pane(ui) else { return };
    let base = egui::TextStyle::Body.resolve(ui.style()).size;
    paint_position(
        ui.painter(),
        pane,
        [pkt.position_x, pkt.position_y, pkt.position_z],
        [pkt.yaw, pkt.pitch, pkt.roll],
        base,
    );
}

/// Position body: world X/Y/Z beside Yaw/Pitch/Roll (wide pane) or the rotation block
/// stacked under the position block (narrow pane) — whichever fits bigger.
fn paint_position(painter: &egui::Painter, pane: Rect, pos: [f32; 3], rot: [f32; 3], base: f32) {
    let dim = crate::theme::TEXT_DIM;
    let txt = crate::theme::TEXT;
    let pos_lines = [
        format!("X: {:>10.2} m", pos[0]),
        format!("Y: {:>10.2} m", pos[1]),
        format!("Z: {:>10.2} m", pos[2]),
    ];
    let rot_lines: Vec<String> = [tr("Yaw"), tr("Pitch"), tr("Roll")]
        .iter()
        .zip(rot)
        .map(|(l, r)| format!("{:<7}{:>7.2}°", format!("{l}:"), r.to_degrees()))
        .collect();

    let mut side = vec![FitRow::new(tr("Position"), dim, tr("Rotation"), dim)];
    for (p, r) in pos_lines.iter().zip(&rot_lines) {
        side.push(FitRow::new(p.clone(), txt, r.clone(), txt));
    }
    let mut stacked = vec![FitRow::new(tr("Position"), dim, "", dim)];
    stacked.extend(pos_lines.iter().map(|p| FitRow::new(p.clone(), txt, "", txt)));
    stacked.push(FitRow::new(tr("Rotation"), dim, "", dim));
    stacked.extend(rot_lines.iter().map(|r| FitRow::new(r.clone(), txt, "", txt)));

    // Wide, short pane: the headers go, leaving three value rows.
    let bare = &side[1..];
    paint_best_fit(painter, pane, base, &[
        FitLayout::new(&side, RightCol::Column, 1),
        FitLayout::new(&stacked, RightCol::Column, 1),
        FitLayout::new(bare, RightCol::Column, 1),
    ]);
}

fn show_race_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    if pkt.race_position == 0 {
        // Captured before the heading so the height budget covers the whole cell.
        let full_rect = ui.available_rect_before_wrap();
        // Title flush like every other widget; the edge margin applies to the rows only.
        widget_title(ui, app, tr("Sprint"));
        ui.add_space(4.0);

        let st = &app.sprint_timer;
        let stype = &app.config.sprint_type;
        let show_other = app.config.sprint_show_other;

        let c100 = cumulative_time(&[st.zero_to_hundred]);
        let c200 = cumulative_time(&[st.zero_to_hundred, st.hundred_to_two]);
        let c300 = cumulative_time(&[st.zero_to_hundred, st.hundred_to_two, st.two_to_three]);
        let c400 = cumulative_time(&[
            st.zero_to_hundred,
            st.hundred_to_two,
            st.two_to_three,
            st.three_to_four,
        ]);
        let c500 = cumulative_time(&[
            st.zero_to_hundred,
            st.hundred_to_two,
            st.two_to_three,
            st.three_to_four,
            st.four_to_five,
        ]);

        let (lbl0, lbl1, lbl2, lbl3, lbl4) = match stype {
            SprintType::Incremental => {
                ("0 → 100", "100 → 200", "200 → 300", "300 → 400", "400 → 500")
            }
            SprintType::Absolute => {
                ("0 → 100", "0 → 200", "0 → 300", "0 → 400", "0 → 500")
            }
        };
        // Scale the rows to fit the cell, like the Engine widget: uniform size,
        // evenly spaced, shrunk to fit but never grown past the base sizes.
        let rows: [(&str, Option<f32>, Option<f32>, bool); 5] = [
            (lbl0, st.zero_to_hundred, c100, false),
            (lbl1, st.hundred_to_two,  c200, show_other),
            (lbl2, st.two_to_three,    c300, show_other),
            (lbl3, st.three_to_four,   c400, show_other),
            (lbl4, st.four_to_five,    c500, show_other),
        ];

        let item_sp_x = ui.spacing().item_spacing.x;
        let measure = |s: String, sz: f32| {
            ui.painter()
                .layout_no_wrap(s, egui::FontId::proportional(sz), Color32::WHITE)
                .rect
                .width()
        };
        // Width scale: widest row (label + main value + optional secondary) vs cell,
        // leaving an edge margin on both sides.
        let avail_w = (ui.available_width() - 2.0 * SPRINT_EDGE).max(1.0);
        let widest = rows.iter().fold(0.0_f32, |acc, &(lbl, seg, cum, so)| {
            let (main, secondary) = match stype {
                SprintType::Incremental => (seg, cum),
                SprintType::Absolute    => (cum, seg),
            };
            let mut w = measure(format!("{lbl:12}"), SPRINT_LABEL_SIZE) + item_sp_x;
            match main {
                Some(t) => {
                    w += measure(format!("{t:.3}s"), SPRINT_MAIN_SIZE);
                    if so {
                        if let Some(s) = secondary {
                            w += item_sp_x + measure(format!("({s:.3}s)"), SPRINT_SECONDARY_SIZE);
                        }
                    }
                }
                None => w += measure("--".to_owned(), SPRINT_MAIN_SIZE),
            }
            acc.max(w)
        });
        let w_scale = if widest > avail_w { (avail_w / widest).max(0.5) } else { 1.0 };

        // Height scale: 5 rows sized to fill the space left below the heading.
        let n = rows.len() as f32;
        let used_h = ui.next_widget_position().y - full_rect.min.y;
        let avail_h = (full_rect.height() - used_h - SPRINT_EDGE).max(0.0); // reserve bottom margin
        let lh = ui
            .painter()
            .layout_no_wrap("0".to_owned(), egui::FontId::proportional(SPRINT_MAIN_SIZE), Color32::WHITE)
            .rect
            .height();
        let sp = ui.spacing().item_spacing.y;
        // The inter-row spacing scales with the font too, so the rows fill the height
        // proportionally instead of leaving fixed gaps when the font shrinks.
        let h_scale = (avail_h / (n * lh + (n - 1.0) * sp)).clamp(0.5, 1.0);

        let scale = w_scale.min(h_scale);
        ui.spacing_mut().item_spacing.y = sp * scale;

        sprint_row(ui, lbl0, st.zero_to_hundred, c100, stype, false, scale);
        sprint_row(ui, lbl1, st.hundred_to_two, c200, stype, show_other, scale);
        sprint_row(ui, lbl2, st.two_to_three, c300, stype, show_other, scale);
        sprint_row(ui, lbl3, st.three_to_four, c400, stype, show_other, scale);
        sprint_row(ui, lbl4, st.four_to_five, c500, stype, show_other, scale);
    } else {
        // Captured before the heading so the height budget covers the whole cell.
        let full_rect = ui.available_rect_before_wrap();
        widget_title(ui, app, tr("Race"));
        ui.add_space(4.0);

        // (text, base font size, colour, gap reserved above the row). Scaled to fit
        // the cell in both axes just like the Sprint branch above.
        let rows: [(String, f32, Option<Color32>, f32); 7] = [
            (format!("{}  P{}", tr("Position"), pkt.race_position), 14.0, None, 0.0),
            (format!("{:<9} {}", tr("Lap"), pkt.lap_number), 14.0, None, 0.0),
            (format!("{:<9} {}", tr("Current"), fmt_lap(pkt.current_lap)), 15.0, None, 6.0),
            (format!("{:<9} {}", tr("Last"), fmt_lap(pkt.last_lap)), 15.0, None, 0.0),
            (format!("{:<9} {}", tr("Best"), fmt_lap(pkt.best_lap)), 15.0, Some(Color32::from_rgb(255, 210, 40)), 0.0),
            (format!("{} {}", tr("Race time"), fmt_lap(pkt.current_race_time)), 14.0, None, 8.0),
            (format!("{}  {:.1} km", tr("Distance"), pkt.distance_traveled / 1000.0), 14.0, None, 0.0),
        ];

        let measure = |s: &str, sz: f32| {
            ui.painter()
                .layout_no_wrap(s.to_owned(), egui::FontId::proportional(sz), Color32::WHITE)
        };
        // Width scale: widest row vs the cell, leaving an edge margin on both sides.
        let avail_w = (ui.available_width() - 2.0 * SPRINT_EDGE).max(1.0);
        let widest = rows.iter().fold(0.0_f32, |acc, (t, sz, _, _)| {
            acc.max(measure(t, *sz).rect.width())
        });
        let w_scale = if widest > avail_w { (avail_w / widest).max(0.5) } else { 1.0 };

        // Height scale: sum of natural row heights + reserved gaps vs the space left.
        let used_h = ui.next_widget_position().y - full_rect.min.y;
        let avail_h = (full_rect.height() - used_h - SPRINT_EDGE).max(0.0);
        let sp = ui.spacing().item_spacing.y;
        let natural_h: f32 = rows.iter().map(|(t, sz, _, gap)| {
            measure(t, *sz).rect.height() + sp + gap
        }).sum();
        let h_scale = if natural_h > 0.0 { (avail_h / natural_h).clamp(0.5, 1.0) } else { 1.0 };

        let scale = w_scale.min(h_scale);
        ui.spacing_mut().item_spacing.y = sp * scale;

        for (text, size, color, gap) in rows {
            if gap > 0.0 {
                ui.add_space(gap * scale);
            }
            let mut rt = RichText::new(text).size(size * scale);
            if let Some(c) = color {
                rt = rt.color(c);
            }
            ui.horizontal(|ui| {
                ui.add_space(SPRINT_EDGE); // left margin, matching the Sprint rows
                ui.label(rt);
            });
        }
    }
}

fn show_tires_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    widget_title(ui, app, tr("Tires"));
    ui.add_space(4.0);
    match app.config.tire_display_style {
        TireDisplayStyle::Tires => show_tires_tiles(ui, app, pkt),
        TireDisplayStyle::Bars  => show_tires_bars(ui, app, pkt),
    }
}

fn show_tires_tiles(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let use_f = app.config.use_fahrenheit;

    let tires = [
        ("FL", pkt.tire_temp_fl, pkt.tire_combined_slip_fl, pkt.wheel_in_puddle_fl),
        ("FR", pkt.tire_temp_fr, pkt.tire_combined_slip_fr, pkt.wheel_in_puddle_fr),
        ("RL", pkt.tire_temp_rl, pkt.tire_combined_slip_rl, pkt.wheel_in_puddle_rl),
        ("RR", pkt.tire_temp_rr, pkt.tire_combined_slip_rr, pkt.wheel_in_puddle_rr),
    ];

    // Choose a grid shape from the widget's aspect ratio:
    //   wide  → 1×4 (single horizontal row)
    //   square→ 2×2 (FL FR / RL RR)
    //   tall  → 4×1 (single vertical column)
    let avail = ui.available_rect_before_wrap();
    let avail_w = avail.width();
    let avail_h = avail.height();
    let (cols, rows) = if avail_w > avail_h * 1.3 {
        (4usize, 1usize)
    } else if avail_h > avail_w * 1.3 {
        (1usize, 4usize)
    } else {
        (2usize, 2usize)
    };

    let gap = 8.0_f32;
    let left_pad = 5.0_f32;
    let right_pad = 5.0_f32;
    let bottom_pad = 5.0_f32;
    // Cap the cell (circle) size by both the per-cell width and height so nothing clips.
    // Reserve the same ~5px margin on the left, right and bottom (the top keeps the
    // small gap the layout already leaves after the widget title) so the gauges never
    // spill past the widget's bottom edge in the tall/square layouts.
    let cell_w = (avail_w - left_pad - right_pad - (cols as f32 - 1.0) * gap) / cols as f32;
    let cell_h = (avail_h - bottom_pad - (rows as f32 - 1.0) * gap) / rows as f32;
    let cell = cell_w.min(cell_h).max(10.0);
    let outer_r = cell / 2.0;
    let inner_r = outer_r * 0.55;

    let grid_w = left_pad + cols as f32 * cell + (cols as f32 - 1.0) * gap;
    let grid_h = rows as f32 * cell + (rows as f32 - 1.0) * gap + bottom_pad;

    let (rect, _) = ui.allocate_exact_size(Vec2::new(grid_w, grid_h), egui::Sense::hover());
    let hole_bg = ui.visuals().panel_fill;
    let p = ui.painter();

    let bg = crate::theme::WELL;
    let puddle_c = Color32::from_rgb(80, 160, 220);
    let rim_c = crate::theme::STROKE_MID;

    let font_size = ((inner_r / 1.8) * 0.8).max(8.0);
    let line_h = font_size * 1.1;
    let fid = egui::FontId::proportional(font_size);

    for (i, &(label, temp_f, slip, puddle)) in tires.iter().enumerate() {
        // Reading order (row-major): FL FR on top, RL RR on bottom for 2×2.
        let col = i % cols;
        let row = i / cols;
        let cx = rect.left() + left_pad + col as f32 * (cell + gap) + outer_r;
        let cy = rect.top() + row as f32 * (cell + gap) + outer_r;
        let center = pos2(cx, cy);

        let slip_abs = slip.abs();
        let grip_color = if slip_abs >= 1.0 {
            Color32::from_rgb(220, 60, 60)
        } else if slip_abs >= 0.8 {
            Color32::from_rgb(230, 160, 40)
        } else {
            Color32::from_rgb(60, 200, 90)
        };

        p.circle_filled(center, outer_r, bg);
        let fill_r = inner_r + slip_abs.min(1.0) * (outer_r - inner_r);
        p.circle_filled(center, fill_r, grip_color);
        p.circle_filled(center, inner_r, hole_bg);
        p.circle_stroke(center, inner_r - 1.0, Stroke::new(1.5, rim_c));
        let outline = if puddle != 0 { puddle_c } else { rim_c };
        p.circle_stroke(center, outer_r, Stroke::new(1.5, outline));

        let temp_val = if use_f { temp_f } else { ForzaPacket::tire_temp_celsius(temp_f) };
        let temp_unit = if use_f { "°F" } else { "°C" };
        let temp_str = format!("{:.0}{temp_unit}", temp_val);
        let temp_c = temp_color(temp_val, use_f);
        let slip_str = format!("{:.2}", slip);

        p.text(pos2(cx, cy - line_h), egui::Align2::CENTER_CENTER, label,    fid.clone(), Color32::WHITE);
        p.text(pos2(cx, cy),          egui::Align2::CENTER_CENTER, temp_str,  fid.clone(), temp_c);
        p.text(pos2(cx, cy + line_h), egui::Align2::CENTER_CENTER, slip_str,  fid.clone(), grip_color);
    }
}

fn show_tires_bars(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let use_f = app.config.use_fahrenheit;
    let temps_f = [pkt.tire_temp_fl, pkt.tire_temp_fr, pkt.tire_temp_rl, pkt.tire_temp_rr];
    let slips = [
        pkt.tire_combined_slip_fl,
        pkt.tire_combined_slip_fr,
        pkt.tire_combined_slip_rl,
        pkt.tire_combined_slip_rr,
    ];
    let puddles = [
        pkt.wheel_in_puddle_fl,
        pkt.wheel_in_puddle_fr,
        pkt.wheel_in_puddle_rl,
        pkt.wheel_in_puddle_rr,
    ];

    let avail_h  = ui.available_rect_before_wrap().height();
    let avail_w  = ui.available_width();
    let label_w  = four_mono_chars(ui);   // "Temp"/"Slip"/unit column — matches Suspension
    let header_h = 18.0_f32;   // "FL"/"FR"/... row
    let text_h   = 14.0_f32;   // height per text row
    let bar_w    = (avail_w - label_w - 4.0) / 4.0;  // 4 px right margin
    let gap_h    = 4.0_f32;                            // gap between bars and text rows
    let bar_h    = (avail_h - header_h - gap_h - 3.0 * text_h).max(24.0);
    let total_h  = header_h + bar_h + gap_h + 3.0 * text_h;

    let origin = ui.cursor().min;
    ui.allocate_exact_size(vec2(avail_w, total_h), egui::Sense::hover());

    let p  = ui.painter();
    let fid = egui::FontId::proportional(11.0);
    let dim = crate::theme::TEXT_DIM;
    let text_col = ui.visuals().text_color();
    let puddle_c = Color32::from_rgb(80, 160, 220);

    // ── Column header: FL / FR / RL / RR ──────────────────────────
    for (i, lbl) in ["FL", "FR", "RL", "RR"].iter().enumerate() {
        let cx = origin.x + label_w + (i as f32 + 0.5) * bar_w;
        let cy = origin.y + header_h * 0.5;
        p.text(pos2(cx, cy), egui::Align2::CENTER_CENTER, *lbl, fid.clone(), text_col);
    }

    // ── Bars: what they visualize is configurable (temp / slip / both) ──
    use crate::config::TireBarValue;
    let bar_top = origin.y + header_h;
    let mode = app.config.tire_bar_value;
    // "Switch Values" swaps temp/slip in the bars only — never in the text rows.
    let swap = app.config.tire_bar_swap;
    let slip_color = |slip: f32| {
        let abs = slip.abs();
        if abs >= 1.0 { Color32::from_rgb(220, 60, 60) }
        else if abs >= 0.8 { Color32::from_rgb(230, 160, 40) }
        else { Color32::from_rgb(60, 200, 90) }
    };
    // Each metric yields (fill fraction 0..1, fill colour). The bar uses the same
    // temp_color helper as the value row, so bar and number always match.
    let temp_metric = |i: usize| {
        let t_c = ForzaPacket::tire_temp_celsius(temps_f[i]);
        let frac = ((t_c - 30.0) / 100.0).clamp(0.0, 1.0);
        (frac, temp_color(t_c, false))
    };
    let slip_metric = |i: usize| (slips[i].abs().clamp(0.0, 1.0), slip_color(slips[i]));

    // Snap the bar rects to the physical pixel grid: bar_w is fractional, so
    // unsnapped x-coords make fills/separators bleed 1 px on some bars only.
    let ppp  = p.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let px   = 1.0 / ppp;
    let bar_snap_w = snap(bar_w - 8.0).max(px);

    // ── Left-column legend for two-metric bar modes ───────────────
    // Combined/Stacked pack two metrics into each bar; label which half is
    // which with small rotated text in the empty column left of the bars.
    // Same mapping for all four bars, so one pair of labels is enough.
    if matches!(mode, TireBarValue::Combined | TireBarValue::Stacked) {
        let a_name = if swap { tr("Slip") } else { tr("Temp") };
        let b_name = if swap { tr("Temp") } else { tr("Slip") };
        let lfid = egui::FontId::proportional(10.0);
        // -FRAC_PI_2 = 90° CCW, so text reads bottom-to-top.
        let angle = -std::f32::consts::FRAC_PI_2;
        // Draw `text` rotated 90° CCW, centered inside `cell`.
        let draw_rot = |text: &str, cell: Rect| {
            let galley = p.layout_no_wrap(text.to_owned(), lfid.clone(), dim);
            let sz = galley.size();
            // After CCW rotation the galley's width maps to vertical extent and
            // its height to horizontal extent; place the un-rotated top-left so
            // the rotated box lands centered in `cell`.
            let pos = pos2(cell.center().x - sz.y * 0.5, cell.center().y + sz.x * 0.5);
            p.add(egui::epaint::TextShape::new(pos, galley, dim).with_angle(angle));
        };
        let col = Rect::from_min_max(
            pos2(origin.x, bar_top),
            pos2(origin.x + label_w, bar_top + bar_h),
        );
        match mode {
            TireBarValue::Combined => {
                // Side by side: left label = a (left half), right label = b.
                let (l, r) = col.split_left_right_at_fraction(0.5);
                draw_rot(a_name, l);
                draw_rot(b_name, r);
            }
            TireBarValue::Stacked => {
                // Stacked: top label = a (top half), bottom label = b.
                let (t, btm) = col.split_top_bottom_at_fraction(0.5);
                draw_rot(a_name, t);
                draw_rot(b_name, btm);
            }
            _ => {}
        }
    }

    for i in 0..4 {
        let x    = origin.x + label_w + i as f32 * bar_w;
        let rect = Rect::from_min_size(
            pos2(snap(x + 4.0), snap(bar_top)),
            vec2(bar_snap_w, snap(bar_h)),
        );

        p.rect_filled(rect, 2.0, crate::theme::TRACK);

        let fill_up = |r: Rect, (frac, col): (f32, Color32)| {
            if frac > 0.001 {
                p.rect_filled(
                    Rect::from_min_max(pos2(r.left(), r.bottom() - frac * r.height()), r.max),
                    0.0, col,
                );
            }
        };
        let (a, b) = if swap {
            (slip_metric(i), temp_metric(i))
        } else {
            (temp_metric(i), slip_metric(i))
        };
        match mode {
            TireBarValue::Temperature => fill_up(rect, temp_metric(i)),
            TireBarValue::Slip => fill_up(rect, slip_metric(i)),
            TireBarValue::Combined => {
                // Two half-width bars side by side, 1 px seam.
                let (l, r) = rect.split_left_right_at_fraction(0.5);
                fill_up(Rect::from_min_max(l.min, pos2(l.max.x - 0.5, l.max.y)), a);
                fill_up(Rect::from_min_max(pos2(r.min.x + 0.5, r.min.y), r.max), b);
            }
            TireBarValue::Stacked => {
                // Split at the vertical middle: `a` grows upward, `b` downward.
                let mid = snap(rect.center().y);
                let half = rect.height() * 0.5;
                if a.0 > 0.001 {
                    p.rect_filled(
                        Rect::from_min_max(pos2(rect.left(), mid - a.0 * half), pos2(rect.right(), mid)),
                        0.0, a.1,
                    );
                }
                if b.0 > 0.001 {
                    p.rect_filled(
                        Rect::from_min_max(pos2(rect.left(), mid), pos2(rect.right(), mid + b.0 * half)),
                        0.0, b.1,
                    );
                }
                // Separator: 1-physical-px filled rect exactly the bar's width.
                p.rect_filled(
                    Rect::from_min_size(pos2(rect.left(), mid), vec2(rect.width(), px)),
                    0.0, crate::theme::STROKE_MID,
                );
            }
        }

        // Wet: water-blue inset outline, drawn inside so the bar keeps its size
        if puddles[i] != 0 {
            p.rect_stroke(rect, 2.0, Stroke::new(2.0, puddle_c), egui::StrokeKind::Inside);
        }
    }

    // ── Text rows: Temp / Slip / wheel speed ───────────────────────
    let temp_unit = if use_f { "°F" } else { "°C" };
    let use_mph = app.config.use_mph;
    let speed_lbl = if use_mph { tr("Mp/h") } else { tr("Km/h") };
    let speed_factor = if use_mph { 2.236_94 } else { 3.6 };
    let rotations = [
        pkt.wheel_rotation_speed_fl,
        pkt.wheel_rotation_speed_fr,
        pkt.wheel_rotation_speed_rl,
        pkt.wheel_rotation_speed_rr,
    ];
    let text_top = bar_top + bar_h + gap_h;
    let rows: [(&str, [(String, Color32); 4]); 3] = [
        (tr("Temp"), std::array::from_fn(|i| {
            let val = if use_f { temps_f[i] } else { ForzaPacket::tire_temp_celsius(temps_f[i]) };
            (format!("{val:.0}{temp_unit}"), temp_color(val, use_f))
        })),
        (tr("Slip"), std::array::from_fn(|i| (format!("{:.2}", slips[i]), slip_color(slips[i])))),
        // Per-wheel speed from rotation × estimated radius; slip-coloured on wheelspin
        (speed_lbl, std::array::from_fn(|i| {
            let v = rotations[i] * app.wheel_radius_est[i] * speed_factor;
            let col = if slips[i].abs() >= 0.8 { slip_color(slips[i]) } else { text_col };
            (format!("{v:.0}"), col)
        })),
    ];
    for (row_i, (lbl, vals)) in rows.iter().enumerate() {
        let cy = text_top + (row_i as f32 + 0.5) * text_h;
        // Row label centered in its column
        p.text(pos2(origin.x + label_w * 0.5, cy), egui::Align2::CENTER_CENTER, *lbl, fid.clone(), dim);
        // Values centered under each bar
        for (i, (val, color)) in vals.iter().enumerate() {
            let cx = origin.x + label_w + (i as f32 + 0.5) * bar_w;
            p.text(pos2(cx, cy), egui::Align2::CENTER_CENTER, val, fid.clone(), *color);
        }
    }
}

fn show_gforce_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    widget_title(ui, app, tr("G-Forces"));
    ui.add_space(4.0);

    let lat = pkt.acceleration_x / 9.81;
    let lon = pkt.acceleration_z / 9.81;
    let vert = pkt.acceleration_y / 9.81;

    let avail_w = ui.available_width();
    let avail_h = ui.available_rect_before_wrap().height();
    // No extra left pad: the widget's own 2px content inset already matches the plot's
    // bottom spacing, so an added left_pad made the left gap visibly bigger.
    let left_pad = 0.0_f32;
    let right_pad = 4.0_f32;
    let gap = 8.0_f32;

    // Hack NerdFont is monospace: every glyph has the same advance width.
    // advance_width = font_size × 0.60  (Hack's fixed advance ratio).
    // Widest possible line: "  Long: +99.00 g" = 16 chars.
    let body_h = ui.text_style_height(&egui::TextStyle::Body);
    let show_text = app.config.gforce_show_text;
    let effective_gap = if show_text { gap } else { 0.0 };

    // The plot gets priority: a square sized by the available HEIGHT so it's as big as
    // the cell allows. When text is shown we only hold back the minimum width the text
    // needs at its smallest (0.5×) size; the rest is the text column and the font scales
    // up to fill it (capped at the default body size). When hidden, the plot spans all.
    let min_text_w = if show_text { 16.0 * body_h * 0.60 * 0.5 } else { 0.0 };
    let plot_cap_w = (avail_w - left_pad - right_pad - effective_gap - min_text_w).max(40.0);
    let plot_size = avail_h.max(40.0).min(plot_cap_w);

    ui.horizontal(|ui| {
        ui.add_space(left_pad);
        draw_gforce_plot(ui, lat, lon, &app.gforce_stats, plot_size);
        if !show_text { return; }
        ui.add_space(gap);
        ui.vertical(|ui| {
            let show_labels = app.config.gforce_show_labels;
            // Peak marker orange, matching the peak ring drawn by draw_gforce_plot.
            let peak_col = Color32::from_rgb(255, 140, 0);
            // Build the lines up front so we can measure the widest and scale to fit.
            let hdr_cur  = tr("Current").to_string();
            let hdr_peak = tr("Peak").to_string();
            let cur_lat  = format!("  {:<5} {:+.2} g", format!("{}:", tr("Lat")),  lat);
            let cur_long = format!("  {:<5} {:+.2} g", format!("{}:", tr("Long")), lon);
            let cur_vert = format!("  {:<5} {:+.2} g", format!("{}:", tr("Vert")), vert);
            let pk_lat  = format!("  {:<5} {:.2} g", format!("{}:", tr("Lat")),  app.gforce_stats.max_lateral);
            let pk_long = format!("  {:<5} {:.2} g", format!("{}:", tr("Long")), app.gforce_stats.max_longitudinal);
            let pk_vert = format!("  {:<5} {:.2} g", format!("{}:", tr("Vert")), app.gforce_stats.max_vertical);

            // Dynamic font scaling, same approach as the Engine widget: measure at the
            // default body size, then derive width/height scales, clamped to 0.5..=1.0.
            // The header rows only participate (in width and in the height budget) when shown.
            let body_font = egui::TextStyle::Body.resolve(ui.style());
            let avail = (ui.available_width() - 2.0).max(1.0);
            let mut lines: Vec<&String> = vec![&cur_lat, &cur_long, &cur_vert, &pk_lat, &pk_long, &pk_vert];
            if show_labels { lines.push(&hdr_cur); lines.push(&hdr_peak); }
            let widest = lines.iter().fold(0.0_f32, |w, s| {
                w.max(ui.painter().layout_no_wrap((*s).clone(), body_font.clone(), Color32::WHITE).rect.width())
            });
            let w_scale = if widest > avail { (avail / widest).max(0.5) } else { 1.0 };

            // Height budget: value rows plus the two header rows when shown (8 lines) or
            // just the 6 value rows when hidden — plus the 2 px break between the Current
            // and Peak blocks — as ONE unit that scales together (the gaps scale with the
            // font too, otherwise the fixed gaps make it overflow). Reserve a ~4 px bottom
            // margin so the block never clips the very bottom edge, then center in it.
            let lh = ui.painter()
                .layout_no_wrap("0".to_owned(), body_font.clone(), Color32::WHITE).rect.height();
            let sp = ui.spacing().item_spacing.y;
            let n_lines = if show_labels { 8.0 } else { 6.0 };
            let text_avail_h = (avail_h - 4.0).max(1.0);
            let unit_h = n_lines * lh + (n_lines - 1.0) * sp + 2.0;
            let h_scale = (text_avail_h / unit_h).clamp(0.5, 1.0);
            let scale = w_scale.min(h_scale);
            let size = body_font.size * scale;

            ui.add_space(((text_avail_h - unit_h * scale) * 0.5).max(0.0)); // center vertically
            ui.spacing_mut().item_spacing.y = sp * scale;

            if show_labels {
                ui.label(RichText::new(hdr_cur).size(size).color(crate::theme::TEXT_DIM));
            }
            ui.label(RichText::new(cur_lat).size(size));
            ui.label(RichText::new(cur_long).size(size));
            ui.label(RichText::new(cur_vert).size(size));
            ui.add_space(2.0 * scale);
            if show_labels {
                ui.label(RichText::new(hdr_peak).size(size).color(crate::theme::TEXT_DIM));
            }
            ui.label(RichText::new(pk_lat).size(size).color(peak_col));
            ui.label(RichText::new(pk_long).size(size).color(peak_col));
            ui.label(RichText::new(pk_vert).size(size).color(peak_col));
        });
    });
}

fn show_suspension_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let susp = &app.suspension_stats;
    let travels = [
        pkt.normalized_suspension_travel_fl,
        pkt.normalized_suspension_travel_fr,
        pkt.normalized_suspension_travel_rl,
        pkt.normalized_suspension_travel_rr,
    ];

    // "Invert values" (default on) shows suspension height — extension grows the
    // bar upward — instead of the raw normalized travel (1.0 = fully compressed).
    // Applied uniformly to the bar fill, the min/max reference lines and the
    // Cur/Min/Max numeric rows so bars and numbers always agree.
    let invert = app.config.suspension_invert;
    let disp = |v: f32| if invert { 1.0 - v } else { v };

    widget_title(ui, app, tr("Suspension"));
    ui.add_space(4.0);

    let avail_h  = ui.available_rect_before_wrap().height();
    let avail_w  = ui.available_width();
    let label_w  = four_mono_chars(ui);   // "Cur"/"Min"/"Max" column — matches Tires
    let header_h = 18.0_f32;   // "FL"/"FR"/... row
    let text_h   = 14.0_f32;   // height per text row
    let bar_w    = (avail_w - label_w - 4.0) / 4.0;  // 4 px right margin
    let gap_h    = 4.0_f32;                            // gap between bars and text rows
    let bar_h    = (avail_h - header_h - gap_h - 3.0 * text_h).max(24.0);
    let total_h  = header_h + bar_h + gap_h + 3.0 * text_h;

    let origin = ui.cursor().min;
    ui.allocate_exact_size(vec2(avail_w, total_h), egui::Sense::hover());

    let p  = ui.painter();
    let fid = egui::FontId::proportional(11.0);
    let red   = Color32::from_rgb(180,  80,  80);
    let green = Color32::from_rgb( 80, 180,  80);
    let dim   = crate::theme::TEXT_DIM;
    let text_col = ui.visuals().text_color();

    // ── Column header: FL / FR / RL / RR ──────────────────────────
    for (i, lbl) in ["FL", "FR", "RL", "RR"].iter().enumerate() {
        let cx = origin.x + label_w + (i as f32 + 0.5) * bar_w;
        let cy = origin.y + header_h * 0.5;
        p.text(pos2(cx, cy), egui::Align2::CENTER_CENTER, *lbl, fid.clone(), text_col);
    }

    // ── Bars ──────────────────────────────────────────────────────
    // Snap track and fill rects to the physical pixel grid so fractional
    // x accumulation (slots are avail/4 wide) never lets a fill overflow
    // its track by a pixel.
    let ppp  = p.ctx().pixels_per_point();
    let px   = |v: f32| (v * ppp).round() / ppp;
    let bar_top = origin.y + header_h;
    for (i, &cur) in travels.iter().enumerate() {
        let x    = origin.x + label_w + i as f32 * bar_w;
        let rect = Rect::from_min_max(
            pos2(px(x + 4.0), px(bar_top)),
            pos2(px(x + bar_w - 4.0), px(bar_top + bar_h)),
        );

        p.rect_filled(rect, 2.0, crate::theme::TRACK);

        // Diverging bar: grows up or down from a 0.5 baseline instead of filling
        // from the bottom. Colour tracks the physical state (raw cur), so it stays
        // correct whether or not the invert toggle flips the display direction.
        let c     = disp(cur).clamp(0.0, 1.0);
        let mid_y = px(rect.top() + rect.height() * 0.5);
        let c_y   = px(rect.bottom() - c * rect.height());
        let (fy0, fy1) = if c_y <= mid_y { (c_y, mid_y) } else { (mid_y, c_y) };
        let fill = Rect::from_min_max(pos2(rect.left(), fy0), pos2(rect.right(), fy1));
        let bar_color = if cur.clamp(0.0, 1.0) >= 0.5 { Color32::from_rgb(215, 85, 70) }  // compressed
                        else { Color32::from_rgb(75, 190, 95) };                          // extended
        p.rect_filled(fill, 0.0, bar_color);
        // Baseline so the 0.5 zero point is readable.
        p.line_segment([pos2(rect.left(), mid_y), pos2(rect.right(), mid_y)],
            Stroke::new(1.0, crate::theme::TEXT_DIM));

        let alpha = if susp.initialized { 255u8 } else { 80u8 };
        let min_y = rect.bottom() - disp(susp.min[i]).clamp(0.0, 1.0) * rect.height();
        let max_y = rect.bottom() - disp(susp.max[i]).clamp(0.0, 1.0) * rect.height();
        // Track-coloured lines above and below make the thin marker pop against the fill.
        let marker = |y: f32, col: Color32| {
            p.line_segment([pos2(rect.left(), y - 1.0), pos2(rect.right(), y - 1.0)],
                Stroke::new(1.0, crate::theme::TRACK));
            p.line_segment([pos2(rect.left(), y + 1.0), pos2(rect.right(), y + 1.0)],
                Stroke::new(1.0, crate::theme::TRACK));
            p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, col));
        };
        marker(min_y, Color32::from_rgba_premultiplied(180, 80, 80, alpha));
        marker(max_y, Color32::from_rgba_premultiplied(80, 180, 80, alpha));
    }

    // ── Left-column legend: which end of the bar is which ──────────
    // A full bar reaches the top; label its meaning there. With invert on
    // (default) a full bar = extension, so top = Extended / bottom = Compressed.
    {
        let lfid = egui::FontId::proportional(10.0);
        // -FRAC_PI_2 = 90° CCW, so text reads bottom-to-top.
        let angle = -std::f32::consts::FRAC_PI_2;
        // Draw `text` rotated 90° CCW, centered inside `cell`.
        let draw_rot = |text: &str, cell: Rect| {
            let galley = p.layout_no_wrap(text.to_owned(), lfid.clone(), dim);
            let sz = galley.size();
            let pos = pos2(cell.center().x - sz.y * 0.5, cell.center().y + sz.x * 0.5);
            p.add(egui::epaint::TextShape::new(pos, galley, dim).with_angle(angle));
        };
        let col = Rect::from_min_max(
            pos2(origin.x, bar_top),
            pos2(origin.x + label_w, bar_top + bar_h),
        );
        let (top, btm) = col.split_top_bottom_at_fraction(0.5);
        let (top_lbl, btm_lbl) = if invert {
            (tr("Extended"), tr("Compressed"))
        } else {
            (tr("Compressed"), tr("Extended"))
        };
        draw_rot(top_lbl, top);
        draw_rot(btm_lbl, btm);
    }

    // ── Text rows: Cur / Min / Max ─────────────────────────────────
    let text_top = bar_top + bar_h + gap_h;
    let rows: [(&str, Color32, [String; 4]); 3] = [
        (tr("Cur"), dim,   travels.map(|v| format!("{:.2}", disp(v)))),
        (tr("Min"), red,   std::array::from_fn(|i| if susp.initialized { format!("{:.2}", disp(susp.min[i])) } else { "0.00".into() })),
        (tr("Max"), green, std::array::from_fn(|i| if susp.initialized { format!("{:.2}", disp(susp.max[i])) } else { "0.00".into() })),
    ];
    for (row_i, (lbl, color, vals)) in rows.iter().enumerate() {
        let cy = text_top + (row_i as f32 + 0.5) * text_h;
        // Row label centered in its column
        p.text(pos2(origin.x + label_w * 0.5, cy), egui::Align2::CENTER_CENTER, *lbl, fid.clone(), *color);
        // Values centered under each bar
        for (i, val) in vals.iter().enumerate() {
            let cx = origin.x + label_w + (i as f32 + 0.5) * bar_w;
            p.text(pos2(cx, cy), egui::Align2::CENTER_CENTER, val, fid.clone(), *color);
        }
    }
}

/// Width of four monospace characters at the row font size (11pt) — the fixed
/// label-column width shared by the Tires and Suspension widgets so they align.
fn four_mono_chars(ui: &Ui) -> f32 {
    ui.painter()
        .layout_no_wrap("0000".to_owned(), egui::FontId::monospace(11.0), crate::theme::TEXT_DIM)
        .size()
        .x
}

// ── Visual widgets ─────────────────────────────────────────────────

fn draw_steering(ui: &mut Ui, steer: i8) {
    let desired = Vec2::new(ui.available_width(), (ui.available_height() - 4.0).max(4.0));
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let rect = rect.shrink2(vec2(3.0, 0.0));
    let painter = ui.painter();

    painter.rect_filled(rect, 4.0, crate::theme::TRACK);

    let norm = (steer as f32 / 127.0).clamp(-1.0, 1.0);
    let cx = rect.center().x;
    let end_x = cx + norm * (rect.width() / 2.0);

    if norm.abs() > 0.001 {
        let (fill_left, fill_right, fill_rounding) = if norm >= 0.0 {
            (cx, end_x, egui::CornerRadius { nw: 0, ne: 4, sw: 0, se: 4 })
        } else {
            (end_x, cx, egui::CornerRadius { nw: 4, ne: 0, sw: 4, se: 0 })
        };
        let fill = egui::Rect::from_x_y_ranges(fill_left..=fill_right, rect.top()..=rect.bottom());
        painter.rect_filled(fill, fill_rounding, Color32::from_rgb(50, 200, 80));
    }

    painter.line_segment(
        [pos2(cx, rect.top()), pos2(cx, rect.bottom())],
        Stroke::new(2.0, Color32::from_rgb(80, 120, 220)),
    );
}

fn draw_gforce_plot(ui: &mut Ui, lat: f32, lon: f32, stats: &GForceStats, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let max_g = 3.0_f32;
    let radius = size / 2.0 - 4.0;

    painter.circle_filled(center, radius, crate::theme::WELL);
    painter.circle_stroke(center, radius, Stroke::new(1.0, crate::theme::STROKE_MID));

    for g in [1.0_f32, 2.0] {
        painter.circle_stroke(
            center,
            g / max_g * radius,
            Stroke::new(0.5, crate::theme::STROKE_DIM),
        );
    }

    let dim = crate::theme::STROKE_DIM;
    painter.line_segment(
        [pos2(center.x - radius, center.y), pos2(center.x + radius, center.y)],
        Stroke::new(0.5, dim),
    );
    painter.line_segment(
        [pos2(center.x, center.y - radius), pos2(center.x, center.y + radius)],
        Stroke::new(0.5, dim),
    );

    let peak_mag =
        (stats.peak_lateral.powi(2) + stats.peak_longitudinal.powi(2)).sqrt();
    if peak_mag > 0.01 {
        let (pdx, pdy) = clip_to_circle(
            -(stats.peak_lateral / max_g * radius),
            stats.peak_longitudinal / max_g * radius,
            radius,
        );
        painter.circle_stroke(
            pos2(center.x + pdx, center.y + pdy),
            4.0,
            Stroke::new(1.5, Color32::from_rgb(255, 140, 0)),
        );
    }

    // Fading trail of recent G-vectors — shows how load transferred over the last ~1.5 s.
    let hist = &stats.g_history;
    let n = hist.len();
    if n >= 2 {
        for i in 1..n {
            let (_, la0, lo0) = hist[i - 1];
            let (_, la1, lo1) = hist[i];
            let (x0, y0) = clip_to_circle(-(la0 / max_g * radius), lo0 / max_g * radius, radius);
            let (x1, y1) = clip_to_circle(-(la1 / max_g * radius), lo1 / max_g * radius, radius);
            let alpha = 20 + (i as f32 / n as f32 * 170.0) as u8;
            painter.line_segment(
                [pos2(center.x + x0, center.y + y0), pos2(center.x + x1, center.y + y1)],
                Stroke::new(2.0, Color32::from_rgba_unmultiplied(120, 180, 255, alpha)),
            );
        }
    }

    let (dx, dy) = clip_to_circle(-(lat / max_g * radius), lon / max_g * radius, radius);
    painter.circle_filled(pos2(center.x + dx, center.y + dy), 4.0, Color32::WHITE);
}

fn clip_to_circle(dx: f32, dy: f32, r: f32) -> (f32, f32) {
    let d = (dx * dx + dy * dy).sqrt();
    if d > r {
        let s = r / d;
        (dx * s, dy * s)
    } else {
        (dx, dy)
    }
}

fn draw_shift_bar(
    ui: &mut Ui,
    rect: egui::Rect,
    pkt: &ForzaPacket,
    low_pct: f32,
    high_pct: f32,
    max_rpm: f32,
) {
    let painter = ui.painter();
    let cur = (pkt.current_engine_rpm / max_rpm).clamp(0.0, 1.0);
    let low = (low_pct / 100.0).clamp(0.0, 1.0);
    let high = (high_pct / 100.0).clamp(0.0, 1.0);

    painter.rect_filled(rect, 4.0, crate::theme::TRACK);

    let green_end = low.min(cur);
    if green_end > 0.0 {
        painter.rect_filled(sub_rect(rect, 0.0, green_end), 4.0, Color32::from_rgb(50, 180, 80));
    }
    let yellow_end = high.min(cur);
    if yellow_end > low {
        painter.rect_filled(
            sub_rect(rect, low, yellow_end),
            0.0,
            Color32::from_rgb(220, 180, 40),
        );
    }
    if cur > high {
        painter.rect_filled(sub_rect(rect, high, cur), 0.0, Color32::from_rgb(220, 50, 50));
    }

    // Threshold lines coloured by meaning: warn (yellow) at the low mark,
    // shift (red) at the high mark — matching the zone fills they border.
    let warn_col = Color32::from_rgb(220, 180, 40);
    let shift_col = Color32::from_rgb(220, 50, 50);
    for &(pct, col) in &[(low, warn_col), (high, shift_col)] {
        let x = rect.left() + rect.width() * pct;
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(2.0, col),
        );
    }

    // Centred value: current RPM bright + bold, the " / {max}" portion dimmed.
    // Lay out both parts separately so the combined string stays centred.
    let font = egui::FontId::proportional(13.0);
    let cur_galley = painter.layout_no_wrap(
        format!("{:.0}", pkt.current_engine_rpm),
        font.clone(),
        Color32::WHITE,
    );
    let max_galley = painter.layout_no_wrap(
        format!(" / {:.0}", max_rpm),
        font,
        crate::theme::TEXT_DIM,
    );
    let total_w = cur_galley.size().x + max_galley.size().x;
    let top = rect.center().y - cur_galley.size().y / 2.0;
    let mut x = rect.center().x - total_w / 2.0;
    painter.galley(pos2(x, top), cur_galley.clone(), Color32::WHITE);
    x += cur_galley.size().x;
    painter.galley(pos2(x, top), max_galley, crate::theme::TEXT_DIM);
}

fn sub_rect(r: egui::Rect, start: f32, end: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        pos2(r.left() + r.width() * start, r.top()),
        pos2(r.left() + r.width() * end, r.bottom()),
    )
}

// ── Mini Map ───────────────────────────────────────────────────────

fn show_minimap_widget(ui: &mut Ui, app: &ForzaApp) {
    let rect = ui.available_rect_before_wrap();
    // Clickable in normal mode so co-op players can drop a shared waypoint; in Edit
    // Mode the grid handles drag/resize instead, so only sense clicks when not editing.
    let map_resp = if app.config.dashboard_edit_mode {
        ui.allocate_rect(rect, egui::Sense::hover())
    } else {
        ui.allocate_rect(rect, egui::Sense::click())
    };

    let cx = rect.center().x;
    let cy = rect.center().y;

    let Some(texture) = &app.minimap_texture else {
        let center = rect.center();
        // The last load failed (no FH6 install to read the tiles from): say so instead of
        // spinning. Retried by Reload Map and when the install folder changes (`app.rs`).
        if let Some(err) = &app.minimap_error {
            use crate::minimap::MapLoadError as E;
            let (label, sub) = match err {
                E::NoInstall => (
                    tr("Map needs your Forza Horizon 6 install"),
                    tr("Set it in Setup → Game Install").to_string(),
                ),
                E::NotReadable(_) | E::MissingZip(_) | E::Decode(_) => {
                    (tr("Map could not be loaded"), err.to_string())
                }
            };
            let p = ui.painter_at(rect);
            p.text(
                center + vec2(0.0, -4.0),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(13.0),
                crate::theme::TEXT_DIM,
            );
            p.text(
                center + vec2(0.0, 14.0),
                egui::Align2::CENTER_CENTER,
                sub,
                egui::FontId::proportional(11.0),
                crate::theme::TEXT_FAINT,
            );
            return;
        }
        ui.ctx().request_repaint_after(Duration::from_millis(100));
        // Spinner — identical position to the regular "Loading map…" screen
        ui.put(
            egui::Rect::from_center_size(center + vec2(0.0, -16.0), Vec2::splat(32.0)),
            egui::Spinner::new().size(24.0),
        );
        let p = ui.painter_at(rect);
        let (label, sub) = match &app.minimap_cache_progress {
            Some(in_progress) if !in_progress.is_empty() => {
                let names = in_progress.join(", ");
                (tr("Creating Map Cache"), Some(format!("{}: {}…", tr("Processing"), names)))
            }
            _ => (tr("Loading map…"), None),
        };
        p.text(
            center + vec2(0.0, 12.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(13.0),
            crate::theme::TEXT_DIM,
        );
        if let Some(sub_text) = sub {
            p.text(
                center + vec2(0.0, 28.0),
                egui::Align2::CENTER_CENTER,
                sub_text,
                egui::FontId::proportional(11.0),
                crate::theme::TEXT_FAINT,
            );
        }
        return;
    };

    let cfg = &app.config;
    let cal = crate::minimap::MapCalibration::from_config(cfg);

    let car_x = app.minimap_cached_car_x;
    let car_z = app.minimap_cached_car_z;
    // North-up locks the map (yaw 0); otherwise it's heading-up (rotates with the car). The
    // right-stick look-around sits on top of that base, see `minimap::LookAround`.
    let yaw   = app.minimap_look.view_yaw(app.minimap_base_yaw());

    // Metres visible from widget centre to nearest edge (zoom); rotates world displacement
    // into car-relative screen space (see `minimap::MapView` for the conventions).
    let view = crate::minimap::MapView::new(
        car_x, car_z, yaw, app.minimap_current_zoom, rect.width().min(rect.height()));
    let to_screen = |wx: f32, wz: f32| -> Pos2 {
        let [ox, oy] = view.world_to_offset(wx, wz);
        pos2(cx + ox, cy + oy)
    };

    let orig_size = app.minimap_orig_size;

    let mut mesh = egui::Mesh::with_texture(texture.id());
    mesh.indices = vec![0, 1, 2, 0, 2, 3];

    if cfg.minimap_mirror_edges {
        // Mesh covers the full widget rect; UVs are derived via the inverse world→screen
        // transform and may exceed [0,1] near map edges — MirroredRepeat fills those
        // regions with a reflected copy of the map.
        let half_w = rect.width()  * 0.5;
        let half_h = rect.height() * 0.5;
        for (sx, sy) in [(-half_w, -half_h), (half_w, -half_h), (half_w, half_h), (-half_w, half_h)] {
            let [u, v] = view.uv_at_offset(&cal, orig_size, sx, sy);
            mesh.vertices.push(egui::epaint::Vertex {
                pos:   pos2(cx + sx, cy + sy),
                uv:    pos2(u, v),
                color: Color32::WHITE,
            });
        }
    } else {
        // Mesh covers exactly the map image; UVs are always [0,1] so no mirroring occurs.
        for (wx, wz, [u, v]) in cal.image_corners(orig_size) {
            mesh.vertices.push(egui::epaint::Vertex {
                pos:   to_screen(wx, wz),
                uv:    pos2(u, v),
                color: Color32::WHITE,
            });
        }
    }

    let painter = ui.painter_at(rect);
    painter.add(egui::Shape::Mesh(std::sync::Arc::new(mesh)));

    // Markers (trails, teammates, own arrow, waypoints) come from `hud::map_shared`, the same
    // code the HUD Minimap draws with.
    let cv = crate::hud::map_shared::MapCanvas {
        p: &painter,
        view: &view,
        centre: rect.center(),
        rect,
        s: 1.0,
        a: 1.0,
        pause_glyph: crate::icons::PAUSE,
    };

    // Breadcrumb trails (drawn behind the car arrows). Each player's recent path fades from
    // faint (old) to solid (recent) in their identity colour; the own trail is recorded solo
    // too and is then white, like the own arrow.
    let in_session = app.coop.role() != crate::coop::Role::Off;
    let local_col = if in_session { crate::ui::coop::hue_color(app.config.coop_hue) } else { Color32::WHITE };
    let remotes = app.coop.remote_players();
    if !app.minimap_trails.is_empty() {
        let now = std::time::Instant::now();
        let fade = crate::hud::map_shared::TrailFade::new(cfg.coop_trail_fade_secs, cfg.coop_trail_fade_m);
        if let Some(tr) = app.minimap_trails.get("local") {
            crate::hud::map_shared::draw_trail(&cv, tr, local_col, fade, now);
        }
        for (info, pkt) in &remotes {
            if pkt.is_paused() {
                continue; // paused teammate — don't draw their line
            }
            if let Some(tr) = app.minimap_trails.get(&info.id) {
                crate::hud::map_shared::draw_trail(&cv, tr, crate::ui::coop::hue_color(info.hue), fade, now);
            }
        }
    }

    // Remote co-op players: identity colour + name. Paused players stop broadcasting a valid
    // position; show them at their last-known spot in grey instead of at the world origin.
    let mates: Vec<crate::hud::map_shared::Remote> = remotes
        .iter()
        .filter_map(|(info, pkt)| {
            let paused = pkt.is_paused();
            let (x, z, yaw) = if paused {
                let s = app.coop_last_pos.get(&info.id)?; // never seen at a valid spot — nothing to show
                (s.x, s.z, s.yaw)
            } else {
                (pkt.position_x, pkt.position_z, pkt.yaw)
            };
            Some(crate::hud::map_shared::Remote {
                id: info.id.clone(),
                name: info.name.clone(),
                x,
                z,
                yaw,
                colour: crate::ui::coop::hue_color(info.hue),
                paused,
            })
        })
        .collect();
    crate::hud::map_shared::draw_remotes(&cv, &mates, (car_x, car_z), yaw);

    // Local car indicator: triangle rotated to show heading relative to map orientation.
    // Uses the player's co-op colour (colour only, no name) when in a session, else white.
    crate::hud::map_shared::draw_own_arrow(&cv, view.arrow_angle(app.minimap_cached_raw_yaw), local_col);

    // ── Co-op shared waypoint ──────────────────────────────────────
    // Left-click drops/moves a waypoint everyone in the session sees; right-click clears.
    if !app.config.dashboard_edit_mode && app.coop.role() != crate::coop::Role::Off {
        if map_resp.clicked() {
            if let Some(m) = map_resp.interact_pointer_pos() {
                let [wx, wz] = view.offset_to_world(m.x - cx, m.y - cy);
                app.coop.set_waypoint(Some((wx, wz)), app.config.coop_hue);
            }
        }
        if map_resp.secondary_clicked() {
            app.coop.set_waypoint(None, 0.0);
        }
    }
    let time = ui.input(|i| i.time) as f32;
    for (_pid, wx, wz, hue) in app.coop.waypoints() {
        crate::hud::map_shared::draw_waypoint(&cv, (wx, wz), crate::ui::coop::hue_color(hue), (car_x, car_z), time);
    }

    // North compass: shared with the HUD Minimap (`hud::minimap::draw_compass`), scaled
    // with the widget (HUD design size = 1.0) and clamped so it stays proportionate.
    if cfg.minimap_show_compass {
        let s = (rect.width().min(rect.height()) / 200.0).clamp(0.8, 1.6);
        let xf = crate::hud::prims::Xf { o: rect.min, s, a: 1.0 };
        crate::hud::minimap::draw_compass(&painter, &xf, view.north_dir());
    }

    // On-map co-op player list. Fixed-width, space-padded columns so the panel
    // never reflows (which would flicker). Front marker is a dot, or the ⏸ glyph
    // (in the player's colour) when paused.
    if cfg.coop_map_playerlist && app.coop.role() != crate::coop::Role::Off {
        let unit = if cfg.use_mph { "mph" } else { "km/h" };
        // (hue colour, paused, row text, class, PI). The class column is drawn as a
        // label image (assets/labels) after the text, so it's excluded from the text.
        let mut rows: Vec<(Color32, bool, String, i32, i32)> = Vec::new();
        let mut push_row = |hue: f32, name: &str, speed_ms: f32, gear: u8, class: i32, pi: i32, dist: f32, is_self: bool, paused: bool| {
            // Name: 12 cells, left-aligned, ellipsised if longer.
            let mut s = if name.chars().count() > 12 {
                name.chars().take(11).collect::<String>() + "…"
            } else {
                format!("{name:<12}")
            };
            if cfg.coop_list_distance {
                let d = if is_self {
                    String::new()
                } else if dist >= 1000.0 {
                    format!("{:.1}km", dist / 1000.0)
                } else {
                    format!("{dist:.0}m")
                };
                s += &format!(" {d:>6}"); // reserves up to "99.9km"
            }
            if cfg.coop_list_speed {
                let disp = if cfg.use_mph { speed_ms * 2.236_94 } else { speed_ms * 3.6 };
                s += &format!(" {disp:>3.0}{unit}");
            }
            if cfg.coop_list_gear {
                let g = match gear {
                    0 => "R".to_string(),
                    11 => "N".to_string(),
                    g => g.to_string(),
                };
                s += &format!(" G{g:<2}"); // "G10" / "G9 " / "GN " / "GR "
            }
            rows.push((crate::ui::coop::hue_color(hue), paused, s, class, pi));
        };
        if let Some(p) = &app.telemetry.latest {
            // Our own class/PI come from the cache so a local pause doesn't blank them.
            push_row(cfg.coop_hue, &cfg.coop_name, p.speed, p.gear, app.cached_car_class, app.cached_car_pi, 0.0, true, p.is_paused());
        }
        for (info, pkt) in app.coop.remote_players() {
            let paused = pkt.is_paused();
            let last = app.coop_last_pos.get(&info.id);
            let (px, pz) = if paused {
                last.map(|s| (s.x, s.z))
                    .unwrap_or((pkt.position_x, pkt.position_z))
            } else {
                (pkt.position_x, pkt.position_z)
            };
            let dist = ((px - car_x).powi(2) + (pz - car_z).powi(2)).sqrt();
            // PI 0 = empty (paused game transmits zeros) — fall back to the last
            // real class/PI we saw from this player.
            let (cl, pi) = if pkt.car_performance_index == 0 {
                last.map(|s| (s.car_class, s.pi))
                    .unwrap_or((pkt.car_class, pkt.car_performance_index))
            } else {
                (pkt.car_class, pkt.car_performance_index)
            };
            push_row(info.hue, &info.name, pkt.speed, pkt.gear, cl, pi, dist, false, paused);
        }
        if !rows.is_empty() {
            let font = egui::FontId::monospace(11.0);
            let (icon_x, text_x, row_h, pad) = (9.0_f32, 19.0_f32, 17.0_f32, 5.0_f32);
            // Class label sized to the row with headroom; native art is 111×40.
            let native = app.labels.class_size(0, 1.0);
            let class_scale = (row_h - 2.0) / native.y;
            let class_gap = 6.0;
            let class_w = if cfg.coop_list_class { native.x * class_scale + class_gap } else { 0.0 };
            let galleys: Vec<(Color32, bool, std::sync::Arc<egui::Galley>, i32, i32)> = rows
                .iter()
                .map(|(c, paused, s, cl, pi)| (*c, *paused, painter.layout_no_wrap(s.clone(), font.clone(), Color32::WHITE), *cl, *pi))
                .collect();
            let text_w = galleys.iter().map(|(_, _, g, _, _)| g.size().x).fold(0.0, f32::max);
            let w = text_x + text_w + class_w + pad;
            let h = pad * 2.0 + row_h * galleys.len() as f32;
            let origin = rect.right_top() + vec2(-w - 6.0, 6.0);
            let panel = egui::Rect::from_min_size(origin, vec2(w, h));
            painter.rect_filled(panel, 4.0, Color32::from_black_alpha(160));
            for (i, (c, paused, g, cl, pi)) in galleys.into_iter().enumerate() {
                let cy = panel.top() + pad + row_h * i as f32 + row_h * 0.5;
                let icon_pos = pos2(panel.left() + icon_x, cy);
                if paused {
                    painter.text(icon_pos, egui::Align2::CENTER_CENTER, crate::icons::PAUSE,
                        egui::FontId::monospace(10.0), c);
                } else {
                    painter.circle_filled(icon_pos, 4.0, c);
                }
                painter.galley(pos2(panel.left() + text_x, cy - g.size().y * 0.5), g, Color32::WHITE);
                if cfg.coop_list_class {
                    let cx0 = panel.left() + text_x + text_w + class_gap;
                    let lbl = app.labels.class_size(cl, class_scale);
                    app.labels.paint_class(&painter, cl, pi, pos2(cx0, cy - lbl.y * 0.5), class_scale);
                }
            }
        }
    }
}

// ── Graph modules (Power Graph + Boost Graph) ─────────────────────
//
// Both graph modules run through the same helpers below, so they share one look and one
// set of options: the blue section title (or, in Compact, a small title painted over the
// plot), the 8px axis-label padding, zero RPM margin, the grid toggle, Compact's peak guide
// lines + labels, and the RPM axis extent. The options are the dashboard mini-settings'
// "Power Graph" page (`power_graph_compact`, `power_graph_show_grid`) plus the shared
// `power_curve_*` capture options.
// Why one shared set: the user asked for the Boost Graph to look and behave like the Power
// Graph "with the same settings" — two modules side by side reading different compact/grid
// flags would drift apart again.

/// Series colours — data colours, matching the Power Curve tab.
const GRAPH_POWER: Color32 = Color32::from_rgb(80, 160, 240);
const GRAPH_TORQUE: Color32 = Color32::from_rgb(240, 140, 40);
const GRAPH_BOOST: Color32 = Color32::from_rgb(180, 80, 220);

/// PSI → bar.
const PSI_TO_BAR: f64 = 0.0689476;

/// Smallest module cell that still draws a graph with axes, title row and legend. Below
/// it the graph switches to the Compact look on its own (title over the plot, no axes,
/// peak lines + labels), so a small cell gets a readable plot instead of tick labels
/// squeezing the plot area to nothing.
const GRAPH_AXES_MIN: Vec2 = vec2(200.0, 120.0);

/// Title row (normal mode only) and the rect the plot gets, plus whether the graph draws
/// compact — the Compact option, or forced by a cell smaller than [`GRAPH_AXES_MIN`].
/// Non-compact: the rotated y-axis label overhangs the plot's left edge (egui_plot draws
/// it at rect.left() - gap), so the plot gets 8px left/right padding or the module cell
/// clips the label. Compact has no axis labels, so it uses the full width — its title is
/// painted over the plot afterwards by [`paint_compact_graph_title`], costing no vertical
/// space.
fn graph_module_rect(ui: &mut Ui, app: &ForzaApp, title: &str) -> (egui::Rect, bool) {
    let cell = ui.available_rect_before_wrap();
    let compact = app.config.power_graph_compact
        || cell.width() < GRAPH_AXES_MIN.x
        || cell.height() < GRAPH_AXES_MIN.y;
    if !compact && !app.config.hide_widget_titles {
        ui.add(egui::Label::new(crate::theme::section_label(title)).truncate());
        ui.add_space(4.0);
    }
    let mut plot_rect = ui.available_rect_before_wrap();
    if !compact {
        plot_rect.min.x += 8.0;
        plot_rect.max.x -= 8.0;
    }
    (plot_rect, compact)
}

/// The plot base both graph modules share: 0 RPM flush on the left edge, a static
/// (non-interactive) view, the grid toggle, axes hidden in Compact, and the right edge
/// 1000 RPM past the highest recorded point (the full rev range before any data).
fn graph_plot(app: &ForzaApp, id: &str, data_max_rpm: f64, compact: bool) -> Plot<'static> {
    let g = app.config.power_graph_show_grid;
    let mut plot = Plot::new(id)
        // egui_plot's default min size is 64×64, which overflowed small cells; the plot
        // takes exactly the rect it is given.
        .min_size(vec2(1.0, 1.0))
        // Zero x-margin so 0 RPM sits exactly on the left edge (no auto-padding).
        .set_margin_fraction(egui::vec2(0.0, 0.05))
        .include_x(0.0)
        .include_y(0.0)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        // Grid lines toggle applies regardless of compact/normal mode.
        .show_grid([g, g]);
    if compact {
        // No legend or axis ticks/labels — but keep the grid lines for reference.
        plot = plot.show_axes([false, false]);
    } else {
        plot = plot.x_axis_label(tr("RPM"));
    }
    let engine_max_rpm = if app.cached_engine_max_rpm > 0.0 { app.cached_engine_max_rpm } else { 8000.0 };
    if data_max_rpm > 0.0 {
        plot.include_x(data_max_rpm + 1000.0)
    } else {
        plot.include_x(engine_max_rpm)
    }
}

/// Peak point (max y and its x) of a series.
fn series_peak(s: &[[f64; 2]]) -> Option<[f64; 2]> {
    s.iter().copied().reduce(|a, b| if b[1] > a[1] { b } else { a })
}

/// Compact mode's peak value labels, in screen space: to the RIGHT of each peak guide line,
/// near the bottom, with the same gap to the line as to the bottom, coloured like the line,
/// and nudged up so overlapping labels don't collide.
fn paint_peak_labels(ui: &Ui, transform: &egui_plot::PlotTransform, mut labels: Vec<(f64, Color32, String)>) {
    let frame = *transform.frame();
    let gap = 3.0_f32;
    let font = egui::FontId::proportional(10.0);
    let painter = ui.painter().with_clip_rect(frame);
    labels.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut placed: Vec<egui::Rect> = Vec::new();
    for (rpm, color, text) in labels {
        let line_x = transform.position_from_point_x(rpm);
        let galley = painter.layout_no_wrap(text, font.clone(), color);
        let sz = galley.size();
        // Right of the line by `gap`; flip to the left if it would overflow the edge.
        let mut x = line_x + gap;
        if x + sz.x > frame.right() { x = line_x - gap - sz.x; }
        let mut y = frame.bottom() - gap - sz.y;
        let mut rect = egui::Rect::from_min_size(pos2(x, y), sz);
        while placed.iter().any(|r| r.intersects(rect.expand(1.0))) {
            y -= sz.y + 2.0;
            rect = egui::Rect::from_min_size(pos2(x, y), sz);
        }
        placed.push(rect);
        painter.galley(pos2(x, y), galley, color);
    }
}

/// Compact mode's title, painted over the plot's top-left corner (no vertical space cost).
fn paint_compact_graph_title(ui: &Ui, app: &ForzaApp, plot_rect: egui::Rect, title: &str, compact: bool) {
    if !compact || app.config.hide_widget_titles {
        return;
    }
    let pos = plot_rect.min + vec2(4.0, 2.0);
    // Shrink to the plot's width (minus the 4px inset each side), never grow past 12.
    let text = title.to_uppercase();
    let w12 = text_w(ui.painter(), &text, &egui::FontId::proportional(12.0));
    let size = 12.0 * fit_scale(vec2(w12, 1.0), vec2(plot_rect.width() - 8.0, 1.0));
    if size < MIN_PAINT_FONT {
        return;
    }
    let galley = ui.painter().layout_no_wrap(text, egui::FontId::proportional(size), crate::theme::ACCENT);
    if app.config.power_graph_show_grid {
        // Gridlines run under the title — back it with a small filled box so it
        // stays readable. Only drawn when the grid is on.
        let bg_rect = egui::Rect::from_min_size(pos, galley.size()).expand(3.0);
        ui.painter().rect_filled(bg_rect, 3.0, egui::Color32::from_black_alpha(160));
    }
    ui.painter().galley(pos, galley, crate::theme::ACCENT);
}

/// The boost series a dashboard graph plots (live capture, falling back to the saved
/// reference), and whether forced-induction detection lets it be shown. Visibility is judged
/// on the series actually plotted — a saved turbo reference must not make an NA car's live
/// (vacuum) series show up. See `power_capture::boost_visible`.
fn dashboard_boost_series(app: &ForzaApp) -> (&[[f64; 2]], bool) {
    let series: &[[f64; 2]] = if !app.power_capture.boost_series.is_empty() {
        &app.power_capture.boost_series
    } else if let Some(curve) = app.saved_power_curve.as_ref() {
        &curve.boost_series
    } else {
        &[]
    };
    let visible = crate::listeners::power_capture::boost_visible(
        app.config.power_curve_forced_induction,
        app.config.power_curve_save_fi_state,
        app.power_capture.fi_detected(),
        &[series],
    );
    (series, visible)
}

fn show_power_graph_widget(ui: &mut Ui, app: &ForzaApp) {
    let title = tr("Power Graph");
    let (plot_rect, compact) = graph_module_rect(ui, app, title);

    // Live capture, falling back to the saved reference curve (same data as the
    // Power Curve tab).
    let has_live_curve = !app.power_capture.power_series.is_empty();
    let (power_series, torque_series) = if has_live_curve {
        (
            app.power_capture.power_series.clone(),
            app.power_capture.torque_series.clone(),
        )
    } else if let Some(curve) = app.saved_power_curve.as_ref() {
        (curve.power_series.clone(), curve.torque_series.clone())
    } else {
        (Vec::new(), Vec::new())
    };

    // Optional boost line from the same series the Boost Graph uses — only when
    // "Show Boost" is on AND forced-induction detection allows it (an NA car's vacuum
    // readings used to draw a flat line at 0 plus a boost axis).
    let use_bar = app.config.use_bar;
    let (raw_boost, boost_ok) = dashboard_boost_series(app);
    let boost_series: Vec<[f64; 2]> = if app.config.power_graph_show_boost && boost_ok {
        raw_boost
            .iter()
            .map(|&[rpm, psi]| {
                let val = if use_bar { psi * PSI_TO_BAR } else { psi };
                [rpm, val.max(0.0)]
            })
            .collect()
    } else {
        Vec::new()
    };

    // Boost values (bar/PSI) are tiny next to PS/Nm, so scale them into the
    // shared plot space and expose the real values on a dedicated right axis.
    let y_top = {
        let m = power_series
            .iter()
            .chain(torque_series.iter())
            .map(|&[_, v]| v)
            .fold(0.0_f64, f64::max);
        if m > 0.0 { m } else { 100.0 }
    };
    let boost_top = if boost_series.is_empty() {
        if use_bar { 1.0 } else { 15.0 }
    } else {
        // Same headroom style as the Boost Graph module.
        let max_boost = boost_series.iter().map(|&[_, v]| v).fold(0.0_f64, f64::max);
        let min_headroom = if use_bar { 0.25 } else { 3.0 };
        max_boost + (max_boost.abs() * 0.15).max(min_headroom)
    };
    let boost_scale = y_top / boost_top;

    // Highest recorded RPM across the plotted series — the axis runs 0..this+1000.
    let data_max_rpm = power_series
        .iter()
        .chain(torque_series.iter())
        .chain(boost_series.iter())
        .map(|&[rpm, _]| rpm)
        .fold(0.0_f64, f64::max);

    // Peak points for compact-mode inline annotations. Compute before the series are
    // moved into the plot below.
    let peak_power = series_peak(&power_series);
    let peak_torque = series_peak(&torque_series);
    let peak_boost = series_peak(&boost_series);

    let mut plot_ui = ui.new_child(egui::UiBuilder::new().max_rect(plot_rect).layout(*ui.layout()));
    let mut plot = graph_plot(app, "dash_power_graph", data_max_rpm, compact);
    if !compact {
        // The default left axis, plus a dedicated right-side scale for the boost
        // line (tick marks converted back to bar/PSI via the scale factor).
        let mut y_axes = vec![AxisHints::new_y().label("PS / Nm")];
        if !boost_series.is_empty() {
            let boost_label = if use_bar { tr("Boost (bar)") } else { tr("Boost (PSI)") };
            y_axes.push(
                AxisHints::new_y()
                    .label(boost_label)
                    .placement(HPlacement::Right)
                    .formatter(move |mark, _| format!("{:.1}", mark.value / boost_scale)),
            );
        }
        plot = plot
            .legend(Legend::default().position(egui_plot::Corner::RightBottom).follow_insertion_order(true))
            .custom_y_axes(y_axes);
    }
    if power_series.is_empty() {
        // No captured data yet — keep the empty plot's y-axis at a sensible extent.
        plot = plot.include_y(100.0);
    }
    let resp = plot.show(&mut plot_ui, |plot_ui| {
        if !power_series.is_empty() {
            plot_ui.line(
                Line::new(tr("Power (PS)"), PlotPoints::new(power_series))
                    .color(GRAPH_POWER)
                    .width(2.5),
            );
            plot_ui.line(
                Line::new(tr("Torque (Nm)"), PlotPoints::new(torque_series))
                    .color(GRAPH_TORQUE)
                    .width(2.5),
            );
        }
        if !boost_series.is_empty() {
            let boost_label = if use_bar { tr("Boost (bar)") } else { tr("Boost (PSI)") };
            let scaled: Vec<[f64; 2]> = boost_series
                .iter()
                .map(|&[rpm, v]| [rpm, v * boost_scale])
                .collect();
            plot_ui.line(
                Line::new(boost_label, PlotPoints::new(scaled))
                    .color(GRAPH_BOOST)
                    .width(2.0),
            );
        }
        if compact {
            // A thin vertical guide line at each series' peak RPM; the value labels are
            // drawn afterwards in screen space (paint_peak_labels).
            for (peak, color) in [(peak_power, GRAPH_POWER), (peak_torque, GRAPH_TORQUE), (peak_boost, GRAPH_BOOST)] {
                if let Some([rpm, _]) = peak {
                    plot_ui.vline(egui_plot::VLine::new("", rpm).color(color).width(1.0));
                }
            }
        }
    });

    if compact {
        let mut labels: Vec<(f64, Color32, String)> = Vec::new();
        if let Some([rpm, v]) = peak_power  { labels.push((rpm, GRAPH_POWER, format!("{:.0} PS", v))); }
        if let Some([rpm, v]) = peak_torque { labels.push((rpm, GRAPH_TORQUE, format!("{:.0} Nm", v))); }
        if let Some([rpm, v]) = peak_boost  { labels.push((rpm, GRAPH_BOOST, format!("{:.2}", v))); }
        paint_peak_labels(&plot_ui, &resp.transform, labels);
    }
    paint_compact_graph_title(ui, app, plot_rect, title, compact);
}

fn show_boost_graph_widget(ui: &mut Ui, app: &ForzaApp) {
    let title = tr("Boost Graph");
    let (plot_rect, compact) = graph_module_rect(ui, app, title);

    // Forced-induction detection controls only whether bars are plotted; the plot itself
    // (axes/grid) always renders, with a dim note in place of the bars.
    let (boost_series, boost_ok) = dashboard_boost_series(app);
    let plot_bars = boost_ok && !boost_series.is_empty();

    let use_bar = app.config.use_bar;
    let step = app.config.power_curve_step as f64;
    let values: Vec<[f64; 2]> = if plot_bars {
        boost_series
            .iter()
            .map(|&[rpm, psi]| [rpm, if use_bar { psi * PSI_TO_BAR } else { psi }])
            .collect()
    } else {
        Vec::new()
    };
    let max_boost = values.iter().map(|&[_, v]| v).fold(0.0_f64, f64::max);
    let min_headroom = if use_bar { 0.25 } else { 3.0 };
    let boost_top = max_boost + (max_boost.abs() * 0.15).max(min_headroom);
    let peak_boost = series_peak(&values);
    // Highest recorded RPM (only when bars are actually shown).
    let data_max_rpm = values.iter().map(|&[rpm, _]| rpm).fold(0.0_f64, f64::max);

    let bars: Vec<Bar> = values
        .iter()
        .map(|&[rpm, v]| Bar::new(rpm, v).fill(GRAPH_BOOST).width(step * 0.8))
        .collect();

    let boost_label = if use_bar { tr("Boost (bar)") } else { tr("Boost (PSI)") };
    let mut plot_ui = ui.new_child(egui::UiBuilder::new().max_rect(plot_rect).layout(*ui.layout()));
    let mut plot = graph_plot(app, "dash_boost_graph", data_max_rpm, compact).include_y(boost_top);
    if !compact {
        plot = plot.custom_y_axes(vec![AxisHints::new_y().label(boost_label)]);
    }
    let resp = plot.show(&mut plot_ui, |plot_ui| {
        if !bars.is_empty() {
            plot_ui.bar_chart(BarChart::new(boost_label, bars));
        }
        if compact {
            if let Some([rpm, _]) = peak_boost {
                plot_ui.vline(egui_plot::VLine::new("", rpm).color(GRAPH_BOOST).width(1.0));
            }
        }
    });

    if compact {
        if let Some([rpm, v]) = peak_boost {
            paint_peak_labels(&plot_ui, &resp.transform, vec![(rpm, GRAPH_BOOST, format!("{:.2}", v))]);
        }
    }
    if !plot_bars {
        // Naturally aspirated (or nothing captured yet) — say so instead of an empty chart.
        // Fitted to the plot frame like every module text (no fixed 12 pt overflow).
        let frame = *resp.transform.frame();
        let painter = plot_ui.painter().with_clip_rect(frame);
        let note = tr("No boost detected");
        let nsz = painter.layout_no_wrap(note.to_owned(), egui::FontId::proportional(12.0), Color32::WHITE).size();
        let size = 12.0 * fit_scale(nsz, frame.shrink(4.0).size());
        if size >= MIN_PAINT_FONT {
            painter.text(frame.center(), egui::Align2::CENTER_CENTER, note,
                egui::FontId::proportional(size), crate::theme::TEXT_DIM);
        }
    }
    paint_compact_graph_title(ui, app, plot_rect, title, compact);
}

// ── Helpers ────────────────────────────────────────────────────────

/// Base font sizes for a sprint row, multiplied by the caller's `scale` so all
/// rows shrink/grow together to fit the widget cell (see `show_race_block`).
const SPRINT_LABEL_SIZE: f32 = 14.0;
const SPRINT_MAIN_SIZE: f32 = 14.0;
const SPRINT_SECONDARY_SIZE: f32 = 12.0;
/// Edge margin around the sprint content (matches the input bars' side inset).
const SPRINT_EDGE: f32 = 3.0;

fn sprint_row(
    ui: &mut Ui,
    label: &str,
    segment: Option<f32>,
    cumulative: Option<f32>,
    stype: &SprintType,
    show_other: bool,
    scale: f32,
) {
    ui.horizontal(|ui| {
        ui.add_space(SPRINT_EDGE); // left margin
        ui.label(RichText::new(format!("{label:12}")).size(SPRINT_LABEL_SIZE * scale).strong());

        let (main, secondary) = match stype {
            SprintType::Incremental => (segment, cumulative),
            SprintType::Absolute    => (cumulative, segment),
        };

        match main {
            Some(t) => {
                ui.label(
                    RichText::new(format!("{t:.3}s"))
                        .size(SPRINT_MAIN_SIZE * scale)
                        .color(Color32::from_rgb(60, 210, 100)),
                );
                if show_other {
                    if let Some(s) = secondary {
                        ui.label(
                            RichText::new(format!("({s:.3}s)"))
                                .size(SPRINT_SECONDARY_SIZE * scale)
                                .color(crate::theme::TEXT_DIM),
                        );
                    }
                }
            }
            None => {
                ui.label(RichText::new("--").size(SPRINT_MAIN_SIZE * scale).color(crate::theme::TEXT_DIM));
            }
        }
    });
}

fn cumulative_time(splits: &[Option<f32>]) -> Option<f32> {
    splits
        .iter()
        .copied()
        .collect::<Option<Vec<_>>>()
        .map(|v| v.iter().sum())
}

fn input_bar(ui: &mut Ui, label: &str, val: u8, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(format!("{label:11}"));
        let pct_w = 46.0_f32;
        let bar_w = (ui.available_width() - pct_w).max(40.0);
        ui.add(
            egui::ProgressBar::new(val as f32 / 255.0)
                .fill(color)
                .desired_width(bar_w),
        );
        ui.label(format!("{:.0}%", val as f32 / 255.0 * 100.0));
    });
}

/// Alternative input style: a full-width bar with the label drawn at the left
/// and the value at the right, both inside the bar. Text is white for now.
fn input_bar_full(ui: &mut Ui, label: &str, val: u8, color: Color32) {
    let h = 18.0_f32;
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), egui::Sense::hover());
    let rect = rect.shrink2(vec2(3.0, 0.0)); // match the steering bar's side inset
    let painter = ui.painter();

    let round = h / 2.0; // fully rounded (pill) ends, like the old progress bar
    painter.rect_filled(rect, round, crate::theme::TRACK);
    let frac = (val as f32 / 255.0).clamp(0.0, 1.0);
    if frac > 0.0 {
        let fill = Rect::from_min_max(rect.min, pos2(rect.left() + frac * rect.width(), rect.bottom()));
        painter.rect_filled(fill, round, color);
    }

    let font = egui::FontId::proportional(12.0);
    painter.text(pos2(rect.left() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER, label, font.clone(), Color32::WHITE);
    painter.text(pos2(rect.right() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER, format!("{:.0}%", frac * 100.0), font, Color32::WHITE);
}

fn temp_color(val: f32, is_f: bool) -> Color32 {
    let (cold, warm, hot) = if is_f {
        (140.0, 200.0, 250.0)
    } else {
        (60.0, 93.0, 121.0)
    };
    if val < cold {
        Color32::from_rgb(100, 140, 220)
    } else if val < warm {
        Color32::from_rgb(60, 200, 90)
    } else if val < hot {
        Color32::from_rgb(230, 160, 40)
    } else {
        Color32::from_rgb(220, 60, 60)
    }
}

fn fmt_lap(secs: f32) -> String {
    if secs <= 0.0 {
        return "--:--.---".to_string();
    }
    let m = (secs / 60.0) as u32;
    let s = secs % 60.0;
    format!("{m}:{s:06.3}")
}

/// Fit-to-pane checks for the migrated modules (Session Stats, Position, Co-Op, Boost):
/// render each body into panes from tiny to large on a real egui context with the app's
/// fonts and theme, and assert every painted shape stays inside its pane.
#[cfg(test)]
mod fit_tests {
    use super::*;

    const SIZES: [(f32, f32); 8] = [
        (24.0, 14.0), (60.0, 30.0), (60.0, 300.0), (400.0, 36.0),
        (140.0, 90.0), (200.0, 150.0), (420.0, 200.0), (900.0, 600.0),
    ];

    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "geist_mono".to_owned(),
            egui::FontData::from_static(include_bytes!("../../assets/fonts/GeistMono-Regular.ttf")).into(),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().insert(0, "geist_mono".to_owned());
        }
        ctx.set_fonts(fonts);
        crate::theme::apply(&ctx);
        ctx
    }

    /// Bounding rects of everything `paint` draws into `pane` (panel background excluded),
    /// and the largest font size used.
    fn render(pane: Rect, paint: impl Fn(&egui::Painter, Rect)) -> (Vec<Rect>, f32) {
        let ctx = ctx();
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(2000.0, 2000.0))),
            ..Default::default()
        };
        let mut out = None;
        for _ in 0..2 {
            // Second pass: fonts are installed at the start of the first.
            out = Some(ctx.run(input(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let p = ui.painter().clone();
                    paint(&p, pane);
                });
            }));
        }
        let mut rects = Vec::new();
        let mut max_font = 0.0_f32;
        for cs in out.unwrap().shapes {
            let r = cs.shape.visual_bounding_rect();
            if !r.is_positive() || (r.width() >= 1000.0 && r.height() >= 1000.0) {
                continue; // panel background
            }
            if let egui::Shape::Text(t) = &cs.shape {
                for row in &t.galley.rows {
                    for g in &row.glyphs {
                        max_font = max_font.max(g.font_height);
                    }
                }
            }
            rects.push(r);
        }
        (rects, max_font)
    }

    fn check(name: &str, paint: impl Fn(&egui::Painter, Rect)) {
        for (w, h) in SIZES {
            let pane = Rect::from_min_size(pos2(100.0, 100.0), vec2(w, h));
            let (rects, font) = render(pane, &paint);
            for r in &rects {
                // Glyph meshes carry ~1px of antialias padding past the galley's logical
                // rect; that stays inside the cell thanks to PANE_EDGE.
                assert!(pane.expand(PANE_EDGE * 0.5).contains_rect(*r),
                    "{name} {w}×{h} (font {font}): shape {r:?} overflows pane {pane:?}");
            }
            if w >= 140.0 && h >= 90.0 {
                assert!(!rects.is_empty(), "{name} {w}×{h}: drew nothing");
            }
        }
    }

    fn stats() -> Vec<(&'static str, String)> {
        vec![
            ("Top Speed", "312 km/h".into()), ("Peak Power", "1203 PS".into()),
            ("Peak Torque", "1408 Nm".into()), ("Peak Boost", "24.37 PSI".into()),
            ("Peak Lat G", "1.84 g".into()), ("Peak Long G", "1.21 g".into()),
            ("Max RPM", "8450".into()),
        ]
    }

    #[test]
    fn session_stats_fit() {
        check("stats", |p, r| paint_session_stats(p, r, &stats(), 13.0));
    }

    #[test]
    fn session_stats_full_size_in_a_large_pane() {
        let (_, f) = render(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0)),
            |p, r| paint_session_stats(p, r, &stats(), 13.0));
        assert!(f >= 12.0, "large pane should reach the base size, got {f}");
    }

    #[test]
    fn position_fits() {
        check("position", |p, r| paint_position(p, r, [-12345.67, 210.5, 9876.54], [3.0, -0.2, 0.1], 13.0));
    }

    fn coop_rows() -> Vec<CoopRow> {
        (0..4).map(|i| CoopRow {
            name: format!("Player with a long name {i}"),
            hue: i as f32 * 0.25,
            speed_ms: 80.0 - i as f32 * 10.0,
            gear: i as u8 + 3,
            is_self: i == 0,
            dist_m: 1234.0 * i as f32,
        }).collect()
    }

    #[test]
    fn coop_fits() {
        check("coop", |p, r| paint_coop_rows(p, r, &coop_rows(), false, 12.0));
    }

    fn gauge() -> BoostGauge {
        BoostGauge { cur: 18.25, peak: 22.5, scale: 30.0, unit: "PSI", color: Color32::from_rgb(200, 120, 70) }
    }

    #[test]
    fn boost_gauge_fits_in_every_style() {
        check("boost in-bar", |p, r| paint_boost_in_bar(p, r, &gauge()));
        check("boost vertical", |p, r| paint_boost_vertical(p, r, &gauge()));
        check("boost horizontal", |p, r| paint_boost_horizontal(p, r, &gauge()));
    }

    /// Software-rasterise one module tile (cell outline, title, body) to a PNG under
    /// `target/dashboard_png/` for eyeballing — CPU triangle fill of egui's tessellated
    /// meshes, sampling the font atlas. Run:
    /// `cargo test dashboard_module_pngs -- --ignored`
    fn tile_png(name: &str, cell: Vec2, title: &str, paint: &dyn Fn(&egui::Painter, Rect)) {
        let margin = 8.0;
        let screen = vec2(cell.x + 2.0 * margin, cell.y + 2.0 * margin);
        let ctx = ctx();
        let mut textures: std::collections::HashMap<egui::TextureId, egui::ColorImage> = Default::default();
        let mut prims = Vec::new();
        for _ in 0..2 {
            let out = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                        let p = ui.painter().clone();
                        let cell_r = Rect::from_min_size(pos2(margin, margin), cell);
                        p.rect_stroke(cell_r, 2.0, Stroke::new(1.5, crate::theme::BORDER), egui::StrokeKind::Middle);
                        let content = cell_r.shrink(2.0);
                        let p = p.with_clip_rect(content);
                        // Title as widget_title draws it (section_label, truncated).
                        let tg = p.layout_no_wrap(title.to_uppercase(), egui::FontId::proportional(12.0), crate::theme::ACCENT);
                        let th = tg.size().y + 3.0;
                        p.galley(content.min, tg, crate::theme::ACCENT);
                        let body = Rect::from_min_max(pos2(content.left(), content.top() + th), content.max);
                        if body.height() > 2.0 * PANE_EDGE {
                            paint(&p, body.shrink(PANE_EDGE));
                        }
                    });
                },
            );
            for (id, delta) in out.textures_delta.set {
                let egui::ImageData::Color(img) = delta.image;
                match delta.pos {
                    None => { textures.insert(id, (*img).clone()); }
                    Some([x0, y0]) => {
                        if let Some(t) = textures.get_mut(&id) {
                            for y in 0..img.size[1] {
                                for x in 0..img.size[0] {
                                    t.pixels[(y0 + y) * t.size[0] + x0 + x] = img.pixels[y * img.size[0] + x];
                                }
                            }
                        }
                    }
                }
            }
            prims = ctx.tessellate(out.shapes, 1.0);
        }

        let (w, h) = (screen.x as usize, screen.y as usize);
        let bg = crate::theme::PANEL;
        let mut buf: Vec<[f32; 4]> = vec![
            [bg.r() as f32 / 255.0, bg.g() as f32 / 255.0, bg.b() as f32 / 255.0, 1.0]; w * h
        ];
        for cp in prims {
            let egui::epaint::Primitive::Mesh(mesh) = cp.primitive else { continue };
            let Some(tex) = textures.get(&mesh.texture_id) else { continue };
            let clip = cp.clip_rect;
            let sample = |u: f32, v: f32| {
                let x = ((u * tex.size[0] as f32) as usize).min(tex.size[0] - 1);
                let y = ((v * tex.size[1] as f32) as usize).min(tex.size[1] - 1);
                tex.pixels[y * tex.size[0] + x]
            };
            for tri in mesh.indices.chunks_exact(3) {
                let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| mesh.vertices[i as usize]);
                let area = (b.pos - a.pos).x * (c.pos - a.pos).y - (b.pos - a.pos).y * (c.pos - a.pos).x;
                if area.abs() < 1e-6 { continue; }
                let minx = a.pos.x.min(b.pos.x).min(c.pos.x).max(clip.left()).max(0.0).floor() as usize;
                let maxx = a.pos.x.max(b.pos.x).max(c.pos.x).min(clip.right()).min(w as f32 - 1.0).ceil() as usize;
                let miny = a.pos.y.min(b.pos.y).min(c.pos.y).max(clip.top()).max(0.0).floor() as usize;
                let maxy = a.pos.y.max(b.pos.y).max(c.pos.y).min(clip.bottom()).min(h as f32 - 1.0).ceil() as usize;
                for py in miny..=maxy.min(h - 1) {
                    for px in minx..=maxx.min(w - 1) {
                        let p = pos2(px as f32 + 0.5, py as f32 + 0.5);
                        if !clip.contains(p) { continue; }
                        let w0 = ((b.pos - p).x * (c.pos - p).y - (b.pos - p).y * (c.pos - p).x) / area;
                        let w1 = ((c.pos - p).x * (a.pos - p).y - (c.pos - p).y * (a.pos - p).x) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 { continue; }
                        let u = a.uv.x * w0 + b.uv.x * w1 + c.uv.x * w2;
                        let v = a.uv.y * w0 + b.uv.y * w1 + c.uv.y * w2;
                        let t = sample(u, v);
                        let col = [0, 1, 2, 3].map(|i| {
                            let vc = [a.color, b.color, c.color].map(|cc| cc.to_array()[i] as f32 / 255.0);
                            (vc[0] * w0 + vc[1] * w1 + vc[2] * w2) * t.to_array()[i] as f32 / 255.0
                        });
                        let d = &mut buf[py * w + px];
                        for i in 0..4 {
                            d[i] = col[i] + d[i] * (1.0 - col[3]);
                        }
                    }
                }
            }
        }
        let raw: Vec<u8> = buf.iter()
            .flat_map(|c| [c[0], c[1], c[2], 1.0].map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8))
            .collect();
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("dashboard_png");
        std::fs::create_dir_all(&dir).unwrap();
        image::save_buffer(dir.join(format!("{name}.png")), &raw, w as u32, h as u32, image::ExtendedColorType::Rgba8)
            .unwrap();
    }

    #[test]
    #[ignore = "writes PNGs for eyeballing; run with --ignored"]
    fn dashboard_module_pngs() {
        let cells = [(90.0, 50.0), (130.0, 110.0), (70.0, 260.0), (420.0, 60.0), (300.0, 200.0), (700.0, 320.0)];
        for (w, h) in cells {
            let c = vec2(w, h);
            let tag = format!("{w}x{h}");
            tile_png(&format!("stats_{tag}"), c, "Session Stats", &|p, r| paint_session_stats(p, r, &stats(), 13.0));
            tile_png(&format!("position_{tag}"), c, "Position",
                &|p, r| paint_position(p, r, [-12345.67, 210.5, 9876.54], [3.0, -0.2, 0.1], 13.0));
            tile_png(&format!("coop_{tag}"), c, "Co-Op", &|p, r| paint_coop_rows(p, r, &coop_rows(), false, 12.0));
            tile_png(&format!("boost_inbar_{tag}"), c, "Boost", &|p, r| paint_boost_in_bar(p, r, &gauge()));
            let vertical = h > w;
            tile_png(&format!("boost_{tag}"), c, "Boost", &|p, r| {
                if vertical { paint_boost_vertical(p, r, &gauge()) } else { paint_boost_horizontal(p, r, &gauge()) }
            });
        }
    }
}
