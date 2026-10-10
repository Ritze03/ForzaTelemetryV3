//! The navigation runtime: keeps a live route from the car to the destination.
//!
//! **Where it runs.** The per-packet half ([`Tracker`]) is owned by the listener thread
//! (`listeners/worker.rs`), not the UI thread: the UI frame loop stops while the game covers the
//! window, which is exactly when the HUD minimap is the only map in use. The route itself is
//! computed on a worker thread (`nav-route`, [`ThreadPlanner`]) because the packet loop sends the
//! gearbox's key presses and must never block. The result reaches the maps through a
//! process-global ([`view`]), like `maprender::store` does for the layers, not through the HUD
//! snapshot.
//!
//! ```text
//!  UI thread ──set_destination / set_prefs / set_follow_shared──▶ Shared.inputs (+ inputs_seq)
//!  listener ──tick(now, sample)──▶ Tracker ──submit──▶ nav-route thread ──reply──▶ Tracker
//!  listener ──set_shared(coop dest)──▶ Tracker                                       │
//!  anyone ◀──────────────────────────── view() = Shared.view ◀──────────publish─────┘
//! ```
//!
//! The tracker is generic over a [`Planner`] (the thread in production, a recorder in tests) and
//! takes its clock as an argument, so the debounce / spacing rules are tested without sleeping.
//!
//! **Rules** (design section 4, D84, D85, D92): requests are debounced [`DEBOUNCE_MS`]; two
//! *automatic* requests (off route, graph replaced, retry after a failure) are at least
//! [`MIN_AUTO_REROUTE_MS`] apart; a reply whose generation is stale is dropped; in a race the
//! route is hidden and nothing is requested; a shared destination overrides the local one and
//! is routed with the setter's filters and curve; the drawn line is published in 150 m chunks.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::cfg::{RouteFilters, RoutePrefs};
use super::follow::{Follower, OFF_ROUTE_M};
use super::graph::RouteGraph;
use super::search::{Endpoint, Route, RouteError};

/// A change of destination / prefs waits this long for more changes (a slider drag).
pub const DEBOUNCE_MS: u64 = 200;
/// Minimum spacing of two automatic requests.
pub const MIN_AUTO_REROUTE_MS: u64 = 3000;
/// A failed route (no road near the car, unreachable) is retried only after the car moved this far.
pub const RETRY_MOVE_M: f32 = 25.0;
/// How often the worker thread looks at the store for a replaced graph while a destination exists.
const GRAPH_WATCH: Duration = if cfg!(test) { Duration::from_millis(40) } else { Duration::from_secs(1) };
/// How often the worker thread retries while the graph is still loading.
const LOAD_RETRY: Duration = Duration::from_millis(250);

// ── what everybody reads ───────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum NavStatus {
    /// No destination.
    #[default]
    Idle,
    /// A destination exists but the game has not sent a driving position yet.
    WaitingForCar,
    /// A route is being computed (a previous route, if any, is still in `line`).
    Routing,
    /// A route is being followed.
    Ok,
    /// No allowed road within 300 m of the car or of the destination.
    NoRoadNear(Endpoint),
    /// Both ends are on the network but the road-type filters leave no connection.
    Unreachable,
    /// No road data (no game install, or the layers failed to load).
    NoRoadData,
    /// In a race (race position != 0): the route is hidden and nothing is recomputed (D92).
    PausedRace,
    /// The destination was reached. A local destination was cleared (see
    /// [`NavView::local_cleared_seq`]); a shared one is only done for this player.
    Arrived,
}

#[derive(Clone, PartialEq, Debug)]
pub enum DestSource {
    Local,
    /// Set by a co-op teammate; routed with *their* filters and curve.
    Shared { setter: String, hue: f32 },
}

/// The destination being navigated to: world x, z (as clicked, not snapped) and the preferences
/// the route is computed with (the user's own for a local one, the setter's for a shared one).
#[derive(Clone, PartialEq, Debug)]
pub struct Dest {
    pub x: f32,
    pub z: f32,
    pub source: DestSource,
    pub prefs: RoutePrefs,
}

/// The part of the route still to drive, as drawn. Immutable and shared: a consumer that caches
/// something per line (the 3D deck mesh) keys it on `rev` / `Arc::ptr_eq`.
#[derive(Clone, PartialEq, Debug)]
pub struct NavLine {
    /// Unique and increasing across all lines of the process.
    pub rev: u64,
    pub pts: Vec<[f32; 2]>,
    /// Height per point (0.0 = unknown).
    pub y: Vec<f32>,
    /// `RoadType::index` per segment (`pts.len() - 1`; 7 = a jump, so the drawing can split there).
    pub seg_kind: Vec<u8>,
}

/// Everything the maps and the tab need; a cheap clone (two `Arc`-free small structs and one `Arc`).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct NavView {
    /// Bumped on every published change of anything below (not for each packet).
    pub rev: u64,
    pub status: NavStatus,
    pub dest: Option<Dest>,
    /// The remaining route, trimmed in ~150 m chunks; `None` while hidden (race), failed, arrived
    /// or without a destination. Compare `Arc::ptr_eq` / `rev` to detect a new line.
    pub line: Option<Arc<NavLine>>,
    /// Metres still to drive / whole route length / remaining time at the assumed speeds (~).
    pub remaining_m: f32,
    pub total_m: f32,
    pub eta_s: f32,
    /// Increments each time a *local* destination was cleared because the car arrived. The UI
    /// clears its persisted copy (`AppConfig.nav.destination`) when it sees this change.
    pub local_cleared_seq: u64,
}

// ── what the UI (and the listener) put in ──────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug)]
struct Inputs {
    local: Option<[f32; 2]>,
    prefs: RoutePrefs,
    follow_shared: bool,
}

impl Default for Inputs {
    fn default() -> Self {
        Inputs { local: None, prefs: RoutePrefs::default(), follow_shared: true }
    }
}

/// The co-op room's shared destination as the tracker takes it (the listener converts
/// `coop::SharedDest`, so `nav` does not import `coop`).
#[derive(Clone, PartialEq, Debug)]
pub struct SharedIn {
    pub x: f32,
    pub z: f32,
    pub hue: f32,
    pub setter: String,
    /// Raw `Dest.f` bits; only the known ones are used (`RouteFilters::from_bits`).
    pub filter_bits: u8,
    pub curve: f32,
    /// The setter's timestamp: the identity of this destination (a late-joiner resend of the same
    /// one keeps it, so an arrival is not forgotten).
    pub ts: u64,
}

/// The process-global mailbox between the UI, the listener thread and the readers.
#[derive(Default)]
pub struct Shared {
    view: Mutex<NavView>,
    inputs: Mutex<Inputs>,
    /// Bumped on every changed input; the tracker looks at this atomic per packet and only locks
    /// `inputs` when it moved.
    inputs_seq: AtomicU64,
    line_rev: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    pub fn view(&self) -> NavView {
        lock(&self.view).clone()
    }

    fn change(&self, f: impl FnOnce(&mut Inputs)) {
        let mut g = lock(&self.inputs);
        let before = g.clone();
        f(&mut g);
        if *g != before {
            self.inputs_seq.fetch_add(1, Ordering::Release);
        }
    }

    pub fn set_destination(&self, dest: Option<[f32; 2]>) {
        self.change(|i| i.local = dest);
    }

    pub fn set_prefs(&self, prefs: RoutePrefs) {
        self.change(|i| i.prefs = prefs);
    }

    pub fn set_follow_shared(&self, on: bool) {
        self.change(|i| i.follow_shared = on);
    }

    pub fn local_destination(&self) -> Option<[f32; 2]> {
        lock(&self.inputs).local
    }
}

