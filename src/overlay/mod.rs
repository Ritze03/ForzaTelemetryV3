//! HUD overlay runtime (Linux/Wayland only, D1/D23): a click-through `wlr-layer-shell`
//! surface on the `overlay` layer, rendered with egui through `egui_glow` on its own
//! in-process thread. That thread owns its own Wayland connection, calloop loop, EGL
//! context, `egui_glow::Painter` and `egui::Context`, so it keeps drawing while the main
//! window is hidden behind the game.
//!
//! Control: [`OverlayCmd`]s over a channel. Data: the listener writes a [`HudSnapshot`]
//! into [`OverlayHandle::slot`] and calls [`Waker::wake`]; each wake draws one frame
//! (D17), paced by the compositor's frame callbacks.
//!
//! Second surface backend (`x11.rs`, [`Backend`]): an override-redirect X11 window, for
//! GNOME (no layer-shell) through XWayland. By default layer-shell is tried first and X11 is
//! the fallback; `FORZA_OVERLAY_BACKEND` forces one (developer override).

#[allow(dead_code)] // pending: most snapshot fields are read by the HUD renderer (I6)
pub mod snapshot;

#[cfg(target_os = "linux")]
mod gl;
#[cfg(target_os = "linux")]
mod render;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "linux")]
mod x11;

use std::fmt;

use crate::i18n::tr;

#[cfg(target_os = "linux")]
pub use linux::*;

/// Why the overlay can't run. Shown to the user; the app keeps working without it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisabledReason {
    /// `WAYLAND_DISPLAY` unset or empty (X11 session, or no desktop).
    NoWayland,
    /// Connecting to the compositor or setting up the event loop failed.
    Wayland(String),
    /// No `zwlr_layer_shell_v1` global (e.g. GNOME/Mutter).
    NoLayerShell,
    /// EGL display/config/context or the egui_glow painter failed.
    Egl(String),
    /// X11 backend: `DISPLAY` unset or empty.
    NoX11,
    /// X11 backend: connecting failed or a needed extension is missing.
    X11(String),
}

impl fmt::Display for DisabledReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // The fixed text is translated; the `{e}` detail stays as the system reported it.
            Self::NoWayland => f.write_str(tr("The overlay needs a Wayland or X11 session (neither WAYLAND_DISPLAY nor DISPLAY is set).")),
            Self::Wayland(e) => write!(f, "{} {e}", tr("Couldn't connect to the Wayland compositor:")),
            Self::NoLayerShell => f.write_str(tr(
                "Your compositor doesn't support wlr-layer-shell (e.g. GNOME) and the X11 fallback is switched off (FORZA_OVERLAY_BACKEND=wayland). Unset it to try the experimental X11/XWayland fallback.",
            )),
            Self::Egl(e) => write!(f, "{} {e}", tr("Couldn't set up OpenGL (EGL) for the overlay:")),
            Self::NoX11 => f.write_str(tr("The X11 overlay needs an X display (DISPLAY is not set).")),
            Self::X11(e) => write!(f, "{} {e}", tr("Couldn't use the X display for the overlay:")),
        }
    }
}

/// Startup capability check, pure so it's unit-testable. The facts are in probe order
/// (a probe only runs when the ones before it passed) and the first failure wins.
pub fn capability(
    wayland_display: Option<&str>,
    has_layer_shell: bool,
    egl_error: Option<&str>,
) -> Result<(), DisabledReason> {
    if wayland_display.is_none_or(str::is_empty) {
        return Err(DisabledReason::NoWayland);
    }
    if !has_layer_shell {
        return Err(DisabledReason::NoLayerShell);
    }
    match egl_error {
        Some(e) => Err(DisabledReason::Egl(e.to_string())),
        None => Ok(()),
    }
}

/// X11 backend's startup check, same shape as [`capability`]: `DISPLAY`, then the first
/// missing X extension (SHAPE, RANDR 1.5), then EGL.
pub fn capability_x11(
    x_display: Option<&str>,
    missing_extension: Option<&str>,
    egl_error: Option<&str>,
) -> Result<(), DisabledReason> {
    if x_display.is_none_or(str::is_empty) {
        return Err(DisabledReason::NoX11);
    }
    if let Some(ext) = missing_extension {
        return Err(DisabledReason::X11(format!("missing extension {ext}")));
    }
    match egl_error {
        Some(e) => Err(DisabledReason::Egl(e.to_string())),
        None => Ok(()),
    }
}

