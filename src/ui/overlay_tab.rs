//! Overlay tab: the settings page for the in-game HUD overlay (D24–D28, the tab mockup).
//!
//! It only edits `app.config.overlay` (and the shared Hide HUD binding). `ForzaApp::sync_overlay`
//! and the listener's per-frame config push carry every change to the running HUD, so the page
//! needs no apply step.

use egui::{pos2, vec2, Color32, CursorIcon, FontId, Id, Painter, Rect, RichText, Sense, Stroke, Ui, Vec2};

use crate::app::{ForzaApp, OverlayStatus};
use crate::config::{ClusterStyle, DriftStyle, HotkeyAction, HudCell, MonitorMethod, OverlayConfig};
use crate::focus::MonitorStatus;
use crate::hud::layout::Module;
use crate::i18n::tr;
use crate::theme;

/// From this page width up the cards sit in three columns, below it in two (D28: three at the
/// 1280 px default window, two near the 800 px minimum). Why 1100: each of three columns is
/// then ≥ 355 px, the narrowest the two-half control rows still read well at.
const THREE_COLS_MIN_W: f32 = 1100.0;

type CardFn = fn(&mut Ui, &mut ForzaApp);

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    let cols: &[&[CardFn]] = if ui.available_width() >= THREE_COLS_MIN_W {
        &[&[general, monitor], &[layout, race, drift], &[cluster, minimap]]
    } else {
        // Layout first: it's the one card you can't find by scrolling past settings.
        &[&[layout, general, monitor], &[cluster, minimap, race, drift]]
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 8.0; // inter-column gap
        ui.columns(cols.len(), |uis| {
            for (ui, cards) in uis.iter_mut().zip(cols) {
                ui.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
                for card in *cards {
                    card(ui, app);
                }
            }
        });
    });
}

// ── Small shared pieces ──────────────────────────────────────────────────────

/// A small status line (a Test result). Never an option explanation: those are tooltips.
fn hint(ui: &mut Ui, text: &str) {
    hint_col(ui, text, Color32::GRAY);
}

fn hint_col(ui: &mut Ui, text: &str, col: Color32) {
    ui.label(RichText::new(text).size(11.0).color(col));
}

/// A coloured status dot + message; the message wraps (Disabled reasons are long).
fn status_line(ui: &mut Ui, col: Color32, msg: &str) {
    // Explicit left-to-right: inside the Detect button's right-to-left row a plain
    // `horizontal` would inherit that direction and put the dot after the text.
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
        ui.label(RichText::new("\u{25CF}").color(col));
        ui.add(egui::Label::new(RichText::new(msg).size(11.0)).wrap());
    });
}

/// Label in the left half, `right` in the right half (Setup's `control_row`).
fn control_row<R>(ui: &mut Ui, label: &str, right: impl FnOnce(&mut Ui) -> R) -> R {
    ui.columns(2, |c| {
        theme::row_label(&mut c[0], label);
        c[1].horizontal(right).inner
    })
}

/// [`control_row`] with a tooltip on the label (the explanation, instead of a helper line).
fn control_row_tip<R>(ui: &mut Ui, label: &str, tip: &str, right: impl FnOnce(&mut Ui) -> R) -> R {
    ui.columns(2, |c| {
        theme::row_label(&mut c[0], label).on_hover_text(tip);
        c[1].horizontal(right).inner
    })
}

/// A 0–1 fraction edited as a percentage slider row; `tip` is shown on hover.
fn pct_row(ui: &mut Ui, label: &str, v: &mut f32, lo: f32, hi: f32, step: f64, tip: Option<&str>) {
    let mut p = *v * 100.0;
    let resp = theme::slider_row(ui, label, &mut p, lo..=hi, step, 1, "%");
    if resp.changed() {
        *v = p / 100.0;
    }
    if let Some(t) = tip {
        resp.on_hover_text(t);
    }
}

