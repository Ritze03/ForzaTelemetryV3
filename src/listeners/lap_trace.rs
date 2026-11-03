//! Best-lap trace and the live lap delta for the HUD race block.
//!
//! Per packet we record `(distance into the lap, current_lap)`. When `lap_number` steps up
//! by one the finished lap is promoted to "best" if it was recorded from its start and beat
//! the previous best. The delta is `current_lap − best_time_at(same distance)`, linearly
//! interpolated. 60 Hz × 120 s ≈ 7.2k samples × 8 B ≈ 58 KB per lap.
//!
//! Distance is measured from the lap's first packet (`lap_start_dist`), which handles
//! `distance_traveled` resetting per lap *and* running on per event (unverified in FH6).

/// Cap on samples per lap (10 min at 60 Hz ≈ 288 KB). A longer lap stops recording and
/// can't become best.
// ponytail: fixed cap; a free-roam "lap" never wraps, so without it the Vec would grow
// forever. Upgrade path: decimate by distance instead of dropping.
const MAX_SAMPLES: usize = 36_000;
/// `current_lap` falling by more than this (s) within a lap = restart or rewind.
const DROP_EPS: f32 = 0.05;
/// A drop to below this (s) is a restart from the line (a fresh lap); higher is a rewind.
const RESTART_BELOW: f32 = 0.5;

#[derive(Default)]
pub struct LapTrace {
    /// `(distance into lap, lap time)`, distance strictly increasing.
    cur: Vec<(f32, f32)>,
    best: Vec<(f32, f32)>,
    best_time: Option<f32>,
    lap_number: Option<u16>,
    lap_start_dist: f32,
    last_time: f32,
    /// The current lap was recorded from its start without rewinds or overflow.
    complete: bool,
}

impl LapTrace {
    /// Forget everything, best lap included (car or event change, drift mode).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The best lap's time, if one has been promoted.
    #[allow(dead_code)] // read by tests; the HUD takes `pkt.best_lap` for display
    pub fn best_time(&self) -> Option<f32> {
        self.best_time
    }

    /// Feed one racing packet; returns the live delta (s, negative = ahead of best).
    pub fn update(&mut self, lap_number: u16, distance: f32, current_lap: f32) -> Option<f32> {
        match self.lap_number {
            None => self.start_lap(lap_number, distance, current_lap),
            Some(n) if lap_number == n.wrapping_add(1) => {
                // Lap wrap: promote the finished lap if it's complete and faster.
                let t = self.last_time;
                if self.complete && self.cur.len() > 1 && self.best_time.is_none_or(|b| t < b) {
                    self.best = std::mem::take(&mut self.cur);
                    self.best_time = Some(t);
                }
                self.start_lap(lap_number, distance, current_lap);
            }
            Some(n) if lap_number != n => {
                // Went backwards or skipped: a restart or a different event. The old best
                // may be for another track, so drop it — no delta beats a wrong one.
                self.reset();
                self.start_lap(lap_number, distance, current_lap);
            }
            Some(_) if current_lap < self.last_time - DROP_EPS => {
                if current_lap < RESTART_BELOW {
                    // Restarted from the line within the same lap number.
                    self.start_lap(lap_number, distance, current_lap);
                } else {
                    // Rewind: the recorded trace no longer matches; keep the delta but don't
                    // let this lap become best.
                    self.complete = false;
                    let d = distance - self.lap_start_dist;
                    self.cur.retain(|&(cd, _)| cd < d);
                }
            }
            Some(_) => {}
        }
        self.last_time = current_lap;

        let d = distance - self.lap_start_dist;
        if self.cur.last().is_none_or(|&(ld, _)| d > ld) {
            if self.cur.len() < MAX_SAMPLES {
                self.cur.push((d, current_lap));
            } else {
                self.complete = false;
            }
        }
        interp(&self.best, d).map(|bt| current_lap - bt)
    }

    fn start_lap(&mut self, lap_number: u16, distance: f32, current_lap: f32) {
        self.lap_number = Some(lap_number);
        self.lap_start_dist = distance;
        self.cur.clear();
        // Joined mid-lap (overlay enabled, app started, packets resumed late) → not a
        // full lap. The lap timer is near 0 on a real lap start.
        self.complete = current_lap < RESTART_BELOW;
        self.last_time = current_lap;
    }
}

