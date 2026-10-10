//! The shared GL 3D map renderer (phase K, K2, D61): terrain from the game's height raster with
//! the satellite image draped on it, roads as ribbons with decks at their nav heights. One
//! implementation for the HUD minimap, the Dashboard map and the Map-tab viewer; the call sites
//! (K3, K4) only fill a [`Scene3d`] and call [`add_scene`].
//!
//! ```text
//! mod       this file: Gl3dHandle / Gl3dState / Gl3dStatus, Scene3d, add_scene, the slow-GPU guard, the debug log
//! scene     Gl3d: the GL objects of one context, render (FBO + depth) and composite
//! clipmap   terrain: 7-level geometry clipmap, R16UI height texture
//! roads     road buffers, the per-frame draw plan (tiles, LOD sets), the style table
//! marker    the own-car marker (3D arrow / low-poly sedan) and the trail ribbons (D77 / D78)
//! racemark  the race lines' start / finish posts (D88)
//! shaders   GLSL for desktop GL 3.3 core and OpenGL ES 3.0
//! probe     what the context offers; the requirements
//! ```
//!
//! # How a map uses it (the shape of the call at all three sites)
//!
//! ```ignore
//! // once, in the owner of the GL context (Renderer on the overlay thread, ForzaApp for eframe):
//! let gl3d = Gl3dHandle::new();
//! // every frame the map is in 3D mode and `store::terrain()` is Ready:
//! let cam = Camera::from_cfg_relief(&cfg.tilt, car, yaw, zoom, rect, Some(&terrain), car_y);
//! if gl3d.wants_underlay() {
//!     // not Ready yet (first frames, or failed for good): the tilted 2D map, exactly as today
//!     draw_base(&painter, &base_params_of(&cam_without_relief)); draw_layers(...);
//! }
//! gl3d::add_scene(&painter, &gl3d, Scene3d { cam: cam.clone(), mesh: store::road_mesh(..), map: Some(tex), ... });
//! // then, over the 3D, as ever: draw_layers_parts(.., Parts::OVER_3D), teammates, then the own car
//! // (D77: gl3d::add_marker(&painter, &gl3d, MarkerScene { .. }) - its own callback, on top), compass, border
//! // `gl3d.busy()` -> ask for another frame (the init is spread over a few);
//! // at shutdown, context current: gl3d.destroy(&gl) (HUD: before painter.destroy()).
//! ```
//!
//! # Design decisions and their whys (`docs/features/minimap.md`, "3D renderer")
//!
//! * **The scene is rendered into the renderer's own FBO + depth buffer and composited as a quad
//!   inside the same `egui_glow::CallbackFn`** - not `register_native_texture`. That needs
//!   `&mut Painter` (only the overlay has it; eframe's `Frame` has no *replace*), so a resized
//!   FBO texture would leak or churn ids, and the Dashboard's widget code has no `Frame`. The
//!   callback needs nothing from the frame loop: the call is identical at all three sites, the
//!   lifetime and failure of the GL objects live in one place, and the composite shader does
//!   the HUD pill's rounded mask and the fade for free.
//! * **One `Gl3d` per GL context, created lazily inside the callback** (the one place with the
//!   right context current), state in a [`Gl3dHandle`] the call site owns and clones into each
//!   callback. The Dashboard and the Viewer share the `ForzaApp`'s handle: the FBO is transient
//!   (render, composite, done), so one serves any number of callbacks in a frame.
//! * **Init is spread over frames** (shaders and static buffers; then the height raster; then the
//!   roads), each step a few ms, so there is no 40 ms hitch on the HUD. Until the terrain is
//!   uploaded the status is not `Ready` and the call site keeps drawing the tilted 2D map.
//! * **Soft fallback.** Missing GL features, a shader that does not compile, an incomplete FBO,
//!   a GL error in the first frames, or a software GL that cannot keep up
//!   ([`Gl3dOptions::guard`], a timer-query / CPU EMA over 8 ms for 2 s) end in
//!   `Gl3dStatus::Failed(reason)` (also [`last_failure`] for the settings UI, which lives on
//!   another thread than the HUD's context); the map then simply keeps drawing 2D. Never a panic.

// Phase K: the call sites (K3 HUD, K4 Dashboard + Viewer) land after this module; until they do, the
// public surface is exercised by the tests only.
#![allow(dead_code)]