static GLOBAL: OnceLock<Arc<Shared>> = OnceLock::new();

/// The process-wide navigation state.
pub fn shared() -> &'static Arc<Shared> {
    GLOBAL.get_or_init(|| Arc::new(Shared::default()))
}

/// The current navigation state. One lock and a clone: call it from any thread, every frame.
pub fn view() -> NavView {
    shared().view()
}

/// Set (`Some([x, z])`) or clear this player's own destination. Idempotent: the same value again
/// changes nothing (the UI may call it every frame). Persisting it is the caller's business.
pub fn set_destination(dest: Option<[f32; 2]>) {
    shared().set_destination(dest);
}

/// The road filters and curve slider for the user's own destination. Idempotent.
pub fn set_prefs(prefs: RoutePrefs) {
    shared().set_prefs(prefs);
}

/// Whether a co-op shared destination replaces the local one (`NavConfig::follow_shared`).
pub fn set_follow_shared(on: bool) {
    shared().set_follow_shared(on);
}

/// The local destination as the navigator has it (cleared on arrival).
pub fn local_destination() -> Option<[f32; 2]> {
    shared().local_destination()
}

// ── the planner: where routes are computed ─────────────────────────────────────────────────────

pub struct Request {
    pub gen: u64,
    /// Car x, z and height.
    pub car: (f32, f32, Option<f32>),
    pub dest: (f32, f32),
    pub prefs: RoutePrefs,
}

pub struct Reply {
    pub gen: u64,
    pub result: Result<Route, RouteError>,
}

#[derive(Default)]
pub struct Polled {
    pub reply: Option<Reply>,
    /// The road graph was replaced (editor Save, season change, install found / gone) since the
    /// last poll.
    pub graph_changed: bool,
}

pub trait Planner {
    /// Replaces any request not started yet.
    fn submit(&mut self, req: Request);
    /// Cheap when there is nothing (one atomic load).
    fn poll(&mut self) -> Polled;
    /// `true` while a destination exists: load the graph now and watch it for replacement.
    fn watch(&mut self, on: bool);
}

/// Where the worker thread gets the graph.
pub trait GraphSource: Send + Sync + 'static {
    fn graph(&self) -> GraphState;
}

pub enum GraphState {
    /// Nothing loaded yet; a load is running.
    Loading,
    Ready(Arc<RouteGraph>),
    /// No install, or the load failed.
    Unavailable,
}

/// The real source: the process-wide map layers (`maprender::store`).
///
/// *Why the navigator calls `store::layers()` itself:* a route is needed with no map on screen
/// (HUD only), and the layers are loaded for a destination as soon as one exists, not when a
/// map layer is switched on. The call is cheap (one lock, an `Arc` clone, a debounced `stat`),
/// it runs on the `nav-route` thread (never the UI's or the packet loop's), and the load itself
/// is on the store's own thread, so a UI frame never waits on it.
pub struct StoreSource;

impl GraphSource for StoreSource {
    fn graph(&self) -> GraphState {
        use crate::maprender::store::{self, LayerStatus};
        let l = store::layers();
        match (l.data, l.status) {
            (Some(d), _) => GraphState::Ready(d.route_graph.clone()),
            (None, LayerStatus::Loading) => GraphState::Loading,
            (None, _) => GraphState::Unavailable,
        }
    }
}

#[derive(Default)]
struct Mailbox {
    req: Option<Request>,
    reply: Option<Reply>,
    watch: bool,
    graph_changed: bool,
    stop: bool,
}

/// The `nav-route` thread: lazy (started by the first `submit` / `watch(true)`), Condvar-woken,
/// one request at a time, latest request wins. Costs nothing while no destination exists.
pub struct ThreadPlanner {
    source: Arc<dyn GraphSource>,
    mb: Arc<(Mutex<Mailbox>, Condvar)>,
    /// Set (under the mailbox lock) when there is a reply / graph change to take.
    ready: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ThreadPlanner {
    pub fn new(source: Arc<dyn GraphSource>) -> ThreadPlanner {
        ThreadPlanner { source, mb: Arc::new((Mutex::new(Mailbox::default()), Condvar::new())), ready: Arc::new(AtomicBool::new(false)), join: None }
    }

    fn ensure_thread(&mut self) {
        if self.join.is_none() {
            let (source, mb, ready) = (self.source.clone(), self.mb.clone(), self.ready.clone());
            self.join = std::thread::Builder::new().name("nav-route".into()).spawn(move || worker(source, mb, ready)).ok();
        }
    }
}

impl Drop for ThreadPlanner {
    fn drop(&mut self) {
        let (m, cv) = &*self.mb;
        lock(m).stop = true;
        cv.notify_all();
    }
}

impl Planner for ThreadPlanner {
    fn submit(&mut self, req: Request) {
        self.ensure_thread();
        let (m, cv) = &*self.mb;
        lock(m).req = Some(req);
        cv.notify_all();
    }

    fn poll(&mut self) -> Polled {
        if !self.ready.load(Ordering::Acquire) {
            return Polled::default();
        }
        let mut g = lock(&self.mb.0);
        self.ready.store(false, Ordering::Release);
        Polled { reply: g.reply.take(), graph_changed: std::mem::take(&mut g.graph_changed) }
    }

