//! HUD drawing: pure egui/epaint draw code for the overlay (`overlay/render.rs` drives it).
//! The widgets follow the round-4 spec sheet of the overlay mockup: drive cluster D1a / D3a′,
//! minimap M2′, race block R1′ and drift counter X1′, placed by the 3×3 slot layout.
//!
//! Everything is drawn in 1920×1080 design px and scaled by `(surface_h / 1080) × cfg.scale`.
//! Animation is time-based on an explicit `now` ([`hud_clock`](crate::overlay::snapshot::hud_clock)
//! seconds), so tests and the PNG harness pin time.

pub mod anim;
pub mod cluster;
pub mod drift;
pub mod fonts;
pub mod layout;
pub mod minimap;
pub mod notify;
pub mod prims;
pub mod race;

use egui::{Painter, Rect};

use crate::config::{ClusterStyle, DriftStyle, HudCell};
use crate::overlay::snapshot::{HudMode, HudSnapshot};
use layout::Module;
use prims::Xf;

/// The HUD's semantic colours (round-4 spec sheet "Colour tokens"). Not app chrome, so not
/// `theme.rs` tokens: these are the mockup's values, and they never follow the app theme.
pub mod col {
    use super::prims::rgba;
    use egui::Color32;

    pub const INK: Color32 = rgba(0xF5, 0xF7, 0xFB, 1.0);
    /// Small labels (R1′ label, D3a′ KM/H).
    pub const DIM: Color32 = rgba(245, 247, 251, 0.72);
    /// D1a unit label (KM/H or rpm).
    pub const DIM_UNIT: Color32 = rgba(245, 247, 251, 0.70);
    /// X1′ cap label.
    pub const DIM_CAP: Color32 = rgba(245, 247, 251, 0.80);
    /// Numeral on the pulsing (ink-filled) gear cell.
    pub const ON_INK: Color32 = rgba(0x0B, 0x10, 0x20, 1.0);
    pub const WHITE: Color32 = Color32::WHITE;
    pub const CELL: Color32 = rgba(255, 255, 255, 0.13);
    pub const OFF: Color32 = rgba(255, 255, 255, 0.15);
    pub const OFF_RED: Color32 = rgba(255, 67, 56, 0.32);
    pub const RED: Color32 = rgba(0xFF, 0x43, 0x38, 1.0);
    pub const SHIFT: Color32 = rgba(0x5B, 0x8B, 0xF0, 1.0);
    pub const SHIFT_FILL: Color32 = rgba(0x3C, 0x6B, 0xDE, 1.0);
    pub const RIM: Color32 = rgba(255, 255, 255, 0.18);
    pub const GAIN: Color32 = rgba(0x2E, 0x9E, 0x48, 1.0);
    pub const LOSS: Color32 = rgba(0xD8, 0x32, 0x2B, 1.0);
    pub const BEST: Color32 = rgba(0x6C, 0xE0, 0x6A, 1.0);
    pub const AMBER: Color32 = rgba(0xFF, 0xB0, 0x2E, 1.0);
    pub const AMBER_TEXT: Color32 = rgba(0x1A, 0x12, 0x04, 1.0);
    pub const DOT: Color32 = rgba(255, 255, 255, 0.22);
    pub const TRACK: Color32 = rgba(255, 255, 255, 0.16);
    pub const FRAME: Color32 = rgba(12, 17, 27, 0.88);
    pub const COMPASS: Color32 = rgba(12, 17, 27, 0.85);
    pub const NORTH: Color32 = rgba(0xFF, 0x5A, 0x4E, 1.0);
    pub const MARKER_EDGE: Color32 = rgba(0, 0, 0, 0.75);
    pub const MAP_TINT: Color32 = rgba(10, 14, 22, 0.12);
    pub const MAP_TINT_WINTER: Color32 = rgba(20, 30, 45, 0.30);

    /// The spec's backings are tuned for the default plate opacity 0.68; the setting scales
    /// every backing (plate, halo disc, cap) by the same factor.
    fn backing(r: u8, g: u8, b: u8, spec_a: f32, plate_opacity: f32) -> Color32 {
        let a = (spec_a * plate_opacity.clamp(0.0, 1.0) / 0.68).min(1.0);
        Color32::from_rgba_unmultiplied(r, g, b, (a * 255.0).round() as u8)
    }
    /// Pill plates, `rgba(9,13,21,.68)` at the default opacity.
    pub fn plate(opacity: f32) -> Color32 {
        backing(9, 13, 21, 0.68, opacity)
    }
    /// Halo disc, `rgba(9,13,21,.70)`.
    pub fn disc(opacity: f32) -> Color32 {
        backing(9, 13, 21, 0.70, opacity)
    }
    /// Position / drift cap, `rgba(58,66,82,.95)`.
    pub fn cap(opacity: f32) -> Color32 {
        backing(58, 66, 82, 0.95, opacity)
    }
}

/// Per-renderer HUD state: the global show/hide fade and the eased values that need the
/// previous frame (drift count-up, map yaw/zoom).
#[derive(Default)]
pub struct Hud {
    /// Global alpha, eased toward `snapshot.visible`. 0 after a finished fade-out, so a
    /// re-created surface fades in.
    fade: f32,
    /// `now` of the previous frame; `None` while fully hidden (the next frame gets dt 0).
    last_now: Option<f64>,
    /// X1′'s displayed total (counts up toward the score).
    shown_score: Option<f32>,
    /// Position + Gain's displayed "+N" and the `at` of the window it belongs to.
    shown_gain: Option<(Option<f64>, f32)>,
    /// With `speed_hold`: the speed on screen and when it was taken.
    held_speed: Option<(f64, i64)>,
    map_anim: minimap::MapAnim,
}