mod clipmap;
mod marker;
mod probe;
mod racemark;
mod roads;
mod scene;
mod shaders;
#[cfg(all(test, target_os = "linux"))]
mod tests;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{PaintCallback, PaintCallbackInfo, Painter, Shape};
use egui_glow::glow::{self, HasContext};

use super::cfg::{RaceCfg, RaceFocusCfg, ReliefCfg, RoadsCfg};
use super::mesh3d::RoadMesh;
use super::paint2d::ImageLook;
use super::racesel::{RaceDraw, RoadFocus};
use super::view::Camera;
use super::MapTex;
use crate::minimap::MapCalibration;
use scene::{Frame, Gl3d, RaceFrame};

pub use marker::{Marker3d, Trail3d, TrailSeg, GROUND_BELOW_M};
pub use probe::Caps;
pub use scene::RenderStats;

/// Reinterpret a slice of plain numbers as bytes for a GL upload (little endian, like every
/// target this builds for; the mesh bytes of `mesh3d` are little endian too).
fn as_bytes<T: Copy>(v: &[T]) -> &[u8] {
    #[cfg(target_endian = "big")]
    compile_error!("the GL uploads assume a little-endian target");
    // SAFETY: `T: Copy` plain numbers (u16 / u32 / f32 here) have no padding and every byte
    // pattern is a valid `u8`; the slice covers exactly the same memory.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) }
}

// ── status ───────────────────────────────────────────────────────────────────────────────────

/// Where a context's 3D renderer stands. The call sites draw the tilted 2D map underneath until
/// it is `Ready` ([`Gl3dHandle::wants_underlay`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gl3dStatus {
    /// Nothing drawn yet (not created, or still uploading).
    Untried,
    /// The terrain is on the GPU and the scene is drawn.
    Ready,
    /// Gave up on this context for good; the text is for the status line.
    Failed(String),
}

impl Gl3dStatus {
    /// The reason, when failed.
    pub fn failure(&self) -> Option<&str> {
        match self {
            Gl3dStatus::Failed(e) => Some(e),
            _ => None,
        }
    }
}

static LAST_FAILURE: Mutex<Option<String>> = Mutex::new(None);