    fn watch(&mut self, on: bool) {
        if on {
            self.ensure_thread();
        }
        let (m, cv) = &*self.mb;
        lock(m).watch = on;
        cv.notify_all();
    }
}

/// Identity of a graph for change detection: its `Arc` address (`None` = no graph).
fn graph_id(s: &GraphState) -> Option<Option<usize>> {
    match s {
        GraphState::Loading => None,
        GraphState::Ready(g) => Some(Some(Arc::as_ptr(g) as usize)),
        GraphState::Unavailable => Some(None),
    }
}

fn worker(source: Arc<dyn GraphSource>, mb: Arc<(Mutex<Mailbox>, Condvar)>, ready: Arc<AtomicBool>) {
    let (m, cv) = &*mb;
    // The graph the last reply (or the first look) used. Outer `None` = never looked.
    let mut last: Option<Option<usize>> = None;
    loop {
        let (req, watch) = {
            let mut g = lock(m);
            loop {
                if g.stop {
                    return;
                }
                if g.req.is_some() || g.watch {
                    break;
                }
                g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
            }
            (g.req.take(), g.watch)
        };

        let state = source.graph();
        let cur = graph_id(&state);
        let mut changed = false;
        if let Some(c) = cur {
            match last {
                None => last = Some(c),
                Some(prev) if prev != c && req.is_none() => {
                    changed = true;
                    last = Some(c);
                }
                _ => {}
            }
        }

        let mut retry_req = None;
        let mut reply = None;
        if let Some(req) = req {
            match state {
                GraphState::Loading => retry_req = Some(req),
                GraphState::Unavailable => {
                    last = Some(None);
                    reply = Some(Reply { gen: req.gen, result: Err(RouteError::EmptyGraph) });
                }
                GraphState::Ready(g) => {
                    last = cur;
                    let result = g.plan(req.car, req.dest, &req.prefs);
                    reply = Some(Reply { gen: req.gen, result });
                }
            }
        }

        let mut g = lock(m);
        if let Some(r) = retry_req {
            if g.req.is_none() {
                g.req = Some(r); // a newer request, if any, wins
            }
        }
        if reply.is_some() || changed {
            if reply.is_some() {
                g.reply = reply;
            }
            g.graph_changed |= changed;
            ready.store(true, Ordering::Release);
        }
        if !g.stop {
            let wait = if g.req.is_some() {
                LOAD_RETRY
            } else if watch && g.watch {
                GRAPH_WATCH
            } else {
                Duration::ZERO
            };
            if !wait.is_zero() {
                let _ = cv.wait_timeout(g, wait).unwrap_or_else(|e| e.into_inner());
            }
        }
    }
}

// ── the tracker ────────────────────────────────────────────────────────────────────────────────

/// One packet (or none) as the tracker needs it.
#[derive(Clone, Copy, Debug)]
pub struct CarSample {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Packet-time step since the previous packet, already capped by the caller (a stalled
    /// frame must not count as seconds off the route).
    pub dt_ms: u32,
    /// Not paused / loading screen / garage (`hud_paused` is false, car present). Paused packets
    /// carry a frozen or zero position and are ignored.
    pub driving: bool,
    /// Race position != 0 (the HUD's `in_race` rule).
    pub in_race: bool,
}

#[derive(Clone, PartialEq, Debug)]
struct Active {
    dest: Dest,
    /// Identity of a shared destination (`SharedIn::ts`).
    shared_ts: Option<u64>,
}

impl Active {
    /// Same trip? (Prefs and the setter's display details can change without a new trip.)
    fn same_trip(&self, o: &Active) -> bool {
        self.shared_ts == o.shared_ts && self.dest.x == o.dest.x && self.dest.z == o.dest.z
    }
}

#[derive(Clone, Copy)]
struct Pending {
    due_ms: u64,
    /// Only because the car left the route: dropped if it gets back.
    off_route_only: bool,
}

#[derive(PartialEq)]
struct ViewKey {
    status: NavStatus,
    dest_rev: u64,
    line_rev: u64,
    remaining: i32,
    eta: i32,
    total: i32,
    cleared: u64,
}

pub struct Tracker<P: Planner> {
    sh: Arc<Shared>,
    planner: P,
    seen_inputs: u64,
    inputs: Inputs,
    shared_in: Option<SharedIn>,
    /// The shared destination (by `ts`) the car arrived at: not followed again.
    shared_arrived: Option<u64>,
    active: Option<Active>,
    /// After an arrival with nothing to follow next: what to show (`Some(dest)` for a shared one).
    arrived: Option<Option<Dest>>,
    dest_rev: u64,
    /// Request generation: a reply with another value is stale.
    gen: u64,
    pending: Option<Pending>,
    inflight: bool,
    last_submit_ms: Option<u64>,
    car: Option<(f32, f32, f32)>,
    /// The car at the last request: the route's start gap and the retry distance.
    req_car: Option<(f32, f32)>,
    in_race: bool,
    follower: Option<Follower>,
    line: Option<Arc<NavLine>>,
    err: Option<RouteError>,
    watching: bool,
    cleared_seq: u64,
    published: Option<ViewKey>,
}

impl Tracker<ThreadPlanner> {
    /// The production tracker: the process-global state, the `nav-route` thread, the store.
    pub fn global() -> Tracker<ThreadPlanner> {
        Tracker::new(shared().clone(), ThreadPlanner::new(Arc::new(StoreSource)))
    }
}

impl<P: Planner> Tracker<P> {
    pub fn new(sh: Arc<Shared>, planner: P) -> Tracker<P> {
        // Start from the inputs as they are, but force a resolve on the first tick.
        Tracker {
            seen_inputs: sh.inputs_seq.load(Ordering::Acquire).wrapping_sub(1),
            sh,
            planner,
            inputs: Inputs::default(),
            shared_in: None,
            shared_arrived: None,
            active: None,
            arrived: None,
            dest_rev: 0,
            gen: 0,
            pending: None,
            inflight: false,
            last_submit_ms: None,
            car: None,
            req_car: None,
            in_race: false,
            follower: None,
            line: None,
            err: None,
            watching: false,
            cleared_seq: 0,
            published: Some(ViewKey { status: NavStatus::Idle, dest_rev: 0, line_rev: 0, remaining: 0, eta: 0, total: 0, cleared: 0 }),
        }
    }

    /// The room's shared destination changed (`None` = cleared / session over). Listener thread,
    /// when `CoopReader::destination_seq` moved.
    pub fn set_shared(&mut self, now_ms: u64, d: Option<SharedIn>) {
        if d == self.shared_in {
            return;
        }
        // A cleared shared destination is no longer "arrived at".
        if d.is_none() {
            self.shared_arrived = None;
            if matches!(self.arrived, Some(Some(_))) {
                self.arrived = None;
            }
        }
        self.shared_in = d;
        self.sync_inputs();
        self.resolve(now_ms);
        self.publish();
    }

    /// Take over the UI's inputs if they changed (one atomic load when they did not).
    fn sync_inputs(&mut self) -> bool {
        let seq = self.sh.inputs_seq.load(Ordering::Acquire);
        if seq == self.seen_inputs {
            return false;
        }
        self.seen_inputs = seq;
        self.inputs = lock(&self.sh.inputs).clone();
        true
    }

    /// Whether anything needs per-packet work (a destination exists). The idle cost of the whole
    /// tracker is [`tick`](Self::tick)'s first atomic load.
    pub fn active(&self) -> bool {
        self.active.is_some()
    }

    /// One loop iteration of the listener thread: `sample` is the packet if there was one (`None`
    /// on the idle timeout, which still has to deliver replies and run the debounce).
    pub fn tick(&mut self, now_ms: u64, sample: Option<&CarSample>) {
        if self.sync_inputs() {
            self.resolve(now_ms);
            self.publish();
        }
        if self.active.is_none() {
            if let Some(s) = sample {
                // Even idle: a destination set a moment later routes from this fix at once.
                self.in_race = s.in_race;
                if s.driving {
                    self.car = Some((s.x, s.y, s.z));
                }
            }
            return; // idle: the whole cost of a packet is the atomic load above
        }

        let polled = self.planner.poll();
        if polled.graph_changed && !self.in_race {
            self.schedule(now_ms, true, false);
        }
        if let Some(r) = polled.reply {
            self.on_reply(now_ms, r);
        }
        if let Some(s) = sample {
            self.on_sample(now_ms, s);
        }
        self.maybe_retry(now_ms);
        self.maybe_submit(now_ms);
        self.publish();
    }

    // ── destination resolution ─────────────────────────────────────────────────────────────────

    /// The destination to navigate to: the shared one when followed (and not yet arrived at),
    /// else the local one. Clearing the shared one falls back to the local one.
    fn compute_active(&self) -> Option<Active> {
        if self.inputs.follow_shared {
            if let Some(s) = &self.shared_in {
                if self.shared_arrived != Some(s.ts) {
                    let prefs = RoutePrefs { filters: RouteFilters::from_bits(s.filter_bits), curves: s.curve };
                    let source = DestSource::Shared { setter: s.setter.clone(), hue: s.hue };
                    return Some(Active { dest: Dest { x: s.x, z: s.z, source, prefs }, shared_ts: Some(s.ts) });
                }
            }
        }
        self.inputs.local.map(|[x, z]| Active { dest: Dest { x, z, source: DestSource::Local, prefs: self.inputs.prefs }, shared_ts: None })
    }

    fn resolve(&mut self, now_ms: u64) {
        let new = self.compute_active();
        let old = self.active.take();
        match (&old, &new) {
            (None, None) => {}
            (Some(o), Some(n)) if o == n => {}
            (Some(o), Some(n)) if o.same_trip(n) => {
                // Only the prefs (or the setter's name / colour) changed: keep the line shown while
                // the new one is computed.
                self.dest_rev += 1;
                if o.dest.prefs != n.dest.prefs {
                    self.restart(now_ms, false);
                }
            }
            (_, Some(_)) => {
                self.arrived = None;
                self.dest_rev += 1;
                self.restart(now_ms, true);
            }
            (Some(_), None) => {
                self.arrived = None;
                self.dest_rev += 1;
                self.reset_route();
            }
        }
        self.active = new;
        if self.active.is_some() && !self.watching {
            self.watching = true;
            self.planner.watch(true); // loads the graph already, before the first car fix
        } else if self.active.is_none() && self.watching {
            self.watching = false;
            self.planner.watch(false);
        }
    }

