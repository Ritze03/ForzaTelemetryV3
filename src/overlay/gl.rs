//! EGL through glutin. The display, config and context live as long as the overlay thread;
//! window surfaces come and go with the layer surface / X window (see `wayland.rs`,
//! `x11.rs`), and between them
//! the context is current *surfaceless*, so fonts and textures never need a re-upload.

use std::any::Any;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::num::NonZeroU32;
use std::ptr::NonNull;
use std::sync::Arc;

use egui_glow::glow;
use glutin::api::egl::{config::Config, context::PossiblyCurrentContext, display::Display, surface::Surface};
use glutin::config::{ColorBufferType, ConfigSurfaceTypes, ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextAttributesBuilder, PossiblyCurrentGlContext};
use glutin::display::GlDisplay;
use glutin::surface::{GlSurface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle};
use smithay_client_toolkit::reexports::client::{protocol::wl_surface::WlSurface, Connection, Proxy};

pub type WinSurface = Surface<WindowSurface>;

/// Dropping it tears EGL down in the only safe order (see `Drop`), on every path: normal
/// shutdown, an `init` error or a panic.
pub struct Gl {
    pub glow: Arc<glow::Context>,
    context: ManuallyDrop<PossiblyCurrentContext>,
    config: Config,
    display: ManuallyDrop<Display>,
    /// Keeps the native display (our `wl_display`, or the X11 backend's xcb connection) alive
    /// until after `eglTerminate`. Must stay the *last* field: fields drop after `Drop::drop`
    /// and in declaration order.
    _native: Box<dyn Any>,
}

impl Gl {
    /// EGL display on our own `wl_display`, an explicit 8-bit RGBA config and a context made
    /// current surfaceless (there is no surface until the first show).
    pub fn new(conn: &Connection) -> Result<Self, String> {
        let ptr = NonNull::new(conn.backend().display_ptr().cast::<c_void>())
            .ok_or("no wl_display pointer")?;
        let raw = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(ptr));
        // SAFETY: `ptr` is the live wl_display of `conn`, and `native` keeps a clone of it.
        unsafe { Self::with_native(raw, &|_| true, Box::new(conn.clone())) }
    }

    /// EGL on any native display. `accept` narrows the config choice (the X11 backend needs
    /// one whose native visual is 32-bit ARGB).
    ///
    /// # Safety
    /// `raw` must stay valid for as long as `native` lives. `Gl` holds `native` as its last
    /// field, so the display outlives the EGLDisplay's teardown in `Drop` (and the error path
    /// below terminates before `native` can go).
    pub unsafe fn with_native(
        raw: RawDisplayHandle,
        accept: &dyn Fn(&Config) -> bool,
        native: Box<dyn Any>,
    ) -> Result<Self, String> {
        // SAFETY: the caller's contract.
        let display = unsafe { Display::new(raw) }.map_err(|e| format!("EGL display: {e}"))?;
        match Self::init(&display, ConfigSurfaceTypes::WINDOW, accept) {
            Ok((glow, context, config)) => Ok(Self {
                glow: Arc::new(glow),
                context: ManuallyDrop::new(context),
                config,
                display: ManuallyDrop::new(display),
                _native: native,
            }),
            Err(e) => {
                // SAFETY: nothing created from this display survived `init`'s failure.
                unsafe { display.terminate() };
                Err(e)
            }
        }
    }

    /// `surfaces`: what the config must support (`WINDOW` for the overlay; nothing for the
    /// headless harness, which renders into an FBO).
    fn init(
        display: &Display,
        surfaces: ConfigSurfaceTypes,
        accept: &dyn Fn(&Config) -> bool,
    ) -> Result<(glow::Context, PossiblyCurrentContext, Config), String> {
        let template = ConfigTemplateBuilder::new()
            .with_alpha_size(8)
            .with_transparency(true)
            .with_surface_type(surfaces)
            .build();
        // SAFETY: plain config query on a valid display.
        let configs = unsafe { display.find_configs(template) }.map_err(|e| format!("EGL configs: {e}"))?;
        // Filtered by hand: `with_transparency` only asks for alpha != 0, and on a 10-bit output
        // Mesa may offer 2101010 (2-bit alpha), which would wreck every fade. No MSAA: epaint
        // feathers every edge already.
        let rgba8 = Some(ColorBufferType::Rgb { r_size: 8, g_size: 8, b_size: 8 });
        let config = configs
            .into_iter()
            .find(|c| c.alpha_size() == 8 && c.num_samples() == 0 && c.color_buffer_type() == rgba8 && accept(c))
            .ok_or("no 8-bit RGBA EGL config")?;

        // SAFETY: config comes from this display; no window handle needed for EGL contexts.
        let context = unsafe { display.create_context(&config, &ContextAttributesBuilder::new().build(None)) }
            .map_err(|e| format!("EGL context: {e}"))?
            .make_current_surfaceless()
            .map_err(|e| format!("EGL surfaceless context (EGL_KHR_surfaceless_context): {e}"))?;

        // SAFETY: the context is current on this thread and stays so for the thread's lifetime.
        let glow = unsafe { glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s)) };
        Ok((glow, context, config))
    }

    /// EGL window surface (`wl_egl_window`) on `wl_surface`, made current, swap interval 0.
    /// The caller must drop it (after `release`) before the `wl_surface` is destroyed.
    pub fn create_surface(&self, wl_surface: &WlSurface, w: u32, h: u32) -> Result<WinSurface, String> {
        let ptr = NonNull::new(wl_surface.id().as_ptr().cast::<c_void>()).ok_or("dead wl_surface")?;
        // SAFETY: the wl_surface outlives the EGL surface (wayland.rs drops this first).
        unsafe { self.create_window_surface(RawWindowHandle::Wayland(WaylandWindowHandle::new(ptr)), w, h) }
    }

    /// [`Self::create_surface`] for any native window.
    ///
    /// # Safety
    /// The native window must outlive the returned surface (drop it, after `release`, first).
    pub unsafe fn create_window_surface(&self, window: RawWindowHandle, w: u32, h: u32) -> Result<WinSurface, String> {
        let (w, h) = (NonZeroU32::new(w).ok_or("zero width")?, NonZeroU32::new(h).ok_or("zero height")?);
        let attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(window, w, h);
        // SAFETY: the caller's contract.
        let surface = unsafe { self.display.create_window_surface(&self.config, &attrs) }
            .map_err(|e| format!("EGL window surface: {e}"))?;
        self.make_current(&surface)?;
        // Interval 0: we pace with our own frame callbacks (Wayland) or pings/timer (X11). With
        // 1, Mesa blocks the next swap on vblank, which throttles twice. Not fatal if refused.
        if let Err(e) = surface.set_swap_interval(&self.context, SwapInterval::DontWait) {
            eprintln!("overlay: swap interval 0 refused: {e}");
        }
        Ok(surface)
    }

    /// The chosen config's `EGL_NATIVE_VISUAL_ID` (the X visual on X11).
    pub fn native_visual(&self) -> u32 {
        self.config.native_visual()
    }

    pub fn make_current(&self, surface: &WinSurface) -> Result<(), String> {
        self.context.make_current(surface).map_err(|e| format!("EGL make current: {e}"))
    }

    /// Detach from any window surface (call before dropping one).
    pub fn release(&self) {
        if let Err(e) = self.context.make_current_surfaceless() {
            eprintln!("overlay: EGL surfaceless make current failed: {e}");
        }
    }

    pub fn resize(&self, surface: &WinSurface, w: u32, h: u32) {
        if let (Some(w), Some(h)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
            surface.resize(&self.context, w, h);
        }
    }

    /// Swap (which also commits the wl_surface).
    // ponytail: full-surface damage. Upgrade: have the draw hook return widget rects, union
    // them with last frame's (plus outline margin), flip y (EGL damage is bottom-left origin)
    // and pass them here. Only pays off when the game shows a static screen.
    pub fn swap(&self, surface: &WinSurface) -> Result<(), String> {
        surface.swap_buffers_with_damage(&self.context, &[]).map_err(|e| format!("EGL swap: {e}"))
    }

}

