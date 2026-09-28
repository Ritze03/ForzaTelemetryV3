//! Time-based animation curves (round-4 spec sheet, "Timings and rules"). Pure functions of
//! an explicit `now` / age in [`hud_clock`](crate::overlay::snapshot::hud_clock) seconds, so
//! tests and the PNG harness pin time. No egui `animate_*`.

/// Show/hide fade, seconds (mockup `.screen{transition:opacity .16s linear}`).
pub const FADE_SECS: f32 = 0.16;
/// Gear-change pulse length.
pub const PULSE_SECS: f64 = 0.25;
/// D15 place-change layer: in, hold until, out until.
pub const PLACE_IN: f64 = 0.15;
pub const PLACE_HOLD: f64 = 2.4;
pub const PLACE_SECS: f64 = 3.0;
/// D21 lap-completion hold.
pub const LAP_HOLD_SECS: f64 = 4.0;
/// Drift gain chip: in, hold until, out until.
pub const CHIP_IN: f64 = 0.3;
pub const CHIP_HOLD: f64 = 3.0;
pub const CHIP_SECS: f64 = 3.6;
/// Chip text slide distance, design px.
pub const CHIP_SLIDE: f32 = 8.0;

/// Seconds since `at`, if it happened and is not in the future.
pub fn age(at: Option<f64>, now: f64) -> Option<f64> {
    at.map(|t| now - t).filter(|a| *a >= 0.0)
}

/// True while `at` is less than `secs` old.
pub fn within(at: Option<f64>, now: f64, secs: f64) -> bool {
    age(at, now).is_some_and(|a| a < secs)
}

/// Shift flash phase: on for 100 ms, off for 100 ms (10 Hz toggle on the HUD clock).
pub fn flash_on(now: f64) -> bool {
    (now * 10.0).floor().rem_euclid(2.0) == 0.0
}

/// D15 place-change layer alpha: 0 → 1 over 0.15 s ease-out (1 − (1 − x)²), hold to 2.4 s,
/// 1 → 0 by 3.0 s ease-in (1 − x²). The mockup's `placeAlpha`.
pub fn place_alpha(age: f64) -> f32 {
    if age.is_nan() || !(0.0..PLACE_SECS).contains(&age) {
        return 0.0;
    }
    if age < PLACE_IN {
        let x = age / PLACE_IN;
        return (1.0 - (1.0 - x) * (1.0 - x)) as f32;
    }
    if age < PLACE_HOLD {
        return 1.0;
    }
    let x = (age - PLACE_HOLD) / (PLACE_SECS - PLACE_HOLD);
    (1.0 - x * x) as f32
}

/// Drift gain chip: `(alpha, text y offset in design px)`. Fade + slide in over 0.3 s from
/// +8, hold to 3.0 s, fade + slide out to −8 by 3.6 s. Linear, like the mockup.
pub fn chip(age: f64) -> (f32, f32) {
    if age.is_nan() || !(0.0..CHIP_SECS).contains(&age) {
        (0.0, 0.0)
    } else if age < CHIP_IN {
        let a = (age / CHIP_IN) as f32;
        (a, CHIP_SLIDE * (1.0 - a))
    } else if age < CHIP_HOLD {
        (1.0, 0.0)
    } else {
        let a = 1.0 - ((age - CHIP_HOLD) / (CHIP_SECS - CHIP_HOLD)) as f32;
        (a, -CHIP_SLIDE * (1.0 - a))
    }
}

/// One count-up step: `shown += (total − shown) × min(1, 6·dt)`, snapping to the total within
/// 1 point; a lower total (new event, reset) snaps down at once.
pub fn count_up(shown: f32, total: f32, dt: f32) -> f32 {
    if total <= shown {
        return total;
    }
    let next = shown + (total - shown) * (6.0 * dt).clamp(0.0, 1.0);
    if total - next < 1.0 {
        total
    } else {
        next
    }
}