    /// Drop everything about the current route and make in-flight replies stale.
    fn reset_route(&mut self) {
        self.gen += 1;
        self.inflight = false;
        self.pending = None;
        self.follower = None;
        self.line = None;
        self.err = None;
    }

    /// A new trip (`keep_line` = false) or new prefs for the same trip: request a route after the
    /// debounce.
    fn restart(&mut self, now_ms: u64, new_trip: bool) {
        let (line, follower) = (self.line.take(), self.follower.take());
        self.reset_route();
        if !new_trip {
            // The old route stays on the maps (and is followed) until the new one arrives: a
            // slider drag must not make the line blink.
            (self.line, self.follower) = (line, follower);
        }
        self.pending = Some(Pending { due_ms: now_ms + DEBOUNCE_MS, off_route_only: false });
    }

    // ── requests ───────────────────────────────────────────────────────────────────────────────

    /// An automatic request: debounced, and `MIN_AUTO_REROUTE_MS` after the last one.
    fn schedule(&mut self, now_ms: u64, spaced: bool, off_route_only: bool) {
        let mut due = now_ms + DEBOUNCE_MS;
        if spaced {
            if let Some(l) = self.last_submit_ms {
                due = due.max(l + MIN_AUTO_REROUTE_MS);
            }
        }
        self.pending = Some(match self.pending {
            Some(p) => Pending { due_ms: p.due_ms.min(due), off_route_only: p.off_route_only && off_route_only },
            None => Pending { due_ms: due, off_route_only },
        });
    }

    fn maybe_submit(&mut self, now_ms: u64) {
        let (Some(p), Some(car)) = (self.pending, self.car) else { return };
        let Some((dest, prefs)) = self.active.as_ref().map(|a| ((a.dest.x, a.dest.z), a.dest.prefs)) else { return };
        if now_ms < p.due_ms || self.in_race {
            return;
        }
        self.gen += 1;
        self.pending = None;
        self.inflight = true;
        self.last_submit_ms = Some(now_ms);
        self.req_car = Some((car.0, car.2));
        self.planner.submit(Request { gen: self.gen, car: (car.0, car.2, Some(car.1)), dest, prefs });
    }

    /// A route that failed because of where the car is (no road near it, or on a disconnected
    /// piece of network) is tried again every 3 s while the car moves; not in a tight loop and
    /// not for a car that stands still in a field.
    fn maybe_retry(&mut self, now_ms: u64) {
        if !matches!(self.err, Some(RouteError::NoRoadNear(Endpoint::Car)) | Some(RouteError::Unreachable)) {
            return;
        }
        if self.inflight || self.pending.is_some() || self.in_race {
            return;
        }
        if let (Some(c), Some(r)) = (self.car, self.req_car) {
            if (c.0 - r.0).hypot(c.2 - r.1) >= RETRY_MOVE_M {
                self.schedule(now_ms, true, false);
            }
        }
    }

    fn on_reply(&mut self, now_ms: u64, r: Reply) {
        if r.gen != self.gen || self.in_race {
            return; // stale: the destination / prefs changed, a newer request went out, or a race began
        }
        self.inflight = false;
        match r.result {
            Ok(route) => {
                self.err = None;
                let gap = match (self.req_car, route.pts.first()) {
                    (Some(c), Some(p)) => (p[0] - c.0).hypot(p[1] - c.1),
                    _ => 0.0,
                };
                let mut f = Follower::new(Arc::new(route), gap);
                if let Some(c) = self.car {
                    f.update(c.0, c.2, 0, true);
                }
                let arrived = f.progress().arrived;
                self.line = Some(self.make_line(&f));
                self.follower = Some(f);
                if arrived {
                    self.arrive(now_ms);
                }
            }
            Err(e) => {
                self.err = Some(e);
                self.follower = None;
                self.line = None;
            }
        }
    }

    fn make_line(&self, f: &Follower) -> Arc<NavLine> {
        let (r, from) = (f.route(), f.drawn_from());
        let rev = self.sh.line_rev.fetch_add(1, Ordering::Relaxed) + 1;
        Arc::new(NavLine { rev, pts: r.pts[from..].to_vec(), y: r.y[from..].to_vec(), seg_kind: r.seg_kind[from.min(r.seg_kind.len())..].to_vec() })
    }

    // ── per packet ─────────────────────────────────────────────────────────────────────────────

    fn on_sample(&mut self, now_ms: u64, s: &CarSample) {
        if s.in_race != self.in_race {
            self.in_race = s.in_race;
            if s.in_race {
                // Hidden and paused (D92): the race road is the same kind of line in the same slot.
                self.reset_route();
            } else {
                self.schedule(now_ms, false, false); // back: re-snap from wherever the race ended
            }
        }
        if s.driving {
            self.car = Some((s.x, s.y, s.z));
        }
        if self.in_race || !s.driving {
            return;
        }
        let Some(f) = &mut self.follower else { return };
        let p = *f.update(s.x, s.z, s.dt_ms, true);
        if p.arrived {
            self.arrive(now_ms);
            return;
        }
        if p.reroute {
            if self.pending.is_none() && !self.inflight {
                self.schedule(now_ms, true, true);
            }
        } else if p.off_m <= OFF_ROUTE_M && self.pending.is_some_and(|p| p.off_route_only) {
            self.pending = None; // back on the route before the new one was asked for
        }
        if self.follower.as_mut().is_some_and(|f| f.advance_trim()) {
            let f = self.follower.as_ref().unwrap();
            self.line = Some(self.make_line(f));
        }
    }

    /// The car reached the end of the route. A local destination is cleared (inputs and, via
    /// [`NavView::local_cleared_seq`], the UI's copy); a shared one is only done for this player.
    fn arrive(&mut self, now_ms: u64) {
        let Some(a) = self.active.clone() else { return };
        let mut arrived_dest = None;
        match a.shared_ts {
            Some(ts) => {
                self.shared_arrived = Some(ts);
                arrived_dest = Some(a.dest.clone());
            }
            None => {
                let mut g = lock(&self.sh.inputs);
                if g.local == Some([a.dest.x, a.dest.z]) {
                    g.local = None;
                    self.sh.inputs_seq.fetch_add(1, Ordering::Release);
                }
                self.inputs = g.clone();
                self.seen_inputs = self.sh.inputs_seq.load(Ordering::Acquire);
                self.cleared_seq += 1;
            }
        }
        self.resolve(now_ms); // the next destination, if any
        if self.active.is_none() {
            self.arrived = Some(arrived_dest);
        }
    }

    // ── output ─────────────────────────────────────────────────────────────────────────────────

    fn status(&self) -> NavStatus {
        if self.active.is_none() {
            return if self.arrived.is_some() { NavStatus::Arrived } else { NavStatus::Idle };
        }
        if self.in_race {
            return NavStatus::PausedRace;
        }
        match self.err {
            Some(RouteError::EmptyGraph) => NavStatus::NoRoadData,
            Some(RouteError::NoRoadNear(e)) => NavStatus::NoRoadNear(e),
            Some(RouteError::Unreachable) => NavStatus::Unreachable,
            None if self.inflight || self.pending.is_some() => {
                if self.car.is_none() && self.follower.is_none() { NavStatus::WaitingForCar } else { NavStatus::Routing }
            }
            None if self.follower.is_some() => NavStatus::Ok,
            None => NavStatus::WaitingForCar,
        }
    }

