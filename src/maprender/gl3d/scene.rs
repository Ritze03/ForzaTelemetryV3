//! [`Gl3d`]: the GL objects of one context and the two passes of a frame - render the scene
//! into the renderer's own FBO (colour + 24-bit depth), then composite that FBO into whatever
//! framebuffer the caller had bound. Every method takes the `glow::Context` explicitly (no
//! context is stored), which is also why [`Gl3d::destroy`] can be called with the context the
//! owner keeps.
//!
//! # State hygiene
//!
//! egui's callback leaves us in its state (its VAO and program, scissor = the clip rect, viewport =
//! the callback rect) and re-establishes its own after we return, *except* the framebuffer
//! binding. So [`Gl3d::render`] reads `FRAMEBUFFER_BINDING`, `VIEWPORT` and `SCISSOR_*` first and
//! puts them back before the composite; the framebuffer is restored to **what it was**
//! (`None` only when it was 0), never to a blind 0, because the Windows overlay renders into an
//! offscreen FBO of its own (`overlay/wgl.rs`) and `Painter::intermediate_fbo()` is always
//! `None`. Our textures live on units 1-3 (egui only uses 0), and unit 0 is active again on exit.

use std::num::NonZeroU32;
use std::time::Instant;

use egui_glow::glow::{self, HasContext};

use super::clipmap::{self, Clipmap, HeightTex};
use super::marker::{self, Marker3d, ModelGpu, Trail3d, TrailGpu};
use super::racemark;
use super::probe::{Caps, TEXTURE_MAX_ANISOTROPY, TIME_ELAPSED};
use super::roads::{self, RoadGpu};
use super::shaders::{self, compile, Common, Prog};
use super::Gl3dOptions;
use crate::maprender::cfg::{NavRouteCfg, OtherRoads, RaceCfg, ReliefCfg, RoadHeight, RoadsCfg, RaceFocusCfg, RouteStyle};
use crate::maprender::mesh3d::{RoadMesh, LIFT_M};
use crate::maprender::racesel::{RaceMark, RoadFocus};
use crate::maprender::style;
use crate::maprender::terrain::Terrain;
use crate::maprender::view::Camera;
use crate::minimap::MapCalibration;
use std::sync::Arc;

/// What a frame needs, in viewport px / design units as documented per field.
pub struct Frame<'a> {
    /// The camera with its relief; its `rect` is the callback rect (the GL viewport).
    pub cam: &'a Camera,
    /// Pixels per point of the callback.
    pub ppp: f32,
    /// The callback viewport in px.
    pub size: [i32; 2],
    /// egui's texture of the satellite image (`None` = the plain backing colour).
    pub map: Option<glow::Texture>,
    pub cal: MapCalibration,
    /// Size of the *original* map image, which `cal` is in.
    pub orig: [u32; 2],
    pub brightness: f32,
    pub saturation: f32,
    /// `ImageCfg::opacity` (the fade alpha is applied to everything at the composite).
    pub opacity: f32,
    pub mirror: bool,
    pub relief: ReliefCfg,
    /// `None` = no roads at all (`roads.on` is off, or no mesh yet).
    pub roads: Option<&'a RoadsCfg>,
    pub focus: Option<&'a RaceFocusCfg>,
    /// The race lines (D80 / D88): drawn when [`Gl3d::sync_race`] holds a mesh; the marks are
    /// posts built per frame.
    pub race: Option<RaceFrame<'a>>,
    /// The navigation route (phase L): drawn when [`Gl3d::sync_nav`] holds a mesh.
    pub nav: Option<NavFrame<'a>>,
    /// Size factor of strokes (HUD design -> screen).
    pub s: f32,
    /// Breadcrumb trails at their recorded heights (D77).
    pub trails: &'a [Trail3d],
    /// Wait for the GPU and read the timer query in this call (tests, perf numbers).
    pub sync_timing: bool,
}

/// The race lines of a frame (D88): the road settings for the Road style's width rule and casing,
/// the race settings (style, colour, line width and alpha, marks on) and the marks to draw.
#[derive(Clone, Copy)]
pub struct RaceFrame<'a> {
    pub roads: &'a RoadsCfg,
    pub cfg: &'a RaceCfg,
    pub marks: &'a [RaceMark],
}

/// The navigation route of a frame: the road settings for the width rule and casing, and the
/// route's own look (colour, width factor).
#[derive(Clone, Copy)]
pub struct NavFrame<'a> {
    pub roads: &'a RoadsCfg,
    pub cfg: &'a NavRouteCfg,
}

/// What one render did.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    pub triangles: usize,
    pub draws: usize,
    pub tiles_near: usize,
    pub tiles_far: usize,
    pub cpu_ms: f64,
    /// GPU time of a finished earlier frame (or of this one with `sync_timing`).
    pub gpu_ms: Option<f64>,
}

struct Fbo {
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    depth: glow::Renderbuffer,
    /// A second depth buffer of the same size, swapped in for the tunnel pass (D95): the tunnels
    /// are drawn over everything, so they need a depth of their own to sort among themselves.
    depth_tunnel: glow::Renderbuffer,
    size: [i32; 2],
}

#[derive(Clone, Copy)]
struct Saved {
    fbo: Option<glow::Framebuffer>,
    viewport: [i32; 4],
    scissor_on: bool,
    scissor: [i32; 4],
}

/// Which winding the road mesh's front faces have on screen. K1's `top_quad` is counter-clockwise
/// in a right-handed reading of (x, y, z); the world is left-handed (x east, z north), so on
/// screen the top faces are **clockwise** (chosen by looking, test `culling_matches_no_culling`).
const ROAD_FRONT_FACE: u32 = glow::CW;

