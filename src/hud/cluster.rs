//! Drive cluster: D1a (pill, level bar) and D3a′ (halo, thick ring). Round-4 spec sheet;
//! all numbers are design px relative to the widget's top-left.

use egui::{pos2, vec2, Color32, Painter, Vec2};

use super::anim;
use super::col;
use super::fonts::{W800, W900};
use super::prims::{self, Anchor, Cells, TextStyle, Xf};
use crate::i18n::tr;
use crate::overlay::snapshot::{DriveMode, HudSnapshot};

pub const PILL_SIZE: Vec2 = vec2(184.0, 46.0);
pub const HALO_SIZE: Vec2 = vec2(112.0, 112.0);

/// D1a rev bar: 16 parallelograms.
const BAR_N: usize = 16;
/// D3a′ ring: 24 sectors on −130°…+130°, 22 % of each pitch is gap.
const RING_N: usize = 24;
const RING_FROM: f32 = -130.0;
const RING_TO: f32 = 130.0;
const RING_GAP: f32 = 0.22;

/// What both clusters show, derived once per frame.
struct Cl {
    /// Auto-gearbox drive-mode letter drawn before `gear` (D/S/R), if any.
    mode: Option<char>,
    gear: String,
    speed: String,
    /// The unit label: "KM/H"/"MPH", or with D13 the engine rpm rounded to 10.
    label: String,
    label_is_rpm: bool,
    rpm: f32,
    idle: f32,
    max: f32,
    redline: f32,
    /// Shift flash, "on" phase right now.
    flash: bool,
    /// Gear-change pulse running.
    pulse: bool,
    /// In the redline zone.
    red: bool,
    /// Something here changes without a new packet (flash toggling, pulse running).
    animating: bool,
}

/// Gear text: 0 → reverse "R", 1–10 → the number, else neutral "N". With the auto gearbox
/// on, a forward gear also gets its drive-mode letter (D Street, S Sport, R Race). Reverse
/// and neutral never do, so a lone "R" is always reverse and "R5" always Race mode.
pub fn gear_label(gear: u8, auto: Option<DriveMode>) -> (Option<char>, String) {
    match gear {
        0 => (None, "R".into()),
        g @ 1..=10 => {
            let letter = auto.map(|m| match m {
                DriveMode::Street => 'D',
                DriveMode::Sport => 'S',
                DriveMode::Race => 'R',
            });
            (letter, g.to_string())
        }
        _ => (None, "N".into()),
    }
}

/// Gear glyph sizes and placement for one cluster, design px.
struct GearSpec {
    cx: f32,
    /// With `centred`, where the digits' ink box is centred (the baseline follows from the
    /// size, so a smaller auto label stays centred); else the baseline itself.
    y: f32,
    centred: bool,
    plain: TextStyle,
    /// Digits when a mode letter precedes them.
    auto: TextStyle,
    /// The mode letter (baseline-aligned with the digits).
    letter: TextStyle,
    /// Widest the letter + digits may get; "D10" is squeezed to fit.
    max_w: f32,
}

/// Big Shoulders' digit/cap height ÷ font size (measured: 25.7 px ink at 32 px).
const CAP: f32 = 0.80;
/// Letter → digits gap, design px.
const LETTER_GAP: f32 = 1.5;

impl GearSpec {
    fn baseline(&self, size: f32) -> f32 {
        if self.centred {
            self.y + size * CAP / 2.0
        } else {
            self.y
        }
    }
}

/// Draw the gear (with its mode letter, if any) centred at `g.cx`.
fn draw_gear(p: &Painter, xf: &Xf, g: &GearSpec, cl: &Cl, shadow: bool, color: Color32) {
    let Some(letter) = cl.mode else {
        let base = g.baseline(g.plain.size);
        prims::text(p, xf, g.cx, base, Anchor::Center, &cl.gear, &TextStyle { shadow, ..g.plain }, color);
        return;
    };
    // Fixed letter cell (widest of D/S/R) and digit cells: nothing moves on a gear change.
    let fit = |k: f32| {
        let num = TextStyle { size: g.auto.size * k, shadow, ..g.auto };
        let let_st = TextStyle { size: g.letter.size * k, shadow, ..g.letter };
        let cell = prims::widest(p, xf.s, &let_st, "DSR");
        let run = prims::layout(p, xf.s, &cl.gear, &num);
        let w = cell + xf.l(LETTER_GAP * k) + run.width;
        (num, let_st, cell, run, w)
    };
    let (mut num, mut let_st, mut cell, mut run, mut w) = fit(1.0);
    if w > xf.l(g.max_w) {
        (num, let_st, cell, run, w) = fit(xf.l(g.max_w) / w);
    }
    let base = xf.p(g.cx, g.baseline(num.size / xf.s));
    let x0 = base.x - w / 2.0;
    let lrun = prims::layout(p, xf.s, &letter.to_string(), &let_st);
    // ponytail: letter at 85 % of the digit colour's alpha, so the gear number leads.
    prims::draw_run(p, xf, &lrun, pos2(x0 + (cell - lrun.width) / 2.0, base.y), let_st.shadow, color.gamma_multiply(0.85));
    prims::draw_run(p, xf, &run, pos2(x0 + w - run.width, base.y), num.shadow, color);
}