    /// Write the view if anything a reader sees changed (remaining distance at 1 m, time at 1 s).
    fn publish(&mut self) {
        let status = self.status();
        let (remaining, eta, total) = match (&self.follower, status) {
            (Some(f), NavStatus::Ok | NavStatus::Routing) => (f.progress().remaining_m, f.progress().remaining_eta_s, f.route().dist_m),
            _ => (0.0, 0.0, 0.0),
        };
        let line = if matches!(status, NavStatus::Ok | NavStatus::Routing) { self.line.clone() } else { None };
        let key = ViewKey {
            status,
            dest_rev: self.dest_rev,
            line_rev: line.as_ref().map_or(0, |l| l.rev),
            remaining: remaining.round() as i32,
            eta: eta.round() as i32,
            total: total.round() as i32,
            cleared: self.cleared_seq,
        };
        if self.published.as_ref() == Some(&key) {
            return;
        }
        let dest = match (&self.active, &self.arrived) {
            (Some(a), _) => Some(a.dest.clone()),
            (None, Some(d)) => d.clone(),
            _ => None,
        };
        let mut v = lock(&self.sh.view);
        let rev = v.rev + 1;
        *v = NavView { rev, status, dest, line, remaining_m: remaining, total_m: total, eta_s: eta, local_cleared_seq: self.cleared_seq };
        drop(v);
        self.published = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use super::super::follow::tests::{route_of, straight};
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Fake {
        submitted: Vec<Request>,
        reply: Option<Reply>,
        graph_changed: bool,
        watching: bool,
    }

    #[derive(Clone, Default)]
    struct FakePlanner(Rc<RefCell<Fake>>);

    impl Planner for FakePlanner {
        fn submit(&mut self, req: Request) {
            self.0.borrow_mut().submitted.push(req);
        }
        fn poll(&mut self) -> Polled {
            let mut f = self.0.borrow_mut();
            Polled { reply: f.reply.take(), graph_changed: std::mem::take(&mut f.graph_changed) }
        }
        fn watch(&mut self, on: bool) {
            self.0.borrow_mut().watching = on;
        }
    }

    impl FakePlanner {
        fn gen(&self) -> u64 {
            self.0.borrow().submitted.last().unwrap().gen
        }
        fn n(&self) -> usize {
            self.0.borrow().submitted.len()
        }
        fn last(&self) -> std::cell::Ref<'_, Request> {
            std::cell::Ref::map(self.0.borrow(), |f| f.submitted.last().unwrap())
        }
        fn reply(&self, gen: u64, result: Result<Route, RouteError>) {
            self.0.borrow_mut().reply = Some(Reply { gen, result }); // latest wins, like the mailbox
        }
        /// Answer the newest request with `route`.
        fn answer(&self, route: Arc<Route>) {
            let gen = self.last().gen;
            self.reply(gen, Ok((*route).clone()));
        }
    }

    struct Rig {
        sh: Arc<Shared>,
        pl: FakePlanner,
        t: Tracker<FakePlanner>,
        now: u64,
        /// The tick time of each submitted request.
        times: Vec<u64>,
    }

    impl Rig {
        fn new() -> Rig {
            let sh = Arc::new(Shared::default());
            let pl = FakePlanner::default();
            let t = Tracker::new(sh.clone(), pl.clone());
            Rig { sh, pl, t, now: 1000, times: vec![] }
        }
        fn view(&self) -> NavView {
            self.sh.view()
        }
        fn tick(&mut self, s: Option<&CarSample>) {
            self.t.tick(self.now, s);
            while self.times.len() < self.pl.n() {
                self.times.push(self.now);
            }
        }
        /// Advance the clock by `ms` (idle ticks, as the listener loop does every 200 ms at worst).
        fn wait(&mut self, ms: u64) {
            let end = self.now + ms;
            while self.now < end {
                self.now = (self.now + 50).min(end);
                self.tick(None);
            }
        }
        fn drive(&mut self, x: f32, z: f32) {
            self.drive_dt(x, z, 100, false);
        }
        fn drive_dt(&mut self, x: f32, z: f32, dt_ms: u32, in_race: bool) {
            self.now += dt_ms as u64;
            let s = CarSample { x, y: 0.0, z, dt_ms, driving: true, in_race };
            self.tick(Some(&s));
        }
        fn shared_dest(&mut self, ts: u64, x: f32, z: f32, bits: u8, curve: f32) {
            self.t.set_shared(self.now, Some(SharedIn { x, z, hue: 200.0, setter: "Anna".into(), filter_bits: bits, curve, ts }));
        }
    }

    /// A rig with a destination at (1000, 0), the car at the start, and the first route adopted.
    fn routed() -> Rig {
        let mut r = Rig::new();
        r.sh.set_destination(Some([1000.0, 0.0]));
        r.drive(0.0, 0.0);
        r.wait(250);
        assert_eq!(r.pl.n(), 1);
        r.pl.answer(straight(1000.0));
        r.drive(0.0, 0.0);
        assert_eq!(r.view().status, NavStatus::Ok);
        r
    }

    #[test]
    fn idle_does_nothing_and_asks_for_nothing() {
        let mut r = Rig::new();
        for _ in 0..100 {
            r.drive(5.0, 5.0);
        }
        assert_eq!(r.view(), NavView::default(), "nothing was ever published");
        assert_eq!(r.pl.n(), 0);
        assert!(!r.pl.0.borrow().watching, "no destination: the worker thread / graph are not touched");
        assert!(!r.t.active());
    }

    #[test]
    fn setting_a_destination_waits_for_a_car_then_debounces() {
        let mut r = Rig::new();
        r.sh.set_destination(Some([1000.0, 0.0]));
        r.wait(500);
        let v = r.view();
        assert_eq!(v.status, NavStatus::WaitingForCar);
        assert_eq!(v.dest.as_ref().map(|d| (d.x, d.z, d.source.clone())), Some((1000.0, 0.0, DestSource::Local)));
        assert_eq!(r.pl.n(), 0, "no car fix, no request");
        assert!(r.pl.0.borrow().watching, "but the graph load / watch has started");
        r.drive(10.0, 0.0);
        assert_eq!(r.pl.n(), 1, "the first fix fires at once: the debounce long passed");
        assert_eq!(r.view().status, NavStatus::Routing);
        let q = r.pl.last();
        assert_eq!((q.car.0, q.car.1, q.dest), (10.0, 0.0, (1000.0, 0.0)));
    }

    #[test]
    fn changes_inside_the_debounce_make_one_request() {
        let mut r = Rig::new();
        r.drive(0.0, 0.0);
        r.sh.set_destination(Some([1000.0, 0.0]));
        r.wait(100);
        r.sh.set_destination(Some([1200.0, 0.0]));
        r.wait(100);
        r.sh.set_prefs(RoutePrefs { curves: 0.5, ..Default::default() });
        r.wait(150);
        assert_eq!(r.pl.n(), 0, "still inside 200 ms of the last change");
        r.wait(100);
        assert_eq!(r.pl.n(), 1);
        let q = r.pl.last();
        assert_eq!((q.dest, q.prefs.curves), ((1200.0, 0.0), 0.5));
        // setting the same inputs again is not a change
        drop(q);
        r.sh.set_destination(Some([1200.0, 0.0]));
        r.sh.set_prefs(RoutePrefs { curves: 0.5, ..Default::default() });
        r.wait(1000);
        assert_eq!(r.pl.n(), 1);
    }