/// Which error to show when [`Backend::Auto`] failed on both backends. The X11 one, since
/// that's the backend that could have worked there (GNOME, X11 sessions) — except:
/// - layer-shell was *present* but failed later (connect/EGL): that's a layer-shell desktop
///   (Hyprland…), so its own error is the real problem, not the fallback's;
/// - there is neither `WAYLAND_DISPLAY` nor `DISPLAY`: [`DisabledReason::NoWayland`], whose
///   text names both variables (no graphical session at all).
pub fn auto_error(layer_shell: DisabledReason, x11: DisabledReason) -> DisabledReason {
    match (layer_shell, x11) {
        (DisabledReason::NoWayland, DisabledReason::NoX11) => DisabledReason::NoWayland,
        (DisabledReason::NoWayland | DisabledReason::NoLayerShell, x11) => x11,
        (layer_shell, _) => layer_shell,
    }
}

/// Which surface backend `OverlayHandle::spawn` uses (`FORZA_OVERLAY_BACKEND`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// wlr-layer-shell only (`wayland`).
    Wayland,
    /// Override-redirect X11 window (XWayland on GNOME, or an X11 session). Experimental.
    X11,
    /// Layer-shell, falling back to X11 when it's unavailable. The default.
    Auto,
}

impl Backend {
    /// `FORZA_OVERLAY_BACKEND`'s value (a developer override); unset, empty or unknown =
    /// [`Backend::Auto`], so a plain `cargo run` works on Hyprland and GNOME alike. On a
    /// layer-shell desktop Auto's first try succeeds, so it behaves exactly like `wayland`.
    pub fn from_env(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(v) if v.eq_ignore_ascii_case("x11") => Self::X11,
            Some(v) if v.eq_ignore_ascii_case("wayland") => Self::Wayland,
            _ => Self::Auto,
        }
    }
}

/// Commands to the overlay thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayCmd {
    /// Create the surface on the target output (no-op if already shown).
    Show,
    /// Destroy the surface (not just hide it: see `wayland.rs`).
    #[allow(dead_code)] // the test pattern's counterpart to Show; nothing sends it yet
    Hide,
    /// Target output by name (e.g. "DP-1"); `None` = the first output. A shown surface
    /// moves by being recreated on the new output.
    SetOutput(Option<String>),
    Shutdown,
}

/// Spawn-time options.
#[derive(Debug, Clone, Default)]
pub struct OverlayOptions {
    /// Initial target output name; `None` = the first output.
    pub output: Option<String>,
    /// Draw the dev test pattern (`FORZA_OVERLAY_TEST=1` or `2`).
    pub test_pattern: bool,
    /// Co-op state, read each HUD frame for the teammate markers on M2′ (`None` = none).
    pub coop: Option<crate::coop::CoopReader>,
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use smithay_client_toolkit::reexports::calloop::{channel, ping};

    use super::snapshot::SnapshotSlot;
    use super::{wayland, x11, Backend, DisabledReason, OverlayCmd, OverlayOptions};

    /// A backend's thread body (`wayland::run` / `x11::run`): same channels, same contract.
    type ThreadBody = fn(
        OverlayOptions,
        channel::Channel<OverlayCmd>,
        ping::PingSource,
        SnapshotSlot,
        mpsc::Sender<Result<(), DisabledReason>>,
    );

    /// Wakes the overlay to draw one frame. Cheap, coalescing, `Send + Clone`: hand it to the
    /// listener thread and call it after each snapshot write.
    #[derive(Clone)]
    pub struct Waker(ping::Ping);

    impl Waker {
        pub fn wake(&self) {
            self.0.ping();
        }
    }

    /// Cloneable command sender for threads other than the handle's owner (the focus
    /// thread's monitor detection sends `SetOutput`). Never blocks.
    #[derive(Clone)]
    pub struct OverlaySender(channel::Sender<OverlayCmd>);

    impl OverlaySender {
        pub fn send(&self, cmd: OverlayCmd) {
            let _ = self.0.send(cmd); // a dead thread is reported by is_dead()
        }
    }

    /// Owns the overlay thread. Dropping it shuts the thread down and joins it.
    pub struct OverlayHandle {
        cmds: channel::Sender<OverlayCmd>,
        waker: Waker,
        slot: SnapshotSlot,
        join: Option<JoinHandle<()>>,
        /// Set on drop; stops the dev 60 Hz pinger (`FORZA_OVERLAY_TEST=2`).
        dev_stop: Option<Arc<AtomicBool>>,
    }

