//! `Hud::draw` state tests: the global fade and the `animating` flag. Runs real egui passes
//! (no GPU: shapes are only collected, never painted).

use std::sync::Arc;

use egui::{pos2, vec2, Id, LayerId, Order, Rect};

use super::{fonts, Hud};
use crate::config::{DriftStyle, OverlayConfig};
use crate::overlay::snapshot::{DriftChip, DriftInfo, HudMode, HudSnapshot, PlaceChange};

fn ctx() -> egui::Context {
    let ctx = egui::Context::default();
    fonts::install(&ctx);
    ctx
}

/// One pass at `now`; returns `animating`.
fn frame(ctx: &egui::Context, hud: &mut Hud, snap: &HudSnapshot, now: f64) -> bool {
    let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1920.0, 1080.0))), ..Default::default() };
    let mut animating = false;
    let _ = ctx.run(raw, |ctx| {
        let p = ctx.layer_painter(LayerId::new(Order::Background, Id::new("hud")));
        animating = hud.draw(&p, ctx.content_rect(), snap, now, None, &Default::default());
    });
    animating
}

/// Frames at 60 Hz from `t0` until `animating` is false; returns the time it settled.
fn settle(ctx: &egui::Context, hud: &mut Hud, snap: &HudSnapshot, t0: f64, max_secs: f64) -> f64 {
    let mut t = t0;
    while frame(ctx, hud, snap, t) {
        t += 1.0 / 60.0;
        assert!(t - t0 < max_secs, "still animating after {max_secs} s");
    }
    t
}

fn snap(cfg: OverlayConfig) -> HudSnapshot {
    let mut s = HudSnapshot { visible: true, connected: true, cfg: Arc::new(cfg), ..Default::default() };
    s.pkt.is_race_on = 1;
    s.pkt.engine_max_rpm = 8000.0;
    s.pkt.engine_idle_rpm = 900.0;
    s.pkt.current_engine_rpm = 3000.0;
    s.pkt.speed = 30.0; // driving: the map zoom holds at the driving value
    s.redline_rpm = 6800.0;
    s.shift_rpm = 7440.0;
    s
}

#[test]
fn fade_in_settles_then_fade_out_reports_done() {
    let (ctx, mut hud) = (ctx(), Hud::default());
    let mut s = snap(OverlayConfig::default());
    // Fresh (surface just created): alpha 0, fading in.
    assert!(frame(&ctx, &mut hud, &s, 10.0));
    assert_eq!(hud.fade, 0.0);
    let t = settle(&ctx, &mut hud, &s, 10.0 + 1.0 / 60.0, 1.0);
    assert_eq!(hud.fade, 1.0);
    assert!(t - 10.0 < 0.25, "fade-in took {} s", t - 10.0);
    // A settled HUD stays settled.
    assert!(!frame(&ctx, &mut hud, &s, t + 0.5));

    // Hide: animating while fading, false exactly when the fade-out is done.
    s.visible = false;
    let t0 = t + 0.5 + 1.0 / 60.0;
    assert!(frame(&ctx, &mut hud, &s, t0));
    let t1 = settle(&ctx, &mut hud, &s, t0 + 1.0 / 60.0, 1.0);
    assert_eq!(hud.fade, 0.0);
    assert!(t1 - t0 < 0.25);
    assert!(!frame(&ctx, &mut hud, &s, t1 + 1.0));
}

#[test]
fn fade_off_snaps() {
    let (ctx, mut hud) = (ctx(), Hud::default());
    let mut s = snap(OverlayConfig { fade: false, ..Default::default() });
    assert!(!frame(&ctx, &mut hud, &s, 1.0));
    assert_eq!(hud.fade, 1.0);
    s.visible = false;
    assert!(!frame(&ctx, &mut hud, &s, 1.016));
    assert_eq!(hud.fade, 0.0);
}

#[test]
fn hidden_from_the_start_never_animates() {
    let (ctx, mut hud) = (ctx(), Hud::default());
    let s = HudSnapshot { visible: false, ..snap(OverlayConfig::default()) };
    assert!(!frame(&ctx, &mut hud, &s, 3.0));
}

