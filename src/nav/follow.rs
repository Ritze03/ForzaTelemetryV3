//! Following a route: where the car is along it, how far and how long is left, whether it left
//! the route and whether it arrived. Pure (no threads, no clocks: the caller hands in the packet
//! time step), so the 50 m / 2 s rule is testable to the millisecond. The runtime that owns a
//! [`Follower`] per route is `state.rs`.
//!
//! * **Progress** keeps the segment the car is on and scans a window around it (`WINDOW_BACK` /
//!   `WINDOW_FWD`) for the nearest segment. A route that passes near itself (an overpass, a loop)
//!   therefore cannot make the car jump to the other pass. Only when the windowed nearest is
//!   further than [`OFF_ROUTE_M`] is the whole polyline scanned once: a shortcut or a U-turn is
//!   "on the route" if the global nearest is within [`OFF_ROUTE_M`].
//! * **Along-route distance is monotone** inside the window (a reversing car does not add
//!   distance back); only the global fallback may move it backwards.
//! * **Off route** = further than the limit for [`OFF_ROUTE_S`] seconds of *packet time*
//!   (`dt_ms` summed over packets while the game is driving), so a frame stall counts as one
//!   capped step and a paused game counts nothing. After a route is adopted the check is
//!   suppressed for [`REROUTE_SUPPRESS_S`] (hysteresis: the new route starts at the car's snap).
//! * **The drawn start** advances in [`TRIM_STEP_M`] chunks (see [`Follower::advance_trim`]).

use std::sync::Arc;

use super::search::Route;

/// "Off the route" distance (D84 "~50 m").
pub const OFF_ROUTE_M: f32 = 50.0;
/// How long the car must stay off the route before a re-route is asked for (packet time).
pub const OFF_ROUTE_S: f32 = 2.0;
/// After a route is adopted the off-route check waits this long.
pub const REROUTE_SUPPRESS_S: f32 = 3.0;
/// Remaining route length below which the trip is over...
pub const ARRIVE_REMAINING_M: f32 = 25.0;
/// ...if the car is also this close to the route's end.
pub const ARRIVE_CAR_M: f32 = 40.0;
/// Segments scanned behind / ahead of the current one (~40 m back, ~1.6 km ahead on 20 m edges).
const WINDOW_BACK: usize = 2;
const WINDOW_FWD: usize = 80;
/// The drawn line starts at most this far (along the route) behind the car.
pub const TRIM_STEP_M: f32 = 150.0;

/// What a [`Follower::update`] worked out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    /// Metres driven along the route (the projection of the car).
    pub along_m: f32,
    pub remaining_m: f32,
    /// Remaining time at the assumed speeds.
    pub remaining_eta_s: f32,
    /// Horizontal distance from the car to the route.
    pub off_m: f32,
    /// Index of the segment the car is on.
    pub seg: usize,
    /// Remaining < [`ARRIVE_REMAINING_M`] and the car within [`ARRIVE_CAR_M`] of the end.
    pub arrived: bool,
    /// The car has been off the route for [`OFF_ROUTE_S`]: ask for a new route. Stays true on
    /// the following updates until the route is replaced or the car is back on it.
    pub reroute: bool,
}

pub struct Follower {
    route: Arc<Route>,
    /// Cumulative length / time at the start of each segment (+ the total at the end).
    cum_m: Vec<f32>,
    cum_s: Vec<f32>,
    /// Segment + fraction of the accepted position.
    seg: usize,
    t: f32,
    p: Progress,
    /// Packet time (ms, an integer so 20 steps of 100 ms are exactly 2 s) spent off the route /
    /// left of the post-adoption suppression.
    off_ms: u32,
    suppress_ms: u32,
    /// The off-route distance that counts: [`OFF_ROUTE_M`] plus the gap the route started with,
    /// until the car was first on the route (see [`Follower::new`]).
    limit_m: f32,
    on_route_seen: bool,
    /// First vertex of the drawn line.
    drawn_from: usize,
}

