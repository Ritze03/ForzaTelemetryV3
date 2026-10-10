//! The car markers - the own car and the co-op teammates (D89) - and the trails in the 3D scene (D77 / D78): two small models built in code
//! (no asset files) - a 3D arrow and a low-poly sedan - and the trail ribbons, all placed at the
//! telemetry position *and height*, so in a tunnel they are at the road down there and on a
//! bridge on its deck, never on the hill surface above.
//!
//! # Models
//!
//! Built once per context from boxes / prisms ([`hexa`], [`prism_x`]) into flat-shaded triangle
//! lists, metres, `x` right, `y` up from the ground, `z` forward (the car's heading):
//!
//! * **Arrow** (14 triangles): the flat HUD arrow's chevron (apex ahead, a shallow notch at the
//!   back), extruded 0.5 m and topped by a low four-facet "gem" so the light reads it as 3D.
//! * **Sedan** (~200 triangles): lower body, a darker glass greenhouse with a roof slab in the
//!   body colour, four octagonal wheels, head- and tail-lights. Read from above (the usual map
//!   view) it is a body-coloured hood / roof / trunk split by the dark windscreen and rear window.
//!
//! Each vertex carries its face normal (flat shading), a smoothed normal (the outline pass pushes
//! the hull out along it) and a material: `colour * k + fixed`, so the body follows the car colour
//! (white solo, the co-op colour in a session) and glass / tyres / lights keep theirs.
//!
//! # Size
//!
//! Real size (the sedan 4.6 m, the arrow 5 m) when that is big enough, else scaled up so the model
//! is at least [`MarkerKind::min_pt`] points long at the car (times the HUD's size factor `s`):
//! `k = max(1, min_pt * s / (len_m * px_per_m_at_car))` ([`model_scale`]). *Why:* at HUD zoom
//! (500 m) a real car is 2 px; the flat arrow was ~14 pt, and the marker has to stay as readable.
//! Heights are divided by the exaggeration so the model is not stretched with the hills.
//!
//! # Visibility (the tunnel case)
//!
//! The markers are drawn **last, after the depth buffer is cleared**, all in one pass (teammates
//! first, the own car last = on top; each marker clears the depth buffer, so a later one is over an
//! earlier one): each always whole and on top
//! (like the tunnels, which are drawn without the depth test), still correctly self-occluded.
//! First a dark hull (the model pushed out ~1 px along its smoothed normals, no depth) for
//! the outline the flat arrow had, then the model over it. The trails are depth-tested twice: the
//! visible parts at full strength, the parts behind terrain (a stretch through a tunnel, or
//! behind a hill) a second time at [`GHOST`] strength with the test inverted - "seen through".

use egui::Color32;
use egui_glow::glow::{self, HasContext};

use super::as_bytes;
use crate::maprender::cfg::MarkerStyle;

/// One car in the scene (the own car, or a teammate: D89). `pos` is the telemetry position (x, y, z; y = the car's height in
/// metres, *not* the terrain's), `yaw` the telemetry yaw (0 = north, clockwise: forward is
/// `(sin yaw, cos yaw)` in (x, z)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marker3d {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub kind: MarkerStyle,
    pub colour: Color32,
}

/// One trail segment at its recorded heights; `alpha` 0..1 is the fade (`TrailFade`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrailSeg {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub alpha: f32,
}

/// One player's trail for the scene (`hud::map_shared::trail_3d` builds it from a `Trail`).
#[derive(Clone, Debug, PartialEq)]
pub struct Trail3d {
    pub segs: Vec<TrailSeg>,
    pub colour: Color32,
}

/// How far below the telemetry position the model's ground is: Forza reports roughly the car's
/// centre of mass, about half a metre above the road.
pub const GROUND_BELOW_M: f32 = 0.45;
/// Outline thickness of the marker, points (x `s`).
pub const OUTLINE_PT: f32 = 1.0;
/// Trail width in points (x `s`), as the flat trail's 2 px.
pub const TRAIL_PT: f32 = 2.0;
/// Strength of a trail's parts hidden behind terrain (through a tunnel, behind a hill).
pub const GHOST: f32 = 0.45;

/// The model data of one marker kind.
pub trait MarkerKind {
    /// Real length, metres.
    fn len_m(self) -> f32;
    /// Minimum on-screen length at the car, points (x the size factor).
    fn min_pt(self) -> f32;
}