    impl OverlayHandle {
        /// Start the overlay thread, hidden. The HUD then shows/hides by following
        /// `HudSnapshot::visible`; the test pattern by [`OverlayCmd::Show`]/`Hide`. Blocks until the
        /// thread has connected and set up EGL, so the caller learns right away whether the
        /// overlay is usable. The backend comes from `FORZA_OVERLAY_BACKEND` ([`Backend`]);
        /// unset = auto (layer-shell, then X11). The dev test pattern goes through here too.
        pub fn spawn(opts: OverlayOptions) -> Result<Self, DisabledReason> {
            match Backend::from_env(std::env::var("FORZA_OVERLAY_BACKEND").ok().as_deref()) {
                Backend::Wayland => Self::spawn_on(wayland::run, false, opts),
                Backend::X11 => Self::spawn_on(x11::run, true, opts),
                Backend::Auto => Self::spawn_on(wayland::run, false, opts.clone()).or_else(|reason| {
                    eprintln!("overlay: layer-shell unavailable ({reason}); trying X11");
                    Self::spawn_on(x11::run, true, opts).map_err(|x11| super::auto_error(reason, x11))
                }),
            }
        }

        /// `is_x11` only picks the variant for thread-start errors.
        fn spawn_on(body: ThreadBody, is_x11: bool, opts: OverlayOptions) -> Result<Self, DisabledReason> {
            let fail = move |e: String| if is_x11 { DisabledReason::X11(e) } else { DisabledReason::Wayland(e) };
            let io = |e: std::io::Error| fail(e.to_string());
            let (cmds, cmd_rx) = channel::channel();
            let (ping, ping_rx) = ping::make_ping().map_err(io)?;
            let slot: SnapshotSlot = Arc::new(Mutex::new(None));
            let (ready_tx, ready_rx) = mpsc::channel();
            let thread_slot = slot.clone();
            let join = thread::Builder::new()
                .name("overlay".into())
                .spawn(move || body(opts, cmd_rx, ping_rx, thread_slot, ready_tx))
                .map_err(io)?;
            // Connect + roundtrip + EGL init is tens of ms; the timeout only guards a hung compositor.
            match ready_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(())) => Ok(Self { cmds, waker: Waker(ping), slot, join: Some(join), dev_stop: None }),
                Ok(Err(reason)) => {
                    let _ = join.join();
                    Err(reason)
                }
                // Timed out (thread left detached; it exits once it sees the closed channel)
                // or the thread died during startup.
                Err(e) => Err(fail(format!("overlay thread didn't start: {e}"))),
            }
        }

        pub fn send(&self, cmd: OverlayCmd) {
            let _ = self.cmds.send(cmd); // a dead thread is reported by is_dead()
        }

        pub fn sender(&self) -> OverlaySender {
            OverlaySender(self.cmds.clone())
        }

        pub fn waker(&self) -> Waker {
            self.waker.clone()
        }

        /// Latest-wins snapshot mailbox the overlay draws from.
        pub fn slot(&self) -> SnapshotSlot {
            self.slot.clone()
        }

        /// The thread exited on its own (connection lost or a panic).
        pub fn is_dead(&self) -> bool {
            self.join.as_ref().is_some_and(|j| j.is_finished())
        }
    }

    impl Drop for OverlayHandle {
        fn drop(&mut self) {
            if let Some(stop) = &self.dev_stop {
                stop.store(true, Ordering::Relaxed);
            }
            self.send(OverlayCmd::Shutdown);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    /// `FORZA_OVERLAY_TEST` asks for the dev test pattern ([`spawn_dev_test`]), which then
    /// owns the overlay.
    pub fn dev_test_requested() -> bool {
        matches!(std::env::var("FORZA_OVERLAY_TEST").as_deref(), Ok("1" | "2"))
    }

    /// Dev switch (phase A):`FORZA_OVERLAY_TEST=1` shows the static test pattern on the
    /// output named by `FORZA_OVERLAY_OUTPUT` (default: the first output). `=2` also wakes
    /// the overlay at ~60 Hz, like packets would (D17), so the frame counter in the pattern
    /// runs and game frametimes can be checked under a redrawing overlay. Keep the returned
    /// handle alive for as long as the pattern should stay up. While it is set the app
    /// doesn't start the real HUD overlay ([`dev_test_requested`]), so two never fight.
    pub fn spawn_dev_test() -> Option<OverlayHandle> {
        let live = match std::env::var("FORZA_OVERLAY_TEST").as_deref() {
            Ok("1") => false,
            Ok("2") => true,
            _ => return None,
        };
        let output = std::env::var("FORZA_OVERLAY_OUTPUT").ok().filter(|s| !s.is_empty());
        match OverlayHandle::spawn(OverlayOptions { output, test_pattern: true, coop: None }) {
            Ok(mut handle) => {
                handle.send(OverlayCmd::Show);
                if live {
                    let stop = Arc::new(AtomicBool::new(false));
                    let (waker, thread_stop) = (handle.waker(), stop.clone());
                    // Detached: it exits within one tick of the handle dropping, and a
                    // process exit ends it anyway, so joining could only delay app close.
                    let spawned = thread::Builder::new().name("overlay-dev-ping".into()).spawn(move || {
                        while !thread_stop.load(Ordering::Relaxed) {
                            waker.wake();
                            thread::sleep(Duration::from_micros(16_667));
                        }
                    });
                    match spawned {
                        Ok(_) => handle.dev_stop = Some(stop),
                        Err(e) => eprintln!("overlay: dev pinger didn't start: {e}"),
                    }
                }
                Some(handle)
            }
            Err(reason) => {
                eprintln!("overlay disabled: {reason}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_reports_first_missing_piece() {
        // X11 session / no desktop: unset or empty WAYLAND_DISPLAY wins over everything.
        assert_eq!(capability(None, true, None), Err(DisabledReason::NoWayland));
        assert_eq!(capability(Some(""), true, None), Err(DisabledReason::NoWayland));
        assert_eq!(capability(None, false, Some("x")), Err(DisabledReason::NoWayland));
        // GNOME: Wayland but no layer-shell.
        assert_eq!(capability(Some("wayland-1"), false, None), Err(DisabledReason::NoLayerShell));
        assert_eq!(capability(Some("wayland-1"), false, Some("x")), Err(DisabledReason::NoLayerShell));
        // EGL failure keeps its message.
        assert_eq!(
            capability(Some("wayland-1"), true, Some("no 8-bit RGBA EGL config")),
            Err(DisabledReason::Egl("no 8-bit RGBA EGL config".into()))
        );
        assert_eq!(capability(Some("wayland-1"), true, None), Ok(()));
    }

    #[test]
    fn capability_x11_reports_first_missing_piece() {
        assert_eq!(capability_x11(None, None, None), Err(DisabledReason::NoX11));
        assert_eq!(capability_x11(Some(""), Some("SHAPE"), Some("x")), Err(DisabledReason::NoX11));
        assert_eq!(
            capability_x11(Some(":0"), Some("SHAPE"), Some("x")),
            Err(DisabledReason::X11("missing extension SHAPE".into()))
        );
        assert_eq!(capability_x11(Some(":0"), None, Some("no config")), Err(DisabledReason::Egl("no config".into())));
        assert_eq!(capability_x11(Some(":0"), None, None), Ok(()));
    }

    #[test]
    fn backend_defaults_to_auto() {
        // Unset / empty / unknown = auto; the env var only forces one backend.
        assert_eq!(Backend::from_env(None), Backend::Auto);
        assert_eq!(Backend::from_env(Some("")), Backend::Auto);
        assert_eq!(Backend::from_env(Some("xorg")), Backend::Auto);
        assert_eq!(Backend::from_env(Some("AUTO")), Backend::Auto);
        assert_eq!(Backend::from_env(Some("wayland")), Backend::Wayland);
        assert_eq!(Backend::from_env(Some("Wayland")), Backend::Wayland);
        assert_eq!(Backend::from_env(Some("x11")), Backend::X11);
        assert_eq!(Backend::from_env(Some(" X11 ")), Backend::X11);
        assert_eq!(Backend::from_env(Some("auto")), Backend::Auto);
    }

    #[test]
    fn auto_error_prefers_the_backend_that_could_work() {
        use DisabledReason::*;
        // GNOME: no layer-shell, X11 failed -> the X11 reason.
        assert_eq!(auto_error(NoLayerShell, X11("e".into())), X11("e".into()));
        assert_eq!(auto_error(NoWayland, Egl("x".into())), Egl("x".into()));
        // No session at all.
        assert_eq!(auto_error(NoWayland, NoX11), NoWayland);
        // Layer-shell desktop whose own setup failed -> its reason.
        assert_eq!(auto_error(Egl("w".into()), NoX11), Egl("w".into()));
        assert_eq!(auto_error(Wayland("c".into()), X11("e".into())), Wayland("c".into()));
    }
}