impl Follower {
    /// `start_gap_m` = how far the car was from the route's first point when the route was
    /// requested (it is the snap distance: 0 on a road, up to 300 m in a field). *Why:* a route
    /// starting 120 m away would otherwise be "off route" at once and re-route forever. Until the
    /// car has been within [`OFF_ROUTE_M`] of the route, the limit is `50 + start_gap`, so only
    /// driving *away* from the road it is heading for counts.
    pub fn new(route: Arc<Route>, start_gap_m: f32) -> Follower {
        let mut cum_m = Vec::with_capacity(route.seg_len.len() + 1);
        let mut cum_s = Vec::with_capacity(route.seg_len.len() + 1);
        let (mut m, mut s) = (0.0, 0.0);
        for (l, t) in route.seg_len.iter().zip(&route.seg_time_s) {
            cum_m.push(m);
            cum_s.push(s);
            m += l;
            s += t;
        }
        cum_m.push(m);
        cum_s.push(s);
        let p = Progress { along_m: 0.0, remaining_m: m, remaining_eta_s: s, off_m: 0.0, seg: 0, arrived: false, reroute: false };
        Follower { route, cum_m, cum_s, seg: 0, t: 0.0, p, off_ms: 0, suppress_ms: (REROUTE_SUPPRESS_S * 1000.0) as u32, limit_m: OFF_ROUTE_M + start_gap_m.max(0.0), on_route_seen: start_gap_m <= OFF_ROUTE_M, drawn_from: 0 }
    }

    pub fn route(&self) -> &Arc<Route> {
        &self.route
    }

    pub fn progress(&self) -> &Progress {
        &self.p
    }

    /// The route's last point (the snapped destination).
    pub fn end(&self) -> [f32; 2] {
        *self.route.pts.last().unwrap_or(&[0.0, 0.0])
    }

    /// First vertex of the drawn (still to drive) line.
    pub fn drawn_from(&self) -> usize {
        self.drawn_from
    }

    /// One packet. `dt_ms` is the (capped) packet-time step; `driving` = false for paused /
    /// loading-screen packets, which change nothing (their position is 0/0/0 or frozen).
    pub fn update(&mut self, x: f32, z: f32, dt_ms: u32, driving: bool) -> &Progress {
        if !driving {
            return &self.p;
        }
        let nseg = self.route.seg_len.len();
        if nseg == 0 {
            // A degenerate route (start and destination on the same spot): only arrival counts.
            let end = self.end();
            self.p.off_m = (x - end[0]).hypot(z - end[1]);
            self.p.arrived = self.p.off_m < ARRIVE_CAR_M;
            return &self.p;
        }

        let (lo, hi) = (self.seg.saturating_sub(WINDOW_BACK), (self.seg + WINDOW_FWD).min(nseg - 1));
        let (mut seg, mut t, mut off) = self.nearest(x, z, lo, hi);
        let mut global = false;
        if off > OFF_ROUTE_M && (lo > 0 || hi < nseg - 1) {
            let g = self.nearest(x, z, 0, nseg - 1);
            if g.2 < off {
                off = g.2; // the true distance to the route
                if off <= OFF_ROUTE_M {
                    (seg, t) = (g.0, g.1); // only a hit is adopted as the new position
                    global = true;
                }
            }
        }
        let along = self.cum_m[seg] + t * self.route.seg_len[seg];
        // Monotone inside the window; a global hit (shortcut, U-turn) may go anywhere.
        if global || along >= self.p.along_m {
            self.seg = seg;
            self.t = t;
            let total = *self.cum_m.last().unwrap();
            self.p.along_m = along;
            self.p.remaining_m = (total - along).max(0.0);
            self.p.remaining_eta_s = ((*self.cum_s.last().unwrap()) - (self.cum_s[seg] + t * self.route.seg_time_s[seg])).max(0.0);
            self.p.seg = seg;
        }
        self.p.off_m = off;
        if off <= OFF_ROUTE_M {
            self.on_route_seen = true;
            self.limit_m = OFF_ROUTE_M;
        }

        // The off-route clock, in packet time. Suppressed right after a route was adopted.
        if self.suppress_ms > 0 {
            self.suppress_ms = self.suppress_ms.saturating_sub(dt_ms);
            self.off_ms = 0;
        } else if off > self.limit_m {
            self.off_ms = self.off_ms.saturating_add(dt_ms);
        } else {
            self.off_ms = 0;
        }
        self.p.reroute = self.off_ms >= (OFF_ROUTE_S * 1000.0) as u32;

        let end = self.end();
        self.p.arrived = self.p.remaining_m < ARRIVE_REMAINING_M && (x - end[0]).hypot(z - end[1]) < ARRIVE_CAR_M;
        &self.p
    }