/// One step of the global show/hide fade toward `target` (0 or 1), linear over
/// [`FADE_SECS`]; `enabled == false` (the Fade setting off) jumps straight there.
pub fn fade_step(cur: f32, target: f32, dt: f32, enabled: bool) -> f32 {
    if !enabled {
        return target;
    }
    let step = dt / FADE_SECS;
    if cur < target {
        (cur + step).min(target)
    } else {
        (cur - step).max(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn place_fade_curve_key_times() {
        assert!(close(place_alpha(0.0), 0.0));
        // Ease-out: fast start (half way in time is ¾ of the way in alpha).
        assert!(close(place_alpha(0.075), 0.75));
        assert!(close(place_alpha(0.15), 1.0));
        assert!(close(place_alpha(1.0), 1.0));
        assert!(close(place_alpha(2.4), 1.0));
        // Ease-in out: half way (2.7 s) is 1 − 0.5² = 0.75.
        assert!(close(place_alpha(2.7), 0.75));
        assert!(close(place_alpha(3.0), 0.0));
        assert!(close(place_alpha(5.0), 0.0));
        assert!(close(place_alpha(-0.1), 0.0));
        assert!(close(place_alpha(f64::NAN), 0.0));
    }

    #[test]
    fn chip_show_hide_timing() {
        assert_eq!(chip(0.0), (0.0, CHIP_SLIDE));
        let (a, off) = chip(0.15);
        assert!(close(a, 0.5) && close(off, 4.0));
        assert_eq!(chip(0.3), (1.0, 0.0));
        assert_eq!(chip(2.99), (1.0, 0.0));
        let (a, off) = chip(3.3);
        assert!(close(a, 0.5) && close(off, -4.0));
        assert_eq!(chip(3.6), (0.0, 0.0));
        assert_eq!(chip(10.0), (0.0, 0.0));
        assert_eq!(chip(-1.0), (0.0, 0.0));
    }

    #[test]
    fn count_up_eases_then_snaps() {
        // 60 Hz: 10 % of the gap per frame.
        let s = count_up(0.0, 1000.0, 1.0 / 60.0);
        assert!(close(s, 100.0));
        // Converges and snaps exactly onto the total.
        let mut s = 0.0;
        let mut frames = 0;
        while s != 1000.0 {
            s = count_up(s, 1000.0, 1.0 / 60.0);
            frames += 1;
            assert!(frames < 200, "never settled");
        }
        assert!(frames > 30, "too fast: {frames}");
        // A long gap (no frames) jumps at most to the total.
        assert_eq!(count_up(0.0, 500.0, 2.0), 500.0);
        // A drop (new event) snaps down.
        assert_eq!(count_up(800.0, 20.0, 0.016), 20.0);
        // Zero dt holds.
        assert_eq!(count_up(10.0, 500.0, 0.0), 10.0);
    }

    #[test]
    fn fade_steps_and_snaps() {
        let mut a = 0.0;
        let mut n = 0;
        while a < 1.0 {
            a = fade_step(a, 1.0, 0.016, true);
            n += 1;
        }
        assert!((10..=11).contains(&n), "{n}"); // 0.16 s at 60 Hz (± float rounding)
        assert_eq!(fade_step(0.5, 0.0, 1.0, true), 0.0);
        assert_eq!(fade_step(0.0, 1.0, 0.0, false), 1.0);
        assert_eq!(fade_step(1.0, 0.0, 0.0, false), 0.0);
    }

    #[test]
    fn flash_toggles_every_100ms() {
        assert!(flash_on(0.05));
        assert!(!flash_on(0.15));
        assert!(flash_on(0.25));
        assert!(!flash_on(12.35));
    }

    #[test]
    fn ages() {
        assert_eq!(age(None, 5.0), None);
        assert_eq!(age(Some(6.0), 5.0), None);
        assert!(within(Some(4.9), 5.0, 0.25));
        assert!(!within(Some(4.0), 5.0, 0.25));
    }
}
