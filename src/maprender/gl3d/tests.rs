//! Headless tests of the 3D renderer: the real `Gl3d` inside a real `egui_glow::Painter` pass on
//! an EGL device without a window (the same code path as `overlay/render.rs::paint`).
//!
//! The GL tests are `#[ignore]`d (they need an EGL device; they write PNGs). Run them one at a
//! time, so the environment overrides do not race:
//!
//! ```text
//! GL3D_PNG_DIR=/some/dir cargo test gl3d -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! * `gl3d_default_gl` - the repo's own context recipe (desktop Core on Mesa).
//! * `gl3d_gles30` - an explicit OpenGL ES context; start the process with
//!   `MESA_GLES_VERSION_OVERRIDE=3.0` so Mesa gives exactly 3.0 (it offers 3.2 otherwise).
//! * `gl3d_llvmpipe` - Mesa's software rasteriser, the no-GPU case (skipped when there is none).
//!
//! The pure-CPU tests (the draw plan) run in every `cargo test`.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{pos2, vec2, Color32, ColorImage, Context, LayerId, Order, Rect, Stroke, StrokeKind, TextureOptions};
use egui_glow::glow::{self, HasContext};

use super::*;
use crate::gamedata::roadtypes::RoadType;
use crate::maprender::cfg::{MapLayerConfig, MarkerStyle, OtherRoads, RaceLineMode, TiltCfg};
use crate::maprender::data::{Chain, MapLayers, RoadLayer};
use crate::maprender::paint2d::{draw_base, draw_layers, BaseParams, LayerCtx};
use crate::maprender::racesel::{MarkKind, RaceDraw, RaceMark, RaceRoad, RaceSel, Run};
use crate::maprender::terrain::Terrain;
use crate::overlay::gl::{Flavour, Headless};

// ── the synthetic world ──────────────────────────────────────────────────────────────────────

struct World {
    terrain: Arc<Terrain>,
    layers: Arc<MapLayers>,
    mesh: Arc<RoadMesh>,
    image: ColorImage,
    cal: MapCalibration,
    orig: [u32; 2],
}

/// The synthetic terrain (three hills on a flat 2 km square), roads of every type with D51's
/// interesting heights (an elevated highway, a tunnel through the big hill, a jump over a small
/// one, orphan nodes at y = 0), and a satellite stand-in that is calibrated to the 2 km square
/// and carries a 200 m grid so the draping can be judged by eye.
fn world() -> World {
    let terrain = Arc::new(Terrain::synthetic());
    let t = &*terrain;
    let ground = |x: f32, z: f32| t.height(x, z);
    // A chain along `pts` (every 40 m), heights from `f(x, z, ground)`.
    let chain = |a: [f32; 2], b: [f32; 2], f: &dyn Fn(f32, f32, f32) -> f32| {
        let n = (((b[0] - a[0]).hypot(b[1] - a[1])) / 40.0).ceil().max(1.0) as usize;
        let mut pts = vec![];
        let mut ys = vec![];
        for i in 0..=n {
            let u = i as f32 / n as f32;
            // a gentle wiggle so the ribbons curve
            let (x, z) = (a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u + 14.0 * (u * 9.0).sin());
            pts.push([x, z]);
            ys.push(f(x, z, ground(x, z)));
        }
        Chain::new(pts, ys)
    };
    let mut roads = RoadLayer::default();
    let on_ground = |_: f32, _: f32, g: f32| g + 0.3;
    roads.by_type[RoadType::Road.index() as usize].push(chain([-900.0, -300.0], [900.0, -300.0], &on_ground));
    roads.by_type[RoadType::Offroad.index() as usize].push(chain([-900.0, -200.0], [900.0, -200.0], &on_ground));
    roads.by_type[RoadType::Other.index() as usize].push(chain([100.0, -250.0], [700.0, -250.0], &on_ground));
    roads.by_type[RoadType::Trail.index() as usize].push(chain([-900.0, -100.0], [900.0, -100.0], &on_ground));
    // Cross-country: node heights far wrong on purpose, it is draped anyway.
    roads.by_type[RoadType::Crosscountry.index() as usize].push(chain([-900.0, 40.0], [900.0, 40.0], &|_, _, _| 0.5));
    // A tunnel through the big hill (-300, 200): underground inside it.
    roads.by_type[RoadType::Tunnel.index() as usize].push(chain([-640.0, 210.0], [40.0, 210.0], &|_, _, g| g.min(150.0)));
    // An elevated highway: 22 m above the ground all along (a bridge over the low parts).
    roads.by_type[RoadType::Highway.index() as usize].push(chain([-900.0, 330.0], [900.0, 330.0], &|_, _, g| g + 22.0));
    // Orphan nodes (y = 0) all along: node-height mode must put them on the ground.
    roads.by_type[3].push(chain([-900.0, -420.0], [900.0, -420.0], &|_, _, _| 0.0));
    // A short untyped one.
    roads.by_type[0].push(chain([-300.0, -360.0], [300.0, -360.0], &on_ground));
    // A jump over the 45 m hill at (50, 450).
    roads.jumps.push([-120.0, 450.0, ground(-120.0, 450.0) + 2.0, 220.0, 450.0, ground(220.0, 450.0) + 2.0]);
    let layers = Arc::new(MapLayers { rev: 1, roads: Arc::new(roads), ..Default::default() });
    let mesh = Arc::new(RoadMesh::build(&layers.roads, &terrain, 1));

    // The satellite stand-in: 512 px over the 2048 m square (4 m per px), calibrated to it.
    let n = 512usize;
    let mut px = vec![0u8; n * n * 4];
    for r in 0..n {
        for c in 0..n {
            let (x, z) = (-1024.0 + (c as f32 + 0.5) * 4.0, 1024.0 - (r as f32 + 0.5) * 4.0);
            let h = ground(x, z);
            let hill = ((h - 100.0) / 220.0).clamp(0.0, 1.0);
            let mut col = [60.0 + 120.0 * hill, 105.0 + 40.0 * hill, 55.0 + 60.0 * hill];
            if (x.rem_euclid(200.0) < 6.0) || (z.rem_euclid(200.0) < 6.0) {
                col = [col[0] * 0.55, col[1] * 0.55, col[2] * 0.55];
            }
            if x.abs() < 14.0 && z.abs() < 14.0 {
                col = [220.0, 40.0, 40.0];
            }
            let i = (r * n + c) * 4;
            px[i..i + 4].copy_from_slice(&[col[0] as u8, col[1] as u8, col[2] as u8, 255]);
        }
    }
    World {
        terrain,
        layers,
        mesh,
        image: ColorImage::from_rgba_unmultiplied([n, n], &px),
        cal: MapCalibration { px_per_m: 0.25, origin_x: -1024.0, origin_z: 1024.0 },
        orig: [n as u32, n as u32],
    }
}

// ── the rig: a headless context, a real painter, a target FBO ───────────────────────────────

struct Out {
    /// RGBA, top row first.
    px: Vec<u8>,
    w: usize,
    h: usize,
    gl_error: u32,
    fbo_restored: bool,
}

impl Out {
    fn at(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.w + x) * 4;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }
    fn save(&self, name: &str) {
        let dir = png_dir();
        let _ = std::fs::create_dir_all(&dir);
        let img = image::RgbaImage::from_raw(self.w as u32, self.h as u32, self.px.clone()).expect("size");
        img.save(dir.join(name)).unwrap_or_else(|e| eprintln!("could not write {name}: {e}"));
    }
    /// How many pixels in the box are within `tol` (per channel sum) of `c`.
    fn count_near(&self, r: [usize; 4], c: [u8; 3], tol: i32) -> usize {
        let mut n = 0;
        for y in r[1]..r[3].min(self.h) {
            for x in r[0]..r[2].min(self.w) {
                let p = self.at(x, y);
                let d: i32 = (0..3).map(|k| (p[k] as i32 - c[k] as i32).abs()).sum();
                n += (d <= tol) as usize;
            }
        }
        n
    }
}

fn png_dir() -> PathBuf {
    std::env::var_os("GL3D_PNG_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("gl3d_png"))
}

struct Rig {
    painter: Option<egui_glow::Painter>,
    ctx: Context,
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    /// Target size in px.
    size: [i32; 2],
    gl: Arc<glow::Context>,
    t: f64,
    /// Texture uploads of the setup frame, painted with the first real one.
    pending: Option<egui::TexturesDelta>,
    /// Declared last: the context outlives everything above (the painter's `destroy` needs it).
    hl: Headless,
}

impl Rig {
    fn new(flavour: Flavour, device: Option<usize>, size: [i32; 2]) -> Result<Rig, String> {
        let hl = Headless::new_with(flavour, device)?;
        let gl = hl.glow.clone();
        // SAFETY: plain GL object setup on the current (surfaceless) context.
        let (fbo, tex) = unsafe {
            let tex = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, size[0], size[1], 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
            let fbo = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                return Err("target FBO incomplete".into());
            }
            (fbo, tex)
        };
        let painter = egui_glow::Painter::new(gl.clone(), "", None, true).map_err(|e| format!("egui_glow: {e}"))?;
        // egui limits textures to 2048 px until a frame has told it the GPU's limit.
        let ctx = Context::default();
        let first = ctx.run(egui::RawInput { max_texture_side: Some(8192), ..Default::default() }, |_| {});
        Ok(Rig { painter: Some(painter), ctx, fbo, tex, size, gl, t: 0.0, pending: Some(first.textures_delta), hl })
    }

    fn info(&self) -> String {
        // SAFETY: plain state queries.
        unsafe { format!("{} | {} | {}", self.gl.get_parameter_string(glow::VERSION), self.gl.get_parameter_string(glow::RENDERER), self.hl.device) }
    }

    fn painter(&self) -> &egui_glow::Painter {
        self.painter.as_ref().unwrap()
    }

    /// One egui frame at `ppp` into the target FBO, read back. `ui` runs once.
    fn frame(&mut self, ppp: f32, ui: impl FnMut(&Context)) -> Out {
        let [w, h] = self.size;
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(w as f32 / ppp, h as f32 / ppp))),
            time: Some(self.t),
            max_texture_side: Some(8192),
            viewports: [(egui::ViewportId::ROOT, egui::ViewportInfo { native_pixels_per_point: Some(ppp), ..Default::default() })].into_iter().collect(),
            ..Default::default()
        };
        self.t += 1.0 / 60.0;
        let mut out = self.ctx.run(raw, ui);
        if let Some(mut d) = self.pending.take() {
            d.append(std::mem::take(&mut out.textures_delta));
            out.textures_delta = d;
        }
        let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let gl = self.gl.clone();
        // SAFETY: plain GL on the current context.
        unsafe {
            // Clear any stale error so the one we read is this frame's.
            while gl.get_error() != 0 {}
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            self.painter().clear([w as u32, h as u32], [0.0; 4]);
        }
        let painter = self.painter.as_mut().unwrap();
        painter.paint_and_update_textures([w as u32, h as u32], out.pixels_per_point, &prims, &out.textures_delta);
        // SAFETY: plain GL on the current context.
        let (bound, err, px) = unsafe {
            let bound = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING) as u32;
            let err = gl.get_error();
            let mut px = vec![0u8; (w * h * 4) as usize];
            gl.read_pixels(0, 0, w, h, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut px)));
            (bound, err, px)
        };
        // bottom-up -> top-down
        let mut flipped = Vec::with_capacity(px.len());
        for row in (0..h as usize).rev() {
            flipped.extend_from_slice(&px[row * w as usize * 4..(row + 1) * w as usize * 4]);
        }
        Out { px: flipped, w: w as usize, h: h as usize, gl_error: err, fbo_restored: bound == self.fbo.0.get() }
    }

    fn load_map(&self, w: &World, opts: TextureOptions) -> (egui::TextureHandle, MapTex) {
        let h = self.ctx.load_texture("test-map", w.image.clone(), opts);
        let tex = MapTex { id: h.id(), orig_size: w.orig, winter: false };
        (h, tex)
    }

    /// Free the 3D resources and the painter, in the order the owners must (3D first).
    fn finish(mut self, h: &Gl3dHandle) {
        h.destroy(&self.gl);
        if let Some(mut p) = self.painter.take() {
            p.destroy();
        }
        // SAFETY: deleting objects created in `new`.
        unsafe {
            self.gl.delete_framebuffer(self.fbo);
            self.gl.delete_texture(self.tex);
            assert_eq!(self.gl.get_error(), 0, "GL error while tearing down");
        }
    }
}

// ── a map frame the way the call sites build it ─────────────────────────────────────────────

/// Dashboard-like (mipmaps off, no corner mask) or HUD-like.
#[derive(Clone, Copy, PartialEq)]
enum Site {
    Hud,
    Dashboard,
}

struct View {
    site: Site,
    rect: Rect,
    car: (f32, f32),
    yaw: f32,
    zoom: f32,
    angle: f32,
    car_y: Option<f32>,
    /// A clip rect for the 3D callback (egui scissor), if any.
    clip: Option<Rect>,
    /// Skip `add_scene` altogether (the pure 2D reference).
    no_3d: bool,
    /// The own car (D77), queued after the scene with `add_marker`.
    marker: Option<Marker3d>,
    /// The co-op teammates (D89), in the same callback as the own car.
    mates: Vec<Marker3d>,
}

impl View {
    fn hud() -> View {
        View { site: Site::Hud, rect: Rect::from_min_size(pos2(14.0, 14.0), vec2(208.0, 136.0)), car: (-60.0, -160.0), yaw: 0.6, zoom: 500.0, angle: 40.0, car_y: None, clip: None, no_3d: false, marker: None, mates: vec![] }
    }
    fn dashboard() -> View {
        View { site: Site::Dashboard, rect: Rect::from_min_size(pos2(10.0, 10.0), vec2(600.0, 400.0)), car: (0.0, -250.0), yaw: 0.0, zoom: 800.0, angle: 50.0, car_y: None, clip: None, no_3d: false, marker: None, mates: vec![] }
    }
}

const PLATE: Color32 = Color32::from_rgb(10, 12, 16);
const BACKDROP: Color32 = Color32::from_rgb(60, 70, 80);

fn tilt(angle: f32) -> TiltCfg {
    let mut t = TiltCfg { on: true, angle_deg: angle, ..TiltCfg::default() };
    t.relief.on = true;
    t
}

fn camera(w: &World, v: &View) -> Camera {
    Camera::from_cfg_relief(&tilt(v.angle), v.car, v.yaw, v.zoom, v.rect, Some(&w.terrain), v.car_y)
}

fn scene(w: &World, cam: Camera, mesh: bool, tex: Option<MapTex>, site: Site) -> Scene3d {
    let mut roads = RoadsCfg::default();
    if site == Site::Hud {
        roads.casing_px = 1.0;
    }
    Scene3d {
        cam,
        mesh: mesh.then(|| w.mesh.clone()),
        map: tex,
        cal: w.cal,
        look: ImageLook::FULL,
        mirror: true,
        a: 1.0,
        s: 1.0,
        corner_radius: if site == Site::Hud { 22.0 } else { 0.0 },
        relief: tilt(40.0).relief,
        roads,
        focus: None,
        race: None,
        route: None,
        trails: vec![],
    }
}

/// One frame of a map: backdrop, plate (HUD), the tilted 2D underlay while the 3D is not ready,
/// the 3D, and a marker + border over it - exactly the order a call site uses.
fn map_frame(rig: &mut Rig, w: &World, h: &Gl3dHandle, tex: MapTex, v: &View, ppp: f32, tweak: &dyn Fn(&mut Scene3d)) -> Out {
    let (mesh_on, size) = (true, rig.size);
    let _ = size;
    rig.frame(ppp, |ctx| {
        let p = ctx.layer_painter(LayerId::new(Order::Background, egui::Id::new("map")));
        p.rect_filled(ctx.content_rect(), 0.0, BACKDROP);
        if v.site == Site::Hud {
            p.rect_filled(v.rect.expand(4.0), 24.0, PLATE);
        }
        let cam3 = camera(w, v);
        if h.wants_underlay() {
            // The 2D tilted map, as today: base image + the roads, from the camera without relief.
            let cam2 = Camera::from_cfg(&tilt(v.angle), v.car, v.yaw, v.zoom, v.rect);
            let outline = [v.rect.left_top(), v.rect.right_top(), v.rect.right_bottom(), v.rect.left_bottom()];
            let pc = p.with_clip_rect(v.rect);
            draw_base(&pc, &BaseParams { cam: &cam2, cal: w.cal, tex, outline: &outline, mirror: true, look: ImageLook::FULL, a: 1.0, far_fade: true });
            let mut cfg = MapLayerConfig::default();
            cfg.pois.on = false;
            cfg.race_lines.mode = RaceLineMode::Off;
            let sel = RaceSel::default();
            let cx = LayerCtx { p: &pc, cam: &cam2, s: 1.0, a: 1.0, car: v.car, corner_clip: None, icons: None, race_sel: &sel, week: None, nav: None };
            draw_layers(&cx, &w.layers, &cfg);
        }
        if !v.no_3d {
            let mut sc = scene(w, cam3.clone(), mesh_on, Some(tex), v.site);
            tweak(&mut sc);
            let pp = match v.clip {
                Some(c) => p.with_clip_rect(c),
                None => p.clone(),
            };
            let (a, s, corner_radius) = (sc.a, sc.s, sc.corner_radius);
            add_scene(&pp, h, sc);
            // (egui vectors over the scene would go here, then the car on top of them)
            if let Some(marker) = v.marker {
                add_marker(&pp, h, MarkerScene { cam: cam3, marker, mates: v.mates.clone(), a, s, corner_radius });
            }
        }
        // Over the 3D: a marker at the car and the border.
        if v.marker.is_none() {
            p.circle_filled(v.rect.center() + vec2(0.0, 36.0), 6.0, Color32::WHITE);
        }
        let radius = if v.site == Site::Hud { 22.0 } else { 0.0 };
        p.rect_stroke(v.rect, radius, Stroke::new(2.0, Color32::from_rgb(230, 235, 240)), StrokeKind::Inside);
    })
}