impl MarkerKind for MarkerStyle {
    fn len_m(self) -> f32 {
        match self {
            MarkerStyle::Arrow => 5.0,
            MarkerStyle::Sedan => 4.6,
        }
    }
    fn min_pt(self) -> f32 {
        match self {
            // The flat arrow is 14 pt from apex to base; the car is narrower, so a bit longer.
            MarkerStyle::Arrow => 16.0,
            MarkerStyle::Sedan => 20.0,
        }
    }
}

/// The model's scale factor: 1 = real size, more when the real size would be smaller than the
/// kind's minimum on screen. `px_per_m` = points per metre at the car (`Camera::view.scale` x the
/// perspective factor there), `s` = the size factor (HUD design -> screen, 1 elsewhere).
pub fn model_scale(kind: MarkerStyle, px_per_m: f32, s: f32) -> f32 {
    if !(px_per_m.is_finite() && px_per_m > 0.0) {
        return 1.0;
    }
    (kind.min_pt() * s.max(0.1) / (kind.len_m() * px_per_m)).max(1.0)
}

/// Where a model-space point (metres, x right, y up, z forward) lands in the world for a marker at
/// `pos` heading `yaw`, scaled `k`, under the exaggeration `exag`: the very maths of `MARKER_VS`.
/// The returned y is in real metres (the shader multiplies it by `exag` like every world point).
pub fn model_to_world(local: [f32; 3], pos: [f32; 3], yaw: f32, k: f32, exag: f32) -> [f32; 3] {
    let (s, c) = yaw.sin_cos();
    let (lx, ly, lz) = (local[0] * k, local[1] * k, local[2] * k);
    // right = (cos, 0, -sin), forward = (sin, 0, cos)
    [pos[0] + lx * c + lz * s, pos[1] - GROUND_BELOW_M + ly / exag.max(0.01), pos[2] - lx * s + lz * c]
}

// ── geometry ─────────────────────────────────────────────────────────────────────────────────

/// Floats per vertex: position 3, face normal 3, smoothed normal 3, material 4.
pub const FLOATS: usize = 13;

/// A material: `rgb = colour * k + fixed`.
#[derive(Clone, Copy)]
struct Mat([f32; 4]);

const BODY: Mat = Mat([1.0, 0.0, 0.0, 0.0]);
const GLASS: Mat = Mat([0.16, 0.05, 0.07, 0.10]);
const TYRE: Mat = Mat([0.0, 0.08, 0.08, 0.09]);
const HEAD: Mat = Mat([0.0, 1.0, 0.96, 0.78]);
const TAIL: Mat = Mat([0.0, 0.85, 0.07, 0.07]);

/// A triangle list being built: one part (a closed solid) at a time, so its smoothed normals only
/// average its own faces.
#[derive(Default)]
pub struct Model {
    /// [`FLOATS`] per vertex, 3 vertices per triangle.
    pub v: Vec<f32>,
}

impl Model {
    pub fn triangles(&self) -> usize {
        self.v.len() / FLOATS / 3
    }