/// The most recent reason any context gave up on 3D (process-wide). *Why a global:* the HUD's
/// renderer fails on the overlay thread, in a context the settings UI cannot touch; the UI
/// reads this for its status line ("3D unavailable: ...").
pub fn last_failure() -> Option<String> {
    LAST_FAILURE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Forget [`last_failure`] if it is still `reason`: a call site that retries a failed context
/// (K4: the user toggled the map into 3D again) clears its own old failure so the status line
/// does not show a stale one; another context's newer failure stays.
pub fn clear_failure_if(reason: &str) {
    let mut g = LAST_FAILURE.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_deref() == Some(reason) {
        *g = None;
    }
}

// ── options ──────────────────────────────────────────────────────────────────────────────────

/// Requirements and switches of one renderer. [`Gl3dOptions::default`] is production; tests turn
/// things off or up.
#[derive(Clone, Debug)]
pub struct Gl3dOptions {
    /// Minimum desktop GL version (3.3: `#version 330 core`).
    pub min_gl: (u32, u32),
    /// Minimum OpenGL ES version (3.0: `#version 300 es`).
    pub min_es: (u32, u32),
    /// Give up when the frame costs more than `.0` ms (EMA of the GPU time, or the callback's CPU
    /// time when no timer query exists) for `.1` seconds. `None` = never (tests on llvmpipe).
    pub guard: Option<(f32, f32)>,
    /// One status line per second on stderr (`FORZA_MAP_3D_DEBUG=1`).
    pub debug: bool,
    /// Cull back faces of the road mesh (half the deck's fragments).
    pub cull: bool,
    /// Wait for the GPU every frame and read the timer query (perf measurement only).
    pub sync_timing: bool,
    /// Test hook: feed the terrain shader garbage, as a driver that cannot compile it would.
    pub break_shader: bool,
}

impl Default for Gl3dOptions {
    fn default() -> Self {
        Gl3dOptions {
            min_gl: (3, 3),
            min_es: (3, 0),
            guard: Some((8.0, 2.0)),
            debug: std::env::var_os("FORZA_MAP_3D_DEBUG").is_some_and(|v| v != "0"),
            cull: true,
            sync_timing: false,
            break_shader: false,
        }
    }
}

// ── the scene a call site hands over ─────────────────────────────────────────────────────────

/// The in-race focus (D66): which roads lie on the picked race line, and how the others look.
#[derive(Clone)]
pub struct Focus3d {
    /// `RaceSel::road_focus(layers)`; the flags are rebuilt when this `Arc` changes.
    pub focus: Arc<RoadFocus>,
    pub cfg: RaceFocusCfg,
}

/// The race lines the map draws (D80 / D88): the same scene geometry for every style and mode.
#[derive(Clone)]
pub struct Race3d {
    /// What to draw (`RaceSel::race_draw`): the marks come from here, the lines through `mesh`.
    pub draw: Arc<RaceDraw>,
    /// The lines of `draw` as one mesh (`store::race_mesh`: the newest finished one, built
    /// off-thread; `None` while the first builds). The GPU copy is replaced when this `Arc`
    /// changes.
    pub mesh: Option<Arc<RoadMesh>>,
    /// Style (road / line), colour, line width and alpha, marks on.
    pub cfg: RaceCfg,
}

/// Everything one frame of one map needs. Built per frame by the call site (cheap: `Arc`s and
/// `Copy` configs), moved into the paint callback.
#[derive(Clone)]
pub struct Scene3d {
    /// The 3D camera: must carry a [`Relief`](super::view::Relief) (`Camera::from_cfg_relief`
    /// with a loaded terrain); without one nothing is drawn. Its `rect` is the callback rect
    /// (the GL viewport), its `scale` / `focal` are in points.
    pub cam: Camera,
    /// The newest road mesh (`store::road_mesh`); `None` while it builds = terrain only. The
    /// GPU copy is replaced when this `Arc` changes.
    pub mesh: Option<Arc<RoadMesh>>,
    /// The satellite image as egui uploaded it (`ctx.load_texture`): the renderer reuses that
    /// GL texture, never a second copy. `None` = the plain backing colour (image switched off).
    pub map: Option<MapTex>,
    pub cal: MapCalibration,
    /// `ImageCfg` (opacity, brightness, saturation), applied exactly in the shader.
    pub look: ImageLook,
    /// Mirror the map past its edges (the texture must wrap with `MirroredRepeat`); off = cut.
    pub mirror: bool,
    /// The caller's fade alpha (the HUD's show/hide fade; 1 on the Dashboard).
    pub a: f32,
    /// Size factor of strokes (the HUD's design -> screen scale; 1 on the Dashboard).
    pub s: f32,
    /// Rounded corners of the view in points (the HUD pill's `22 * s`; 0 = square).
    pub corner_radius: f32,
    /// Road height mode, deck thickness, hill shading (`TiltCfg::relief`; the exaggeration is the camera's).
    pub relief: ReliefCfg,
    /// Road styles and the width rule; `roads.on == false` draws no roads.
    pub roads: RoadsCfg,
    /// The in-race focus, when a race line is picked.
    pub focus: Option<Focus3d>,
    /// The race lines of the current mode, hidden behind hills and decks like the roads (D88).
    pub race: Option<Race3d>,
    /// Breadcrumb trails at their recorded heights (D77), drawn after the roads.
    pub trails: Vec<Trail3d>,
}

/// Queue the 3D scene as an egui paint callback over `scene.cam.rect`, clipped by the painter's
/// clip rect. Draw everything that must lie *under* the 3D (the tilted 2D underlay while
/// [`Gl3dHandle::wants_underlay`]) before this call and everything over it (POIs, markers,
/// compass, border) after it: egui keeps the order.
///
/// Does nothing for a camera without relief or a rect without area.
pub fn add_scene(painter: &Painter, handle: &Gl3dHandle, scene: Scene3d) {
    if scene.cam.relief.is_none() || !scene.cam.rect.is_positive() {
        return;
    }
    let rect = scene.cam.rect;
    let h = handle.clone();
    let cb = egui_glow::CallbackFn::new(move |info, painter| h.paint(&info, painter, &scene));
    painter.add(Shape::Callback(PaintCallback { rect, callback: Arc::new(cb) }));
}

/// The own car of one map in 3D (D77 / D78), for [`add_marker`].
#[derive(Clone)]
pub struct MarkerScene {
    /// The same camera as the map's [`Scene3d`].
    pub cam: Camera,
    pub marker: Marker3d,
    /// The fade alpha, size factor and rounded corners, as in the [`Scene3d`].
    pub a: f32,
    pub s: f32,
    pub corner_radius: f32,
}

/// Queue the own-car marker as a paint callback of its own over `m.cam.rect`: call it **after**
/// the egui vectors over the scene (POIs, race lines, teammates), where the flat arrow was drawn,
/// so the car stays on top of them. *Why not in the scene pass:* a callback is composited where it
/// is queued, and the POIs and race lines are egui shapes queued after the scene; the car under a
/// POI icon or the HUD's tint would be a regression from the flat arrow.
///
/// Draws nothing until the scene of this handle is `Ready` (the call site draws the flat arrow
/// while [`Gl3dHandle::wants_underlay`]).
pub fn add_marker(painter: &Painter, handle: &Gl3dHandle, m: MarkerScene) {
    if m.cam.relief.is_none() || !m.cam.rect.is_positive() {
        return;
    }
    let rect = m.cam.rect;
    let h = handle.clone();
    let cb = egui_glow::CallbackFn::new(move |info, painter| h.paint_marker(&info, painter, &m));
    painter.add(Shape::Callback(PaintCallback { rect, callback: Arc::new(cb) }));
}

// ── the handle ───────────────────────────────────────────────────────────────────────────────

/// What the last frame did (tests, the debug line, perf numbers).
#[derive(Clone, Copy, Debug, Default)]
pub struct Gl3dStats {
    /// Frames drawn (not counting the init steps).
    pub frames: u64,
    pub last: RenderStats,
    /// CPU time of the whole callback, ms.
    pub callback_ms: f64,
    /// The guard's running average, ms.
    pub ema_ms: f32,
}

pub struct Gl3dState {
    pub status: Gl3dStatus,
    gl3d: Option<Gl3d>,
    opts: Gl3dOptions,
    guard: Guard,
    stats: Gl3dStats,
    busy: bool,
    errors_checked: u32,
    last_log: Option<Instant>,
}

/// One per GL context, owned by whoever owns the context; cheap to clone (an `Arc`).
#[derive(Clone)]
pub struct Gl3dHandle(Arc<Mutex<Gl3dState>>);

impl Default for Gl3dHandle {
    fn default() -> Self {
        Self::with_options(Gl3dOptions::default())
    }
}

impl Gl3dHandle {
    pub fn new() -> Gl3dHandle {
        Self::default()
    }

    pub fn with_options(opts: Gl3dOptions) -> Gl3dHandle {
        Gl3dHandle(Arc::new(Mutex::new(Gl3dState {
            status: Gl3dStatus::Untried,
            gl3d: None,
            opts,
            guard: Guard::default(),
            stats: Gl3dStats::default(),
            busy: false,
            errors_checked: 0,
            last_log: None,
        })))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Gl3dState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> Gl3dStatus {
        self.lock().status.clone()
    }

    /// Draw the tilted 2D map under the callback this frame? True until the scene is `Ready`
    /// and for good when it failed; the one fallback mechanism.
    pub fn wants_underlay(&self) -> bool {
        self.lock().status != Gl3dStatus::Ready
    }

    /// The init is still being spread over frames (or a road upload waits its turn): the caller
    /// should request another frame even if nothing else changed.
    pub fn busy(&self) -> bool {
        let s = self.lock();
        s.busy || s.status == Gl3dStatus::Untried
    }

    pub fn stats(&self) -> Gl3dStats {
        self.lock().stats
    }

    /// What the context offered, once probed.
    pub fn caps(&self) -> Option<Caps> {
        self.lock().gl3d.as_ref().map(|g| g.caps.clone())
    }

    /// Free every GL object and go back to `Untried`. The context must be current: the HUD's
    /// `Renderer::drop` calls it *before* `painter.destroy()` (and before the context goes),
    /// eframe's `on_exit(Some(gl))` with the `gl` it is given. Safe to call twice and when
    /// nothing was ever created.
    pub fn destroy(&self, gl: &glow::Context) {
        let mut s = self.lock();
        if let Some(mut g) = s.gl3d.take() {
            g.destroy(gl);
        }
        s.status = Gl3dStatus::Untried;
        s.busy = false;
        s.guard = Guard::default();
    }

    /// The paint callback body. Runs with the context current and egui's state set (scissor =
    /// clip rect, viewport = callback rect).
    fn paint(&self, info: &PaintCallbackInfo, painter: &egui_glow::Painter, sc: &Scene3d) {
        let mut s = self.lock();
        s.paint(info, painter, sc);
    }

    /// The marker callback body ([`add_marker`]).
    fn paint_marker(&self, info: &PaintCallbackInfo, painter: &egui_glow::Painter, m: &MarkerScene) {
        let mut s = self.lock();
        s.paint_marker(info, painter, m);
    }
}

impl Gl3dState {
    fn fail(&mut self, gl: &glow::Context, why: String) {
        eprintln!("3D map unavailable: {why}");
        *LAST_FAILURE.lock().unwrap_or_else(|e| e.into_inner()) = Some(why.clone());
        if let Some(mut g) = self.gl3d.take() {
            g.destroy(gl);
        }
        self.status = Gl3dStatus::Failed(why);
        self.busy = false;
    }

    fn paint(&mut self, info: &PaintCallbackInfo, painter: &egui_glow::Painter, sc: &Scene3d) {
        if matches!(self.status, Gl3dStatus::Failed(_)) {
            return;
        }
        let t0 = Instant::now();
        let gl: &glow::Context = painter.gl();
        let vp = info.viewport_in_pixels();
        let size = [vp.width_px, vp.height_px];
        if size[0] <= 0 || size[1] <= 0 {
            return;
        }
        // Errors somebody else left are not ours: drain them (and say so when debugging).
        // SAFETY: plain error query.
        unsafe {
            let mut n = 0;
            loop {
                let e = gl.get_error();
                if e == 0 || n >= 8 {
                    break;
                }
                if self.opts.debug {
                    eprintln!("3D map: pending GL error 0x{e:X} before the callback");
                }
                n += 1;
            }
        }
        self.busy = false;
        let uploaded = match self.step(gl, painter, info, sc, size) {
            Ok(u) => u,
            Err(e) => {
                self.fail(gl, e);
                return;
            }
        };
        // SAFETY: plain error query.
        let err = unsafe { gl.get_error() };
        if err != 0 {
            // The first frames must be clean ("get_error() == 0 after the first frame"); a later
            // stray error (a driver quirk on resize, say) is logged, not fatal.
            if self.errors_checked < 3 || self.stats.frames < 3 {
                self.fail(gl, format!("OpenGL error 0x{err:X} while drawing the scene"));
                return;
            } else if self.opts.debug {
                eprintln!("3D map: GL error 0x{err:X}");
            }
        }
        self.errors_checked += 1;
        self.stats.callback_ms = t0.elapsed().as_secs_f64() * 1e3;
        if self.status == Gl3dStatus::Ready {
            self.feed_guard(gl, uploaded);
            self.log(info, size);
        }
    }

    /// One callback: at most one heavy upload, then (once the terrain is there) the frame.
    /// Returns whether it uploaded something (the guard ignores such frames).
    fn step(&mut self, gl: &glow::Context, painter: &egui_glow::Painter, info: &PaintCallbackInfo, sc: &Scene3d, size: [i32; 2]) -> Result<bool, String> {
        let Some(relief) = sc.cam.relief.as_ref() else { return Ok(false) };
        if self.gl3d.is_none() {
            let caps = probe::probe(gl, &self.opts)?;
            if self.opts.debug {
                eprintln!("3D map: {} | {} | max texture {} | aniso {:?} | timer {}", caps.version, caps.renderer, caps.max_texture, caps.aniso, caps.timer);
            }
            self.gl3d = Some(Gl3d::new(gl, caps, &self.opts)?);
            self.busy = true;
            return Ok(true); // programs and static buffers this call; the heights next
        }
        let g = self.gl3d.as_mut().expect("created above");
        let mut uploaded = false;
        if g.heights.as_ref().map(|h| h.rev) != Some(relief.terrain.rev) {
            g.set_terrain(gl, &relief.terrain)?;
            uploaded = true;
        }
        if let Some(mesh) = &sc.mesh {
            if g.roads.as_ref().is_none_or(|r| !Arc::ptr_eq(&r.mesh, mesh)) {
                if uploaded {
                    self.busy = true; // one heavy upload per callback: the roads next frame
                } else {
                    g.set_roads(gl, mesh.clone())?;
                    uploaded = true;
                }
            }
        }
        g.sync_focus(gl, sc.focus.as_ref().map(|f| &f.focus));
        let race_mesh = sc.race.as_ref().and_then(|r| r.mesh.as_ref());
        if uploaded && !g.race_is(race_mesh) {
            self.busy = true; // one heavy upload per callback: the race lines next frame
        } else {
            uploaded |= g.sync_race(gl, race_mesh)?;
        }
        let ppp = info.pixels_per_point;
        let map = sc.map.and_then(|m| painter.texture(m.id).map(|t| (t, m.orig_size)));
        let frame = Frame {
            cam: &sc.cam,
            ppp,
            size,
            map: map.map(|m| m.0),
            cal: sc.cal,
            orig: map.map_or([1, 1], |m| m.1),
            brightness: sc.look.brightness.clamp(0.0, 1.0),
            saturation: sc.look.saturation.clamp(0.0, 1.0),
            // (the fade alpha `a` multiplies the whole picture, terrain and roads, at the composite)
            opacity: sc.look.opacity.clamp(0.0, 1.0),
            mirror: sc.mirror,
            relief: sc.relief.sane(),
            roads: (sc.roads.on && g.roads.is_some()).then_some(&sc.roads),
            focus: sc.focus.as_ref().map(|f| &f.cfg),
            race: sc.race.as_ref().map(|r| RaceFrame { roads: &sc.roads, cfg: &r.cfg, marks: &r.draw.marks }),
            s: sc.s,
            trails: &sc.trails,
            sync_timing: self.opts.sync_timing,
        };
        let st = g.render(gl, &frame)?;
        g.composite(gl, size, sc.corner_radius * ppp, sc.a.clamp(0.0, 1.0))?;
        self.stats.last = st;
        self.stats.frames += 1;
        self.status = Gl3dStatus::Ready;
        Ok(uploaded)
    }

    /// Draw the own car into the FBO and composite it. Only once the scene is `Ready` (the same
    /// context drew its terrain), else nothing. A GL error in the first frames fails the context
    /// like one in the scene would; later ones are logged.
    fn paint_marker(&mut self, info: &PaintCallbackInfo, painter: &egui_glow::Painter, m: &MarkerScene) {
        if self.status != Gl3dStatus::Ready {
            return;
        }
        let gl: &glow::Context = painter.gl();
        // Errors somebody else left are not ours.
        // SAFETY: plain error queries.
        unsafe { for _ in 0..8 { if gl.get_error() == 0 { break; } } }
        let Some(g) = self.gl3d.as_mut() else { return };
        let vp = info.viewport_in_pixels();
        let size = [vp.width_px, vp.height_px];
        if size[0] <= 0 || size[1] <= 0 {
            return;
        }
        let r = g
            .render_marker(gl, &m.cam, info.pixels_per_point, size, m.s, &m.marker)
            .and_then(|tris| g.composite(gl, size, m.corner_radius * info.pixels_per_point, m.a.clamp(0.0, 1.0)).map(|_| tris));
        match r {
            Ok(tris) => {
                self.stats.last.triangles += tris;
                self.stats.last.draws += 3;
            }
            Err(e) => {
                self.fail(gl, e);
                return;
            }
        }
        // SAFETY: plain error query.
        let err = unsafe { gl.get_error() };
        if err != 0 {
            if self.stats.frames < 3 {
                self.fail(gl, format!("OpenGL error 0x{err:X} while drawing the car marker"));
            } else if self.opts.debug {
                eprintln!("3D map: GL error 0x{err:X} (marker)");
            }
        }
    }

    fn feed_guard(&mut self, gl: &glow::Context, uploaded: bool) {
        let Some((limit_ms, secs)) = self.opts.guard else { return };
        if uploaded {
            self.guard.skip = 5;
        }
        // The GPU time when a timer query finished, else the callback's CPU time (software GL
        // does its vertex work on the calling thread, so it shows up there).
        let ms = self.stats.last.gpu_ms.map_or(self.stats.callback_ms as f32, |g| g as f32).max(0.0);
        let slow = self.guard.feed(ms, Instant::now(), limit_ms, Duration::from_secs_f32(secs));
        self.stats.ema_ms = self.guard.ema;
        if slow {
            self.fail(gl, "3D is too slow on this GPU".to_string());
        }
    }

    fn log(&mut self, info: &PaintCallbackInfo, size: [i32; 2]) {
        if !self.opts.debug {
            return;
        }
        let now = Instant::now();
        if self.last_log.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) {
            return;
        }
        self.last_log = Some(now);
        let (s, st) = (self.stats.last, self.stats);
        eprintln!(
            "3D map: {:?} {}x{} px (ppp {}) | {} tri, {} draws, tiles {} near / {} far | cpu {:.2} ms, gpu {} | ema {:.2} ms | frame {}",
            self.status,
            size[0],
            size[1],
            info.pixels_per_point,
            s.triangles,
            s.draws,
            s.tiles_near,
            s.tiles_far,
            st.callback_ms,
            s.gpu_ms.map_or("n/a".to_string(), |g| format!("{g:.3} ms")),
            st.ema_ms,
            st.frames
        );
    }
}

/// The slow-GPU guard: an exponential average of the frame cost that must stay above the limit
/// for `secs` before the renderer gives up. *Why:* a software GL (llvmpipe) passes every feature
/// test and then costs 6-40 ms per frame; the HUD would stutter instead of falling back.
#[derive(Default, Debug)]
struct Guard {
    ema: f32,
    over_since: Option<Instant>,
    /// Frames to ignore (after an upload, the first frame is slow without being a verdict).
    skip: u32,
    seeded: bool,
    last: Option<Instant>,
}

/// A pause between two frames longer than this (the HUD hidden, the window minimised) starts the
/// guard afresh: the verdict is about a running scene, not about the hitch before the pause.
const GUARD_GAP: Duration = Duration::from_millis(1000);

impl Guard {
    /// Feed one frame's cost; true when the limit has been exceeded for `hold`.
    fn feed(&mut self, ms: f32, now: Instant, limit_ms: f32, hold: Duration) -> bool {
        if self.last.is_some_and(|l| now.duration_since(l) > GUARD_GAP) {
            *self = Guard { skip: 3, ..Guard::default() };
        }
        self.last = Some(now);
        if self.skip > 0 {
            self.skip -= 1;
            return false;
        }
        self.ema = if self.seeded { self.ema * 0.9 + ms * 0.1 } else { ms };
        self.seeded = true;
        if self.ema > limit_ms {
            let since = *self.over_since.get_or_insert(now);
            now.duration_since(since) >= hold
        } else {
            self.over_since = None;
            false
        }
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn guard_trips_only_after_a_sustained_overrun() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut g = Guard::default();
        let hold = Duration::from_secs(2);
        // A fast scene never trips.
        for i in 0..200 {
            assert!(!g.feed(1.0, at(i * 16), 8.0, hold));
        }
        // A slow one trips two seconds after the average crossed the limit, not before.
        let mut tripped_at = None;
        for i in 200..600 {
            if g.feed(30.0, at(i * 16), 8.0, hold) {
                tripped_at = Some(i);
                break;
            }
        }
        let i = tripped_at.expect("trips");
        assert!(i * 16 - 200 * 16 >= 2000, "needs >= 2 s, tripped after {} ms", i * 16 - 200 * 16);
        // A single hitch among fast frames does not.
        let mut g = Guard::default();
        for i in 0..400 {
            let ms = if i == 100 { 60.0 } else { 1.0 };
            assert!(!g.feed(ms, at(i * 16), 8.0, hold));
        }
        // A long pause (the HUD hidden) forgets a slow stretch before it: no instant verdict after it.
        let mut g = Guard::default();
        for i in 0..100 {
            g.feed(30.0, at(i * 16), 8.0, hold);
        }
        assert!(g.ema > 8.0);
        assert!(!g.feed(30.0, at(100 * 16 + 60_000), 8.0, hold), "first frame after a minute's pause");
        assert!(g.over_since.is_none() || g.skip > 0);
        // Frames after an upload are skipped.
        let mut g = Guard { skip: 5, ..Default::default() };
        for i in 0..5 {
            assert!(!g.feed(500.0, at(i * 16), 8.0, hold));
        }
        assert_eq!(g.ema, 0.0);
    }

    #[test]
    fn as_bytes_is_little_endian_and_exact() {
        assert_eq!(as_bytes(&[1u16, 0x0203]), &[1, 0, 3, 2]);
        assert_eq!(as_bytes(&[1.0f32]).len(), 4);
        assert_eq!(as_bytes::<u32>(&[]).len(), 0);
    }
}
