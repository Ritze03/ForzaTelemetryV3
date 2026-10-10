//! The Navigation tab (phase L, D83-D85): road-type filters, the faster-roads <-> more-curves
//! slider, the route's status / distance / ETA, the co-op switches, and a map where every left
//! click sets the destination. See `docs/features/navigation.md` section 4.
//!
//! Three jobs live here, kept apart so each can be tested without the app:
//!
//! * the **tab** ([`show`] / [`left_cards`]): a fixed-width left pane of cards and the map pane
//!   (`map_tab::map_pane`, the Map tab viewer's code with the Dashboard map's settings, D73);
//! * the **bridge** between `AppConfig.nav` and the navigator ([`sync`], [`set_destination`],
//!   [`clear_destination`]): the config is what is saved, the navigator (`nav::`) is what routes;
//! * the **co-op sharing** rule ([`room_op`]): when a destination set or cleared here also goes to
//!   the room, and when the room's own is cleared.

use egui::{pos2, vec2, Color32, FontId, Rect, Ui, UiBuilder};

use crate::app::ForzaApp;
use crate::i18n::tr;
use crate::nav::{self, DestSource, NavConfig, NavStatus, NavView, RoutePrefs};
use crate::theme;
use crate::ui::coop::hue_color;
use crate::ui::map_tab::{self, MapPane, MapTabState};
use crate::ui::overlay_tab::{control_row, status_line};

// ── state and the config <-> navigator bridge ─────────────────────────────────────────────────

/// Navigation state that is not saved: the tab's own map view, and what the app last told the
/// navigator.
#[derive(Default)]
pub struct NavState {
    /// The Navigation tab's map: its own pan / zoom and race-line selection (the Map tab's viewer
    /// has another), with the Dashboard map's settings.
    pub map: MapTabState,
    /// The last `NavView::local_cleared_seq` the config has taken over (0 = nothing arrived yet;
    /// the counter starts at 0 in this process, so a destination loaded from the config at start
    /// is never mistaken for an arrived one).
    seen: u64,
    /// What the navigator was last given; `None` until the first [`sync`] (which pushes the
    /// loaded config, the saved destination included).
    pushed: Option<Pushed>,
    /// Filters or slider changed: tell the room (if this player's destination is in it) once the
    /// pointer is released, not on every frame of a slider drag.
    resend: bool,
    /// When this co-op session came up, to share a destination set before joining.
    join: JoinWatch,
    /// The room's destination last frame (in a session), to notice a teammate clearing ours.
    room_seen: Option<crate::coop::SharedDest>,
}

/// Shares a destination that existed before the session (set earlier, or saved from the last
/// run) once per session, [`JOIN_GRACE`] after the session is up: by then a late joiner has
/// received the room's own destination (the host / peers resend it right after the handshake),
/// so [`room_op`] can leave a room that already has one alone.
#[derive(Default, Debug)]
struct JoinWatch {
    /// When the session was first seen up (`None`: not yet, or co-op is off).
    since: Option<std::time::Instant>,
    /// Already decided for this session.
    done: bool,
}

/// How long after the session is up the join share waits for the room's destination.
const JOIN_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// `session`: co-op is on; `up`: the session is connected and this player has its id (listed
/// in the roster). True once per session, [`JOIN_GRACE`] after `up` was first seen.
fn join_due(w: &mut JoinWatch, session: bool, up: bool, now: std::time::Instant) -> bool {
    if !session {
        *w = JoinWatch::default();
        return false;
    }
    if w.done {
        return false;
    }
    let since = match w.since {
        Some(t) => t,
        None if up => *w.since.insert(now),
        None => return false,
    };
    if now.duration_since(since) < JOIN_GRACE {
        return false;
    }
    w.done = true;
    true
}

#[derive(Clone, PartialEq, Debug)]
struct Pushed {
    dest: Option<[f32; 2]>,
    prefs: RoutePrefs,
    follow: bool,
}

/// The inputs the navigator has to be given because they differ from what it has.
#[derive(Default, PartialEq, Debug)]
struct Push {
    dest: Option<Option<[f32; 2]>>,
    prefs: Option<RoutePrefs>,
    follow: Option<bool>,
}

impl Push {
    fn is_empty(&self) -> bool {
        *self == Push::default()
    }
}

fn wanted(cfg: &NavConfig) -> Pushed {
    // `curves()` clamps (and turns NaN into 0): a NaN would never compare equal and be "changed"
    // every frame.
    let mut prefs = cfg.prefs();
    prefs.curves = prefs.curves();
    Pushed { dest: cfg.destination, prefs, follow: cfg.follow_shared }
}

/// What differs between `pushed` and `want`; `pushed` becomes `want`. The first call (`None`)
/// differs in everything, so the loaded config reaches the navigator once.
fn diff(pushed: &mut Option<Pushed>, want: Pushed) -> Push {
    let out = match pushed.as_ref() {
        None => Push { dest: Some(want.dest), prefs: Some(want.prefs), follow: Some(want.follow) },
        Some(p) => Push {
            dest: (p.dest != want.dest).then_some(want.dest),
            prefs: (p.prefs != want.prefs).then_some(want.prefs),
            follow: (p.follow != want.follow).then_some(want.follow),
        },
    };
    *pushed = Some(want);
    out
}

fn apply_push(p: &Push) {
    if let Some(d) = p.dest {
        nav::set_destination(d);
    }
    if let Some(x) = p.prefs {
        nav::set_prefs(x);
    }
    if let Some(f) = p.follow {
        nav::set_follow_shared(f);
    }
}

