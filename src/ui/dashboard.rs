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
    GameMode, SprintType, TextAlign, TireDisplayStyle, TireSlipStyle, WidgetKind, WidgetLayout,
};
use crate::i18n::tr;
use crate::packet::ForzaPacket;

const RESIZE_STRIP: f32 = 8.0;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    let Some(pkt) = app.telemetry.latest.clone() else {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new(tr(
                    "Waiting for telemetry…\n\nEnable Data Out in Forza:\nSETTINGS → HUD AND GAMEPLAY → Data Out",
                ))
                .size(18.0)
                .color(crate::theme::TEXT_DIM),
            );
        });
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

fn render_widget(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket, kind: &WidgetKind) {
    match kind {
        WidgetKind::Empty      => {}
        WidgetKind::Speed      => show_speed_widget(ui, app, pkt),
        WidgetKind::Gear       => show_gear_widget(ui, app, pkt),
        WidgetKind::Rpm        => show_rpm_widget(ui, app, pkt),
        WidgetKind::Inputs     => show_inputs_block(ui, app, pkt),
        WidgetKind::Car        => show_car_block(ui, app, pkt),
        WidgetKind::Engine     => show_engine_block(ui, app, pkt),
        WidgetKind::Position   => show_position_block(ui, pkt),
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
fn show_session_stats(ui: &mut Ui, app: &ForzaApp, _pkt: &ForzaPacket) {
    ui.label(crate::theme::section_label(tr("Session Stats")));
    ui.add_space(4.0);

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
    let val_col = Color32::from_rgb(230, 200, 90);

    let mut stat = |label: &str, value: String| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(crate::theme::TEXT_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(value).strong().color(val_col));
            });
        });
    };

    stat(tr("Top Speed"), format!("{spd:.0} {spd_u}"));
    stat(tr("Peak Power"), format!("{:.0} PS", app.max_power_ps));
    stat(tr("Peak Torque"), format!("{:.0} Nm", app.max_torque_nm));
    stat(tr("Peak Boost"), format!("{boost:.2} {boost_u}"));
    stat(tr("Peak Lat G"), format!("{:.2} g", app.gforce_stats.max_lateral));
    stat(tr("Peak Long G"), format!("{:.2} g", app.gforce_stats.max_longitudinal));
    // Cached max, not pkt.engine_max_rpm — the packet field zeroes while paused.
    let max_rpm = app.dynamic_max_rpm.max(app.cached_engine_max_rpm as f32);
    stat(tr("Max RPM"), format!("{max_rpm:.0}"));
}

/// Turbo/supercharger boost gauge — current value + a bar with the session-peak tick.
/// Adapts to its cell: taller-than-wide renders a vertical (bottom-up) gauge,
/// square or wider keeps the default horizontal bar.
fn show_boost_widget(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let full = ui.available_rect_before_wrap();
    let vertical = full.height() > full.width();
    ui.label(crate::theme::section_label(tr("Boost")));

    let use_bar = app.config.use_bar;
    let conv = |psi: f32| if use_bar { psi * 0.068_947_6 } else { psi };
    let unit = if use_bar { "bar" } else { "PSI" };
    let cur = conv(pkt.boost);
    let peak = conv(app.max_boost_psi);
    // Colour ramps green→orange→red with boost pressure.
    let level = (pkt.boost / 20.0).clamp(0.0, 1.0);
    let bar_col = Color32::from_rgb(
        (70.0 + 160.0 * level) as u8,
        (200.0 - 120.0 * level) as u8,
        70,
    );
    let scale = peak.max(cur).max(conv(7.0)) * 1.15;
    let tick = Stroke::new(2.0, Color32::from_rgb(240, 220, 90));

    if vertical {
        // Bottom-up bar on top, stacked readout underneath it. Measure the two
        // readout lines (unwrapped galleys, not a magic constant), reserve them
        // first, and give the bar whatever truly remains.
        let rect = ui.available_rect_before_wrap();
        let spacing = ui.spacing().item_spacing.y;
        let value_galley = ui.painter().layout_no_wrap(
            format!("{cur:+.2}"), egui::FontId::proportional(22.0), bar_col);
        let small_galley = ui.painter().layout_no_wrap(
            format!("{unit} · {} {peak:.2}", tr("peak")),
            egui::FontId::proportional(11.0), crate::theme::TEXT_DIM);
        let bottom_margin = 2.0;
        let min_bar_h = 10.0;
        // Value line + unit·peak line + the spacing between them; drop the
        // small line when the cell is too short for both plus a minimum bar.
        let mut text_h = value_galley.size().y + spacing + small_galley.size().y;
        let mut show_small = true;
        if rect.height() - spacing - min_bar_h - bottom_margin < text_h {
            show_small = false;
            text_h = value_galley.size().y;
        }
        let w = rect.width().min(26.0);
        let bar = egui::Rect::from_min_size(
            pos2(rect.center().x - w * 0.5, rect.top()),
            egui::vec2(w, (rect.height() - text_h - spacing - bottom_margin).max(min_bar_h)),
        );
        ui.allocate_rect(bar, egui::Sense::hover());
        let painter = ui.painter_at(bar);
        painter.rect_filled(bar, 4.0, Color32::from_rgb(22, 24, 27));

        if scale > 0.0 {
            let frac = (cur / scale).clamp(0.0, 1.0);
            if frac > 0.001 {
                let h = bar.height() * frac;
                let fill = egui::Rect::from_min_max(pos2(bar.left(), bar.bottom() - h), bar.max);
                painter.rect_filled(fill, 4.0, bar_col);
            }
            // Peak tick
            let pf = (peak / scale).clamp(0.0, 1.0);
            if pf > 0.001 {
                let y = bar.bottom() - bar.height() * pf;
                painter.line_segment([pos2(bar.left() + 2.0, y), pos2(bar.right() - 2.0, y)], tick);
            }
        }

        // Readout, centred under the bar (painted, so it never wraps).
        let painter = ui.painter();
        let cx = rect.center().x;
        let text_top = bar.bottom() + spacing;
        let value_size = value_galley.size();
        painter.galley(pos2(cx - value_size.x * 0.5, text_top), value_galley, bar_col);
        if show_small {
            painter.galley(
                pos2(cx - small_galley.size().x * 0.5, text_top + value_size.y + spacing),
                small_galley, crate::theme::TEXT_DIM);
        }
        return;
    }

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{cur:+.2}")).size(22.0).strong().color(bar_col));
        ui.label(RichText::new(unit).size(12.0).color(crate::theme::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{} {peak:.2}", tr("peak"))).size(11.0).color(crate::theme::TEXT_DIM));
        });
    });
    ui.add_space(4.0);

    let rect = ui.available_rect_before_wrap();
    let bar = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), rect.height().min(26.0)));
    ui.allocate_rect(bar, egui::Sense::hover());
    let painter = ui.painter_at(bar);
    painter.rect_filled(bar, 4.0, Color32::from_rgb(22, 24, 27));

    if scale > 0.0 {
        let frac = (cur / scale).clamp(0.0, 1.0);
        if frac > 0.001 {
            let fill = egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * frac, bar.height()));
            painter.rect_filled(fill, 4.0, bar_col);
        }
        // Peak tick
        let pf = (peak / scale).clamp(0.0, 1.0);
        if pf > 0.001 {
            let x = bar.left() + bar.width() * pf;
            painter.line_segment([pos2(x, bar.top() + 2.0), pos2(x, bar.bottom() - 2.0)], tick);
        }
    }
}