pub struct Gl3d {
    pub caps: Caps,
    terrain: Prog,
    road: Prog,
    comp: Prog,
    marker: Prog,
    trail: Prog,
    /// The arrow and the sedan (`MarkerStyle` order), uploaded at the first marker.
    models: Option<[ModelGpu; 2]>,
    /// The streamed trail buffer, created at the first trail.
    trail_buf: Option<TrailGpu>,
    clip: Clipmap,
    pub heights: Option<HeightTex>,
    pub roads: Option<RoadGpu>,
    /// The race lines (D80 / D88): one mesh of every line drawn, built off-thread
    /// (`store::race_mesh`).
    pub race: Option<RoadGpu>,
    /// The navigation route (phase L): one small mesh, built by `Route3d::new` and re-uploaded
    /// when its `Arc` changes (a new route, or the next 150 m chunk of it).
    pub nav: Option<RoadGpu>,
    fbo: Option<Fbo>,
    empty_vao: glow::VertexArray,
    queries: Vec<glow::Query>,
    q_inflight: Vec<bool>,
    q_next: usize,
    /// The egui texture whose mip chain we have prepared (and its generation: see [`Gl3d::prepare_map`]).
    map_prepared: Option<glow::Texture>,
    cull: bool,
    /// [`Gl3d::destroy`] ran (the context is the owner's, so `Drop` cannot free anything itself).
    destroyed: bool,
}

impl Gl3d {
    /// Compile the programs and build the static clipmap buffers. ~5-15 ms. Heights and roads
    /// follow separately ([`Gl3d::set_terrain`], [`Gl3d::set_roads`]) so the caller can spread the
    /// uploads over frames.
    pub fn new(gl: &glow::Context, caps: Caps, opts: &Gl3dOptions) -> Result<Gl3d, String> {
        let (tvs, tfs) = (if opts.break_shader { "this is not glsl" } else { shaders::TERRAIN_VS }, shaders::TERRAIN_FS);
        let terrain = compile(gl, "terrain", tvs, tfs, Common::Yes, shaders::TERRAIN_UNIFORMS)?;
        let road = compile(gl, "road", shaders::ROAD_VS, shaders::ROAD_FS, Common::Yes, shaders::ROAD_UNIFORMS)?;
        let comp = compile(gl, "composite", shaders::COMP_VS, shaders::COMP_FS, Common::No, shaders::COMP_UNIFORMS)?;
        let marker = compile(gl, "marker", shaders::MARKER_VS, shaders::MARKER_FS, Common::Yes, shaders::MARKER_UNIFORMS)?;
        let trail = compile(gl, "trail", shaders::TRAIL_VS, shaders::TRAIL_FS, Common::Yes, shaders::TRAIL_UNIFORMS)?;
        let clip = Clipmap::new(gl)?;
        // SAFETY: plain GL object creation on the current context.
        let (empty_vao, queries) = unsafe {
            let v = gl.create_vertex_array()?;
            let q: Vec<glow::Query> = if caps.timer { (0..3).filter_map(|_| gl.create_query().ok()).collect() } else { Vec::new() };
            (v, q)
        };
        let n = queries.len();
        Ok(Gl3d { caps, terrain, road, comp, marker, trail, models: None, trail_buf: None, clip, heights: None, roads: None, race: None, nav: None, fbo: None, empty_vao, queries, q_inflight: vec![false; n], q_next: 0, map_prepared: None, cull: opts.cull, destroyed: false })
    }

    /// Upload the height raster (15 MB for the island, ~16 ms).
    pub fn set_terrain(&mut self, gl: &glow::Context, terrain: &Terrain) -> Result<(), String> {
        let h = HeightTex::upload(gl, &terrain.grid, terrain.rev, self.caps.max_texture)?;
        if let Some(old) = self.heights.replace(h) {
            old.destroy(gl);
        }
        Ok(())
    }

    /// Upload a road mesh (25 MB for the island, ~10 ms), replacing the previous one.
    pub fn set_roads(&mut self, gl: &glow::Context, mesh: Arc<RoadMesh>) -> Result<(), String> {
        let new = RoadGpu::upload(gl, mesh)?;
        if let Some(mut old) = self.roads.replace(new) {
            old.destroy(gl);
        }
        Ok(())
    }

    /// Make the in-race focus flags match `focus` (a new picked line / road rev only; a 0.2 ms
    /// loop plus a 485 KB `buffer_sub_data`).
    pub fn sync_focus(&mut self, gl: &glow::Context, focus: Option<&Arc<RoadFocus>>) {
        let Some(r) = self.roads.as_mut() else { return };
        let same = match (&r.rel_focus, focus) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        if same {
            return;
        }
        let flags = focus.map(|f| r.mesh.build_rel(f));
        r.set_rel(gl, flags.as_deref(), focus.cloned());
    }

    /// Does the GPU already hold exactly `mesh` as the race mesh (`None` = none)?
    pub fn race_is(&self, mesh: Option<&Arc<RoadMesh>>) -> bool {
        match (&self.race, mesh) {
            (None, None) => true,
            (Some(r), Some(m)) => Arc::ptr_eq(&r.mesh, m),
            _ => false,
        }
    }