/// A module card: greyed out while the overlay is off (the mockup's `card.off`), its body
/// greyed while the module's own "Enabled" is off. `tip` is the tooltip of its "Enabled" box.
fn module_card(
    ui: &mut Ui,
    app: &mut ForzaApp,
    title: &str,
    on: fn(&mut OverlayConfig) -> &mut bool,
    tip: Option<&str>,
    body: impl FnOnce(&mut Ui, &mut ForzaApp),
) {
    let overlay_on = app.config.overlay.enabled;
    ui.add_enabled_ui(overlay_on, |ui| {
        theme::card(ui, title, |ui| {
            let flag = on(&mut app.config.overlay);
            let resp = theme::checkbox_row(ui, flag, tr("Enabled"));
            if let Some(text) = tip {
                resp.on_hover_text(text);
            }
            let module_on = *flag;
            ui.add_enabled_ui(module_on, |ui| body(ui, app));
        });
    });
}

// ── General ─────────────────────────────────────────────────────────────────

fn general(ui: &mut Ui, app: &mut ForzaApp) {
    theme::card(ui, tr("General"), |ui| {
        // The overlay is Linux only (layer-shell or X11): greyed out elsewhere.
        ui.add_enabled_ui(cfg!(target_os = "linux"), |ui| {
            theme::checkbox_row(ui, &mut app.config.overlay.enabled, tr("Enable overlay"))
                .on_hover_text(tr("The HUD hides by itself while the game is paused."));
        });
        overlay_status_line(ui, app);
        hide_hud_row(ui, app);
        let o = &mut app.config.overlay;
        pct_row(ui, tr("Scale"), &mut o.scale, 50.0, 200.0, 5.0, None);
        pct_row(ui, tr("Plate opacity"), &mut o.plate_opacity, 0.0, 100.0, 1.0, None);
        theme::checkbox_row(ui, &mut o.fade, tr("Fade on show / hide"));
        theme::checkbox_row(ui, &mut o.focus_only, tr("Only when game window is focused"))
            .on_hover_text(tr("Hides the HUD while another window is focused. Uses the Window Detection method set in Setup."));
    });
}

/// The overlay runtime's state. Not in the mockup; it sits under the enable toggle because
/// it is that toggle's outcome (e.g. "needs Wayland").
fn overlay_status_line(ui: &mut Ui, app: &ForzaApp) {
    if !cfg!(target_os = "linux") {
        status_line(ui, theme::FAINT, tr("The in-game overlay is available on Linux only (Wayland or X11)."));
        return;
    }
    let (col, msg) = match app.overlay_status() {
        OverlayStatus::Off => (theme::FAINT, tr("Overlay off").to_string()),
        OverlayStatus::Starting => (theme::FAINT, tr("Overlay starting…").to_string()),
        OverlayStatus::Running => (theme::GOOD, tr("Overlay running").to_string()),
        OverlayStatus::Disabled(reason) => (theme::DANGER, reason.to_string()),
        OverlayStatus::Stopped => (
            theme::DANGER,
            tr("The overlay stopped. Turn it off and on again to retry.").to_string(),
        ),
    };
    status_line(ui, col, &msg);
}

/// The shared Hide HUD binding (D16/D28): the same `hotkeys.bindings` entry Setup → Hotkey
/// edits. Clicking arms `app.rebinding`; `ForzaApp::capture_rebind` takes the next key.
fn hide_hud_row(ui: &mut Ui, app: &mut ForzaApp) {
    let action = HotkeyAction::HideHud;
    let capturing = app.rebinding == Some(action);
    let binding = app.config.hotkeys.bindings.get(&action).copied();
    let text = match (capturing, binding) {
        (true, _) => RichText::new(tr("Press a key…")),
        (false, Some(b)) => RichText::new(b.label()),
        (false, None) => RichText::new(tr("Not set")).color(theme::FAINT),
    };
    let resp = control_row(ui, tr(action.label()), |ui| {
        let mut btn = egui::Button::new(text);
        if capturing {
            btn = btn.stroke(Stroke::new(1.0, theme::ACCENT));
        }
        ui.add_sized([ui.available_width(), ui.spacing().interact_size.y], btn)
            .on_hover_text(tr("Esc cancels. Backspace clears the binding."))
    });
    if resp.clicked() {
        app.rebinding = if capturing { None } else { Some(action) };
    }
    app.track_rebind_button(action, &resp);
    if let (false, Some(b)) = (capturing, binding) {
        let clash = HotkeyAction::ALL
            .iter()
            .find(|&&a| a != action && app.config.hotkeys.bindings.get(&a) == Some(&b));
        if let Some(other) = clash {
            let msg = format!("{} {} ({})", tr("Also bound to"), tr(other.label()), tr("Setup → Hotkey"));
            hint_col(ui, &msg, theme::DANGER);
        }
    }
    if app.hud_hidden && app.config.overlay.enabled {
        hint_col(ui, tr("The HUD is hidden. Press the Hide HUD key again to show it."), theme::WARN);
    }
}

