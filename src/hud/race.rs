//! R1′ race block (minimal, no "P"), 196 × 46. Round-4 spec sheet.

use egui::{vec2, Painter, Vec2};

use super::anim;
use super::col;
use super::fonts::{W800, W900};
use super::prims::{self, Anchor, Cells, TextStyle, Xf};
use crate::i18n::tr;
use crate::overlay::snapshot::HudSnapshot;

pub const SIZE: Vec2 = vec2(196.0, 46.0);

/// The cap: 60 × 46, left side round.
pub const CAP: [f32; 4] = [0.0, 0.0, 60.0, 46.0];
pub const CAP_R: [f32; 4] = [23.0, 4.0, 4.0, 23.0];

const POS: TextStyle = TextStyle { family: W900, size: 38.0, tracking: 0.0, cells: Cells::Widest, shadow: true };
const LABEL: TextStyle = TextStyle { family: W800, size: 12.0, tracking: 12.0 * 0.14, cells: Cells::Off, shadow: true };
const TIME: TextStyle = TextStyle { family: W800, size: 26.0, tracking: 0.0, cells: Cells::Widest, shadow: true };
/// Lap delta chip text (not in the round-4 sheet; styled after round 3's `.rc-dl`, sized to
/// R1′'s label row).
const DELTA: TextStyle = TextStyle { family: W800, size: 12.0, tracking: 0.0, cells: Cells::Widest, shadow: false };

/// The position cap: backing, D15 place-change layer and the position ("–" for 0). Shared
/// with the drift counter's Position + Gain style. Returns true while the layer animates.
pub fn position_cap(p: &Painter, xf: &Xf, snap: &HudSnapshot, now: f64) -> bool {
    let cfg = &*snap.cfg;
    prims::rounded(p, xf, CAP, CAP_R, col::cap(cfg.plate_opacity));

    // D15: green/red colour layer over the cap, alpha-lerped through the 3 s curve.
    let mut animating = false;
    if let Some(pc) = snap.events.place_change {
        let age = now - pc.at;
        animating |= (0.0..anim::PLACE_SECS).contains(&age);
        let a = anim::place_alpha(age);
        if cfg.place_colour && a > 0.0 {
            prims::rounded(p, &xf.fade(a), CAP, CAP_R, if pc.gained { col::GAIN } else { col::LOSS });
        }
    }

    // Position, centred on x 31.5 (the cap minus its 3 px left padding), baseline 37.
    let pos = snap.pkt.race_position;
    let pos = if pos == 0 { "\u{2013}".to_string() } else { pos.to_string() };
    prims::text(p, xf, 31.5, 37.0, Anchor::Center, &pos, &POS, col::INK);
    animating
}

/// Draw R1′. Returns true while animating (place-change layer, lap hold).
pub fn draw(p: &Painter, xf: &Xf, snap: &HudSnapshot, now: f64) -> bool {
    let (pkt, cfg) = (&snap.pkt, &*snap.cfg);
    prims::rounded(p, xf, [0.0, 0.0, 196.0, 46.0], [23.0; 4], col::plate(cfg.plate_opacity));
    let mut animating = position_cap(p, xf, snap, now);

    // D21 lap hold: 4 s of LAST LAP with last_lap, green if it set the best.
    let hold = anim::within(snap.events.lap_completed_at, now, anim::LAP_HOLD_SECS);
    animating |= hold;
    let (label, time, time_col) = if hold {
        let best = pkt.best_lap > 0.0 && (pkt.last_lap - pkt.best_lap).abs() < 5e-4;
        (tr("LAST LAP").to_string(), prims::lap_time(pkt.last_lap), if best { col::BEST } else { col::INK })
    } else {
        // lap_number is 0-based (D21).
        (format!("{} {}", tr("LAP"), pkt.lap_number as u32 + 1), prims::lap_time(pkt.current_lap), col::INK)
    };
    prims::text(p, xf, 70.0, 15.0, Anchor::Left, &label, &LABEL, col::DIM);
    prims::text(p, xf, 70.0, 42.0, Anchor::Left, &time, &TIME, time_col);

    // Lap delta chip, right-aligned in the label row (hidden during the hold: the snapshot's
    // delta already belongs to the new lap).
    if let Some(d) = snap.lap_delta.filter(|d| cfg.lap_delta && !hold && d.is_finite()) {
        let txt = prims::delta(d);
        let run = prims::layout(p, xf.s, &txt, &DELTA);
        let w = run.width / xf.s + 12.0;
        let x = 186.0 - w;
        prims::rounded(p, xf, [x, 2.0, w, 16.0], [8.0; 4], if d < 0.0 { col::GAIN } else { col::LOSS });
        prims::draw_run(p, xf, &run, xf.p(x + 6.0, 15.0), false, col::WHITE);
    }
    animating
}

#[cfg(test)]
mod tests {
    use super::*;

    /// German labels are wider than English ("RUNDE nn" vs "LAP nn"; "LETZTE RUNDE" vs
    /// "LAST LAP"). The label must clear the right-aligned lap delta chip with room to spare.
    /// "LETZTE RUNDE" never meets the chip: the chip hides during the lap hold.
    #[test]
    fn german_lap_label_clears_the_delta_chip() {
        let ctx = egui::Context::default();
        super::super::fonts::install(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let p = ctx.layer_painter(egui::LayerId::background());
            let w = |t: &str, st: &TextStyle| prims::layout(&p, 1.0, t, st).width;
            for label in ["RUNDE 99", "RUNDE 199", "LAP 199"] {
                for d in [9.99f32, -9.99, 99.99] {
                    let label_end = 70.0 + w(label, &LABEL);
                    let chip_x = 186.0 - (w(&prims::delta(d), &DELTA) + 12.0);
                    println!("{label:>10} ends {label_end:.1}, chip {} starts {chip_x:.1}", prims::delta(d));
                    assert!(chip_x - label_end >= 4.0, "{label} / {}: {label_end} vs {chip_x}", prims::delta(d));
                }
            }
            // The hold label alone fits the plate (right edge 186).
            let hold_end = 70.0 + w("LETZTE RUNDE", &LABEL);
            println!("LETZTE RUNDE ends {hold_end:.1}");
            assert!(hold_end <= 186.0);
        });
    }
}
