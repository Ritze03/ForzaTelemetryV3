//! The 2D renderer, egui `Painter` output: the base image ([`draw_base`]) and the vector layers
//! ([`draw_layers`]: roads, jump lines, race lines, POIs). Both maps call these two functions
//! with different parameters (D61).
//!
//! Approach (measured in the design scout, `docs/features/minimap.md`): one `Shape::line` per
//! visible road chain, a casing pass and a fill pass per type, chains culled by their bounding
//! box against the view's world box, vertices thinned to 2 px on screen. No baking into the
//! map texture and no pre-tessellated world meshes: road types change on every editor Save,
//! types are toggled per map, and widths are in screen px and change with the eased zoom.

use egui::epaint::Vertex;
use egui::{pos2, vec2, Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke, TextureId};

use super::cfg::{ImageCfg, MapLayerConfig, RaceCfg, RaceLineMode, Rgb};
use super::data::{MapLayers, NO_CAT};
use super::style::{self, Shape as Marker};
use super::view::{bbox_hits, clip_convex, clip_polyline_convex, fan, inside_convex, thin, Camera};
use super::MapTex;
use crate::gamedata::roadtypes::RoadType;
use crate::minimap::MapCalibration;

// ── base image ───────────────────────────────────────────────────────────────────────────────

/// The image's look (from [`ImageCfg`]) times the caller's fade alpha.
#[derive(Clone, Copy, Debug)]
pub struct ImageLook {
    pub opacity: f32,
    pub brightness: f32,
    pub saturation: f32,
}

impl From<&ImageCfg> for ImageLook {
    fn from(c: &ImageCfg) -> Self {
        Self { opacity: c.opacity, brightness: c.brightness, saturation: c.saturation }
    }
}

impl ImageLook {
    #[cfg(test)]
    pub const FULL: ImageLook = ImageLook { opacity: 1.0, brightness: 1.0, saturation: 1.0 };
}

/// Where and how to draw the base image.
pub struct BaseParams<'a> {
    pub cam: &'a Camera,
    pub cal: MapCalibration,
    pub tex: MapTex,
    /// The convex polygon (screen space, either winding) the map fills: the widget rect's four
    /// corners on the Dashboard, the rounded pill's outline on the HUD.
    pub outline: &'a [Pos2],
    /// Mirror the map past its edges (the texture wraps with `MirroredRepeat`); off = the
    /// shape is cut to the image and whatever is behind shows outside it.
    pub mirror: bool,
    pub look: ImageLook,
    /// Fade alpha (the HUD's show/hide fade; 1.0 on the Dashboard).
    pub a: f32,
}

/// Cells per side of the tilted map's mesh: egui interpolates UVs affinely inside a triangle,
/// the perspective is not affine, so the plane is subdivided until the error is invisible.
pub const TILT_CELLS: usize = 24;

/// The map image under the camera. Flat: a triangle fan over `outline` (cut to the image when
/// not mirroring) with per-vertex UVs from the inverse mapping, which is exact for an affine
/// map. Tilted: the visible part of the map plane in `TILT_CELLS`² cells, each projected and
/// cut to `outline`, with each vertex's UV taken from the inverse projection.
pub fn draw_base(p: &Painter, m: &BaseParams) {
    let cam = m.cam;
    let look = m.look;
    let o = (look.opacity * m.a).clamp(0.0, 1.0);
    let v = (look.brightness.clamp(0.0, 1.0) * o * 255.0).round() as u8;
    let col = Color32::from_rgba_premultiplied(v, v, v, (o * 255.0).round() as u8);
    let mut img = Mesh::with_texture(m.tex.id);
    // Saturation below 1 is approximated by a grey veil over the same shapes (egui cannot
    // desaturate a texture); built alongside the image mesh.
    let veil_a = ((1.0 - look.saturation.clamp(0.0, 1.0)) * 0.6 * o * 255.0).round() as u8;
    let mut veil = Mesh::default();
    let veil_col = Color32::from_rgba_unmultiplied(110, 110, 110, veil_a);

    let uv_of = |pt: Pos2| -> Pos2 {
        match cam.unproject(pt) {
            Some([wx, wz]) => {
                let [u, vv] = m.cal.world_to_uv(wx, wz, m.tex.orig_size);
                pos2(u, vv)
            }
            None => pos2(0.0, 0.0),
        }
    };
    let mut add = |shape: &[Pos2]| {
        if shape.len() < 3 {
            return;
        }
        let hub = (shape.iter().fold(vec2(0.0, 0.0), |a, q| a + q.to_vec2()) / shape.len() as f32).to_pos2();
        fan(&mut img, hub, shape, |pt| Vertex { pos: pt, uv: uv_of(pt), color: col });
        if veil_a > 0 {
            fan(&mut veil, hub, shape, |pt| Vertex { pos: pt, uv: egui::epaint::WHITE_UV, color: veil_col });
        }
    };

    // The image's corners as plane offsets (px from the car, before any perspective).
    let corners: Vec<Pos2> = m
        .cal
        .image_corners(m.tex.orig_size)
        .iter()
        .map(|&(wx, wz, _)| {
            let [ox, oy] = cam.view.world_to_offset(wx, wz);
            pos2(ox, oy)
        })
        .collect();

    if cam.is_flat() {
        let shape = if m.mirror {
            m.outline.to_vec()
        } else {
            let quad: Vec<Pos2> = corners.iter().map(|c| cam.centre + c.to_vec2()).collect();
            clip_convex(m.outline, &quad)
        };
        add(&shape);
    } else {
        let pr = cam.plane_rect();
        let n = TILT_CELLS;
        let (dx, dy) = (pr.width() / n as f32, pr.height() / n as f32);
        let bounds = outline_bounds(m.outline);
        let rect_outline = m.outline.len() == 4;
        for i in 0..n {
            for j in 0..n {
                let (x0, y0) = (pr.min.x + dx * i as f32, pr.min.y + dy * j as f32);
                let cell = [pos2(x0, y0), pos2(x0 + dx, y0), pos2(x0 + dx, y0 + dy), pos2(x0, y0 + dy)];
                let plane: Vec<Pos2> = if m.mirror { cell.to_vec() } else { clip_convex(&cell, &corners) };
                if plane.len() < 3 {
                    continue;
                }
                let Some(screen) = plane.iter().map(|q| cam.project_offset(q.x, q.y)).collect::<Option<Vec<Pos2>>>() else { continue };
                let sb = Rect::from_points(&screen);
                if !sb.intersects(bounds) {
                    continue;
                }
                if rect_outline && bounds.contains_rect(sb) {
                    add(&screen);
                } else {
                    add(&clip_convex(&screen, m.outline));
                }
            }
        }
    }
    if !img.is_empty() {
        p.add(Shape::mesh(img));
    }
    if !veil.is_empty() {
        p.add(Shape::mesh(veil));
    }
}