/// Rolling speed (km/h) + RPM sparkline over the last ~30 s, hand-drawn to match
/// the lightweight dashboard widgets.
fn show_trace_widget(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    ui.label(crate::theme::section_label(tr("Speed Trace")));
    ui.add_space(2.0);

    let use_mph = app.config.use_mph;
    let unit = if use_mph { "mph" } else { "km/h" };
    let speed_disp = if use_mph { pkt.speed_mph() } else { pkt.speed_kmh() };

    // Legend / current values
    ui.horizontal(|ui| {
        ui.colored_label(Color32::from_rgb(80, 200, 110), format!("{speed_disp:.0} {unit}"));
        ui.colored_label(Color32::from_rgb(230, 160, 40), format!("{:.0} rpm", pkt.current_engine_rpm));
    });

    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, egui::Sense::hover());
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
    ui.label(crate::theme::section_label(tr("Co-Op")));

    if app.coop.role() == Role::Off {
        ui.add_space(4.0);
        ui.label(RichText::new(tr("Not in a session.")).size(12.0).color(crate::theme::TEXT_DIM));
        ui.label(RichText::new(tr("Host or join from the Co-Op tab."))
            .size(11.0).color(crate::theme::TEXT_FAINT));
        return;
    }

    let use_mph = app.config.use_mph;
    let unit = if use_mph { "mph" } else { "km/h" };

    // Collect (name, hue, speed m/s, gear, is_self, distance_m). Self first, then remotes.
    let mut rows: Vec<(String, f32, f32, u8, bool, f32)> = Vec::new();
    rows.push((app.config.coop_name.clone(), app.config.coop_hue, pkt.speed, pkt.gear, true, 0.0));
    for (info, rp) in app.coop.remote_players() {
        let dist = ((rp.position_x - pkt.position_x).powi(2)
            + (rp.position_z - pkt.position_z).powi(2)).sqrt();
        rows.push((info.name.clone(), info.hue, rp.speed, rp.gear, false, dist));
    }
    rows.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    // Bar scale: fixed reference top speed so bars are comparable frame-to-frame.
    let max_kmh = 320.0_f32;

    // Static column widths (in monospace character cells) so the row layout never
    // shifts as names/speeds change — only the speed bar in the middle flexes.
    // Monospace advance ≈ 0.6 em; exact value doesn't matter, only that it's constant.
    let mono = egui::TextStyle::Monospace.resolve(ui.style());
    let ch = (mono.size * 0.6).max(6.0);
    let name_w = 15.0 * ch; // rank prefix + 12-char name
    let dist_w = 6.0 * ch;
    let speed_w = 8.0 * ch; // "207 km/h"
    let gear_w = 3.0 * ch;  // "G10"
    let spacing = ui.spacing().item_spacing.x;

    // Fixed-width text cell (min-width forces the reserved space even when empty).
    let cell = |ui: &mut Ui, w: f32, text: RichText, right: bool| {
        let layout = if right {
            egui::Layout::right_to_left(egui::Align::Center)
        } else {
            egui::Layout::left_to_right(egui::Align::Center)
        };
        ui.allocate_ui_with_layout(egui::vec2(w, 18.0), layout, |ui| {
            ui.set_min_width(w);
            ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
        });
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

    ui.add_space(4.0);
    for (rank, (name, hue, speed_ms, gear, is_self, dist_m)) in rows.iter().enumerate() {
        let col = crate::ui::coop::hue_color(*hue);
        let kmh = speed_ms * 3.6;
        let disp = if use_mph { speed_ms * 2.236_94 } else { kmh };
        let gear_str = match gear { 0 => "N".to_string(), g => g.to_string() };
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(14.0, 12.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 5.0, col);

            let name_rt = RichText::new(fit(&format!("{}. {}", rank + 1, name), 15)).monospace();
            cell(ui, name_w, if *is_self { name_rt.strong() } else { name_rt }, false);

            let dtxt = if *is_self {
                String::new()
            } else if *dist_m >= 1000.0 {
                format!("{:.1}km", dist_m / 1000.0)
            } else {
                format!("{:.0}m", dist_m)
            };
            cell(ui, dist_w, RichText::new(dtxt).monospace().size(11.0).color(crate::theme::TEXT_FAINT), true);

            // The one flexible element: fill what's left after the fixed speed+gear cells.
            let bar_w = (ui.available_width() - speed_w - gear_w - 2.0 * spacing).max(20.0);
            ui.add(egui::ProgressBar::new((kmh / max_kmh).clamp(0.0, 1.0))
                .fill(col)
                .desired_width(bar_w));

            cell(ui, speed_w, RichText::new(format!("{disp:>3.0} {unit}")).monospace(), true);
            cell(ui, gear_w, RichText::new(format!("G{gear_str}")).monospace().color(crate::theme::TEXT_DIM), true);
        });
        ui.add_space(2.0);
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
    let avail_h = ui.available_rect_before_wrap().height();
    let rpm_font = (avail_h * 0.20).min(28.0).max(12.0);

    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{}: {:>5.0}", tr("RPM"), pkt.current_engine_rpm))
            .size(rpm_font).strong());
    });
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{}: {:>5.0}", tr("max"), max_rpm))
            .size((rpm_font * 0.45).max(9.0)).color(crate::theme::TEXT_DIM));
    });

    let bar_h = (ui.available_height() - 4.0).max(4.0);
    let bar_size = Vec2::new(ui.available_width(), bar_h);
    let (rect, _) = ui.allocate_exact_size(bar_size, egui::Sense::hover());
    draw_shift_bar(
        ui,
        rect.shrink2(vec2(4.0, 0.0)),
        pkt,
        app.config.shift_low_pct,
        app.config.shift_high_pct,
        max_rpm,
    );
}

// ── Block renderers (unchanged) ────────────────────────────────────

fn show_inputs_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    ui.label(crate::theme::section_label(tr("Inputs")));
    ui.add_space(4.0);

    // Backfire briefly injects a synthetic 'W' keypress, which the game reports back as a real
    // (but fake) pkt.accel for a few frames — hide that blip from the Accel bar when opted in.
    let suppress = app.config.inputs_filter_backfire_accel && app.backfire_echo_active();
    let accel = if suppress { 0 } else { pkt.accel };
    input_bar(ui, tr("Accel"),     accel,          Color32::from_rgb(60, 200, 90));
    input_bar(ui, tr("Brake"),     pkt.brake,      Color32::from_rgb(220, 60, 60));
    input_bar(ui, tr("Clutch"),    pkt.clutch,     Color32::from_rgb(80, 140, 220));
    input_bar(ui, tr("HandBrake"), pkt.hand_brake, Color32::from_rgb(230, 150, 40));

    ui.add_space(6.0);
    ui.label(tr("Steer"));
    draw_steering(ui, pkt.steer);
}

