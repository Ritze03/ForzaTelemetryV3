use std::collections::HashSet;

use egui::{
    Align, Color32, Layout, Pos2, Rect, RichText, Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};

use crate::app::{
    DashboardDragState, DashboardResizeState, ForzaApp, GForceStats, ResizeEdge,
};
use crate::config::{
    GameMode, SprintType, TextAlign, TireDisplayStyle, TireSlipStyle, WidgetKind, WidgetLayout,
};
use crate::packet::ForzaPacket;

const RESIZE_STRIP: f32 = 8.0;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    let Some(pkt) = app.telemetry.latest.clone() else {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new(
                    "Waiting for telemetry…\n\nEnable Data Out in Forza:\nSETTINGS → HUD AND GAMEPLAY → Data Out",
                )
                .size(18.0)
                .color(Color32::GRAY),
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
        let empty_stroke = Stroke::new(1.0, Color32::from_rgb(38, 38, 38));
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
        WidgetKind::Race       => show_race_block(ui, app, pkt),
        WidgetKind::Tires      => show_tires_block(ui, app, pkt),
        WidgetKind::GForce     => show_gforce_block(ui, app, pkt),
        WidgetKind::Suspension => show_suspension_block(ui, app, pkt),
        WidgetKind::MiniMap    => show_minimap_widget(ui, app),
    }
}

// ── New top-row widget renderers ───────────────────────────────────

fn show_speed_widget(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let speed = if app.config.use_mph { pkt.speed_mph() } else { pkt.speed_kmh() };
    let unit_str = if app.config.use_mph { "Mph" } else { "Km/h" };
    let legend_color = Color32::from_rgb(140, 140, 140);

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
                gray_str, fid.clone(), Color32::from_rgb(70, 70, 70));
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
    let legend_color = Color32::from_rgb(140, 140, 140);

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
        "Gear",
        egui::FontId::proportional(12.0),
        legend_color,
    );

    ui.allocate_space(avail.size());
}

fn show_rpm_widget(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let max_rpm = pkt.engine_max_rpm.max(1.0);
    let avail_h = ui.available_rect_before_wrap().height();
    let rpm_font = (avail_h * 0.20).min(28.0).max(12.0);

    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(RichText::new(format!("RPM: {:>5.0}", pkt.current_engine_rpm))
            .size(rpm_font).strong());
    });
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(RichText::new(format!("max: {:>5.0}", max_rpm))
            .size((rpm_font * 0.45).max(9.0)).color(Color32::GRAY));
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

fn show_inputs_block(ui: &mut Ui, _app: &ForzaApp, pkt: &ForzaPacket) {
    ui.heading("Inputs");
    ui.add_space(4.0);

    input_bar(ui, "Accel",     pkt.accel,      Color32::from_rgb(60, 200, 90));
    input_bar(ui, "Brake",     pkt.brake,      Color32::from_rgb(220, 60, 60));
    input_bar(ui, "Clutch",    pkt.clutch,     Color32::from_rgb(80, 140, 220));
    input_bar(ui, "HandBrake", pkt.hand_brake, Color32::from_rgb(230, 150, 40));

    ui.add_space(6.0);
    ui.label("Steer");
    draw_steering(ui, pkt.steer);
}