fn outline_bounds(outline: &[Pos2]) -> Rect {
    Rect::from_points(outline)
}

// ── layers ───────────────────────────────────────────────────────────────────────────────────

/// POI icons: one texture plus a UV rect per category (the order of `style::POI_CATS`). The
/// game's own icons come from the icon decoder; until one is wired, coloured markers are drawn.
#[derive(Clone, Debug)]
pub struct IconAtlas {
    pub texture: TextureId,
    pub rects: Vec<Option<Rect>>,
}

#[allow(dead_code)] // the icon decoder fills it (I29b)
impl IconAtlas {
    pub fn new(texture: TextureId) -> IconAtlas {
        IconAtlas { texture, rects: vec![None; style::POI_CATS.len()] }
    }
    /// Set the UV rect of category `id` (a `style::POI_CATS` id); unknown ids are ignored.
    pub fn set(&mut self, id: &str, uv: Rect) {
        if let Some(i) = style::cat_index(id) {
            self.rects[i] = Some(uv);
        }
    }
    fn uv(&self, cat: usize) -> Option<Rect> {
        self.rects.get(cat).copied().flatten()
    }
}

/// Keeps vectors inside a non-rectangular map shape (the HUD pill's rounded corners): anything
/// outside `safe` is tested against / cut to `poly`.
#[derive(Clone, Copy)]
pub struct CornerClip<'a> {
    pub poly: &'a [Pos2],
    /// A rect fully inside `poly` (the pill's rect shrunk by its corner radius, or more).
    pub safe: Rect,
}

/// Per-frame inputs of [`draw_layers`].
pub struct LayerCtx<'a> {
    /// Painter clipped to the map area.
    pub p: &'a Painter,
    pub cam: &'a Camera,
    /// Size factor (strokes, markers): 1.0 on the Dashboard, the HUD's design→screen scale.
    pub s: f32,
    /// Alpha applied to every colour (the HUD's fade).
    pub a: f32,
    /// The car, world x, z.
    pub car: (f32, f32),
    pub corner_clip: Option<CornerClip<'a>>,
    pub icons: Option<&'a IconAtlas>,
    /// Race lines to draw for the Nearest / Near / Current modes (`RaceSel::picked`); `All`
    /// ignores it.
    pub race_sel: &'a [usize],
}

/// What a [`draw_layers`] call drew (tests, perf numbers).
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerStats {
    pub chains: usize,
    pub vertices: usize,
    pub pois: usize,
    pub race_lines: usize,
}

/// Vector layers over the base image: roads (bottom to top: `style::ROAD_DRAW_ORDER`), jump
/// lines, race lines with start / finish marks, POIs. Each part is skipped when its switch is off.
pub fn draw_layers(cx: &LayerCtx, layers: &MapLayers, cfg: &MapLayerConfig) -> LayerStats {
    let mut st = LayerStats::default();
    if cfg.roads.on {
        draw_roads(cx, layers, cfg, &mut st);
    }
    if cfg.race_lines.mode != RaceLineMode::Off {
        draw_race_lines(cx, layers, &cfg.race_lines, &mut st);
    }
    if cfg.pois.on {
        draw_pois(cx, layers, cfg, &mut st);
    }
    st
}

