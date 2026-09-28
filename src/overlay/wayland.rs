//! The overlay thread: its own Wayland connection, a calloop loop, one full-output
//! `Layer::Overlay` surface that exists only while shown, and frame-callback pacing.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::channel::{Channel, Event};
use smithay_client_toolkit::reexports::calloop::ping::PingSource;
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle, RegistrationToken};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::{registry_queue_init, GlobalList};
use smithay_client_toolkit::reexports::client::protocol::{wl_output::{self, WlOutput}, wl_surface::WlSurface};
use smithay_client_toolkit::reexports::client::{Connection, EventQueue, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::{delegate_compositor, delegate_layer, delegate_output, delegate_registry, registry_handlers};

use super::gl::{Gl, WinSurface};
use super::render::Renderer;
use super::snapshot::{HudSnapshot, SnapshotSlot};
use super::{capability, DisabledReason, OverlayCmd, OverlayOptions};

/// Layer namespace, for compositor rules (e.g. Hyprland `no_anim`).
const NAMESPACE: &str = "forza-telemetry-hud";

/// No ping for this long means packets stopped (2.5 frames at FH6's ~60 Hz), so a running
/// animation must be driven by our own timer instead.
const PING_STALE: Duration = Duration::from_millis(40);
/// Animation step when no packets drive frames: ~60 Hz, like packets, never the monitor's
/// rate (DP-1 is 280 Hz VRR; extra commits there cause judder).
const ANIM_FRAME: Duration = Duration::from_millis(16);
/// Consecutive make_current/swap failures before the overlay gives up (lost context or GPU
/// reset: every swap fails, so recreating the surface forever would only spam stderr).
const MAX_EGL_FAILURES: u8 = 3;

/// D17 pacing: after a frame, when must the next one come without a new packet? `None` =
/// only on the next ping. While packets flow the timer is a fallback that the next ping
/// cancels, so it only fires if packets stop mid-animation (then frames step at
/// [`ANIM_FRAME`]). A frame is never scheduled per frame callback, i.e. at the monitor's rate.
fn next_wake(animating: bool, since_ping: Option<Duration>) -> Option<Duration> {
    if !animating {
        return None;
    }
    match since_ping {
        Some(t) if t < PING_STALE => Some(PING_STALE - t),
        _ => Some(ANIM_FRAME),
    }
}

/// The mapped surface. Field order is drop order: the EGL surface (and its
/// `wl_egl_window`) must go before the `wl_surface` the layer surface owns.
struct Live {
    egl: Option<WinSurface>,
    layer: LayerSurface,
    output: WlOutput,
    size: [u32; 2],
}

pub(super) struct Overlay {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    qh: QueueHandle<Overlay>,
    handle: LoopHandle<'static, Overlay>,
    // Drop order (see `impl Drop for Overlay`): `live` (window surface) is already gone, then
    // `renderer` (painter, needs the context current), then `gl` (context, eglTerminate).
    live: Option<Live>,
    renderer: Renderer,
    gl: Option<Gl>,
    slot: SnapshotSlot,
    latest: Option<HudSnapshot>,
    visible: bool,
    /// Output name to show on; `None` = the first output.
    target: Option<String>,
    test_pattern: bool,
    /// A redraw is wanted (new packet, configure, or the animation timer fired).
    dirty: bool,
    /// A frame callback is outstanding: the compositor hasn't shown our last frame yet.
    frame_pending: bool,
    last_ping: Option<Instant>,
    /// The one pending animation timer (see [`next_wake`]).
    anim_timer: Option<RegistrationToken>,
    egl_failures: u8,
    exit: bool,
}

impl Drop for Overlay {
    /// Runs on every path (shutdown, init error, panic): detach and drop the window surface
    /// so the remaining fields drop with the context current surfaceless.
    fn drop(&mut self) {
        self.destroy_surface();
    }
}

struct Wl {
    conn: Connection,
    globals: GlobalList,
    queue: EventQueue<Overlay>,
}

fn connect() -> Result<Wl, DisabledReason> {
    let err = |e: &dyn std::fmt::Display| DisabledReason::Wayland(e.to_string());
    let conn = Connection::connect_to_env().map_err(|e| err(&e))?;
    let (globals, queue) = registry_queue_init(&conn).map_err(|e| err(&e))?;
    Ok(Wl { conn, globals, queue })
}

/// Thread body. Reports startup success/failure on `ready`, then runs until Shutdown or
/// until the handle's command sender is dropped.
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
            eprintln!("overlay: event loop stopped: {e}");
            break;
        }
    }
    // Teardown is structural: Overlay's Drop releases the surface, then the painter and EGL
    // drop in field order. `Gl` holds its own connection clone, so the wl_display outlives it.
    drop(state);
    drop(event_loop);
}