    /// Add one closed part: `tris` as corner triples, each turned to face away from `centre`.
    fn part(&mut self, tris: &[[[f32; 3]; 3]], centre: [f32; 3], m: Mat) {
        let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let norm = |a: [f32; 3]| {
            let l = dot(a, a).sqrt().max(1e-9);
            [a[0] / l, a[1] / l, a[2] / l]
        };
        // Face normals, outward.
        let mut faces: Vec<([[f32; 3]; 3], [f32; 3])> = Vec::with_capacity(tris.len());
        for t in tris {
            let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
            if dot(n, n) < 1e-12 {
                continue; // degenerate
            }
            let mid = [(t[0][0] + t[1][0] + t[2][0]) / 3.0, (t[0][1] + t[1][1] + t[2][1]) / 3.0, (t[0][2] + t[1][2] + t[2][2]) / 3.0];
            let (tri, n) = if dot(n, sub(mid, centre)) < 0.0 { ([t[0], t[2], t[1]], [-n[0], -n[1], -n[2]]) } else { (*t, n) };
            faces.push((tri, norm(n)));
        }
        // Smoothed normals: the mean of the face normals at a corner, divided by its squared
        // length, so pushing a box corner out by g moves each of its faces out by exactly g.
        let key = |p: [f32; 3]| [(p[0] * 1000.0).round() as i32, (p[1] * 1000.0).round() as i32, (p[2] * 1000.0).round() as i32];
        let mut acc: Vec<([i32; 3], [f32; 3], Vec<[f32; 3]>)> = Vec::new();
        for (tri, n) in &faces {
            for p in tri {
                let k = key(*p);
                match acc.iter_mut().find(|e| e.0 == k) {
                    Some(e) => {
                        // one normal per distinct face direction (two triangles of a quad count once)
                        if !e.2.iter().any(|q| dot(*q, *n) > 0.999) {
                            e.2.push(*n);
                        }
                    }
                    None => acc.push((k, *p, vec![*n])),
                }
            }
        }
        let smooth = |p: [f32; 3]| -> [f32; 3] {
            let e = acc.iter().find(|e| e.0 == key(p)).expect("every corner was added");
            let mut s = [0.0f32; 3];
            for q in &e.2 {
                s = [s[0] + q[0], s[1] + q[1], s[2] + q[2]];
            }
            let c = e.2.len() as f32;
            let m = [s[0] / c, s[1] / c, s[2] / c];
            let l2 = dot(m, m).max(0.25); // at most twice the push (sharp corners)
            [m[0] / l2, m[1] / l2, m[2] / l2]
        };
        for (tri, n) in &faces {
            for p in tri {
                let sm = smooth(*p);
                self.v.extend_from_slice(&[p[0], p[1], p[2], n[0], n[1], n[2], sm[0], sm[1], sm[2]]);
                self.v.extend_from_slice(&m.0);
            }
        }
    }
}

/// A six-faced solid from its bottom ring `b` and top ring `t` (same order around): 12 triangles.
fn hexa(b: [[f32; 3]; 4], t: [[f32; 3]; 4]) -> (Vec<[[f32; 3]; 3]>, [f32; 3]) {
    let mut tris = vec![[b[0], b[1], b[2]], [b[0], b[2], b[3]], [t[0], t[1], t[2]], [t[0], t[2], t[3]]];
    for i in 0..4 {
        let j = (i + 1) % 4;
        tris.push([b[i], b[j], t[j]]);
        tris.push([b[i], t[j], t[i]]);
    }
    let mut c = [0.0f32; 3];
    for p in b.iter().chain(&t) {
        c = [c[0] + p[0] / 8.0, c[1] + p[1] / 8.0, c[2] + p[2] / 8.0];
    }
    (tris, c)
}

/// A box between `y0` / `y1`; its bottom spans `x` ±`bw`, `z` `bz`, its top ±`tw`, `tz` (a
/// tapered box: a greenhouse, a hood).
fn tapered(y0: f32, y1: f32, bw: f32, bz: [f32; 2], tw: f32, tz: [f32; 2]) -> (Vec<[[f32; 3]; 3]>, [f32; 3]) {
    hexa(
        [[-bw, y0, bz[0]], [bw, y0, bz[0]], [bw, y0, bz[1]], [-bw, y0, bz[1]]],
        [[-tw, y1, tz[0]], [tw, y1, tz[0]], [tw, y1, tz[1]], [-tw, y1, tz[1]]],
    )
}

/// A box `x0..x1`, `y0..y1`, `z0..z1`.
fn cuboid(x: [f32; 2], y: [f32; 2], z: [f32; 2]) -> (Vec<[[f32; 3]; 3]>, [f32; 3]) {
    hexa(
        [[x[0], y[0], z[0]], [x[1], y[0], z[0]], [x[1], y[0], z[1]], [x[0], y[0], z[1]]],
        [[x[0], y[1], z[0]], [x[1], y[1], z[0]], [x[1], y[1], z[1]], [x[0], y[1], z[1]]],
    )
}

/// An `n`-sided prism along x (a wheel): centre (y, z), radius `r`, from `x0` to `x1`.
fn prism_x(n: usize, x0: f32, x1: f32, y: f32, z: f32, r: f32) -> (Vec<[[f32; 3]; 3]>, [f32; 3]) {
    let ring = |x: f32| -> Vec<[f32; 3]> {
        (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                [x, y + r * a.sin(), z + r * a.cos()]
            })
            .collect()
    };
    let (a, b) = (ring(x0), ring(x1));
    let mut tris = Vec::new();
    for i in 0..n {
        let j = (i + 1) % n;
        tris.push([a[i], a[j], b[j]]);
        tris.push([a[i], b[j], b[i]]);
        if i > 0 && j > 0 {
            tris.push([a[0], a[i], a[j]]);
            tris.push([b[0], b[i], b[j]]);
        }
    }
    (tris, [(x0 + x1) * 0.5, y, z])
}