fn show_car_block(ui: &mut Ui, app: &ForzaApp, _pkt: &ForzaPacket) {
    // Capture the full widget rect before the heading consumes any space.
    let full_rect = ui.available_rect_before_wrap();

    ui.label(crate::theme::section_label(tr("Car")));

    let no_data = app.cached_car_class_str.is_empty();
    let class = if no_data { -1 } else { app.cached_car_class };
    let dt = if no_data { -1 } else { app.cached_drivetrain };
    let pi = if no_data { 0 } else { app.cached_car_pi };
    let cyl_text = if no_data || !app.config.car_show_cylinders {
        String::new()
    } else if app.cached_num_cylinders == 0 {
        tr("Electric").to_string()
    } else {
        format!("{} {}", app.cached_num_cylinders, tr("cyl"))
    };
    let has_caption = !cyl_text.is_empty();

    // Gap floors (also reserved in the scale budget below, so the drivetrain
    // label can never be pushed past the cell's bottom edge). Gaps here mean
    // the TOTAL visual gap — egui's implicit item_spacing.y after each widget
    // is part of it, so the add_space calls below subtract it back out.
    let gap = 4.0_f32;
    let spacing = ui.spacing().item_spacing.y;
    let floor_top = 4.0;
    let floor_caption_gap = spacing;
    let floor_mid = gap.max(spacing);
    let floor_bottom = spacing;
    let n_gaps = if has_caption { 4.0 } else { 3.0 };
    let floor_total = floor_top
        + if has_caption { floor_caption_gap } else { 0.0 }
        + floor_mid
        + floor_bottom;

    // Scale the labels to the space that truly remains below the heading —
    // minus the gap floors AND the caption row. The caption is reserved at its
    // minimum size here: it only grows when there is slack, i.e. when width
    // (not height) is the binding constraint, so the estimate stays safe.
    let avail_w = full_rect.width();
    let used_h = ui.next_widget_position().y - full_rect.min.y;
    let avail_h = (full_rect.height() - used_h).max(0.0);
    let caption_min_h = if has_caption {
        ui.fonts_mut(|f| f.row_height(&egui::FontId::proportional(12.0)))
    } else {
        0.0
    };
    let cnative = app.labels.class_size(class, 1.0);
    let dnative = app.labels.drivetrain_size(dt, 1.0);
    let scale = (avail_w * 0.92 / cnative.x.max(dnative.x))
        .min((avail_h - floor_total - caption_min_h).max(0.0) / (cnative.y + dnative.y))
        .clamp(0.2, 1.4);
    let csize = cnative * scale;
    let dsize = dnative * scale;

    // In a narrow/tall cell the label images are width-bound, leaving vertical
    // slack. Spread it across every gap (top margin, caption gap, mid gap,
    // bottom margin) instead of one lump, and let the caption grow with the
    // slack so a generous cell gets a legible caption rather than an
    // afterthought. Each gap keeps its floor, so tight cells fall back to the
    // original layout.
    let slack_estimate = (avail_h - floor_total - caption_min_h - csize.y - dsize.y).max(0.0);
    let caption_size = if has_caption {
        (12.0 + slack_estimate * 0.06).clamp(12.0, 20.0)
    } else {
        12.0
    };
    let caption_font = egui::FontId::proportional(caption_size);
    let caption_h = if has_caption {
        ui.fonts_mut(|f| f.row_height(&caption_font))
    } else {
        0.0
    };

    // Now divide the real leftover slack evenly across the active gaps.
    let slack = (avail_h - floor_total - caption_h - csize.y - dsize.y).max(0.0);
    let extra = slack / n_gaps;
    let top_margin = floor_top + extra;
    let caption_gap = floor_caption_gap + extra;
    let mid_gap = floor_mid + extra;
    let bottom_margin = floor_bottom + extra;

    ui.add_space(top_margin);

    // Cylinder/Electric caption. Painted (not laid out as a Label) so it can
    // never wrap onto a second, unbudgeted row in a narrow cell; if the text
    // would overflow the cell width, the font shrinks to fit instead.
    if has_caption {
        let color = crate::theme::steel(180);
        let mut galley = ui.painter().layout_no_wrap(cyl_text.clone(), caption_font, color);
        if galley.size().x > avail_w * 0.95 {
            let fitted = (caption_size * avail_w * 0.95 / galley.size().x).max(9.0);
            galley = ui.painter().layout_no_wrap(
                cyl_text.clone(),
                egui::FontId::proportional(fitted),
                color,
            );
        }
        let (caprow, _) =
            ui.allocate_exact_size(egui::vec2(avail_w, caption_h), egui::Sense::hover());
        let gsize = galley.size();
        ui.painter().galley(
            egui::pos2(caprow.center().x - gsize.x * 0.5, caprow.center().y - gsize.y * 0.5),
            galley,
            color,
        );
        // The allocation above already advanced the cursor by item_spacing.y —
        // it's part of the budgeted gap, not extra.
        ui.add_space((caption_gap - spacing).max(0.0));
    }

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
    ui.label(crate::theme::section_label(tr("Engine")));
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

    // Full lines carry the "Power:/Torque:/Boost:" label and a "(max …)" tail.
    let full = [
        format!("{}  {:>5.0} PS   ({} {:>5.0})", tr("Power:"),  power_cur,  tr("max"), app.max_power_ps),
        format!("{} {:>5.0} Nm   ({} {:>5.0})",  tr("Torque:"), torque_cur, tr("max"), app.max_torque_nm),
        format!("{}  {:5.2} {boost_unit}  ({} {:5.2})", tr("Boost:"), boost_cur, tr("max"), boost_max),
    ];

    // When the widest full line would overflow the widget (i.e. wrap), drop the
    // leading label and the "max" word so only "value unit (peak)" remains.
    let body_font = egui::TextStyle::Body.resolve(ui.style());
    let widest = full.iter().fold(0.0_f32, |w, s| {
        let gw = ui.painter().layout_no_wrap(s.clone(), body_font.clone(), Color32::WHITE).rect.width();
        w.max(gw)
    });
    if widest > ui.available_width() - 2.0 {
        ui.label(format!("{:>5.0} PS   ({:>5.0})", power_cur, app.max_power_ps));
        ui.label(format!("{:>5.0} Nm   ({:>5.0})", torque_cur, app.max_torque_nm));
        ui.label(format!("{:5.2} {boost_unit}  ({:5.2})", boost_cur, boost_max));
    } else {
        for line in full { ui.label(line); }
    }

    if app.config.game_mode == GameMode::ForzaMotorsport7 {
        ui.add_space(4.0);
        ui.label(format!("{}   {:.0}%", tr("Fuel:"), pkt.fuel * 100.0));
        ui.add(
            egui::ProgressBar::new(pkt.fuel)
                .fill(Color32::from_rgb(60, 160, 240))
                .desired_width(160.0),
        );
    }
}

fn show_position_block(ui: &mut Ui, pkt: &ForzaPacket) {
    ui.label(crate::theme::section_label(tr("Position")));
    ui.add_space(4.0);
    ui.columns(2, |cols| {
        cols[0].label(RichText::new(tr("Position")).size(11.0).color(crate::theme::TEXT_DIM));
        cols[0].label(format!("X: {:>10.2} m", pkt.position_x));
        cols[0].label(format!("Y: {:>10.2} m", pkt.position_y));
        cols[0].label(format!("Z: {:>10.2} m", pkt.position_z));
        cols[1].label(RichText::new(tr("Rotation")).size(11.0).color(crate::theme::TEXT_DIM));
        cols[1].label(format!("{:<7}{:>6.2}°", format!("{}:", tr("Yaw")), pkt.yaw.to_degrees()));
        cols[1].label(format!("{:<7}{:>6.2}°", format!("{}:", tr("Pitch")), pkt.pitch.to_degrees()));
        cols[1].label(format!("{:<7}{:>6.2}°", format!("{}:", tr("Roll")), pkt.roll.to_degrees()));
    });
}