/// The live speed in the display unit (km/h or mph), rounded.
pub fn speed(snap: &HudSnapshot) -> i64 {
    (snap.pkt.speed.max(0.0) * if snap.use_mph { 2.237 } else { 3.6 }).round() as i64
}

/// `speed` is the number to show: [`speed`], or the held one with `speed_hold`.
fn derive(snap: &HudSnapshot, now: f64, speed: i64) -> Cl {
    let (p, cfg) = (&snap.pkt, &*snap.cfg);
    let (mode, gear) = gear_label(p.gear, snap.auto_gear);
    let rpm = p.current_engine_rpm.max(0.0);
    let (label, label_is_rpm) = if cfg.rpm_label {
        (((rpm / 10.0).round() as i64 * 10).to_string(), true)
    } else {
        (tr(if snap.use_mph { "MPH" } else { "KM/H" }).to_string(), false)
    };
    let valid = p.engine_max_rpm > 0.0;
    let shifting = cfg.shift_flash && valid && snap.shift_rpm > 0.0 && rpm >= snap.shift_rpm;
    let pulsing = cfg.gear_pulse && anim::within(snap.events.gear_changed_at, now, anim::PULSE_SECS);
    Cl {
        mode,
        gear,
        speed: speed.to_string(),
        label,
        label_is_rpm,
        rpm,
        idle: p.engine_idle_rpm.max(0.0),
        max: p.engine_max_rpm,
        redline: snap.redline_rpm,
        flash: shifting && anim::flash_on(now),
        pulse: pulsing,
        red: valid && snap.redline_rpm > 0.0 && rpm >= snap.redline_rpm,
        animating: shifting || pulsing,
    }
}