#[allow(clippy::type_complexity)]
fn init(
    opts: OverlayOptions,
    cmds: Channel<OverlayCmd>,
    ping: PingSource,
    slot: SnapshotSlot,
) -> Result<(EventLoop<'static, Overlay>, Overlay), DisabledReason> {
    let display = std::env::var("WAYLAND_DISPLAY").ok();
    // Probes run in order, each only when the ones before it passed; capability() names the
    // first failure. WAYLAND_DISPLAY is checked before connecting because connect_to_env
    // falls back to "wayland-0" and could reach a stray socket from an X11 session.
    let wl = if display.as_deref().is_some_and(|d| !d.is_empty()) { Some(connect()?) } else { None };
    let layer_shell = wl.as_ref().and_then(|w| LayerShell::bind(&w.globals, &w.queue.handle()).ok());
    let gpu = match (&wl, &layer_shell) {
        (Some(w), Some(_)) => Gl::new(&w.conn).and_then(|gl| Ok((Renderer::new(gl.glow.clone())?, gl))),
        _ => Err(String::new()),
    };
    capability(display.as_deref(), layer_shell.is_some(), gpu.as_ref().err().map(String::as_str))?;
    let (Some(Wl { conn, globals, mut queue }), Some(layer_shell), Ok(gpu)) = (wl, layer_shell, gpu) else {
        return Err(DisabledReason::Wayland("startup probe mismatch".into())); // unreachable
    };

    // `gpu` stays one tuple until moved into `state`: a tuple drops its fields in order, so an
    // early `?` below still drops the painter (renderer) before Gl terminates EGL.
    let err = |e: &dyn std::fmt::Display| DisabledReason::Wayland(e.to_string());
    let event_loop = EventLoop::<Overlay>::try_new().map_err(|e| err(&e))?;
    let handle = event_loop.handle();
    let qh = queue.handle();
    let mut state = Overlay {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        compositor: CompositorState::bind(&globals, &qh).map_err(|e| err(&e))?,
        layer_shell,
        qh,
        handle: handle.clone(),
        live: None,
        renderer: gpu.0,
        gl: Some(gpu.1),
        slot,
        latest: None,
        visible: false,
        target: opts.output,
        test_pattern: opts.test_pattern,
        dirty: false,
        frame_pending: false,
        last_ping: None,
        anim_timer: None,
        egl_failures: 0,
        exit: false,
    };
    // One roundtrip so output names are known before the first Show.
    queue.roundtrip(&mut state).map_err(|e| err(&e))?;

    WaylandSource::new(conn, queue).insert(handle.clone()).map_err(|e| err(&e.error))?;
    handle
        .insert_source(cmds, |ev, _, s: &mut Overlay| match ev {
            Event::Msg(cmd) => s.command(cmd),
            Event::Closed => s.exit = true,
        })
        .map_err(|e| err(&e.error))?;
    // Per-packet wake-up (D17): one redraw per ping, coalesced by calloop.
    handle
        .insert_source(ping, |_, _, s: &mut Overlay| {
            s.last_ping = Some(Instant::now());
            if let Some(t) = s.anim_timer.take() {
                s.handle.remove(t); // packets are driving frames again
            }
            s.dirty = true;
            s.maybe_render();
        })
        .map_err(|e| err(&e.error))?;
    Ok((event_loop, state))
}

