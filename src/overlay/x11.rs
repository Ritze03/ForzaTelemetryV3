//! Opt-in X11 backend (`FORZA_OVERLAY_BACKEND=x11|auto`), for GNOME, which has no
//! wlr-layer-shell: an **override-redirect** window with a 32-bit ARGB visual, click-through
//! via an empty XShape input region, sized to one XRandR monitor. It runs through XWayland on
//! GNOME Wayland (Mutter always stacks override-redirect windows above normal and fullscreen
//! ones) and natively in X11 sessions.
//!
//! Same external contract as `wayland.rs`: same commands, snapshot/visibility/fade rules and
//! pacing, and the window exists only while shown. X11 has no frame callbacks, so each ping
//! (packet) draws one frame and the animation timer covers fades without packets.

use std::os::fd::AsFd;
use std::ptr::NonNull;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use raw_window_handle::{RawDisplayHandle, RawWindowHandle, XcbDisplayHandle, XcbWindowHandle};
use smithay_client_toolkit::reexports::calloop::channel::{Channel, Event as ChanEvent};
use smithay_client_toolkit::reexports::calloop::generic::Generic;
use smithay_client_toolkit::reexports::calloop::ping::PingSource;
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ClipOrdering, ColormapAlloc, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, VisualClass,
    WindowClass,
};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;

use super::gl::{Gl, WinSurface};
use super::render::Renderer;
use super::snapshot::{HudSnapshot, SnapshotSlot};
use super::{capability_x11, DisabledReason, OverlayCmd, OverlayOptions};

/// `WM_NAME`, so `xwininfo -tree` shows which window is ours.
const NAME: &[u8] = b"forza-telemetry-hud";
// Pacing constants and `next_wake` mirror `wayland.rs` (see there for the why).
// ponytail: hoist `next_wake` into mod.rs and share it once wayland.rs is next edited.
const PING_STALE: Duration = Duration::from_millis(40);
const ANIM_FRAME: Duration = Duration::from_millis(16);
const MAX_EGL_FAILURES: u8 = 3;

fn next_wake(animating: bool, since_ping: Option<Duration>) -> Option<Duration> {
    if !animating {
        return None;
    }
    match since_ping {
        Some(t) if t < PING_STALE => Some(PING_STALE - t),
        _ => Some(ANIM_FRAME),
    }
}

/// One XRandR monitor (RandR 1.5 `GetMonitors`), in root coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Monitor {
    name: String,
    primary: bool,
    geo: [i32; 4], // x, y, w, h
}

/// The target by name, else the primary, else the first. XWayland on older Mutter names its
/// outputs `XWAYLAND0…`, which never match the connector names monitor detection reports.
fn pick<'a>(monitors: &'a [Monitor], target: Option<&str>) -> Option<&'a Monitor> {
    target
        .and_then(|t| monitors.iter().find(|m| m.name == t))
        .or_else(|| monitors.iter().find(|m| m.primary))
        .or_else(|| monitors.first())
}

/// The mapped window. Field order is drop order: the EGL surface before the window.
struct Live {
    egl: Option<WinSurface>,
    window: u32,
    monitor: Monitor,
}

struct Overlay {
    conn: Arc<XCBConnection>,
    root: u32,
    visual: u32,
    colormap: u32,
    monitors: Vec<Monitor>,
    handle: LoopHandle<'static, Overlay>,
    // Drop order as in wayland.rs: `live` is already gone (Drop), then the painter, then `gl`
    // (eglTerminate), whose `_native` Arc keeps the xcb connection alive past it.
    live: Option<Live>,
    renderer: Renderer,
    gl: Option<Gl>,
    slot: SnapshotSlot,
    latest: Option<HudSnapshot>,
    visible: bool,
    target: Option<String>,
    /// Last monitor logged as "showing on", so a show/hide cycle doesn't repeat the line.
    logged: Option<String>,
    test_pattern: bool,
    dirty: bool,
    last_ping: Option<Instant>,
    anim_timer: Option<RegistrationToken>,
    egl_failures: u8,
    exit: bool,
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.destroy_window();
        let _ = self.conn.free_colormap(self.colormap);
        let _ = self.conn.flush();
    }
}

