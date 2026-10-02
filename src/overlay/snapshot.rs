//! What the overlay thread draws from. The listener thread (`listeners/worker.rs`, via
//! `listeners/hud.rs`) builds a [`HudSnapshot`] per packet, overwrites it in a
//! [`SnapshotSlot`] (latest wins) and calls [`HudSink::wake`].
//!
//! Platform-neutral on purpose: the listener compiles on Windows too, so nothing here may
//! name the Linux-only overlay types. `app.rs` wraps `OverlayHandle::{slot, waker}` into a
//! [`HudSink`] and hands it to `ListenerHandle::set_hud_sink`.
//!
//! **Clock.** Every time in here is seconds on [`hud_clock`], one process-wide monotonic
//! clock (an `Instant` epoch taken on first use). The renderer compares event times with
//! `hud_clock()` for "now"; tests and the PNG harness can pin `now` to any value.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::config::{GearboxMode, OverlayConfig};
use crate::packet::ForzaPacket;

/// Seconds since the first call in this process. Monotonic; shared by the listener (event
/// stamps) and the overlay (animation "now").
pub fn hud_clock() -> f64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Which block fills the race/drift slot (D25/D27), detected from how `current_lap` behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HudMode {
    /// `current_lap` is a lap timer → race block (R1′).
    #[default]
    Race,
    /// `current_lap` is a drift score → drift counter (X1′).
    Drift,
}

/// A race-position change (D15 backdrop, 3 s on the HUD side).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaceChange {
    /// When it happened, [`hud_clock`] seconds.
    pub at: f64,
    /// True = moved up (green), false = dropped back (red).
    pub gained: bool,
}

/// Latest time each animated event happened, [`hud_clock`] seconds. `None` = not seen since
/// the overlay was enabled. The HUD decides how long each one animates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HudEvents {
    /// Last race-position change (both old and new position non-zero).
    pub place_change: Option<PlaceChange>,
    /// Last lap completion (`lap_number` stepped up by one). The finished lap's time is
    /// `pkt.last_lap`; D21 holds it for 4 s.
    pub lap_completed_at: Option<f64>,
    /// Last gear change (gear-change pulse).
    pub gear_changed_at: Option<f64>,
    /// Start of the current pause (`listeners::hud::hud_paused`: race off, max rpm 0
    /// (menus), or 0/0/0 orientation (loading screens));
    /// `None` while running.
    pub paused_since: Option<f64>,
}

/// One finished drift gain window: the "+N" chip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriftChip {
    /// Score gained over the window. Never negative (a score reset restarts the window);
    /// may be 0 — the HUD decides whether a "+0" chip shows.
    pub gain: f32,
    /// When the window closed, [`hud_clock`] seconds.
    pub at: f64,
}

/// Drift counter data (D10/D27). Meaningful while [`HudSnapshot::mode`] is `Drift`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DriftInfo {
    /// Live drift score (`pkt.current_lap` read as a score).
    pub score: f32,
    /// Event high score (`pkt.best_lap` read as a score).
    pub best: f32,
    /// Start of the running gain window, [`hud_clock`] seconds; `None` while not drifting
    /// or paused. Progress bar = `(now − window_start) / interval`.
    pub window_start: Option<f64>,
    /// Window length, seconds (`OverlayConfig::drift_chip_secs`).
    pub interval: f32,
    /// The most recently closed window.
    pub chip: Option<DriftChip>,
    /// [`hud_clock`] time of the last score increase; `None` before the first. X1′'s live
    /// dot is amber while this is recent (`hud::drift::DRIFT_ACTIVE_SECS`).
    pub last_rise_at: Option<f64>,
}

/// The auto gearbox's drive mode, for the gear letter. Mirrors [`GearboxMode`], which isn't
/// `Debug` (and lives in the shared config).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveMode {
    Street,
    Sport,
    Race,
}

impl From<GearboxMode> for DriveMode {
    fn from(m: GearboxMode) -> Self {
        match m {
            // Manual is never shown (the HUD treats it as the gearbox off); see `dsg_resolved_mode`.
            GearboxMode::Street | GearboxMode::Manual => Self::Street,
            GearboxMode::Sport => Self::Sport,
            GearboxMode::Race => Self::Race,
        }
    }
}

/// World → map-image transform from the app config (the Dashboard map's calibration):
/// `px = (x − origin_x) · px_per_m`, `py = (origin_z − z) · px_per_m`. The season image is
/// time-based (`crate::minimap::current_season`), so the overlay picks it itself.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MinimapCalib {
    pub px_per_m: f32,
    pub origin_x: f32,
    pub origin_z: f32,
}

/// How a notification looks (the dot colour). Not a severity: it says what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifKind {
    /// Something switched on (green dot).
    On,
    /// Something switched off (red dot).
    Off,
    /// A change or a status (blue dot).
    Info,
    /// A nudge to do something (yellow dot): "Shift at redline".
    Hint,
}