#[test]
fn place_change_and_lap_hold_animate_then_settle() {
    let (ctx, mut hud) = (ctx(), Hud::default());
    let mut s = snap(OverlayConfig { fade: false, ..Default::default() });
    s.pkt.race_position = 2;
    s.events.place_change = Some(PlaceChange { at: 100.0, gained: true });
    assert!(frame(&ctx, &mut hud, &s, 101.0));
    assert!(frame(&ctx, &mut hud, &s, 102.9));
    assert!(!frame(&ctx, &mut hud, &s, 103.1));
    s.events.lap_completed_at = Some(200.0);
    assert!(frame(&ctx, &mut hud, &s, 203.9));
    assert!(!frame(&ctx, &mut hud, &s, 204.1));
}

#[test]
fn shift_flash_and_pulse_animate_only_while_active() {
    let (ctx, mut hud) = (ctx(), Hud::default());
    let mut s = snap(OverlayConfig { fade: false, ..Default::default() });
    s.pkt.current_engine_rpm = 7600.0;
    assert!(frame(&ctx, &mut hud, &s, 5.0));
    s.cfg = Arc::new(OverlayConfig { fade: false, shift_flash: false, ..Default::default() });
    assert!(!frame(&ctx, &mut hud, &s, 5.016));
    s.events.gear_changed_at = Some(6.0);
    assert!(frame(&ctx, &mut hud, &s, 6.2));
    assert!(!frame(&ctx, &mut hud, &s, 6.3));
}

#[test]
fn drift_count_up_chip_and_bar() {
    let (ctx, mut hud) = (ctx(), Hud::default());
    let mut s = snap(OverlayConfig { fade: false, drift_bar: false, drift_style: DriftStyle::Total, ..Default::default() });
    s.mode = HudMode::Drift;
    s.drift = DriftInfo { score: 1000.0, interval: 5.0, ..Default::default() };
    // First frame snaps to the score: nothing to count.
    assert!(!frame(&ctx, &mut hud, &s, 1.0));
    assert_eq!(hud.shown_score, Some(1000.0));
    // The score jumps: counts up, then settles on it exactly.
    s.drift.score = 5000.0;
    assert!(frame(&ctx, &mut hud, &s, 1.016));
    let shown = hud.shown_score.unwrap_or_default();
    assert!(shown > 1000.0 && shown < 5000.0, "{shown}");
    settle(&ctx, &mut hud, &s, 1.032, 3.0);
    assert_eq!(hud.shown_score, Some(5000.0));
    // A chip animates for 3.6 s; a +0 chip never shows or animates.
    s.drift.chip = Some(DriftChip { gain: 400.0, at: 10.0 });
    assert!(frame(&ctx, &mut hud, &s, 13.5));
    assert!(!frame(&ctx, &mut hud, &s, 13.7));
    s.drift.chip = Some(DriftChip { gain: 0.0, at: 20.0 });
    assert!(!frame(&ctx, &mut hud, &s, 20.5));
    // The window bar animates while a window runs.
    s.cfg = Arc::new(OverlayConfig { fade: false, drift_bar: true, drift_style: DriftStyle::Total, ..Default::default() });
    s.drift.window_start = Some(20.0);
    assert!(frame(&ctx, &mut hud, &s, 21.0));
}

#[test]
fn map_zoom_eases_out_when_stopped_then_settles() {
    let (ctx, mut hud) = (ctx(), Hud::default());
    let mut s = snap(OverlayConfig { fade: false, ..Default::default() });
    s.pkt.speed = 0.0;
    // Packets keep coming while stopped (60 Hz frames); nothing eases for the first 1.5 s.
    let mut t = 1.0;
    while t < 2.4 {
        assert!(!frame(&ctx, &mut hud, &s, t), "at {t}");
        t += 1.0 / 60.0;
    }
    // Past 1.5 s stopped: the zoom eases out to the stopped zoom, then settles.
    assert!(frame(&ctx, &mut hud, &s, 2.6));
    settle(&ctx, &mut hud, &s, 2.6 + 1.0 / 60.0, 10.0);
    assert!((hud.map_anim.zoom().unwrap_or_default() - s.cfg.zoom_stopped_m).abs() <= 0.5);
}