/// The 3D arrow: the flat arrow's chevron, 5 m long and 3.8 m wide, extruded 0.5 m, a low
/// four-facet gem on top. 14 triangles.
pub fn arrow_model() -> Model {
    let (h, ridge) = (0.5f32, 0.95f32);
    let tip = [0.0, 3.0];
    let right = [1.9, -2.0];
    let notch = [0.0, -1.3];
    let left = [-1.9, -2.0];
    let ring = [tip, right, notch, left];
    let at = |p: [f32; 2], y: f32| [p[0], y, p[1]];
    let apex = [0.0, ridge, -0.25];
    let mut tris = vec![
        // bottom (a concave quad: two triangles through the notch)
        [at(tip, 0.0), at(right, 0.0), at(notch, 0.0)],
        [at(tip, 0.0), at(notch, 0.0), at(left, 0.0)],
    ];
    for i in 0..4 {
        let j = (i + 1) % 4;
        tris.push([at(ring[i], 0.0), at(ring[j], 0.0), at(ring[j], h)]);
        tris.push([at(ring[i], 0.0), at(ring[j], h), at(ring[i], h)]);
        tris.push([at(ring[i], h), at(ring[j], h), apex]);
    }
    let mut m = Model::default();
    // The centre is inside the solid near the apex column: every face's outward side faces away from it.
    m.part(&tris, [0.0, 0.3, -0.2], BODY);
    m
}

/// The low-poly sedan, 4.6 m long, 1.84 m wide, 1.42 m tall.
pub fn sedan_model() -> Model {
    let mut m = Model::default();
    let mut add = |(t, c): (Vec<[[f32; 3]; 3]>, [f32; 3]), mat: Mat| m.part(&t, c, mat);
    // Lower body: slightly narrower and shorter on top (rounded-off shoulders, nose and tail).
    add(tapered(0.26, 0.80, 0.92, [-2.3, 2.3], 0.88, [-2.22, 2.2]), BODY);
    // Greenhouse: windscreen raked forward, rear window steeper; glass.
    add(tapered(0.80, 1.34, 0.82, [-1.30, 0.80], 0.68, [-0.95, 0.12]), GLASS);
    // Roof slab in the body colour.
    add(tapered(1.34, 1.42, 0.69, [-0.96, 0.13], 0.64, [-0.90, 0.07]), BODY);
    // Wheels: octagonal, sticking out of the body sides a little.
    for (x0, x1) in [(-0.95, -0.70), (0.70, 0.95)] {
        for z in [-1.42, 1.42] {
            add(prism_x(8, x0, x1, 0.34, z, 0.34), TYRE);
        }
    }
    // Head- and tail-lights.
    for sx in [-1.0f32, 1.0] {
        let x = if sx < 0.0 { [-0.82, -0.48] } else { [0.48, 0.82] };
        add(cuboid(x, [0.56, 0.70], [2.16, 2.32]), HEAD);
        add(cuboid(x, [0.58, 0.72], [-2.34, -2.18]), TAIL);
    }
    m
}

// ── GPU side ─────────────────────────────────────────────────────────────────────────────────

/// One model on the GPU.
pub struct ModelGpu {
    pub vao: glow::VertexArray,
    vbo: glow::Buffer,
    pub count: i32,
}

impl ModelGpu {
    pub fn upload(gl: &glow::Context, m: &Model) -> Result<ModelGpu, String> {
        // SAFETY: plain buffer setup on the current context; the VAO is unbound again.
        unsafe {
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));
            let vbo = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&m.v), glow::STATIC_DRAW);
            let stride = (FLOATS * 4) as i32;
            for (loc, n, off) in [(0u32, 3, 0), (1, 3, 12), (2, 3, 24), (3, 4, 36)] {
                gl.enable_vertex_attrib_array(loc);
                gl.vertex_attrib_pointer_f32(loc, n, glow::FLOAT, false, stride, off);
            }
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            Ok(ModelGpu { vao, vbo, count: (m.v.len() / FLOATS) as i32 })
        }
    }

    pub fn destroy(&self, gl: &glow::Context) {
        // SAFETY: deleting objects this struct created.
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
        }
    }
}