impl Drop for Gl {
    /// Context before display, then `eglTerminate`: our EGLDisplay belongs to our own
    /// wl_display / xcb connection, and this Mesa lacks `EGL_KHR_display_reference`, so nothing
    /// else would terminate it; a stale initialised one could be handed back by Mesa if a later
    /// connection reuses the same pointer. `_native` drops after this body.
    /// Callers must have dropped every window surface first (`Live` goes before `Gl`).
    fn drop(&mut self) {
        // SAFETY: each field is taken exactly once, here, and never touched again.
        let (context, display) = unsafe { (ManuallyDrop::take(&mut self.context), ManuallyDrop::take(&mut self.display)) };
        // Not current first, or eglDestroyContext only defers the destruction.
        drop(context.make_not_current());
        // SAFETY: every surface and the context from this display are gone, and no other
        // library uses this EGLDisplay (it wraps our private wl_display). `config` still holds
        // a display handle, but dropping it after terminate only releases an Arc.
        unsafe { display.terminate() };
    }
}

/// Headless EGL for the offscreen PNG harness: a GPU device display (`EGL_EXT_device_*`,
/// no Wayland, no window) with the overlay's context setup, current surfaceless. Render into
/// an FBO; `Drop` tears down in `Gl`'s order.
#[cfg(test)]
pub struct Headless {
    pub glow: Arc<glow::Context>,
    context: ManuallyDrop<PossiblyCurrentContext>,
    display: ManuallyDrop<Display>,
}

#[cfg(test)]
impl Headless {
    pub fn new() -> Result<Self, String> {
        use glutin::api::egl::device::Device;
        let mut last = String::from("no EGL device");
        let devices = Device::query_devices().map_err(|e| format!("EGL device query: {e}"))?;
        for device in devices {
            // SAFETY: no native display handle; the device outlives the EGL library.
            let display = match unsafe { Display::with_device(&device, None) } {
                Ok(d) => d,
                Err(e) => {
                    last = format!("EGL device display: {e}");
                    continue;
                }
            };
            match Gl::init(&display, ConfigSurfaceTypes::empty(), &|_| true) {
                Ok((glow, context, _)) => {
                    return Ok(Self { glow: Arc::new(glow), context: ManuallyDrop::new(context), display: ManuallyDrop::new(display) })
                }
                Err(e) => {
                    last = e;
                    // SAFETY: nothing created from this display survived `init`'s failure.
                    unsafe { display.terminate() };
                }
            }
        }
        Err(last)
    }
}

#[cfg(test)]
impl Drop for Headless {
    fn drop(&mut self) {
        // SAFETY: as `Gl::drop`: each field taken once; the caller dropped the painter first.
        let (context, display) = unsafe { (ManuallyDrop::take(&mut self.context), ManuallyDrop::take(&mut self.display)) };
        drop(context.make_not_current());
        // SAFETY: the context from this display is gone.
        unsafe { display.terminate() };
    }
}