#[test]
fn free_roam_hides_race_block_and_closes_the_gap() {
    use super::layout::{layout, Module};
    use crate::config::HudCell;
    // Map and race stacked in the top-left cell.
    let cfg = OverlayConfig { race_cell: HudCell::TopLeft, minimap_cell: HudCell::TopLeft, cluster_on: false, ..Default::default() };
    let mut s = snap(cfg);
    s.pkt.race_position = 3;
    let items = super::modules(&s);
    assert_eq!(items.iter().map(|i| i.0).collect::<Vec<_>>(), vec![Module::Map, Module::Race]);
    // Free roam (position 0): the race slot is empty, the map alone at the edge.
    s.pkt.race_position = 0;
    let items = super::modules(&s);
    assert_eq!(items.iter().map(|i| i.0).collect::<Vec<_>>(), vec![Module::Map]);
    assert_eq!(layout(vec2(1920.0, 1080.0), 1.0, 44.0, 12.0, &items)[0].min, pos2(44.0, 44.0));
    // The drift counter doesn't depend on the race position.
    s.mode = HudMode::Drift;
    s.drift.score = 120.0;
    assert!(super::modules(&s).iter().any(|i| i.0 == Module::Race));
}

#[test]
fn drift_mode_without_drift_data_hides_the_slot() {
    use super::layout::Module;
    let mut s = snap(OverlayConfig::default());
    s.mode = HudMode::Drift;
    let has_slot = |s: &HudSnapshot| super::modules(s).iter().any(|i| i.0 == Module::Race);
    // Free roam after a drift event: still Drift mode, but score and best are 0.
    assert!(!has_slot(&s));
    // A live score or just an event high score (score reset to 0 at the start) shows it.
    s.drift.score = 50.0;
    assert!(has_slot(&s));
    s.drift = DriftInfo { best: 61_200.0, ..Default::default() };
    assert!(has_slot(&s));
}

#[test]
fn drift_dot_lit_only_while_scoring_and_settles() {
    use super::drift::{scoring, DRIFT_ACTIVE_SECS};
    let (ctx, mut hud) = (ctx(), Hud::default());
    let mut s = snap(OverlayConfig { fade: false, drift_bar: false, ..Default::default() });
    s.mode = HudMode::Drift;
    s.drift = DriftInfo { score: 1000.0, interval: 5.0, window_start: Some(9.0), ..Default::default() };
    // Windows run the whole time, but no rise yet: grey, and nothing animates.
    assert!(!scoring(&s, 10.0));
    assert!(!frame(&ctx, &mut hud, &s, 10.0));
    // A rise lights it and keeps the frame loop going until it goes grey, then settles.
    s.drift.last_rise_at = Some(10.0);
    assert!(scoring(&s, 10.5));
    assert!(frame(&ctx, &mut hud, &s, 10.5));
    assert!(!scoring(&s, 10.0 + DRIFT_ACTIVE_SECS));
    settle(&ctx, &mut hud, &s, 10.5 + 1.0 / 60.0, DRIFT_ACTIVE_SECS + 0.1);
}

fn shown_gain(hud: &Hud) -> f32 {
    hud.shown_gain.map_or(-1.0, |g| g.1)
}

#[test]
fn position_gain_counts_up_from_zero_then_holds() {
    let (ctx, mut hud) = (ctx(), Hud::default());
    let cfg = OverlayConfig { fade: false, drift_bar: false, ..Default::default() };
    assert_eq!(cfg.drift_style, DriftStyle::PositionGain, "the default style");
    let mut s = snap(cfg);
    s.mode = HudMode::Drift;
    s.pkt.race_position = 3;
    s.drift = DriftInfo { score: 1000.0, interval: 5.0, ..Default::default() };
    // No window closed yet: "+0", settled.
    assert!(!frame(&ctx, &mut hud, &s, 1.0));
    assert_eq!(shown_gain(&hud), 0.0);
    // A window closes with +4,039: counts up from 0 and settles on it exactly.
    s.drift.chip = Some(DriftChip { gain: 4039.4, at: 2.0 });
    assert!(frame(&ctx, &mut hud, &s, 2.016));
    let g = shown_gain(&hud);
    assert!(g > 0.0 && g < 4039.0, "{g}");
    let t = settle(&ctx, &mut hud, &s, 2.032, 3.0);
    assert_eq!(shown_gain(&hud), 4039.0);
    // Holds (no redraw needed) until the next window closes, however long that takes.
    assert!(!frame(&ctx, &mut hud, &s, t + 4.0));
    assert_eq!(shown_gain(&hud), 4039.0);
    // The next window restarts the count from 0, even for a smaller gain.
    s.drift.chip = Some(DriftChip { gain: 1200.0, at: 10.0 });
    assert!(frame(&ctx, &mut hud, &s, 10.016));
    assert!(shown_gain(&hud) < 1200.0);
    settle(&ctx, &mut hud, &s, 10.032, 3.0);
    assert_eq!(shown_gain(&hud), 1200.0);
    // A window with no gain: "+0" at once, nothing to count.
    s.drift.chip = Some(DriftChip { gain: 0.0, at: 15.0 });
    assert!(!frame(&ctx, &mut hud, &s, 15.016));
    assert_eq!(shown_gain(&hud), 0.0);
    // Total style leaves the gain state alone and counts the score instead.
    s.cfg = Arc::new(OverlayConfig { fade: false, drift_bar: false, drift_style: DriftStyle::Total, ..Default::default() });
    frame(&ctx, &mut hud, &s, 16.0);
    assert_eq!(hud.shown_score, Some(1000.0));
}