// ── Monitor Detection ───────────────────────────────────────────────────────

fn method_label(m: MonitorMethod) -> &'static str {
    tr(match m {
        MonitorMethod::Hyprland => "Hyprland (built in)",
        MonitorMethod::Custom => "Custom command",
        MonitorMethod::Fixed => "Fixed monitor",
    })
}

const READ_WHILE_FOCUSED: &str = "Read only while Forza is the active window. Otherwise the HUD stays where it was.";

fn monitor(ui: &mut Ui, app: &mut ForzaApp) {
    // Last Test / Detect result, kept in egui memory (UI-only, not config).
    let test_id = Id::new("overlay_monitor_test");
    let mut test: Option<Result<String, String>> = ui.data(|d| d.get_temp(test_id)).flatten();
    theme::card(ui, tr("Monitor Detection"), |ui| {
        let o = &mut app.config.overlay;
        let before = o.monitor_method;
        let method_tip = match o.monitor_method {
            MonitorMethod::Hyprland => {
                format!("{}\n{}", tr("Runs hyprctl activeworkspace and reads the monitor it names."), tr(READ_WHILE_FOCUSED))
            }
            MonitorMethod::Custom => tr(READ_WHILE_FOCUSED).to_string(),
            MonitorMethod::Fixed => String::new(),
        };
        control_row_tip(ui, tr("Method"), &method_tip, |ui| {
            egui::ComboBox::from_id_salt("overlay_monitor_method")
                .selected_text(method_label(o.monitor_method))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for m in [MonitorMethod::Hyprland, MonitorMethod::Custom, MonitorMethod::Fixed] {
                        ui.selectable_value(&mut o.monitor_method, m, method_label(m));
                    }
                });
        });
        if o.monitor_method != before {
            test = None; // a result from the old method would mislead
        }
        match o.monitor_method {
            MonitorMethod::Hyprland => {}
            MonitorMethod::Custom => {
                control_row_tip(ui, tr("Command"), tr("Must print one monitor name, e.g. DP-1."), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tr("Test")).clicked() {
                            test = Some(crate::focus::query_monitor(o.monitor_method, &o.monitor_cmd));
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut o.monitor_cmd)
                                .hint_text(theme::placeholder(tr("prints a monitor name")))
                                .desired_width(ui.available_width()),
                        );
                    });
                });
            }
            MonitorMethod::Fixed => {
                control_row_tip(ui, tr("Monitor"), tr("The output name, e.g. DP-1. Empty = the first monitor."), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Fills the field with the monitor Hyprland reports as focused: the one
                        // this window is on when you click.
                        if ui.button(tr("Detect")).clicked() {
                            match crate::focus::query_monitor(MonitorMethod::Hyprland, "") {
                                Ok(name) => o.monitor_fixed = name,
                                Err(e) => test = Some(Err(e)),
                            }
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut o.monitor_fixed)
                                .hint_text(theme::placeholder(tr("first monitor")))
                                .desired_width(ui.available_width()),
                        );
                    });
                });
            }
        }
        match &test {
            Some(Ok(name)) => hint(ui, &format!("\u{2192} {name}")),
            Some(Err(e)) => hint_col(ui, &format!("\u{2192} {}: {e}", tr("error")), theme::DANGER),
            None => {}
        }
        monitor_status_line(ui, app, &mut test);
    });
    ui.data_mut(|d| d.insert_temp(test_id, test));
}