pub(super) fn run(
    opts: OverlayOptions,
    cmds: Channel<OverlayCmd>,
    ping: PingSource,
    slot: SnapshotSlot,
    ready: mpsc::Sender<Result<(), DisabledReason>>,
) {
    let (mut event_loop, mut state) = match init(opts, cmds, ping, slot) {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    while !state.exit {
        if let Err(e) = event_loop.dispatch(None, &mut state) {
            eprintln!("overlay[x11]: event loop stopped: {e}");
            break;
        }
        // xcb may have queued events while waiting for a reply; the fd won't signal those.
        state.pump();
    }
    drop(state);
    drop(event_loop);
}

fn x11_err(e: impl std::fmt::Display) -> DisabledReason {
    DisabledReason::X11(e.to_string())
}

/// Depth-32 TrueColor visuals of the screen: the ones a transparent window can use.
fn argb_visuals(screen: &x11rb::protocol::xproto::Screen) -> Vec<u32> {
    let depths = screen.allowed_depths.iter().filter(|d| d.depth == 32);
    depths.flat_map(|d| &d.visuals).filter(|v| v.class == VisualClass::TRUE_COLOR).map(|v| v.visual_id).collect()
}

#[allow(clippy::type_complexity)]
fn init(
    opts: OverlayOptions,
    cmds: Channel<OverlayCmd>,
    ping: PingSource,
    slot: SnapshotSlot,
) -> Result<(EventLoop<'static, Overlay>, Overlay), DisabledReason> {
    let display = std::env::var("DISPLAY").ok();
    let conn = if display.as_deref().is_some_and(|d| !d.is_empty()) {
        Some(XCBConnection::connect(None).map(|(c, screen)| (Arc::new(c), screen)).map_err(x11_err)?)
    } else {
        None
    };
    // Probes in order, like wayland.rs: display, extensions, then EGL; capability_x11 names
    // the first failure.
    let missing = conn.as_ref().map(|(c, _)| missing_extension(c)).unwrap_or(Some("-"));
    let gpu = match (&conn, missing) {
        (Some((c, screen)), None) => egl(c, *screen).and_then(|gl| Ok((Renderer::new(gl.glow.clone(), opts.coop.clone())?, gl))),
        _ => Err(String::new()),
    };
    capability_x11(display.as_deref(), missing, gpu.as_ref().err().map(String::as_str))?;
    let (Some((conn, screen)), Ok(gpu)) = (conn, gpu) else {
        return Err(x11_err("startup probe mismatch")); // unreachable
    };

    let root = conn.setup().roots.get(screen).map(|s| s.root).ok_or_else(|| x11_err("no X screen"))?;
    let visual = gpu.1.native_visual();
    let colormap = conn.generate_id().map_err(x11_err)?;
    conn.create_colormap(ColormapAlloc::NONE, colormap, root, visual).map_err(x11_err)?;
    // ScreenChange covers resolution/layout changes, the Notify ones hotplug.
    let mask = randr::NotifyMask::SCREEN_CHANGE | randr::NotifyMask::OUTPUT_CHANGE | randr::NotifyMask::CRTC_CHANGE;
    conn.randr_select_input(root, mask).map_err(x11_err)?;

    let event_loop = EventLoop::<Overlay>::try_new().map_err(x11_err)?;
    let handle = event_loop.handle();
    let mut state = Overlay {
        conn: conn.clone(),
        root,
        visual,
        colormap,
        monitors: Vec::new(),
        handle: handle.clone(),
        live: None,
        renderer: gpu.0,
        gl: Some(gpu.1),
        slot,
        latest: None,
        visible: false,
        target: opts.output,
        logged: None,
        test_pattern: opts.test_pattern,
        dirty: false,
        last_ping: None,
        anim_timer: None,
        egl_failures: 0,
        exit: false,
    };
    state.refresh_monitors();

    // The loop gets its own dup of the connection fd; readiness only triggers `pump`.
    let fd = conn.as_fd().try_clone_to_owned().map_err(x11_err)?;
    handle
        .insert_source(Generic::new(fd, Interest::READ, Mode::Level), |_, _, s: &mut Overlay| {
            s.pump();
            Ok(PostAction::Continue)
        })
        .map_err(|e| x11_err(e.error))?;
    handle
        .insert_source(cmds, |ev, _, s: &mut Overlay| match ev {
            ChanEvent::Msg(cmd) => s.command(cmd),
            ChanEvent::Closed => s.exit = true,
        })
        .map_err(|e| x11_err(e.error))?;
    handle
        .insert_source(ping, |_, _, s: &mut Overlay| {
            s.last_ping = Some(Instant::now());
            if let Some(t) = s.anim_timer.take() {
                s.handle.remove(t);
            }
            s.dirty = true;
            s.follow_snapshot();
            s.maybe_render();
        })
        .map_err(|e| x11_err(e.error))?;
    conn.flush().map_err(x11_err)?;
    Ok((event_loop, state))
}

/// The first extension the backend needs but the server lacks (`None` = all there).
fn missing_extension(conn: &XCBConnection) -> Option<&'static str> {
    let has = |name| conn.extension_information(name).ok().flatten().is_some();
    if !has(shape::X11_EXTENSION_NAME) {
        return Some("SHAPE");
    }
    let randr_ok = has(randr::X11_EXTENSION_NAME)
        && conn
            .randr_query_version(1, 5)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|v| (v.major_version, v.minor_version) >= (1, 5));
    if !randr_ok {
        return Some("RANDR 1.5");
    }
    None
}