impl Cl {
    /// Fill level idle → max, 0..=1 (the mockup's `wing`).
    fn level(&self) -> f32 {
        if self.max > self.idle {
            ((self.rpm - self.idle) / (self.max - self.idle)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
    /// Segment `i` of `n`: lit when the level reaches its top, redline when the rpm at its
    /// centre is in the zone (mockup `rzAt(i, n, idle)`).
    fn segment(&self, i: usize, n: usize) -> egui::Color32 {
        let lit = self.level() >= (i + 1) as f32 / n as f32 - 1e-6;
        let rz = self.max > self.idle
            && self.redline > 0.0
            && self.idle + (i as f32 + 0.5) / n as f32 * (self.max - self.idle) >= self.redline;
        match (lit, rz) {
            (true, _) if self.flash => col::SHIFT,
            (true, true) => col::RED,
            (true, false) => col::INK,
            (false, true) => col::OFF_RED,
            (false, false) => col::OFF,
        }
    }
}

const GEAR_PILL: TextStyle = TextStyle { family: W900, size: 32.0, tracking: 0.0, cells: Cells::Widest, shadow: true };
// why (user: gear sat too high): the spec's baseline 35 put the ink at 9.3–35 (centre 22.2) in
// the cell centred on 23; it read high in-game at 1080p. Ink was moved 1 px below the cell
// centre (y 24), then back up 1 px because it then read 1 px too low: y 23 (ink centred on
// the cell).
const PILL_GEAR: GearSpec = GearSpec {
    cx: 23.0,
    y: 23.0,
    centred: true,
    plain: GEAR_PILL,
    auto: TextStyle { size: 26.0, ..GEAR_PILL },
    letter: TextStyle { family: W900, size: 22.0, tracking: 0.0, cells: Cells::Off, shadow: true },
    max_w: 30.0,
};
const SPEED_PILL: TextStyle = TextStyle { family: W800, size: 27.0, tracking: 0.0, cells: Cells::Widest, shadow: true };
const UNIT_PILL: TextStyle = TextStyle { family: W800, size: 12.0, tracking: 12.0 * 0.12, cells: Cells::Off, shadow: false };
const RPM_PILL: TextStyle = TextStyle { family: W800, size: 12.0, tracking: 0.0, cells: Cells::Fixed(6.0), shadow: false };

/// D1a, 184 × 46. Returns true while animating.
pub fn draw_pill(p: &Painter, xf: &Xf, snap: &HudSnapshot, now: f64, speed: i64) -> bool {
    let cl = derive(snap, now, speed);
    prims::rounded(p, xf, [0.0, 0.0, 184.0, 46.0], [23.0; 4], col::plate(snap.cfg.plate_opacity));

    // Gear cell: circle r 19 at (23, 23). Flash fills blue; else the pulse fills ink (dark
    // numeral, no shadow); the redline adds a 2 px inner ring (both may show together).
    let c = xf.p(23.0, 23.0);
    let fill = if cl.flash {
        col::SHIFT_FILL
    } else if cl.pulse {
        col::INK
    } else {
        col::CELL
    };
    p.circle_filled(c, xf.l(19.0), xf.c(fill));
    if cl.red && !cl.flash {
        prims::ring(p, xf, [23.0, 23.0], 18.0, 2.0, col::RED); // inset box-shadow: r 17–19
    }
    let pulse_dark = cl.pulse && !cl.flash;
    draw_gear(p, xf, &PILL_GEAR, &cl, !pulse_dark, if pulse_dark { col::ON_INK } else { col::INK });

    // Speed (pen x 52, baseline 25), then the unit label 5 px after its advance.
    let w = prims::text(p, xf, 52.0, 25.0, Anchor::Left, &cl.speed, &SPEED_PILL, col::INK);
    let st = if cl.label_is_rpm { &RPM_PILL } else { &UNIT_PILL };
    prims::text(p, xf, 52.0 + w + 5.0, 25.0, Anchor::Left, &cl.label, st, col::DIM_UNIT);

    // Rev bar: pitch 7.5, width 5.5, height 5 → 12, bottom y 41, top shifted right 0.2 × h.
    for i in 0..BAR_N {
        let h = 5.0 + i as f32 * 7.0 / 15.0;
        let (x, sk) = (52.0 + i as f32 * 7.5, h * 0.2);
        let pts = [[x, 41.0], [x + 5.5, 41.0], [x + 5.5 + sk, 41.0 - h], [x + sk, 41.0 - h]];
        prims::poly(p, xf, &pts, cl.segment(i, BAR_N));
    }
    cl.animating
}

const GEAR_HALO: TextStyle = TextStyle { family: W900, size: 44.0, tracking: 0.0, cells: Cells::Widest, shadow: true };
/// The spec's baseline 63 for both labels: the auto label keeps the gap to the speed, its
/// smaller glyphs just lose height at the top.
const HALO_GEAR: GearSpec = GearSpec {
    cx: 56.0,
    y: 63.0,
    centred: false,
    plain: GEAR_HALO,
    auto: TextStyle { size: 38.0, ..GEAR_HALO },
    letter: TextStyle { family: W900, size: 30.0, tracking: 0.0, cells: Cells::Off, shadow: true },
    max_w: 50.0,
};
const SPEED_HALO: TextStyle = TextStyle { family: W800, size: 18.0, tracking: 0.0, cells: Cells::Widest, shadow: true };
const UNIT_HALO: TextStyle = TextStyle { family: W800, size: 10.0, tracking: 10.0 * 0.16, cells: Cells::Off, shadow: false };
const RPM_HALO: TextStyle = TextStyle { family: W800, size: 10.0, tracking: 0.0, cells: Cells::Fixed(6.0), shadow: false };

/// Inner radius of ring sector `i` (D14): 47 → 40 from idle to max, outer edge fixed at 52,
/// so the sector is 5 → 12 px thick.
pub fn ring_inner(i: usize) -> f32 {
    47.0 - 7.0 * i as f32 / (RING_N - 1) as f32
}

/// Start and end angle of ring sector `i`, degrees (0° = 12 o'clock, clockwise).
pub fn ring_span(i: usize) -> [f32; 2] {
    let step = (RING_TO - RING_FROM) / RING_N as f32;
    let a0 = RING_FROM + i as f32 * step + step * RING_GAP / 2.0;
    [a0, a0 + step * (1.0 - RING_GAP)]
}

/// D3a′, Ø 112. Returns true while animating.
pub fn draw_halo(p: &Painter, xf: &Xf, snap: &HudSnapshot, now: f64, speed: i64) -> bool {
    let cl = derive(snap, now, speed);
    p.circle_filled(xf.p(56.0, 56.0), xf.l(55.0), xf.c(col::disc(snap.cfg.plate_opacity)));
    // Rim 1.5 px; the shift flash turns it 3 px blue, else the pulse 3 px ink.
    let (rim_w, rim) = if cl.flash {
        (3.0, col::SHIFT)
    } else if cl.pulse {
        (3.0, col::INK)
    } else {
        (1.5, col::RIM)
    };
    prims::ring(p, xf, [56.0, 56.0], 55.0, rim_w, rim);

    for i in 0..RING_N {
        prims::sector(p, xf, [56.0, 56.0], ring_span(i), ring_inner(i), 52.0, cl.segment(i, RING_N));
    }

    draw_gear(p, xf, &HALO_GEAR, &cl, true, col::INK);
    prims::text(p, xf, 56.0, 87.0, Anchor::Center, &cl.speed, &SPEED_HALO, col::INK);
    if cl.label_is_rpm {
        prims::text(p, xf, 56.0, 100.0, Anchor::Center, &cl.label, &RPM_HALO, col::INK);
    } else {
        prims::text(p, xf, 56.0, 100.0, Anchor::Center, &cl.label, &UNIT_HALO, col::DIM);
    }
    cl.animating
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halo_sector_thickness_grows_with_rpm() {
        assert!((52.0 - ring_inner(0) - 5.0).abs() < 1e-5);
        assert!((52.0 - ring_inner(23) - 12.0).abs() < 1e-5);
        for i in 1..RING_N {
            assert!(ring_inner(i) < ring_inner(i - 1));
        }
    }

    #[test]
    fn halo_spans_match_spec() {
        // Pitch 10.83°, sector 8.45°, gap 2.38°, symmetric about 12 o'clock.
        let [a0, a1] = ring_span(0);
        assert!((a1 - a0 - 8.45).abs() < 0.01);
        assert!((a0 - (-130.0 + 2.383 / 2.0)).abs() < 0.01);
        let [b0, _] = ring_span(1);
        assert!((b0 - a0 - 10.833).abs() < 0.01);
        let [_, z1] = ring_span(RING_N - 1);
        assert!((z1 + a0).abs() < 1e-3);
    }

    #[test]
    fn gear_labels() {
        let l = |g, m| gear_label(g, m);
        assert_eq!(l(4, None), (None, "4".to_string()));
        assert_eq!(l(0, None), (None, "R".to_string()));
        assert_eq!(l(11, None), (None, "N".to_string()));
        assert_eq!(l(4, Some(DriveMode::Street)), (Some('D'), "4".to_string()));
        assert_eq!(l(3, Some(DriveMode::Sport)), (Some('S'), "3".to_string()));
        assert_eq!(l(5, Some(DriveMode::Race)), (Some('R'), "5".to_string()));
        assert_eq!(l(10, Some(DriveMode::Street)), (Some('D'), "10".to_string()));
        // Reverse and neutral stay bare in auto, so "R" alone is always reverse.
        assert_eq!(l(0, Some(DriveMode::Race)), (None, "R".to_string()));
        assert_eq!(l(11, Some(DriveMode::Sport)), (None, "N".to_string()));
    }

    fn cl(rpm: f32) -> Cl {
        Cl {
            mode: None,
            gear: String::new(),
            speed: String::new(),
            label: String::new(),
            label_is_rpm: false,
            rpm,
            idle: 900.0,
            max: 8000.0,
            redline: 6800.0,
            flash: false,
            pulse: false,
            red: false,
            animating: false,
        }
    }

    #[test]
    fn segments_light_from_idle_and_redline_by_centre_rpm() {
        // Spec: D1a segments 13–15 are redline (0.85 × 8000, idle 900); D3a′ sectors 20–23.
        let c = cl(0.0);
        let rz: Vec<usize> = (0..BAR_N).filter(|&i| c.segment(i, BAR_N) == col::OFF_RED).collect();
        assert_eq!(rz, vec![13, 14, 15]);
        let rz: Vec<usize> = (0..RING_N).filter(|&i| c.segment(i, RING_N) == col::OFF_RED).collect();
        assert_eq!(rz, vec![20, 21, 22, 23]);
        // At idle nothing is lit; at max everything is.
        let c = cl(900.0);
        assert!((0..BAR_N).all(|i| !matches!(c.segment(i, BAR_N), x if x == col::INK || x == col::RED)));
        let c = cl(8000.0);
        assert!((0..BAR_N).all(|i| matches!(c.segment(i, BAR_N), x if x == col::INK || x == col::RED)));
        // Flash turns every lit segment blue.
        let c = Cl { flash: true, ..cl(8000.0) };
        assert!((0..BAR_N).all(|i| c.segment(i, BAR_N) == col::SHIFT));
    }
}