/// The detection status dot (D28): green detected, amber (`WARN`) waiting for the game
/// window, red failed. Hyprland/Custom get a Detect button that runs the query right now.
fn monitor_status_line(ui: &mut Ui, app: &ForzaApp, test: &mut Option<Result<String, String>>) {
    let o = &app.config.overlay;
    let out = app.focus.monitor_output().unwrap_or_else(|| tr("the first monitor").to_string());
    let (col, msg) = if !cfg!(target_os = "linux") {
        (theme::FAINT, tr("Monitor detection runs on Linux only.").to_string())
    } else if !o.enabled {
        (theme::FAINT, tr("Detection is off while the overlay is disabled.").to_string())
    } else {
        match app.focus.monitor_status() {
            MonitorStatus::Idle => (theme::FAINT, tr("Detecting…").to_string()),
            MonitorStatus::Ok if o.monitor_method == MonitorMethod::Fixed => {
                (theme::GOOD, format!("{} {out}", tr("HUD pinned to")))
            }
            MonitorStatus::Ok => (theme::GOOD, format!("{} {out}", tr("Game on"))),
            MonitorStatus::WaitingForGame => {
                (theme::WARN, format!("{} {out}", tr("Game window not focused, keeping")))
            }
            MonitorStatus::Failed => {
                let e = app.focus.monitor_error().unwrap_or_default();
                (theme::DANGER, format!("{}: {e}", tr("Monitor detection failed")))
            }
        }
    };
    ui.horizontal(|ui| {
        if o.monitor_method != MonitorMethod::Fixed {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Detect")).clicked() {
                    *test = Some(crate::focus::query_monitor(o.monitor_method, &o.monitor_cmd));
                }
                status_line(ui, col, &msg);
            });
        } else {
            status_line(ui, col, &msg);
        }
    });
}

// ── Layout: the shared 3×3 grid (D19, D22, D26) ─────────────────────────────

/// The three placeable chips, in D22 stacking order (Map → cluster → race/drift).
const MODULES: [Module; 3] = [Module::Map, Module::Cluster, Module::Race];

fn module_name(m: Module) -> &'static str {
    tr(match m {
        Module::Map => "Minimap",
        Module::Cluster => "Drive cluster",
        Module::Race => "Race / Drift",
    })
}

fn module_cell(o: &OverlayConfig, m: Module) -> HudCell {
    match m {
        Module::Map => o.minimap_cell,
        Module::Cluster => o.cluster_cell,
        Module::Race => o.race_cell,
    }
}

fn module_cell_mut(o: &mut OverlayConfig, m: Module) -> &mut HudCell {
    match m {
        Module::Map => &mut o.minimap_cell,
        Module::Cluster => &mut o.cluster_cell,
        Module::Race => &mut o.race_cell,
    }
}

/// Off modules stay on the grid, greyed, and can still be moved (mockup).
fn module_on(o: &OverlayConfig, m: Module) -> bool {
    match m {
        Module::Map => o.minimap_on,
        Module::Cluster => o.cluster_on,
        Module::Race => o.race_on || o.drift_on,
    }
}

/// Keyboard fallback: one cell over in the arrow's direction, clamped at the grid edge.
fn step(cell: HudCell, drow: i32, dcol: i32) -> HudCell {
    let r = (cell.row() as i32 + drow).clamp(0, 2) as usize;
    let c = (cell.col() as i32 + dcol).clamp(0, 2) as usize;
    HudCell::from_row_col(r, c).unwrap_or(cell)
}

const CHIP_H: f32 = 17.0;
const CELL_PAD: f32 = 4.0;
const CHIP_GAP: f32 = 3.0;
const GRID_GAP: f32 = 4.0;

/// Chip rects inside one grid cell, placed by the HUD's own stacking code
/// (`hud::layout::layout`) so the grid shows exactly what the overlay will do. Scaled so the
/// HUD's 12 px gap becomes [`CHIP_GAP`]; the surface is grown by `trim` on every side so its
/// (scaled) 44 px edge margin lands at [`CELL_PAD`] inside the cell. Always the default
/// margin/gap, not the user's: the grid shows which cell and stacking order, and a 0 gap
/// would divide by zero here.
fn chip_rects(cell_rect: Rect, cell: HudCell, chips: &[(Module, Vec2)]) -> Vec<Rect> {
    use crate::hud::layout::{layout, GAP, MARGIN};
    let s = CHIP_GAP / GAP;
    let trim = MARGIN * s - CELL_PAD;
    let screen = cell_rect.size() + Vec2::splat(2.0 * trim);
    let items: Vec<_> = chips.iter().map(|&(m, size)| (m, cell, size / s)).collect();
    let shift = cell_rect.min.to_vec2() - Vec2::splat(trim);
    layout(screen, s, MARGIN, GAP, &items).into_iter().map(|r| r.translate(shift)).collect()
}

fn dashed_rect(p: &Painter, r: Rect, stroke: Stroke) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    p.extend(egui::Shape::dashed_line(&pts, stroke, 3.0, 2.0));
}