/// EGL on the xcb connection (`EGL_EXT_platform_xcb`, Mesa ≥ 21) with a config whose native
/// visual is 32-bit ARGB, so the window's alpha reaches the compositor.
// ponytail: xcb platform only. NVIDIA drivers before ~560 only do EGL_KHR_platform_x11 (Xlib);
// add an Xlib display (x11-dl is already in the tree) if a tester hits that.
fn egl(conn: &Arc<XCBConnection>, screen: usize) -> Result<Gl, String> {
    let setup_screen = conn.setup().roots.get(screen).ok_or("no X screen")?;
    let argb = argb_visuals(setup_screen);
    if argb.is_empty() {
        return Err("the X server has no 32-bit ARGB visual".into());
    }
    let ptr = NonNull::new(conn.get_raw_xcb_connection()).ok_or("no xcb connection pointer")?;
    let raw = RawDisplayHandle::Xcb(XcbDisplayHandle::new(Some(ptr), screen as i32));
    // SAFETY: `ptr` is `conn`'s live xcb connection and `Gl` keeps an Arc of it.
    unsafe { Gl::with_native(raw, &|c| argb.contains(&c.native_visual()), Box::new(conn.clone())) }
        .map_err(|e| format!("{e} (X11, 32-bit ARGB visual)"))
}

impl Overlay {
    fn command(&mut self, cmd: OverlayCmd) {
        match cmd {
            OverlayCmd::Show => {
                self.visible = true;
                self.ensure_window();
            }
            OverlayCmd::Hide => {
                self.visible = false;
                self.destroy_window();
            }
            OverlayCmd::SetOutput(name) => {
                self.target = name;
                self.follow_monitor();
            }
            OverlayCmd::Shutdown => self.exit = true,
        }
    }

    /// Recreate a shown window when its monitor is no longer the one to use (target changed,
    /// or the layout changed under it). Recreating, not moving: the EGL surface must be
    /// resized anyway, and it mirrors the Wayland backend.
    fn follow_monitor(&mut self) {
        let want = pick(&self.monitors, self.target.as_deref()).cloned();
        if self.live.as_ref().is_some_and(|l| Some(&l.monitor) != want.as_ref()) {
            self.destroy_window();
        }
        self.ensure_window();
    }