impl LayerCtx<'_> {
    fn c(&self, rgb: Rgb, alpha: f32) -> Color32 {
        rgb.color(alpha * self.a)
    }

    /// Project a world polyline: thinned to `THIN_PX`, broken where a vertex is not in front of
    /// the eye, cut to the corner clip. Pieces are appended to `out`.
    fn polyline(&self, pts: &[[f32; 2]], close: bool, out: &mut Vec<Vec<Pos2>>, scratch: &mut Vec<Pos2>) {
        scratch.clear();
        let cam = self.cam;
        let flush = |scratch: &mut Vec<Pos2>, out: &mut Vec<Vec<Pos2>>, cc: &Option<CornerClip>| {
            if scratch.len() >= 2 {
                thin(scratch, style::THIN_PX);
                match cc {
                    Some(cc) if scratch.iter().any(|q| !cc.safe.contains(*q)) => out.extend(clip_polyline_convex(scratch, cc.poly)),
                    _ => out.push(scratch.clone()),
                }
            }
            scratch.clear();
        };
        let n = pts.len() + usize::from(close && pts.len() > 2);
        for i in 0..n {
            let q = pts[i % pts.len()];
            match cam.project(q[0], q[1]) {
                Some(s) => scratch.push(s),
                None => flush(scratch, out, &self.corner_clip),
            }
        }
        flush(scratch, out, &self.corner_clip);
    }

    /// Is the screen point inside the map shape (corner clip) and the painter's clip rect margin?
    fn visible(&self, at: Pos2, margin: f32) -> bool {
        if !self.cam.rect.expand(margin).contains(at) {
            return false;
        }
        match &self.corner_clip {
            Some(cc) if !cc.safe.contains(at) => inside_convex(at, cc.poly),
            _ => true,
        }
    }
}

fn draw_roads(cx: &LayerCtx, layers: &MapLayers, cfg: &MapLayerConfig, st: &mut LayerStats) {
    let cam = cx.cam;
    let c = &cfg.roads;
    let aabb = cam.world_aabb(30.0);
    let base = style::road_base_px(c, cam.scale()) * cx.s;
    let dashes = cam.scale() >= style::DASH_MIN_PX_PER_M;
    let mut scratch: Vec<Pos2> = Vec::new();
    let mut lines: Vec<Vec<Pos2>> = Vec::new();

    for &slot in &style::ROAD_DRAW_ORDER {
        // (colour, alpha, width factor, dash, casing colour)
        let (color, alpha, factor, dash, casing) = if slot == 0 {
            (style::UNSET_COLOR, style::UNSET_ALPHA, style::UNSET_WIDTH, super::cfg::DashStyle::Dashed, None)
        } else {
            let Some(s) = RoadType::from_index(slot as u8).and_then(|t| c.styles.get(t)) else { continue };
            if !s.on {
                continue;
            }
            (s.color, s.alpha, s.width, s.dash, s.casing.then_some(s.casing_color))
        };
        lines.clear();
        for ch in &layers.roads.by_type[slot] {
            if bbox_hits(&ch.bbox, &aabb) {
                cx.polyline(&ch.pts, false, &mut lines, &mut scratch);
                st.chains += 1;
            }
        }
        if lines.is_empty() {
            continue;
        }
        st.vertices += lines.iter().map(Vec::len).sum::<usize>();
        let w = style::line_px(base, factor);
        if let Some(cc) = casing {
            let stroke = Stroke::new(w + c.casing_px * cx.s, cx.c(cc, c.casing_alpha));
            for l in &lines {
                cx.p.add(Shape::line(l.clone(), stroke));
            }
        }
        let stroke = Stroke::new(w, cx.c(color, alpha));
        match style::dash_pattern(dash, w).filter(|_| dashes) {
            Some((d, g)) => {
                for l in &lines {
                    cx.p.extend(Shape::dashed_line(l, stroke, d, g));
                }
            }
            None => {
                for l in lines.drain(..) {
                    cx.p.add(Shape::line(l, stroke));
                }
            }
        }
    }

    // Jump lines: take-off → landing, drawn after the chains.
    let Some(js) = c.styles.get(RoadType::Jump).filter(|s| s.on) else { return };
    let w = (base * js.width).max(1.4);
    for j in &layers.roads.jumps {
        let bb = [j[0].min(j[3]), j[1].min(j[4]), j[0].max(j[3]), j[1].max(j[4])];
        if !bbox_hits(&bb, &aabb) {
            continue;
        }
        let (Some(a), Some(b)) = (cam.project(j[0], j[1]), cam.project(j[3], j[4])) else { continue };
        st.chains += 1;
        let seg = [a, b];
        if js.casing {
            cx.p.add(Shape::line_segment(seg, Stroke::new(w + 1.8 * cx.s, cx.c(js.casing_color, c.casing_alpha))));
        }
        let stroke = Stroke::new(w, cx.c(js.color, js.alpha));
        match js.dash {
            super::cfg::DashStyle::None => {
                cx.p.add(Shape::line_segment(seg, stroke));
            }
            _ => {
                cx.p.extend(Shape::dashed_line(&seg, stroke, 4.0 * cx.s, 3.0 * cx.s));
            }
        }
    }
}

/// Vertex budget for the "all lines" mode (one frame).
const RACE_ALL_BUDGET: usize = 40_000;

