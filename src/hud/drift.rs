//! X1′ drift counter (pill counter, R1′'s size so the two swap in one slot). Round-4 spec
//! sheet. In drift events FH6 sends the score in `current_lap` (D27); the listener hands it
//! over as [`DriftInfo`](crate::overlay::snapshot::DriftInfo).

use egui::Painter;

use super::anim;
use super::col;
use super::fonts::W800;
use super::prims::{self, Anchor, Cells, TextStyle, Xf};
use super::race::{CAP, CAP_R};
use crate::i18n::tr;
use crate::overlay::snapshot::HudSnapshot;

/// R1′'s size: the two swap in one slot.
pub const SIZE: egui::Vec2 = super::race::SIZE;

const LABEL: TextStyle = TextStyle { family: W800, size: 11.0, tracking: 11.0 * 0.14, cells: Cells::Off, shadow: false };
const GAIN: TextStyle = TextStyle { family: W800, size: 16.0, tracking: 0.0, cells: Cells::Widest, shadow: false };
/// The live dot is amber this long (s) after the score last rose. why: the mockup lights it
/// while points are being scored (`driftRate > 0`); packets only show the score stepping,
/// with flat stretches between steps mid-drift, so a short hold keeps it from flickering.
/// Gone grey means "not scoring" (the spec's figure). A guess until checked on real packets.
pub const DRIFT_ACTIVE_SECS: f64 = 1.0;

/// The gain text drops to 14 px above 7 characters so "+12,345" still fits the cap.
const GAIN_SMALL: TextStyle = TextStyle { size: 14.0, ..GAIN };
const TOTAL: TextStyle = TextStyle { family: W800, size: 30.0, tracking: 0.0, cells: Cells::Widest, shadow: true };

/// Draw X1′ with `shown` as the (counted-up) total. Returns true while animating (chip, bar).
pub fn draw(p: &Painter, xf: &Xf, snap: &HudSnapshot, now: f64, shown: f32) -> bool {
    let (d, cfg) = (&snap.drift, &*snap.cfg);
    prims::rounded(p, xf, [0.0, 0.0, 196.0, 46.0], [23.0; 4], col::plate(cfg.plate_opacity));
    prims::rounded(p, xf, CAP, CAP_R, col::cap(cfg.plate_opacity));

    // Gain chip in the cap: amber layer over it, "+N" sliding in; "+0" never shows.
    let mut animating = false;
    let (a, off, gain) = match d.chip.filter(|c| c.gain.round() >= 1.0) {
        Some(c) => {
            let age = now - c.at;
            animating |= (0.0..anim::CHIP_SECS).contains(&age);
            let (a, off) = anim::chip(age);
            (a, off, c.gain)
        }
        None => (0.0, 0.0, 0.0),
    };

    // Normal cap (fades out while the gain shows): live dot, "DRIFT".
    // The bar keeps cycling while not scoring (the spec's "Not scoring" figure has it at 0.8).
    let drifting = scoring(snap, now);
    // Redraw until the dot goes grey: that edge has no other trigger when the bar is off.
    animating |= drifting;
    let norm = xf.fade(1.0 - a);
    if norm.a > 0.0 {
        p.circle_filled(norm.p(32.0, 15.0), norm.l(4.0), norm.c(if drifting { col::AMBER } else { col::DOT }));
        prims::text(p, &norm, 32.0, 34.0, Anchor::Center, tr("DRIFT"), &LABEL, col::DIM_CAP);
    }
    if a > 0.0 {
        let amb = xf.fade(a);
        prims::rounded(p, &amb, CAP, CAP_R, col::AMBER);
        let txt = format!("+{}", prims::thousands(gain.round() as i64));
        let st = if txt.chars().count() > 7 { &GAIN_SMALL } else { &GAIN };
        prims::text(p, &amb, 32.0, 29.0 + off, Anchor::Center, &txt, st, col::AMBER_TEXT);
    }

    // Total, right edge x 186, baseline 32.
    let total = prims::thousands(shown.max(0.0).floor() as i64);
    prims::text(p, xf, 186.0, 32.0, Anchor::Right, &total, &TOTAL, col::INK);

    // Window progress bar (68, 38, 118 × 3): time since the window started ÷ interval.
    if cfg.drift_bar {
        prims::rounded(p, xf, [68.0, 38.0, 118.0, 3.0], [0.0; 4], col::TRACK);
        if let Some(t0) = d.window_start {
            let f = if d.interval > 0.0 { ((now - t0) / d.interval as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
            if f > 0.0 {
                prims::rounded(p, xf, [68.0, 38.0, 118.0 * f, 3.0], [0.0; 4], col::AMBER);
            }
            animating = true;
        }
    }
    animating
}

/// The score rose within the last [`DRIFT_ACTIVE_SECS`].
pub fn scoring(snap: &HudSnapshot, now: f64) -> bool {
    snap.drift.last_rise_at.is_some_and(|t| (0.0..DRIFT_ACTIVE_SECS).contains(&(now - t)))
}