fn show_race_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let is_fh6 = app.config.game_mode == GameMode::ForzaHorizon6;

    if is_fh6 && pkt.race_position == 0 {
        ui.label(crate::theme::section_label(tr("Sprint")));
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
        sprint_row(ui, lbl0, st.zero_to_hundred, c100, stype, false);
        sprint_row(ui, lbl1, st.hundred_to_two, c200, stype, show_other);
        sprint_row(ui, lbl2, st.two_to_three, c300, stype, show_other);
        sprint_row(ui, lbl3, st.three_to_four, c400, stype, show_other);
        sprint_row(ui, lbl4, st.four_to_five, c500, stype, show_other);
    } else {
        ui.label(crate::theme::section_label(tr("Race")));
        ui.add_space(4.0);

        ui.label(format!("{} P{}", tr("Position:"), pkt.race_position));
        ui.label(format!("{:<9} {}", tr("Lap:"), pkt.lap_number));
        ui.add_space(6.0);
        ui.label(RichText::new(format!("{:<9} {}", tr("Current"), fmt_lap(pkt.current_lap))).size(15.0));
        ui.label(RichText::new(format!("{:<9} {}", tr("Last"), fmt_lap(pkt.last_lap))).size(15.0));
        ui.label(
            RichText::new(format!("{:<9} {}", tr("Best"), fmt_lap(pkt.best_lap)))
                .size(15.0)
                .color(Color32::from_rgb(255, 210, 40)),
        );
        ui.add_space(8.0);
        ui.label(format!("{} {}", tr("Race time:"), fmt_lap(pkt.current_race_time)));
        ui.label(format!("{}  {:.1} km", tr("Distance:"), pkt.distance_traveled / 1000.0));
    }
}

fn show_tires_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    ui.label(crate::theme::section_label(tr("Tires")));
    ui.add_space(4.0);
    match app.config.tire_display_style {
        TireDisplayStyle::Separate => show_tires_separate(ui, app, pkt),
        TireDisplayStyle::Combined => show_tires_combined(ui, app, pkt),
        TireDisplayStyle::Bars     => show_tires_bars(ui, app, pkt),
    }
}

fn show_tires_separate(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let use_f = app.config.use_fahrenheit;
    let is_fh6 = app.config.game_mode == GameMode::ForzaHorizon6;
    let slip_style = &app.config.tire_slip_style;

    egui::Grid::new("tire_grid")
        .num_columns(5)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("");
            for lbl in ["FL", "FR", "RL", "RR"] {
                ui.label(RichText::new(lbl).strong());
            }
            ui.end_row();

            ui.label(tr("Temp"));
            tire_temp_label(ui, pkt.tire_temp_fl, use_f);
            tire_temp_label(ui, pkt.tire_temp_fr, use_f);
            tire_temp_label(ui, pkt.tire_temp_rl, use_f);
            tire_temp_label(ui, pkt.tire_temp_rr, use_f);
            ui.end_row();

            ui.label(tr("Slip"));
            for &slip in &[
                pkt.tire_combined_slip_fl,
                pkt.tire_combined_slip_fr,
                pkt.tire_combined_slip_rl,
                pkt.tire_combined_slip_rr,
            ] {
                match slip_style {
                    TireSlipStyle::Values => slip_label(ui, slip),
                    TireSlipStyle::Graph  => draw_slip_circle(ui, slip, false),
                    TireSlipStyle::Both   => draw_slip_circle(ui, slip, true),
                }
            }
            ui.end_row();

            let water_icon = |v: i32| if v != 0 { crate::icons::TINT } else { "  " };
            ui.label(tr("Water"));
            ui.colored_label(Color32::from_rgb(80, 160, 220), water_icon(pkt.wheel_in_puddle_fl));
            ui.colored_label(Color32::from_rgb(80, 160, 220), water_icon(pkt.wheel_in_puddle_fr));
            ui.colored_label(Color32::from_rgb(80, 160, 220), water_icon(pkt.wheel_in_puddle_rl));
            ui.colored_label(Color32::from_rgb(80, 160, 220), water_icon(pkt.wheel_in_puddle_rr));
            ui.end_row();

            if !is_fh6 {
                let rumble = |v: i32| if v != 0 { crate::icons::CIRCLE } else { "  " };
                ui.label(tr("Rumble"));
                ui.label(rumble(pkt.wheel_on_rumble_strip_fl));
                ui.label(rumble(pkt.wheel_on_rumble_strip_fr));
                ui.label(rumble(pkt.wheel_on_rumble_strip_rl));
                ui.label(rumble(pkt.wheel_on_rumble_strip_rr));
                ui.end_row();
            }
        });
}