    /// Make the race mesh the GPU holds `mesh` (D88: built off-thread by `store::race_mesh`, which
    /// serves the newest finished one): uploaded when the `Arc` changes, dropped on `None`.
    /// Returns whether it uploaded (a heavy step: the caller counts it).
    pub fn sync_race(&mut self, gl: &glow::Context, mesh: Option<&Arc<RoadMesh>>) -> Result<bool, String> {
        if self.race_is(mesh) {
            return Ok(false);
        }
        if let Some(mut old) = self.race.take() {
            old.destroy(gl);
        }
        match mesh.filter(|m| !m.samples.is_empty()) {
            Some(m) => {
                self.race = Some(RoadGpu::upload(gl, m.clone())?);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Does the GPU already hold exactly `mesh` as the route mesh (`None` = none)?
    pub fn nav_is(&self, mesh: Option<&Arc<RoadMesh>>) -> bool {
        match (&self.nav, mesh) {
            (None, None) => true,
            (Some(r), Some(m)) => Arc::ptr_eq(&r.mesh, m),
            _ => false,
        }
    }

    /// Make the route mesh the GPU holds `mesh` (`Route3d::new` builds it once per route chunk,
    /// not per frame): uploaded when the `Arc` changes, dropped on `None`. Returns whether it
    /// uploaded.
    pub fn sync_nav(&mut self, gl: &glow::Context, mesh: Option<&Arc<RoadMesh>>) -> Result<bool, String> {
        if self.nav_is(mesh) {
            return Ok(false);
        }
        if let Some(mut old) = self.nav.take() {
            old.destroy(gl);
        }
        match mesh.filter(|m| !m.samples.is_empty()) {
            Some(m) => {
                self.nav = Some(RoadGpu::upload(gl, m.clone())?);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn ensure_fbo(&mut self, gl: &glow::Context, w: i32, h: i32) -> Result<(), String> {
        let (w, h) = (w.max(1), h.max(1));
        if self.fbo.as_ref().is_some_and(|f| f.size[0] >= w && f.size[1] >= h) {
            return Ok(());
        }
        // Grow only: one FBO serves every callback of a frame (they run one after the other).
        let (cw, ch) = self.fbo.as_ref().map_or((0, 0), |f| (f.size[0], f.size[1]));
        let (w, h) = (w.max(cw), h.max(ch));
        let max = self.caps.max_texture.min(self.caps.max_renderbuffer);
        if w > max || h > max {
            return Err(format!("the view ({w} x {h} px) is larger than the GPU's limit ({max} px)"));
        }
        if let Some(old) = self.fbo.take() {
            // SAFETY: deleting objects this struct created.
            unsafe {
                gl.delete_framebuffer(old.fbo);
                gl.delete_texture(old.tex);
                gl.delete_renderbuffer(old.depth);
                gl.delete_renderbuffer(old.depth_tunnel);
            }
        }
        // SAFETY: plain GL object creation on the current context; the caller restores the
        // framebuffer binding afterwards (`render` does).
        unsafe {
            let tex = gl.create_texture()?;
            gl.active_texture(glow::TEXTURE3);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            let depth = gl.create_renderbuffer()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH24_STENCIL8, w, h);
            let depth_tunnel = gl.create_renderbuffer()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth_tunnel));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH24_STENCIL8, w, h);
            let fbo = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_STENCIL_ATTACHMENT, glow::RENDERBUFFER, Some(depth));
            gl.active_texture(glow::TEXTURE0);
            let st = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if st != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(fbo);
                gl.delete_texture(tex);
                gl.delete_renderbuffer(depth);
                gl.delete_renderbuffer(depth_tunnel);
                return Err(format!("the scene framebuffer is incomplete (0x{st:X})"));
            }
            self.fbo = Some(Fbo { fbo, tex, depth, depth_tunnel, size: [w, h] });
        }
        Ok(())
    }

    fn save(gl: &glow::Context) -> Saved {
        // SAFETY: plain state queries.
        unsafe {
            let id = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
            let mut viewport = [0i32; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
            let mut scissor = [0i32; 4];
            gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut scissor);
            Saved {
                // 0 is the window's framebuffer: `None`. Anything else is an FBO of the owner's.
                fbo: NonZeroU32::new(id as u32).map(glow::NativeFramebuffer),
                viewport,
                scissor_on: gl.is_enabled(glow::SCISSOR_TEST),
                scissor,
            }
        }
    }