    fn refresh_monitors(&mut self) {
        let reply = self.conn.randr_get_monitors(self.root, true).ok().and_then(|c| c.reply().ok());
        let Some(reply) = reply else {
            eprintln!("overlay[x11]: RandR GetMonitors failed; keeping the old monitor list");
            return;
        };
        let name = |atom| {
            let reply = self.conn.get_atom_name(atom).ok().and_then(|c| c.reply().ok());
            reply.map_or_else(|| format!("atom{atom}"), |r| String::from_utf8_lossy(&r.name).into_owned())
        };
        let monitors: Vec<Monitor> = reply
            .monitors
            .iter()
            .map(|m| Monitor {
                name: name(m.name),
                primary: m.primary,
                geo: [m.x.into(), m.y.into(), m.width.into(), m.height.into()],
            })
            .collect();
        if monitors != self.monitors {
            let list: Vec<String> = monitors
                .iter()
                .map(|m| {
                    let [x, y, w, h] = m.geo;
                    format!("{} {w}x{h}+{x}+{y}{}", m.name, if m.primary { " (primary)" } else { "" })
                })
                .collect();
            eprintln!("overlay[x11]: outputs: {}", list.join(", "));
            self.monitors = monitors;
        }
    }

    /// Create and map the window if shown, missing, and a monitor exists.
    fn ensure_window(&mut self) {
        if !self.visible || self.live.is_some() {
            return;
        }
        let Some(monitor) = pick(&self.monitors, self.target.as_deref()).cloned() else {
            eprintln!("overlay[x11]: no RandR monitor to show on");
            return;
        };
        if self.logged.as_deref() != Some(&monitor.name) {
            let wanted = self.target.as_deref().filter(|t| *t != monitor.name);
            let why = wanted.map_or(String::new(), |t| format!(" (no output named \"{t}\")"));
            eprintln!("overlay[x11]: showing on {}{why}", monitor.name);
            self.logged = Some(monitor.name.clone());
        }
        match self.create_window(&monitor) {
            Ok(window) => {
                self.live = Some(Live { egl: None, window, monitor });
                self.attach_egl();
            }
            Err(e) => eprintln!("overlay[x11]: creating the window failed: {e}"),
        }
    }