fn chip_font() -> FontId {
    FontId::monospace(11.0)
}

/// Chip: 6×10 grip, module name. `lit` = selected / drag ghost / hovered border.
fn paint_chip(p: &Painter, r: Rect, m: Module, on: bool, selected: bool, hovered: bool) {
    let round = 5.0;
    if on {
        let fill = if selected { theme::SEL } else { theme::BTN };
        let bd = if selected || hovered { theme::ACCENT } else { theme::BTNBD };
        p.rect(r, round, fill, Stroke::new(1.0, bd), egui::StrokeKind::Inside);
    } else {
        let bd = if selected || hovered { theme::ACCENT } else { theme::FAINT };
        if selected {
            p.rect_filled(r, round, theme::SEL);
        }
        dashed_rect(p, r.shrink(0.5), Stroke::new(1.0, bd));
    }
    let clip = p.with_clip_rect(r.shrink(1.0));
    let grip = pos2(r.left() + 4.0, r.center().y - 5.0);
    for (dx, dy) in [(1.2, 1.5), (4.8, 1.5), (1.2, 5.0), (4.8, 5.0), (1.2, 8.5), (4.8, 8.5)] {
        clip.circle_filled(grip + vec2(dx, dy), 1.0, theme::FAINT);
    }
    let text_col = if on { theme::TEXT } else { theme::FAINT };
    clip.text(
        pos2(r.left() + 15.0, r.center().y),
        egui::Align2::LEFT_CENTER,
        module_name(m),
        chip_font(),
        text_col,
    );
}

/// egui temp-data key of the layout grid's selected chip.
const LAYOUT_SEL_KEY: &str = "overlay_layout_sel";

/// Drop the layout grid's chip selection (on a tab switch, so stray arrow keys can't move a
/// chip after you come back).
pub fn clear_layout_selection(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<Option<Module>>(Id::new(LAYOUT_SEL_KEY)));
}

fn layout(ui: &mut Ui, app: &mut ForzaApp) {
    theme::card(ui, tr("Layout"), |ui| {
        let o = &mut app.config.overlay;
        let sel_id = Id::new(LAYOUT_SEL_KEY);
        let mut sel: Option<Module> = ui.data(|d| d.get_temp(sel_id)).flatten();
        layout_grid(ui, o, &mut sel);
        let px_tip = tr("In pixels at 1080p. Both scale with the resolution and the HUD scale.");
        theme::slider_row(ui, tr("Edge margin"), &mut o.margin_px, 0.0..=200.0, 1.0, 0, " px").on_hover_text(px_tip);
        theme::slider_row(ui, tr("Module spacing"), &mut o.gap_px, 0.0..=60.0, 1.0, 0, " px").on_hover_text(px_tip);
        if ui.add(theme::secondary_button(tr("Reset layout"))).clicked() {
            o.reset_layout();
            sel = None;
        }
        ui.data_mut(|d| d.insert_temp(sel_id, sel));
    });
}