    #[test]
    fn a_stale_result_is_dropped() {
        let mut r = Rig::new();
        r.drive(0.0, 0.0);
        r.sh.set_destination(Some([1000.0, 0.0]));
        r.wait(250);
        let first = r.pl.last().gen;
        // the user clicks elsewhere while the first request is out
        r.sh.set_destination(Some([-800.0, 0.0]));
        r.wait(250);
        assert_eq!(r.pl.n(), 2);
        let second = r.pl.last().gen;
        assert_ne!(first, second);
        // the first answer arrives late: ignored
        r.pl.reply(first, Ok((*straight(1000.0)).clone()));
        r.wait(50);
        assert_eq!(r.view().status, NavStatus::Routing);
        assert!(r.view().line.is_none());
        r.pl.reply(second, Ok((*straight(800.0)).clone()));
        r.wait(50);
        let v = r.view();
        assert_eq!(v.status, NavStatus::Ok);
        assert!((v.total_m - 800.0).abs() < 1e-3);
    }

    #[test]
    fn a_route_is_followed_and_the_line_is_republished_every_150_m() {
        let mut r = routed();
        let first = r.view();
        assert_eq!(first.line.as_ref().unwrap().pts.len(), 51);
        assert!((first.remaining_m - 1000.0).abs() < 1.0 && (first.eta_s - 50.0).abs() < 1.0);
        let mut revs = vec![first.line.as_ref().unwrap().rev];
        let mut views = 0;
        let mut last_rev = first.rev;
        let mut x = 0.0;
        while x < 900.0 {
            x += 5.0;
            r.drive_dt(x, 0.0, 70, false);
            let v = r.view();
            if v.rev != last_rev {
                views += 1;
                last_rev = v.rev;
            }
            let rev = v.line.as_ref().unwrap().rev;
            if *revs.last().unwrap() != rev {
                revs.push(rev);
            }
        }
        // 900 m driven: 6 trims (at 150, 290, 430, 570, 710, 850 m) -> 7 distinct lines, never one
        // per packet (180 packets)
        assert_eq!(revs.len(), 7, "{revs:?}");
        assert!(revs.windows(2).all(|w| w[0] < w[1]));
        let v = r.view();
        assert!((v.remaining_m - 100.0).abs() < 8.0, "{}", v.remaining_m);
        assert_eq!(v.line.as_ref().unwrap().pts[0], [840.0, 0.0], "the drawn start is the last chunk boundary");
        assert_eq!(v.line.as_ref().unwrap().seg_kind.len(), v.line.as_ref().unwrap().pts.len() - 1);
        // the view is rewritten only when a number changes at its precision: 1 m steps of 5 m
        // packets are one write per packet at worst, but a standing car writes nothing
        assert!(views <= 190);
        let before = r.view().rev;
        for _ in 0..100 {
            r.drive_dt(900.0, 0.0, 70, false);
        }
        assert_eq!(r.view().rev, before, "a car that stands still publishes nothing");
        // no extra route requests happened
        assert_eq!(r.pl.n(), 1);
    }

    #[test]
    fn leaving_the_route_asks_for_a_new_one_after_2s_and_not_more_often_than_3s() {
        let mut r = routed();
        for _ in 0..40 {
            r.drive(100.0, 0.0); // 4 s on the route: the adoption suppression is over
        }
        // 80 m off, driving on: nothing for 1.9 s of packet time...
        for _ in 0..19 {
            r.drive(100.0, 80.0);
        }
        assert_eq!(r.pl.n(), 1);
        r.drive(100.0, 80.0);
        r.wait(250); // + the debounce
        assert_eq!(r.pl.n(), 2, "re-route requested from the car's position");
        assert_eq!((r.pl.last().car.0, r.pl.last().car.1), (100.0, 80.0));
        let t_second = r.times[1];
        // the new route (from the car, on a parallel road at z = 80) is adopted
        let new_route = route_of((5..=45).map(|i| [20.0 * i as f32, 80.0]).collect());
        r.pl.answer(new_route.clone());
        r.drive(100.0, 80.0);
        assert_eq!(r.view().status, NavStatus::Ok);
        // the car leaves that one too at once: suppressed 3 s, then 2 s, then the 3 s spacing is long over
        for _ in 0..60 {
            r.drive(100.0, 300.0);
        }
        assert!(r.now - t_second >= 3000);
        r.wait(250);
        assert_eq!(r.pl.n(), 3);
        assert!(r.now - t_second >= MIN_AUTO_REROUTE_MS + 2000);
    }

    #[test]
    fn automatic_requests_are_spaced_at_least_3s() {
        let mut r = routed();
        r.wait(1000);
        // the graph is replaced a second after the request: wait for the 3 s spacing
        r.pl.0.borrow_mut().graph_changed = true;
        r.wait(1500);
        assert_eq!(r.pl.n(), 1, "not yet: < 3 s after the first request");
        r.wait(1000);
        assert_eq!(r.pl.n(), 2);
        assert!(r.times[1] - r.times[0] >= MIN_AUTO_REROUTE_MS, "{:?}", r.times);
        r.pl.answer(straight(1000.0));
        r.wait(100);
        // two graph changes inside one window are one request
        r.pl.0.borrow_mut().graph_changed = true;
        r.wait(50);
        r.pl.0.borrow_mut().graph_changed = true;
        r.wait(3500);
        assert_eq!(r.pl.n(), 3);
        assert!(r.times[2] - r.times[1] >= MIN_AUTO_REROUTE_MS, "{:?}", r.times);
    }

    #[test]
    fn a_car_off_the_road_retries_every_3s_while_it_moves() {
        let mut r = Rig::new();
        r.sh.set_destination(Some([1000.0, 0.0]));
        r.drive(0.0, 500.0);
        r.wait(250);
        assert_eq!(r.pl.n(), 1);
        let t0 = r.times[0];
        r.pl.reply(r.pl.gen(), Err(RouteError::NoRoadNear(Endpoint::Car)));
        r.wait(50);
        assert_eq!(r.view().status, NavStatus::NoRoadNear(Endpoint::Car));
        assert!(r.view().line.is_none());
        // parked in the field: never a retry
        for _ in 0..100 {
            r.drive(0.0, 500.0);
        }
        r.wait(5000);
        assert_eq!(r.pl.n(), 1, "no movement, no retry");
        // moving: one retry, not one per packet
        let mut sent_at = vec![];
        let mut z = 500.0;
        for _ in 0..400 {
            z += 1.0;
            let n = r.pl.n();
            r.drive(0.0, z);
            if r.pl.n() > n {
                sent_at.push(r.now);
                r.pl.reply(r.pl.gen(), Err(RouteError::NoRoadNear(Endpoint::Car)));
            }
        }
        assert!(sent_at.len() >= 2 && sent_at.len() <= 14, "{sent_at:?}");
        assert!(sent_at.windows(2).all(|w| w[1] - w[0] >= MIN_AUTO_REROUTE_MS), "{sent_at:?}");
        assert!(sent_at[0] - t0 >= MIN_AUTO_REROUTE_MS);
        // the status stays the error while retrying
        assert_eq!(r.view().status, NavStatus::NoRoadNear(Endpoint::Car));
        // and a route finally arrives
        r.pl.answer(straight(1000.0));
        r.drive(0.0, z);
        r.drive(0.0, z);
        assert_eq!(r.view().status, NavStatus::Ok);
    }