/// The navigator cleared this player's own destination because the car arrived (`cleared_seq`
/// moved): drop the saved copy too, or it would come back at the next start. Returns whether
/// the counter moved.
///
/// `navigator_local` is the destination the navigator holds *now*: if it is not empty the user
/// set a new one after the arrival (the click and the arrival raced inside one frame), and that
/// one must stay.
fn take_arrival(seen: &mut u64, cleared_seq: u64, navigator_local: Option<[f32; 2]>, dest: &mut Option<[f32; 2]>) -> bool {
    if cleared_seq == *seen {
        return false;
    }
    *seen = cleared_seq;
    if navigator_local.is_none() {
        *dest = None;
    }
    true
}

/// Once per frame, before any tab draws: take over an arrival, then hand the navigator whatever
/// of the config differs from what it has (everything on the first frame), and send a pending
/// filter change to the room.
pub fn sync(app: &mut ForzaApp, ctx: &egui::Context) {
    let v = nav::view();
    take_arrival(&mut app.nav_ui.seen, v.local_cleared_seq, nav::local_destination(), &mut app.config.nav.destination);
    push_inputs(app);
    if app.nav_ui.resend && !ctx.input(|i| i.pointer.any_down()) {
        app.nav_ui.resend = false;
        room_event(app, RoomEvent::PrefsChanged, &v);
    }
    let session = app.coop.role() != crate::coop::Role::Off;
    // A host's room exists at once (its tunnel may still be starting); a client is up once
    // connected and welcomed.
    let host = app.coop.role() == crate::coop::Role::Host;
    let up = session && !app.nav_ui.join.done && (host || !app.coop.is_connecting()) && {
        let me = app.coop.my_id();
        app.coop.roster().iter().any(|p| p.id == me)
    };
    if join_due(&mut app.nav_ui.join, session, up, std::time::Instant::now()) {
        room_event(app, RoomEvent::Joined, &v);
    }
    let room = if session { app.coop.destination() } else { None };
    let prev = std::mem::replace(&mut app.nav_ui.room_seen, room.clone());
    if room.is_none() && session {
        let by = app.coop.cleared_by();
        if cleared_by_teammate(prev.as_ref(), by.as_deref(), &app.coop.my_id(), app.config.nav.destination) {
            app.config.nav.destination = None;
            push_inputs(app);
        }
    }
}

/// The room's destination was this player's own (`prev`, last frame) and a teammate has just
/// cleared it for everyone (`cleared_by`, the clearer, while the room has none): the own copy of
/// it (`local`, the saved destination the share came from) goes too, or "Clear for everyone"
/// would leave the setter navigating there. Not when this player took it back themselves
/// (sharing switched off: the clearer is them) or the session ended (no clearer).
fn cleared_by_teammate(prev: Option<&crate::coop::SharedDest>, cleared_by: Option<&str>, my_id: &str, local: Option<[f32; 2]>) -> bool {
    let Some(p) = prev else { return false };
    p.setter_id == my_id && cleared_by.is_some_and(|by| by != my_id) && local == Some([p.x, p.z])
}

/// Before the config is saved at exit: an arrival the UI has not seen yet (the window was
/// covered by the game, so no frame ran since) must not leave the old destination in the file.
pub fn take_arrival_now(app: &mut ForzaApp) {
    let v = nav::view();
    take_arrival(&mut app.nav_ui.seen, v.local_cleared_seq, nav::local_destination(), &mut app.config.nav.destination);
}

fn push_inputs(app: &mut ForzaApp) {
    let push = diff(&mut app.nav_ui.pushed, wanted(&app.config.nav));
    if !push.is_empty() {
        apply_push(&push);
    }
}

/// A click on a map: this is the destination now. Saved, handed to the navigator at once, and
/// shared with the room when that is on.
pub(crate) fn set_destination(app: &mut ForzaApp, pos: [f32; 2]) {
    app.config.nav.destination = Some(pos);
    push_inputs(app);
    room_event(app, RoomEvent::Set(pos), &nav::view());
}

/// The Clear buttons. Clears this player's own destination and, for a shared one (followed, or
/// this player's own in the room), the room's too: anyone may clear it for everyone (D92).
pub(crate) fn clear_destination(app: &mut ForzaApp) {
    room_event(app, RoomEvent::Clear, &nav::view());
    app.config.nav.destination = None;
    push_inputs(app);
}

// ── co-op sharing ─────────────────────────────────────────────────────────────────────────────

/// Something the user did that may change the room's destination.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum RoomEvent {
    /// A new destination of their own.
    Set([f32; 2]),
    /// Clear pressed.
    Clear,
    /// "Share my destination" switched on or off.
    ShareToggled,
    /// Their filters or slider changed.
    PrefsChanged,
    /// The session came up (a few seconds ago) and this player had a destination already.
    Joined,
}

/// What to do to the room.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum RoomOp {
    Set([f32; 2]),
    Clear,
}

/// The facts [`room_op`] decides on.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(crate) struct RoomCtx {
    pub in_session: bool,
    /// The room has a destination ...
    pub room_has: bool,
    /// ... and it was set by this player (it came back to the navigator as a shared one).
    pub room_mine: bool,
    /// The navigator is following a shared destination (this player's own or a teammate's).
    pub followed_shared: bool,
}