impl Overlay {
    fn command(&mut self, cmd: OverlayCmd) {
        match cmd {
            OverlayCmd::Show => {
                self.visible = true;
                self.ensure_surface(None);
            }
            OverlayCmd::Hide => {
                self.visible = false;
                self.destroy_surface();
            }
            OverlayCmd::SetOutput(name) => {
                self.target = name;
                // A layer surface can't move between outputs: recreate it on the new one.
                if self.live.as_ref().is_some_and(|l| Some(&l.output) != self.pick_output(None).as_ref()) {
                    self.destroy_surface();
                }
                self.ensure_surface(None);
            }
            OverlayCmd::Shutdown => self.exit = true,
        }
    }

    fn pick_output(&self, skip: Option<&WlOutput>) -> Option<WlOutput> {
        let mut all = self.outputs.outputs().filter(|o| Some(o) != skip);
        match &self.target {
            Some(name) => all.find(|o| self.outputs.info(o).and_then(|i| i.name).as_deref() == Some(name)),
            None => all.next(),
        }
    }

    /// Create the layer surface if shown, missing, and the target output exists. It is
    /// drawn once the compositor's first configure arrives.
    fn ensure_surface(&mut self, skip: Option<&WlOutput>) {
        if !self.visible || self.live.is_some() {
            return;
        }
        let Some(output) = self.pick_output(skip) else { return };
        // Click-through: an *empty* input region. `None` would mean "everything" and eat all input.
        let Ok(region) = Region::new(&self.compositor) else { return };
        let wl_surface = self.compositor.create_surface(&self.qh);
        wl_surface.set_input_region(Some(region.wl_region()));
        let layer = self.layer_shell.create_layer_surface(&self.qh, wl_surface, Layer::Overlay, Some(NAMESPACE), Some(&output));
        layer.set_anchor(Anchor::all());
        layer.set_size(0, 0); // fill the output
        layer.set_exclusive_zone(-1); // ignore other surfaces' exclusive zones (bars)
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit(); // no buffer yet: asks for the first configure
        self.live = Some(Live { egl: None, layer, output, size: [0, 0] });
    }

    /// Unmap by destroying: while an overlay-layer surface is mapped, Hyprland disables
    /// direct scanout/tearing on that output, so an invisible-but-mapped surface still costs.
    /// The EGL context and painter stay alive (surfaceless) so a re-show re-uploads nothing.
    fn destroy_surface(&mut self) {
        if let Some(Live { egl, layer, .. }) = self.live.take() {
            if let Some(gl) = &self.gl {
                gl.release();
            }
            drop(egl); // eglDestroySurface + wl_egl_window_destroy
            drop(layer); // layer role, then wl_surface
        }
        self.frame_pending = false;
    }

    fn maybe_render(&mut self) {
        if self.exit || self.frame_pending || !self.dirty {
            return;
        }
        let (Some(live), Some(gl)) = (&self.live, &self.gl) else { return };
        let Some(egl) = &live.egl else { return };
        if let Ok(slot) = self.slot.try_lock() {
            self.latest.clone_from(&slot);
        }
        let result = gl.make_current(egl).map(|()| {
            let wl = live.layer.wl_surface();
            wl.frame(&self.qh, wl.clone()); // before the swap, which commits
            self.renderer.frame(live.size, self.latest.as_ref(), self.test_pattern)
        });
        match result.and_then(|animating| gl.swap(egl).map(|()| animating)) {
            Ok(animating) => {
                self.frame_pending = true;
                self.dirty = false;
                self.egl_failures = 0;
                let wake = next_wake(animating, self.last_ping.map(|t| t.elapsed()));
                self.arm_anim_timer(wake);
            }
            Err(e) => self.egl_failed(&e),
        }
    }

    /// Count an EGL failure (swap, or surface creation on configure). Below the cap, rebuild the
    /// surface so the next configure retries; at the cap, stop the thread (`is_dead()`).
    fn egl_failed(&mut self, e: &dyn std::fmt::Display) {
        self.egl_failures += 1;
        if self.egl_failures >= MAX_EGL_FAILURES {
            // Lost context / GPU reset: stop the thread; the handle reports is_dead().
            eprintln!("overlay: {e}; {MAX_EGL_FAILURES} EGL failures in a row, shutting the overlay down");
            self.exit = true;
            return;
        }
        // Usually EGL_BAD_SURFACE / BAD_NATIVE_WINDOW: rebuild the surface.
        eprintln!("overlay: {e}; recreating the surface");
        self.destroy_surface();
        self.ensure_surface(None);
    }