/// Floats per trail vertex: this end 3, the other end 3, side, alpha.
pub const TRAIL_FLOATS: usize = 8;

/// The trail ribbons as one triangle list: 6 vertices per segment, each carrying both ends so
/// the vertex shader can widen it to a constant screen width. `rgb` per trail goes in a uniform,
/// so the list is per trail: `(first vertex, count)` per entry of `trails`.
pub fn trail_vertices(trails: &[Trail3d]) -> (Vec<f32>, Vec<(i32, i32)>) {
    let mut v = Vec::new();
    let mut ranges = Vec::with_capacity(trails.len());
    for t in trails {
        let first = (v.len() / TRAIL_FLOATS) as i32;
        for s in &t.segs {
            let al = s.alpha.clamp(0.0, 1.0);
            // P = this end, Q = the other; the side flips with the direction at the far end, so
            // (a,+1) and (b,-1) are on the same side of the line.
            let mut put = |p: [f32; 3], q: [f32; 3], side: f32| v.extend_from_slice(&[p[0], p[1], p[2], q[0], q[1], q[2], side, al]);
            put(s.a, s.b, 1.0);
            put(s.a, s.b, -1.0);
            put(s.b, s.a, -1.0);
            put(s.a, s.b, -1.0);
            put(s.b, s.a, 1.0);
            put(s.b, s.a, -1.0);
        }
        ranges.push((first, (v.len() / TRAIL_FLOATS) as i32 - first));
    }
    (v, ranges)
}

/// The streamed trail buffer (re-filled every frame that has trails).
pub struct TrailGpu {
    pub vao: glow::VertexArray,
    pub vbo: glow::Buffer,
}