    #[test]
    fn errors_map_to_statuses() {
        for (e, want) in [
            (RouteError::EmptyGraph, NavStatus::NoRoadData),
            (RouteError::NoRoadNear(Endpoint::Destination), NavStatus::NoRoadNear(Endpoint::Destination)),
            (RouteError::Unreachable, NavStatus::Unreachable),
        ] {
            let mut r = Rig::new();
            r.sh.set_destination(Some([1000.0, 0.0]));
            r.drive(0.0, 0.0);
            r.wait(250);
            r.pl.reply(r.pl.gen(), Err(e));
            r.wait(50);
            assert_eq!(r.view().status, want);
            r.wait(10_000);
            assert_eq!(r.pl.n(), 1, "{e:?}: only the user's change or a new graph retries it");
            // a replaced graph does
            r.pl.0.borrow_mut().graph_changed = true;
            r.wait(300);
            assert_eq!(r.pl.n(), 2);
        }
    }

    #[test]
    fn a_race_hides_the_route_and_resumes_with_a_new_one() {
        let mut r = routed();
        for _ in 0..40 {
            r.drive(100.0, 0.0);
        }
        assert!(r.view().line.is_some());
        r.drive_dt(100.0, 0.0, 100, true);
        let v = r.view();
        assert_eq!(v.status, NavStatus::PausedRace);
        assert!(v.line.is_none());
        assert!(v.dest.is_some(), "the destination stays");
        // far off the old route for a long time: no re-route in a race
        for _ in 0..200 {
            r.drive_dt(5000.0, 5000.0, 100, true);
        }
        assert_eq!(r.pl.n(), 1);
        // a destination change during the race is held back, too
        r.sh.set_destination(Some([-500.0, 0.0]));
        r.wait(1000);
        assert_eq!(r.pl.n(), 1);
        assert_eq!(r.view().status, NavStatus::PausedRace);
        // the race ends: a new route from the car's snap
        r.drive_dt(-100.0, 0.0, 100, false);
        r.wait(250);
        assert_eq!(r.pl.n(), 2);
        assert_eq!((r.pl.last().car.0, r.pl.last().dest), (-100.0, (-500.0, 0.0)));
        assert_eq!(r.view().status, NavStatus::Routing);
        r.pl.answer(route_of(vec![[-100.0, 0.0], [-500.0, 0.0]]));
        r.drive(-100.0, 0.0);
        assert_eq!(r.view().status, NavStatus::Ok);
        // a reply that was in flight when the race began is dropped
        r.drive_dt(-100.0, 0.0, 100, true);
        let g = r.pl.last().gen;
        r.pl.reply(g, Ok((*straight(1000.0)).clone()));
        r.wait(100);
        assert_eq!(r.view().status, NavStatus::PausedRace);
        assert!(r.view().line.is_none());
    }

    #[test]
    fn paused_packets_are_ignored() {
        let mut r = routed();
        for _ in 0..40 {
            r.drive(100.0, 0.0);
        }
        let before = r.view();
        for _ in 0..200 {
            r.now += 100;
            let s = CarSample { x: 0.0, y: 0.0, z: 0.0, dt_ms: 100, driving: false, in_race: false };
            r.tick(Some(&s));
        }
        assert_eq!(r.view(), before);
        assert_eq!(r.pl.n(), 1);
    }

    #[test]
    fn local_arrival_clears_the_destination() {
        let mut r = routed();
        for x in (0..=980).step_by(20) {
            r.drive(x as f32, 0.0);
        }
        let v = r.view();
        assert_eq!(v.status, NavStatus::Arrived);
        assert!(v.dest.is_none() && v.line.is_none());
        assert_eq!(v.local_cleared_seq, 1);
        assert_eq!(r.sh.local_destination(), None, "the navigator's copy is cleared");
        // and it stays so: no new request
        r.wait(5000);
        assert_eq!(r.pl.n(), 1);
        assert_eq!(r.view().status, NavStatus::Arrived);
        assert!(!r.pl.0.borrow().watching, "back to idle: the graph watch stops");
        // a new destination leaves Arrived
        r.sh.set_destination(Some([-300.0, 0.0]));
        r.wait(100);
        assert_eq!(r.view().status, NavStatus::Routing);
    }

    #[test]
    fn shared_overrides_local_and_falls_back_on_clear() {
        let mut r = Rig::new();
        r.drive(0.0, 0.0);
        let own = RoutePrefs { filters: RouteFilters::ALL, curves: 0.1 };
        r.sh.set_prefs(own);
        r.sh.set_destination(Some([1000.0, 0.0]));
        r.wait(250);
        assert_eq!((r.pl.last().dest, r.pl.last().prefs), ((1000.0, 0.0), own));

        // Anna sets a destination: road + dirt only, curvy. Routed from our car with HER prefs.
        r.shared_dest(10, -600.0, 0.0, 1 | 4, 0.8);
        r.drive(0.0, 0.0);
        r.wait(250);
        assert_eq!(r.pl.n(), 2);
        let q = r.pl.last();
        assert_eq!(q.dest, (-600.0, 0.0));
        assert_eq!(q.prefs, RoutePrefs { filters: RouteFilters { road: true, dirt: true, ..RouteFilters::NONE }, curves: 0.8 });
        drop(q);
        let v = r.view();
        assert_eq!(v.dest.as_ref().unwrap().source, DestSource::Shared { setter: "Anna".into(), hue: 200.0 });
        // our own slider does not touch the shared route
        r.sh.set_prefs(RoutePrefs { curves: 0.9, ..own });
        r.wait(500);
        assert_eq!(r.pl.n(), 2);

        // she clears it: back to ours, with our (current) prefs
        r.t.set_shared(r.now, None);
        r.wait(250);
        assert_eq!(r.pl.n(), 3);
        assert_eq!((r.pl.last().dest, r.pl.last().prefs.curves), ((1000.0, 0.0), 0.9));
        assert_eq!(r.view().dest.unwrap().source, DestSource::Local);

        // opting out of following: a shared one is ignored...
        r.sh.set_follow_shared(false);
        r.shared_dest(11, 50.0, 50.0, 1, 0.0);
        r.wait(500);
        assert_eq!(r.pl.n(), 3);
        assert_eq!(r.view().dest.unwrap().source, DestSource::Local);
        // ...and adopted when it is switched on again
        r.sh.set_follow_shared(true);
        r.wait(250);
        assert_eq!(r.pl.n(), 4);
        assert_eq!(r.pl.last().dest, (50.0, 50.0));
    }

    #[test]
    fn shared_arrival_is_only_marked_locally() {
        let mut r = Rig::new();
        r.shared_dest(10, 1000.0, 0.0, 1, 0.0);
        r.drive(0.0, 0.0);
        r.wait(250);
        r.pl.answer(straight(1000.0));
        r.drive(0.0, 0.0);
        assert_eq!(r.view().status, NavStatus::Ok);
        for x in (0..=980).step_by(20) {
            r.drive(x as f32, 0.0);
        }
        let v = r.view();
        assert_eq!(v.status, NavStatus::Arrived);
        assert!(v.line.is_none());
        assert_eq!(v.dest.as_ref().map(|d| (d.x, matches!(d.source, DestSource::Shared { .. }))), Some((1000.0, true)), "still the room's destination");
        assert_eq!(v.local_cleared_seq, 0, "a shared arrival clears no local destination");
        // the late-joiner resend of the same destination does not bring it back
        r.shared_dest(10, 1000.0, 0.0, 1, 0.0);
        r.wait(2000);
        assert_eq!(r.view().status, NavStatus::Arrived);
        assert_eq!(r.pl.n(), 1);
        // a new one does
        r.shared_dest(11, 400.0, 0.0, 1, 0.0);
        r.wait(300);
        assert_eq!(r.view().status, NavStatus::Routing);
        // clearing the room's destination ends the "arrived" state
        r.t.set_shared(r.now, None);
        r.wait(100);
        assert_eq!(r.view().status, NavStatus::Idle);
        // and so does it when the room clears it after our arrival
        r.shared_dest(12, 1000.0, 0.0, 1, 0.0);
        r.wait(250);
        r.pl.answer(straight(1000.0));
        r.drive(0.0, 0.0);
        for x in (0..=980).step_by(20) {
            r.drive(x as f32, 0.0);
        }
        assert_eq!(r.view().status, NavStatus::Arrived);
        r.t.set_shared(r.now, None);
        assert_eq!(r.view().status, NavStatus::Idle);
    }