/// Best lap's time at distance `d`, linear between samples. `None` before the first or past
/// the last sample.
fn interp(trace: &[(f32, f32)], d: f32) -> Option<f32> {
    let i = trace.partition_point(|&(td, _)| td < d);
    let &(d1, t1) = trace.get(i)?;
    if i == 0 {
        return (d == d1).then_some(t1);
    }
    let (d0, t0) = trace[i - 1];
    Some(t0 + (t1 - t0) * (d - d0) / (d1 - d0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive one lap of `len` metres at `speed` m/s, 60 Hz, from `start_dist`.
    fn drive(tr: &mut LapTrace, lap: u16, start_dist: f32, len: f32, speed: f32) -> Option<f32> {
        let mut t = 0.0_f32;
        let mut last = None;
        while t * speed < len {
            last = tr.update(lap, start_dist + t * speed, t);
            t += 1.0 / 60.0;
        }
        last
    }

    #[test]
    fn lap_trace_first_lap_has_no_delta_then_best_is_promoted() {
        let mut tr = LapTrace::default();
        assert_eq!(drive(&mut tr, 0, 0.0, 1000.0, 50.0), None, "no best yet");
        // Lap 2 (distance keeps running per event), faster: 1000 m at 50 → 20 s best.
        let d = tr.update(1, 1000.0, 0.0);
        assert_eq!(tr.best_time().map(|b| (b - 20.0).abs() < 0.05), Some(true));
        assert_eq!(d.map(|d| d.abs() < 1e-3), Some(true), "delta at the line is ~0");
        // Half-way at 40 m/s: 12.5 s vs best 10 s → +2.5 s.
        let d = tr.update(1, 1500.0, 12.5).unwrap_or(f32::NAN);
        assert!((d - 2.5).abs() < 0.02, "delta {d}");
    }

    #[test]
    fn lap_trace_new_best_replaces_and_slower_lap_does_not() {
        let mut tr = LapTrace::default();
        drive(&mut tr, 0, 0.0, 1000.0, 50.0); // 20 s
        drive(&mut tr, 1, 0.0, 1000.0, 40.0); // 25 s — slower (distance resets per lap)
        tr.update(2, 0.0, 0.0);
        let b = tr.best_time().unwrap_or(0.0);
        assert!((b - 20.0).abs() < 0.05, "slower lap must not replace best, got {b}");
        drive(&mut tr, 2, 0.0, 1000.0, 100.0); // 10 s — new best
        tr.update(3, 0.0, 0.0);
        let b = tr.best_time().unwrap_or(0.0);
        assert!((b - 10.0).abs() < 0.05, "faster lap must become best, got {b}");
        // Ahead of best → negative delta: 500 m in 4 s vs best 5 s.
        let d = tr.update(3, 500.0, 4.0).unwrap_or(f32::NAN);
        assert!((d + 1.0).abs() < 0.02, "delta {d}");
    }

    #[test]
    fn lap_trace_restart_and_partial_laps_are_handled() {
        let mut tr = LapTrace::default();
        drive(&mut tr, 0, 0.0, 1000.0, 50.0);
        tr.update(1, 1000.0, 0.0);
        assert!(tr.best_time().is_some());
        // Event restart: lap_number goes back to 0 → best dropped.
        tr.update(0, 0.0, 0.0);
        assert_eq!(tr.best_time(), None);

        // Joining mid-lap: that lap can't become best.
        let mut tr = LapTrace::default();
        tr.update(0, 300.0, 6.0);
        tr.update(0, 800.0, 16.0);
        tr.update(1, 1000.0, 0.0);
        assert_eq!(tr.best_time(), None, "partial lap promoted");

        // Restart from the line on the same lap number resets the current lap.
        let mut tr = LapTrace::default();
        drive(&mut tr, 0, 0.0, 400.0, 50.0);
        tr.update(0, 0.0, 0.0);
        drive(&mut tr, 0, 0.0, 1000.0, 50.0);
        tr.update(1, 1000.0, 0.0);
        let b = tr.best_time().unwrap_or(0.0);
        assert!((b - 20.0).abs() < 0.05, "restarted lap should count, got {b}");
    }

    #[test]
    fn lap_trace_delta_is_none_past_best_distance() {
        let mut tr = LapTrace::default();
        drive(&mut tr, 0, 0.0, 1000.0, 50.0);
        tr.update(1, 1000.0, 0.0);
        assert_eq!(tr.update(1, 2500.0, 40.0), None);
    }
}