/// The drag-and-drop grid. Chips: drag onto a cell, or select (click / Tab + Space) and then
/// click a cell or press the arrow keys; Esc drops the selection.
fn layout_grid(ui: &mut Ui, o: &mut OverlayConfig, sel: &mut Option<Module>) {
    let chip_id = |m: Module| Id::new(("overlay_chip", m as usize));
    let w = ui.available_width();
    let (grid, _) = ui.allocate_exact_size(vec2(w, w * 9.0 / 16.0), Sense::hover());
    let inner = grid.shrink(GRID_GAP);
    let cell_size = (inner.size() - Vec2::splat(2.0 * GRID_GAP)) / 3.0;
    let cell_rect = |c: HudCell| {
        let at = vec2(c.col() as f32, c.row() as f32) * (cell_size + Vec2::splat(GRID_GAP));
        Rect::from_min_size(inner.min + at, cell_size)
    };
    let pointer = ui.ctx().pointer_interact_pos();
    let cell_at = |pos: egui::Pos2| HudCell::ALL.into_iter().find(|&c| cell_rect(c).contains(pos));
    let dragged = MODULES.into_iter().find(|&m| ui.ctx().is_being_dragged(chip_id(m)));
    let mut move_to: Option<(Module, HudCell)> = None;

    // A press outside the grid drops the selection, so arrow keys meant for something else
    // on the page can't move a chip.
    let pressed_outside = ui.input(|i| {
        i.pointer.primary_pressed() && i.pointer.interact_pos().is_some_and(|p| !grid.contains(p))
    });
    if pressed_outside {
        *sel = None;
    }

    ui.painter().rect(grid, 6.0, theme::FIELD, Stroke::new(1.0, theme::BTNBD), egui::StrokeKind::Inside);

    // Cells first, so the chips drawn after them sit on top for hover and clicks.
    for cell in HudCell::ALL {
        let r = cell_rect(cell);
        let resp = ui
            .interact(r, Id::new(("overlay_cell", cell as usize)), Sense::click())
            .on_hover_text(format!(
                "{}\n{}",
                tr("Drag a module onto a cell. Or select one, then click a cell or use the arrow keys."),
                tr("Modules in one cell stack from the screen edge inward: Minimap, then Drive cluster, then Race / Drift."),
            ));
        if let (true, Some(m)) = (resp.clicked(), *sel) {
            move_to = Some((m, cell));
            *sel = None; // placed: the mockup drops the selection
        }
        let target = (dragged.is_some() && pointer.is_some_and(|p| r.contains(p)))
            || (sel.is_some() && resp.hovered());
        if target {
            ui.painter().rect(r, 4.0, theme::SEL, Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
        } else {
            dashed_rect(ui.painter(), r.shrink(0.5), Stroke::new(1.0, theme::BORDER));
        }
        if sel.is_some() {
            resp.on_hover_cursor(CursorIcon::PointingHand);
        }
    }

    for cell in HudCell::ALL {
        let r = cell_rect(cell);
        let chips: Vec<(Module, Vec2)> = MODULES
            .into_iter()
            .filter(|&m| module_cell(o, m) == cell)
            .map(|m| {
                let text_w = ui
                    .painter()
                    .layout_no_wrap(module_name(m).to_owned(), chip_font(), theme::TEXT)
                    .size()
                    .x;
                (m, vec2((text_w + 22.0).min(r.width() - 2.0 * CELL_PAD), CHIP_H))
            })
            .collect();
        for (&(m, _), cr) in chips.iter().zip(chip_rects(r, cell, &chips)) {
            let id = chip_id(m);
            let resp = ui
                .interact(cr, id, Sense::click_and_drag())
                .on_hover_text(module_name(m))
                .on_hover_cursor(CursorIcon::Grab);
            if resp.clicked() {
                *sel = if *sel == Some(m) { None } else { Some(m) };
                resp.request_focus();
            }
            if resp.has_focus() {
                // Arrow keys move the chip instead of egui's focus.
                ui.memory_mut(|mem| {
                    mem.set_focus_lock_filter(
                        id,
                        egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() },
                    )
                });
            }
            if resp.drag_stopped() {
                if let Some(c) = pointer.and_then(cell_at) {
                    move_to = Some((m, c));
                    *sel = None;
                }
            }
            let on = module_on(o, m);
            let selected = *sel == Some(m) || resp.has_focus();
            if resp.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                let mut faded = ui.painter().clone();
                faded.multiply_opacity(0.25);
                paint_chip(&faded, cr, m, on, false, false);
                // The ghost follows the pointer, held where it was grabbed.
                if let (Some(p), Some(origin)) = (pointer, ui.input(|i| i.pointer.press_origin())) {
                    let ghost = cr.translate(p - origin);
                    let layer = egui::LayerId::new(egui::Order::Tooltip, id.with("ghost"));
                    paint_chip(&ui.ctx().layer_painter(layer), ghost, m, on, true, true);
                }
            } else {
                paint_chip(ui.painter(), cr, m, on, selected, resp.hovered());
            }
        }
    }

    // Arrow keys: the focused chip, or the selected one while nothing else has focus.
    let focused = MODULES.into_iter().find(|&m| ui.memory(|mem| mem.has_focus(chip_id(m))));
    let nothing_focused = ui.memory(|mem| mem.focused().is_none());
    if let Some(m) = focused.or(sel.filter(|_| nothing_focused)) {
        let d = ui.input_mut(|i| {
            use egui::{Key, Modifiers};
            [(Key::ArrowUp, -1, 0), (Key::ArrowDown, 1, 0), (Key::ArrowLeft, 0, -1), (Key::ArrowRight, 0, 1)]
                .into_iter()
                .find(|&(k, _, _)| i.consume_key(Modifiers::NONE, k))
        });
        if let Some((_, dr, dc)) = d {
            move_to = Some((m, step(module_cell(o, m), dr, dc)));
            *sel = Some(m);
        }
    }
    if sel.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        *sel = None;
    }
    if let Some((m, c)) = move_to {
        *module_cell_mut(o, m) = c;
    }
}