/// Run frames until the 3D is ready (the init is spread over a few), asserting every one is clean.
fn warm_up(rig: &mut Rig, w: &World, h: &Gl3dHandle, tex: MapTex, v: &View, ppp: f32) -> Out {
    for i in 0..12 {
        let o = map_frame(rig, w, h, tex, v, ppp, &|_| {});
        assert_eq!(o.gl_error, 0, "frame {i}: GL error 0x{:X}", o.gl_error);
        assert!(o.fbo_restored, "frame {i}: the target framebuffer was not restored");
        if h.status() == Gl3dStatus::Ready && !h.busy() {
            return map_frame(rig, w, h, tex, v, ppp, &|_| {});
        }
        if let Some(e) = h.status().failure() {
            panic!("3D failed in frame {i}: {e}");
        }
    }
    panic!("3D not ready after 12 frames: {:?}", h.status());
}

fn inside(v: &View, ppp: f32) -> [usize; 4] {
    let r = v.rect.shrink(6.0);
    [(r.min.x * ppp) as usize, (r.min.y * ppp) as usize, (r.max.x * ppp) as usize, (r.max.y * ppp) as usize]
}

/// Mean absolute error per channel (0..255) between two pictures of the same size.
fn mae(a: &Out, b: &Out) -> f64 {
    assert_eq!(a.px.len(), b.px.len());
    a.px.iter().zip(&b.px).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as f64).sum::<f64>() / a.px.len() as f64
}

/// Roads wide enough that the fill colour, not only the casing, is on screen.
fn thick(s: &mut Scene3d) {
    s.roads.min_px = 5.0;
    s.roads.max_px = 8.0;
}

fn rgb(c: crate::maprender::cfg::Rgb) -> [u8; 3] {
    c.0
}

// ── the suite, run per GL flavour ────────────────────────────────────────────────────────────

fn open(flavour: Flavour, device: Option<usize>, size: [i32; 2]) -> Option<Rig> {
    match Rig::new(flavour, device, size) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("SKIP: no {flavour:?} context on device {device:?}: {e} (EGL devices: {:?})", Headless::devices());
            None
        }
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() { f64::NAN } else { v[v.len() / 2] }
}

/// What every flavour must do: the HUD scene and the Dashboard scene come out clean, covered, with
/// roads in their type colours, the markers above, the framebuffer restored, and a PNG.
fn suite(flavour: Flavour, device: Option<usize>, tag: &str) {
    let Some(mut rig) = open(flavour, device, [1100, 520]) else { return };
    eprintln!("[{tag}] {}", rig.info());
    let w = world();
    let (_hold, tex) = rig.load_map(&w, crate::minimap::OVERLAY_MAP_TEXTURE_OPTIONS);
    let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, sync_timing: true, ..Default::default() });

    // ── HUD pill, ppp 1 and 1.5
    for ppp in [1.0f32, 1.5] {
        let hud = View::hud();
        let o = warm_up(&mut rig, &w, &h, tex, &hud, ppp);
        let caps = h.caps().expect("probed");
        if ppp == 1.0 {
            eprintln!("[{tag}] caps: {} | es {} | max tex {} | aniso {:?} | timer {}", caps.version, caps.es, caps.max_texture, caps.aniso, caps.timer);
            assert_eq!(caps.es, flavour == Flavour::Gles3);
        }
        o.save(&format!("{tag}_hud_ppp{ppp}.png"));
        // Covered: nearly every pixel inside the pill (shrunk past the rounded corners) is map, not plate.
        let b = inside(&hud, ppp);
        let total = (b[2] - b[0]) * (b[3] - b[1]);
        let plate = o.count_near(b, [10, 12, 16], 6);
        assert!(plate * 20 < total, "[{tag}] HUD ppp {ppp}: {plate} of {total} pixels are still the plate");
        // Over the 3D: the marker (white) and the border.
        let m = o.at(((hud.rect.center().x) * ppp) as usize, ((hud.rect.center().y + 36.0) * ppp) as usize);
        assert!(m[0] > 240 && m[1] > 240 && m[2] > 240, "[{tag}] marker above the 3D: {m:?}");
        // Under the 3D, outside the callback: backdrop and plate untouched.
        assert_eq!(o.at((2.0 * ppp) as usize, (2.0 * ppp) as usize)[..3], [60, 70, 80]);
        let corner = o.at(((hud.rect.min.x + 4.0) * ppp) as usize, ((hud.rect.min.y + 4.0) * ppp) as usize);
        assert_eq!(corner[..3], [10, 12, 16], "[{tag}] the rounded mask cuts the pill corner (plate shows): {corner:?}");
        assert_eq!(h.status(), Gl3dStatus::Ready);
    }
    if flavour == Flavour::Gles3 && std::env::var("MESA_GLES_VERSION_OVERRIDE").as_deref() == Ok("3.0") {
        assert!(h.caps().unwrap().version.contains("ES 3.0"), "{:?}", h.caps().unwrap().version);
    }

    // ── Dashboard: roads in type colours
    let dash = View::dashboard();
    warm_up(&mut rig, &w, &h, tex, &dash, 1.0);
    let o = map_frame(&mut rig, &w, &h, tex, &dash, 1.0, &thick);
    o.save(&format!("{tag}_dashboard.png"));
    // A close look at the bridge and the tunnel portal: HUD at 3x, 120 m zoom.
    let close = View { zoom: 120.0, car: (-250.0, 300.0), yaw: 0.0, angle: 55.0, ..View::hud() };
    map_frame(&mut rig, &w, &h, tex, &close, 3.0, &|_| {}).save(&format!("{tag}_hud_close_x3.png"));
    // The elevated highway seen from the side (a 22 m bridge over flat ground, 3 m deck).
    let side = View { zoom: 70.0, car: (450.0, 250.0), yaw: 0.0, angle: 72.0, car_y: Some(100.5), ..View::hud() };
    map_frame(&mut rig, &w, &h, tex, &side, 3.0, &|s| s.relief.deck_m = 6.0).save(&format!("{tag}_hud_bridge_side_x3.png"));
    let b = [0usize, 0, rig.size[0] as usize, rig.size[1] as usize];
    let styles = RoadsCfg::default().styles;
    let mut found = vec![];
    for ty in [RoadType::Road, RoadType::Offroad, RoadType::Other, RoadType::Trail, RoadType::Crosscountry, RoadType::Tunnel, RoadType::Highway, RoadType::Jump] {
        let st = styles.get(ty).unwrap();
        let n = o.count_near(b, rgb(st.color), 30);
        found.push((ty, n));
    }
    eprintln!("[{tag}] road colours found in the Dashboard scene: {found:?}");
    assert!(found.iter().filter(|(_, n)| *n > 20).count() >= 7, "[{tag}] roads by type: {found:?}");
    let s = h.stats();
    eprintln!("[{tag}] Dashboard 600x400: {} tri, {} draws, tiles {} near / {} far, cpu {:.2} ms, gpu {:?} ms", s.last.triangles, s.last.draws, s.last.tiles_near, s.last.tiles_far, s.callback_ms, s.last.gpu_ms);

    // ── perf: medians over 30 frames
    for (name, v, ppp) in [("HUD 208x136 driving 500 m", View::hud(), 1.0f32), ("Dashboard 600x400 800 m", View::dashboard(), 1.0)] {
        let (mut gpu, mut cpu) = (vec![], vec![]);
        for _ in 0..30 {
            let o = map_frame(&mut rig, &w, &h, tex, &v, ppp, &|_| {});
            assert_eq!(o.gl_error, 0);
            let s = h.stats();
            gpu.extend(s.last.gpu_ms);
            cpu.push(s.last.cpu_ms);
        }
        let s = h.stats();
        eprintln!("[{tag}] PERF {name}: {} tri, {} draws, cpu {:.3} ms, gpu {:.3} ms (median of 30)", s.last.triangles, s.last.draws, median(cpu), median(gpu));
    }
    // ── the navigation route (phase L): its own road pass, in every flavour
    {
        let ch = &w.layers.roads.by_type[RoadType::Road.index() as usize][0];
        let line = Arc::new(crate::nav::NavLine { rev: 910_000, pts: ch.pts.clone(), y: ch.y.iter().map(|y| y + 0.3).collect(), seg_kind: vec![2; ch.pts.len() - 1] });
        let r = route3d(&w, &line);
        let mut v = View::dashboard();
        v.car = (0.0, -250.0);
        let mut last = None;
        for _ in 0..3 {
            last = Some(map_frame(&mut rig, &w, &h, tex, &v, 1.0, &|s| {
                thick(s);
                s.route = Some(r.clone());
            }));
        }
        let o = last.unwrap();
        assert_eq!(o.gl_error, 0, "[{tag}] route frame");
        o.save(&format!("{tag}_nav_route.png"));
        let n = o.count_near(b, route_rgb(), 30);
        eprintln!("[{tag}] navigation route: {n} px");
        assert!(n > 300, "[{tag}] the navigation route is drawn ({n})");
    }
    markers(&mut rig, &w, &h, tex, tag);
    mates(&mut rig, &w, &h, tex, tag);
    rig.finish(&h);
}

// ── the own car and the trails in the scene (D77 / D78) ─────────────────────────────────────

/// A test colour nothing else in the synthetic world has.
const CAR: Color32 = Color32::from_rgb(235, 60, 205);
const TRAIL: Color32 = Color32::from_rgb(40, 225, 255);

/// A trail of `n` points from `a` to `b` (x, z), heights from `y`, all recent and solid.
fn trail(a: (f32, f32), b: (f32, f32), n: usize, y: &dyn Fn(f32, f32) -> f32) -> Trail3d {
    let pts: Vec<[f32; 3]> = (0..=n)
        .map(|i| {
            let u = i as f32 / n as f32;
            let (x, z) = (a.0 + (b.0 - a.0) * u, a.1 + (b.1 - a.1) * u);
            [x, y(x, z), z]
        })
        .collect();
    Trail3d { segs: pts.windows(2).map(|p| TrailSeg { a: p[0], b: p[1], alpha: 0.86 }).collect(), colour: TRAIL }
}