fn draw_race_lines(cx: &LayerCtx, layers: &MapLayers, rc: &RaceCfg, st: &mut LayerStats) {
    let aabb = cx.cam.world_aabb(20.0);
    let lines = &layers.races.lines;
    let idx: Vec<usize> = if rc.mode == RaceLineMode::All { (0..lines.len()).collect() } else { cx.race_sel.iter().copied().filter(|&i| i < lines.len()).collect() };
    let (mut scratch, mut pieces) = (Vec::new(), Vec::new());
    let mut budget = RACE_ALL_BUDGET;
    for i in idx {
        let l = &lines[i];
        if !bbox_hits(&l.bbox, &aabb) {
            continue;
        }
        let color = cx.c(if l.circuit { rc.circuit_color } else { rc.sprint_color }, rc.alpha);
        pieces.clear();
        cx.polyline(&l.pts, l.closed, &mut pieces, &mut scratch);
        let n: usize = pieces.iter().map(Vec::len).sum();
        if rc.mode == RaceLineMode::All {
            if n > budget {
                continue;
            }
            budget -= n;
        }
        st.race_lines += 1;
        st.vertices += n;
        for pc in pieces.drain(..) {
            cx.p.add(Shape::line(pc, Stroke::new(rc.width_px * cx.s, color)));
        }
        if rc.marks {
            draw_race_marks(cx, l);
        }
    }
}

fn draw_race_marks(cx: &LayerCtx, l: &crate::gamedata::racelines::RaceLine) {
    let (Some(&first), Some(&last)) = (l.pts.first(), l.pts.last()) else { return };
    let size = 9.0 * cx.s;
    if let Some(at) = cx.cam.project(first[0], first[1]).filter(|a| cx.visible(*a, size)) {
        if l.circuit {
            chequer(cx, at, size);
        } else {
            cx.p.circle(at, 4.5 * cx.s, cx.c_col(style::START_DOT), Stroke::new(1.5 * cx.s, cx.c_col(style::START_DOT_OUTLINE)));
        }
    }
    if !l.circuit {
        if let Some(at) = cx.cam.project(last[0], last[1]).filter(|a| cx.visible(*a, size)) {
            chequer(cx, at, size);
        }
    }
}

impl LayerCtx<'_> {
    fn c_col(&self, c: Color32) -> Color32 {
        c.gamma_multiply(self.a)
    }
}

/// A 3×3 chequered flag square centred on `at`.
fn chequer(cx: &LayerCtx, at: Pos2, size: f32) {
    let h = size / 2.0;
    let cell = size / 3.0;
    cx.p.rect_filled(Rect::from_center_size(at, vec2(size + 2.0, size + 2.0)), 0.0, cx.c_col(Color32::BLACK));
    for i in 0..3 {
        for j in 0..3 {
            let col = if (i + j) % 2 == 1 { Color32::from_gray(0x11) } else { Color32::WHITE };
            let min = pos2(at.x - h + i as f32 * cell, at.y - h + j as f32 * cell);
            cx.p.rect_filled(Rect::from_min_size(min, vec2(cell, cell)), 0.0, cx.c_col(col));
        }
    }
}

/// POIs drawn at most this many per frame.
const POI_BUDGET: usize = 6000;

fn draw_pois(cx: &LayerCtx, layers: &MapLayers, cfg: &MapLayerConfig, st: &mut LayerStats) {
    let pc = &cfg.pois;
    let cam = cx.cam;
    let zoom_m = cam.zoom_m();
    if zoom_m > pc.max_zoom_m {
        return;
    }
    let mask = style::cat_mask(&pc.categories);
    if mask == 0 {
        return;
    }
    let size = pc.size_px * cx.s;
    let aabb = cam.world_aabb(size / cam.scale().max(1e-6));
    let pois = &layers.pois;
    let r2 = pc.radius_m * pc.radius_m;
    let mut vis: Vec<(u8, Pos2, f32)> = Vec::new();
    pois.grid.query(&aabb, |i| {
        let i = i as usize;
        let cat = pois.cat[i];
        if cat == NO_CAT || mask & (1u64 << cat) == 0 {
            return;
        }
        let it = &pois.items[i];
        if pc.near_only && (it.x - cx.car.0).powi(2) + (it.z - cx.car.1).powi(2) > r2 {
            return;
        }
        let [ox, oy] = cam.view.world_to_offset(it.x, it.z);
        let Some(at) = cam.project_offset(ox, oy) else { return };
        if vis.len() < POI_BUDGET && cx.visible(at, size) {
            vis.push((cat, at, cam.perspective_at(oy)));
        }
    });
    // Back to front: further (smaller y) first, so near icons overlap far ones.
    vis.sort_by(|a, b| a.1.y.total_cmp(&b.1.y));
    for &(cat, at, k) in &vis {
        draw_poi(cx, cat as usize, at, size * k);
    }
    st.pois = vis.len();
}