    fn restore(gl: &glow::Context, s: &Saved) {
        // SAFETY: plain state changes.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, s.fbo);
            gl.viewport(s.viewport[0], s.viewport[1], s.viewport[2], s.viewport[3]);
            gl.scissor(s.scissor[0], s.scissor[1], s.scissor[2], s.scissor[3]);
            if s.scissor_on {
                gl.enable(glow::SCISSOR_TEST);
            } else {
                gl.disable(glow::SCISSOR_TEST);
            }
            gl.depth_mask(true);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.front_face(glow::CCW);
            gl.active_texture(glow::TEXTURE0);
            gl.use_program(None);
            gl.bind_vertex_array(None);
        }
    }

    /// Mipmaps + anisotropy on the texture egui uploaded (the Dashboard's has none). egui sets
    /// `MIN_FILTER` on every full upload, so it is read each frame (a cached client-side query
    /// in the drivers): a texture that is not mipmapped (new, or re-uploaded and reset) gets its
    /// chain built. A texture egui already mipmapped (the HUD's) only gets the anisotropy.
    fn prepare_map(&mut self, gl: &glow::Context, t: glow::Texture) {
        // SAFETY: plain texture state on the current context.
        unsafe {
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            let f = gl.get_tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER) as u32;
            let mipped = matches!(f, glow::NEAREST_MIPMAP_NEAREST | glow::LINEAR_MIPMAP_NEAREST | glow::NEAREST_MIPMAP_LINEAR | glow::LINEAR_MIPMAP_LINEAR);
            if !mipped {
                gl.generate_mipmap(glow::TEXTURE_2D);
            }
            if f != glow::LINEAR_MIPMAP_LINEAR || self.map_prepared != Some(t) {
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
                if let Some(a) = self.caps.aniso {
                    gl.tex_parameter_f32(glow::TEXTURE_2D, TEXTURE_MAX_ANISOTROPY, a);
                }
            }
            gl.active_texture(glow::TEXTURE0);
        }
        self.map_prepared = Some(t);
    }

    fn camera_uniforms(&self, gl: &glow::Context, p: &Prog, f: &Frame, h: &HeightTex) {
        Self::cam_uniforms(gl, p, f.cam, f.ppp, h);
    }

    fn cam_uniforms(gl: &glow::Context, p: &Prog, cam: &Camera, ppp: f32, h: &HeightTex) {
        let (n, far) = cam.near_far(ppp);
        let a1 = (far + n) / (far - n);
        let car = cam.car_exag();
        // SAFETY: uniform uploads on the current program.
        unsafe {
            gl.uniform_matrix_4_f32_slice(p.u("uVP"), false, &cam.view_proj_rel(ppp));
            gl.uniform_3_f32(p.u("uCar"), car[0], car[1], car[2]);
            gl.uniform_1_f32(p.u("uExag"), cam.exag());
            gl.uniform_3_f32(p.u("uCam"), cam.view.scale * ppp, cam.focal * ppp, a1);
            gl.uniform_1_i32(p.u("uH"), 2);
            gl.uniform_2_i32(p.u("uHSize"), h.size[0], h.size[1]);
            gl.uniform_3_f32(p.u("uHGeo"), h.geo[0], h.geo[1], h.geo[2]);
        }
    }

    /// Render the scene into the own FBO and put the caller's framebuffer, viewport and scissor
    /// back. Needs [`Gl3d::set_terrain`] first.
    pub fn render(&mut self, gl: &glow::Context, f: &Frame) -> Result<RenderStats, String> {
        let t0 = Instant::now();
        let Some(h) = self.heights.as_ref().map(|h| (h.tex, h.size, h.geo)) else { return Err("no terrain uploaded".into()) };
        let saved = Self::save(gl);
        let r = self.render_inner(gl, f, &h);
        Self::restore(gl, &saved);
        let mut st = r?;
        st.cpu_ms = t0.elapsed().as_secs_f64() * 1e3;
        Ok(st)
    }

    fn render_inner(&mut self, gl: &glow::Context, f: &Frame, h: &(glow::Texture, [i32; 2], [f32; 3])) -> Result<RenderStats, String> {
        let (w, hh) = (f.size[0], f.size[1]);
        self.ensure_fbo(gl, w, hh)?;
        let fbo = self.fbo.as_ref().map(|b| b.fbo).ok_or("no framebuffer")?;
        if let Some(t) = f.map {
            self.prepare_map(gl, t);
        }
        let mut st = RenderStats::default();
        let heights = HeightTex { tex: h.0, size: h.1, geo: h.2, rev: 0 };
        // The timer query of an earlier frame, if it finished.
        let mut gpu_ms = None;
        let q = if self.queries.is_empty() { None } else { Some(self.q_next) };
        // SAFETY: GL state and draw calls on the current context with objects this struct owns.
        unsafe {
            for (i, q) in self.queries.iter().enumerate() {
                if self.q_inflight[i] && gl.get_query_parameter_u32(*q, glow::QUERY_RESULT_AVAILABLE) != 0 {
                    gpu_ms = Some(gl.get_query_parameter_u32(*q, glow::QUERY_RESULT) as f64 / 1e6);
                    self.q_inflight[i] = false;
                }
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.viewport(0, 0, w, hh);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::CULL_FACE);
            gl.color_mask(true, true, true, true);
            gl.depth_mask(true);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            let q = q.filter(|&i| !self.q_inflight[i]);
            if let Some(i) = q {
                gl.begin_query(TIME_ELAPSED, self.queries[i]);
            }

            // Heights on unit 2, the satellite on unit 1 (egui keeps unit 0).
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(h.0));
            gl.active_texture(glow::TEXTURE1);
            if let Some(t) = f.map {
                gl.bind_texture(glow::TEXTURE_2D, Some(t));
            }

            // ── terrain
            gl.disable(glow::BLEND);
            let p = &self.terrain;
            gl.use_program(Some(p.p));
            self.camera_uniforms(gl, p, f, &heights);
            gl.uniform_1_i32(p.u("uMap"), 1);
            gl.uniform_1_f32(p.u("uHasMap"), f.map.is_some() as u8 as f32);
            let c = f.cal;
            gl.uniform_4_f32(p.u("uMapGeo"), c.origin_x, c.origin_z, c.px_per_m / f.orig[0].max(1) as f32, c.px_per_m / f.orig[1].max(1) as f32);
            gl.uniform_3_f32(p.u("uLook"), f.brightness, f.saturation, f.opacity);
            let bg = style::MAP_BACKING;
            gl.uniform_3_f32(p.u("uNoMap"), bg.r() as f32 / 255.0, bg.g() as f32 / 255.0, bg.b() as f32 / 255.0);
            gl.uniform_3_f32(p.u("uSun"), -0.5, 1.0, 0.35);
            gl.uniform_1_f32(p.u("uShade"), f.relief.shading);
            gl.uniform_1_f32(p.u("uMirror"), f.mirror as u8 as f32);
            gl.uniform_1_i32(p.u("uM"), clipmap::M);
            gl.bind_vertex_array(Some(self.clip.vao));
            let (car_x, car_z) = (f.cam.view.car_x as f64, f.cam.view.car_z as f64);
            // Tests only: pretend the terrain LOD was last placed for another car position (the
            // frame before a level snap), under this frame's camera.
            #[cfg(test)]
            let (car_x, car_z) = clipmap::LOD_CAR.get().map_or((car_x, car_z), |c| (c[0], c[1]));
            let car_px = [(car_x - h.2[0] as f64) / h.2[2] as f64 - 0.5, (h.2[1] as f64 - car_z) / h.2[2] as f64 - 0.5];
            gl.uniform_2_f32(p.u("uCarPx"), car_px[0] as f32, car_px[1] as f32);
            for lv in clipmap::levels(car_px) {
                gl.uniform_2_i32(p.u("uBase"), lv.base[0] as i32, lv.base[1] as i32);
                gl.uniform_1_i32(p.u("uStride"), lv.stride as i32);
                gl.uniform_1_f32(p.u("uHasCoarser"), lv.has_coarser as u8 as f32);
                let (ibo, n) = self.clip.ibo[lv.variant];
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
                gl.draw_elements(glow::TRIANGLES, n, glow::UNSIGNED_INT, 0);
                st.triangles += n as usize / 3;
                st.draws += 1;
            }

            // ── roads
            // D82: "race road only" draws nothing of the road mesh while the race road is there.
            let race_only = f.race.is_some_and(|r| r.cfg.route == RouteStyle::Road) && self.race.is_some() && f.focus.is_some_and(|c| c.other_roads == OtherRoads::RaceOnly);
            if let (Some(rc), Some(r), false) = (f.roads, self.roads.as_ref(), race_only) {
                let table = roads::style_table(rc, f.focus, f.cam.view.scale, f.s, f.ppp);                self.draw_roads(gl, f, &heights, &table, r, None, &mut st);
            }
            // ── the race lines (D80 / D88): after every road, over them where nothing is in front
            // of them: their open stretches are depth-tested like the roads' (hidden behind hills
            // and under decks), only their tunnel stretches go through (`draw_roads`, same as the
            // roads'); then the start / finish posts, depth-tested the same way
            if let Some(rf) = f.race {
                if let Some(r) = self.race.as_ref() {
                    let table = roads::race_table(rf.roads, rf.cfg, f.cam.view.scale, f.s, f.ppp);
                    // a thin line has no deck to speak of
                    let deck = if rf.cfg.route == RouteStyle::Road { f.relief.deck_m } else { 0.0 };
                    self.draw_roads(gl, f, &heights, &table, r, Some((RACE_BIAS, deck)), &mut st);
                }
                if rf.cfg.marks && !rf.marks.is_empty() {
                    self.draw_race_marks(gl, f, &heights, rf.marks, &mut st)?;
                }
            }
            // ── the navigation route (phase L, D84): over the roads and the race lines, depth-tested
            // like the race road (hills and decks in front hide it, only tunnel stretches show
            // through); "race road only" hides roads, not this
            if let (Some(nf), Some(r)) = (f.nav, self.nav.as_ref()) {
                let table = roads::nav_table(nf.roads, nf.cfg, f.cam.view.scale, f.s, f.ppp);
                self.draw_roads(gl, f, &heights, &table, r, Some((NAV_BIAS, f.relief.deck_m)), &mut st);
            }
            // ── trails (D77; the own car is a pass of its own, `render_marker`)
            if !f.trails.is_empty() {
                self.draw_trails(gl, f, &heights, &mut st)?;
            }

            if let Some(i) = q {
                gl.end_query(TIME_ELAPSED);
                self.q_inflight[i] = true;
                self.q_next = (i + 1) % self.queries.len();
            }
            if f.sync_timing {
                gl.finish();
                if let Some(i) = q {
                    gpu_ms = Some(gl.get_query_parameter_u32(self.queries[i], glow::QUERY_RESULT) as f64 / 1e6);
                    self.q_inflight[i] = false;
                }
            }
            gl.bind_vertex_array(None);
        }
        st.gpu_ms = gpu_ms;
        Ok(st)
    }

    /// # Safety
    /// The scene FBO is bound with depth testing on; a current context.
    /// `race` = the race lines' pass (D80 / D88): their depth bias base (casing; the fill a step
    /// nearer) and deck thickness (m), node heights always (the line's own heights), no
    /// per-rank steps.
    unsafe fn draw_roads(&self, gl: &glow::Context, f: &Frame, heights: &HeightTex, table: &roads::StyleTable, r: &RoadGpu, race: Option<(f32, f32)>, st: &mut RenderStats) {
        // SAFETY: the caller's contract.
        unsafe {
            let plan = roads::plan(&r.mesh, f.cam, f.ppp, table.rw[2].max(table.rw[1]));
            st.tiles_near = plan.tiles_near;
            st.tiles_far = plan.tiles_far;
            if plan.normal.is_empty() && plan.tunnel.is_empty() {
                return;
            }
            gl.enable(glow::BLEND);
            gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
            gl.blend_func_separate(glow::ONE, glow::ONE_MINUS_SRC_ALPHA, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            let p = &self.road;
            gl.use_program(Some(p.p));
            self.camera_uniforms(gl, p, f, heights);
            gl.uniform_1_f32(p.u("uMode"), (race.is_some() || f.relief.road_height == RoadHeight::Nodes) as u8 as f32);
            gl.uniform_1_f32(p.u("uThick"), race.map_or(f.relief.deck_m, |r| r.1));
            gl.uniform_1_f32(p.u("uLift"), LIFT_M);
            let flat = |v: &[[f32; 4]; roads::SLOTS]| -> Vec<f32> { v.iter().flatten().copied().collect() };
            gl.uniform_4_f32_slice(p.u("uSlotA"), &flat(&table.a));
            gl.uniform_4_f32_slice(p.u("uSlotB"), &flat(&table.b));
            gl.uniform_4_f32_slice(p.u("uSlotC"), &flat(&table.c));
            gl.uniform_4_f32(p.u("uRW"), table.rw[0], table.rw[1], table.rw[2], table.rw[3]);
            gl.uniform_4_f32(p.u("uFocus"), table.focus[0], table.focus[1], table.focus[2], table.focus[3]);
            gl.uniform_3_f32(p.u("uMuteRgb"), table.mute_rgb[0], table.mute_rgb[1], table.mute_rgb[2]);
            gl.uniform_1_f32(p.u("uCasingAlpha"), table.casing_alpha);
            gl.bind_vertex_array(Some(r.vao));
            if self.cull {
                gl.enable(glow::CULL_FACE);
                gl.cull_face(glow::BACK);
                gl.front_face(ROAD_FRONT_FACE);
            }
            let draw = |d: &roads::Draw, st: &mut RenderStats| {
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(if d.far { r.ibo_far } else { r.ibo_near }));
                gl.draw_elements(glow::TRIANGLES, d.count as i32, glow::UNSIGNED_INT, d.first as i32 * 4);
                st.triangles += d.count as usize / 3;
                st.draws += 1;
            };
            // Two passes (D81): every casing, then every fill over them (`shaders::ROAD_VS`).
            // (pass 2 = the translucent roads' one ribbon, at the fill's depth step)
            let pass = |which: f32| {
                gl.uniform_1_f32(p.u("uPass"), which);
                let (base, rank) = match race {
                    Some((b, _)) => (if which == 0.0 { b } else { b + RACE_BIAS_FILL }, 0.0),
                    None if which == 0.0 => (BIAS_BASE, BIAS_RANK_CASING),
                    None => (BIAS_BASE + BIAS_FILL, BIAS_RANK),
                };
                gl.uniform_2_f32(p.u("uBias"), base, rank);
            };
            // The translucent roads (D95), blended once where they are nearest: their depth first
            // (colour masked), then their colour where the depth is that nearest one (`LEQUAL`,
            // the same program with the same uniforms gives the same depth twice). Two surfaces
            // at the very same depth (two coplanar roads at a junction; 24 bits do not tell
            // them apart) would both pass, so the first to pass also marks the pixel in the
            // stencil, and no other is drawn there. The stencil is this pass's own: cleared
            // before it (the previous road mesh's pass left its marks).
            let translucent = |draws: &[roads::Draw], st: &mut RenderStats| {
                pass(2.0);
                gl.color_mask(false, false, false, false);
                gl.depth_mask(true);
                for d in draws {
                    draw(d, st);
                }
                gl.color_mask(true, true, true, true);
                gl.stencil_mask(0xFF);
                gl.clear_stencil(0);
                gl.clear(glow::STENCIL_BUFFER_BIT);
                gl.enable(glow::STENCIL_TEST);
                gl.stencil_func(glow::EQUAL, 0, 0xFF);
                gl.stencil_op(glow::KEEP, glow::KEEP, glow::INCR);
                for d in draws {
                    draw(d, st);
                }
                gl.disable(glow::STENCIL_TEST);
            };
            // (the roads' tunnel pass before a race road pass left the depth test off)
            gl.enable(glow::DEPTH_TEST);
            for which in [0.0, 1.0] {
                pass(which);
                for d in &plan.normal {
                    draw(d, st);
                }
            }
            if table.translucent(false) {
                translucent(&plan.normal, st);
            }
            // Tunnels are underground: drawn last, over everything. Opaque ones without the depth
            // test, in draw order, as ever; with a translucent road among them, in a depth buffer
            // of their own (cleared, so still over everything) so that their overlaps blend once.
            if !plan.tunnel.is_empty() {
                let own_depth = table.translucent(true).then(|| self.fbo.as_ref().map(|b| b.depth_tunnel)).flatten();
                if let Some(d) = own_depth {
                    gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_STENCIL_ATTACHMENT, glow::RENDERBUFFER, Some(d));
                    gl.depth_mask(true);
                    gl.clear_depth_f32(1.0);
                    gl.clear(glow::DEPTH_BUFFER_BIT);
                } else {
                    // (no own depth: this is the frame's, and the test off means no depth writes)
                    gl.disable(glow::DEPTH_TEST);
                }
                for which in [0.0, 1.0] {
                    pass(which);
                    for d in &plan.tunnel {
                        draw(d, st);
                    }
                }
                if own_depth.is_some() {
                    translucent(&plan.tunnel, st);
                    if let Some(b) = self.fbo.as_ref() {
                        gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_STENCIL_ATTACHMENT, glow::RENDERBUFFER, Some(b.depth));
                    }
                }
            }
            gl.disable(glow::CULL_FACE);
        }
    }

    /// The race lines' start / finish posts (D88, `racemark`): ribbons of the trail program,
    /// depth-tested against the terrain, the roads and the race mesh, so a hill hides them (no
    /// ghost pass: unlike a trail they are not "seen through"); the ones in a tunnel go last
    /// without the depth test, like the road tunnels.
    ///
    /// # Safety
    /// The scene FBO is bound with depth testing on; a current context.
    unsafe fn draw_race_marks(&mut self, gl: &glow::Context, f: &Frame, heights: &HeightTex, marks: &[RaceMark], st: &mut RenderStats) -> Result<(), String> {
        let Some(relief) = f.cam.relief.as_ref() else { return Ok(()) };
        // a post's real height: POST_H_PT points at the car's distance
        let height_m = racemark::POST_H_PT * f.s / f.cam.view.scale.max(1e-6);
        if self.trail_buf.is_none() {
            self.trail_buf = Some(TrailGpu::new(gl)?);
        }
        let tb = self.trail_buf.as_ref().expect("created above");
        // SAFETY: the caller's contract.
        unsafe {
            let p = &self.trail;
            gl.use_program(Some(p.p));
            self.camera_uniforms(gl, p, f, heights);
            gl.uniform_2_f32(p.u("uVp"), f.size[0] as f32, f.size[1] as f32);
            gl.uniform_1_f32(p.u("uAlpha"), 1.0);
            gl.enable(glow::BLEND);
            gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
            gl.blend_func_separate(glow::ONE, glow::ONE_MINUS_SRC_ALPHA, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.disable(glow::CULL_FACE);
            gl.depth_mask(false);
            gl.depth_func(glow::LEQUAL);
            gl.bind_vertex_array(Some(tb.vao));
            for tunnel in [false, true] {
                let batches = racemark::batches(marks, &relief.terrain, height_m, relief.exag, tunnel);
                if batches.is_empty() {
                    continue;
                }
                if tunnel {
                    gl.disable(glow::DEPTH_TEST);
                } else {
                    gl.enable(glow::DEPTH_TEST);
                }
                for b in &batches {
                    let (v, _) = marker::trail_vertices(std::slice::from_ref(&b.trail));
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(tb.vbo));
                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, super::as_bytes(&v), glow::STREAM_DRAW);
                    gl.uniform_2_f32(p.u("uW"), b.width_pt * f.s * f.ppp, RACE_BIAS_MARK);
                    let c = b.trail.colour;
                    gl.uniform_3_f32(p.u("uColor"), c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
                    let n = (v.len() / marker::TRAIL_FLOATS) as i32;
                    gl.draw_arrays(glow::TRIANGLES, 0, n);
                    st.triangles += n as usize / 3;
                    st.draws += 1;
                }
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_mask(true);
            gl.bind_vertex_array(None);
        }
        Ok(())
    }

    /// The trail ribbons: depth-tested against the terrain and roads at full strength, then the
    /// parts behind them (through a tunnel, behind a hill) again with the test inverted at
    /// [`marker::GHOST`] strength, so a trail into a tunnel stays visible like the tunnel.
    ///
    /// # Safety
    /// The scene FBO is bound with depth testing on; a current context.
    unsafe fn draw_trails(&mut self, gl: &glow::Context, f: &Frame, heights: &HeightTex, st: &mut RenderStats) -> Result<(), String> {
        let (v, ranges) = marker::trail_vertices(f.trails);
        if v.is_empty() {
            return Ok(());
        }
        if self.trail_buf.is_none() {
            self.trail_buf = Some(TrailGpu::new(gl)?);
        }
        let tb = self.trail_buf.as_ref().expect("created above");
        // SAFETY: the caller's contract.
        unsafe {
            gl.bind_vertex_array(Some(tb.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(tb.vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, super::as_bytes(&v), glow::STREAM_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            let p = &self.trail;
            gl.use_program(Some(p.p));
            self.camera_uniforms(gl, p, f, heights);
            gl.uniform_2_f32(p.u("uW"), marker::TRAIL_PT * f.s * f.ppp, BIAS_BASE + BIAS_RANK * 12.0);
            gl.uniform_2_f32(p.u("uVp"), f.size[0] as f32, f.size[1] as f32);
            gl.enable(glow::BLEND);
            gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
            gl.blend_func_separate(glow::ONE, glow::ONE_MINUS_SRC_ALPHA, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_mask(false);
            for (func, strength) in [(glow::LEQUAL, 1.0), (glow::GREATER, marker::GHOST)] {
                gl.depth_func(func);
                gl.uniform_1_f32(p.u("uAlpha"), strength);
                for (t, &(first, count)) in f.trails.iter().zip(&ranges) {
                    if count == 0 {
                        continue;
                    }
                    let c = t.colour;
                    gl.uniform_3_f32(p.u("uColor"), c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
                    gl.draw_arrays(glow::TRIANGLES, first, count);
                    st.triangles += count as usize / 3;
                    st.draws += 1;
                }
            }
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
            gl.bind_vertex_array(None);
        }
        Ok(())
    }

    /// The car markers (the own car D77 / D78, the co-op teammates D89) into the own FBO, alone
    /// (transparent elsewhere), for a composite of its own *after* the egui vectors over the scene
    /// (POIs, race lines), so they are on top of them like the flat arrow was. `markers` are drawn
    /// in order, **the later one on top** (the call site puts the own car last): each gets a depth
    /// buffer of its own (cleared in between), so it is always whole wherever it is (a tunnel,
    /// under a bridge) and correctly self-occluded. A dark hull (pushed out along the smoothed
    /// normals, no depth) first gives each the flat arrow's outline. One FBO clear, one composite,
    /// whatever the number of markers: an extra marker costs 2 draws and a depth clear. Puts the
    /// caller's framebuffer, viewport and scissor back like [`Gl3d::render`]. Returns the triangles
    /// drawn and the draw calls made.
    pub fn render_markers(&mut self, gl: &glow::Context, cam: &Camera, ppp: f32, size: [i32; 2], s: f32, markers: &[Marker3d]) -> Result<(usize, usize), String> {
        let Some(h) = self.heights.as_ref().map(|h| HeightTex { tex: h.tex, size: h.size, geo: h.geo, rev: 0 }) else { return Err("no terrain uploaded".into()) };
        let saved = Self::save(gl);
        let r = self.render_markers_inner(gl, cam, ppp, size, s, markers, &h);
        Self::restore(gl, &saved);
        r
    }

    #[allow(clippy::too_many_arguments)]
    fn render_markers_inner(&mut self, gl: &glow::Context, cam: &Camera, ppp: f32, size: [i32; 2], s: f32, markers: &[Marker3d], heights: &HeightTex) -> Result<(usize, usize), String> {
        self.ensure_fbo(gl, size[0], size[1])?;
        let fbo = self.fbo.as_ref().map(|b| b.fbo).ok_or("no framebuffer")?;
        if self.models.is_none() {
            self.models = Some([ModelGpu::upload(gl, &marker::arrow_model())?, ModelGpu::upload(gl, &marker::sedan_model())?]);
        }
        let models = self.models.as_ref().expect("uploaded above");
        let (mut tris, mut draws) = (0usize, 0usize);
        // SAFETY: GL state and draws on the current context with objects this struct owns.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.viewport(0, 0, size[0], size[1]);
            gl.disable(glow::SCISSOR_TEST);
            gl.color_mask(true, true, true, true);
            gl.depth_mask(true);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            let p = &self.marker;
            gl.use_program(Some(p.p));
            Self::cam_uniforms(gl, p, cam, ppp, heights);
            gl.uniform_1_f32(p.u("uAlpha"), 1.0);
            gl.enable(glow::BLEND);
            gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
            gl.blend_func_separate(glow::ONE, glow::ONE_MINUS_SRC_ALPHA, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.disable(glow::CULL_FACE);
            for (i, m) in markers.iter().enumerate() {
                let g = &models[match m.kind {
                    crate::maprender::cfg::MarkerStyle::Arrow => 0,
                    crate::maprender::cfg::MarkerStyle::Sedan => 1,
                }];
                // Points per metre at the marker: the camera's scale times the perspective there.
                let k_persp = cam.project3(m.pos[0], m.pos[1], m.pos[2]).map_or(1.0, |(_, cz)| (cam.focal / cz).clamp(0.05, 4.0));
                let ppm = cam.view.scale * k_persp;
                let k = marker::model_scale(m.kind, ppm, s);
                let grow = marker::OUTLINE_PT * s.max(0.1) / ppm.max(1e-6);
                let (sy, cy) = m.yaw.sin_cos();
                let c = m.colour;
                gl.uniform_3_f32(p.u("uPos"), m.pos[0], m.pos[1] - marker::GROUND_BELOW_M, m.pos[2]);
                gl.uniform_2_f32(p.u("uYaw"), sy, cy);
                gl.uniform_3_f32(p.u("uColor"), c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
                gl.bind_vertex_array(Some(g.vao));
                // The outline hull: no depth at all.
                gl.disable(glow::DEPTH_TEST);
                gl.uniform_2_f32(p.u("uK"), k, grow);
                gl.uniform_1_f32(p.u("uHull"), 1.0);
                gl.draw_arrays(glow::TRIANGLES, 0, g.count);
                // The model over it, depth-tested against itself only (a fresh depth buffer per
                // marker: the later marker is on top of the earlier ones, hull and all).
                if i > 0 {
                    gl.clear(glow::DEPTH_BUFFER_BIT);
                }
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LESS);
                gl.uniform_2_f32(p.u("uK"), k, 0.0);
                gl.uniform_1_f32(p.u("uHull"), 0.0);
                gl.draw_arrays(glow::TRIANGLES, 0, g.count);
                gl.depth_func(glow::LEQUAL);
                tris += g.count as usize / 3 * 2;
                draws += 2;
            }
            gl.bind_vertex_array(None);
        }
        Ok((tris, draws))
    }

    /// Draw the scene FBO into the framebuffer [`Gl3d::render`] restored: the viewport and scissor
    /// are the caller's (egui's callback rect and clip rect). `size` = the callback viewport in
    /// px, `radius_px` = the rounded-corner mask (the HUD pill, 0 = square), `alpha` = the fade.
    pub fn composite(&mut self, gl: &glow::Context, size: [i32; 2], radius_px: f32, alpha: f32) -> Result<(), String> {
        let Some(fb) = self.fbo.as_ref() else { return Err("no scene to composite".into()) };
        let p = &self.comp;
        // SAFETY: GL state and one draw on the current context.
        unsafe {
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::BLEND);
            gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
            // egui's own blend (premultiplied; destination alpha kept right for the layer surface).
            gl.blend_func_separate(glow::ONE, glow::ONE_MINUS_SRC_ALPHA, glow::ONE_MINUS_DST_ALPHA, glow::ONE);
            gl.use_program(Some(p.p));
            gl.active_texture(glow::TEXTURE3);
            gl.bind_texture(glow::TEXTURE_2D, Some(fb.tex));
            gl.uniform_1_i32(p.u("uTex"), 3);
            gl.uniform_2_f32(p.u("uUv"), size[0] as f32 / fb.size[0] as f32, size[1] as f32 / fb.size[1] as f32);
            gl.uniform_2_f32(p.u("uSizePx"), size[0] as f32, size[1] as f32);
            gl.uniform_1_f32(p.u("uRadius"), radius_px);
            gl.uniform_1_f32(p.u("uAlpha"), alpha);
            gl.bind_vertex_array(Some(self.empty_vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.active_texture(glow::TEXTURE0);
        }
        Ok(())
    }

    /// Free every GL object. The context must be current (the owner's drop order: before the
    /// egui painter's own `destroy`, before the context goes).
    pub fn destroy(&mut self, gl: &glow::Context) {
        self.destroyed = true;
        // SAFETY: deleting objects this struct created, on their context.
        unsafe {
            for p in [&self.terrain, &self.road, &self.comp, &self.marker, &self.trail] {
                gl.delete_program(p.p);
            }
            gl.delete_vertex_array(self.empty_vao);
            for q in self.queries.drain(..) {
                gl.delete_query(q);
            }
            if let Some(f) = self.fbo.take() {
                gl.delete_framebuffer(f.fbo);
                gl.delete_texture(f.tex);
                gl.delete_renderbuffer(f.depth);
                gl.delete_renderbuffer(f.depth_tunnel);
            }
        }
        self.clip.destroy(gl);
        for m in self.models.take().into_iter().flatten() {
            m.destroy(gl);
        }
        if let Some(t) = self.trail_buf.take() {
            t.destroy(gl);
        }
        if let Some(h) = self.heights.take() {
            h.destroy(gl);
        }
        if let Some(mut r) = self.roads.take() {
            r.destroy(gl);
        }
        if let Some(mut r) = self.race.take() {
            r.destroy(gl);
        }
        if let Some(mut r) = self.nav.take() {
            r.destroy(gl);
        }
    }
}

impl Drop for Gl3d {
    /// Nothing can be freed here (no context): an owner that lets the renderer go without
    /// `Gl3dHandle::destroy` leaks its GL objects until the context dies. Say so, so a wrong drop
    /// order shows up in the logs instead of as a slow VRAM leak.
    fn drop(&mut self) {
        if !self.destroyed && !std::thread::panicking() {
            eprintln!("gl3d: renderer dropped without destroy(): its GL objects leak until the context is gone");
        }
    }
}

/// Depth bias of roads toward the eye (fractions of the camera depth): the base keeps them above
/// the coarse terrain levels (cells of ~distance / 32 sit above or below the exact surface the
/// roads follow), the per-rank step orders overlapping types (highway over road over trail).
const BIAS_BASE: f32 = 0.002;
const BIAS_RANK: f32 = 0.0002;
/// The casing pass (D81) ranks its types with a smaller step, and the fill pass starts this much
/// nearer: every fill is over every casing (9 ranks x 0.00005 < 0.0005), and the topmost fill
/// (0.002 + 0.0005 + 8 x 0.0002 = 0.0041) stays under the trails (`BIAS_RANK * 12` = 0.0044).
const BIAS_RANK_CASING: f32 = 0.00005;
const BIAS_FILL: f32 = 0.0005;
/// The race road (D80) lies on the road it runs on (its heights are the driving line's, within a
/// metre of the road's nodes): its casing is biased nearer than every road fill (top 0.0041), its
/// fill a step more, both under the trails (0.0044). So it reads as one road over the other one,
/// with no z-fighting.
const RACE_BIAS: f32 = 0.0042;
const RACE_BIAS_FILL: f32 = 0.00008;
/// The navigation route (phase L) lies over the race road and the roads, under the trails (0.0044):
/// its casing a step nearer than the race fill (0.00428), its fill (+ `RACE_BIAS_FILL`) at 0.00438.
const NAV_BIAS: f32 = 0.0043;
/// The start / finish posts stand on the race road: a little nearer than its fill.
const RACE_BIAS_MARK: f32 = 0.0045;