/// With `OverlayConfig::speed_hold`, the cluster's speed number refreshes at most this often (s).
pub const SPEED_HOLD_SECS: f64 = 0.5;

impl Hud {
    /// The cluster speed to show. With `speed_hold` the last shown value holds for
    /// [`SPEED_HOLD_SECS`], then the next frame takes the live one. why no `animating`: the
    /// packet stream redraws at 60 Hz anyway, so a due refresh just waits for the next packet.
    fn speed(&mut self, snap: &HudSnapshot, now: f64) -> i64 {
        let live = cluster::speed(snap);
        if !snap.cfg.speed_hold {
            self.held_speed = None;
            return live;
        }
        match self.held_speed {
            Some((t, v)) if (0.0..SPEED_HOLD_SECS).contains(&(now - t)) => v,
            _ => {
                self.held_speed = Some((now, live));
                live
            }
        }
    }

    /// Draw the whole HUD on `screen`. Returns true while anything still animates (fade,
    /// shift flash, pulse, place layer, lap hold, chip, count-up, drift bar, map easing);
    /// false once settled, and false after the fade-out finished (the surface can go).
    /// `teammates` are the co-op markers for M2′ (empty when off or not connected).
    pub fn draw(&mut self, p: &Painter, screen: Rect, snap: &HudSnapshot, now: f64, map: Option<minimap::MapTex>, teammates: &[minimap::Teammate]) -> bool {
        let cfg = &*snap.cfg;
        let dt = self.last_now.map_or(0.0, |t| (now - t).clamp(0.0, 0.1) as f32);
        self.last_now = Some(now);
        let target = if snap.visible { 1.0 } else { 0.0 };
        self.fade = anim::fade_step(self.fade, target, dt, cfg.fade);
        let mut animating = self.fade != target;
        if self.fade <= 0.0 {
            if target == 0.0 {
                // Fully hidden: forget the eased state, the next show snaps it fresh.
                *self = Self::default();
            }
            return animating;
        }

        let s = screen.height() / 1080.0 * cfg.scale.max(0.05);
        let drift = snap.mode == HudMode::Drift;
        let items = modules(snap);
        let rects = layout::layout(screen.size(), s, cfg.margin_px, cfg.gap_px, &items);

        for ((module, _, _), rect) in items.iter().zip(rects) {
            let xf = Xf { o: screen.min + rect.min.to_vec2(), s, a: self.fade };
            animating |= match module {
                Module::Map => minimap::draw(p, &xf, snap, now, &mut self.map_anim, map, teammates),
                Module::Cluster => {
                    let speed = self.speed(snap, now);
                    match cfg.cluster_style {
                        ClusterStyle::Pill => cluster::draw_pill(p, &xf, snap, now, speed),
                        ClusterStyle::Halo => cluster::draw_halo(p, &xf, snap, now, speed),
                    }
                }
                Module::Race if drift => match cfg.drift_style {
                    DriftStyle::Total => {
                        let total = snap.drift.score.max(0.0);
                        let shown = self.shown_score.map_or(total, |v| anim::count_up(v, total, dt));
                        self.shown_score = Some(shown);
                        drift::draw(p, &xf, snap, now, shown) | (shown != total)
                    }
                    DriftStyle::PositionGain => {
                        // A newly closed window (different `at`) restarts the count from 0; the
                        // value then holds until the next one. The first frame snaps, like Total.
                        let (at, target) = (snap.drift.chip.map(|c| c.at), drift::gain_target(snap));
                        let shown = match self.shown_gain {
                            Some((prev, v)) if prev == at => anim::count_up(v, target, dt),
                            Some(_) => anim::count_up(0.0, target, dt),
                            None => target,
                        };
                        self.shown_gain = Some((at, shown));
                        drift::draw_position_gain(p, &xf, snap, now, shown) | (shown != target)
                    }
                },
                Module::Race => race::draw(p, &xf, snap, now),
            };
        }
        animating | notify::draw(p, screen, snap, now, self.fade, s)
    }
}

/// The enabled modules with their cells and design sizes, in the layout's input form.
pub fn modules(snap: &HudSnapshot) -> Vec<(Module, HudCell, egui::Vec2)> {
    let cfg = &*snap.cfg;
    let mut items = Vec::with_capacity(3);
    if cfg.minimap_on {
        items.push((Module::Map, cfg.minimap_cell, minimap::SIZE));
    }
    if cfg.cluster_on {
        let size = match cfg.cluster_style {
            ClusterStyle::Pill => cluster::PILL_SIZE,
            ClusterStyle::Halo => cluster::HALO_SIZE,
        };
        items.push((Module::Cluster, cfg.cluster_cell, size));
    }
    // Race and drift share the slot (D25/D27): the drift counter takes it while drifting.
    // why: race_position 0 = free roam, no race to show (a stock HUD shows nothing there);
    // the slot counts as empty so stacked modules close the gap.
    // why: score 0 and best 0 = no drift event (free roam after one: `current_lap` drops to
    // 0, and the classifier stays in Drift since flat windows are no evidence), so X1′
    // would sit on "0" forever; the same free-roam rule as R1′'s position 0.
    let has_drift = snap.drift.score != 0.0 || snap.drift.best != 0.0;
    let slot = match snap.mode {
        HudMode::Drift => (cfg.drift_on && has_drift).then_some(drift::SIZE),
        HudMode::Race => (cfg.race_on && snap.pkt.race_position != 0).then_some(race::SIZE),
    };
    if let Some(size) = slot {
        items.push((Module::Race, cfg.race_cell, size));
    }
    items
}

#[cfg(test)]
mod tests;