/// The marker on a hill, on the bridge, in the tunnel (with a trail going in), both kinds, at the
/// HUD's size (1x and 3x) and the Viewer's; clean frames, the marker where the car is and whole
/// in the tunnel, PNGs to look at.
fn markers(rig: &mut Rig, w: &World, h: &Gl3dHandle, tex: MapTex, tag: &str) {
    let t = w.terrain.clone();
    let tunnel_y = |x: f32, z: f32| t.height(x, z).min(150.0) + 0.45;
    let ground = |x: f32, z: f32| t.height(x, z) + 0.45;
    let east = std::f32::consts::FRAC_PI_2;
    // (name, car (x, z), car height, yaw, the trail behind it, the view's yaw: across the bridge
    // and the tunnel, so the deck height and the trail into the hill are seen from the side)
    let spots: Vec<(&str, (f32, f32), f32, f32, Trail3d, f32)> = vec![
        ("hill", (-60.0, -160.0), ground(-60.0, -160.0), 0.6, trail((-60.0 - 300.0 * 0.6f32.sin(), -160.0 - 300.0 * 0.6f32.cos()), (-60.0, -160.0), 30, &ground), 0.6),
        ("bridge", (450.0, 330.0), t.height(450.0, 330.0) + 22.45, east, trail((150.0, 330.0), (450.0, 330.0), 30, &|x, z| t.height(x, z) + 22.45), 0.7),
        ("tunnel", (-300.0, 210.0), tunnel_y(-300.0, 210.0), east, trail((-700.0, 210.0), (-300.0, 210.0), 40, &tunnel_y), 0.35),
    ];
    for (name, car, y, yaw, tr, view_yaw) in &spots {
        assert!(*name != "tunnel" || t.height(car.0, car.1) > y + 40.0, "the tunnel spot is deep under the hill");
        for kind in [MarkerStyle::Arrow, MarkerStyle::Sedan] {
            let k = if kind == MarkerStyle::Arrow { "arrow" } else { "sedan" };
            for (site, ppp, zoom) in [("hud", 1.0f32, 500.0f32), ("hud", 3.0, 500.0), ("viewer", 1.0, 120.0)] {
                let base = if site == "hud" { View::hud() } else { View { zoom, ..View::dashboard() } };
                let mk = Marker3d { pos: [car.0, *y, car.1], yaw: *yaw, kind, colour: CAR };
                let v = View { marker: Some(mk), car: *car, yaw: *view_yaw, zoom, angle: if site == "hud" { 40.0 } else { 50.0 }, car_y: Some(y + 1.0), ..base };
                let with = |s: &mut Scene3d| s.trails = vec![tr.clone()];
                warm_up(rig, w, h, tex, &v, ppp);
                let o = map_frame(rig, w, h, tex, &v, ppp, &with);
                assert_eq!(o.gl_error, 0, "[{tag}] {name} {k}: GL error 0x{:X}", o.gl_error);
                assert!(o.fbo_restored);
                o.save(&format!("{tag}_marker_{name}_{k}_{site}_x{ppp}.png"));
                // The marker is where the car is (the camera's car point) and whole: enough pixels of
                // the car colour (the tops are lit at full colour) in a box around it.
                let cam = camera(w, &v);
                let (at, _) = cam.project3(car.0, *y, car.1).expect("in front");
                let r = 26.0;
                let b = [((at.x - r) * ppp) as usize, ((at.y - r) * ppp) as usize, ((at.x + r) * ppp) as usize, ((at.y + r) * ppp) as usize];
                let n = o.count_near(b, [CAR.r(), CAR.g(), CAR.b()], 70);
                let min = (18.0 * ppp * ppp) as usize;
                eprintln!("[{tag}] {name} {k} {site} x{ppp}: {n} car-coloured px near {at:?}");
                assert!(n >= min, "[{tag}] {name} {k} {site} x{ppp}: only {n} car-coloured px (want >= {min}) - is the marker hidden?");
                // The outline: dark pixels around the model.
                let dark = o.count_near(b, [5, 6, 9], 40);
                assert!(dark > 0, "[{tag}] {name} {k} {site} x{ppp}: no outline");
                if *name == "tunnel" && site == "hud" && ppp == 1.0 {
                    // The trail into the tunnel shows (seen through the hill): its colour over the
                    // hill, west of the car, compared with the same frame without trails.
                    let without = map_frame(rig, w, h, tex, &v, ppp, &|_| {});
                    let diff = o.px.chunks(4).zip(without.px.chunks(4)).filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 12)).count();
                    eprintln!("[{tag}] tunnel trail changes {diff} px");
                    assert!(diff > 60, "[{tag}] the trail into the tunnel is not visible ({diff} px)");
                }
            }
        }
    }
    // The model on screen at HUD size: about the minimum length (20 pt for the sedan at x1).
    let mk = Marker3d { pos: [-60.0, ground(-60.0, -160.0), -160.0], yaw: 0.0, kind: MarkerStyle::Sedan, colour: CAR };
    let v = View { car: (-60.0, -160.0), yaw: 0.0, car_y: Some(ground(-60.0, -160.0) + 1.0), marker: Some(mk), ..View::hud() };
    let o = map_frame(rig, w, h, tex, &v, 1.0, &|_| {});
    let (mut y0, mut y1) = (usize::MAX, 0);
    for y in 0..o.h {
        for x in 0..o.w {
            let p = o.at(x, y);
            if (0..3).map(|k| (p[k] as i32 - [CAR.r(), CAR.g(), CAR.b()][k] as i32).abs()).sum::<i32>() <= 120 {
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    eprintln!("[{tag}] sedan at HUD x1 heading up the screen: {} px tall on screen", y1.saturating_sub(y0));
    assert!(y1 > y0 && (6..=30).contains(&(y1 - y0)), "[{tag}] sedan screen size {}..{}", y0, y1);
}

// ── the co-op teammates in the same callback as the own car (D89) ───────────────────────────

const MATE_A: Color32 = Color32::from_rgb(255, 215, 20);
const MATE_B: Color32 = Color32::from_rgb(40, 255, 90);

/// A Viewer-sized view centred on `car`, `own` as the own car and `mates` as teammates.
fn mate_view(car: (f32, f32), own: Marker3d, mates: Vec<Marker3d>) -> View {
    View { zoom: 300.0, yaw: 0.0, car, car_y: Some(own.pos[1] + 1.0), marker: Some(own), mates, ..View::dashboard() }
}

/// Teammates are drawn with the own car's model in their colours at their own positions and
/// heights, in the one marker callback: 2 more draw calls each and nothing else (no extra callback,
/// clear or composite); a teammate in a tunnel is down at the tunnel like the own car; the own car
/// is on top of a teammate at the same spot.
fn mates(rig: &mut Rig, w: &World, h: &Gl3dHandle, tex: MapTex, tag: &str) {
    let t = w.terrain.clone();
    let ground = |x: f32, z: f32| t.height(x, z) + 0.45;
    let near = |o: &Out, cam: &Camera, p: [f32; 3], c: Color32, r: f32, ppp: f32| -> usize {
        let (at, _) = cam.project3(p[0], p[1], p[2]).expect("in front");
        let b = [((at.x - r) * ppp).max(0.0) as usize, ((at.y - r) * ppp).max(0.0) as usize, ((at.x + r) * ppp) as usize, ((at.y + r) * ppp) as usize];
        o.count_near(b, [c.r(), c.g(), c.b()], 70)
    };
    for kind in [MarkerStyle::Arrow, MarkerStyle::Sedan] {
        let k = if kind == MarkerStyle::Arrow { "arrow" } else { "sedan" };
        let car = (-60.0f32, -160.0f32);
        let own = Marker3d { pos: [car.0, ground(car.0, car.1), car.1], yaw: 0.6, kind, colour: CAR };
        let a = Marker3d { pos: [car.0 + 45.0, ground(car.0 + 45.0, car.1 + 30.0), car.1 + 30.0], yaw: 2.0, kind, colour: MATE_A };
        let b = Marker3d { pos: [car.0 - 50.0, ground(car.0 - 50.0, car.1 - 40.0), car.1 - 40.0], yaw: 4.0, kind, colour: MATE_B };
        let solo = mate_view(car, own, vec![]);
        warm_up(rig, w, h, tex, &solo, 1.0);
        let o0 = map_frame(rig, w, h, tex, &solo, 1.0, &|_| {});
        let (draws0, tris0) = (h.stats().last.draws, h.stats().last.triangles);
        let v = mate_view(car, own, vec![a, b]);
        let o = map_frame(rig, w, h, tex, &v, 1.0, &|_| {});
        let st = h.stats().last;
        assert_eq!(o.gl_error, 0, "[{tag}] mates {k}: GL error 0x{:X}", o.gl_error);
        assert!(o.fbo_restored);
        o.save(&format!("{tag}_mates_{k}.png"));
        // One callback: each teammate adds the hull and the model, nothing more.
        assert_eq!(st.draws, draws0 + 4, "[{tag}] mates {k}: draws {} vs {} without teammates", st.draws, draws0);
        assert!(st.triangles > tris0, "[{tag}] mates {k}: triangles");
        let cam = camera(w, &v);
        for (name, m) in [("A", a), ("B", b)] {
            let n = near(&o, &cam, m.pos, m.colour, 40.0, 1.0);
            let before = near(&o0, &cam, m.pos, m.colour, 40.0, 1.0);
            eprintln!("[{tag}] mates {k} {name}: {n} px of its colour ({before} without it)");
            // (the synthetic terrain has some yellowish pixels of its own: count what the teammate adds)
            assert!(n >= before + 30, "[{tag}] mates {k} {name}: {n} px (solo frame: {before})");
        }
        // The own car is still there, whole.
        assert!(near(&o, &cam, own.pos, CAR, 40.0, 1.0) >= 40, "[{tag}] mates {k}: own car");
        // A teammate exactly under the own car: the own car is drawn on top of it.
        let ou = map_frame(rig, w, h, tex, &mate_view(car, own, vec![Marker3d { colour: MATE_A, ..own }]), 1.0, &|_| {});
        let (n_own, n_mate) = (near(&ou, &cam, own.pos, CAR, 40.0, 1.0), near(&ou, &cam, own.pos, MATE_A, 40.0, 1.0).saturating_sub(near(&o0, &cam, own.pos, MATE_A, 40.0, 1.0)));
        assert!(n_own >= 40 && n_mate * 4 < n_own, "[{tag}] mates {k}: own car {n_own} px, the teammate under it {n_mate} px");
    }
    // A teammate in the tunnel under the big hill: drawn down at the tunnel's road, not on the hill
    // above it. Also at HUD size (scaled up) and 3x.
    let (tx, tz) = (-300.0f32, 210.0f32);
    let ty = t.height(tx, tz).min(150.0) + 0.45;
    assert!(t.height(tx, tz) > ty + 40.0, "the tunnel spot is deep under the hill");
    for (site, ppp, zoom) in [("viewer", 1.0f32, 70.0f32), ("hud", 1.0, 500.0), ("hud", 3.0, 500.0)] {
        // The own car is in the tunnel too, behind the teammate (the camera follows its height).
        let car = (tx - if site == "hud" { 120.0 } else { 40.0 }, tz);
        let own = Marker3d { pos: [car.0, ty, car.1], yaw: std::f32::consts::FRAC_PI_2, kind: MarkerStyle::Sedan, colour: CAR };
        let mate = Marker3d { pos: [tx, ty, tz], yaw: std::f32::consts::FRAC_PI_2, kind: MarkerStyle::Sedan, colour: MATE_A };
        let view = |mates: Vec<Marker3d>| View { marker: Some(own), mates, car, yaw: 0.35, zoom, angle: if site == "hud" { 40.0 } else { 50.0 }, car_y: Some(own.pos[1] + 1.0), ..if site == "hud" { View::hud() } else { View::dashboard() } };
        let v = view(vec![mate]);
        warm_up(rig, w, h, tex, &v, ppp);
        let o = map_frame(rig, w, h, tex, &v, ppp, &|_| {});
        let o0 = map_frame(rig, w, h, tex, &view(vec![]), ppp, &|_| {});
        assert_eq!(o.gl_error, 0, "[{tag}] tunnel mate {site} x{ppp}: GL error 0x{:X}", o.gl_error);
        o.save(&format!("{tag}_mates_tunnel_{site}_x{ppp}.png"));
        let cam = camera(w, &v);
        eprintln!("[{tag}] tunnel mate {site} x{ppp}: tunnel point {:?}, car {:?}, rect {:?}", cam.project3(tx, ty, tz).map(|p| p.0), cam.project3(car.0, own.pos[1], car.1).map(|p| p.0), v.rect);
        let r = 24.0;
        let added = |p: [f32; 3]| near(&o, &cam, p, MATE_A, r, ppp).saturating_sub(near(&o0, &cam, p, MATE_A, r, ppp));
        let down = added(mate.pos);
        // The same teammate drawn as if on the terrain surface above the tunnel.
        let (hill, _) = cam.project3(tx, t.height(tx, tz), tz).expect("in front");
        let (at, _) = cam.project3(tx, ty, tz).expect("in front");
        let on_hill = added([tx, t.height(tx, tz), tz]);
        eprintln!("[{tag}] tunnel mate {site} x{ppp}: {down} px at the tunnel, {on_hill} px at the hill surface ({} px apart)", (hill - at).length());
        // (at HUD zoom the hill surface point is only a few px from the tunnel point: no negative check there)
        let far = (hill - at).length() > r * 1.5;
        assert!(site == "hud" || far, "[{tag}] the hill surface point is far from the tunnel point on screen");
        assert!(down >= (18.0 * ppp * ppp) as usize, "[{tag}] tunnel mate {site} x{ppp}: only {down} px at the tunnel - hidden by the hill?");
        assert!(!far || on_hill < down / 4, "[{tag}] tunnel mate {site} x{ppp}: {on_hill} px on the hill surface vs {down} in the tunnel");
    }
}

#[test]
#[ignore = "needs an EGL device; writes PNGs (GL3D_PNG_DIR)"]
fn gl3d_default_gl() {
    suite(Flavour::Default, None, "default");
}

#[test]
#[ignore = "needs an EGL device; run alone with MESA_GLES_VERSION_OVERRIDE=3.0 in the environment"]
fn gl3d_gles30() {
    // Mesa reads the override when it initialises, i.e. at the first context of the process, so
    // it has to be in the environment of the test process (a `set_var` here would race the other
    // tests' contexts and be ignored after them).
    if std::env::var("MESA_GLES_VERSION_OVERRIDE").as_deref() != Ok("3.0") {
        eprintln!("NOTE gl3d_gles30: MESA_GLES_VERSION_OVERRIDE=3.0 is not set: this is Mesa's ES 3.2, not 3.0");
    }
    suite(Flavour::Gles3, None, "gles30");
}

/// Mesa's software rasteriser: the last EGL device whose renderer says llvmpipe (or softpipe).
#[test]
#[ignore = "needs a software EGL device (llvmpipe); writes PNGs"]
fn gl3d_llvmpipe() {
    let n = Headless::devices().len();
    let sw = (0..n).rev().find_map(|i| {
        let r = Rig::new(Flavour::Default, Some(i), [64, 64]).ok()?;
        let name = r.info().to_lowercase();
        let ok = name.contains("llvmpipe") || name.contains("softpipe");
        r.finish(&Gl3dHandle::new());
        ok.then_some(i)
    });
    let Some(i) = sw else {
        eprintln!("SKIP gl3d_llvmpipe: no software EGL device among {:?}", Headless::devices());
        return;
    };
    suite(Flavour::Default, Some(i), "llvmpipe");
}

// ── egui callback hygiene ────────────────────────────────────────────────────────────────────

#[test]
#[ignore = "needs an EGL device"]
fn gl3d_callback_keeps_egui_state_and_respects_the_clip_rect() {
    let Some(mut rig) = open(Flavour::Default, None, [520, 300]) else { return };
    let w = world();
    let (_hold, tex) = rig.load_map(&w, TextureOptions { wrap_mode: egui::TextureWrapMode::MirroredRepeat, ..TextureOptions::LINEAR });
    let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
    for ppp in [1.0f32, 1.5] {
        let mut v = View::hud();
        // The 3D is clipped to the left 120 points of the pill.
        v.clip = Some(Rect::from_min_max(v.rect.min, pos2(v.rect.min.x + 120.0, v.rect.max.y)));
        let o = warm_up(&mut rig, &w, &h, tex, &v, ppp);
        o.save(&format!("callback_clip_ppp{ppp}.png"));
        assert!(o.fbo_restored && o.gl_error == 0);
        let px = |x: f32, y: f32| o.at((x * ppp) as usize, (y * ppp) as usize);
        // Inside the clip: the scene. Outside it (still inside the pill): not drawn; the 2D underlay
        // is gone once Ready, so what is there is the plate.
        let inside_clip = px(v.rect.min.x + 60.0, v.rect.min.y + 100.0);
        let outside = px(v.rect.min.x + 170.0, v.rect.min.y + 100.0);
        assert_ne!(inside_clip[..3], [10, 12, 16], "the 3D is drawn inside the clip");
        assert_eq!(outside[..3], [10, 12, 16], "nothing of the 3D leaks past the clip rect (ppp {ppp}): {outside:?}");
        // egui meshes before (backdrop, plate) and after (marker, border) the callback are intact.
        assert_eq!(px(2.0, 2.0)[..3], [60, 70, 80]);
        let marker = px(v.rect.center().x, v.rect.center().y + 36.0);
        assert!(marker[0] > 240, "marker over the 3D: {marker:?}");
        // The fade alpha applies once to the whole picture: half alpha = half the scene over the plate.
        let half = map_frame(&mut rig, &w, &h, tex, &v, ppp, &|s| s.a = 0.5);
        let (f, hf) = (px(v.rect.min.x + 60.0, v.rect.min.y + 100.0), half.at(((v.rect.min.x + 60.0) * ppp) as usize, ((v.rect.min.y + 100.0) * ppp) as usize));
        for k in 0..3 {
            let want = (f[k] as f32 * 0.5 + [10.0, 12.0, 16.0][k] * 0.5).round() as i32;
            assert!((hf[k] as i32 - want).abs() <= 3, "fade alpha 0.5 (ppp {ppp}): got {hf:?}, wanted about {want} in channel {k} (full {f:?})");
        }
    }
    rig.finish(&h);
}

// ── fallback ─────────────────────────────────────────────────────────────────────────────────

/// A context that cannot do 3D (or a driver that cannot compile the shaders) must end in a
/// status message and the plain tilted 2D map, pixel for pixel the map without any 3D call.
#[test]
#[ignore = "needs an EGL device"]
fn gl3d_fallback_status_and_the_2d_map() {
    let Some(mut rig) = open(Flavour::Default, None, [260, 180]) else { return };
    let w = world();
    let (_hold, tex) = rig.load_map(&w, crate::minimap::OVERLAY_MAP_TEXTURE_OPTIONS);
    let mut reference = View::hud();
    reference.no_3d = true;
    let ref_h = Gl3dHandle::new();
    let want = map_frame(&mut rig, &w, &ref_h, tex, &reference, 1.0, &|_| {});
    want.save("fallback_2d_reference.png");
    // The reference has real content (the 2D map).
    assert!(want.count_near(inside(&reference, 1.0), [10, 12, 16], 6) * 10 < 90 * 120);

    let cases: [(&str, Gl3dOptions, &str); 3] = [
        ("a context that is too old", Gl3dOptions { min_gl: (9, 9), min_es: (9, 9), ..Default::default() }, "too old"),
        ("a shader that does not compile", Gl3dOptions { break_shader: true, ..Default::default() }, "shader"),
        ("a GPU that is too slow", Gl3dOptions { guard: Some((0.0, 0.0)), ..Default::default() }, "too slow"),
    ];
    for (what, opts, needle) in cases {
        let h = Gl3dHandle::with_options(opts);
        let mut last = None;
        for _ in 0..14 {
            let o = map_frame(&mut rig, &w, &h, tex, &View::hud(), 1.0, &|_| {});
            assert_eq!(o.gl_error, 0, "{what}: GL error 0x{:X}", o.gl_error);
            assert!(o.fbo_restored);
            last = Some(o);
            if h.status().failure().is_some() {
                break;
            }
        }
        let msg = h.status().failure().map(str::to_owned).unwrap_or_else(|| panic!("{what}: no failure, status {:?}", h.status()));
        eprintln!("fallback ({what}): {msg}");
        assert!(msg.contains(needle), "{what}: {msg}");
        assert_eq!(last_failure().as_deref(), Some(msg.as_str()));
        assert!(h.wants_underlay());
        // After the failure the frame is the 2D reference, exactly.
        let after = map_frame(&mut rig, &w, &h, tex, &View::hud(), 1.0, &|_| {});
        // A failure before the first frame leaves the map texture untouched: identical to the pixel.
        // One after frames were drawn (the guard) finds egui's texture mipmapped + anisotropic by us,
        // which only makes the 2D map a little smoother.
        if needle == "too slow" {
            assert!(mae(&after, &want) < 2.0, "{what}: after the failure the picture is the 2D map (mae {:.2})", mae(&after, &want));
        } else {
            assert!(after.px == want.px, "{what}: after the failure the picture is exactly the plain 2D map");
        }
        let _ = last;
        h.destroy(&rig.gl);
    }
    rig.finish(&ref_h);
}

// ── winding / culling ────────────────────────────────────────────────────────────────────────

/// The road mesh's winding vs the GL cull face, chosen by looking: culling back faces must not change
/// the picture (a wrong face would cull the road tops).
#[test]
#[ignore = "needs an EGL device"]
fn gl3d_culling_matches_no_culling() {
    let Some(mut rig) = open(Flavour::Default, None, [620, 420]) else { return };
    let w = world();
    let (_hold, tex) = rig.load_map(&w, TextureOptions::LINEAR);
    let mut imgs = vec![];
    for cull in [false, true] {
        let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, cull, ..Default::default() });
        warm_up(&mut rig, &w, &h, tex, &View::dashboard(), 1.0);
        let o = map_frame(&mut rig, &w, &h, tex, &View::dashboard(), 1.0, &thick);
        o.save(&format!("culling_{cull}.png"));
        imgs.push(o);
        h.destroy(&rig.gl);
    }
    let differing = imgs[0].px.chunks(4).zip(imgs[1].px.chunks(4)).filter(|(a, b)| (0..3).map(|k| (a[k] as i32 - b[k] as i32).abs()).sum::<i32>() > 12).count();
    let road = rgb(RoadsCfg::default().styles.road.color);
    let (r_off, r_on) = (imgs[0].count_near([0, 0, 620, 420], road, 30), imgs[1].count_near([0, 0, 620, 420], road, 30));
    eprintln!("culling: {differing} of {} pixels differ (translucent tunnel pixels blend twice without culling); road pixels {r_off} without, {r_on} with culling", 620 * 420);
    assert!(r_on > 400 && r_on * 10 >= r_off * 9, "back-face culling removed road tops: the winding is wrong ({r_off} vs {r_on})");
    assert!(differing * 100 < 620 * 420, "culling changed {differing} pixels");
    rig.finish(&Gl3dHandle::new());
}

// ── flat 3D == tilted 2D ─────────────────────────────────────────────────────────────────────