/// When a destination change here also goes to the room (D85). `nav` is the config *after* the
/// change. Without a session there is no room; sharing needs `share_destination`.
///
/// * Set: shared when sharing is on.
/// * Clear: clears the room's when the destination being cleared is a shared one (followed, or
///   this player's own while sharing), for everyone. A teammate's destination that this player
///   does not follow (`follow_shared` off) is left alone.
/// * Sharing switched on with a destination: send it. Switched off: take this player's own out of
///   the room (a teammate's stays).
/// * Joined a session with a destination already set: send it if sharing is on and the room has
///   none (a room's destination is not replaced by one that only predates the join).
/// * Filters / slider changed: the room's copy of this player's destination carries the old
///   ones, so send it again (the navigator routes a shared destination, even one's own, with the
///   room's filters).
pub(crate) fn room_op(ev: RoomEvent, nav: &NavConfig, c: &RoomCtx) -> Option<RoomOp> {
    if !c.in_session {
        return None;
    }
    match ev {
        RoomEvent::Set(p) => nav.share_destination.then_some(RoomOp::Set(p)),
        RoomEvent::Clear => (c.followed_shared || (nav.share_destination && c.room_has && c.room_mine)).then_some(RoomOp::Clear),
        RoomEvent::ShareToggled => match nav.destination {
            Some(p) if nav.share_destination => Some(RoomOp::Set(p)),
            _ if !nav.share_destination && c.room_has && c.room_mine => Some(RoomOp::Clear),
            _ => None,
        },
        RoomEvent::Joined => match nav.destination {
            Some(p) if nav.share_destination && !c.room_has => Some(RoomOp::Set(p)),
            _ => None,
        },
        RoomEvent::PrefsChanged => match nav.destination {
            Some(p) if nav.share_destination && c.room_has && c.room_mine => Some(RoomOp::Set(p)),
            _ => None,
        },
    }
}

fn room_event(app: &ForzaApp, ev: RoomEvent, v: &NavView) {
    if app.coop.role() == crate::coop::Role::Off {
        return;
    }
    let room = app.coop.destination();
    let c = RoomCtx {
        in_session: true,
        room_has: room.is_some(),
        room_mine: room.as_ref().is_some_and(|d| d.setter_id == app.coop.my_id()),
        followed_shared: matches!(v.dest.as_ref().map(|d| &d.source), Some(DestSource::Shared { .. })),
    };
    let n = &app.config.nav;
    match room_op(ev, n, &c) {
        Some(RoomOp::Set(p)) => app.coop.set_destination((p[0], p[1]), app.config.coop_hue, n.filters.to_bits(), n.prefs().curves()),
        Some(RoomOp::Clear) => app.coop.clear_destination(),
        None => {}
    }
}

/// The room's destination as the tab shows it.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct RoomInfo {
    pub name: String,
    pub hue: f32,
    /// Set by this player.
    pub mine: bool,
}

fn room_info(app: &ForzaApp) -> Option<RoomInfo> {
    if app.coop.role() == crate::coop::Role::Off {
        return None;
    }
    let d = app.coop.destination()?;
    let mine = d.setter_id == app.coop.my_id();
    // The setter may have left (the destination stays the room's): no name then.
    let name = if d.setter_name.is_empty() { tr("a teammate").to_string() } else { d.setter_name };
    Some(RoomInfo { name, hue: d.hue, mine })
}

// ── the tab ───────────────────────────────────────────────────────────────────────────────────

/// Width of the left pane at most (D83 layout), and its share of the tab on a narrow window.
const LEFT_MAX_W: f32 = 360.0;
const LEFT_SHARE: f32 = 0.4;
const GAP: f32 = 8.0;

/// The two panes of the tab: the cards on the left, the map filling the rest.
pub(crate) fn split(full: Rect) -> (Rect, Rect) {
    let w = (full.width() * LEFT_SHARE).min(LEFT_MAX_W);
    let left = Rect::from_min_size(full.min, vec2(w, full.height()));
    let right = Rect::from_min_max(pos2(full.left() + w + GAP, full.top()), full.max);
    (left, right)
}

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    app.ensure_map_image();
    let full = ui.available_rect_before_wrap();
    let (left, right) = split(full);

    let view = nav::view();
    let pane = PaneIn {
        pending: app.config.nav.destination.is_some() && view.dest.is_none(),
        view: &view,
        in_session: app.coop.role() != crate::coop::Role::Off,
        room: room_info(app),
        use_mph: app.config.use_mph,
    };
    let mut cfg = app.config.nav.clone();
    let out = ui
        .scope_builder(UiBuilder::new().max_rect(left), |ui| {
            ui.shrink_clip_rect(left);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| left_cards(ui, &mut cfg, &pane)).inner
        })
        .inner;
    app.config.nav = cfg;

    if out.prefs_changed {
        app.nav_ui.resend = true;
    }
    if out.share_toggled {
        room_event(app, RoomEvent::ShareToggled, &view);
    }
    if out.clear {
        clear_destination(app);
    }
    push_inputs(app);

    // The map pane: a pane of its own, clipped (the Panes rule).
    ui.scope_builder(UiBuilder::new().max_rect(right), |ui| {
        ui.shrink_clip_rect(right);
        map_tab::map_pane(ui, app, right, MapPane::Navigation);
    });
}

/// What the left pane draws from.
pub(crate) struct PaneIn<'a> {
    pub view: &'a NavView,
    /// This player's own destination is saved but the navigator has not published it yet.
    pub pending: bool,
    pub in_session: bool,
    pub room: Option<RoomInfo>,
    pub use_mph: bool,
}

/// What the left pane's controls asked for.
#[derive(Default, Debug, PartialEq)]
pub(crate) struct PaneOut {
    pub clear: bool,
    /// Filters or slider edited (not set while they are read-only).
    pub prefs_changed: bool,
    pub share_toggled: bool,
}