fn show_tires_combined(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let use_f = app.config.use_fahrenheit;

    let n = 4_f32;
    let gap = 8.0_f32;
    let left_pad = 5.0_f32;
    let right_pad = 5.0_f32;
    let avail = ui.available_rect_before_wrap();
    let avail_w = avail.width();
    let avail_h = avail.height();
    // Cap circle size by both available width and height
    let cell = ((avail_w - left_pad - right_pad - (n - 1.0) * gap) / n)
        .min(avail_h - 4.0)
        .max(10.0);
    let outer_r = cell / 2.0;
    let inner_r = outer_r * 0.55;
    let total_w = left_pad + n * cell + (n - 1.0) * gap;

    let (rect, _) = ui.allocate_exact_size(Vec2::new(total_w, cell), egui::Sense::hover());
    let hole_bg = ui.visuals().panel_fill;
    let p = ui.painter();

    let tires = [
        ("FL", pkt.tire_temp_fl, pkt.tire_combined_slip_fl, pkt.wheel_in_puddle_fl),
        ("FR", pkt.tire_temp_fr, pkt.tire_combined_slip_fr, pkt.wheel_in_puddle_fr),
        ("RL", pkt.tire_temp_rl, pkt.tire_combined_slip_rl, pkt.wheel_in_puddle_rl),
        ("RR", pkt.tire_temp_rr, pkt.tire_combined_slip_rr, pkt.wheel_in_puddle_rr),
    ];

    let bg = crate::theme::WELL;
    let puddle_c = Color32::from_rgb(80, 160, 220);
    let rim_c = crate::theme::STROKE_MID;

    let font_size = ((inner_r / 1.8) * 0.8).max(8.0);
    let line_h = font_size * 1.1;
    let fid = egui::FontId::proportional(font_size);

    for (i, &(label, temp_f, slip, puddle)) in tires.iter().enumerate() {
        let cx = rect.left() + left_pad + i as f32 * (cell + gap) + outer_r;
        let cy = rect.center().y;
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
    let label_w  = 36.0_f32;   // "Temp"/"Slip"/unit label column
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
    ui.label(crate::theme::section_label(tr("G-Forces")));
    ui.add_space(4.0);

    let lat = pkt.acceleration_x / 9.81;
    let lon = pkt.acceleration_z / 9.81;
    let vert = pkt.acceleration_y / 9.81;

    let avail_w = ui.available_width();
    let avail_h = ui.available_rect_before_wrap().height();
    let left_pad = 4.0_f32;
    let right_pad = 4.0_f32;
    let gap = 8.0_f32;

    // Hack NerdFont is monospace: every glyph has the same advance width.
    // advance_width = font_size × 0.60  (Hack's fixed advance ratio).
    // Widest possible line: "  Long: +99.00 g" = 16 chars.
    let body_h = ui.text_style_height(&egui::TextStyle::Body);
    let show_text = app.config.gforce_show_text;
    // Text column takes width only when shown; otherwise the plot spans the widget.
    let text_col_w = if show_text { 16.0 * body_h * 0.60 + 4.0 } else { 0.0 };
    let effective_gap = if show_text { gap } else { 0.0 };

    // Plot fills remaining width, capped to a square by available height.
    let plot_size = (avail_w - left_pad - right_pad - effective_gap - text_col_w)
        .min(avail_h)
        .max(40.0);

    ui.horizontal(|ui| {
        ui.add_space(left_pad);
        draw_gforce_plot(ui, lat, lon, &app.gforce_stats, plot_size);
        if !show_text { return; }
        ui.add_space(gap);
        ui.vertical(|ui| {
            ui.label(RichText::new(tr("Current:")).size(12.0).color(crate::theme::TEXT_DIM));
            ui.label(format!("  {:<5} {:+.2} g", format!("{}:", tr("Lat")), lat));
            ui.label(format!("  {:<5} {:+.2} g", format!("{}:", tr("Long")), lon));
            ui.label(format!("  {:<5} {:+.2} g", format!("{}:", tr("Vert")), vert));
            ui.add_space(4.0);
            ui.label(RichText::new(tr("Peak:")).size(12.0).color(crate::theme::TEXT_DIM));
            ui.colored_label(
                Color32::YELLOW,
                format!("  {:<5} {:.2} g", format!("{}:", tr("Lat")), app.gforce_stats.max_lateral),
            );
            ui.colored_label(
                Color32::YELLOW,
                format!("  {:<5} {:.2} g", format!("{}:", tr("Long")), app.gforce_stats.max_longitudinal),
            );
            ui.colored_label(
                Color32::YELLOW,
                format!("  {:<5} {:.2} g", format!("{}:", tr("Vert")), app.gforce_stats.max_vertical),
            );
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

    ui.label(crate::theme::section_label(tr("Suspension")));
    ui.add_space(4.0);

    let avail_h  = ui.available_rect_before_wrap().height();
    let avail_w  = ui.available_width();
    let label_w  = 28.0_f32;   // "Cur"/"Min"/"Max" label column
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

        let c = cur.clamp(0.0, 1.0);
        let fill = Rect::from_min_max(pos2(rect.left(), px(rect.bottom() - c * rect.height())), rect.max);
        let bar_color = if c < 0.33 { Color32::from_rgb(80, 120, 220) }
                        else if c < 0.66 { Color32::from_rgb(50, 200, 80) }
                        else { Color32::from_rgb(230, 140, 40) };
        p.rect_filled(fill, 0.0, bar_color);

        let alpha = if susp.initialized { 255u8 } else { 80u8 };
        let min_y = rect.bottom() - susp.min[i].clamp(0.0, 1.0) * rect.height();
        let max_y = rect.bottom() - susp.max[i].clamp(0.0, 1.0) * rect.height();
        p.line_segment([pos2(rect.left(), min_y), pos2(rect.right(), min_y)],
            Stroke::new(1.0, Color32::from_rgba_premultiplied(180, 80, 80, alpha)));
        p.line_segment([pos2(rect.left(), max_y), pos2(rect.right(), max_y)],
            Stroke::new(1.0, Color32::from_rgba_premultiplied(80, 180, 80, alpha)));
    }

    // ── Text rows: Cur / Min / Max ─────────────────────────────────
    let text_top = bar_top + bar_h + gap_h;
    let rows: [(&str, Color32, [String; 4]); 3] = [
        (tr("Cur"), dim,   travels.map(|v| format!("{:.2}", v))),
        (tr("Min"), red,   std::array::from_fn(|i| if susp.initialized { format!("{:.2}", susp.min[i]) } else { "0.00".into() })),
        (tr("Max"), green, std::array::from_fn(|i| if susp.initialized { format!("{:.2}", susp.max[i]) } else { "0.00".into() })),
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


fn draw_slip_circle(ui: &mut Ui, slip: f32, show_value: bool) {
    let abs = slip.abs();
    let r = 13.0_f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(r * 2.0 + 2.0), egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();

    painter.circle_filled(center, r, crate::theme::WELL);
    painter.circle_stroke(center, r, Stroke::new(1.0, crate::theme::STROKE_MID));

    let fill_r = abs.min(1.0) * r;
    let base_color = if abs >= 1.0 {
        let t = ((abs - 1.0) / 1.0).clamp(0.0, 1.0);
        Color32::from_rgb(
            (220.0 - 110.0 * t) as u8,
            (60.0 - 30.0 * t) as u8,
            (60.0 - 30.0 * t) as u8,
        )
    } else if abs >= 0.8 {
        Color32::from_rgb(230, 160, 40)
    } else {
        Color32::from_rgb(60, 200, 90)
    };

    let brightness: f32 = if show_value {
        if abs >= 1.0 { 0.25 } else { 0.5 }
    } else {
        1.0
    };
    let fill_color = Color32::from_rgb(
        (base_color.r() as f32 * brightness) as u8,
        (base_color.g() as f32 * brightness) as u8,
        (base_color.b() as f32 * brightness) as u8,
    );

    if fill_r > 0.5 {
        painter.circle_filled(center, fill_r, fill_color);
    }

    if show_value {
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            format!("{:.2}", slip),
            egui::FontId::proportional(9.0),
            Color32::WHITE,
        );
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

    for &pct in &[low, high] {
        let x = rect.left() + rect.width() * pct;
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(2.0, Color32::WHITE),
        );
    }

    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("{:.0} / {:.0}", pkt.current_engine_rpm, max_rpm),
        egui::FontId::proportional(13.0),
        Color32::WHITE,
    );
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
        ui.ctx().request_repaint_after(Duration::from_millis(100));
        let center = rect.center();
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
    let px_per_m  = cfg.minimap_px_per_m;
    let origin_wx = cfg.minimap_world_origin_x;
    let origin_wz = cfg.minimap_world_origin_z;

    let car_x = app.minimap_cached_car_x;
    let car_z = app.minimap_cached_car_z;
    // North-up locks the map (yaw 0); otherwise it's heading-up (rotates with the car).
    let yaw   = if cfg.minimap_north_up { 0.0 } else { app.minimap_smoothed_yaw };

    // Metres visible from widget centre to nearest edge
    let zoom  = app.minimap_current_zoom.max(1.0);
    let scale = rect.width().min(rect.height()) / (2.0 * zoom);

    let [orig_w, orig_h] = app.minimap_orig_size;

    // Rotate world displacement into car-relative screen space.
    // Assumes yaw=0 → car faces +Z (north); positive yaw clockwise viewed from above.
    let cos_yaw = yaw.cos();
    let sin_yaw = yaw.sin();

    let mut mesh = egui::Mesh::with_texture(texture.id());
    mesh.indices = vec![0, 1, 2, 0, 2, 3];

    if cfg.minimap_mirror_edges {
        // Mesh covers the full widget rect; UVs are derived via the inverse world→screen
        // transform and may exceed [0,1] near map edges — MirroredRepeat fills those
        // regions with a reflected copy of the map.
        let half_w = rect.width()  * 0.5;
        let half_h = rect.height() * 0.5;
        let inv_scale = 1.0 / scale;
        for (sx, sy) in [(-half_w, -half_h), (half_w, -half_h), (half_w, half_h), (-half_w, half_h)] {
            let wx = car_x + (sx * cos_yaw - sy * sin_yaw) * inv_scale;
            let wz = car_z - (sx * sin_yaw + sy * cos_yaw) * inv_scale;
            mesh.vertices.push(egui::epaint::Vertex {
                pos:   pos2(cx + sx, cy + sy),
                uv:    pos2((wx - origin_wx) * px_per_m / orig_w as f32,
                            (origin_wz - wz) * px_per_m / orig_h as f32),
                color: Color32::WHITE,
            });
        }
    } else {
        // Mesh covers exactly the map image; UVs are always [0,1] so no mirroring occurs.
        let map_world_w = orig_w as f32 / px_per_m;
        let map_world_h = orig_h as f32 / px_per_m;
        let corners = [
            (origin_wx,               origin_wz,               pos2(0.0, 0.0)),
            (origin_wx + map_world_w, origin_wz,               pos2(1.0, 0.0)),
            (origin_wx + map_world_w, origin_wz - map_world_h, pos2(1.0, 1.0)),
            (origin_wx,               origin_wz - map_world_h, pos2(0.0, 1.0)),
        ];
        for (wx, wz, uv) in corners {
            let dx = wx - car_x;
            let dz = wz - car_z;
            mesh.vertices.push(egui::epaint::Vertex {
                pos:   pos2(cx + (dx * cos_yaw - dz * sin_yaw) * scale,
                            cy - (dx * sin_yaw + dz * cos_yaw) * scale),
                uv,
                color: Color32::WHITE,
            });
        }
    }

    let painter = ui.painter_at(rect);
    painter.add(egui::Shape::Mesh(std::sync::Arc::new(mesh)));

    // Co-op breadcrumb trails (drawn behind the car arrows). Each player's recent
    // path fades from faint (old) to solid (recent) in their identity colour.
    if !app.minimap_trails.is_empty() {
        let to_screen = |wx: f32, wz: f32| -> Pos2 {
            let dx = wx - car_x;
            let dz = wz - car_z;
            pos2(cx + (dx * cos_yaw - dz * sin_yaw) * scale,
                 cy - (dx * sin_yaw + dz * cos_yaw) * scale)
        };
        let now = std::time::Instant::now();
        let fade_secs = cfg.coop_trail_fade_secs.max(0.5);
        let fade_m = cfg.coop_trail_fade_m.max(1.0);
        let draw_trail = |pts: &std::collections::VecDeque<(f32, f32, std::time::Instant)>, col: Color32| {
            let n = pts.len();
            if n < 2 { return; }
            let (hx, hz, _) = pts[n - 1]; // head = player's current position
            for i in 1..n {
                let (ax, az, _) = pts[i - 1];
                let (bx, bz, bt) = pts[i];
                // Fade the segment out by whichever hits first: age or distance behind.
                let age = now.duration_since(bt).as_secs_f32();
                let dist = (ax - hx).hypot(az - hz);
                let tf = (1.0 - age / fade_secs).clamp(0.0, 1.0);
                let df = (1.0 - dist / fade_m).clamp(0.0, 1.0);
                let alpha = (tf.min(df) * 220.0) as u8;
                if alpha < 4 { continue; }
                let c = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha);
                painter.line_segment([to_screen(ax, az), to_screen(bx, bz)], Stroke::new(2.0, c));
            }
        };
        if let Some(tr) = app.minimap_trails.get("local") {
            draw_trail(tr, crate::ui::coop::hue_color(app.config.coop_hue));
        }
        for (info, pkt) in app.coop.remote_players() {
            if pkt.is_paused() {
                continue; // paused teammate — don't draw their line
            }
            if let Some(tr) = app.minimap_trails.get(&info.id) {
                draw_trail(tr, crate::ui::coop::hue_color(info.hue));
            }
        }
    }

    let s = 7.0_f32;

    // Remote co-op players: place each on the map relative to the local car using
    // the same world→screen rotation, with their identity colour + name.
    let remotes = app.coop.remote_players();
    if !remotes.is_empty() {
        // Names are drawn in a second pass so labels of cars close together (e.g. racing
        // side-by-side) can be nudged apart instead of stacking illegibly.
        let mut labels: Vec<(Pos2, String, Color32)> = Vec::new();
        for (info, pkt) in remotes {
            let paused = pkt.is_paused();
            // Paused players stop broadcasting a valid position; show them at their
            // last-known spot in grey instead of drawing them at the world origin.
            let (px, pz, pyaw) = if paused {
                match app.coop_last_pos.get(&info.id) {
                    Some(s) => (s.x, s.z, s.yaw),
                    None => continue, // never seen at a valid spot — nothing to show
                }
            } else {
                (pkt.position_x, pkt.position_z, pkt.yaw)
            };
            let dx = px - car_x;
            let dz = pz - car_z;
            let sx = cx + (dx * cos_yaw - dz * sin_yaw) * scale;
            let sy = cy - (dx * sin_yaw + dz * cos_yaw) * scale;
            let col = if paused {
                crate::theme::steel(170)
            } else {
                crate::ui::coop::hue_color(info.hue)
            };

            if rect.shrink(8.0).contains(pos2(sx, sy)) {
                // On-screen: full heading arrow; name deferred to the 2nd pass.
                let (sa, ca) = (pyaw - yaw).sin_cos();
                let rr = |vx: f32, vy: f32| pos2(sx + vx * ca - vy * sa, sy + vx * sa + vy * ca);
                painter.add(egui::Shape::convex_polygon(
                    vec![rr(0.0, -s * 1.4), rr(s, s * 0.6), rr(-s, s * 0.6)],
                    col,
                    Stroke::new(1.5, Color32::BLACK),
                ));
                let label = if paused {
                    format!("{} {}", crate::icons::PAUSE, info.name)
                } else {
                    info.name.clone()
                };
                labels.push((pos2(sx, sy - s * 1.9), label, col));
            } else {
                // Off-screen: clamp to the map edge and point a marker toward them,
                // so you always know which way your teammates are.
                let c = rect.center();
                let d = pos2(sx, sy) - c;
                let half = rect.size() * 0.5 - Vec2::splat(10.0);
                let kx = if d.x.abs() > 0.01 { half.x / d.x.abs() } else { f32::INFINITY };
                let ky = if d.y.abs() > 0.01 { half.y / d.y.abs() } else { f32::INFINITY };
                let edge = c + d * kx.min(ky).min(1.0);
                let (sa, ca) = d.y.atan2(d.x).sin_cos();
                let m = 6.5_f32;
                let rr = |vx: f32, vy: f32| pos2(edge.x + vx * ca - vy * sa, edge.y + vx * sa + vy * ca);
                painter.add(egui::Shape::convex_polygon(
                    vec![rr(m, 0.0), rr(-m * 0.7, m * 0.7), rr(-m * 0.7, -m * 0.7)],
                    col,
                    Stroke::new(1.0, Color32::BLACK),
                ));
                // Distance to the teammate, nudged inward from the edge marker.
                let dist_m = (dx * dx + dz * dz).sqrt();
                let dist_txt = if dist_m >= 1000.0 {
                    format!("{:.1}km", dist_m / 1000.0)
                } else {
                    format!("{:.0}m", dist_m)
                };
                let dpos = edge - d.normalized() * 14.0;
                let dfont = egui::FontId::proportional(10.0);
                for off in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
                    painter.text(dpos + vec2(off.0, off.1), egui::Align2::CENTER_CENTER,
                        &dist_txt, dfont.clone(), Color32::from_black_alpha(200));
                }
                painter.text(dpos, egui::Align2::CENTER_CENTER, &dist_txt, dfont, col);
            }
        }

        // 2nd pass: draw names, nudging each down while it collides with a placed one.
        let name_font = egui::FontId::proportional(11.0);
        let shadow = Color32::from_black_alpha(200);
        let mut placed: Vec<Pos2> = Vec::new();
        for (mut pos, name, col) in labels {
            while placed.iter().any(|p| (p.x - pos.x).abs() < 46.0 && (p.y - pos.y).abs() < 13.0) {
                pos.y += 13.0;
            }
            placed.push(pos);
            for off in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
                painter.text(pos + vec2(off.0, off.1), egui::Align2::CENTER_BOTTOM,
                    &name, name_font.clone(), shadow);
            }
            painter.text(pos, egui::Align2::CENTER_BOTTOM, &name, name_font.clone(), col);
        }
    }

    // Local car indicator: triangle rotated to show heading relative to map orientation.
    // Uses the player's co-op colour (colour only, no name) when in a session.
    let local_col = if app.coop.role() != crate::coop::Role::Off {
        crate::ui::coop::hue_color(app.config.coop_hue)
    } else {
        Color32::WHITE
    };
    let arrow_angle = app.minimap_cached_raw_yaw - yaw;
    let (sin_a, cos_a) = arrow_angle.sin_cos();
    let rot = |vx: f32, vy: f32| -> Pos2 {
        pos2(cx + vx * cos_a - vy * sin_a, cy + vx * sin_a + vy * cos_a)
    };
    let tip   = rot(0.0,      -s * 1.4);
    let left  = rot(-s,        s * 0.6);
    let right = rot( s,        s * 0.6);
    painter.add(egui::Shape::convex_polygon(
        vec![tip, right, left],
        local_col,
        Stroke::new(1.5, Color32::BLACK),
    ));

    // ── Co-op shared waypoint ──────────────────────────────────────
    // Left-click drops/moves a waypoint everyone in the session sees; right-click clears.
    if !app.config.dashboard_edit_mode && app.coop.role() != crate::coop::Role::Off {
        if map_resp.clicked() {
            if let Some(m) = map_resp.interact_pointer_pos() {
                let a = (m.x - cx) / scale;
                let b = -(m.y - cy) / scale;
                let wx = car_x + a * cos_yaw + b * sin_yaw;
                let wz = car_z - a * sin_yaw + b * cos_yaw;
                app.coop.set_waypoint(Some((wx, wz)), app.config.coop_hue);
            }
        }
        if map_resp.secondary_clicked() {
            app.coop.set_waypoint(None, 0.0);
        }
    }
    for (_pid, wx, wz, hue) in app.coop.waypoints() {
        let dx = wx - car_x;
        let dz = wz - car_z;
        let mut mx = cx + (dx * cos_yaw - dz * sin_yaw) * scale;
        let mut my = cy - (dx * sin_yaw + dz * cos_yaw) * scale;
        let col = crate::ui::coop::hue_color(hue);
        if !rect.shrink(6.0).contains(pos2(mx, my)) {
            let d = pos2(mx, my) - rect.center();
            let half = rect.size() * 0.5 - Vec2::splat(8.0);
            let kx = if d.x.abs() > 0.01 { half.x / d.x.abs() } else { f32::INFINITY };
            let ky = if d.y.abs() > 0.01 { half.y / d.y.abs() } else { f32::INFINITY };
            let e = rect.center() + d * kx.min(ky).min(1.0);
            mx = e.x;
            my = e.y;
        }
        let c = pos2(mx, my);
        // Gentle pulse to draw the eye to the destination.
        let p = 1.0 + 0.16 * (ui.input(|i| i.time) as f32 * 4.0).sin();
        painter.add(egui::Shape::convex_polygon(
            vec![c + vec2(0.0, -9.0 * p), c + vec2(7.0 * p, 0.0), c + vec2(0.0, 9.0 * p), c + vec2(-7.0 * p, 0.0)],
            col,
            Stroke::new(1.5, Color32::BLACK),
        ));
        painter.circle_filled(c, 2.5, Color32::WHITE);
        let dist = (dx * dx + dz * dz).sqrt();
        let dtxt = if dist >= 1000.0 { format!("{:.1}km", dist / 1000.0) } else { format!("{:.0}m", dist) };
        let dfont = egui::FontId::proportional(10.0);
        let dpos = c + vec2(0.0, -12.0);
        for off in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
            painter.text(dpos + vec2(off.0, off.1), egui::Align2::CENTER_BOTTOM, &dtxt, dfont.clone(), Color32::from_black_alpha(200));
        }
        painter.text(dpos, egui::Align2::CENTER_BOTTOM, &dtxt, dfont, col);
    }

    // North compass — the map is heading-up (rotates with the car), so show where
    // north is. `yaw` is the map rotation; screen-north is the up vector rotated by it.
    if cfg.minimap_show_compass {
        let cc = rect.min + vec2(22.0, 22.0);
        let r = 12.0_f32;
        painter.circle_filled(cc, r + 2.0, Color32::from_black_alpha(130));
        painter.circle_stroke(cc, r, Stroke::new(1.0, crate::theme::steel(150)));
        let (ns, nc) = yaw.sin_cos();
        let north = vec2(-ns, -nc); // screen direction of world-north
        painter.line_segment([cc, cc + north * r], Stroke::new(2.0, Color32::from_rgb(230, 80, 80)));
        painter.text(cc + north * (r + 6.0), egui::Align2::CENTER_CENTER, "N",
            egui::FontId::proportional(11.0), Color32::WHITE);
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

fn show_power_graph_widget(ui: &mut Ui, app: &ForzaApp) {
    ui.heading(tr("Power Graph"));
    ui.add_space(4.0);

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

    // Optional boost line from the same series the Boost Graph uses.
    let use_bar = app.config.use_bar;
    let boost_series: Vec<[f64; 2]> = if app.config.power_graph_show_boost {
        let raw: &[[f64; 2]] = if !app.power_capture.boost_series.is_empty() {
            &app.power_capture.boost_series
        } else if let Some(curve) = app.saved_power_curve.as_ref() {
            &curve.boost_series
        } else {
            &[]
        };
        raw.iter()
            .map(|&[rpm, psi]| {
                let val = if use_bar { psi * 0.0689476 } else { psi };
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
        // Same headroom style as the Boost Graph widget.
        let max_boost = boost_series.iter().map(|&[_, v]| v).fold(0.0_f64, f64::max);
        let min_headroom = if use_bar { 0.25 } else { 3.0 };
        max_boost + (max_boost.abs() * 0.15).max(min_headroom)
    };
    let boost_scale = y_top / boost_top;

    let engine_max_rpm = if app.cached_engine_max_rpm > 0.0 {
        app.cached_engine_max_rpm
    } else {
        8000.0
    };

    // The rotated y-axis label overhangs the plot's left edge (egui_plot draws it
    // at rect.left() - gap), so give the plot a child rect with left padding or
    // the widget cell clips the label. Right padding keeps it off the cell edge.
    let mut plot_rect = ui.available_rect_before_wrap();
    plot_rect.min.x += 8.0;
    plot_rect.max.x -= 8.0;
    let mut plot_ui = ui.new_child(egui::UiBuilder::new().max_rect(plot_rect).layout(*ui.layout()));
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
    let mut plot = Plot::new("dash_power_graph")
        .legend(Legend::default().position(egui_plot::Corner::RightBottom).follow_insertion_order(true))
        .x_axis_label(tr("RPM"))
        .custom_y_axes(y_axes)
        .include_x(0.0)
        .include_x(engine_max_rpm)
        .include_y(0.0)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false);
    if power_series.is_empty() {
        // No captured data yet — keep the empty plot's axes at a sensible extent.
        plot = plot.include_y(100.0);
    }
    plot.show(&mut plot_ui, |plot_ui| {
        if !power_series.is_empty() {
            plot_ui.line(
                Line::new(tr("Power (PS)"), PlotPoints::new(power_series))
                    .color(Color32::from_rgb(80, 160, 240))
                    .width(2.5),
            );
            plot_ui.line(
                Line::new(tr("Torque (Nm)"), PlotPoints::new(torque_series))
                    .color(Color32::from_rgb(240, 140, 40))
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
                    .color(Color32::from_rgb(180, 80, 220))
                    .width(2.0),
            );
        }
    });
}

fn show_boost_graph_widget(ui: &mut Ui, app: &ForzaApp) {
    ui.heading(tr("Boost Graph"));
    ui.add_space(4.0);

    let saved_curve = app.saved_power_curve.as_ref();

    // Same forced-induction visibility rule as the Power Curve tab:
    // Detection ON  → show boost only when positive pressure was actually captured.
    // Detection OFF → always show boost (no filtering).
    let has_boost_data = if app.config.power_curve_forced_induction {
        app.power_capture.boost_series.iter().any(|&[_, v]| v > 0.05)
            || saved_curve
                .map(|curve| curve.boost_series.iter().any(|&[_, v]| v > 0.05))
                .unwrap_or(false)
            || (app.config.power_curve_save_fi_state && app.fi_detected)
    } else {
        true
    };

    // Live capture, falling back to the saved reference curve.
    let boost_series: &[[f64; 2]] = if !app.power_capture.boost_series.is_empty() {
        &app.power_capture.boost_series
    } else if let Some(curve) = saved_curve {
        &curve.boost_series
    } else {
        &[]
    };

    // Forced-induction detection controls only whether bars are plotted; the
    // plot itself (axes/grid) always renders.
    let plot_bars = has_boost_data && !boost_series.is_empty();

    let engine_max_rpm = if app.cached_engine_max_rpm > 0.0 {
        app.cached_engine_max_rpm
    } else {
        8000.0
    };

    let use_bar = app.config.use_bar;
    let step = app.config.power_curve_step as f64;
    let max_boost = boost_series
        .iter()
        .map(|&[_, psi]| if use_bar { psi * 0.0689476 } else { psi })
        .fold(0.0_f64, f64::max);
    let min_headroom = if use_bar { 0.25 } else { 3.0 };
    let boost_top = if max_boost.is_finite() {
        max_boost + (max_boost.abs() * 0.15).max(min_headroom)
    } else {
        min_headroom
    };

    let bars: Vec<Bar> = if plot_bars {
        boost_series
            .iter()
            .map(|&[rpm, psi]| {
                let val = if use_bar { psi * 0.0689476 } else { psi };
                Bar::new(rpm, val)
                    .fill(Color32::from_rgb(180, 80, 220))
                    .width(step * 0.8)
            })
            .collect()
    } else {
        Vec::new()
    };

    let boost_label = if use_bar { tr("Boost (bar)") } else { tr("Boost (PSI)") };
    // Left/right padding for the rotated y-axis label's overhang (see show_power_graph_widget).
    let mut plot_rect = ui.available_rect_before_wrap();
    plot_rect.min.x += 8.0;
    plot_rect.max.x -= 8.0;
    let mut plot_ui = ui.new_child(egui::UiBuilder::new().max_rect(plot_rect).layout(*ui.layout()));
    Plot::new("dash_boost_graph")
        .x_axis_label(tr("RPM"))
        .y_axis_label(boost_label)
        .include_x(0.0)
        .include_x(engine_max_rpm)
        .include_y(0.0)
        .include_y(boost_top)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .show(&mut plot_ui, |plot_ui| {
            if !bars.is_empty() {
                plot_ui.bar_chart(BarChart::new(tr("Boost"), bars));
            }
        });
}

// ── Helpers ────────────────────────────────────────────────────────

fn sprint_row(
    ui: &mut Ui,
    label: &str,
    segment: Option<f32>,
    cumulative: Option<f32>,
    stype: &SprintType,
    show_other: bool,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{label:12}")).size(14.0).strong());

        let (main, secondary) = match stype {
            SprintType::Incremental => (segment, cumulative),
            SprintType::Absolute    => (cumulative, segment),
        };

        match main {
            Some(t) => {
                ui.label(
                    RichText::new(format!("{t:.3}s"))
                        .size(14.0)
                        .color(Color32::from_rgb(60, 210, 100)),
                );
                if show_other {
                    if let Some(s) = secondary {
                        ui.label(
                            RichText::new(format!("({s:.3}s)"))
                                .size(12.0)
                                .color(crate::theme::TEXT_DIM),
                        );
                    }
                }
            }
            None => {
                ui.label(RichText::new("--").color(crate::theme::TEXT_DIM));
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

fn tire_temp_label(ui: &mut Ui, temp_f: f32, use_f: bool) {
    let (val, unit) = if use_f {
        (temp_f, "°F")
    } else {
        (ForzaPacket::tire_temp_celsius(temp_f), "°C")
    };
    ui.colored_label(temp_color(val, use_f), format!("{val:.0}{unit}"));
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

fn slip_label(ui: &mut Ui, slip: f32) {
    let abs = slip.abs();
    let color = if abs >= 1.0 {
        Color32::from_rgb(220, 60, 60)
    } else if abs >= 0.8 {
        Color32::from_rgb(230, 160, 40)
    } else {
        Color32::from_rgb(60, 200, 90)
    };
    ui.colored_label(color, format!("{slip:.2}"));
}

fn fmt_lap(secs: f32) -> String {
    if secs <= 0.0 {
        return "--:--.---".to_string();
    }
    let m = (secs / 60.0) as u32;
    let s = secs % 60.0;
    format!("{m}:{s:06.3}")
}