/// Over a flat world the 3D camera is the tilt maths (design 3.1): the draped image must be the
/// tilted 2D base, and a road must lie on `Camera::project` to a fraction of a pixel.
#[test]
#[ignore = "needs an EGL device"]
fn gl3d_flat_world_equals_the_tilted_2d_map() {
    let Some(mut rig) = open(Flavour::Default, None, [260, 180]) else { return };
    let mut w = world();
    let flat = Arc::new(Terrain::flat(100.0));
    w.terrain = flat.clone();
    let (_hold, tex) = rig.load_map(&w, crate::minimap::OVERLAY_MAP_TEXTURE_OPTIONS);
    let mut v = View::hud();
    v.car = (100.0, 50.0);
    v.yaw = 0.5;
    v.car_y = Some(100.0);
    // 2D reference: the base image only.
    let none = Gl3dHandle::new();
    let mut ref_v = View::hud();
    ref_v.car = v.car;
    ref_v.yaw = v.yaw;
    ref_v.car_y = v.car_y;
    ref_v.no_3d = true;
    // (map_frame draws roads in the underlay: use an empty road layer for a pure base comparison)
    let empty = World { layers: Arc::new(MapLayers::default()), mesh: Arc::new(RoadMesh::default()), terrain: flat.clone(), image: w.image.clone(), cal: w.cal, orig: w.orig };
    let want = map_frame(&mut rig, &empty, &none, tex, &ref_v, 1.0, &|_| {});
    let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
    for i in 0..12 {
        map_frame(&mut rig, &empty, &h, tex, &v, 1.0, &|s| {
            s.relief.shading = 0.0;
            s.roads.on = false;
        });
        if h.status() == Gl3dStatus::Ready && !h.busy() && i > 2 {
            break;
        }
    }
    let got = map_frame(&mut rig, &empty, &h, tex, &v, 1.0, &|s| {
        s.relief.shading = 0.0;
        s.roads.on = false;
    });
    want.save("flat_2d_base.png");
    got.save("flat_3d_base.png");
    let b = inside(&v, 1.0);
    let (mut sum, mut n) = (0u64, 0u64);
    for y in b[1]..b[3] {
        for x in b[0]..b[2] {
            let (p, q) = (want.at(x, y), got.at(x, y));
            sum += (0..3).map(|k| (p[k] as i32 - q[k] as i32).unsigned_abs() as u64).sum::<u64>();
            n += 3;
        }
    }
    let mae = sum as f64 / n as f64;
    eprintln!("flat world, 3D vs tilted 2D base: mean abs error {mae:.2} / 255 over {} px", n / 3);
    assert!(mae < 6.0, "the flat 3D map differs from the tilted 2D one by {mae:.2}/255");

    // A road along x at z = 0, 3 px wide: its pixels centre on Camera::project of its centreline.
    let mut roads = RoadLayer::default();
    roads.by_type[1].push(Chain::new(vec![[-300.0, 0.0], [300.0, 0.0]], vec![100.0, 100.0]));
    let layers = Arc::new(MapLayers { rev: 1, roads: Arc::new(roads), ..Default::default() });
    let mesh = Arc::new(RoadMesh::build(&layers.roads, &flat, 1));
    let one = World { layers, mesh, terrain: flat.clone(), image: w.image.clone(), cal: w.cal, orig: w.orig };
    v.car = (-60.0, -60.0);
    v.yaw = 0.0;
    let cam = camera(&one, &v);
    let on = |s: &mut Scene3d| {
        s.relief.shading = 0.0;
        s.roads.min_px = 3.0;
        s.roads.max_px = 3.0;
        s.roads.casing_px = 0.0;
        s.roads.styles.road.casing = false;
    };
    for _ in 0..8 {
        map_frame(&mut rig, &one, &h, tex, &v, 1.0, &on);
    }
    let o = map_frame(&mut rig, &one, &h, tex, &v, 1.0, &on);
    o.save("flat_3d_road.png");
    let c = rgb(RoadsCfg::default().styles.road.color);
    let p = cam.project(40.0, 0.0).expect("in front");
    // centroid of road-coloured pixels in the column of p
    let x = p.x.round() as usize;
    let (mut sy, mut sw) = (0.0f64, 0.0f64);
    for y in 0..o.h {
        for xx in x - 1..=x + 1 {
            let q = o.at(xx, y);
            let d: i32 = (0..3).map(|k| (q[k] as i32 - c[k] as i32).abs()).sum();
            if d < 60 {
                sy += y as f64 + 0.5;
                sw += 1.0;
            }
        }
    }
    assert!(sw > 3.0, "the road is not on screen");
    let cy = sy / sw;
    eprintln!("road centre row {cy:.2} vs Camera::project {:.2}", p.y);
    assert!((cy - p.y as f64).abs() < 0.8, "the road lies {:.2} px from Camera::project", (cy - p.y as f64).abs());
    rig.finish(&h);
}

// ── in-race focus ────────────────────────────────────────────────────────────────────────────

#[test]
#[ignore = "needs an EGL device"]
fn gl3d_in_race_focus_mutes_or_hides_the_other_roads() {
    let Some(mut rig) = open(Flavour::Default, None, [620, 420]) else { return };
    let w = world();
    let (_hold, tex) = rig.load_map(&w, TextureOptions::LINEAR);
    // Focus: the Road chain is on the race line, every other chain is not.
    let mut focus = crate::maprender::racesel::RoadFocus::default();
    for (slot, chains) in w.layers.roads.by_type.iter().enumerate() {
        for (ci, ch) in chains.iter().enumerate() {
            focus.runs[slot].push(Run { chain: ci as u32, a: 0, b: ch.pts.len() as u32 - 1, relevant: slot == RoadType::Road.index() as usize, bbox: ch.bbox });
        }
    }
    focus.jumps = vec![false; w.layers.roads.jumps.len()];
    let focus = Arc::new(focus);
    let styles = RoadsCfg::default().styles;
    let count = |o: &Out, ty: RoadType| o.count_near([0, 0, 620, 420], rgb(styles.get(ty).unwrap().color), 30);
    let mut counts = vec![];
    for mode in [OtherRoads::Normal, OtherRoads::Muted, OtherRoads::Hidden] {
        let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
        let f = Focus3d { focus: focus.clone(), cfg: RaceFocusCfg { other_roads: mode, ..Default::default() } };
        let mut v = View::dashboard();
        v.no_3d = false;
        let mut last = None;
        for _ in 0..8 {
            last = Some(map_frame(&mut rig, &w, &h, tex, &v, 1.0, &|s| {
                thick(s);
                s.focus = Some(f.clone());
            }));
        }
        let o = last.unwrap();
        o.save(&format!("focus_{mode:?}.png"));
        counts.push((mode, count(&o, RoadType::Road), count(&o, RoadType::Offroad), count(&o, RoadType::Highway)));
        h.destroy(&rig.gl);
    }
    eprintln!("focus counts (mode, road, offroad, highway): {counts:?}");
    let (normal, muted, hidden) = (counts[0], counts[1], counts[2]);
    assert!(normal.2 > 20 && normal.3 > 20, "unfocused: other roads in their colours {normal:?}");
    assert!(muted.1 > 20, "the road on the race line keeps its colour when others are muted");
    assert!(muted.2 < 5 && muted.3 < 5, "muted roads lose their type colour {muted:?}");
    assert!(hidden.1 > 20 && hidden.2 < 5 && hidden.3 < 5, "hidden roads are gone {hidden:?}");
    rig.finish(&Gl3dHandle::new());
}

// ── the race road (D80) and "race road only" (D82) ──────────────────────────────────────────

/// The race lines of a scene (D88): `lines` and `marks` as `RaceSel::race_draw` would hand them
/// over, their mesh built here (in the app `store::race_mesh` does it off-thread).
fn race3d(w: &World, lines: Vec<RaceRoad>, marks: Vec<RaceMark>, cfg: crate::maprender::cfg::RaceCfg) -> Race3d {
    let draw = Arc::new(RaceDraw { lines, marks });
    let mesh = Some(Arc::new(crate::maprender::mesh3d::RoadMesh::race_roads(&draw.lines, &w.terrain)));
    Race3d { draw, mesh, cfg }
}

/// A race line along chain 0 of `slot` (its points, the driving line 0.3 m above the heights).
fn race_along(w: &World, slot: RoadType) -> RaceRoad {
    let ch = &w.layers.roads.by_type[slot.index() as usize][0];
    RaceRoad { pts: ch.pts.clone(), y: ch.y.iter().map(|y| y + 0.3).collect(), closed: false }
}

/// The focus of the synthetic world with chain 0 of `slot` relevant, every other one not.
fn race_focus(w: &World, slot: RoadType) -> Arc<crate::maprender::racesel::RoadFocus> {
    let mut focus = crate::maprender::racesel::RoadFocus::default();
    for (s, chains) in w.layers.roads.by_type.iter().enumerate() {
        for (ci, ch) in chains.iter().enumerate() {
            focus.runs[s].push(Run { chain: ci as u32, a: 0, b: ch.pts.len() as u32 - 1, relevant: s == slot.index() as usize && ci == 0, bbox: ch.bbox });
        }
    }
    focus.jumps = vec![false; w.layers.roads.jumps.len()];
    Arc::new(focus)
}

#[test]
#[ignore = "needs an EGL device; writes PNGs (GL3D_PNG_DIR)"]
fn gl3d_race_road_over_the_roads_and_race_only() {
    let Some(mut rig) = open(Flavour::Default, None, [620, 420]) else { return };
    let w = world();
    let (_hold, tex) = rig.load_map(&w, TextureOptions::LINEAR);
    let race = crate::maprender::cfg::RaceCfg::default().color;
    let styles = RoadsCfg::default().styles;
    let count = |o: &Out, c: [u8; 3]| o.count_near([0, 0, 620, 420], c, 30);
    let mut v = View::dashboard();
    v.no_3d = false;
    // (name, race road along, view centre, other roads)
    let cases = [
        ("race_road_muted", RoadType::Road, (0.0, -250.0), OtherRoads::Muted),
        ("race_road_hidden", RoadType::Road, (0.0, -250.0), OtherRoads::Hidden),
        ("race_road_only", RoadType::Road, (0.0, -250.0), OtherRoads::RaceOnly),
        ("race_road_tunnel", RoadType::Tunnel, (-300.0, 0.0), OtherRoads::Muted),
    ];
    let mut counts = vec![];
    for (name, slot, car, mode) in cases {
        let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
        let f = Focus3d { focus: race_focus(&w, slot), cfg: RaceFocusCfg { other_roads: mode, ..Default::default() } };
        let r = race3d(&w, vec![race_along(&w, slot)], vec![], crate::maprender::cfg::RaceCfg::default());
        v.car = car;
        let mut last = None;
        for _ in 0..8 {
            last = Some(map_frame(&mut rig, &w, &h, tex, &v, 1.0, &|s| {
                thick(s);
                s.focus = Some(f.clone());
                s.race = Some(r.clone());
            }));
        }
        let o = last.unwrap();
        assert_eq!(o.gl_error, 0, "{name}");
        o.save(&format!("{name}.png"));
        let c = (name, count(&o, race.0), count(&o, rgb(styles.road.color)), count(&o, rgb(styles.offroad.color)), count(&o, rgb(styles.highway.color)), h.stats().last.triangles);
        eprintln!("{c:?}");
        counts.push(c);
        h.destroy(&rig.gl);
    }
    let (muted, hidden, only, tunnel) = (counts[0], counts[1], counts[2], counts[3]);
    assert!(muted.1 > 300 && hidden.1 > 300 && only.1 > 300, "the race road is drawn {counts:?}");
    assert!(muted.2 < 20, "the road under the race road is covered by it {muted:?}");
    assert!(only.2 < 5 && only.3 < 5 && only.4 < 5, "race road only: no road of the road mesh {only:?}");
    // Hidden still draws the road mesh (its hidden pieces discarded); race-only draws the terrain
    // and the race mesh alone: the road mesh's triangles (two passes) are gone.
    let road_tris = w.mesh.triangles().0;
    assert!(hidden.5 >= only.5 + road_tris / 2, "race only: no road mesh triangles {hidden:?} {only:?} (road mesh {road_tris})");
    assert!(tunnel.1 > 200, "the race road through the hill is drawn over it like a tunnel {tunnel:?}");
    rig.finish(&Gl3dHandle::new());
}

/// D88 (the user: "occlusion should work for the race circuit as well"): an open stretch of the
/// race road (not a tunnel: the driving line is on the ground) runs on the far side of the big
/// hill. It is drawn where nothing is in its way, and not at all where the hill is in front of it,
/// like every other road.
#[test]
#[ignore = "needs an EGL device; writes PNGs (GL3D_PNG_DIR)"]
fn gl3d_race_road_is_hidden_behind_hills() {
    let Some(mut rig) = open(Flavour::Default, None, [620, 420]) else { return };
    let w = world();
    let (_hold, tex) = rig.load_map(&w, TextureOptions::LINEAR);
    let race = crate::maprender::cfg::RaceCfg::default().color;
    let count = |o: &Out| o.count_near([0, 0, 620, 420], race.0, 30);
    let mut v = View::dashboard();
    v.no_3d = false;
    v.angle = 75.0;
    v.zoom = 300.0;
    // The same straight race road (z = 440, x = -440..-160, on the ground, "race road only" so it
    // is the only road in the picture) seen from the far side of the big hill (plain view: the
    // control, it is drawn) and from the near side (the hill is in the way).
    let mut counts = vec![];
    for (name, car, yaw) in [("race_road_plain_view", (-300.0f32, 700.0f32), std::f32::consts::PI), ("race_road_behind_hill", (-300.0, -20.0), 0.0)] {
        v.car = car;
        v.yaw = yaw;
        let pts: Vec<[f32; 2]> = (0..=14).map(|i| [-440.0 + 20.0 * i as f32, 440.0]).collect();
        let y: Vec<f32> = pts.iter().map(|p| w.terrain.height(p[0], p[1]) + 0.3).collect();
        let mut focus = crate::maprender::racesel::RoadFocus::default();
        focus.jumps = vec![false; w.layers.roads.jumps.len()];
        let r = race3d(&w, vec![RaceRoad { pts, y, closed: false }], vec![], crate::maprender::cfg::RaceCfg::default());
        let f = Focus3d { focus: Arc::new(focus), cfg: RaceFocusCfg { other_roads: OtherRoads::RaceOnly, ..Default::default() } };
        let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
        let mut last = None;
        for _ in 0..8 {
            last = Some(map_frame(&mut rig, &w, &h, tex, &v, 1.0, &|s| {
                s.roads.min_px = 14.0; // wide, so the race road is easy to count
                s.roads.max_px = 20.0;
                s.focus = Some(f.clone());
                s.race = Some(r.clone());
            }));
        }
        let o = last.unwrap();
        assert_eq!(o.gl_error, 0, "{name}");
        o.save(&format!("{name}.png"));
        eprintln!("{name}: {} race pixels", count(&o));
        counts.push(count(&o));
        h.destroy(&rig.gl);
    }
    assert!(counts[0] > 200, "the control: the race road in plain view is drawn ({counts:?})");
    assert!(counts[1] < 10, "the race road behind the hill is hidden by it ({counts:?})");

    // The same for a road above it: the race road runs along the elevated highway (22 m up) on the
    // ground under it, then again on the deck itself (the control: nothing covers it).
    v.car = (600.0, 120.0);
    v.yaw = 0.0;
    v.angle = 12.0; // from nearly straight above
    v.zoom = 250.0;
    let hw = &w.layers.roads.by_type[RoadType::Highway.index() as usize][0];
    let pts: Vec<[f32; 2]> = hw.pts.iter().copied().filter(|p| p[0] > 380.0 && p[0] < 820.0).collect();
    let mut under = vec![];
    for (name, lift) in [("race_road_on_the_deck", 22.3f32), ("race_road_under_the_deck", 0.3)] {
        let y: Vec<f32> = pts.iter().map(|p| w.terrain.height(p[0], p[1]) + lift).collect();
        let r = race3d(&w, vec![RaceRoad { pts: pts.clone(), y, closed: false }], vec![], crate::maprender::cfg::RaceCfg::default());
        let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
        let mut last = None;
        for _ in 0..8 {
            last = Some(map_frame(&mut rig, &w, &h, tex, &v, 1.0, &|s| {
                s.roads.min_px = 8.0;
                s.roads.max_px = 12.0;
                s.race = Some(r.clone());
            }));
        }
        let o = last.unwrap();
        assert_eq!(o.gl_error, 0, "{name}");
        o.save(&format!("{name}.png"));
        eprintln!("{name}: {} race pixels", count(&o));
        under.push(count(&o));
        h.destroy(&rig.gl);
    }
    assert!(under[0] > 200, "the control: the race road on the deck is drawn ({under:?})");
    // (what peeks out below the deck is the parallax of the 22 m between them; without the depth test the whole road would be drawn: as many as on the deck)
    assert!(under[1] * 3 < under[0] * 2, "the deck above the race road covers it ({under:?})");
    rig.finish(&Gl3dHandle::new());
}