/// One short on-HUD message (D26). Created on the listener thread, which sees every source
/// (hotkeys, config pushed from the UI, the gearbox/calibration state); the HUD only draws
/// it while `hud_clock() − created < hud::notify::TTL_SECS`.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    /// Unique per process, rising; the HUD could key animation state on it.
    pub id: u64,
    pub text: String,
    pub kind: NotifKind,
    /// [`hud_clock`] seconds.
    pub created: f64,
}

/// Everything the HUD draws from. Cheap to clone (the packet is ~200 B, the config an Arc).
#[derive(Debug, Clone, Default)]
pub struct HudSnapshot {
    /// The latest packet (kept after packets stop, for the fade-out). Note `lap_number` is
    /// 0-based: the HUD displays `lap_number + 1` (D21).
    pub pkt: ForzaPacket,
    /// [`hud_clock`] when this snapshot was built.
    pub built_at: f64,
    /// **Target** visibility: overlay enabled, Hide HUD off, not paused for ≥ 300 ms,
    /// a packet within 2 s, and — with `focus_only` — the game focused. The overlay
    /// shows/hides (with its fade) to follow it.
    pub visible: bool,
    /// A packet arrived within the last 2 s.
    pub connected: bool,
    /// The latest packet is paused (`listeners::hud::hud_paused`: race off, max rpm 0
    /// (menus), or 0/0/0 orientation (loading screens); raw, no 300 ms delay).
    pub paused: bool,
    /// Hide HUD hotkey state (raw; already folded into `visible`).
    pub hud_hidden: bool,
    /// `redline_frac ×` the gearbox's calibrated max rpm (`engine_max_rpm` before one
    /// exists). See `listeners::hud::cue_rpms`.
    pub redline_rpm: f32,
    /// The gearbox's full-throttle upshift rpm (`dsg_shift_rpm_pct ×` calibrated max rpm);
    /// `shift_frac × engine_max_rpm` before a calibration exists.
    pub shift_rpm: f32,
    /// Race block or drift counter.
    pub mode: HudMode,
    /// Current lap time minus the best lap's time at the same distance into the lap
    /// (negative = ahead). `None` without a best lap, past the best lap's distance, or in
    /// drift mode.
    pub lap_delta: Option<f32>,
    pub drift: DriftInfo,
    pub events: HudEvents,
    /// The overlay settings (styles, cells, scale, module toggles, …).
    pub cfg: Arc<OverlayConfig>,
    /// `AppConfig::use_mph` — speed unit for the cluster.
    pub use_mph: bool,
    /// `Some(mode)` while the auto gearbox (DSG) is switched on: the cluster prefixes the
    /// forward gear with D/S/R. The mode is the one in effect (Race when auto-switched in a
    /// race).
    pub auto_gear: Option<DriveMode>,
    pub minimap: MinimapCalib,
    /// `AppConfig::coop_hue`: the own identity colour (hue, degrees) the Minimap draws the own
    /// arrow and trail in while in a co-op session, as the Dashboard map does.
    pub coop_hue: f32,
    /// Right-stick vector (x right, y up, post-deadzone) for the Minimap's look-around; set
    /// by [`HudSink::publish`], `(0, 0)` without a pad. Only used with `map_look_stick`.
    pub look_stick: (f32, f32),
    /// Recent notifications, oldest first (bounded; the HUD ignores expired ones).
    pub notifications: Vec<Notification>,
}

/// Latest-wins mailbox. Writer `lock`s and overwrites; the overlay `try_lock`s and clones,
/// keeping its previous copy on a miss (same pattern as the listener ↔ UI mailboxes).
pub type SnapshotSlot = Arc<Mutex<Option<HudSnapshot>>>;

/// Where the listener publishes: the overlay's slot plus a wake callback. Built by `app.rs`
/// from `OverlayHandle::{slot, waker}` (e.g. `HudSink::new(h.slot(), move || w.wake())`)
/// so the listener never names the Linux-only `Waker`.
pub struct HudSink {
    pub slot: SnapshotSlot,
    /// Wakes the overlay to draw one frame. Must be cheap and non-blocking.
    pub wake: Box<dyn Fn() + Send>,
    /// Source of `HudSnapshot::look_stick` (the listener thread has no gamepad of its own).
    pub stick: Option<crate::gamepad::Gamepad>,
}

impl HudSink {
    pub fn new(slot: SnapshotSlot, wake: impl Fn() + Send + 'static) -> Self {
        Self { slot, wake: Box::new(wake), stick: None }
    }

    /// Stamp every published snapshot with this pad's right stick.
    pub fn with_stick(mut self, pad: crate::gamepad::Gamepad) -> Self {
        self.stick = Some(pad);
        self
    }

    /// Overwrite the slot (latest wins), then wake the overlay. The lock is held only for
    /// the move.
    pub fn publish(&self, mut snap: HudSnapshot) {
        snap.look_stick = self.stick.as_ref().map_or((0.0, 0.0), |g| g.right_stick());
        if let Ok(mut s) = self.slot.lock() {
            *s = Some(snap);
        }
        (self.wake)();
    }
}