    /// Nearest segment of `lo..=hi` to (x, z): `(segment, fraction along it, horizontal distance)`.
    /// Ties go to the earlier segment.
    fn nearest(&self, x: f32, z: f32, lo: usize, hi: usize) -> (usize, f32, f32) {
        let pts = &self.route.pts;
        let mut best = (lo, 0.0_f32, f32::INFINITY);
        for i in lo..=hi {
            let (a, b) = (pts[i], pts[i + 1]);
            let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
            let len2 = dx * dx + dz * dz;
            let t = if len2 > 0.0 { (((x - a[0]) * dx + (z - a[1]) * dz) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let d2 = (x - (a[0] + t * dx)).powi(2) + (z - (a[1] + t * dz)).powi(2);
            if d2 < best.2 {
                best = (i, t, d2);
            }
        }
        (best.0, best.1, best.2.sqrt())
    }

    /// Move the drawn start forward if the car is [`TRIM_STEP_M`] or more past it. Returns true
    /// when it moved (the caller then publishes a new line). The new start is the current
    /// segment's first vertex, so the line keeps at most one chunk of tail behind the car.
    /// *Why chunks:* a new line `Arc` rebuilds the 3D deck mesh; trimming per packet would do
    /// that 70 times a second.
    pub fn advance_trim(&mut self) -> bool {
        if self.p.seg > self.drawn_from && self.p.along_m - self.cum_m[self.drawn_from] >= TRIM_STEP_M {
            self.drawn_from = self.p.seg;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A straight route along +x from 0 to `len` m with a vertex every 20 m (assumed 20 m/s).
    pub(crate) fn straight(len: f32) -> Arc<Route> {
        let n = (len / 20.0).round() as usize;
        let pts: Vec<[f32; 2]> = (0..=n).map(|i| [20.0 * i as f32, 0.0]).collect();
        route_of(pts)
    }

    pub(crate) fn route_of(pts: Vec<[f32; 2]>) -> Arc<Route> {
        let seg_len: Vec<f32> = pts.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).collect();
        let seg_time_s: Vec<f32> = seg_len.iter().map(|l| l / 20.0).collect();
        let dist_m = seg_len.iter().sum();
        let eta_s = seg_time_s.iter().sum();
        Arc::new(Route { y: vec![0.0; pts.len()], seg_kind: vec![1; seg_len.len()], seg_len, seg_time_s, dist_m, eta_s, cost: 0.0, pts })
    }

    const DT: u32 = 100;

    #[test]
    fn progress_follows_the_car_and_is_monotone() {
        let mut f = Follower::new(straight(1000.0), 0.0);
        let p = *f.update(0.0, 0.0, DT, true);
        assert_eq!((p.along_m, p.remaining_m, p.seg), (0.0, 1000.0, 0));
        assert!((p.remaining_eta_s - 50.0).abs() < 1e-3);
        let p = *f.update(310.0, 3.0, DT, true);
        assert!((p.along_m - 310.0).abs() < 1e-3 && (p.remaining_m - 690.0).abs() < 1e-3, "{p:?}");
        assert!((p.remaining_eta_s - 34.5).abs() < 1e-3, "eta {}", p.remaining_eta_s);
        assert_eq!(p.seg, 15);
        assert!((p.off_m - 3.0).abs() < 1e-4);
        // a car that reverses a little does not give distance back
        let p = *f.update(300.0, 0.0, DT, true);
        assert!((p.along_m - 310.0).abs() < 1e-3, "{p:?}");
        assert!(p.off_m < 1e-4, "but it is still on the route");
    }

    #[test]
    fn a_route_passing_near_itself_does_not_jump_to_the_other_pass() {
        // east 2.4 km, 30 m north, back west 2.4 km: the passes are 30 m apart but ~120 segments
        // apart along the route, beyond the scan window
        let mut pts: Vec<[f32; 2]> = (0..=120).map(|i| [20.0 * i as f32, 0.0]).collect();
        pts.extend((0..=120).rev().map(|i| [20.0 * i as f32, 30.0]));
        let mut f = Follower::new(route_of(pts), 0.0);
        f.update(0.0, 0.0, DT, true);
        // 18 m from the first pass, 12 m from the return pass: the window keeps the first
        let p = *f.update(110.0, 18.0, DT, true);
        assert_eq!(p.seg, 5, "{p:?}");
        assert!((p.along_m - 110.0).abs() < 1e-3 && (p.off_m - 18.0).abs() < 1e-3);
    }

    #[test]
    fn windowed_scan_falls_back_to_a_global_scan() {
        let mut f = Follower::new(straight(4000.0), 0.0);
        f.update(0.0, 0.0, DT, true);
        // 2.5 km ahead of the start is outside the window (80 segments = 1.6 km): the windowed
        // nearest is 900 m away, the global scan finds the car on the route.
        let p = *f.update(2500.0, 2.0, DT, true);
        assert!(p.off_m < 3.0, "{p:?}");
        assert!((p.along_m - 2500.0).abs() < 1e-2);
        // and a shortcut backwards (U-turn) is adopted as well
        let p = *f.update(1000.0, 0.0, DT, true);
        assert!((p.along_m - 1000.0).abs() < 1e-2, "{p:?}");
        // far from everything: off, position unchanged
        let p = *f.update(1000.0, 500.0, DT, true);
        assert!((p.off_m - 500.0).abs() < 1e-3 && (p.along_m - 1000.0).abs() < 1e-2, "{p:?}");
    }

    /// 50 m for 2 s of packet time, with the 3 s suppression and the hysteresis around 50 m.
    #[test]
    fn off_route_needs_more_than_50m_for_2s_of_packet_time() {
        let mut f = Follower::new(straight(1000.0), 0.0);
        let mut t = 0u32;
        let mut step = |f: &mut Follower, x: f32, z: f32, ms: u32| {
            t += ms;
            f.update(x, z, ms, true).reroute
        };
        // adopted: the first 3 s are suppressed whatever the car does
        for _ in 0..30 {
            assert!(!step(&mut f, 100.0, 80.0, 100));
        }
        // 49 m off never triggers
        for _ in 0..100 {
            assert!(!step(&mut f, 100.0, 49.0, 100));
        }
        // 51 m off: not for 1.9 s, then yes at 2.0 s
        for i in 0..19 {
            assert!(!step(&mut f, 100.0, 51.0, 100), "at {i}");
        }
        assert!(step(&mut f, 100.0, 51.0, 100), "2.0 s reached");
        // stays asserted until the car is back or the route is replaced
        assert!(step(&mut f, 100.0, 51.0, 100));
        // back within 50 m: cleared, and the clock restarts from 0
        assert!(!step(&mut f, 100.0, 40.0, 100));
        for _ in 0..19 {
            assert!(!step(&mut f, 100.0, 90.0, 100));
        }
        assert!(step(&mut f, 100.0, 90.0, 100));
    }

    #[test]
    fn a_stalled_frame_counts_as_one_capped_step_and_a_pause_counts_nothing() {
        let mut f = Follower::new(straight(1000.0), 0.0);
        for _ in 0..40 {
            f.update(100.0, 0.0, 100, true); // 4 s on route: suppression over
        }
        // A 250 ms step (the worker's cap) off the route is not 2 s.
        assert!(!f.update(100.0, 90.0, 250, true).reroute);
        // paused packets (position 0/0/0) are ignored: no progress, no off clock
        let before = *f.progress();
        for _ in 0..100 {
            assert_eq!(*f.update(0.0, 0.0, 1000, false), before);
        }
        assert!(!f.update(100.0, 90.0, 250, true).reroute, "still only 0.5 s off");
    }

    #[test]
    fn a_route_that_starts_far_from_the_car_does_not_re_route_forever() {
        // The car is in a field 120 m from the road: the route starts on the road.
        let mut f = Follower::new(straight(1000.0), 120.0);
        for _ in 0..100 {
            f.update(100.0, 120.0, 100, true);
        }
        let p = *f.update(100.0, 120.0, 100, true);
        assert!(!p.reroute, "{p:?}: 120 m off is where it started");
        // driving away from the road by 50 m more does count
        for _ in 0..30 {
            f.update(100.0, 175.0, 100, true);
        }
        assert!(f.progress().reroute);

        // Once the car reached the road, the plain 50 m limit applies.
        let mut f = Follower::new(straight(1000.0), 120.0);
        for _ in 0..40 {
            f.update(100.0, 0.0, 100, true);
        }
        for _ in 0..30 {
            f.update(100.0, 80.0, 100, true);
        }
        assert!(f.progress().reroute);
    }

    #[test]
    fn arrival_needs_the_route_end_in_reach() {
        let mut f = Follower::new(straight(1000.0), 0.0);
        assert!(!f.update(970.0, 0.0, DT, true).arrived, "30 m left: not yet");
        assert!(f.update(980.0, 0.0, DT, true).arrived, "20 m left, on the end");
        // 20 m left but the car 60 m to the side: not arrived
        let mut f = Follower::new(straight(1000.0), 0.0);
        assert!(!f.update(980.0, 60.0, DT, true).arrived);
        // paused packets never arrive
        let mut f = Follower::new(straight(1000.0), 0.0);
        assert!(!f.update(1000.0, 0.0, DT, false).arrived);
    }

    #[test]
    fn a_degenerate_route_arrives_when_the_car_is_there() {
        let r = route_of(vec![[10.0, 10.0]]);
        let mut f = Follower::new(r, 0.0);
        assert!(f.update(12.0, 10.0, DT, true).arrived);
        let mut f = Follower::new(route_of(vec![[10.0, 10.0]]), 0.0);
        assert!(!f.update(500.0, 10.0, DT, true).arrived);
    }

    #[test]
    fn the_drawn_start_moves_in_150m_chunks() {
        let mut f = Follower::new(straight(1000.0), 0.0);
        f.update(0.0, 0.0, DT, true);
        assert!(!f.advance_trim());
        let mut moves = vec![];
        for x in (0..=1000).step_by(10) {
            f.update(x as f32, 0.0, DT, true);
            if f.advance_trim() {
                moves.push((x, f.drawn_from()));
            }
        }
        // 150 m past the drawn start (a vertex every 20 m): at 150 m the start moves to vertex 7
        // (140 m), at 290 m to vertex 14 (280 m), ... never once per step
        assert_eq!(moves[0], (150, 7));
        assert_eq!(moves[1], (290, 14));
        assert_eq!(moves.len(), 7, "{moves:?}");
    }
}