// ── Module cards ─────────────────────────────────────────────────────────────

fn cluster(ui: &mut Ui, app: &mut ForzaApp) {
    module_card(ui, app, tr("Drive Cluster"), |o| &mut o.cluster_on, None, |ui, app| {
        let use_mph = app.config.use_mph;
        let o = &mut app.config.overlay;
        control_row(ui, tr("Style"), |ui| {
            let label = |s: ClusterStyle| match s {
                ClusterStyle::Pill => tr("Pill"),
                ClusterStyle::Halo => tr("Halo"),
            };
            egui::ComboBox::from_id_salt("overlay_cluster_style")
                .selected_text(label(o.cluster_style))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for s in [ClusterStyle::Pill, ClusterStyle::Halo] {
                        ui.selectable_value(&mut o.cluster_style, s, label(s));
                    }
                });
        });
        let rpm_label = if use_mph {
            tr("Show engine RPM instead of MPH label")
        } else {
            tr("Show engine RPM instead of KM/H label")
        };
        theme::checkbox_row(ui, &mut o.rpm_label, rpm_label).on_hover_text(tr("The speed stays. Only the unit text changes."));
        theme::checkbox_row(ui, &mut o.speed_hold, tr("Update speed only every 0.5 s"))
            .on_hover_text(tr("Calmer to read. Gear and revs stay live."));
        theme::checkbox_row(ui, &mut o.shift_flash, tr("Shift flash"));
        theme::checkbox_row(ui, &mut o.gear_pulse, tr("Gear-change pulse"));
        let redline_tip = tr("The shift cue is the gearbox's own shift point (Gearbox → Shift RPM), taken from the max rpm the gearbox calibrates for each car. This works with the automatic gearbox off too. To calibrate again, use the \"Reset RPM Calibration\" hotkey (Setup → Hotkey) or Gearbox → \"Clear RPM calibration\".");
        pct_row(ui, tr("Redline at (max rpm)"), &mut o.redline_frac, 50.0, 100.0, 0.5, Some(redline_tip));
        pct_row(
            ui,
            tr("Shift cue before calibration"),
            &mut o.shift_frac,
            50.0,
            100.0,
            0.5,
            Some(tr("Until the first full pull and manual upshift in a car, both use the game's max rpm and this fallback.")),
        );
    });
}

fn minimap(ui: &mut Ui, app: &mut ForzaApp) {
    module_card(ui, app, tr("Minimap"), |o| &mut o.minimap_on, None, |ui, app| {
        let o = &mut app.config.overlay;
        theme::checkbox_row(ui, &mut o.compass, tr("Compass"));
        // 5000 m, not the mockup's 2000: the defaults (3000 / 1500 m) must fit the range.
        theme::slider_row(ui, tr("Zoom when stopped"), &mut o.zoom_stopped_m, 100.0..=5000.0, 50.0, 0, " m");
        theme::slider_row(ui, tr("Zoom when driving"), &mut o.zoom_driving_m, 100.0..=5000.0, 50.0, 0, " m");
        theme::checkbox_row(ui, &mut o.coop_teammates, tr("Show co-op teammates"));
    });
}

fn race(ui: &mut Ui, app: &mut ForzaApp) {
    let tip = tr("Swaps to the drift counter by itself when drifting is detected. Placed as Race / Drift in Layout.");
    module_card(ui, app, tr("Race Block"), |o| &mut o.race_on, Some(tip), |ui, app| {
        let o = &mut app.config.overlay;
        theme::checkbox_row(ui, &mut o.lap_delta, tr("Lap delta chip"));
        theme::checkbox_row(ui, &mut o.place_colour, tr("Place-change colour"))
            .on_hover_text(tr("Green fade when you gain a place, red when you lose one."));
    });
}