fn show_car_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    ui.heading("Car");
    ui.add_space(4.0);

    ui.label(RichText::new(format!("Ordinal #{}", pkt.car_ordinal)).size(16.0).strong());
    ui.label(format!("Class {} — PI {}", app.cached_car_class_str, app.cached_car_pi));
    ui.label(format!("{} | {} cyl", app.cached_drivetrain_str, app.cached_num_cylinders));

    ui.add_space(8.0);

    let (boost_cur, boost_max, boost_unit) = if app.config.use_bar {
        (pkt.boost * 0.0689476, app.max_boost_psi * 0.0689476, "bar")
    } else {
        (pkt.boost, app.max_boost_psi, "PSI")
    };

    ui.label(format!(
        "Power:  {:>5.0} PS   (max {:>5.0})",
        pkt.power_ps().max(0.0),
        app.max_power_ps
    ));
    ui.label(format!(
        "Torque: {:>5.0} Nm   (max {:>5.0})",
        pkt.torque_nm().max(0.0),
        app.max_torque_nm
    ));
    ui.label(format!(
        "Boost:  {:5.2} {boost_unit}  (max {:5.2})",
        boost_cur.max(0.0),
        boost_max.max(0.0)
    ));

    if app.config.game_mode == GameMode::ForzaMotorsport7 {
        ui.add_space(4.0);
        ui.label(format!("Fuel:   {:.0}%", pkt.fuel * 100.0));
        ui.add(
            egui::ProgressBar::new(pkt.fuel)
                .fill(Color32::from_rgb(60, 160, 240))
                .desired_width(160.0),
        );
    }

    if app.config.car_widget_show_position {
        ui.add_space(4.0);
        ui.label(RichText::new("Position").size(11.0).color(Color32::GRAY));
        ui.label(format!("X: {:>10.2} m", pkt.position_x));
        ui.label(format!("Y: {:>10.2} m", pkt.position_y));
        ui.label(format!("Z: {:>10.2} m", pkt.position_z));
    }

    if app.config.car_widget_show_rotation {
        ui.add_space(4.0);
        ui.label(RichText::new("Rotation").size(11.0).color(Color32::GRAY));
        ui.label(format!("Yaw:   {:>8.2}°", pkt.yaw.to_degrees()));
        ui.label(format!("Pitch: {:>8.2}°", pkt.pitch.to_degrees()));
        ui.label(format!("Roll:  {:>8.2}°", pkt.roll.to_degrees()));
    }
}

fn show_race_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let is_fh6 = app.config.game_mode == GameMode::ForzaHorizon6;

    if is_fh6 && pkt.race_position == 0 {
        ui.heading("Sprint");
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
        ui.heading("Race");
        ui.add_space(4.0);

        ui.label(format!("Position: P{}", pkt.race_position));
        ui.label(format!("Lap:      {}", pkt.lap_number));
        ui.add_space(6.0);
        ui.label(RichText::new(format!("Current   {}", fmt_lap(pkt.current_lap))).size(15.0));
        ui.label(RichText::new(format!("Last      {}", fmt_lap(pkt.last_lap))).size(15.0));
        ui.label(
            RichText::new(format!("Best      {}", fmt_lap(pkt.best_lap)))
                .size(15.0)
                .color(Color32::from_rgb(255, 210, 40)),
        );
        ui.add_space(8.0);
        ui.label(format!("Race time: {}", fmt_lap(pkt.current_race_time)));
        ui.label(format!("Distance:  {:.1} km", pkt.distance_traveled / 1000.0));
    }
}