/// Every filled path's colour in one pass (the cap layers are convex polygons).
fn fills(ctx: &egui::Context, hud: &mut Hud, snap: &HudSnapshot, now: f64) -> Vec<egui::Color32> {
    let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1920.0, 1080.0))), ..Default::default() };
    let out = ctx.run(raw, |ctx| {
        let p = ctx.layer_painter(LayerId::new(Order::Background, Id::new("hud")));
        hud.draw(&p, ctx.content_rect(), snap, now, None, &Default::default());
    });
    out.shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::Shape::Path(ps) => Some(ps.fill),
            _ => None,
        })
        .collect()
}

#[test]
fn position_gain_cap_gets_the_place_change_layer() {
    use super::col;
    let (ctx, mut hud) = (ctx(), Hud::default());
    let mut s = snap(OverlayConfig { fade: false, drift_bar: false, ..Default::default() });
    s.mode = HudMode::Drift;
    s.pkt.race_position = 2;
    s.drift = DriftInfo { score: 1000.0, interval: 5.0, ..Default::default() };
    s.events.place_change = Some(PlaceChange { at: 100.0, gained: true });
    assert!(fills(&ctx, &mut hud, &s, 101.0).contains(&col::GAIN));
    assert!(frame(&ctx, &mut hud, &s, 102.9), "animates through the 3 s curve");
    assert!(!frame(&ctx, &mut hud, &s, 103.1));
    assert!(!fills(&ctx, &mut hud, &s, 103.2).contains(&col::GAIN));
    s.events.place_change = Some(PlaceChange { at: 200.0, gained: false });
    assert!(fills(&ctx, &mut hud, &s, 201.0).contains(&col::LOSS));
    // The Race Block's colour switch governs it here too.
    s.cfg = Arc::new(OverlayConfig { fade: false, drift_bar: false, place_colour: false, ..Default::default() });
    assert!(!fills(&ctx, &mut hud, &s, 201.0).contains(&col::LOSS));
}

#[test]
fn speed_hold_refreshes_at_most_every_half_second() {
    let (ctx, mut hud) = (ctx(), Hud::default());
    let mut s = snap(OverlayConfig { fade: false, speed_hold: true, ..Default::default() });
    s.pkt.speed = 100.0 / 3.6;
    assert!(!frame(&ctx, &mut hud, &s, 1.0));
    assert_eq!(hud.held_speed, Some((1.0, 100)));
    // Packets keep changing the speed at 60 Hz; the shown number holds for 0.5 s and the
    // hold never keeps the frame loop alive by itself.
    let mut t = 1.0;
    for kmh in 101..=129 {
        t += 1.0 / 60.0;
        s.pkt.speed = kmh as f32 / 3.6;
        assert!(!frame(&ctx, &mut hud, &s, t), "at {t}");
        assert_eq!(hud.held_speed.map(|h| h.1), Some(100), "at {t}");
    }
    // The first frame at ≥ 0.5 s takes the latest value and restarts the hold.
    s.pkt.speed = 130.0 / 3.6;
    assert!(!frame(&ctx, &mut hud, &s, 1.5));
    assert_eq!(hud.held_speed, Some((1.5, 130)));
    s.pkt.speed = 131.0 / 3.6;
    frame(&ctx, &mut hud, &s, 1.6);
    assert_eq!(hud.speed(&s, 1.6), 130);
    assert_eq!(hud.speed(&s, 2.0), 131);
    // Off: always live, nothing held.
    s.cfg = Arc::new(OverlayConfig { fade: false, ..Default::default() });
    s.pkt.speed = 140.0 / 3.6;
    assert_eq!(hud.speed(&s, 2.01), 140);
    assert_eq!(hud.held_speed, None);
}