/// D88, the rest of the race lines: whatever the race-line mode draws is in the scene, so the big
/// hill hides it like the roads. Each case is seen from the far side of the hill (the control:
/// drawn) and from the near side (hidden): the **Road** style of lines that are not the in-race
/// focus (modes `nearest` / `near` / `all`: no focus at all here), the thin **Line** style (a
/// ribbon of the configured width), and the start / finish **marks** (posts, a green one for a
/// sprint's start). No roads are drawn, so only the race colour / the mark colour is counted.
#[test]
#[ignore = "needs an EGL device; writes PNGs (GL3D_PNG_DIR)"]
fn gl3d_race_lines_and_marks_are_hidden_behind_hills() {
    use crate::maprender::cfg::{RaceCfg, RouteStyle};
    let Some(mut rig) = open(Flavour::Default, None, [620, 420]) else { return };
    let w = world();
    let (_hold, tex) = rig.load_map(&w, TextureOptions::LINEAR);
    let mut v = View::dashboard();
    v.no_3d = false;
    v.angle = 75.0;
    v.zoom = 300.0;
    // Two lines of the same set, as `All` / `Near` would draw them: z = 440 and z = 480.
    let line = |z: f32| {
        let pts: Vec<[f32; 2]> = (0..=14).map(|i| [-440.0 + 20.0 * i as f32, z]).collect();
        let y: Vec<f32> = pts.iter().map(|p| w.terrain.height(p[0], p[1]) + 0.3).collect();
        RaceRoad { pts, y, closed: false }
    };
    let posts: Vec<RaceMark> = (0..5).map(|i| RaceMark { at: [-440.0 + 70.0 * i as f32, 440.0], y: w.terrain.height(-440.0 + 70.0 * i as f32, 440.0) + 0.3, kind: MarkKind::SprintStart }).collect();
    let base = RaceCfg::default();
    let mark_colour = crate::maprender::style::START_DOT.to_array();
    // (name, lines, marks, cfg, the colour that is counted)
    let cases: Vec<(&str, Vec<RaceRoad>, Vec<RaceMark>, RaceCfg, [u8; 3])> = vec![
        ("road_all", vec![line(440.0), line(480.0)], vec![], base, base.color.0),
        ("thin_line", vec![line(440.0), line(480.0)], vec![], RaceCfg { route: RouteStyle::Line, width_px: 6.0, alpha: 1.0, ..base }, base.color.0),
        ("marks", vec![], posts, base, [mark_colour[0], mark_colour[1], mark_colour[2]]),
    ];
    for (name, lines, marks, cfg, colour) in cases {
        let r = race3d(&w, lines, marks, cfg);
        let mut counts = vec![];
        for (view, car, yaw) in [("plain", (-300.0f32, 700.0f32), std::f32::consts::PI), ("behind_hill", (-300.0, -20.0), 0.0)] {
            v.car = car;
            v.yaw = yaw;
            let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
            let mut last = None;
            for _ in 0..8 {
                last = Some(map_frame(&mut rig, &w, &h, tex, &v, 1.0, &|s| {
                    s.roads.on = false;
                    s.roads.min_px = 14.0; // wide race roads, easy to count
                    s.roads.max_px = 20.0;
                    s.race = Some(r.clone());
                }));
            }
            let o = last.unwrap();
            assert_eq!(o.gl_error, 0, "{name} {view}");
            o.save(&format!("race_{name}_{view}.png"));
            let n = o.count_near([0, 0, 620, 420], colour, 30);
            eprintln!("{name} {view}: {n} pixels");
            counts.push(n);
            h.destroy(&rig.gl);
        }
        assert!(counts[0] > 100, "{name}: the control (plain view) is drawn ({counts:?})");
        assert!(counts[1] < 10, "{name}: hidden behind the hill ({counts:?})");
    }
    rig.finish(&Gl3dHandle::new());
}

// ── the navigation route (phase L) ───────────────────────────────────────────────────────────

/// A navigation line over the synthetic world: `pts` at `lift` m above the ground (0 = unknown
/// heights), `kinds` per segment.
fn nav_line(w: &World, rev: u64, pts: Vec<[f32; 2]>, lift: Option<f32>, kinds: Vec<u8>) -> Arc<crate::nav::NavLine> {
    let y = pts.iter().map(|p| lift.map_or(0.0, |l| w.terrain.height(p[0], p[1]) + l)).collect();
    let kinds = if kinds.is_empty() { vec![2; pts.len() - 1] } else { kinds };
    Arc::new(crate::nav::NavLine { rev, pts, y, seg_kind: kinds })
}

fn route3d(w: &World, line: &Arc<crate::nav::NavLine>) -> Route3d {
    Route3d::new(line, &w.terrain, NavRouteCfg::default()).expect("a route mesh")
}

/// The default route colour (fuchsia): nothing else in the synthetic world is near it.
fn route_rgb() -> [u8; 3] {
    NavRouteCfg::default().color.0
}

/// Eight frames of `v` with `tweak` applied, the last one back; the handle is destroyed.
fn settled(rig: &mut Rig, w: &World, tex: MapTex, v: &View, name: &str, tweak: &dyn Fn(&mut Scene3d)) -> Out {
    let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
    let mut last = None;
    for _ in 0..8 {
        last = Some(map_frame(rig, w, &h, tex, v, 1.0, tweak));
    }
    let o = last.unwrap();
    assert_eq!(o.gl_error, 0, "{name}");
    o.save(&format!("{name}.png"));
    h.destroy(&rig.gl);
    o
}

/// L3: the route is a road of its own in 3D: over the roads it runs on (covering them), still
/// drawn when "race road only" hides the road mesh, through a hill like a tunnel, and a jump
/// stretch is a dashed line over the gap instead of a deck.
#[test]
#[ignore = "needs an EGL device; writes PNGs (GL3D_PNG_DIR)"]
fn gl3d_nav_route_over_the_roads_and_race_only() {
    let Some(mut rig) = open(Flavour::Default, None, [620, 420]) else { return };
    let w = world();
    let (_hold, tex) = rig.load_map(&w, TextureOptions::LINEAR);
    let styles = RoadsCfg::default().styles;
    let race = crate::maprender::cfg::RaceCfg::default().color;
    let count = |o: &Out, c: [u8; 3]| o.count_near([0, 0, 620, 420], c, 30);
    let mut v = View::dashboard();
    v.no_3d = false;

    // Over the road it runs on: the route's colour is drawn, the road under it is covered.
    let ch = &w.layers.roads.by_type[RoadType::Road.index() as usize][0];
    let line = Arc::new(crate::nav::NavLine { rev: 1, pts: ch.pts.clone(), y: ch.y.iter().map(|y| y + 0.3).collect(), seg_kind: vec![2; ch.pts.len() - 1] });
    let r = route3d(&w, &line);
    v.car = (0.0, -250.0);
    let o = settled(&mut rig, &w, tex, &v, "nav_route_over_road", &|s| {
        thick(s);
        s.route = Some(r.clone());
    });
    let (on, road_px) = (count(&o, route_rgb()), count(&o, rgb(styles.road.color)));
    eprintln!("route over the road: {on} route px, {road_px} road px");
    assert!(on > 300, "the route is drawn ({on})");
    assert!(road_px < 20, "the road under the route is covered by it ({road_px})");
    let none = settled(&mut rig, &w, tex, &v, "nav_route_off", &|s| thick(s));
    assert_eq!(count(&none, route_rgb()), 0, "no route, none of its colour");

    // "Race road only" (D82) hides the road mesh while a race road is there; the route is not a
    // road of the mesh and stays. Race road along the offroad chain (z = -200), route along the road (z = -300).
    let off = &w.layers.roads.by_type[RoadType::Offroad.index() as usize][0];
    let rr = race3d(&w, vec![RaceRoad { pts: off.pts.clone(), y: off.y.iter().map(|y| y + 0.3).collect(), closed: false }], vec![], crate::maprender::cfg::RaceCfg::default());
    let mut focus = crate::maprender::racesel::RoadFocus::default();
    focus.jumps = vec![false; w.layers.roads.jumps.len()];
    let f = Focus3d { focus: Arc::new(focus), cfg: RaceFocusCfg { other_roads: OtherRoads::RaceOnly, ..Default::default() } };
    let o = settled(&mut rig, &w, tex, &v, "nav_route_race_only", &|s| {
        thick(s);
        s.focus = Some(f.clone());
        s.race = Some(rr.clone());
        s.route = Some(r.clone());
    });
    let (rt, rc, road_px) = (count(&o, route_rgb()), count(&o, race.0), count(&o, rgb(styles.road.color)));
    eprintln!("race road only: route {rt}, race road {rc}, road {road_px}");
    assert!(rt > 300 && rc > 300, "both the race road and the route are drawn (route {rt}, race {rc})");
    assert!(road_px < 5, "race road only: no road of the road mesh ({road_px})");

    // Through the big hill: the tunnel stretch shows through it, like a road tunnel.
    let tun = &w.layers.roads.by_type[RoadType::Tunnel.index() as usize][0];
    let line = Arc::new(crate::nav::NavLine { rev: 2, pts: tun.pts.clone(), y: tun.y.iter().map(|y| y + 0.3).collect(), seg_kind: vec![11; tun.pts.len() - 1] });
    let rt3 = route3d(&w, &line);
    assert!(!rt3.mesh.samples.is_empty() && rt3.mesh.samples.iter().any(|s| s.slot == crate::maprender::mesh3d::SLOT_TUNNEL), "the stretch under the hill is in the tunnel slot");
    v.car = (-300.0, 0.0);
    let o = settled(&mut rig, &w, tex, &v, "nav_route_tunnel", &|s| {
        thick(s);
        s.roads.on = false;
        s.route = Some(rt3.clone());
    });
    let n = count(&o, route_rgb());
    eprintln!("tunnel: {n} route px");
    assert!(n > 100, "the route through the hill is drawn over it like a tunnel ({n})");

    // A jump stretch: road, gap, road. The gap over the small hill at (50, 450) is a dashed line
    // in the route colour (some pixels, far fewer than a deck's worth) and no deck.
    let pts: Vec<[f32; 2]> = vec![[-400.0, 450.0], [-300.0, 450.0], [-120.0, 450.0], [220.0, 450.0], [320.0, 450.0], [420.0, 450.0]];
    let line = nav_line(&w, 3, pts, Some(0.3), vec![2, 2, crate::maprender::style::NAV_SEG_JUMP, 2, 2]);
    let rj = route3d(&w, &line);
    assert!(rj.mesh.samples.iter().any(|s| s.slot == crate::maprender::mesh3d::SLOT_JUMP) && !rj.mesh.samples.iter().any(|s| s.slot != crate::maprender::mesh3d::SLOT_JUMP && s.x > -119.0 && s.x < 219.0));
    v.car = (50.0, 450.0);
    v.zoom = 450.0;
    v.yaw = 0.0;
    let o = settled(&mut rig, &w, tex, &v, "nav_route_jump", &|s| {
        s.roads.on = false;
        s.route = Some(rj.clone());
    });
    let gap_px = o.count_near([0, 0, 620, 420], route_rgb(), 30);
    // the same route as one unbroken road (no jump) for scale
    let full = nav_line(&w, 4, vec![[-400.0, 450.0], [-300.0, 450.0], [-120.0, 450.0], [220.0, 450.0], [320.0, 450.0], [420.0, 450.0]], Some(0.3), vec![]);
    let rf = route3d(&w, &full);
    let o2 = settled(&mut rig, &w, tex, &v, "nav_route_jump_as_road", &|s| {
        s.roads.on = false;
        s.route = Some(rf.clone());
    });
    let road_px = o2.count_near([0, 0, 620, 420], route_rgb(), 30);
    eprintln!("jump: {gap_px} px with a dashed gap vs {road_px} px with a road over it");
    assert!(gap_px > 100 && gap_px * 10 < road_px * 9, "the gap is dashes, not a deck ({gap_px} vs {road_px})");
    rig.finish(&Gl3dHandle::new());
}

/// L3, like the race road (D88): occlusion. A route on the far side of the big hill is hidden by it
/// (seen from the near side), drawn from the other side; a road deck above covers a route under it.
#[test]
#[ignore = "needs an EGL device; writes PNGs (GL3D_PNG_DIR)"]
fn gl3d_nav_route_is_hidden_behind_hills_and_under_decks() {
    let Some(mut rig) = open(Flavour::Default, None, [620, 420]) else { return };
    let w = world();
    let (_hold, tex) = rig.load_map(&w, TextureOptions::LINEAR);
    let count = |o: &Out| o.count_near([0, 0, 620, 420], route_rgb(), 30);
    let mut v = View::dashboard();
    v.no_3d = false;
    v.angle = 75.0;
    v.zoom = 300.0;
    let pts: Vec<[f32; 2]> = (0..=14).map(|i| [-440.0 + 20.0 * i as f32, 440.0]).collect();
    let line = nav_line(&w, 1, pts, Some(0.3), vec![]);
    let r = route3d(&w, &line);
    let mut counts = vec![];
    for (name, car, yaw) in [("nav_route_plain_view", (-300.0f32, 700.0f32), std::f32::consts::PI), ("nav_route_behind_hill", (-300.0, -20.0), 0.0)] {
        v.car = car;
        v.yaw = yaw;
        let o = settled(&mut rig, &w, tex, &v, name, &|s| {
            s.roads.on = false;
            s.roads.min_px = 14.0; // wide, so the route is easy to count
            s.roads.max_px = 20.0;
            s.route = Some(r.clone());
        });
        eprintln!("{name}: {} route px", count(&o));
        counts.push(count(&o));
    }
    assert!(counts[0] > 200, "the control: the route in plain view is drawn ({counts:?})");
    assert!(counts[1] < 10, "the route behind the hill is hidden by it ({counts:?})");

    // Under the elevated highway (22 m up): on the deck itself (the control) and on the ground under it.
    v.car = (600.0, 120.0);
    v.yaw = 0.0;
    v.angle = 12.0;
    v.zoom = 250.0;
    let hw = &w.layers.roads.by_type[RoadType::Highway.index() as usize][0];
    let pts: Vec<[f32; 2]> = hw.pts.iter().copied().filter(|p| p[0] > 380.0 && p[0] < 820.0).collect();
    let mut under = vec![];
    for (i, (name, lift)) in [("nav_route_on_the_deck", 22.3f32), ("nav_route_under_the_deck", 0.3)].into_iter().enumerate() {
        let line = nav_line(&w, 10 + i as u64, pts.clone(), Some(lift), vec![]); // (a different rev: the mesh cache is keyed on it)
        let r = route3d(&w, &line);
        let o = settled(&mut rig, &w, tex, &v, name, &|s| {
            s.roads.min_px = 8.0;
            s.roads.max_px = 12.0;
            s.route = Some(r.clone());
        });
        eprintln!("{name}: {} route px", count(&o));
        under.push(count(&o));
    }
    assert!(under[0] > 200, "the control: the route on the deck is drawn ({under:?})");
    assert!(under[1] * 3 < under[0] * 2, "the deck above the route covers it ({under:?})");
    rig.finish(&Gl3dHandle::new());
}

/// L3: the route mesh is built once per `NavLine::rev` (a new route or the next 150 m chunk) and
/// uploaded once per mesh: the same line over many frames builds and uploads nothing more, a new
/// `rev` does exactly one of each, a changed look (colour, width) neither.
#[test]
#[ignore = "needs an EGL device; writes PNGs (GL3D_PNG_DIR)"]
fn gl3d_nav_route_mesh_is_rebuilt_only_when_the_line_changes() {
    use std::sync::atomic::Ordering::Relaxed;
    let Some(mut rig) = open(Flavour::Default, None, [620, 420]) else { return };
    let w = world();
    let (_hold, tex) = rig.load_map(&w, TextureOptions::LINEAR);
    let mut v = View::dashboard();
    v.no_3d = false;
    v.car = (0.0, -250.0);
    let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
    let gpu = |h: &Gl3dHandle| h.lock().gl3d.as_ref().and_then(|g| g.nav.as_ref().map(|n| Arc::as_ptr(&n.mesh) as usize));
    let ch = &w.layers.roads.by_type[RoadType::Road.index() as usize][0];
    let mk = |rev: u64, from: usize| Arc::new(crate::nav::NavLine { rev, pts: ch.pts[from..].to_vec(), y: ch.y[from..].iter().map(|y| y + 0.3).collect(), seg_kind: vec![2; ch.pts.len() - from - 1] });
    let builds0 = NAV_MESH_BUILDS.load(Relaxed);
    let mut frame = |line: &Arc<crate::nav::NavLine>, cfg: NavRouteCfg| {
        // (every frame goes through `Route3d::new`, as the call sites do per frame)
        let r = Route3d::new(line, &w.terrain, cfg).unwrap();
        let o = map_frame(&mut rig, &w, &h, tex, &v, 1.0, &|s| {
            thick(s);
            s.route = Some(r.clone());
        });
        assert_eq!(o.gl_error, 0);
        o
    };
    let a = mk(900_001, 0);
    for _ in 0..12 {
        frame(&a, NavRouteCfg::default());
    }
    let first = gpu(&h).expect("the route is on the GPU");
    assert_eq!(NAV_MESH_BUILDS.load(Relaxed) - builds0, 1, "one build for twelve frames of one line");
    for _ in 0..20 {
        frame(&a, NavRouteCfg::default());
    }
    assert_eq!((NAV_MESH_BUILDS.load(Relaxed) - builds0, gpu(&h)), (1, Some(first)), "no rebuild, no re-upload while the line is the same");
    // Colour and width are uniforms: no rebuild, no re-upload, and the picture changes.
    let before = frame(&a, NavRouteCfg::default());
    let green = NavRouteCfg { color: crate::maprender::cfg::Rgb([0, 255, 0]), width: 1.5, ..NavRouteCfg::default() };
    let after = frame(&a, green);
    assert_eq!((NAV_MESH_BUILDS.load(Relaxed) - builds0, gpu(&h)), (1, Some(first)));
    assert!(before.count_near([0, 0, 620, 420], route_rgb(), 30) > 300 && after.count_near([0, 0, 620, 420], [0, 255, 0], 30) > 300 && after.count_near([0, 0, 620, 420], route_rgb(), 30) == 0);
    // The next chunk (a new rev): exactly one more build, and the GPU copy is replaced.
    let b = mk(900_002, 3);
    for _ in 0..6 {
        frame(&b, NavRouteCfg::default());
    }
    assert_eq!(NAV_MESH_BUILDS.load(Relaxed) - builds0, 2, "one more build for the new rev");
    assert!(gpu(&h).is_some_and(|p| p != first), "the new mesh is on the GPU");
    // No route: the GPU copy is dropped.
    for _ in 0..3 {
        map_frame(&mut rig, &w, &h, tex, &v, 1.0, &|s| thick(s));
    }
    assert_eq!(gpu(&h), None);
    rig.finish(&h);
}