    #[test]
    fn shared_arrival_hands_over_to_the_local_destination() {
        let mut r = Rig::new();
        r.sh.set_destination(Some([-300.0, 0.0]));
        r.shared_dest(10, 400.0, 0.0, 1, 0.0);
        r.drive(0.0, 0.0);
        r.wait(250);
        r.pl.answer(straight(400.0));
        r.drive(0.0, 0.0);
        for x in (0..=380).step_by(20) {
            r.drive(x as f32, 0.0);
        }
        r.wait(300);
        assert_eq!(r.view().status, NavStatus::Routing);
        assert_eq!(r.pl.last().dest, (-300.0, 0.0), "now our own");
        assert_eq!(r.sh.local_destination(), Some([-300.0, 0.0]));
    }

    #[test]
    fn editing_the_prefs_keeps_the_old_line_until_the_new_one_arrives() {
        let mut r = routed();
        let old = r.view().line.unwrap();
        r.sh.set_prefs(RoutePrefs { curves: 0.7, ..Default::default() });
        r.wait(250);
        let v = r.view();
        assert_eq!(v.status, NavStatus::Routing);
        assert!(Arc::ptr_eq(v.line.as_ref().unwrap(), &old), "no flicker while the slider route is computed");
        r.pl.answer(straight(1100.0));
        r.drive(0.0, 0.0);
        let v = r.view();
        assert_eq!(v.status, NavStatus::Ok);
        assert!(!Arc::ptr_eq(v.line.as_ref().unwrap(), &old));
        // a different destination drops the line at once
        r.sh.set_destination(Some([5.0, 5.0]));
        r.wait(50);
        assert!(r.view().line.is_none());
    }

    /// The per-packet cost when no destination is set: one atomic load. (Run in release and read
    /// the printed number: `cargo test --release tick_cost -- --nocapture --ignored`.)
    #[test]
    #[ignore]
    fn tick_cost() {
        let mut r = Rig::new();
        let s = CarSample { x: 1.0, y: 2.0, z: 3.0, dt_ms: 14, driving: true, in_race: false };
        let n = 2_000_000u32;
        let t = Instant::now();
        for i in 0..n {
            r.t.tick(i as u64, Some(&s));
        }
        println!("idle tick: {:.1} ns", t.elapsed().as_nanos() as f64 / n as f64);
        r.sh.set_destination(Some([1000.0, 0.0]));
        r.t.tick(0, Some(&s));
        r.now = 0;
        r.wait(300);
        r.pl.answer(straight(20_000.0));
        r.t.tick(1000, Some(&s));
        let t = Instant::now();
        let n = 500_000u32;
        for i in 0..n {
            let x = (i % 18_000) as f32;
            let s = CarSample { x, y: 0.0, z: 1.0, dt_ms: 14, driving: true, in_race: false };
            r.t.tick(2000 + i as u64, Some(&s));
        }
        println!("following tick (20 km route): {:.1} ns", t.elapsed().as_nanos() as f64 / n as f64);
    }

    // ── the thread planner, with a scripted graph source ────────────────────────────────────────

    struct Script {
        state: Mutex<Option<Arc<RouteGraph>>>,
        loading_calls: AtomicU64,
        loading_until: u64,
    }

    impl GraphSource for Script {
        fn graph(&self) -> GraphState {
            let n = self.loading_calls.fetch_add(1, Ordering::SeqCst);
            if n < self.loading_until {
                return GraphState::Loading;
            }
            match &*lock(&self.state) {
                Some(g) => GraphState::Ready(g.clone()),
                None => GraphState::Unavailable,
            }
        }
    }

    fn tiny_graph(z: f32) -> Arc<RouteGraph> {
        use crate::gamedata::nav::{Nav, NavVert};
        use crate::gamedata::roadtypes::RoadTypes;
        let v = |id, x| NavVert { id, x, z, y: 0.0 };
        let nav = Nav { sha1: String::new(), nodes: 0, cls: vec![0], hi: vec![0], polys: vec![(0..=10).map(|i| v(i + 1, 100.0 * i as f32)).collect()], orphans: vec![] };
        let rt = RoadTypes::raw();
        Arc::new(RouteGraph::build(&nav, &rt, &crate::maprender::data::node_positions(&nav, &rt)))
    }

    fn wait_for<T>(pl: &mut ThreadPlanner, mut f: impl FnMut(Polled) -> Option<T>) -> T {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if let Some(x) = f(pl.poll()) {
                return x;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out");
    }

    #[test]
    fn the_worker_thread_waits_for_the_graph_and_routes() {
        let src = Arc::new(Script { state: Mutex::new(Some(tiny_graph(0.0))), loading_calls: AtomicU64::new(0), loading_until: 2 });
        let mut pl = ThreadPlanner::new(src.clone());
        assert!(pl.poll().reply.is_none(), "idle: nothing, no thread");
        pl.submit(Request { gen: 7, car: (50.0, 5.0, None), dest: (950.0, 0.0), prefs: RoutePrefs::default() });
        let r = wait_for(&mut pl, |p| p.reply);
        assert_eq!(r.gen, 7);
        let route = r.result.unwrap();
        assert!((route.dist_m - 900.0).abs() < 1.0, "{}", route.dist_m);
        assert!(src.loading_calls.load(Ordering::SeqCst) >= 3, "it polled the loading graph");
    }

    #[test]
    fn the_worker_thread_reports_a_missing_install_and_a_replaced_graph() {
        let src = Arc::new(Script { state: Mutex::new(Some(tiny_graph(0.0))), loading_calls: AtomicU64::new(0), loading_until: 0 });
        let mut pl = ThreadPlanner::new(src.clone());
        pl.watch(true);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!pl.poll().graph_changed, "the first look is not a change");
        *lock(&src.state) = Some(tiny_graph(0.0)); // a rebuilt graph: another Arc
        wait_for(&mut pl, |p| p.graph_changed.then_some(()));
        *lock(&src.state) = None;
        wait_for(&mut pl, |p| p.graph_changed.then_some(()));
        pl.submit(Request { gen: 1, car: (0.0, 0.0, None), dest: (900.0, 0.0), prefs: RoutePrefs::default() });
        let r = wait_for(&mut pl, |p| p.reply);
        assert_eq!(r.result, Err(RouteError::EmptyGraph));
        // the install appears again: that is a change, too
        *lock(&src.state) = Some(tiny_graph(0.0));
        wait_for(&mut pl, |p| p.graph_changed.then_some(()));
        pl.watch(false);
    }

    #[test]
    fn the_latest_request_wins() {
        let src = Arc::new(Script { state: Mutex::new(Some(tiny_graph(0.0))), loading_calls: AtomicU64::new(0), loading_until: 3 });
        let mut pl = ThreadPlanner::new(src);
        for gen in 1..=5 {
            pl.submit(Request { gen, car: (0.0, 0.0, None), dest: (900.0, 0.0), prefs: RoutePrefs::default() });
        }
        let r = wait_for(&mut pl, |p| p.reply);
        assert_eq!(r.gen, 5);
    }
}