fn draw_poi(cx: &LayerCtx, cat: usize, at: Pos2, size: f32) {
    if let Some(atlas) = cx.icons {
        if let Some(uv) = atlas.uv(cat) {
            let mut m = Mesh::with_texture(atlas.texture);
            m.add_rect_with_uv(Rect::from_center_size(at, vec2(size, size)), uv, Color32::WHITE.gamma_multiply(cx.a));
            cx.p.add(Shape::mesh(m));
            return;
        }
    }
    let c = &style::POI_CATS[cat];
    let r = (size * 0.27).max(2.5);
    let fill = cx.c(c.color, 1.0);
    let edge = Stroke::new(1.0, cx.c_col(Color32::BLACK));
    match c.shape {
        Marker::Circle => {
            cx.p.circle(at, r, fill, edge);
        }
        Marker::Ring => {
            cx.p.circle(at, r, cx.c_col(Color32::from_black_alpha(110)), Stroke::new(2.0, fill));
        }
        Marker::Square => {
            let q = r * 0.9;
            cx.p.add(Shape::convex_polygon(vec![at + vec2(-q, -q), at + vec2(q, -q), at + vec2(q, q), at + vec2(-q, q)], fill, edge));
        }
        Marker::Diamond => {
            let q = r * 1.25;
            cx.p.add(Shape::convex_polygon(vec![at + vec2(0.0, -q), at + vec2(q, 0.0), at + vec2(0.0, q), at + vec2(-q, 0.0)], fill, edge));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maprender::data::{Chain, RoadLayer};
    use crate::maprender::racesel::RaceSel;
    use egui::epaint::ClippedShape;
    use egui::Vec2;
    use std::sync::Arc;

    /// Run `f` inside a throwaway egui pass and return the shapes it painted.
    fn paint(rect: Rect, f: impl FnOnce(&Painter)) -> Vec<ClippedShape> {
        let ctx = egui::Context::default();
        let mut f = Some(f);
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            let p = Painter::new(ctx.clone(), egui::LayerId::background(), rect);
            if let Some(f) = f.take() {
                f(&p);
            }
        });
        out.shapes
    }

    fn flat_cam(rect: Rect, car: (f32, f32), yaw: f32, zoom: f32) -> Camera {
        Camera::new(car.0, car.1, yaw, zoom, rect, rect.center(), 0.0, 1.0)
    }

    fn path_points(shapes: &[ClippedShape]) -> Vec<Vec<Pos2>> {
        shapes
            .iter()
            .filter_map(|s| match &s.shape {
                Shape::Path(p) => Some(p.points.clone()),
                Shape::LineSegment { points, .. } => Some(points.to_vec()),
                _ => None,
            })
            .collect()
    }

    fn layers_with(chain: Vec<[f32; 2]>, ty: RoadType) -> MapLayers {
        let mut roads = RoadLayer::default();
        let y = vec![0.0; chain.len()];
        roads.by_type[ty.index() as usize].push(Chain::new(chain, y));
        MapLayers { rev: 1, roads: Arc::new(roads), ..Default::default() }
    }

    fn only_roads() -> MapLayerConfig {
        let mut c = MapLayerConfig::default();
        c.pois.on = false;
        c.race_lines.mode = RaceLineMode::Off;
        c
    }

    fn ctx<'a>(p: &'a Painter, cam: &'a Camera) -> LayerCtx<'a> {
        LayerCtx { p, cam, s: 1.0, a: 1.0, car: (0.0, 0.0), corner_clip: None, icons: None, race_sel: &[] }
    }

    #[test]
    fn a_known_chain_vertex_lands_at_the_expected_pixel() {
        let rect = Rect::from_min_size(pos2(100.0, 50.0), vec2(400.0, 300.0));
        // Heading-up quarter turn: the car at (1000, 2000), zoom 500 m → 0.3 px/m.
        let cam = flat_cam(rect, (1000.0, 2000.0), std::f32::consts::FRAC_PI_2, 500.0);
        let layers = layers_with(vec![[1000.0, 2000.0], [1000.0, 2100.0], [1100.0, 2100.0]], RoadType::Road);
        let mut cfg = only_roads();
        cfg.roads.styles.road.casing = false;
        let mut stats = LayerStats::default();
        let shapes = paint(rect, |p| stats = draw_layers(&ctx(p, &cam), &layers, &cfg));
        let lines = path_points(&shapes);
        assert_eq!(lines.len(), 1, "{shapes:?}");
        // World north (+z) 100 m at yaw 90°: map turned clockwise a quarter, so +z points left
        // on screen: offset (-30, 0) from the centre; 100 m east then points up: (0, -30).
        let c = rect.center();
        assert!((lines[0][0] - c).length() < 1e-3);
        assert!((lines[0][1] - (c + vec2(-30.0, 0.0))).length() < 1e-3, "{:?}", lines[0]);
        assert!((lines[0][2] - (c + vec2(-30.0, -30.0))).length() < 1e-3, "{:?}", lines[0]);
        assert_eq!((stats.chains, stats.vertices), (1, 3));
    }

    #[test]
    fn chains_outside_the_view_are_culled_and_off_types_not_drawn() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 100.0);
        let far = layers_with(vec![[5000.0, 5000.0], [5100.0, 5000.0]], RoadType::Road);
        let cfg = only_roads();
        let mut st = LayerStats::default();
        let shapes = paint(rect, |p| st = draw_layers(&ctx(p, &cam), &far, &cfg));
        assert!(path_points(&shapes).is_empty());
        assert_eq!(st.chains, 0);
        // A visible turnaround is never drawn, whatever the config says.
        let t = layers_with(vec![[0.0, 0.0], [50.0, 0.0]], RoadType::Turnaround);
        let shapes = paint(rect, |p| st = draw_layers(&ctx(p, &cam), &t, &cfg));
        assert!(path_points(&shapes).is_empty());
        // A type switched off is skipped; switched on it is drawn.
        let r = layers_with(vec![[0.0, 0.0], [50.0, 0.0]], RoadType::Trail);
        let mut off = only_roads();
        off.roads.styles.trail.on = false;
        assert!(path_points(&paint(rect, |p| {
            draw_layers(&ctx(p, &cam), &r, &off);
        }))
        .is_empty());
        assert!(!path_points(&paint(rect, |p| {
            draw_layers(&ctx(p, &cam), &r, &cfg);
        }))
        .is_empty());
        // Whole roads layer off.
        let mut none = only_roads();
        none.roads.on = false;
        assert!(path_points(&paint(rect, |p| {
            draw_layers(&ctx(p, &cam), &r, &none);
        }))
        .is_empty());
    }

    #[test]
    fn casing_is_drawn_under_the_fill_and_wider() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 100.0);
        let layers = layers_with(vec![[0.0, 0.0], [50.0, 0.0]], RoadType::Road);
        let shapes = paint(rect, |p| {
            draw_layers(&ctx(p, &cam), &layers, &only_roads());
        });
        let widths: Vec<f32> = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                Shape::Path(p) => Some(p.stroke.width),
                Shape::LineSegment { stroke, .. } => Some(stroke.width),
                _ => None,
            })
            .collect();
        assert_eq!(widths.len(), 2);
        assert!(widths[0] > widths[1], "{widths:?}"); // casing first, wider
        assert!((widths[0] - widths[1] - 1.4).abs() < 1e-4); // casing_px
    }

    #[test]
    fn tilted_layers_project_through_the_camera() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(208.0, 136.0));
        let centre = Camera::tilt_centre(rect, 0.85);
        let cam = Camera::new(0.0, 0.0, 0.0, 300.0, rect, centre, 55f32.to_radians(), 200.0);
        let layers = layers_with(vec![[0.0, 0.0], [0.0, 100.0], [30.0, 150.0]], RoadType::Road);
        let mut cfg = only_roads();
        cfg.roads.styles.road.casing = false;
        let shapes = paint(rect, |p| {
            draw_layers(&ctx(p, &cam), &layers, &cfg);
        });
        let lines = path_points(&shapes);
        assert_eq!(lines.len(), 1);
        assert!((lines[0][0] - centre).length() < 1e-3);
        let want = cam.project(0.0, 100.0).unwrap();
        assert!(lines[0].iter().any(|q| (*q - want).length() < 1e-3), "{:?} vs {want:?}", lines[0]);
        assert!(want.y < centre.y); // north is up, towards the horizon
    }

    #[test]
    fn corner_clip_cuts_roads_to_the_pill_shape() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 100.0);
        // A road through the top-left corner, which the chamfered shape cuts off.
        let layers = layers_with(vec![[-99.0, 99.0], [-90.0, 90.0]], RoadType::Road);
        let poly = vec![pos2(60.0, 0.0), pos2(200.0, 0.0), pos2(200.0, 200.0), pos2(0.0, 200.0), pos2(0.0, 60.0)];
        let mut cfg = only_roads();
        cfg.roads.styles.road.casing = false;
        let cc = CornerClip { poly: &poly, safe: Rect::from_min_max(pos2(60.0, 60.0), pos2(140.0, 140.0)) };
        let shapes = paint(rect, |p| {
            let mut c = ctx(p, &cam);
            c.corner_clip = Some(cc);
            draw_layers(&c, &layers, &cfg);
        });
        assert!(path_points(&shapes).is_empty(), "{shapes:?}");
    }

    #[test]
    fn pois_respect_categories_zoom_limit_and_radius() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 300.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 1000.0);
        let layers = MapLayers::synthetic();
        let mut cfg = MapLayerConfig::default();
        cfg.roads.on = false;
        cfg.race_lines.mode = RaceLineMode::Off;
        let n = |cfg: &MapLayerConfig, cam: &Camera| {
            let mut st = LayerStats::default();
            paint(rect, |p| st = draw_layers(&ctx(p, cam), &layers, cfg));
            st.pois
        };
        // synthetic: house, fast travel, car meet, speed trap, barn find; all default-on.
        assert_eq!(n(&cfg, &cam), 5);
        cfg.pois.categories = vec!["house".into(), "not_a_category".into()];
        assert_eq!(n(&cfg, &cam), 1);
        cfg.pois = MapLayerConfig::default().pois;
        cfg.pois.on = false;
        assert_eq!(n(&cfg, &cam), 0);
        // Hidden above max_zoom_m (3000): the Dashboard default of 5000 m shows none.
        cfg.pois.on = true;
        let wide = flat_cam(rect, (0.0, 0.0), 0.0, 5000.0);
        assert_eq!(n(&cfg, &wide), 0);
        // near_only within 400 m of the car at the origin keeps house (316 m), fast travel (316 m)
        // and car meet (250 m); speed trap and barn find (515 m) go.
        cfg.pois.near_only = true;
        cfg.pois.radius_m = 400.0;
        assert_eq!(n(&cfg, &cam), 3);
    }

    #[test]
    fn icons_replace_the_markers_when_the_atlas_has_the_category() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 300.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 1000.0);
        let layers = MapLayers::synthetic();
        let mut cfg = MapLayerConfig::default();
        cfg.roads.on = false;
        cfg.race_lines.mode = RaceLineMode::Off;
        let mut atlas = IconAtlas::new(TextureId::Managed(7));
        atlas.set("house", Rect::from_min_max(pos2(0.0, 0.0), pos2(0.5, 0.5)));
        atlas.set("nonsense", Rect::from_min_max(pos2(0.0, 0.0), pos2(0.5, 0.5)));
        let shapes = paint(rect, |p| {
            let mut c = ctx(p, &cam);
            c.icons = Some(&atlas);
            draw_layers(&c, &layers, &cfg);
        });
        let meshes: Vec<_> = shapes.iter().filter_map(|s| if let Shape::Mesh(m) = &s.shape { Some(m) } else { None }).collect();
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].texture_id, TextureId::Managed(7));
        assert_eq!(meshes[0].vertices.len(), 4);
        // The other four POIs are still markers.
        assert_eq!(shapes.len(), 5);
    }

    #[test]
    fn race_lines_follow_the_selection_and_all_mode() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 600.0);
        let layers = MapLayers::synthetic();
        let mut cfg = MapLayerConfig::default();
        cfg.roads.on = false;
        cfg.pois.on = false;
        let run = |cfg: &MapLayerConfig, sel: &[usize]| {
            let mut st = LayerStats::default();
            let shapes = paint(rect, |p| {
                let mut c = ctx(p, &cam);
                c.race_sel = sel;
                st = draw_layers(&c, &layers, cfg);
            });
            (st, shapes.len())
        };
        // Current with nothing selected (not in a race): nothing.
        assert_eq!(run(&cfg, &[]).0.race_lines, 0);
        // Selected: the line plus the circuit's start mark (chequer = 1 backing + 9 cells).
        let (st, n) = run(&cfg, &[0]);
        assert_eq!(st.race_lines, 1);
        assert_eq!(n, 1 + 1 + 9);
        cfg.race_lines.marks = false;
        assert_eq!(run(&cfg, &[0]).1, 1);
        cfg.race_lines.mode = RaceLineMode::All;
        assert_eq!(run(&cfg, &[]).0.race_lines, 1);
        cfg.race_lines.mode = RaceLineMode::Off;
        assert_eq!(run(&cfg, &[0]).0.race_lines, 0);
        // The selector feeds the painter: put the car on the ring and let `Current` pick it.
        let mut sel = RaceSel::default();
        let rc = RaceCfg::default();
        let picked = sel.update(&layers.races, &rc, (400.0, 0.0), 0.0, true).to_vec();
        assert_eq!(picked, vec![0]);
    }

    /// Frame cost on the real data: `cargo test --release bench_ -- --ignored --nocapture`.
    /// Prints the time to build the shapes (`draw_layers`) and to tessellate them (what egui does
    /// afterwards), as the median of 40 frames.
    #[test]
    #[ignore]
    fn bench_draw_layers_on_the_real_install() {
        use crate::gamedata::roadtypes::RoadTypes;
        let Some(media) = crate::gamedata::install::find_media(None) else { return };
        let g = crate::maprender::data::GameData::load(&media).expect("game data");
        let cur = RoadTypes::current_with(RoadTypes::project(), &RoadTypes::project_sha1(), std::path::Path::new("/nonexistent"), &g.nav);
        let layers = g.layers(&cur, 1);
        let med = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        // (label, rect size, zoom m, tilt, car) — the car on a busy part of the island.
        let car = (1500.0, 800.0);
        let cases: [(&str, Vec2, f32, bool); 6] = [
            ("Dashboard 900x600 @ 5000 m", vec2(900.0, 600.0), 5000.0, false),
            ("Dashboard 900x600 @ 6000 m (whole island)", vec2(900.0, 600.0), 6000.0, false),
            ("Dashboard 600x400 @ 1500 m", vec2(600.0, 400.0), 1500.0, false),
            ("Dashboard 420x420 @ 5000 m", vec2(420.0, 420.0), 5000.0, false),
            ("HUD 208x136 @ 300 m tilted", vec2(208.0, 136.0), 300.0, true),
            ("Dashboard 900x600 @ 5000 m tilted", vec2(900.0, 600.0), 5000.0, true),
        ];
        for (label, size, zoom, tilted) in cases {
            let rect = Rect::from_min_size(Pos2::ZERO, size);
            let cam = if tilted {
                Camera::new(car.0, car.1, 0.6, zoom, rect, Camera::tilt_centre(rect, 0.85), 55f32.to_radians(), 200.0 * size.y / 136.0)
            } else {
                flat_cam(rect, car, 0.6, zoom)
            };
            let cfg = MapLayerConfig::default();
            let (mut t_build, mut t_tess) = (Vec::new(), Vec::new());
            let mut st = LayerStats::default();
            for _ in 0..40 {
                let ectx = egui::Context::default();
                let out = ectx.run(egui::RawInput::default(), |c| {
                    let p = Painter::new(c.clone(), egui::LayerId::background(), rect);
                    let t0 = std::time::Instant::now();
                    st = draw_layers(&ctx(&p, &cam), &layers, &cfg);
                    t_build.push(t0.elapsed().as_secs_f64() * 1e3);
                });
                let t1 = std::time::Instant::now();
                let prims = ectx.tessellate(out.shapes, 1.0);
                t_tess.push(t1.elapsed().as_secs_f64() * 1e3);
                std::hint::black_box(prims);
            }
            eprintln!("{label}: build {:.2} ms + tessellate {:.2} ms ({st:?})", med(t_build), med(t_tess));
        }
    }

    /// Mesh shape of the base image, without a GPU: the vertices' UVs must be the calibration of
    /// the world point under each vertex.
    #[test]
    fn base_image_uvs_match_the_calibration_flat_and_tilted() {
        let rect = Rect::from_min_size(pos2(20.0, 10.0), vec2(300.0, 200.0));
        let outline = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
        let cal = MapCalibration::DEFAULT;
        let tex = MapTex { id: TextureId::Managed(3), orig_size: [8192, 8192], winter: false };
        for pitch in [0.0f32, 55f32.to_radians()] {
            let centre = if pitch == 0.0 { rect.center() } else { Camera::tilt_centre(rect, 0.85) };
            let cam = Camera::new(-409.0, -6541.0, 0.4, 800.0, rect, centre, pitch, 400.0);
            let shapes = paint(rect, |p| {
                draw_base(p, &BaseParams { cam: &cam, cal, tex, outline: &outline, mirror: true, look: ImageLook::FULL, a: 1.0 });
            });
            let meshes: Vec<_> = shapes.iter().filter_map(|s| if let Shape::Mesh(m) = &s.shape { Some(m.clone()) } else { None }).collect();
            assert_eq!(meshes.len(), 1, "pitch {pitch}");
            let m = &meshes[0];
            assert!(m.vertices.len() >= 5);
            assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
            for v in &m.vertices {
                assert!(rect.expand(0.01).contains(v.pos), "vertex outside the outline: {:?}", v.pos);
                let [wx, wz] = cam.unproject(v.pos).expect("vertex below the horizon");
                let uv = cal.world_to_uv(wx, wz, tex.orig_size);
                assert!((v.uv.x - uv[0]).abs() < 1e-5 && (v.uv.y - uv[1]).abs() < 1e-5);
            }
            if pitch != 0.0 {
                // The subdivided mesh covers the screen rect below the horizon: area close to the rect's.
                let area: f32 = m.indices.chunks(3).map(|t| {
                    let (a, b, c) = (m.vertices[t[0] as usize].pos, m.vertices[t[1] as usize].pos, m.vertices[t[2] as usize].pos);
                    ((b - a).x * (c - a).y - (b - a).y * (c - a).x).abs() / 2.0
                }).sum();
                assert!(area > 0.6 * rect.area() && area <= rect.area() * 1.001, "area {area} of {}", rect.area());
            }
        }
    }

    #[test]
    fn base_image_not_mirrored_is_cut_to_the_image_and_look_applies() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 400.0));
        let outline = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
        let cal = MapCalibration::DEFAULT;
        let tex = MapTex { id: TextureId::Managed(3), orig_size: [8192, 8192], winter: false };
        // Car at the map's top-left corner (world origin of the calibration): only the part of the
        // screen right/below the corner is image.
        let cam = Camera::new(cal.origin_x, cal.origin_z, 0.0, 200.0, rect, rect.center(), 0.0, 1.0);
        let look = ImageLook { opacity: 0.5, brightness: 0.5, saturation: 0.5 };
        let shapes = paint(rect, |p| {
            draw_base(p, &BaseParams { cam: &cam, cal, tex, outline: &outline, mirror: false, look, a: 1.0 });
        });
        let metas: Vec<_> = shapes.iter().filter_map(|s| if let Shape::Mesh(m) = &s.shape { Some(m.clone()) } else { None }).collect();
        assert_eq!(metas.len(), 2, "image + saturation veil");
        let img = &metas[0];
        let (mn, mx) = img.vertices.iter().fold((Pos2::new(1e9, 1e9), Pos2::new(-1e9, -1e9)), |(a, b), v| (pos2(a.x.min(v.pos.x), a.y.min(v.pos.y)), pos2(b.x.max(v.pos.x), b.y.max(v.pos.y))));
        assert!((mn.x - 200.0).abs() < 0.01 && (mn.y - 200.0).abs() < 0.01, "{mn:?}");
        assert!((mx.x - 400.0).abs() < 0.01 && (mx.y - 400.0).abs() < 0.01, "{mx:?}");
        // Opacity 0.5, brightness 0.5: premultiplied (64, 64, 64, 128).
        assert_eq!(img.vertices[0].color, Color32::from_rgba_premultiplied(64, 64, 64, 128));
        // Full look: no veil.
        let full = paint(rect, |p| {
            draw_base(p, &BaseParams { cam: &cam, cal, tex, outline: &outline, mirror: false, look: ImageLook::FULL, a: 1.0 });
        });
        assert_eq!(full.iter().filter(|s| matches!(s.shape, Shape::Mesh(_))).count(), 1);
    }
}