    /// Replace any pending animation timer with one firing after `after` (`None`: cancel).
    /// A skipped render gets no frame callback, so without this timer nothing would wake us.
    fn arm_anim_timer(&mut self, after: Option<Duration>) {
        if let Some(t) = self.anim_timer.take() {
            self.handle.remove(t);
        }
        let Some(after) = after else { return };
        let timer = Timer::from_duration(after);
        match self.handle.insert_source(timer, |_, _, s: &mut Overlay| {
            s.anim_timer = None;
            s.dirty = true;
            s.maybe_render(); // if a frame is still pending, its callback renders instead
            TimeoutAction::Drop
        }) {
            Ok(token) => self.anim_timer = Some(token),
            Err(e) => eprintln!("overlay: animation timer: {}", e.error),
        }
    }
}

impl CompositorHandler for Overlay {
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        if self.live.as_ref().is_some_and(|l| l.layer.wl_surface() == surface) {
            self.frame_pending = false;
            self.maybe_render();
        }
    }
    // ponytail: integer/fractional scale ignored (all target monitors are 1080p @1).
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: wl_output::Transform) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &WlOutput) {}
}

impl OutputHandler for Overlay {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    // A new output, or a name arriving late, may be the target we are waiting for.
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {
        self.ensure_surface(None);
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {
        self.ensure_surface(None);
    }
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        if self.live.as_ref().is_some_and(|l| l.output == output) {
            self.destroy_surface();
            // sctk still lists the dying output during this call, so skip it explicitly.
            self.ensure_surface(Some(&output));
        }
    }
}

impl LayerShellHandler for Overlay {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        // Compositor withdrew it; wait for the next Show / output change rather than fight it.
        if self.live.as_ref().is_some_and(|l| &l.layer == layer) {
            self.destroy_surface();
        }
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, cfg: LayerSurfaceConfigure, _: u32) {
        let (Some(live), Some(gl)) = (self.live.as_mut(), &self.gl) else { return };
        if &live.layer != layer {
            return;
        }
        let (w, h) = cfg.new_size;
        if w == 0 || h == 0 {
            eprintln!("overlay: compositor configured a {w}x{h} surface; waiting");
            return;
        }
        live.size = [w, h];
        match &live.egl {
            Some(egl) => gl.resize(egl, w, h),
            None => match gl.create_surface(layer.wl_surface(), w, h) {
                Ok(egl) => live.egl = Some(egl),
                Err(e) => return self.egl_failed(&e),
            },
        }
        self.dirty = true;
        self.maybe_render();
    }
}

impl ProvidesRegistryState for Overlay {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_compositor!(Overlay);
delegate_output!(Overlay);
delegate_layer!(Overlay);
delegate_registry!(Overlay);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_wake_idle_when_not_animating() {
        assert_eq!(next_wake(false, None), None);
        assert_eq!(next_wake(false, Some(Duration::ZERO)), None);
        assert_eq!(next_wake(false, Some(Duration::from_secs(5))), None);
    }

    #[test]
    fn next_wake_packets_flowing_only_arms_the_stale_fallback() {
        // Frame drawn right on a ping: the next ping (~16.7 ms) comes before the fallback.
        assert_eq!(next_wake(true, Some(Duration::ZERO)), Some(PING_STALE));
        assert_eq!(next_wake(true, Some(Duration::from_millis(10))), Some(Duration::from_millis(30)));
        assert!(next_wake(true, Some(Duration::ZERO)).is_some_and(|d| d > Duration::from_micros(16_667)));
    }

    #[test]
    fn next_wake_packets_stopped_steps_at_anim_frame() {
        assert_eq!(next_wake(true, None), Some(ANIM_FRAME));
        assert_eq!(next_wake(true, Some(PING_STALE)), Some(ANIM_FRAME));
        assert_eq!(next_wake(true, Some(Duration::from_secs(3))), Some(ANIM_FRAME));
    }
}