fn drift(ui: &mut Ui, app: &mut ForzaApp) {
    let tip = tr("Replaces the race block automatically while you drift, in the same spot.");
    module_card(ui, app, tr("Drift Counter"), |o| &mut o.drift_on, Some(tip), |ui, app| {
        let o = &mut app.config.overlay;
        let style_tip = tr("Position + Gain shows your place and the points of the last interval, counting up. Total shows the event score, which Forza also shows itself.");
        control_row_tip(ui, tr("Style"), style_tip, |ui| {
            theme::styled_radio(ui, &mut o.drift_style, DriftStyle::PositionGain, tr("Position + Gain"));
            theme::styled_radio(ui, &mut o.drift_style, DriftStyle::Total, tr("Total score"));
        });
        theme::slider_row(ui, tr("Gain chip interval"), &mut o.drift_chip_secs, 1.0..=10.0, 1.0, 0, " s");
        let bar = format!("{} ({:.0} s)", tr("Progress bar"), o.drift_chip_secs);
        theme::checkbox_row(ui, &mut o.drift_bar, bar);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_steps_clamp_at_the_grid_edge() {
        assert_eq!(step(HudCell::Center, -1, 0), HudCell::TopCenter);
        assert_eq!(step(HudCell::Center, 0, 1), HudCell::MiddleRight);
        assert_eq!(step(HudCell::TopLeft, -1, 0), HudCell::TopLeft);
        assert_eq!(step(HudCell::TopLeft, 0, -1), HudCell::TopLeft);
        assert_eq!(step(HudCell::BottomRight, 1, 0), HudCell::BottomRight);
        assert_eq!(step(HudCell::BottomRight, 0, 1), HudCell::BottomRight);
        assert_eq!(step(HudCell::BottomLeft, -1, 1), HudCell::Center);
    }

    const CELL: Rect = Rect { min: egui::Pos2 { x: 100.0, y: 50.0 }, max: egui::Pos2 { x: 230.0, y: 122.0 } };

    fn three() -> Vec<(Module, Vec2)> {
        vec![(Module::Map, vec2(60.0, CHIP_H)), (Module::Cluster, vec2(90.0, CHIP_H)), (Module::Race, vec2(80.0, CHIP_H))]
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn bottom_row_stacks_upward_map_lowest() {
        let r = chip_rects(CELL, HudCell::BottomCenter, &three());
        // Map sits on the bottom padding, cluster above it, race/drift on top.
        assert!(approx(r[0].bottom(), CELL.bottom() - CELL_PAD));
        assert!(approx(r[1].bottom(), r[0].top() - CHIP_GAP));
        assert!(approx(r[2].bottom(), r[1].top() - CHIP_GAP));
        // Centre column: horizontally centred.
        for x in &r {
            assert!(approx(x.center().x, CELL.center().x));
        }
    }

    #[test]
    fn top_row_stacks_downward_map_highest() {
        let r = chip_rects(CELL, HudCell::TopLeft, &three());
        assert!(approx(r[0].top(), CELL.top() + CELL_PAD));
        assert!(approx(r[1].top(), r[0].bottom() + CHIP_GAP));
        assert!(approx(r[2].top(), r[1].bottom() + CHIP_GAP));
        for x in &r {
            assert!(approx(x.left(), CELL.left() + CELL_PAD));
        }
    }

    #[test]
    fn middle_row_centres_the_stack_map_lowest() {
        let r = chip_rects(CELL, HudCell::MiddleRight, &three());
        let top = r[2].top();
        let bottom = r[0].bottom();
        assert!(approx((top + bottom) / 2.0, CELL.center().y));
        assert!(r[0].top() > r[1].top() && r[1].top() > r[2].top());
        for x in &r {
            assert!(approx(x.right(), CELL.right() - CELL_PAD));
        }
    }

    #[test]
    fn modules_map_to_their_config_cells() {
        let mut o = OverlayConfig::default();
        *module_cell_mut(&mut o, Module::Race) = HudCell::Center;
        assert_eq!(o.race_cell, HudCell::Center);
        assert_eq!(module_cell(&o, Module::Map), OverlayConfig::DEFAULT_MINIMAP_CELL);
        o.race_on = false;
        assert!(module_on(&o, Module::Race), "drift still on keeps the Race / Drift chip lit");
        o.drift_on = false;
        assert!(!module_on(&o, Module::Race));
    }
}