impl PaneIn<'_> {
    /// The preferences of the shared destination being followed, which the filters and slider then
    /// show read-only: a group routes over the same roads (D85). This player's own destination in
    /// the room is not "followed": their controls stay theirs.
    fn followed(&self) -> Option<RoutePrefs> {
        let d = self.view.dest.as_ref()?;
        let mine = self.room.as_ref().is_some_and(|r| r.mine);
        (matches!(d.source, DestSource::Shared { .. }) && !mine).then_some(d.prefs)
    }

    /// A shared destination (a teammate's or this player's own) is the one being navigated to.
    fn shared(&self) -> bool {
        self.view.dest.as_ref().is_some_and(|d| matches!(d.source, DestSource::Shared { .. }))
    }

    fn status(&self) -> NavStatus {
        if self.pending && self.view.status == NavStatus::Idle {
            NavStatus::Routing
        } else {
            self.view.status
        }
    }
}

/// The four cards of the left pane. `nav` is edited in place.
pub(crate) fn left_cards(ui: &mut Ui, nav: &mut NavConfig, p: &PaneIn) -> PaneOut {
    ui.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
    let mut out = PaneOut::default();
    let followed = p.followed();
    route_card(ui, p, &mut out);
    road_types_card(ui, nav, followed, &mut out);
    preference_card(ui, nav, followed, &mut out);
    coop_card(ui, nav, p, &mut out);
    out
}

fn status_text(s: NavStatus) -> (Color32, &'static str) {
    use crate::nav::Endpoint;
    match s {
        NavStatus::Idle => (theme::FAINT, tr("Click the map to set a destination")),
        NavStatus::WaitingForCar => (theme::WARN, tr("Waiting for the car's position")),
        NavStatus::Routing => (theme::WARN, tr("Calculating…")),
        NavStatus::Ok => (theme::GOOD, tr("On route")),
        NavStatus::NoRoadNear(Endpoint::Car) => (theme::DANGER, tr("No road near the car")),
        NavStatus::NoRoadNear(Endpoint::Destination) => (theme::DANGER, tr("No road near the destination")),
        NavStatus::Unreachable => (theme::DANGER, tr("No route with these road types")),
        NavStatus::NoRoadData => (theme::DANGER, tr("No road data (needs your Forza Horizon 6 install)")),
        NavStatus::PausedRace => (theme::WARN, tr("Paused during a race")),
        NavStatus::Arrived => (theme::GOOD, tr("Arrived")),
    }
}

/// "~12.4 km" / "~850 m", or miles / feet when `mph` (the app's unit switch, design 11).
pub(crate) fn fmt_distance(m: f32, mph: bool) -> String {
    let m = m.max(0.0);
    if mph {
        let mi = m / 1609.344;
        if mi < 0.1 {
            format!("~{:.0} ft", (m * 3.280_84 / 10.0).round() * 10.0)
        } else if mi < 100.0 {
            format!("~{mi:.1} mi")
        } else {
            format!("~{mi:.0} mi")
        }
    } else if m < 1000.0 {
        format!("~{:.0} m", (m / 10.0).round() * 10.0)
    } else if m < 100_000.0 {
        format!("~{:.1} km", m / 1000.0)
    } else {
        format!("~{:.0} km", m / 1000.0)
    }
}

/// "~9 min", "~1 h 05 min", "< 1 min".
pub(crate) fn fmt_eta(s: f32) -> String {
    let min = (s.max(0.0) / 60.0).round() as u32;
    if s < 30.0 {
        "< 1 min".to_string()
    } else if min < 60 {
        format!("~{min} min")
    } else {
        format!("~{} h {:02} min", min / 60, min % 60)
    }
}

fn route_card(ui: &mut Ui, p: &PaneIn, out: &mut PaneOut) {
    theme::card(ui, tr("Route"), |ui| {
        let (col, text) = status_text(p.status());
        status_line(ui, col, text);
        if p.shared() {
            if let Some(r) = &p.room {
                let who = if r.mine { tr("you").to_string() } else { r.name.clone() };
                status_line(ui, hue_color(r.hue), &format!("{} {}", tr("Set by"), who));
            }
        }
        let have = p.view.line.is_some() && matches!(p.status(), NavStatus::Ok | NavStatus::Routing);
        let (dist, eta) = if have { (fmt_distance(p.view.remaining_m, p.use_mph), fmt_eta(p.view.eta_s)) } else { ("–".into(), "–".into()) };
        ui.add_space(2.0);
        control_row(ui, tr("Distance"), |ui| ui.label(dist));
        control_row(ui, tr("Time"), |ui| ui.label(eta));
        let has_dest = p.view.dest.is_some() || p.pending;
        let label = if p.shared() { tr("Clear for everyone") } else { tr("Clear route") };
        ui.add_space(2.0);
        let b = ui.add_enabled(has_dest, theme::danger_button(label));
        let b = if p.shared() { b.on_hover_text(tr("Clears the destination for the whole co-op room.")) } else { b };
        out.clear |= b.clicked();
    });
}

/// One road-type checkbox: `shown` is what is drawn (the setter's while following), `value` is
/// written only when editable. The tooltip also shows while the row is greyed.
fn road_row(ui: &mut Ui, shown: &mut bool, label: &'static str, tip: &'static str) -> bool {
    theme::checkbox_row(ui, shown, tr(label)).on_hover_text(tr(tip)).on_disabled_hover_text(tr(tip)).changed()
}

fn road_types_card(ui: &mut Ui, nav: &mut NavConfig, followed: Option<RoutePrefs>, out: &mut PaneOut) {
    theme::card(ui, tr("Road types"), |ui| {
        let mut f = followed.map_or(nav.filters, |p| p.filters);
        let mut changed = false;
        ui.add_enabled_ui(followed.is_none(), |ui| {
            changed |= road_row(ui, &mut f.road, "Road", "Paved roads, also those of another or unknown type. Tunnels are driven when Road or Highway is on. Turnaround crossovers are never used.");
            changed |= road_row(ui, &mut f.highway, "Highway", "Motorways. Tunnels are also driven when this is on.");
            changed |= road_row(ui, &mut f.dirt, "Dirt", "Dirt and off-road tracks.");
            changed |= road_row(ui, &mut f.trail, "Trail", "Small game trails.");
            changed |= road_row(ui, &mut f.cross_country, "Cross-country", "Straight lines across country that are no road in the game.");
            changed |= road_row(ui, &mut f.jumps, "Jumps", "One-way jumps from take-off to landing. Risky: the landing can wreck the car.");
        });
        if changed && followed.is_none() {
            nav.filters = f;
            out.prefs_changed = true;
        }
    });
}