/// The one mesh cache is shared by the maps and keyed on the line's rev and the terrain's (CPU only).
#[test]
fn route3d_builds_once_per_rev_and_terrain() {
    use std::sync::atomic::Ordering::Relaxed;
    let w = world();
    let line = |rev: u64, n: usize| Arc::new(crate::nav::NavLine { rev, pts: (0..n).map(|i| [10.0 * i as f32, -600.0]).collect(), y: vec![0.0; n], seg_kind: vec![2; n - 1] });
    let (a, b) = (line(700_001, 30), line(700_002, 30));
    let n0 = NAV_MESH_BUILDS.load(Relaxed);
    let r1 = Route3d::new(&a, &w.terrain, NavRouteCfg::default()).unwrap();
    let r2 = Route3d::new(&a, &w.terrain, NavRouteCfg { width: 2.0, ..NavRouteCfg::default() }).unwrap();
    assert!(Arc::ptr_eq(&r1.mesh, &r2.mesh), "same rev: the cached mesh, whatever the look");
    let r3 = Route3d::new(&b, &w.terrain, NavRouteCfg::default()).unwrap();
    assert!(!Arc::ptr_eq(&r1.mesh, &r3.mesh), "a new rev: a new mesh");
    assert_eq!(NAV_MESH_BUILDS.load(Relaxed) - n0, 2);
    // Another terrain (a new `rev`) rebuilds.
    let mut t2 = Terrain::synthetic();
    t2.rev += 1;
    let r4 = Route3d::new(&b, &t2, NavRouteCfg::default()).unwrap();
    assert!(!Arc::ptr_eq(&r3.mesh, &r4.mesh));
    // Fewer than two points: no route.
    assert!(Route3d::new(&line(700_003, 1), &w.terrain, NavRouteCfg::default()).is_none());
}

/// Real routes over the island (the user's road data, all filters on so a jump can be used):
/// across the whole island (node 1 to the farthest node), a medium one (about 5 km away), and one
/// that flies a jump.
fn real_routes(w: &World) -> Vec<(&'static str, crate::nav::Route)> {
    use crate::nav::{RouteFilters, RoutePrefs};
    let g = &w.layers.route_graph;
    let prefs = RoutePrefs { filters: RouteFilters::ALL, curves: 0.0 };
    let n0 = g.node_index(1).expect("node 1");
    let p0 = g.node_pos(n0);
    let dist = |n: u32| (g.node_pos(n)[0] - p0[0]).hypot(g.node_pos(n)[1] - p0[1]);
    let far = (0..g.node_count() as u32).max_by(|&a, &b| dist(a).total_cmp(&dist(b))).expect("a node");
    let mid = (0..g.node_count() as u32).min_by(|&a, &b| (dist(a) - 5000.0).abs().total_cmp(&(dist(b) - 5000.0).abs())).expect("a node");
    let plan = |a: [f32; 3], b: [f32; 3]| g.plan((a[0], a[1], Some(a[2])), (b[0], b[1]), &prefs).expect("a route");
    let mut out = vec![("island", plan(p0, g.node_pos(far))), ("5km", plan(p0, g.node_pos(mid)))];
    for j in w.layers.roads.jumps.iter() {
        // ~300 m before the take-off to ~300 m after the landing, at the fixed take-off / landing.
        let (a, b) = ([j[0], j[1], j[2]], [j[3], j[4], j[5]]);
        if let Ok(r) = g.plan((a[0], a[1], Some(a[2])), (b[0], b[1]), &prefs) {
            if r.seg_kind.contains(&crate::maprender::style::NAV_SEG_JUMP) {
                out.push(("jump", r));
                break;
            }
        }
    }
    out
}

/// Real install, CPU only: what the route mesh costs. Printed per route (points, samples,
/// triangles, build ms; run `--release` for the numbers that matter), and the shape asserted: jump
/// stretches in the jump slot with no road deck over them, tunnels in the tunnel slot.
/// `FH6_INSTALL_DIR=... cargo test --release real_install_nav_route_mesh -- --ignored --nocapture`
#[test]
#[ignore = "needs an FH6 install"]
fn real_install_nav_route_mesh_cost() {
    let Some(w) = real_world() else {
        eprintln!("SKIP real_install_nav_route_mesh_cost: no FH6 install");
        return;
    };
    for (name, r) in real_routes(&w) {
        let mut best = std::time::Duration::MAX;
        let mut mesh = None;
        for _ in 0..7 {
            let t = std::time::Instant::now();
            let m = RoadMesh::nav_route(&r.pts, &r.y, &r.seg_kind, &w.terrain);
            best = best.min(t.elapsed());
            mesh = Some(m);
        }
        let m = mesh.unwrap();
        let slot = |s: u8| m.samples.iter().filter(|x| x.slot == s).count();
        let jumps = r.seg_kind.iter().filter(|&&k| k == crate::maprender::style::NAV_SEG_JUMP).count();
        eprintln!(
            "route {name}: {:.1} km, {} points, {} jump segments -> {} samples ({} tunnel, {} jump), {} + {} triangles, {} KB of vertices; build best of 7 {:.2} ms ({})",
            r.dist_m / 1000.0,
            r.pts.len(),
            jumps,
            m.samples.len(),
            slot(crate::maprender::mesh3d::SLOT_TUNNEL),
            slot(crate::maprender::mesh3d::SLOT_JUMP),
            m.triangles().0,
            m.triangles().1,
            m.vertices.len() / 1024,
            best.as_secs_f64() * 1e3,
            if cfg!(debug_assertions) { "debug" } else { "release" }
        );
        assert!(!m.samples.is_empty());
        assert_eq!(jumps > 0, slot(crate::maprender::mesh3d::SLOT_JUMP) > 0, "{name}: jump stretches are in the jump slot");
        // The trailing part a chunk later (a new `NavLine::rev` every 150 m): the same order of cost.
        let cut = (150.0 / r.dist_m * r.pts.len() as f32) as usize;
        let t = std::time::Instant::now();
        let m2 = RoadMesh::nav_route(&r.pts[cut..], &r.y[cut..], &r.seg_kind[cut..], &w.terrain);
        eprintln!("  next chunk ({} points): {:.2} ms, {} samples", r.pts.len() - cut, t.elapsed().as_secs_f64() * 1e3, m2.samples.len());
        // Cached: the same line again costs a lookup.
        let line = Arc::new(crate::nav::NavLine { rev: 800_000 + name.len() as u64, pts: r.pts.clone(), y: r.y.clone(), seg_kind: r.seg_kind.clone() });
        let first = Route3d::new(&line, &w.terrain, NavRouteCfg::default()).unwrap();
        let t = std::time::Instant::now();
        let again = Route3d::new(&line, &w.terrain, NavRouteCfg::default()).unwrap();
        eprintln!("  cached Route3d::new: {:?}", t.elapsed());
        assert!(Arc::ptr_eq(&first.mesh, &again.mesh));
    }
}

/// Real install, GL: the island-crossing route and the route over a jump in a HUD-sized and a
/// Dashboard-sized 3D view, and a 2D check that the same line is in the picture. PNGs to look at;
/// the route colour is on screen, the GL state clean.
#[test]
#[ignore = "needs an EGL device and an FH6 install; writes PNGs"]
fn gl3d_real_install_nav_route() {
    let Some(w) = real_world() else {
        eprintln!("SKIP gl3d_real_install_nav_route: no FH6 install");
        return;
    };
    let Some(mut rig) = open(Flavour::Default, None, [620, 420]) else { return };
    let (_hold, tex) = rig.load_map(&w, crate::minimap::OVERLAY_MAP_TEXTURE_OPTIONS);
    let routes = real_routes(&w);
    for (i, (name, r)) in routes.iter().enumerate() {
        // The car 10 % along the route, looking along it.
        let k = (r.pts.len() / 10).max(1).min(r.pts.len() - 2);
        let (car, next) = (r.pts[k], r.pts[(k + 8).min(r.pts.len() - 1)]);
        let line = Arc::new(crate::nav::NavLine { rev: 810_000 + i as u64, pts: r.pts[k..].to_vec(), y: r.y[k..].to_vec(), seg_kind: r.seg_kind[k..].to_vec() });
        let rt = route3d(&w, &line);
        for (vname, mut v) in [("hud", View::hud()), ("dashboard", View::dashboard())] {
            v.no_3d = false;
            v.car = (car[0], car[1]);
            v.yaw = (next[0] - car[0]).atan2(next[1] - car[1]);
            v.zoom = if *name == "jump" { 350.0 } else { 600.0 };
            v.car_y = Some(r.y[k].max(w.terrain.height(car[0], car[1])) + 1.0);
            let o = settled(&mut rig, &w, tex, &v, &format!("real_nav_{name}_{vname}"), &|s| {
                s.route = Some(rt.clone());
                if vname == "hud" {
                    s.roads.casing_px = 1.0;
                }
            });
            let n = o.count_near([0, 0, 620, 420], route_rgb(), 30);
            eprintln!("real route {name} ({vname}): {n} route px");
            assert!(n > if vname == "hud" { 3 } else { 60 }, "{name} {vname}: the route is on screen ({n})"); // (the HUD map is small and its roads hairlines at this zoom)
        }
    }
    rig.finish(&Gl3dHandle::new());
}

// ── the draw plan (CPU only) ─────────────────────────────────────────────────────────────────

#[test]
fn plan_uses_the_far_set_when_zoomed_out_and_culls_what_is_off_screen() {
    let w = world();
    let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(208.0, 136.0));
    let cam = |car: (f32, f32), zoom: f32| Camera::from_cfg_relief(&tilt(40.0), car, 0.0, zoom, rect, Some(&w.terrain), None);
    // Driving zoom: every tile within reach is the near set, with the deck.
    let near = roads::plan(&w.mesh, &cam((0.0, 0.0), 500.0), 1.0, 10.0);
    assert!(near.tiles_near > 0 && near.tiles_far == 0, "{near:?}");
    assert!(near.triangles > 0 && !near.normal.is_empty() && near.normal.iter().all(|d| !d.far));
    // Stopped zoom 3 km: 0.02 px per metre, the 32 m top-only set.
    let far = roads::plan(&w.mesh, &cam((0.0, 0.0), 3000.0), 1.0, 10.0);
    assert!(far.tiles_far > 0 && far.tiles_near == 0, "{far:?}");
    assert!(far.normal.iter().all(|d| d.far));
    assert!(far.triangles * 2 < near.triangles * 3, "the far set is much lighter: {} vs {} triangles", far.triangles, near.triangles);
    // Tunnels are separate ranges (drawn last, on top).
    assert!(!near.tunnel.is_empty(), "the tunnel through the hill");
    // A view 6 km away from every road sees none of them.
    let away = roads::plan(&w.mesh, &cam((6000.0, 6000.0), 200.0), 1.0, 10.0);
    assert!(away.normal.is_empty() && away.tunnel.is_empty(), "{away:?}");
}

#[test]
fn orphan_roads_do_not_dip_below_the_ground_in_the_mesh_the_renderer_gets() {
    // The synthetic world carries an all-orphan (y = 0) chain; in node-height mode (the default)
    // its vertices must lie on the terrain, not at 0.
    let w = world();
    let orphan_slot = 3u8;
    let samples: Vec<_> = w.mesh.samples.iter().filter(|s| s.slot == orphan_slot && (s.z + 420.0).abs() < 40.0).collect();
    assert!(samples.len() > 100);
    for s in samples {
        assert!((s.y_node - w.terrain.height(s.x, s.z)).abs() < 0.5 && s.y_node > 90.0, "orphan sample at ({}, {}) y {}", s.x, s.z, s.y_node);
    }
}

// ── the real install ─────────────────────────────────────────────────────────────────────────

/// The island: real terrain, the user's road data, the satellite image. `None` without an install.
fn real_world() -> Option<World> {
    let media = crate::gamedata::install::find_media(None)?;
    let t0 = std::time::Instant::now();
    let terrain = Arc::new(Terrain::load(&media, &|_| {}).map_err(|e| eprintln!("terrain: {e}")).ok()?);
    let game = crate::maprender::data::GameData::load(&media).map_err(|e| eprintln!("game data: {e}")).ok()?;
    let cur = crate::gamedata::roadtypes::RoadTypes::current(&crate::gamedata::roadtypes::override_path(), &game.nav);
    let layers = Arc::new(game.layers(&cur, 1));
    let mesh = Arc::new(RoadMesh::build(&layers.roads, &terrain, 1));
    let (image, orig) = crate::minimap::overlay_map_image(crate::minimap::current_season()).map_err(|e| eprintln!("map image: {e:?}")).ok()?;
    eprintln!("real world loaded in {} ms: {} road samples, {} + {} triangles (near + far set)", t0.elapsed().as_millis(), mesh.samples.len(), mesh.triangles().0, mesh.triangles().1);
    Some(World { terrain, layers, mesh, image, cal: MapCalibration::DEFAULT, orig })
}

/// Scenes of the real island, rendered the way the three sites would: PNGs to look at, the GL
/// state asserted clean, perf numbers printed (GPU time by timer query where available).
#[test]
#[ignore = "needs an EGL device and an FH6 install; writes PNGs"]
fn gl3d_real_install_scenes() {
    let Some(w) = real_world() else {
        eprintln!("SKIP gl3d_real_install_scenes: no FH6 install");
        return;
    };
    // GL3D_DEVICE=<index> picks another EGL device (e.g. the llvmpipe one) for the perf numbers.
    let device = std::env::var("GL3D_DEVICE").ok().and_then(|d| d.parse().ok());
    let Some(mut rig) = open(Flavour::Default, device, [1300, 760]) else { return };
    eprintln!("{}", rig.info());
    let (_hold, tex) = rig.load_map(&w, crate::minimap::OVERLAY_MAP_TEXTURE_OPTIONS);
    let t = &w.terrain;

    // Where to look: an elevated highway (node height well above the ground), the highest road
    // vertex, and the densest 200 m cell of the road mesh (the city).
    let mut bridge = (0.0f32, 0.0f32, 0.0f32);
    for ch in &w.layers.roads.by_type[RoadType::Highway.index() as usize] {
        for (p, &y) in ch.pts.iter().zip(&ch.y) {
            let ex = y - t.height(p[0], p[1]);
            if ex > bridge.2 && ex < 60.0 {
                bridge = (p[0], p[1], ex);
            }
        }
    }
    let mut peak = (0.0f32, 0.0f32, f32::MIN);
    for ch in w.layers.roads.by_type.iter().flatten() {
        for p in &ch.pts {
            let h = t.height(p[0], p[1]);
            if h > peak.2 {
                peak = (p[0], p[1], h);
            }
        }
    }
    let mut cells = std::collections::HashMap::<(i32, i32), u32>::new();
    for s in &w.mesh.samples {
        *cells.entry(((s.x / 200.0).floor() as i32, (s.z / 200.0).floor() as i32)).or_default() += 1;
    }
    let (&(cx, cz), &n) = cells.iter().max_by_key(|(_, &n)| n).unwrap();
    let city = (cx as f32 * 200.0 + 100.0, cz as f32 * 200.0 + 100.0);
    let b = t.grid.bounds();
    let centre = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    eprintln!("scenes: bridge at ({:.0}, {:.0}) {:.0} m above ground; highest road ({:.0}, {:.0}) at {:.0} m; densest 200 m cell ({:.0}, {:.0}) with {n} samples; island centre ({:.0}, {:.0})", bridge.0, bridge.1, bridge.2, peak.0, peak.1, peak.2, city.0, city.1, centre.0, centre.1);

    let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, sync_timing: true, ..Default::default() });
    let hud = |car: (f32, f32), zoom: f32, yaw: f32| View { car, zoom, yaw, car_y: Some(t.height(car.0, car.1) + 1.0), ..View::hud() };
    let big = |car: (f32, f32), zoom: f32, angle: f32, w_: f32, h_: f32| View { site: Site::Dashboard, rect: Rect::from_min_size(pos2(10.0, 10.0), vec2(w_, h_)), car, yaw: 0.0, zoom, angle, car_y: None, clip: None, no_3d: false, marker: None, mates: vec![] };
    let scenes: Vec<(&str, View, f32)> = vec![
        ("hud_bridge_150_x3", hud((bridge.0, bridge.1), 150.0, 0.4), 3.0),
        ("hud_mountain_500_x3", hud((peak.0, peak.1), 500.0, 0.0), 3.0),
        ("hud_city_500_x3", hud(city, 500.0, 0.8), 3.0),
        ("hud_city_stopped_3000", hud(city, 3000.0, 0.0), 1.0),
        ("dashboard_city_1500", big(city, 1500.0, 55.0, 600.0, 400.0), 1.0),
        ("dashboard_bridge_150", big((bridge.0, bridge.1), 150.0, 55.0, 600.0, 400.0), 1.0),
        ("viewer_island_8000", big(centre, 8000.0, 55.0, 1280.0, 720.0), 1.0),
        ("viewer_mountain_3000", big((peak.0, peak.1), 3000.0, 55.0, 1280.0, 720.0), 1.0),
        ("viewer_west_edge_3000", big((b[0] + 600.0, centre.1), 3000.0, 55.0, 1280.0, 720.0), 1.0),
    ];
    for (name, v, ppp) in scenes {
        let o = warm_up(&mut rig, &w, &h, tex, &v, ppp);
        assert_eq!(o.gl_error, 0, "{name}: GL error 0x{:X}", o.gl_error);
        o.save(&format!("real_{name}.png"));
        let (mut gpu, mut cpu) = (vec![], vec![]);
        for _ in 0..20 {
            let o = map_frame(&mut rig, &w, &h, tex, &v, ppp, &|_| {});
            assert_eq!(o.gl_error, 0, "{name}: GL error 0x{:X}", o.gl_error);
            assert!(o.fbo_restored);
            let s = h.stats();
            gpu.extend(s.last.gpu_ms);
            cpu.push(s.last.cpu_ms);
        }
        let s = h.stats().last;
        eprintln!("PERF real {name}: {} tri, {} draws, tiles {} near / {} far, render cpu {:.3} ms, gpu {:.3} ms", s.triangles, s.draws, s.tiles_near, s.tiles_far, median(cpu), median(gpu));
    }
    rig.finish(&h);
}