impl TrailGpu {
    pub fn new(gl: &glow::Context) -> Result<TrailGpu, String> {
        // SAFETY: plain buffer setup on the current context.
        unsafe {
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));
            let vbo = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = (TRAIL_FLOATS * 4) as i32;
            for (loc, n, off) in [(0u32, 3, 0), (1, 3, 12), (2, 2, 24)] {
                gl.enable_vertex_attrib_array(loc);
                gl.vertex_attrib_pointer_f32(loc, n, glow::FLOAT, false, stride, off);
            }
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            Ok(TrailGpu { vao, vbo })
        }
    }

    pub fn destroy(&self, gl: &glow::Context) {
        // SAFETY: deleting objects this struct created.
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(m: &Model) -> ([f32; 3], [f32; 3]) {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in m.v.chunks(FLOATS) {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        (lo, hi)
    }

    #[test]
    fn models_are_small_closed_solids_of_the_documented_size() {
        let (a, s) = (arrow_model(), sedan_model());
        assert_eq!(a.triangles(), 14);
        assert!((100..=400).contains(&s.triangles()), "sedan: {} triangles", s.triangles());
        for (m, len) in [(&a, MarkerStyle::Arrow.len_m()), (&s, MarkerStyle::Sedan.len_m())] {
            let (lo, hi) = bounds(m);
            assert!(lo[1].abs() < 1e-6, "sits on y = 0: {lo:?}");
            assert!(((hi[2] - lo[2]) - len).abs() < 0.2, "length {} vs {len}", hi[2] - lo[2]);
            assert!(hi[0] + lo[0] < 1e-4, "symmetric in x");
            // Forward is +z: the arrow's apex and the sedan's headlights are ahead.
            assert!(hi[2] > -lo[2] - 0.2);
            // Every face normal is a unit vector; the smoothed ones push outward (away from the
            // model's vertical axis or up).
            for v in m.v.chunks(FLOATS) {
                let n = (v[3] * v[3] + v[4] * v[4] + v[5] * v[5]).sqrt();
                assert!((n - 1.0).abs() < 1e-3);
                // (a tapered solid's bottom corner pushes out sideways, a little against its bottom face)
                assert!(v[3] * v[6] + v[4] * v[7] + v[5] * v[8] > -0.3, "smoothed normal against its face: len {len} {v:?}");
            }
        }
        // The arrow's top faces point up (lit), its bottom down.
        let ups = a.v.chunks(FLOATS).filter(|v| v[4] > 0.5).count();
        let downs = a.v.chunks(FLOATS).filter(|v| v[4] < -0.99).count();
        assert_eq!((ups, downs), (12, 6));
    }

    #[test]
    fn marker_transform_places_the_model_at_the_car_heading_its_yaw() {
        let pos = [100.0, 50.0, -200.0];
        // Model origin: the ground under the telemetry point.
        let o = model_to_world([0.0, 0.0, 0.0], pos, 0.7, 1.0, 1.0);
        assert!((o[0] - 100.0).abs() < 1e-4 && (o[1] - (50.0 - GROUND_BELOW_M)).abs() < 1e-4 && (o[2] + 200.0).abs() < 1e-4);
        // Yaw 0 = north: forward is +z; a quarter turn clockwise: forward is east (+x), right is south.
        let f = model_to_world([0.0, 0.0, 2.0], pos, 0.0, 1.0, 1.0);
        assert!((f[2] - (-198.0)).abs() < 1e-4 && (f[0] - 100.0).abs() < 1e-4);
        let f = model_to_world([0.0, 0.0, 2.0], pos, std::f32::consts::FRAC_PI_2, 1.0, 1.0);
        assert!((f[0] - 102.0).abs() < 1e-4 && (f[2] + 200.0).abs() < 1e-4, "{f:?}");
        let r = model_to_world([1.0, 0.0, 0.0], pos, std::f32::consts::FRAC_PI_2, 1.0, 1.0);
        assert!((r[2] - (-201.0)).abs() < 1e-4, "right of an east-bound car is south: {r:?}");
        // Scale grows the model about its ground point; the exaggeration does not stretch it.
        let t = model_to_world([0.0, 1.4, 0.0], pos, 0.0, 3.0, 2.0);
        assert!((t[1] - (50.0 - GROUND_BELOW_M + 1.4 * 3.0 / 2.0)).abs() < 1e-4);
    }

    #[test]
    fn model_scale_is_real_size_unless_that_is_too_small() {
        // Viewer zoomed in: 20 pt per metre -> a 4.6 m car is 92 pt: real size.
        assert_eq!(model_scale(MarkerStyle::Sedan, 20.0, 1.0), 1.0);
        // HUD at 500 m: ~0.14 pt per metre -> scaled to the minimum, 20 pt x s.
        let k = model_scale(MarkerStyle::Sedan, 0.136, 1.5);
        assert!((4.6 * k * 0.136 - 20.0 * 1.5).abs() < 1e-3, "{k}");
        let k = model_scale(MarkerStyle::Arrow, 0.136, 1.0);
        assert!((5.0 * k * 0.136 - 16.0).abs() < 1e-3);
        // Garbage in: real size.
        assert_eq!(model_scale(MarkerStyle::Arrow, f32::NAN, 1.0), 1.0);
        assert_eq!(model_scale(MarkerStyle::Arrow, 0.0, 1.0), 1.0);
    }

    #[test]
    fn trail_vertices_carry_the_heights_and_the_fade() {
        let t = Trail3d {
            segs: vec![TrailSeg { a: [0.0, 150.0, 0.0], b: [10.0, 151.0, 0.0], alpha: 0.5 }, TrailSeg { a: [10.0, 151.0, 0.0], b: [20.0, 152.0, 5.0], alpha: 0.9 }],
            colour: Color32::WHITE,
        };
        let (v, r) = trail_vertices(&[t.clone(), t]);
        assert_eq!(r, vec![(0, 12), (12, 12)]);
        assert_eq!(v.len(), 24 * TRAIL_FLOATS);
        let vert = |i: usize| &v[i * TRAIL_FLOATS..(i + 1) * TRAIL_FLOATS];
        // First segment: ends at the recorded heights (a tunnel's 150 m stays 150 m), its alpha.
        assert_eq!(&vert(0)[..6], &[0.0, 150.0, 0.0, 10.0, 151.0, 0.0]);
        assert_eq!(&vert(2)[..6], &[10.0, 151.0, 0.0, 0.0, 150.0, 0.0]);
        assert_eq!(vert(0)[7], 0.5);
        assert_eq!(vert(6)[7], 0.9);
        // The two triangles of a quad cover both sides at both ends.
        let sides: Vec<(bool, f32)> = (0..6).map(|i| (vert(i)[0] == 0.0, vert(i)[6])).collect();
        assert!(sides.contains(&(true, 1.0)) && sides.contains(&(true, -1.0)) && sides.contains(&(false, 1.0)) && sides.contains(&(false, -1.0)));
    }
}
