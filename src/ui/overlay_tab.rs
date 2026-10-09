//! Overlay tab: the settings page for the in-game HUD overlay (D24–D28, the tab mockup).
//!
//! A module selector (`theme::segmented`, the Co-Op tab's control) at the top picks which
//! module's cards the page shows: General, Minimap, Drive cluster, Race / Drift, Notifications.
//! The selection is `config.overlay_page` (remembered, never exported). The Minimap page is the
//! module's *frame* only (shape, size, outline, background; D75); the map layers and view options
//! live on the Map tab (the Minimap and Dashboard map pages moved there in D67, `map_tab`).
//!
//! It edits `app.config.overlay` (and the shared Hide HUD binding). `ForzaApp::sync_overlay` and the listener's per-frame config push carry every
//! change to the running HUD, so the page needs no apply step.


use egui::{pos2, vec2, Color32, CursorIcon, FontId, Id, Painter, Rect, RichText, Sense, Stroke, Ui, Vec2};

use crate::app::{ForzaApp, OverlayStatus};
use crate::config::{ClusterStyle, DriftStyle, HotkeyAction, HudCell, MapShape, MonitorMethod, OverlayConfig, OverlayPage};
use crate::focus::MonitorStatus;
use crate::hud::layout::Module;
use crate::i18n::tr;
use crate::theme;

/// From this page width up the cards sit in three columns, below it in two (D28: three at the
/// 1280 px default window, two near the 800 px minimum). Why 1100: each of three columns is
/// then ≥ 355 px, the narrowest the two-half control rows still read well at.
pub(crate) const THREE_COLS_MIN_W: f32 = 1100.0;

type CardFn = fn(&mut Ui, &mut ForzaApp);

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    if page_selector(ui, &mut app.config.overlay_page) {
        clear_layout_selection(ui.ctx()); // a half-made chip selection must not outlive its tab
    }
    ui.add_space(8.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match app.config.overlay_page {
        OverlayPage::General => {
            // Layout first when narrow: it's the one card you can't find by scrolling past settings.
            cards_page(ui, app, &[&[general], &[monitor], &[layout]], &[&[layout], &[general, monitor]]);
        }
        OverlayPage::Minimap => cards_page(ui, app, &[&[minimap], &[], &[]], &[&[minimap], &[]]),
        OverlayPage::Cluster => cards_page(ui, app, &[&[cluster], &[], &[]], &[&[cluster], &[]]),
        OverlayPage::Race => cards_page(ui, app, &[&[race], &[drift], &[]], &[&[race], &[drift]]),
        OverlayPage::Notifications => cards_page(ui, app, &[&[notifications], &[], &[]], &[&[notifications], &[]]),
    });
}

/// The module selector. Fits in one row where the labels do; otherwise (a narrow window, a long
/// German label) it breaks into two rows, so a label never spills into its neighbour.
/// Returns true when the selection changed.
fn page_selector(ui: &mut Ui, page: &mut OverlayPage) -> bool {
    let opts = [
        (OverlayPage::General, tr("General")),
        (OverlayPage::Minimap, tr("Minimap")),
        (OverlayPage::Cluster, tr("Drive cluster")),
        (OverlayPage::Race, tr("Race / Drift")),
        (OverlayPage::Notifications, tr("Notifications")),
    ];
    page_selector_with(ui, page, &opts)
}

/// [`page_selector`] for any page enum (the Map tab's settings use it too): `opts` in one row
/// where the labels fit, else in two halves.
pub(crate) fn page_selector_with<T: PartialEq + Copy>(ui: &mut Ui, page: &mut T, opts: &[(T, &str)]) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(theme::WELL)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            // 6 px = the control's own padding; 24 px of air around the longest label.
            let widest = opts
                .iter()
                .map(|(_, l)| ui.painter().layout_no_wrap(l.to_string(), FontId::proportional(14.0), theme::TEXT).size().x)
                .fold(0.0_f32, f32::max);
            let one_row = (ui.available_width() - 6.0) / opts.len() as f32 >= widest + 24.0;
            let half = opts.len().div_ceil(2);
            let rows: &[&[(T, &str)]] = if one_row { &[opts] } else { &[&opts[..half], &opts[half..]] };
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    ui.add_space(4.0);
                }
                // An id scope per row: `segmented` derives its click ids from the Ui's id.
                changed |= ui.push_id(("overlay_page_row", i), |ui| theme::segmented(ui, page, row)).inner;
            }
        });
    changed
}