fn preference_card(ui: &mut Ui, nav: &mut NavConfig, followed: Option<RoutePrefs>, out: &mut PaneOut) {
    theme::card(ui, tr("Preference"), |ui| {
        let tip = tr("Left: the fastest roads. Right: the most winding roads. The route is always the best one under the chosen road types.");
        let mut pct = followed.map_or(nav.prefs(), |p| p).curves() * 100.0;
        theme::columns(ui, 2, |c| {
            c[0].add(egui::Label::new(egui::RichText::new(tr("Faster roads")).size(11.0).color(theme::TEXT_DIM)).wrap());
            c[1].with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(tr("More curves")).size(11.0).color(theme::TEXT_DIM)).wrap());
            });
        });
        let mut changed = false;
        ui.add_enabled_ui(followed.is_none(), |ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 8.0).max(40.0);
            let r = ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).show_value(false)).on_hover_text(tip).on_disabled_hover_text(tip);
            changed = r.changed();
        });
        if changed && followed.is_none() {
            nav.curves = pct / 100.0;
            out.prefs_changed = true;
        }
    });
}

fn coop_card(ui: &mut Ui, nav: &mut NavConfig, p: &PaneIn, out: &mut PaneOut) {
    theme::card(ui, tr("Co-Op"), |ui| {
        if !p.in_session {
            status_line(ui, theme::FAINT, tr("Not in a co-op session"));
        }
        ui.add_enabled_ui(p.in_session, |ui| {
            let tip = tr("Send the destination you set to your co-op room. Teammates who follow shared destinations navigate there with your road types.");
            let r = theme::checkbox_row(ui, &mut nav.share_destination, tr("Share my destination")).on_hover_text(tip).on_disabled_hover_text(tip);
            out.share_toggled |= r.changed();
            let tip = tr("Navigate to the destination a teammate sets, from your own car and with their road types. Off: keep your own destination.");
            theme::checkbox_row(ui, &mut nav.follow_shared, tr("Follow shared destinations")).on_hover_text(tip).on_disabled_hover_text(tip);
            match (&p.room, p.in_session) {
                (Some(r), true) => {
                    let who = if r.mine { tr("you").to_string() } else { r.name.clone() };
                    status_line(ui, hue_color(r.hue), &format!("{} {}", tr("Shared destination set by"), who));
                }
                (None, true) => status_line(ui, theme::FAINT, tr("No shared destination")),
                _ => {}
            }
        });
    });
}