    fn create_window(&self, m: &Monitor) -> Result<u32, String> {
        let c = &*self.conn;
        let e = |e: &dyn std::fmt::Display| e.to_string();
        let [x, y, w, h] = m.geo;
        let (x, y, w, h) = (x as i16, y as i16, w as u16, h as u16); // RandR's own types
        let window = c.generate_id().map_err(|x| e(&x))?;
        // A depth-32 visual differs from the root's, so border_pixel and colormap are
        // mandatory (else BadMatch). Background None: we paint every pixel ourselves.
        let aux = CreateWindowAux::new()
            .background_pixmap(x11rb::NONE)
            .border_pixel(0)
            .override_redirect(1)
            .colormap(self.colormap)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY);
        c.create_window(32, window, self.root, x, y, w, h, 0, WindowClass::INPUT_OUTPUT, self.visual, &aux)
            .map_err(|x| e(&x))?;
        // Click-through: an empty *input* shape. The bounding shape stays full, so it still draws.
        c.shape_rectangles(shape::SO::SET, shape::SK::INPUT, ClipOrdering::UNSORTED, window, 0, 0, &[])
            .map_err(|x| e(&x))?;
        c.change_property8(PropMode::REPLACE, window, AtomEnum::WM_NAME, AtomEnum::STRING, NAME).map_err(|x| e(&x))?;
        // Override-redirect: no WM decoration, placement or focus. Mapping raises it to the top.
        // ponytail: in a native X11 session a game raised later could cover it (XWayland on
        // Mutter keeps OR windows in its top layer); re-raise on VisibilityNotify if that bites.
        c.map_window(window).map_err(|x| e(&x))?;
        c.flush().map_err(|x| e(&x))?;
        Ok(window)
    }

    /// EGL surface on the live window, then draw. Scale is 1.0.
    // ponytail: no fractional/integer scaling (all target monitors are 1080p @1, and XWayland
    // on Mutter without xwayland-native-scaling reports logical = physical px at scale 1).
    fn attach_egl(&mut self) {
        let (Some(live), Some(gl)) = (self.live.as_mut(), &self.gl) else { return };
        if live.egl.is_some() {
            return;
        }
        let Some(id) = std::num::NonZeroU32::new(live.window) else { return };
        let [_, _, w, h] = live.monitor.geo;
        // SAFETY: the window outlives the surface (destroy_window drops `egl` first).
        match unsafe { gl.create_window_surface(RawWindowHandle::Xcb(XcbWindowHandle::new(id)), w as u32, h as u32) } {
            Ok(egl) => live.egl = Some(egl),
            Err(e) => return self.egl_failed(&e),
        }
        self.dirty = true;
        self.maybe_render();
    }

    /// Hide = destroy, as on Wayland: a hidden HUD has no window. Context and painter stay.
    fn destroy_window(&mut self) {
        if let Some(Live { egl, window, .. }) = self.live.take() {
            if let Some(gl) = &self.gl {
                gl.release();
            }
            drop(egl);
            let _ = self.conn.destroy_window(window);
            let _ = self.conn.flush();
        }
    }

    /// Drain queued X events.
    fn pump(&mut self) {
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(ev)) => self.x_event(ev),
                Ok(None) => return,
                Err(e) => {
                    eprintln!("overlay[x11]: X connection lost: {e}");
                    self.exit = true;
                    return;
                }
            }
        }
    }

    fn x_event(&mut self, ev: Event) {
        match ev {
            // Frames drawn before XWayland/the compositor took the window may be lost; a
            // static test pattern would then stay blank, so redraw on map and expose.
            Event::Expose(e) if e.count == 0 => self.redraw_if_live(e.window),
            Event::MapNotify(e) => self.redraw_if_live(e.window),
            Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_) => {
                self.refresh_monitors();
                self.follow_monitor();
            }
            Event::Error(e) => eprintln!("overlay[x11]: X error: {e:?}"),
            _ => {}
        }
    }

    fn redraw_if_live(&mut self, window: u32) {
        if self.live.as_ref().is_some_and(|l| l.window == window) {
            self.dirty = true;
            self.maybe_render();
        }
    }

    fn refresh_snapshot(&mut self) {
        if let Ok(slot) = self.slot.try_lock() {
            self.latest.clone_from(&slot);
        }
    }

    fn target_visible(&self) -> bool {
        self.latest.as_ref().is_some_and(|s| s.visible)
    }

    /// HUD mode, on each wake: as `wayland.rs` (visible → create; hidden → draw the fade-out,
    /// `maybe_render` destroys the window once it ends). The test pattern follows Show/Hide.
    fn follow_snapshot(&mut self) {
        if self.test_pattern {
            return;
        }
        self.refresh_snapshot();
        if self.target_visible() {
            self.visible = true;
            self.ensure_window();
        } else if self.live.is_none() {
            self.visible = false;
        }
    }

    fn maybe_render(&mut self) {
        if self.exit || !self.dirty {
            return;
        }
        self.refresh_snapshot();
        let (Some(live), Some(gl)) = (&self.live, &self.gl) else { return };
        let Some(egl) = &live.egl else { return };
        let [_, _, w, h] = live.monitor.geo;
        let result = gl
            .make_current(egl)
            .map(|()| self.renderer.frame([w as u32, h as u32], self.latest.as_ref(), self.test_pattern));
        match result.and_then(|animating| gl.swap(egl).map(|()| animating)) {
            Ok(animating) => {
                self.dirty = false;
                self.egl_failures = 0;
                if !self.test_pattern && !animating && !self.target_visible() {
                    self.visible = false;
                    self.arm_anim_timer(None);
                    self.destroy_window();
                    return;
                }
                let wake = next_wake(animating, self.last_ping.map(|t| t.elapsed()));
                self.arm_anim_timer(wake);
            }
            Err(e) => self.egl_failed(&e),
        }
    }

    fn egl_failed(&mut self, e: &dyn std::fmt::Display) {
        self.egl_failures += 1;
        if self.egl_failures >= MAX_EGL_FAILURES {
            eprintln!("overlay[x11]: {e}; {MAX_EGL_FAILURES} EGL failures in a row, shutting the overlay down");
            self.exit = true;
            return;
        }
        eprintln!("overlay[x11]: {e}; recreating the window");
        self.destroy_window();
        self.ensure_window();
    }

    fn arm_anim_timer(&mut self, after: Option<Duration>) {
        if let Some(t) = self.anim_timer.take() {
            self.handle.remove(t);
        }
        let Some(after) = after else { return };
        match self.handle.insert_source(Timer::from_duration(after), |_, _, s: &mut Overlay| {
            s.anim_timer = None;
            s.dirty = true;
            s.maybe_render();
            TimeoutAction::Drop
        }) {
            Ok(token) => self.anim_timer = Some(token),
            Err(e) => eprintln!("overlay[x11]: animation timer: {}", e.error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(name: &str, primary: bool) -> Monitor {
        Monitor { name: name.into(), primary, geo: [0, 0, 1920, 1080] }
    }

    #[test]
    fn pick_prefers_name_then_primary_then_first() {
        let ms = [mon("DP-3", false), mon("DP-2", true)];
        assert_eq!(pick(&ms, Some("DP-3")).map(|m| &*m.name), Some("DP-3"));
        assert_eq!(pick(&ms, Some("DP-1")).map(|m| &*m.name), Some("DP-2")); // unknown → primary
        assert_eq!(pick(&ms, None).map(|m| &*m.name), Some("DP-2"));
        let xwl = [mon("XWAYLAND0", false), mon("XWAYLAND1", false)];
        assert_eq!(pick(&xwl, Some("DP-1")).map(|m| &*m.name), Some("XWAYLAND0")); // no primary → first
        assert_eq!(pick(&[], Some("DP-1")), None);
    }

    /// Opens the real X11 backend on `$DISPLAY` for ~1 s with the test pattern, then tears it
    /// down: `cargo test x11_backend_smoke -- --ignored --nocapture`. Ignored because it opens
    /// a window.
    #[test]
    #[ignore]
    fn x11_backend_smoke() {
        use smithay_client_toolkit::reexports::calloop::{channel, ping};
        let (cmds, cmd_rx) = channel::channel();
        let (ping, ping_rx) = ping::make_ping().expect("ping");
        let slot: SnapshotSlot = Arc::new(std::sync::Mutex::new(None));
        let (ready_tx, ready_rx) = mpsc::channel();
        let opts = OverlayOptions {
            output: std::env::var("FORZA_OVERLAY_OUTPUT").ok(),
            test_pattern: true,
            coop: None,
        };
        let join = std::thread::spawn(move || run(opts, cmd_rx, ping_rx, slot, ready_tx));
        let ready = ready_rx.recv_timeout(Duration::from_secs(5)).expect("thread start");
        assert_eq!(ready, Ok(()));
        cmds.send(OverlayCmd::Show).expect("show");
        for _ in 0..60 {
            ping.ping();
            std::thread::sleep(Duration::from_millis(16));
        }
        // Inspect our window from a second connection: override-redirect, mapped, depth 32,
        // and an empty input shape (click-through).
        let (x, screen) = XCBConnection::connect(None).expect("connect");
        let root = x.setup().roots[screen].root;
        let ours = |x: &XCBConnection| -> Option<u32> {
            let tree = x.query_tree(root).ok()?.reply().ok()?;
            tree.children.into_iter().find(|&w| {
                let p = x.get_property(false, w, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 64).ok().and_then(|c| c.reply().ok());
                p.is_some_and(|p| p.value == NAME)
            })
        };
        let w = ours(&x).expect("overlay window exists while shown");
        let attrs = x.get_window_attributes(w).expect("attrs").reply().expect("attrs reply");
        assert!(attrs.override_redirect, "override-redirect");
        assert_eq!(attrs.map_state, x11rb::protocol::xproto::MapState::VIEWABLE);
        assert_eq!(x.get_geometry(w).expect("geo").reply().expect("geo reply").depth, 32);
        let input = x.shape_get_rectangles(w, shape::SK::INPUT).expect("shape").reply().expect("shape reply");
        assert!(input.rectangles.is_empty(), "input shape must be empty: {:?}", input.rectangles);

        cmds.send(OverlayCmd::Hide).expect("hide");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(ours(&x), None, "hidden = no window");
        cmds.send(OverlayCmd::Shutdown).expect("shutdown");
        join.join().expect("join");
    }
}