/// A page of plain cards: three columns from [`THREE_COLS_MIN_W`] up, otherwise two.
fn cards_page(ui: &mut Ui, app: &mut ForzaApp, three: &[&[CardFn]], two: &[&[CardFn]]) {
    let cols = if ui.available_width() >= THREE_COLS_MIN_W { three } else { two };
    ui.spacing_mut().item_spacing.x = 8.0; // inter-column gap
    theme::columns(ui, cols.len(), |uis| {
        for (ui, cards) in uis.iter_mut().zip(cols) {
            ui.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
            for card in *cards {
                card(ui, app);
            }
        }
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
pub(crate) fn status_line(ui: &mut Ui, col: Color32, msg: &str) {
    // Explicit left-to-right: inside the Detect button's right-to-left row a plain
    // `horizontal` would inherit that direction and put the dot after the text.
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
        ui.label(RichText::new("\u{25CF}").color(col));
        ui.add(egui::Label::new(RichText::new(msg).size(11.0)).wrap());
    });
}

/// Label in the left half, `right` in the right half (Setup's `control_row`).
pub(crate) fn control_row<R>(ui: &mut Ui, label: &str, right: impl FnOnce(&mut Ui) -> R) -> R {
    crate::theme::columns(ui, 2, |c| {
        theme::row_label(&mut c[0], label);
        c[1].horizontal(right).inner
    })
}

/// [`control_row`] with a tooltip on the label (the explanation, instead of a helper line).
pub(crate) fn control_row_tip<R>(ui: &mut Ui, label: &str, tip: &str, right: impl FnOnce(&mut Ui) -> R) -> R {
    crate::theme::columns(ui, 2, |c| {
        theme::row_label(&mut c[0], label).on_hover_text(tip);
        c[1].horizontal(right).inner
    })
}

/// A 0–1 fraction edited as a percentage slider row; `tip` is shown on hover.
pub(crate) fn pct_row(ui: &mut Ui, label: &str, v: &mut f32, lo: f32, hi: f32, step: f64, tip: Option<&str>) {
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
pub(crate) fn module_card(
    ui: &mut Ui,
    o: &mut OverlayConfig,
    title: &str,
    on: fn(&mut OverlayConfig) -> &mut bool,
    tip: Option<&str>,
    body: impl FnOnce(&mut Ui, &mut OverlayConfig),
) {
    let overlay_on = o.enabled;
    ui.add_enabled_ui(overlay_on, |ui| {
        theme::card(ui, title, |ui| {
            let flag = on(o);
            let resp = theme::checkbox_row(ui, flag, tr("Enabled"));
            if let Some(text) = tip {
                resp.on_hover_text(text);
            }
            let module_on = *flag;
            ui.add_enabled_ui(module_on, |ui| body(ui, o));
        });
    });
}

// ── General ─────────────────────────────────────────────────────────────────

/// The OS has an overlay backend (Linux layer-shell/X11, Windows layered window).
const OVERLAY_OS: bool = cfg!(any(target_os = "linux", target_os = "windows"));

fn general(ui: &mut Ui, app: &mut ForzaApp) {
    theme::card(ui, tr("General"), |ui| {
        // The overlay exists on Linux (layer-shell or X11) and Windows: greyed out elsewhere.
        ui.add_enabled_ui(OVERLAY_OS, |ui| {
            let tip = if cfg!(target_os = "windows") {
                format!(
                    "{}\n{}",
                    tr("The HUD hides by itself while the game is paused."),
                    tr("Windows (experimental): Borderless or Windowed only — exclusive fullscreen can't be overlaid."),
                )
            } else {
                tr("The HUD hides by itself while the game is paused.").to_string()
            };
            theme::checkbox_row(ui, &mut app.config.overlay.enabled, tr("Enable overlay")).on_hover_text(tip);
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
    if !OVERLAY_OS {
        status_line(ui, theme::FAINT, tr("The in-game overlay needs Linux (Wayland or X11) or Windows."));
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
            .on_hover_text(tr("Esc cancels. Backspace or Delete clears the binding."))
    });
    if resp.clicked() {
        app.rebinding = if capturing { None } else { Some(action) };
    }
    app.track_rebind_button(action, &resp);
    if app.hud_hidden && app.config.overlay.enabled {
        hint_col(ui, tr("The HUD is hidden. Press the Hide HUD key again to show it."), theme::WARN);
    }
}

// ── Monitor Detection ───────────────────────────────────────────────────────

fn method_label(m: MonitorMethod) -> &'static str {
    tr(match m {
        // `Hyprland` is the "built-in" detector: hyprctl on Linux, the foreground window's
        // monitor on Windows.
        MonitorMethod::Hyprland if cfg!(target_os = "windows") => "Active window (built in)",
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
        if cfg!(target_os = "windows") && o.monitor_method == MonitorMethod::Custom {
            o.monitor_method = MonitorMethod::Hyprland; // no shell commands on Windows
        }
        let before = o.monitor_method;
        let method_tip = match o.monitor_method {
            MonitorMethod::Hyprland if cfg!(target_os = "windows") => {
                format!("{}\n{}", tr("Uses the monitor the focused window is on."), tr(READ_WHILE_FOCUSED))
            }
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
                    let methods: &[MonitorMethod] = if cfg!(target_os = "windows") {
                        &[MonitorMethod::Hyprland, MonitorMethod::Fixed]
                    } else {
                        &[MonitorMethod::Hyprland, MonitorMethod::Custom, MonitorMethod::Fixed]
                    };
                    for &m in methods {
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
                let fixed_tip = if cfg!(target_os = "windows") {
                    tr("DISPLAY2, \\\\.\\DISPLAY2 or just 2. Empty = the primary monitor.")
                } else {
                    tr("The output name, e.g. DP-1. Empty = the first monitor.")
                };
                control_row_tip(ui, tr("Monitor"), fixed_tip, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Fills the field with the monitor the built-in method reports as focused
                        // (Hyprland / the foreground window): the one this window is on when you click.
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
    let (col, msg) = if !OVERLAY_OS {
        (theme::FAINT, tr("Monitor detection needs Linux or Windows.").to_string())
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
    // A name wider than the chip (a narrow window makes the grid cells small) is cut with "…"
    // instead of being clipped mid-letter.
    let mut job = egui::text::LayoutJob::simple_singleline(module_name(m).to_owned(), chip_font(), text_col);
    job.wrap = egui::text::TextWrapping::truncate_at_width((r.width() - 15.0 - 3.0).max(0.0));
    let galley = p.layout_job(job);
    clip.galley(pos2(r.left() + 15.0, r.center().y - galley.size().y / 2.0), galley, text_col);
}

/// egui temp-data key of the layout grid's selected chip.
const LAYOUT_SEL_KEY: &str = "overlay_layout_sel";

/// Drop the layout grid's chip selection (on a tab switch, so stray arrow keys can't move a
/// chip after you come back).
pub fn clear_layout_selection(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<Option<Module>>(Id::new(LAYOUT_SEL_KEY)));
}

fn layout(ui: &mut Ui, app: &mut ForzaApp) {
    layout_card(ui, &mut app.config.overlay);
}

fn layout_card(ui: &mut Ui, o: &mut OverlayConfig) {
    theme::card(ui, tr("Layout"), |ui| {
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

fn minimap(ui: &mut Ui, app: &mut ForzaApp) {
    let mut open_map_tab = false;
    minimap_card(ui, &mut app.config.overlay, &mut open_map_tab);
    if open_map_tab {
        // The layers live on the Map tab's settings mode, Minimap page.
        app.current_tab = crate::app::Tab::Map;
        app.config.map_tab_settings = true;
        app.config.map_tab_page = crate::config::MapPage::Minimap;
    }
}

/// The Minimap module's frame (D75): shape, size, corner radius, outline and background. What
/// the map shows is on the Map tab (the button jumps there). Sizes are HUD design px (1080p,
/// scaled with the resolution and the HUD scale like the Layout card's).
fn minimap_card(ui: &mut Ui, o: &mut OverlayConfig, open_map_tab: &mut bool) {
    module_card(ui, o, tr("Minimap frame"), |o| &mut o.minimap_on, None, |ui, o| {
        let shape_tip = tr("A circle takes its diameter from the width alone.");
        control_row_tip(ui, tr("Shape"), shape_tip, |ui| {
            theme::radio_group(ui, &mut o.map_shape, &[(MapShape::RoundedRect, tr("Rounded")), (MapShape::Circle, tr("Circle"))]);
        });
        let px_tip = tr("In pixels at 1080p. Both scale with the resolution and the HUD scale.");
        let size = OverlayConfig::MAP_SIZE_RANGE;
        if o.map_shape == MapShape::Circle {
            theme::slider_row(ui, tr("Diameter"), &mut o.map_width, size, 1.0, 0, " px").on_hover_text(px_tip);
        } else {
            theme::slider_row(ui, tr("Width"), &mut o.map_width, size.clone(), 1.0, 0, " px").on_hover_text(px_tip);
            theme::slider_row(ui, tr("Height"), &mut o.map_height, size, 1.0, 0, " px").on_hover_text(px_tip);
            // The radius can't exceed half the shorter side: the slider stops there, and a value
            // saved above it is shown clamped (not rewritten until the user touches it).
            let half = (o.map_width.min(o.map_height) / 2.0).floor().max(1.0);
            let mut r = o.map_corner_radius.min(half);
            if theme::slider_row(ui, tr("Corner radius"), &mut r, 0.0..=half, 1.0, 0, " px").changed() {
                o.map_corner_radius = r;
            }
        }
        ui.add_space(4.0);
        ui.label(theme::section_label(tr("Outline")));
        theme::slider_row(ui, tr("Outline width"), &mut o.map_border_width, 0.0..=20.0, 0.5, 1, " px");
        control_row(ui, tr("Outline colour"), |ui| {
            egui::color_picker::color_edit_button_srgb(ui, &mut o.map_border_color);
        });
        pct_row(ui, tr("Outline opacity"), &mut o.map_border_opacity, 0.0, 100.0, 1.0, None);
        ui.add_space(4.0);
        ui.label(theme::section_label(tr("Background")));
        control_row(ui, tr("Background colour"), |ui| {
            egui::color_picker::color_edit_button_srgb(ui, &mut o.map_plate_color);
        });
        let bg_tip = tr("Behind the map image. At 0 the game shows through; the far edge of a tilted map fades into it.");
        pct_row(ui, tr("Background opacity"), &mut o.map_plate_opacity, 0.0, 100.0, 1.0, Some(bg_tip));
        ui.add_space(4.0);
        if ui.add(theme::secondary_button(tr("Reset frame"))).clicked() {
            o.reset_map_frame();
        }
        let layers_tip = tr("What the map shows (image, roads, POIs, race lines, tilt) and how it moves is set on the Map tab → Minimap.");
        if ui.add(theme::secondary_button(tr("Map layers…"))).on_hover_text(layers_tip).clicked() {
            *open_map_tab = true;
        }
    });
}

fn cluster(ui: &mut Ui, app: &mut ForzaApp) {
    cluster_card(ui, &mut app.config.overlay, app.config.use_mph);
}

fn cluster_card(ui: &mut Ui, o: &mut OverlayConfig, use_mph: bool) {
    module_card(ui, o, tr("Drive Cluster"), |o| &mut o.cluster_on, None, |ui, o| {
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
        let redline_tip = tr("The shift cue is the gearbox's own shift point (Gearbox → Shift RPM), taken from the max rpm the gearbox calibrates for each car. This works with the automatic gearbox off too. To calibrate again, use the \"Clear RPM calibration\" hotkey (Setup → Hotkey) or Gearbox → \"Clear RPM calibration\".");
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

fn race(ui: &mut Ui, app: &mut ForzaApp) {
    race_card(ui, &mut app.config.overlay);
}

fn race_card(ui: &mut Ui, o: &mut OverlayConfig) {
    let tip = tr("Swaps to the drift counter by itself when drifting is detected. Placed as Race / Drift in Layout.");
    module_card(ui, o, tr("Race Block"), |o| &mut o.race_on, Some(tip), |ui, o| {
        theme::checkbox_row(ui, &mut o.lap_delta, tr("Lap delta chip"));
        theme::checkbox_row(ui, &mut o.place_colour, tr("Place-change colour"))
            .on_hover_text(tr("Green fade when you gain a place, red when you lose one."));
    });
}

fn drift(ui: &mut Ui, app: &mut ForzaApp) {
    drift_card(ui, &mut app.config.overlay);
}

fn drift_card(ui: &mut Ui, o: &mut OverlayConfig) {
    let tip = tr("Replaces the race block automatically while you drift, in the same spot.");
    module_card(ui, o, tr("Drift Counter"), |o| &mut o.drift_on, Some(tip), |ui, o| {
        let style_tip = tr("Position + Gain shows your place and the points of the last interval, counting up. Total shows the event score, which Forza also shows itself.");
        control_row_tip(ui, tr("Style"), style_tip, |ui| {
            // Wraps: side by side when the half fits both, stacked in a narrow column.
            theme::radio_group(
                ui,
                &mut o.drift_style,
                &[(DriftStyle::PositionGain, tr("Position + Gain")), (DriftStyle::Total, tr("Total score"))],
            );
        });
        theme::slider_row(ui, tr("Gain chip interval"), &mut o.drift_chip_secs, 1.0..=10.0, 1.0, 0, " s");
        let bar = format!("{} ({:.0} s)", tr("Progress bar"), o.drift_chip_secs);
        theme::checkbox_row(ui, &mut o.drift_bar, bar);
    });
}

/// D26: the master switch and the stack's anchor. The per-event toggles live in
/// Mini-Settings → Overlay (`app.rs`), where they're one click from the game.
fn notifications(ui: &mut Ui, app: &mut ForzaApp) {
    notifications_card(ui, &mut app.config.overlay);
}

fn notifications_card(ui: &mut Ui, o: &mut OverlayConfig) {
    module_card(ui, o, tr("Notifications"), |o| &mut o.notif_on, None, |ui, o| {
        ui.label(theme::section_label(tr("Position")));
        anchor_picker(ui, &mut o.notif_cell);
    });
}

/// A small 3×3 picker for one [`HudCell`]: click a cell to anchor there.
fn anchor_picker(ui: &mut Ui, cell: &mut HudCell) {
    let w = ui.available_width().min(220.0);
    let (grid, _) = ui.allocate_exact_size(vec2(w, w * 9.0 / 16.0), Sense::hover());
    ui.painter().rect(grid, 6.0, theme::FIELD, Stroke::new(1.0, theme::BTNBD), egui::StrokeKind::Inside);
    let inner = grid.shrink(GRID_GAP);
    let size = (inner.size() - Vec2::splat(2.0 * GRID_GAP)) / 3.0;
    for c in HudCell::ALL {
        let at = vec2(c.col() as f32, c.row() as f32) * (size + Vec2::splat(GRID_GAP));
        let r = Rect::from_min_size(inner.min + at, size);
        let resp = ui
            .interact(r, Id::new(("notif_cell", c as usize)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if resp.clicked() {
            *cell = c;
        }
        if *cell == c {
            ui.painter().rect(r, 4.0, theme::SEL, Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
            ui.painter().circle_filled(r.center(), 4.0, theme::ACCENT);
        } else {
            let (fill, stroke) = if resp.hovered() { (theme::SEL, theme::ACCENT) } else { (Color32::TRANSPARENT, theme::BORDER) };
            ui.painter().rect(r, 4.0, fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
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

    // ── Panes: every page of the tab, at the minimum, the column switch and the user's width ──

    use crate::ui::test_render;
    use crate::maprender::paint2d::IconAtlas;
    use crate::maprender::store::Layers;
    use crate::config::AppConfig;
    use std::sync::Arc;

    pub(crate) const WIDTHS: [f32; 4] = [700.0, 1000.0, 1100.0, 1235.0];

    /// Nothing paints outside the window, no text is cut off at a pane edge (everything
    /// genuinely fits, not just clipped), and no two card frames overlap (a card never paints
    /// into another column). Returns the number of card frames.
    pub(crate) fn check_panes(out: &egui::FullOutput, w: f32, what: &str) -> usize {
        let mut frames: Vec<Rect> = Vec::new();
        for c in &out.shapes {
            match &c.shape {
                egui::Shape::Rect(r) if r.corner_radius == egui::CornerRadius::same(7) && r.stroke.width > 0.0 && r.rect.height() > 40.0 && r.rect.width() > 100.0 => {
                    frames.push(r.rect);
                }
                egui::Shape::Text(t) => {
                    let b = t.visual_bounding_rect();
                    assert!(
                        b.left() >= c.clip_rect.left() - 0.5 && b.right() <= c.clip_rect.right() + 0.5,
                        "{what} at {w} px: text {:?} is cut off at a pane edge ({b:?} vs clip {:?})",
                        t.galley.text(),
                        c.clip_rect
                    );
                }
                _ => {}
            }
        }
        for r in test_render::visible_rects(out) {
            assert!(r.left() >= -0.5 && r.right() <= w + 0.5, "{what} at {w} px: a shape leaves the window: {r:?}");
        }
        for (i, a) in frames.iter().enumerate() {
            for b in &frames[i + 1..] {
                let i = a.intersect(*b);
                assert!(!(i.width() > 1.0 && i.height() > 1.0), "{what} at {w} px: cards overlap: {a:?} / {b:?}");
            }
        }
        frames.len()
    }

    /// Run `page` with this context's POI icon atlas (the synthetic icons uploaded for real).
    pub(crate) fn render(name: &str, w: f32, h: f32, mut page: impl FnMut(&mut Ui, &IconAtlas)) -> egui::FullOutput {
        let ctx = test_render::context();
        let mut tex = crate::maprender::icontex::IconTex::default();
        let icons = Arc::new(crate::maprender::icontex::synthetic_icons());
        let atlas = tex.ensure(&ctx, Some(&icons)).expect("atlas");
        let (out, textures) = test_render::run(&ctx, w, h, |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            page(ui, &atlas)
        });
        test_render::snapshot(&ctx, &out, &textures, w as u32, h as u32, &format!("{name}_{w}"));
        out
    }

    pub(crate) fn layers_ready() -> Layers {
        use crate::maprender::data::MapLayers;
        let note = "Your saved road types were ignored: they were made for a different version of the game's road network. The project data is used instead.";
        let data = MapLayers { note: Some(note.into()), ..MapLayers::synthetic() };
        Layers { status: crate::maprender::LayerStatus::Ready, data: Some(Arc::new(data)) }
    }

    type Card = fn(&mut Ui, &mut OverlayConfig);

    /// The stand-in for the General card (the real one needs the whole app) with the same rows.
    fn general_stand_in(ui: &mut Ui, o: &mut OverlayConfig) {
        theme::card(ui, tr("General"), |ui| {
            theme::checkbox_row(ui, &mut o.enabled, tr("Enable overlay"));
            status_line(ui, theme::GOOD, tr("Overlay running"));
            control_row(ui, "Hide HUD", |ui| ui.add_sized([ui.available_width(), 22.0], egui::Button::new("J")));
            pct_row(ui, tr("Scale"), &mut o.scale, 50.0, 200.0, 5.0, None);
            pct_row(ui, tr("Plate opacity"), &mut o.plate_opacity, 0.0, 100.0, 1.0, None);
            theme::checkbox_row(ui, &mut o.fade, tr("Fade on show / hide"));
            theme::checkbox_row(ui, &mut o.focus_only, tr("Only when game window is focused"));
        });
    }

    /// The module cards of the pages.
    #[test]
    fn card_pages_stay_inside_their_panes() {
        let layout: Card = |u, o| layout_card(u, o);
        let minimap: Card = |u, o| minimap_card(u, o, &mut false);
        let cluster: Card = |u, o| cluster_card(u, o, false);
        let race: Card = |u, o| race_card(u, o);
        let drift: Card = |u, o| drift_card(u, o);
        let notif: Card = |u, o| notifications_card(u, o);
        for w in WIDTHS {
            let three = w >= THREE_COLS_MIN_W;
            let general: Card = general_stand_in;
            let pages: Vec<(&str, usize, Vec<Vec<Card>>)> = vec![
                ("general", 2, if three { vec![vec![general], vec![layout]] } else { vec![vec![layout, general]] }),
                ("minimap", 1, vec![vec![minimap]]),
                ("cluster", 1, vec![vec![cluster]]),
                ("race", 2, vec![vec![race], vec![drift]]),
                ("notifications", 1, vec![vec![notif]]),
            ];
            for (name, expect, cols) in pages {
                let mut o = OverlayConfig::default();
                let out = render(&format!("overlay_{name}"), w, 1500.0, |ui, _| {
                    let n = if three { 3 } else { 2 };
                    theme::columns(ui, n, |uis| {
                        for (ui, cards) in uis.iter_mut().zip(&cols) {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for card in cards {
                                card(ui, &mut o);
                            }
                        }
                    });
                });
                assert_eq!(check_panes(&out, w, name), expect, "{name} at {w} px");
            }
        }
    }

    /// The Minimap frame page (D75), both shapes and both languages, enabled and greyed out:
    /// nothing leaves the pane or is cut off, and the card fits in one column at every width.
    /// (The German labels are the long ones.)
    #[test]
    fn minimap_frame_page_stays_inside_its_pane_in_both_languages() {
        use crate::i18n::{with_language, Language};
        for lang in [Language::English, Language::German] {
            with_language(lang, || {
                for w in [700.0, 1000.0, 1235.0] {
                    for (shape, on) in [(MapShape::RoundedRect, true), (MapShape::Circle, true), (MapShape::RoundedRect, false)] {
                        let mut o = OverlayConfig { map_shape: shape, minimap_on: on, ..Default::default() };
                        let out = render(&format!("overlay_minimap_{lang:?}_{shape:?}_{on}"), w, 900.0, |ui, _| {
                            theme::columns(ui, if w >= THREE_COLS_MIN_W { 3 } else { 2 }, |uis| {
                                uis[0].spacing_mut().item_spacing.y = 0.0;
                                minimap_card(&mut uis[0], &mut o, &mut false);
                            });
                        });
                        let n = check_panes(&out, w, &format!("minimap {lang:?} {shape:?}"));
                        assert_eq!(n, 1, "{lang:?} {shape:?} at {w} px");
                    }
                }
            });
        }
    }

    /// The Minimap page edits the frame fields, the Reset button restores exactly the defaults
    /// (and leaves the map layers alone), and "Map layers…" asks for the Map tab.
    #[test]
    fn minimap_frame_reset_restores_the_pill_and_keeps_the_layers() {
        let mut o = OverlayConfig::default();
        o.map_shape = MapShape::Circle;
        o.map_width = 400.0;
        o.map_height = 300.0;
        o.map_corner_radius = 60.0;
        o.map_border_width = 8.0;
        o.map_border_color = [200, 10, 10];
        o.map_border_opacity = 0.3;
        o.map_plate_color = [1, 2, 3];
        o.map_plate_opacity = 0.5;
        o.map_layers.roads.on = !o.map_layers.roads.on;
        let layers = o.map_layers.clone();
        o.reset_map_frame();
        assert_eq!(OverlayConfig { map_layers: layers.clone(), ..OverlayConfig::default() }, o);
        assert_eq!(o.map_layers, layers);
    }

    /// The module selector: one row where the labels fit, two rows where they don't (the long
    /// German labels at the window minimum), and its labels never run into each other.
    #[test]
    fn page_selector_never_overlaps_its_labels() {
        let en = [
            (OverlayPage::General, "General"),
            (OverlayPage::Minimap, "Minimap"),
            (OverlayPage::Cluster, "Drive cluster"),
            (OverlayPage::Race, "Race / Drift"),
            (OverlayPage::Notifications, "Notifications"),
        ];
        let de = [
            (OverlayPage::General, "Allgemein"),
            (OverlayPage::Minimap, "Minikarte"),
            (OverlayPage::Cluster, "Fahranzeige"),
            (OverlayPage::Race, "Rennen / Drift"),
            (OverlayPage::Notifications, "Benachrichtigungen"),
        ];
        for (lang, opts) in [("en", &en), ("de", &de)] {
            for w in [600.0, 700.0, 800.0, 1000.0, 1280.0] {
                let mut page = OverlayPage::General;
                let out = render(&format!("overlay_selector_{lang}"), w, 120.0, |ui, _| {
                    page_selector_with(ui, &mut page, opts);
                });
                let mut labels: Vec<Rect> = Vec::new();
                for c in &out.shapes {
                    if let egui::Shape::Text(t) = &c.shape {
                        labels.push(t.visual_bounding_rect());
                    }
                }
                assert_eq!(labels.len(), 5, "{lang} at {w} px: {:?}", out.shapes.iter().filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.galley.text().to_string()) } else { None }).collect::<Vec<_>>());
                for (i, a) in labels.iter().enumerate() {
                    assert!(a.left() >= 0.0 && a.right() <= w, "{lang} at {w} px: label leaves the window: {a:?}");
                    for b in &labels[i + 1..] {
                        assert!(!a.expand(2.0).intersects(*b), "{lang} at {w} px: labels touch: {a:?} / {b:?}");
                    }
                }
            }
        }
    }

    /// The remembered page round-trips through the config JSON and is never exported.
    #[test]
    fn remembered_page_is_saved_but_not_exported() {
        let mut c = AppConfig::default();
        c.overlay_page = OverlayPage::Race;
        let back: AppConfig = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.overlay_page, OverlayPage::Race);
        let all = vec![true; crate::config::KEY_GROUPS.len()];
        assert!(!crate::config::export_selected(&c, &all).contains("overlay_page"));
    }
}