/// A faint hint at the bottom of the Navigation tab's map while there is no destination. Left
/// out on a narrow map, where it would run into the buttons.
pub(crate) fn hint_pill(ui: &Ui, rect: Rect) {
    if rect.width() < 520.0 {
        return;
    }
    let g = ui.painter().layout_no_wrap(tr("Click the map to set a destination").to_string(), FontId::proportional(12.0), theme::TEXT_DIM);
    let at = pos2(rect.center().x - g.size().x / 2.0, rect.bottom() - 10.0 - g.size().y - 5.0);
    ui.painter().rect_filled(Rect::from_min_size(at, g.size()).expand2(vec2(10.0, 5.0)), 10.0, Color32::from_black_alpha(150));
    ui.painter().galley(at, g, theme::TEXT_DIM);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{with_language, Language};
    use crate::nav::cfg::RouteFilters;
    use crate::nav::{Dest, Endpoint};
    use crate::ui::overlay_tab::tests::{check_panes, render};

    fn dest(source: DestSource, prefs: RoutePrefs) -> Dest {
        Dest { x: 1000.0, z: -2000.0, source, prefs }
    }

    fn line() -> std::sync::Arc<nav::NavLine> {
        std::sync::Arc::new(nav::NavLine { rev: 1, pts: vec![[0.0, 0.0], [100.0, 0.0]], y: vec![0.0, 0.0], seg_kind: vec![1] })
    }

    // ── the bridge ──

    /// A destination loaded from the config is handed to the navigator on the first frame, and a
    /// startup never counts as an arrival (the counter starts at the 0 the state starts at).
    #[test]
    fn the_loaded_destination_reaches_the_navigator_and_startup_clears_nothing() {
        let mut cfg = NavConfig::default();
        cfg.destination = Some([5.0, 6.0]);
        cfg.curves = 0.4;
        let mut pushed = None;
        let p = diff(&mut pushed, wanted(&cfg));
        assert_eq!(p.dest, Some(Some([5.0, 6.0])));
        assert_eq!(p.prefs.map(|x| x.curves), Some(0.4));
        assert_eq!(p.follow, Some(true));
        let mut seen = 0;
        assert!(!take_arrival(&mut seen, 0, Some([5.0, 6.0]), &mut cfg.destination));
        assert_eq!(cfg.destination, Some([5.0, 6.0]), "the freshly loaded destination stays");
        assert!(diff(&mut pushed, wanted(&cfg)).is_empty(), "idempotent: nothing to push the second time");
    }

    /// Only what changed is pushed; a NaN slider does not look changed every frame.
    #[test]
    fn only_a_changed_input_is_pushed() {
        let mut cfg = NavConfig::default();
        let mut pushed = None;
        diff(&mut pushed, wanted(&cfg));
        cfg.filters.jumps = true;
        let p = diff(&mut pushed, wanted(&cfg));
        assert!(p.dest.is_none() && p.follow.is_none());
        assert!(p.prefs.is_some_and(|x| x.filters.jumps));
        cfg.follow_shared = false;
        assert_eq!(diff(&mut pushed, wanted(&cfg)), Push { follow: Some(false), ..Push::default() });
        cfg.destination = Some([1.0, 2.0]);
        assert_eq!(diff(&mut pushed, wanted(&cfg)), Push { dest: Some(Some([1.0, 2.0])), ..Push::default() });
        cfg.curves = 0.5;
        diff(&mut pushed, wanted(&cfg));
        cfg.curves = f32::NAN;
        let first = diff(&mut pushed, wanted(&cfg));
        assert!(first.prefs.is_some());
        assert!(diff(&mut pushed, wanted(&cfg)).is_empty(), "NaN is clamped to 0, not 'changed' every frame");
    }

    /// An arrival clears the saved destination exactly once.
    #[test]
    fn a_local_arrival_clears_the_saved_destination_exactly_once() {
        let mut dest = Some([5.0, 6.0]);
        let mut seen = 0;
        // The navigator cleared its copy and counted the arrival.
        assert!(take_arrival(&mut seen, 1, None, &mut dest));
        assert_eq!(dest, None);
        assert_eq!(seen, 1);
        // The user sets a new destination; the counter has not moved, so it is left alone.
        dest = Some([7.0, 8.0]);
        assert!(!take_arrival(&mut seen, 1, Some([7.0, 8.0]), &mut dest));
        assert_eq!(dest, Some([7.0, 8.0]), "no second clear for the same arrival");
        // A second arrival clears again.
        assert!(take_arrival(&mut seen, 2, None, &mut dest));
        assert_eq!(dest, None);
    }

    /// A click that landed in the same frame as an arrival is not undone by it.
    #[test]
    fn an_arrival_does_not_clear_a_destination_set_in_the_same_frame() {
        let mut dest = Some([9.0, 9.0]);
        let mut seen = 0;
        assert!(take_arrival(&mut seen, 1, Some([9.0, 9.0]), &mut dest));
        assert_eq!(dest, Some([9.0, 9.0]));
        assert_eq!(seen, 1, "the arrival is still consumed");
    }

    // ── co-op sharing ──

    fn cfg(share: bool, dest: Option<[f32; 2]>) -> NavConfig {
        NavConfig { share_destination: share, destination: dest, ..NavConfig::default() }
    }

    fn ctx(has: bool, mine: bool, followed: bool) -> RoomCtx {
        RoomCtx { in_session: true, room_has: has, room_mine: mine, followed_shared: followed }
    }

    #[test]
    fn nothing_goes_to_the_room_outside_a_session() {
        let off = RoomCtx::default();
        for ev in [RoomEvent::Set([1.0, 2.0]), RoomEvent::Clear, RoomEvent::ShareToggled, RoomEvent::PrefsChanged, RoomEvent::Joined] {
            assert_eq!(room_op(ev, &cfg(true, Some([1.0, 2.0])), &off), None, "{ev:?}");
        }
    }

    #[test]
    fn setting_a_destination_is_shared_only_when_sharing_is_on() {
        let p = [3.0, 4.0];
        assert_eq!(room_op(RoomEvent::Set(p), &cfg(true, Some(p)), &ctx(false, false, false)), Some(RoomOp::Set(p)));
        assert_eq!(room_op(RoomEvent::Set(p), &cfg(false, Some(p)), &ctx(false, false, false)), None);
    }

    #[test]
    fn clearing_clears_the_room_when_the_destination_was_a_shared_one() {
        let c = cfg(true, None);
        // Following a teammate's (or one's own, shared): everyone's is cleared.
        assert_eq!(room_op(RoomEvent::Clear, &c, &ctx(true, false, true)), Some(RoomOp::Clear));
        assert_eq!(room_op(RoomEvent::Clear, &cfg(false, None), &ctx(true, false, true)), Some(RoomOp::Clear), "also with sharing off: anyone may clear it");
        // This player's own, in the room, sharing on: cleared there too.
        assert_eq!(room_op(RoomEvent::Clear, &c, &ctx(true, true, false)), Some(RoomOp::Clear));
        // A teammate's destination this player does not follow is not theirs to clear.
        assert_eq!(room_op(RoomEvent::Clear, &c, &ctx(true, false, false)), None);
        // Nothing in the room.
        assert_eq!(room_op(RoomEvent::Clear, &c, &ctx(false, false, false)), None);
    }

    #[test]
    fn switching_sharing_on_sends_the_destination_and_off_takes_it_back() {
        let p = [1.0, 2.0];
        assert_eq!(room_op(RoomEvent::ShareToggled, &cfg(true, Some(p)), &ctx(false, false, false)), Some(RoomOp::Set(p)));
        assert_eq!(room_op(RoomEvent::ShareToggled, &cfg(true, None), &ctx(false, false, false)), None, "no destination, nothing to share");
        assert_eq!(room_op(RoomEvent::ShareToggled, &cfg(false, Some(p)), &ctx(true, true, false)), Some(RoomOp::Clear));
        assert_eq!(room_op(RoomEvent::ShareToggled, &cfg(false, Some(p)), &ctx(true, false, false)), None, "a teammate's stays");
    }

    #[test]
    fn a_destination_set_before_joining_is_shared_unless_the_room_has_one() {
        let p = [1.0, 2.0];
        assert_eq!(room_op(RoomEvent::Joined, &cfg(true, Some(p)), &ctx(false, false, false)), Some(RoomOp::Set(p)));
        assert_eq!(room_op(RoomEvent::Joined, &cfg(true, Some(p)), &ctx(true, false, true)), None, "the room's stays");
        assert_eq!(room_op(RoomEvent::Joined, &cfg(false, Some(p)), &ctx(false, false, false)), None, "sharing off");
        assert_eq!(room_op(RoomEvent::Joined, &cfg(true, None), &ctx(false, false, false)), None, "nothing to share");
    }

    #[test]
    fn a_teammate_clearing_my_shared_destination_clears_my_copy() {
        let mine = crate::coop::SharedDest { setter_id: "me".into(), setter_name: "Me".into(), x: 1.0, z: 2.0, hue: 0.0, filter_bits: 1, curve: 0.0, ts: 5 };
        let theirs = crate::coop::SharedDest { setter_id: "mate".into(), ..mine.clone() };
        let local = Some([1.0, 2.0]);
        assert!(cleared_by_teammate(Some(&mine), Some("mate"), "me", local));
        assert!(!cleared_by_teammate(Some(&mine), Some("me"), "me", local), "taken back by me (sharing off): keep my own");
        assert!(!cleared_by_teammate(Some(&mine), None, "me", local), "session over: keep it");
        assert!(!cleared_by_teammate(Some(&theirs), Some("mate"), "me", local), "a teammate's: my own stays underneath");
        assert!(!cleared_by_teammate(Some(&mine), Some("mate"), "me", Some([9.0, 9.0])), "I have another one by now");
        assert!(!cleared_by_teammate(None, Some("mate"), "me", local));
    }

    #[test]
    fn the_join_share_fires_once_per_session_after_the_grace() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut w = JoinWatch::default();
        assert!(!join_due(&mut w, true, false, t0), "connecting");
        assert!(!join_due(&mut w, true, true, t0 + Duration::from_secs(5)), "up: the grace starts now");
        assert!(!join_due(&mut w, true, true, t0 + Duration::from_secs(7)));
        assert!(join_due(&mut w, true, true, t0 + Duration::from_secs(8)));
        assert!(!join_due(&mut w, true, true, t0 + Duration::from_secs(20)), "once per session");
        assert!(!join_due(&mut w, true, false, t0 + Duration::from_secs(21)), "a reconnect does not repeat it");
        assert!(!join_due(&mut w, false, false, t0 + Duration::from_secs(22)), "session over");
        assert!(!join_due(&mut w, true, true, t0 + Duration::from_secs(30)), "next session: grace again");
        assert!(join_due(&mut w, true, true, t0 + Duration::from_secs(33)));
    }

    #[test]
    fn changed_filters_resend_the_players_own_shared_destination() {
        let p = [1.0, 2.0];
        assert_eq!(room_op(RoomEvent::PrefsChanged, &cfg(true, Some(p)), &ctx(true, true, false)), Some(RoomOp::Set(p)));
        assert_eq!(room_op(RoomEvent::PrefsChanged, &cfg(true, Some(p)), &ctx(true, false, false)), None, "not another player's");
        assert_eq!(room_op(RoomEvent::PrefsChanged, &cfg(false, Some(p)), &ctx(true, true, false)), None);
        assert_eq!(room_op(RoomEvent::PrefsChanged, &cfg(true, None), &ctx(true, true, false)), None);
    }

    /// The co-op calls the tab relies on are no-ops outside a session, so a stray call can never
    /// start anything (the in-session behaviour is covered by `coop.rs`'s own tests).
    #[test]
    fn the_coop_calls_do_nothing_outside_a_session() {
        let st = crate::coop::CoopState::new("Me", 10.0, 0);
        st.set_destination((1.0, 2.0), 10.0, RouteFilters::default().to_bits(), 0.0);
        assert!(st.destination().is_none());
        st.clear_destination();
        assert!(st.destination().is_none());
    }

    // ── formatting ──

    #[test]
    fn distances_and_times_read_naturally_in_both_units() {
        assert_eq!(fmt_distance(850.0, false), "~850 m");
        assert_eq!(fmt_distance(12_449.0, false), "~12.4 km");
        assert_eq!(fmt_distance(123_000.0, false), "~123 km");
        assert_eq!(fmt_distance(12_449.0, true), "~7.7 mi");
        assert_eq!(fmt_distance(100.0, true), "~330 ft");
        assert_eq!(fmt_eta(10.0), "< 1 min");
        assert_eq!(fmt_eta(9.0 * 60.0), "~9 min");
        assert_eq!(fmt_eta(65.0 * 60.0), "~1 h 05 min");
    }

    // ── layout ──

    #[test]
    fn the_two_panes_split_without_overlap() {
        for w in [600.0, 700.0, 1000.0, 1235.0, 1920.0] {
            let full = Rect::from_min_size(pos2(8.0, 8.0), vec2(w, 600.0));
            let (l, r) = split(full);
            assert!(l.width() <= LEFT_MAX_W && l.width() <= w * LEFT_SHARE + 0.01);
            assert_eq!(l.left(), full.left());
            assert_eq!(r.right(), full.right());
            assert!((r.left() - l.right() - GAP).abs() < 0.01, "an 8px gap at {w}");
            assert!(!l.intersects(r));
        }
    }

    fn states() -> Vec<(&'static str, NavView, bool, Option<RoomInfo>)> {
        let prefs = RoutePrefs { filters: RouteFilters::ALL, curves: 0.7 };
        let mut v = Vec::new();
        v.push(("idle", NavView::default(), false, None));
        for (name, status) in [
            ("waiting", NavStatus::WaitingForCar),
            ("routing", NavStatus::Routing),
            ("nocar", NavStatus::NoRoadNear(Endpoint::Car)),
            ("nodest", NavStatus::NoRoadNear(Endpoint::Destination)),
            ("unreachable", NavStatus::Unreachable),
            ("nodata", NavStatus::NoRoadData),
            ("race", NavStatus::PausedRace),
            ("arrived", NavStatus::Arrived),
        ] {
            let mut n = NavView::default();
            n.status = status;
            n.dest = Some(dest(DestSource::Local, RoutePrefs::default()));
            v.push((name, n, true, None));
        }
        let mut ok = NavView::default();
        ok.status = NavStatus::Ok;
        ok.dest = Some(dest(DestSource::Local, RoutePrefs::default()));
        ok.line = Some(line());
        ok.remaining_m = 123_456.0;
        ok.eta_s = 3.0 * 3600.0 + 59.0 * 60.0;
        v.push(("ok", ok.clone(), true, None));
        let mut shared = ok.clone();
        shared.dest = Some(dest(DestSource::Shared { setter: "A Very Long Player Name Indeed".into(), hue: 120.0 }, prefs));
        let room = RoomInfo { name: "A Very Long Player Name Indeed".into(), hue: 120.0, mine: false };
        v.push(("shared", shared.clone(), true, Some(room)));
        let mine = RoomInfo { name: "Me".into(), hue: 30.0, mine: true };
        shared.dest = Some(dest(DestSource::Shared { setter: "Me".into(), hue: 30.0 }, RoutePrefs::default()));
        v.push(("shared_mine", shared, true, Some(mine)));
        v
    }

    /// The four cards stay inside the left pane at its narrowest and widest, in both languages,
    /// in every status and with a shared destination (read-only filters, long names).
    #[test]
    fn the_left_pane_stays_inside_its_panes() {
        for lang in [Language::English, Language::German] {
            with_language(lang, || {
                // The pane is 40% of the tab up to 360 px: ~240 px at the window minimum.
                for pane_w in [240.0, 280.0, 360.0] {
                    let w = pane_w + 16.0; // + the test panel's margins
                    for in_session in [false, true] {
                        for (name, view, _, room) in states() {
                            let mut nav = NavConfig::default();
                            let pin = PaneIn { pending: false, view: &view, in_session, room: room.clone(), use_mph: true };
                            let out = render(&format!("nav_{name}"), w, 900.0, |ui, _| {
                                left_cards(ui, &mut nav, &pin);
                            });
                            assert_eq!(check_panes(&out, w, &format!("{lang:?} {name} session={in_session}")), 4, "{lang:?} {name} at {pane_w} px: four cards");
                        }
                    }
                }
            });
        }
    }

    /// While a teammate's destination is followed the filters and slider show the setter's and
    /// cannot be changed; this player's own settings are untouched.
    #[test]
    fn followed_filters_are_read_only_and_show_the_setters() {
        let setter = RoutePrefs { filters: RouteFilters { road: false, jumps: true, ..RouteFilters::default() }, curves: 0.9 };
        let mut view = NavView::default();
        view.status = NavStatus::Ok;
        view.dest = Some(dest(DestSource::Shared { setter: "Ann".into(), hue: 10.0 }, setter));
        let room = Some(RoomInfo { name: "Ann".into(), hue: 10.0, mine: false });
        let pin = PaneIn { pending: false, view: &view, in_session: true, room, use_mph: false };
        assert_eq!(pin.followed(), Some(setter));
        // Own destination in the room: not followed.
        let mine = PaneIn { room: Some(RoomInfo { name: "Me".into(), hue: 1.0, mine: true }), view: &view, pending: false, in_session: true, use_mph: false };
        assert_eq!(mine.followed(), None);
        // Local: not followed.
        let mut local = NavView::default();
        local.dest = Some(dest(DestSource::Local, RoutePrefs::default()));
        assert_eq!(PaneIn { view: &local, room: None, pending: false, in_session: false, use_mph: false }.followed(), None);

        // Drawing it edits nothing.
        let mut nav = NavConfig::default();
        let before = nav.clone();
        let mut out = PaneOut::default();
        let ctx = crate::ui::test_render::context();
        let _ = crate::ui::test_render::run(&ctx, 400.0, 900.0, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            road_types_card(ui, &mut nav, pin.followed(), &mut out);
            preference_card(ui, &mut nav, pin.followed(), &mut out);
        });
        assert_eq!(nav, before);
        assert_eq!(out, PaneOut::default());
    }

    /// The Clear button reads "Clear for everyone" for a shared destination, and every status has
    /// its own text.
    #[test]
    fn every_status_has_text_and_clear_says_for_everyone_when_shared() {
        let all = [
            NavStatus::Idle,
            NavStatus::WaitingForCar,
            NavStatus::Routing,
            NavStatus::Ok,
            NavStatus::NoRoadNear(Endpoint::Car),
            NavStatus::NoRoadNear(Endpoint::Destination),
            NavStatus::Unreachable,
            NavStatus::NoRoadData,
            NavStatus::PausedRace,
            NavStatus::Arrived,
        ];
        let texts: std::collections::HashSet<&str> = all.iter().map(|s| status_text(*s).1).collect();
        assert_eq!(texts.len(), all.len(), "no two statuses share a text");
        with_language(Language::German, || {
            for s in all {
                let t = status_text(s).1;
                assert!(!t.is_empty());
            }
        });
        let mut view = NavView::default();
        view.dest = Some(dest(DestSource::Shared { setter: "Ann".into(), hue: 10.0 }, RoutePrefs::default()));
        let pin = PaneIn { pending: false, view: &view, in_session: true, room: Some(RoomInfo { name: "Ann".into(), hue: 10.0, mine: false }), use_mph: false };
        assert!(pin.shared());
    }
}