// ── road joins (D81): close-ups of real junctions, 2D and 3D ─────────────────────────────────

/// Real road nodes worth a close look at the join geometry: `(name, x, z, y)`. Found from the chain
/// vertices alone (equal positions = one node), nearest to `near` per kind.
fn join_spots(l: &RoadLayer, near: (f32, f32)) -> Vec<(&'static str, f32, f32, f32)> {
    #[derive(Default)]
    struct Node {
        x: f32,
        z: f32,
        y: f32,
        ends: u32,
        dirs: Vec<[f32; 2]>,
        slots: Vec<usize>,
    }
    let mut nodes: std::collections::HashMap<(u32, u32), Node> = Default::default();
    for (slot, chains) in l.by_type.iter().enumerate() {
        if slot == RoadType::Turnaround.index() as usize {
            continue;
        }
        for ch in chains {
            let n = ch.pts.len();
            for (i, p) in ch.pts.iter().enumerate() {
                let e = nodes.entry((p[0].to_bits(), p[1].to_bits())).or_default();
                (e.x, e.z, e.y) = (p[0], p[1], ch.y[i]);
                for j in [i.wrapping_sub(1), i + 1] {
                    if let Some(q) = ch.pts.get(j) {
                        let d = [q[0] - p[0], q[1] - p[1]];
                        let len = d[0].hypot(d[1]).max(1e-6);
                        e.dirs.push([d[0] / len, d[1] / len]);
                        e.slots.push(slot);
                    }
                }
                e.ends += (i == 0 || i + 1 == n) as u32;
            }
        }
    }
    let dist = |n: &Node| (n.x - near.0).hypot(n.z - near.1);
    let angle = |a: [f32; 2], b: [f32; 2]| (a[0] * b[0] + a[1] * b[1]).clamp(-1.0, 1.0).acos().to_degrees();
    let road = RoadType::Road.index() as usize;
    let hw = RoadType::Highway.index() as usize;
    type Pick = Box<dyn Fn(&Node) -> bool>;
    let kinds: Vec<(&'static str, Pick)> = vec![
        ("city_x", Box::new(move |n: &Node| n.dirs.len() >= 4 && n.ends > 0 && n.slots.iter().all(|&s| s == road))),
        ("city_t", Box::new(move |n: &Node| n.dirs.len() == 3 && n.ends == 1 && n.slots.iter().all(|&s| s == road))),
        ("highway", Box::new(move |n: &Node| n.dirs.len() >= 3 && n.slots.contains(&hw) && n.slots.iter().any(|&s| s != hw))),
        ("l_corner", Box::new(move |n: &Node| n.dirs.len() == 2 && n.ends == 2 && n.slots.iter().all(|&s| s == road) && (60.0..120.0).contains(&angle(n.dirs[0], n.dirs[1])))),
        ("type_change", Box::new(move |n: &Node| n.dirs.len() == 2 && n.ends == 2 && n.slots[0] != n.slots[1] && n.slots.iter().all(|&s| s != 0))),
        ("y_shallow", Box::new(move |n: &Node| n.dirs.len() == 3 && n.ends > 0 && (0..3).any(|i| (i + 1..3).any(|j| angle(n.dirs[i], n.dirs[j]) < 30.0)))),
    ];
    let mut out = vec![];
    for (name, f) in kinds {
        if let Some(n) = nodes.values().filter(|n| f(n)).min_by(|a, b| dist(a).total_cmp(&dist(b))) {
            out.push((name, n.x, n.z, n.y));
        }
    }
    out
}

/// The synthetic join scenes (D81) on flat ground at 100 m, 200 m apart along x: `(name, centre)`
/// and the world. Roads at node height 100.3 unless noted.
fn join_world() -> (World, Vec<(&'static str, (f32, f32))>) {
    let base = world();
    let flat = Arc::new(Terrain::flat(100.0));
    let mut roads = RoadLayer::default();
    let y = 100.3;
    let mut add = |t: RoadType, pts: &[[f32; 2]], h: f32| roads.by_type[t.index() as usize].push(Chain::new(pts.to_vec(), vec![h; pts.len()]));
    let mut spots = vec![];
    let mut at = |name: &'static str, i: usize| {
        let o = (-700.0 + 200.0 * i as f32, 0.0);
        spots.push((name, o));
        o
    };
    // A type change straight through (road -> highway) and one at a 45 degree bend (road -> offroad).
    let o = at("type_change", 0);
    add(RoadType::Road, &[[o.0 - 70.0, o.1 - 20.0], [o.0, o.1 - 20.0]], y);
    add(RoadType::Highway, &[[o.0, o.1 - 20.0], [o.0 + 70.0, o.1 - 20.0]], y);
    add(RoadType::Road, &[[o.0 - 70.0, o.1 + 25.0], [o.0, o.1 + 25.0]], y);
    add(RoadType::Offroad, &[[o.0, o.1 + 25.0], [o.0 + 40.0, o.1 + 65.0]], y);
    // An L-corner of two chains (the screenshot).
    let o = at("l_corner", 1);
    add(RoadType::Road, &[[o.0 - 70.0, o.1], [o.0, o.1]], y);
    add(RoadType::Road, &[[o.0, o.1], [o.0, o.1 + 70.0]], y);
    // A T: a through road and a trail ending on it.
    let o = at("t_junction", 2);
    add(RoadType::Road, &[[o.0 - 70.0, o.1], [o.0, o.1], [o.0 + 70.0, o.1]], y);
    add(RoadType::Trail, &[[o.0, o.1], [o.0, o.1 + 70.0]], y);
    // An X of four chain ends: road, road, highway, offroad.
    let o = at("x_junction", 3);
    add(RoadType::Road, &[[o.0 - 70.0, o.1], [o.0, o.1]], y);
    add(RoadType::Road, &[[o.0, o.1], [o.0 + 70.0, o.1]], y);
    add(RoadType::Highway, &[[o.0, o.1 - 70.0], [o.0, o.1]], y);
    add(RoadType::Offroad, &[[o.0, o.1], [o.0, o.1 + 70.0]], y);
    // A shallow Y (two branches 15 degrees off the stem).
    let o = at("y_shallow", 4);
    add(RoadType::Road, &[[o.0 - 70.0, o.1], [o.0, o.1]], y);
    add(RoadType::Road, &[[o.0, o.1], [o.0 + 70.0, o.1 + 18.8]], y);
    add(RoadType::Road, &[[o.0, o.1], [o.0 + 70.0, o.1 - 18.8]], y);
    // An overpass: a highway 12 m up crossing a road, no shared node (not a junction).
    let o = at("overpass", 5);
    add(RoadType::Road, &[[o.0 - 70.0, o.1], [o.0 + 70.0, o.1]], y);
    add(RoadType::Highway, &[[o.0 - 50.0, o.1 - 50.0], [o.0 + 50.0, o.1 + 50.0]], 112.0);
    // A plain 4-way of road chain ends (the in-race focus hides one arm).
    let o = at("four_way", 6);
    add(RoadType::Road, &[[o.0 - 70.0, o.1], [o.0, o.1]], y);
    add(RoadType::Road, &[[o.0, o.1], [o.0 + 70.0, o.1]], y);
    add(RoadType::Road, &[[o.0, o.1 - 70.0], [o.0, o.1]], y);
    add(RoadType::Road, &[[o.0, o.1], [o.0, o.1 + 70.0]], y);
    let layers = Arc::new(MapLayers { rev: 1, roads: Arc::new(roads), ..Default::default() });
    let mesh = Arc::new(RoadMesh::build(&layers.roads, &flat, 1));
    (World { terrain: flat, layers, mesh, image: base.image, cal: base.cal, orig: base.orig }, spots)
}

/// Before / after pictures of the synthetic join scenes in flat 2D and in 3D (`JOIN_TAG`), plus
/// the 4-way with its north arm hidden by the in-race focus (3D).
#[test]
#[ignore = "needs an EGL device; writes PNGs"]
fn gl3d_synthetic_joins() {
    let (w, spots) = join_world();
    let tag = std::env::var("JOIN_TAG").unwrap_or_else(|_| "now".into());
    let Some(mut rig) = open(Flavour::Default, None, [620, 420]) else { return };
    let (_hold, tex) = rig.load_map(&w, TextureOptions::LINEAR);
    let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
    let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(600.0, 400.0));
    for (name, (x, z)) in spots {
        let o = rig.frame(1.0, |ctx| {
            let p = ctx.layer_painter(LayerId::new(Order::Background, egui::Id::new("map")));
            p.rect_filled(ctx.content_rect(), 0.0, BACKDROP);
            let cam = Camera::from_cfg(&TiltCfg::default(), (x, z), 0.0, 90.0, rect);
            let outline = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
            let pc = p.with_clip_rect(rect);
            draw_base(&pc, &BaseParams { cam: &cam, cal: w.cal, tex, outline: &outline, mirror: true, look: ImageLook::FULL, a: 1.0, far_fade: false });
            let mut cfg = MapLayerConfig::default();
            cfg.pois.on = false;
            cfg.race_lines.mode = RaceLineMode::Off;
            cfg.roads.max_px = 24.0;
            cfg.roads.casing_px = 3.0;
            let sel = RaceSel::default();
            let cx = LayerCtx { p: &pc, cam: &cam, s: 1.0, a: 1.0, car: (x, z), corner_clip: None, icons: None, race_sel: &sel, week: None, nav: None };
            draw_layers(&cx, &w.layers, &cfg);
        });
        o.save(&format!("synth_{tag}_{name}_2d.png"));
        let v = View { site: Site::Dashboard, rect, car: (x, z - 30.0), yaw: 0.25, zoom: 70.0, angle: 55.0, car_y: Some(101.0), clip: None, no_3d: false, marker: None, mates: vec![] };
        let wide = |s: &mut Scene3d| {
            s.roads.max_px = 30.0;
            s.roads.casing_px = 3.0;
        };
        warm_up(&mut rig, &w, &h, tex, &v, 1.0);
        let o = map_frame(&mut rig, &w, &h, tex, &v, 1.0, &wide);
        assert_eq!(o.gl_error, 0);
        o.save(&format!("synth_{tag}_{name}_3d.png"));
        if name == "four_way" {
            // The north arm (chain 3) off the race: hidden. Its cap must not leave a stub.
            let mut focus = crate::maprender::racesel::RoadFocus::default();
            let road = RoadType::Road.index() as usize;
            for (slot, chains) in w.layers.roads.by_type.iter().enumerate() {
                for (ci, ch) in chains.iter().enumerate() {
                    let north = slot == road && ch.pts[0][0] == x && ch.pts[0][1] == z && ch.pts[1][1] > z;
                    focus.runs[slot].push(Run { chain: ci as u32, a: 0, b: ch.pts.len() as u32 - 1, relevant: !north, bbox: ch.bbox });
                }
            }
            let f = Focus3d { focus: Arc::new(focus), cfg: RaceFocusCfg { other_roads: OtherRoads::Hidden, ..Default::default() } };
            for _ in 0..3 {
                map_frame(&mut rig, &w, &h, tex, &v, 1.0, &|s| {
                    wide(s);
                    s.focus = Some(f.clone());
                });
            }
            let o = map_frame(&mut rig, &w, &h, tex, &v, 1.0, &|s| {
                wide(s);
                s.focus = Some(f.clone());
            });
            o.save(&format!("synth_{tag}_{name}_hidden_3d.png"));
        }
    }
    rig.finish(&h);
}

/// Close-ups of real junctions (city crossing and T, a highway junction, an L-corner, a type
/// change, a shallow Y) in flat 2D and in 3D, for looking at the join geometry. `JOIN_TAG` names
/// the set (`before` / `after`).
#[test]
#[ignore = "needs an EGL device and an FH6 install; writes PNGs"]
fn gl3d_real_install_joins() {
    let Some(w) = real_world() else {
        eprintln!("SKIP gl3d_real_install_joins: no FH6 install");
        return;
    };
    let tag = std::env::var("JOIN_TAG").unwrap_or_else(|_| "now".into());
    let Some(mut rig) = open(Flavour::Default, None, [620, 420]) else { return };
    let (_hold, tex) = rig.load_map(&w, crate::minimap::OVERLAY_MAP_TEXTURE_OPTIONS);
    let mut cells = std::collections::HashMap::<(i32, i32), u32>::new();
    for s in &w.mesh.samples {
        *cells.entry(((s.x / 200.0).floor() as i32, (s.z / 200.0).floor() as i32)).or_default() += 1;
    }
    let (&(cx, cz), _) = cells.iter().max_by_key(|(_, &n)| n).unwrap();
    let city = (cx as f32 * 200.0 + 100.0, cz as f32 * 200.0 + 100.0);
    let spots = join_spots(&w.layers.roads, city);
    let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
    let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(600.0, 400.0));
    for (name, x, z, y) in spots {
        eprintln!("join spot {name}: ({x:.1}, {z:.1}) y {y:.1}");
        // Flat 2D, the Dashboard's look, at 120 m and 400 m.
        for zoom in [120.0f32, 400.0] {
            let o = rig.frame(1.0, |ctx| {
                let p = ctx.layer_painter(LayerId::new(Order::Background, egui::Id::new("map")));
                p.rect_filled(ctx.content_rect(), 0.0, BACKDROP);
                let cam = Camera::from_cfg(&TiltCfg::default(), (x, z), 0.0, zoom, rect);
                let outline = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
                let pc = p.with_clip_rect(rect);
                draw_base(&pc, &BaseParams { cam: &cam, cal: w.cal, tex, outline: &outline, mirror: true, look: ImageLook::FULL, a: 1.0, far_fade: false });
                let mut cfg = MapLayerConfig::default();
                cfg.pois.on = false;
                cfg.race_lines.mode = RaceLineMode::Off;
                let sel = RaceSel::default();
                let cx = LayerCtx { p: &pc, cam: &cam, s: 1.0, a: 1.0, car: (x, z), corner_clip: None, icons: None, race_sel: &sel, week: None, nav: None };
                draw_layers(&cx, &w.layers, &cfg);
            });
            o.save(&format!("join_{tag}_{name}_2d_{zoom:.0}.png"));
        }
        // 3D, the Dashboard at 50 degrees: a close-up (40 m) and the neighbourhood (150 m).
        for zoom in [40.0f32, 150.0] {
            let v = View { site: Site::Dashboard, rect, car: (x, z), yaw: 0.3, zoom, angle: 50.0, car_y: Some(y + 1.0), clip: None, no_3d: false, marker: None, mates: vec![] };
            let o = warm_up(&mut rig, &w, &h, tex, &v, 1.0);
            assert_eq!(o.gl_error, 0);
            o.save(&format!("join_{tag}_{name}_3d_{zoom:.0}.png"));
        }
    }
    rig.finish(&h);
}