fn show_tires_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    ui.heading("Tires");
    ui.add_space(4.0);
    match app.config.tire_display_style {
        TireDisplayStyle::Separate => show_tires_separate(ui, app, pkt),
        TireDisplayStyle::Combined => show_tires_combined(ui, app, pkt),
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

            ui.label("Temp");
            tire_temp_label(ui, pkt.tire_temp_fl, use_f);
            tire_temp_label(ui, pkt.tire_temp_fr, use_f);
            tire_temp_label(ui, pkt.tire_temp_rl, use_f);
            tire_temp_label(ui, pkt.tire_temp_rr, use_f);
            ui.end_row();

            ui.label("Slip");
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
            ui.label("Water");
            ui.colored_label(Color32::from_rgb(80, 160, 220), water_icon(pkt.wheel_in_puddle_fl));
            ui.colored_label(Color32::from_rgb(80, 160, 220), water_icon(pkt.wheel_in_puddle_fr));
            ui.colored_label(Color32::from_rgb(80, 160, 220), water_icon(pkt.wheel_in_puddle_rl));
            ui.colored_label(Color32::from_rgb(80, 160, 220), water_icon(pkt.wheel_in_puddle_rr));
            ui.end_row();

            if !is_fh6 {
                let rumble = |v: i32| if v != 0 { crate::icons::CIRCLE } else { "  " };
                ui.label("Rumble");
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

    let bg = Color32::from_rgb(20, 20, 20);
    let puddle_c = Color32::from_rgb(80, 160, 220);
    let rim_c = Color32::from_rgb(80, 80, 80);

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

fn show_gforce_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    ui.heading("G-Forces");
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
    let text_col_w = 16.0 * body_h * 0.60 + 4.0;

    // Plot fills remaining width, capped to a square by available height.
    let plot_size = (avail_w - left_pad - right_pad - gap - text_col_w)
        .min(avail_h)
        .max(40.0);

    ui.horizontal(|ui| {
        ui.add_space(left_pad);
        draw_gforce_plot(ui, lat, lon, &app.gforce_stats, plot_size);
        ui.add_space(gap);
        ui.vertical(|ui| {
            ui.label(RichText::new("Current:").size(12.0).color(Color32::GRAY));
            ui.label(format!("  Lat:  {:+.2} g", lat));
            ui.label(format!("  Long: {:+.2} g", lon));
            ui.label(format!("  Vert: {:+.2} g", vert));
            ui.add_space(4.0);
            ui.label(RichText::new("Peak:").size(12.0).color(Color32::GRAY));
            ui.colored_label(
                Color32::YELLOW,
                format!("  Lat:  {:.2} g", app.gforce_stats.max_lateral),
            );
            ui.colored_label(
                Color32::YELLOW,
                format!("  Long: {:.2} g", app.gforce_stats.max_longitudinal),
            );
            ui.colored_label(
                Color32::YELLOW,
                format!("  Vert: {:.2} g", app.gforce_stats.max_vertical),
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

    ui.heading("Suspension");
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
    let gray  = Color32::GRAY;
    let text_col = ui.visuals().text_color();

    // ── Column header: FL / FR / RL / RR ──────────────────────────
    for (i, lbl) in ["FL", "FR", "RL", "RR"].iter().enumerate() {
        let cx = origin.x + label_w + (i as f32 + 0.5) * bar_w;
        let cy = origin.y + header_h * 0.5;
        p.text(pos2(cx, cy), egui::Align2::CENTER_CENTER, *lbl, fid.clone(), text_col);
    }

    // ── Bars ──────────────────────────────────────────────────────
    let bar_top = origin.y + header_h;
    for (i, &cur) in travels.iter().enumerate() {
        let x    = origin.x + label_w + i as f32 * bar_w;
        let rect = Rect::from_min_size(pos2(x + 4.0, bar_top), vec2(bar_w - 8.0, bar_h));

        p.rect_filled(rect, 2.0, Color32::from_rgb(40, 40, 40));

        let c = cur.clamp(0.0, 1.0);
        let fill = Rect::from_min_max(pos2(rect.left(), rect.bottom() - c * bar_h), rect.max);
        let bar_color = if c < 0.33 { Color32::from_rgb(80, 120, 220) }
                        else if c < 0.66 { Color32::from_rgb(50, 200, 80) }
                        else { Color32::from_rgb(230, 140, 40) };
        p.rect_filled(fill, 0.0, bar_color);

        let alpha = if susp.initialized { 255u8 } else { 80u8 };
        let min_y = rect.bottom() - susp.min[i].clamp(0.0, 1.0) * bar_h;
        let max_y = rect.bottom() - susp.max[i].clamp(0.0, 1.0) * bar_h;
        p.line_segment([pos2(rect.left(), min_y), pos2(rect.right(), min_y)],
            Stroke::new(1.0, Color32::from_rgba_premultiplied(180, 80, 80, alpha)));
        p.line_segment([pos2(rect.left(), max_y), pos2(rect.right(), max_y)],
            Stroke::new(1.0, Color32::from_rgba_premultiplied(80, 180, 80, alpha)));
    }

    // ── Text rows: Cur / Min / Max ─────────────────────────────────
    let text_top = bar_top + bar_h + gap_h;
    let rows: [(&str, Color32, [String; 4]); 3] = [
        ("Cur", gray,  travels.map(|v| format!("{:.2}", v))),
        ("Min", red,   std::array::from_fn(|i| if susp.initialized { format!("{:.2}", susp.min[i]) } else { "0.00".into() })),
        ("Max", green, std::array::from_fn(|i| if susp.initialized { format!("{:.2}", susp.max[i]) } else { "0.00".into() })),
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

    painter.rect_filled(rect, 4.0, Color32::from_rgb(40, 40, 40));

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

    painter.circle_filled(center, radius, Color32::from_rgb(28, 28, 28));
    painter.circle_stroke(center, radius, Stroke::new(1.0, Color32::from_rgb(80, 80, 80)));

    for g in [1.0_f32, 2.0] {
        painter.circle_stroke(
            center,
            g / max_g * radius,
            Stroke::new(0.5, Color32::from_rgb(55, 55, 55)),
        );
    }

    let dim = Color32::from_rgb(55, 55, 55);
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

    painter.circle_filled(center, r, Color32::from_rgb(20, 20, 20));
    painter.circle_stroke(center, r, Stroke::new(1.0, Color32::from_rgb(80, 80, 80)));

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

    painter.rect_filled(rect, 4.0, Color32::from_rgb(40, 40, 40));

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
    ui.allocate_space(rect.size());

    let cx = rect.center().x;
    let cy = rect.center().y;

    let Some(texture) = &app.minimap_texture else {
        ui.ctx().request_repaint();
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
                ("Creating Map Cache", Some(format!("Processing: {}…", names)))
            }
            _ => ("Loading map…", None),
        };
        p.text(
            center + vec2(0.0, 12.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(13.0),
            Color32::GRAY,
        );
        if let Some(sub_text) = sub {
            p.text(
                center + vec2(0.0, 28.0),
                egui::Align2::CENTER_CENTER,
                sub_text,
                egui::FontId::proportional(11.0),
                Color32::from_gray(100),
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
    let yaw   = app.minimap_smoothed_yaw;

    // Metres visible from widget centre to nearest edge
    let zoom  = app.minimap_current_zoom.max(1.0);
    let scale = rect.width().min(rect.height()) / (2.0 * zoom);

    // World-space coverage always based on original (pre-quality-resize) image dims
    let [orig_w, orig_h] = app.minimap_orig_size;
    let map_world_w = orig_w as f32 / px_per_m;
    let map_world_h = orig_h as f32 / px_per_m;
    let corners_world: [(f32, f32); 4] = [
        (origin_wx,               origin_wz              ),  // TL → uv(0,0)
        (origin_wx + map_world_w, origin_wz              ),  // TR → uv(1,0)
        (origin_wx + map_world_w, origin_wz - map_world_h),  // BR → uv(1,1)
        (origin_wx,               origin_wz - map_world_h),  // BL → uv(0,1)
    ];

    // Rotate world displacement into car-relative screen space.
    // Assumes yaw=0 → car faces +Z (north); positive yaw clockwise viewed from above.
    let cos_yaw = yaw.cos();
    let sin_yaw = yaw.sin();

    let screen_corners: [Pos2; 4] = std::array::from_fn(|i| {
        let (wx, wz) = corners_world[i];
        let dx = wx - car_x;
        let dz = wz - car_z;
        let sx =  (dx * cos_yaw - dz * sin_yaw) * scale;
        let sy = -(dx * sin_yaw + dz * cos_yaw) * scale;
        pos2(cx + sx, cy + sy)
    });

    let uvs: [Pos2; 4] = [
        pos2(0.0, 0.0),
        pos2(1.0, 0.0),
        pos2(1.0, 1.0),
        pos2(0.0, 1.0),
    ];

    let mut mesh = egui::Mesh::with_texture(texture.id());
    mesh.indices = vec![0, 1, 2, 0, 2, 3];
    for i in 0..4 {
        mesh.vertices.push(egui::epaint::Vertex {
            pos:   screen_corners[i],
            uv:    uvs[i],
            color: Color32::WHITE,
        });
    }

    let painter = ui.painter_at(rect);
    painter.add(egui::Shape::Mesh(std::sync::Arc::new(mesh)));

    // Car indicator: triangle rotated to show actual car heading relative to map orientation
    let s = 7.0_f32;
    let arrow_angle = app.minimap_cached_raw_yaw - app.minimap_smoothed_yaw;
    let (sin_a, cos_a) = arrow_angle.sin_cos();
    let rot = |vx: f32, vy: f32| -> Pos2 {
        pos2(cx + vx * cos_a - vy * sin_a, cy + vx * sin_a + vy * cos_a)
    };
    let tip   = rot(0.0,      -s * 1.4);
    let left  = rot(-s,        s * 0.6);
    let right = rot( s,        s * 0.6);
    painter.add(egui::Shape::convex_polygon(
        vec![tip, right, left],
        Color32::WHITE,
        Stroke::new(1.5, Color32::BLACK),
    ));
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
                                .color(Color32::GRAY),
                        );
                    }
                }
            }
            None => {
                ui.label(RichText::new("--").color(Color32::GRAY));
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
