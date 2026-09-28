//! HUD overlay runtime (Linux/Wayland only, D1/D23): a click-through `wlr-layer-shell`
//! surface on the `overlay` layer, rendered with egui through `egui_glow` on its own
//! in-process thread. That thread owns its own Wayland connection, calloop loop, EGL
//! context, `egui_glow::Painter` and `egui::Context`, so it keeps drawing while the main
//! window is hidden behind the game.
//!
//! Control: [`OverlayCmd`]s over a channel. Data: the listener writes a [`HudSnapshot`]
//! into [`OverlayHandle::slot`] and calls [`Waker::wake`]; each wake draws one frame
//! (D17), paced by the compositor's frame callbacks.

pub mod snapshot;

#[cfg(target_os = "linux")]
mod gl;
#[cfg(target_os = "linux")]
mod render;
#[cfg(target_os = "linux")]
mod wayland;

use std::fmt;

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
}

impl fmt::Display for DisabledReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoWayland => write!(f, "The overlay needs a Wayland session (WAYLAND_DISPLAY is not set)."),
            Self::Wayland(e) => write!(f, "Couldn't connect to the Wayland compositor: {e}"),
            Self::NoLayerShell => write!(
                f,
                "Your compositor doesn't support wlr-layer-shell (e.g. GNOME). The overlay works on Hyprland, Sway and KDE Plasma."
            ),
            Self::Egl(e) => write!(f, "Couldn't set up OpenGL (EGL) for the overlay: {e}"),
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

/// Commands to the overlay thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayCmd {
    /// Create the surface on the target output (no-op if already shown).
    Show,
    /// Destroy the surface (not just hide it: see `wayland.rs`).
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
    /// Draw the dev test pattern (`FORZA_OVERLAY_TEST=1`).
    pub test_pattern: bool,
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use smithay_client_toolkit::reexports::calloop::{channel, ping};

    use super::snapshot::SnapshotSlot;
    use super::{wayland, DisabledReason, OverlayCmd, OverlayOptions};

    /// Wakes the overlay to draw one frame. Cheap, coalescing, `Send + Clone`: hand it to the
    /// listener thread and call it after each snapshot write.
    #[derive(Clone)]
    pub struct Waker(ping::Ping);

    impl Waker {
        pub fn wake(&self) {
            self.0.ping();
        }
    }

    /// Owns the overlay thread. Dropping it shuts the thread down and joins it.
    pub struct OverlayHandle {
        cmds: channel::Sender<OverlayCmd>,
        waker: Waker,
        slot: SnapshotSlot,
        join: Option<JoinHandle<()>>,
    }

    impl OverlayHandle {
        /// Start the overlay thread (hidden until [`OverlayCmd::Show`]). Blocks until the
        /// thread has connected and set up EGL, so the caller learns right away whether the
        /// overlay is usable.
        pub fn spawn(opts: OverlayOptions) -> Result<Self, DisabledReason> {
            let io = |e: std::io::Error| DisabledReason::Wayland(e.to_string());
            let (cmds, cmd_rx) = channel::channel();
            let (ping, ping_rx) = ping::make_ping().map_err(io)?;
            let slot: SnapshotSlot = Arc::new(Mutex::new(None));
            let (ready_tx, ready_rx) = mpsc::channel();
            let thread_slot = slot.clone();
            let join = thread::Builder::new()
                .name("overlay".into())
                .spawn(move || wayland::run(opts, cmd_rx, ping_rx, thread_slot, ready_tx))
                .map_err(io)?;
            // Connect + roundtrip + EGL init is tens of ms; the timeout only guards a hung compositor.
            match ready_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(())) => Ok(Self { cmds, waker: Waker(ping), slot, join: Some(join) }),
                Ok(Err(reason)) => {
                    let _ = join.join();
                    Err(reason)
                }
                // Timed out (thread left detached; it exits once it sees the closed channel)
                // or the thread died during startup.
                Err(e) => Err(DisabledReason::Wayland(format!("overlay thread didn't start: {e}"))),
            }
        }

        pub fn send(&self, cmd: OverlayCmd) {
            let _ = self.cmds.send(cmd); // a dead thread is reported by is_dead()
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
            self.send(OverlayCmd::Shutdown);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    /// Dev switch (phase A): `FORZA_OVERLAY_TEST=1` shows the test pattern on the output
    /// named by `FORZA_OVERLAY_OUTPUT` (default: the first output). Keep the returned handle
    /// alive for as long as the pattern should stay up. I7 replaces this with the real spawn.
    pub fn spawn_dev_test() -> Option<OverlayHandle> {
        if std::env::var("FORZA_OVERLAY_TEST").as_deref() != Ok("1") {
            return None;
        }
        let output = std::env::var("FORZA_OVERLAY_OUTPUT").ok().filter(|s| !s.is_empty());
        match OverlayHandle::spawn(OverlayOptions { output, test_pattern: true }) {
            Ok(handle) => {
                handle.send(OverlayCmd::Show);
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
}