/// D80 on the island: race roads (a turn at a junction, a road passing over the route, a
/// cross-country route, The Goliath 5555) in 3D (HUD and Dashboard looks, other roads muted, and
/// the Goliath with "race road only") and in flat 2D, to look at. Prints where each one is and the
/// race mesh build time.
#[test]
#[ignore = "needs an EGL device and an FH6 install; writes PNGs"]
fn gl3d_real_install_race_roads() {
    use crate::gamedata::icons::RaceClass;
    use crate::maprender::racesel::RoadFocus;
    let Some(w) = real_world() else {
        eprintln!("SKIP gl3d_real_install_race_roads: no FH6 install");
        return;
    };
    let Some(mut rig) = open(Flavour::Default, None, [620, 420]) else { return };
    let (_hold, tex) = rig.load_map(&w, crate::minimap::OVERLAY_MAP_TEXTURE_OPTIONS);
    let races = &w.layers.races;
    let heading = |l: &crate::gamedata::racelines::RaceLine, i: usize| {
        let (a, b) = (l.pts[i.saturating_sub(2)], l.pts[(i + 2).min(l.pts.len() - 1)]);
        (b[0] - a[0]).atan2(b[1] - a[1])
    };
    let wrap = |x: f32| (x + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    // A sharp turn (> 70 degrees within 20 m) on an asphalt route at a road node.
    let mut nodes = std::collections::HashSet::new();
    for ch in w.layers.roads.by_type.iter().flatten() {
        for p in &ch.pts {
            nodes.insert(((p[0] / 4.0).round() as i32, (p[1] / 4.0).round() as i32));
        }
    }
    let class = |li: usize| w.layers.race_class.get(&races.lines[li].route).copied();
    let mut junction = None;
    'j: for li in 0..races.lines.len() {
        if !matches!(class(li), Some(RaceClass::AsphaltP2p | RaceClass::AsphaltCircuit | RaceClass::Streetracing)) {
            continue;
        }
        let l = &races.lines[li];
        for i in 4..l.pts.len().saturating_sub(4) {
            let turn = wrap(heading(l, i + 2) - heading(l, i - 2)).abs();
            let p = l.pts[i];
            if turn > 1.2 && nodes.contains(&((p[0] / 4.0).round() as i32, (p[1] / 4.0).round() as i32)) {
                junction = Some((li, i));
                break 'j;
            }
        }
    }
    // A road crossing over the route (more than 6 m above it, within 3 m horizontally).
    let mut over = None;
    'o: for li in 0..races.lines.len() {
        let l = &races.lines[li];
        for ch in w.layers.roads.by_type.iter().flatten() {
            if !crate::maprender::view::bbox_hits(&ch.bbox, &l.bbox) {
                continue;
            }
            for (k, p) in ch.pts.iter().enumerate() {
                for i in (0..l.pts.len()).step_by(2) {
                    let q = l.pts[i];
                    if (p[0] - q[0]).abs() < 3.0 && (p[1] - q[1]).abs() < 3.0 && ch.y[k] - l.y[i] > 6.0 && ch.y[k] - l.y[i] < 20.0 {
                        over = Some((li, i));
                        break 'o;
                    }
                }
            }
        }
    }
    let cross = (0..races.lines.len()).find(|&li| matches!(class(li), Some(RaceClass::CrosscountryP2p | RaceClass::CrosscountryCircuit))).map(|li| (li, races.lines[li].pts.len() / 3));
    let goliath = races.lines.iter().position(|l| l.route == 5555).map(|li| (li, 40));
    let spots = [("junction", junction, 120.0f32), ("overpass", over, 70.0), ("crosscountry", cross, 400.0), ("goliath", goliath, 600.0)];
    let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(600.0, 400.0));
    let rc = crate::maprender::cfg::RaceCfg::default();
    for (name, spot, zoom) in spots {
        let Some((li, i)) = spot else {
            eprintln!("race road spot {name}: none found");
            continue;
        };
        let l = &races.lines[li];
        let (x, z, y, yaw) = (l.pts[i][0], l.pts[i][1], l.y[i], heading(l, i));
        let focus = RoadFocus::build(&w.layers.roads, l);
        let road = RaceRoad { pts: l.pts.clone(), y: l.y.clone(), closed: l.closed };
        let t0 = std::time::Instant::now();
        let m = crate::maprender::mesh3d::RoadMesh::race_road(&road, &w.terrain);
        let r3 = race3d(&w, vec![road], vec![], rc);
        eprintln!("race road spot {name}: route {} ({:.1} km, class {:?}) at ({x:.0}, {z:.0}) y {y:.1}; race mesh {} samples, {} triangles, built in {:.1} ms", l.route, l.length_m / 1000.0, class(li), m.samples.len(), m.triangles().0, t0.elapsed().as_secs_f64() * 1e3);
        let focus = Arc::new(focus);
        let modes: &[OtherRoads] = match name {
            "goliath" => &[OtherRoads::Muted, OtherRoads::RaceOnly],
            "overpass" => &[OtherRoads::Muted, OtherRoads::Normal],
            _ => &[OtherRoads::Muted],
        };
        for &mode in modes {
            let f = Focus3d { focus: focus.clone(), cfg: RaceFocusCfg { other_roads: mode, ..Default::default() } };
            let views = [
                ("hud", View { car: (x, z), yaw, zoom: zoom.min(500.0), car_y: Some(y + 1.0), ..View::hud() }, 2.0f32),
                ("dash", View { site: Site::Dashboard, rect, car: (x, z), yaw, zoom, angle: 50.0, car_y: Some(y + 1.0), clip: None, no_3d: false, marker: None, mates: vec![] }, 1.0),
            ];
            for (vn, v, ppp) in views {
                let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
                warm_up(&mut rig, &w, &h, tex, &v, ppp);
                let mut o = None;
                for _ in 0..4 {
                    o = Some(map_frame(&mut rig, &w, &h, tex, &v, ppp, &|s| {
                        s.focus = Some(f.clone());
                        s.race = Some(r3.clone());
                    }));
                }
                let o = o.unwrap();
                assert_eq!(o.gl_error, 0, "{name} {vn}");
                o.save(&format!("race_{name}_{vn}_{mode:?}_3d.png"));
                h.destroy(&rig.gl);
            }
        }
        // Flat 2D (the Dashboard look) with the race road and the marks, other roads muted.
        let o = rig.frame(1.0, |ctx| {
            let p = ctx.layer_painter(LayerId::new(Order::Background, egui::Id::new("map")));
            p.rect_filled(ctx.content_rect(), 0.0, BACKDROP);
            let cam = Camera::from_cfg(&TiltCfg::default(), (x, z), 0.0, zoom * 1.5, rect);
            let outline = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
            let pc = p.with_clip_rect(rect);
            draw_base(&pc, &BaseParams { cam: &cam, cal: w.cal, tex, outline: &outline, mirror: true, look: ImageLook::FULL, a: 1.0, far_fade: false });
            let mut cfg = MapLayerConfig::default();
            cfg.pois.on = false;
            let sel = RaceSel::fixed(vec![li], true);
            let cx = LayerCtx { p: &pc, cam: &cam, s: 1.0, a: 1.0, car: (x, z), corner_clip: None, icons: None, race_sel: &sel, week: None, nav: None };
            draw_layers(&cx, &w.layers, &cfg);
        });
        o.save(&format!("race_{name}_2d.png"));
    }
    rig.finish(&Gl3dHandle::new());
}

/// D88 on the island: every line of mode `all` (170 lines, ~1 000 km) as one race mesh, in both
/// styles and with the marks: build time, size and a frame to look at. `--release` for the numbers.
#[test]
#[ignore = "needs an EGL device and an FH6 install; writes PNGs"]
fn gl3d_real_install_all_race_lines() {
    use crate::maprender::cfg::{RaceCfg, RaceLineMode, RouteStyle};
    let Some(w) = real_world() else {
        eprintln!("SKIP gl3d_real_install_all_race_lines: no FH6 install");
        return;
    };
    let Some(mut rig) = open(Flavour::Default, None, [900, 600]) else { return };
    let (_hold, tex) = rig.load_map(&w, crate::minimap::OVERLAY_MAP_TEXTURE_OPTIONS);
    let cfg = RaceCfg { mode: RaceLineMode::All, ..RaceCfg::default() };
    let t0 = std::time::Instant::now();
    let draw = RaceSel::default().race_draw(&w.layers, &cfg).expect("all lines");
    let t_draw = t0.elapsed().as_secs_f64() * 1e3;
    let t1 = std::time::Instant::now();
    let mesh = Arc::new(crate::maprender::mesh3d::RoadMesh::race_roads(&draw.lines, &w.terrain));
    let t_mesh = t1.elapsed().as_secs_f64() * 1e3;
    let km: f64 = w.layers.races.lines.iter().map(|l| l.length_m).sum::<f64>() / 1000.0;
    eprintln!(
        "all race lines: {} lines, {} marks, {:.0} km; race_draw {:.1} ms; mesh {:.1} ms, {} samples, {} + {} triangles (near + far), {:.1} MB vertices",
        draw.lines.len(), draw.marks.len(), km, t_draw, t_mesh, mesh.samples.len(), mesh.triangles().0, mesh.triangles().1, mesh.vertices.len() as f64 / 1e6
    );
    let l = &w.layers.races.lines[w.layers.races.lines.len() / 2];
    let mid = l.pts[l.pts.len() / 2];
    for (name, route, zoom, angle) in [("road_3km", RouteStyle::Road, 3000.0f32, 50.0f32), ("line_3km", RouteStyle::Line, 3000.0, 50.0), ("road_300m", RouteStyle::Road, 300.0, 50.0), ("line_300m", RouteStyle::Line, 300.0, 50.0)] {
        let r = Race3d { draw: draw.clone(), mesh: Some(mesh.clone()), cfg: RaceCfg { route, ..cfg } };
        let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(880.0, 580.0));
        let v = View { site: Site::Dashboard, rect, car: (mid[0], mid[1]), yaw: 0.0, zoom, angle, car_y: None, clip: None, no_3d: false, marker: None, mates: vec![] };
        let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, sync_timing: true, ..Default::default() });
        warm_up(&mut rig, &w, &h, tex, &v, 1.0);
        let mut o = None;
        for _ in 0..4 {
            o = Some(map_frame(&mut rig, &w, &h, tex, &v, 1.0, &|s| {
                s.race = Some(r.clone());
                s.roads.on = false;
            }));
        }
        let o = o.unwrap();
        assert_eq!(o.gl_error, 0, "{name}");
        o.save(&format!("race_all_{name}.png"));
        let st = h.stats().last;
        eprintln!("all lines {name}: {} triangles drawn, {} draws, gpu {:?} ms", st.triangles, st.draws, st.gpu_ms);
        h.destroy(&rig.gl);
    }
    rig.finish(&Gl3dHandle::new());
}

// ── clipmap geomorphing: no pops while driving ──────────────────────────────────────────────

/// The 360 m of a real road with the most up and down (straight-ish), in `step` m steps: (x, z, yaw).
fn hilly_path(w: &World, step: f32, len: f32) -> Vec<(f32, f32, f32)> {
    let t = &w.terrain;
    let mut best: (f32, Vec<(f32, f32)>) = (0.0, vec![]);
    for ch in &w.layers.roads.by_type[RoadType::Road.index() as usize] {
        if ch.pts.len() < 10 {
            continue;
        }
        let mut pts = vec![(ch.pts[0][0], ch.pts[0][1])];
        let mut carry = 0.0f32;
        for k in 1..ch.pts.len() {
            let (a, b) = (ch.pts[k - 1], ch.pts[k]);
            let d = (b[0] - a[0]).hypot(b[1] - a[1]);
            let mut s = step - carry;
            while s <= d {
                let u = s / d;
                pts.push((a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u));
                s += step;
            }
            carry = d - (s - step);
        }
        let n = (len / step) as usize;
        if pts.len() < n + 2 {
            continue;
        }
        let hs: Vec<f32> = pts.iter().map(|p| t.height(p.0, p.1)).collect();
        let mut i = 0;
        while i + n < pts.len() {
            let chord = (pts[i + n].0 - pts[i].0).hypot(pts[i + n].1 - pts[i].1);
            if chord > 0.9 * len {
                let sl: f32 = hs[i..i + n].windows(2).map(|w| (w[1] - w[0]).abs()).sum();
                if sl > best.0 {
                    best = (sl, pts[i..i + n].to_vec());
                }
            }
            i += 10;
        }
    }
    assert!(!best.1.is_empty(), "no straight stretch of road found");
    eprintln!("hilly path: total |dh| {:.0} m over {len} m, from ({:.0}, {:.0})", best.0, best.1[0].0, best.1[0].1);
    path_yaws(&best.1)
}

/// Positions with the heading towards the point 3 steps ahead.
fn path_yaws(p: &[(f32, f32)]) -> Vec<(f32, f32, f32)> {
    (0..p.len())
        .map(|i| {
            let (a, b) = (p[i.saturating_sub(3)], p[(i + 3).min(p.len() - 1)]);
            (p[i].0, p[i].1, (b.0 - a.0).atan2(b.1 - a.1))
        })
        .collect()
}

/// (mean abs diff /255, % of pixels differing visibly - summed RGB difference > 24) inside `r`.
fn diff_share(a: &Out, b: &Out, r: [usize; 4]) -> (f64, f64) {
    let (mut sum, mut cnt, mut n) = (0u64, 0usize, 0usize);
    for y in r[1]..r[3] {
        for x in r[0]..r[2] {
            let (p, q) = (a.at(x, y), b.at(x, y));
            let d: i32 = (0..3).map(|k| (p[k] as i32 - q[k] as i32).abs()).sum();
            sum += d as u64;
            n += 1;
            if d > 24 {
                cnt += 1;
            }
        }
    }
    (sum as f64 / n as f64 / 3.0, 100.0 * cnt as f64 / n as f64)
}

/// Drive `path` and compare, at every step, the frame the terrain levels of THIS position give with
/// the frame the levels of the PREVIOUS position would give under the same camera: that difference
/// is exactly what a level snap changes on screen (a "pop"). Terrain only (no roads). Returns the
/// worst frame's (mean abs diff /255, % of pixels differing visibly) per view kind.
fn pop_run(rig: &mut Rig, w: &World, tex: MapTex, path: &[(f32, f32, f32)], tag: &str) -> Vec<(&'static str, f64, f64)> {
    let t = w.terrain.clone();
    let mk = |kind: &str, p: (f32, f32, f32)| -> (View, f32) {
        let car_y = Some(t.height(p.0, p.1) + 1.0);
        match kind {
            "hud500" => (View { car: (p.0, p.1), zoom: 500.0, yaw: p.2, car_y, ..View::hud() }, 3.0),
            "hud150" => (View { car: (p.0, p.1), zoom: 150.0, yaw: p.2, car_y, ..View::hud() }, 3.0),
            "dash800" => (View { car: (p.0, p.1), zoom: 800.0, yaw: p.2, car_y, ..View::dashboard() }, 1.0),
            _ => (View { car: (p.0, p.1), zoom: 1500.0, yaw: p.2, car_y, ..View::dashboard() }, 1.0),
        }
    };
    let off = |s: &mut Scene3d| {
        s.mesh = None;
        s.roads.on = false;
    };
    let h = Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Default::default() });
    let mut out = vec![];
    for kind in ["hud500", "hud150", "dash800", "dash1500"] {
        let (v0, ppp) = mk(kind, path[path.len() / 2]);
        let _ = warm_up(rig, w, &h, tex, &v0, ppp);
        let r = inside(&v0, ppp);
        let (mut worst, mut worst_pct, mut sum_pct, mut moved) = (0.0f64, 0.0f64, 0.0f64, 0usize);
        for i in 0..path.len() - 1 {
            let (va, _) = mk(kind, path[i]);
            let (vb, _) = mk(kind, path[i + 1]);
            *clipmap::LOD_CAR.lock().unwrap() = None;
            let new = map_frame(rig, w, &h, tex, &vb, ppp, &off);
            *clipmap::LOD_CAR.lock().unwrap() = Some([va.car.0 as f64, va.car.1 as f64]);
            let stale = map_frame(rig, w, &h, tex, &vb, ppp, &off);
            *clipmap::LOD_CAR.lock().unwrap() = None;
            assert_eq!(new.gl_error, 0);
            let (m, pct) = diff_share(&new, &stale, r);
            if m > 0.0 {
                moved += 1;
            }
            sum_pct += pct;
            if pct > worst_pct {
                worst_pct = pct;
                new.save(&format!("pop_{tag}_{kind}_new.png"));
                stale.save(&format!("pop_{tag}_{kind}_stale.png"));
            }
            worst = worst.max(m);
        }
        eprintln!(
            "POPS [{tag}] {kind}: {} steps, {moved} with any difference; worst frame: mean |diff| {worst:.4} /255, {worst_pct:.4} % of pixels visibly different (avg {:.5} %)",
            path.len() - 1,
            sum_pct / (path.len() - 1) as f64
        );
        out.push((kind, worst, worst_pct));
    }
    h.destroy(&rig.gl);
    out
}

/// The worst frame may change at most this share (%) of the pixels when a terrain level snaps, on
/// the real island / on the synthetic hills. Measured without the geomorph: real 0.07..0.41,
/// synthetic 0.021 (hud150); with it: real 0.003..0.004, synthetic 0.004.
const POP_LIMIT_PCT: f64 = 0.05;
const POP_LIMIT_SYNTHETIC_PCT: f64 = 0.01;

/// Geomorphing (the hills-pop fix): the terrain levels re-centre every 16 * 2^l m, and without the
/// blend towards the coarser level in the outer cells the strip that switches jumps in height and
/// shading. Synthetic hills, 3 m steps over the big one.
#[test]
#[ignore = "needs an EGL device; writes PNGs (GL3D_PNG_DIR)"]
fn gl3d_terrain_levels_do_not_pop() {
    let Some(mut rig) = open(Flavour::Default, None, [1300, 760]) else { return };
    let w = world();
    let (_hold, tex) = rig.load_map(&w, crate::minimap::OVERLAY_MAP_TEXTURE_OPTIONS);
    let pts: Vec<(f32, f32)> = (0..120).map(|i| (-520.0 + 3.0 * i as f32, 150.0 + 0.6 * i as f32)).collect();
    let path = path_yaws(&pts);
    let res = pop_run(&mut rig, &w, tex, &path, "synthetic");
    if let Some(mut p) = rig.painter.take() {
        p.destroy();
    }
    for (kind, mean, pct) in res {
        assert!(pct < POP_LIMIT_SYNTHETIC_PCT, "{kind}: the worst frame changes {pct:.4} % of the pixels (mean {mean:.4}) when a terrain level snaps");
    }
}

/// The same on the real island along its hilliest straight road stretch (skipped without an install).
#[test]
#[ignore = "needs an EGL device and an FH6 install; writes PNGs"]
fn gl3d_real_install_terrain_levels_do_not_pop() {
    let Some(w) = real_world() else {
        eprintln!("SKIP gl3d_real_install_terrain_levels_do_not_pop: no FH6 install");
        return;
    };
    let Some(mut rig) = open(Flavour::Default, None, [1300, 760]) else { return };
    let (_hold, tex) = rig.load_map(&w, crate::minimap::OVERLAY_MAP_TEXTURE_OPTIONS);
    let path = hilly_path(&w, 3.0, 360.0);
    let res = pop_run(&mut rig, &w, tex, &path, "real");
    if let Some(mut p) = rig.painter.take() {
        p.destroy();
    }
    for (kind, mean, pct) in res {
        assert!(pct < POP_LIMIT_PCT, "{kind}: the worst frame changes {pct:.4} % of the pixels (mean {mean:.4}) when a terrain level snaps");
    }
}
