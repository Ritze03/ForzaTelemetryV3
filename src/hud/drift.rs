//! Drift counter (pill counter, R1′'s size so the two swap in one slot). Two styles
//! ([`DriftStyle`](crate::config::DriftStyle)): Position + Gain (default; R1′'s position
//! cap, the last window's "+N" counting up on the right) and Total (the round-4 spec sheet's
//! X1′). In drift events FH6 sends the score in `current_lap` (D27); the listener hands it
//! over as [`DriftInfo`](crate::overlay::snapshot::DriftInfo).
//!
//! why Position + Gain is the default: FH6's own drift score UI can't be hidden, so a second
//! total on screen is redundant (user, task 25); Total stays for if that ever changes.

use egui::Painter;

use super::anim;
use super::col;
use super::fonts::W800;
use super::prims::{self, Anchor, Cells, TextStyle, Xf};
use super::race::{self, CAP, CAP_R};
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

/// Draw X1′ (Total style) with `shown` as the (counted-up) total. Returns true while
/// animating (chip, bar, dot).
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

    animating | bar(p, xf, snap, now)
}

/// Left edge of the "+N" text area in Position + Gain: past the scoring dot (x 68–76), 4 px
/// clear of it. Wider numbers shrink to fit rather than run into the dot.
const PG_TEXT_LEFT: f32 = 80.0;
/// Scoring dot centre in Position + Gain: its left edge lines up with the bar's (x 68), and
/// it sits on the digits' vertical centre (baseline 32, cap height ≈ 0.8 × 30 px).
const PG_DOT: [f32; 2] = [72.0, 20.0];

/// Draw the Position + Gain style: R1′'s position cap (with the D15 place-change backdrop),
/// the last closed window's "+N" (`shown`, counting up from 0 in the caller) right-aligned,
/// the scoring dot left of it and the window bar under it. A window that closed with no gain
/// (or none closed yet) shows a dimmed "+0". Returns true while animating (place layer, dot,
/// bar); the count-up itself is the caller's.
pub fn draw_position_gain(p: &Painter, xf: &Xf, snap: &HudSnapshot, now: f64, shown: f32) -> bool {
    let cfg = &*snap.cfg;
    prims::rounded(p, xf, [0.0, 0.0, 196.0, 46.0], [23.0; 4], col::plate(cfg.plate_opacity));
    let mut animating = race::position_cap(p, xf, snap, now);

    let drifting = scoring(snap, now);
    animating |= drifting; // the dot's grey edge has no other redraw trigger
    p.circle_filled(xf.p(PG_DOT[0], PG_DOT[1]), xf.l(4.0), xf.c(if drifting { col::AMBER } else { col::DOT }));

    // why "+0" dimmed rather than holding the previous gain: a held "+4,039" after a window
    // that scored nothing would read as a fresh gain; a grey "+0" says "that window: nothing".
    let n = shown.max(0.0).floor() as i64;
    let txt = format!("+{}", prims::thousands(n));
    let run = prims::layout(p, xf.s, &txt, &TOTAL);
    let max_w = 186.0 - PG_TEXT_LEFT;
    let st = if run.width / xf.s > max_w { TextStyle { size: TOTAL.size * max_w * xf.s / run.width, ..TOTAL } } else { TOTAL };
    let colour = if gain_target(snap) >= 1.0 { col::INK } else { col::DIM };
    prims::text(p, xf, 186.0, 32.0, Anchor::Right, &txt, &st, colour);

    animating | bar(p, xf, snap, now)
}

/// The "+N" the Position + Gain style counts up to: the most recently closed window's gain,
/// rounded (0 before the first window closes).
pub fn gain_target(snap: &HudSnapshot) -> f32 {
    snap.drift.chip.map_or(0.0, |c| c.gain.max(0.0).round())
}

/// Window progress bar (68, 38, 118 × 3): time since the window started ÷ interval. The bar
/// keeps cycling while not scoring (the spec's "Not scoring" figure has it at 0.8). Returns
/// true while a window runs.
fn bar(p: &Painter, xf: &Xf, snap: &HudSnapshot, now: f64) -> bool {
    let d = &snap.drift;
    if !snap.cfg.drift_bar {
        return false;
    }
    prims::rounded(p, xf, [68.0, 38.0, 118.0, 3.0], [0.0; 4], col::TRACK);
    let Some(t0) = d.window_start else { return false };
    let f = if d.interval > 0.0 { ((now - t0) / d.interval as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
    if f > 0.0 {
        prims::rounded(p, xf, [68.0, 38.0, 118.0 * f, 3.0], [0.0; 4], col::AMBER);
    }
    true
}

/// The score rose within the last [`DRIFT_ACTIVE_SECS`].
pub fn scoring(snap: &HudSnapshot, now: f64) -> bool {
    snap.drift.last_rise_at.is_some_and(|t| (0.0..DRIFT_ACTIVE_SECS).contains(&(now - t)))
}
