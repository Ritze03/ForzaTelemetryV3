//! The 2D renderer, egui `Painter` output: the base image ([`draw_base`]) and the vector layers
//! ([`draw_layers`]: roads, jump lines, race lines, POIs). Both maps call these two functions
//! with different parameters (D61).
//!
//! Approach (measured in the design scout, `docs/features/minimap.md`): one `Shape::line` per
//! visible road chain, chains culled by their bounding box against the view's world box,
//! vertices thinned to 2 px on screen. No baking into the map texture and no pre-tessellated
//! world meshes: road types change on every editor Save, types are toggled per map, and widths
//! are in screen px and change with the eased zoom. Joins (D81): every type's casing first, then
//! every fill; chain ends closed after `RoadLayer::joins` (mitred into the next piece at a
//! degree-2 node, a round cap at a dead end or junction); overpasses drawn again with their
//! casing over the road below.

use std::collections::HashMap;

use egui::epaint::Vertex;
use egui::{pos2, vec2, Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke, TextureId, Vec2};

use super::cfg::{DashStyle, ImageCfg, MapLayerConfig, OtherRoads, RaceLineMode, Rgb, RouteStyle};
use super::data::{End, Joins, MapLayers, NO_CAT};
use super::racesel::{Poly, RaceSel, RoadFocus};
use super::style::{self, Shape as Marker};
use super::view::{bbox_hits, clip_convex, clip_polyline_convex, clip_segment_convex, fan, inside_convex, thin, Camera, FAR_MIN_SCALE};
use super::MapTex;
use crate::gamedata::icons::{PoiIcons, RaceClass};
use crate::gamedata::poi::{week_index_now, Poi, PoiKind};
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
    /// Tilted only: the image fades out towards the far edge of the plane, just short of the
    /// horizon (over [`style::FAR_FADE_DEPTH`] of depth scale), into whatever is behind it: the Dashboard's
    /// background, the HUD's plate (or the game, with no plate). Without it the plane ends in a
    /// hard line. The demo fades the same way, with a gradient of the plate colour over the image.
    pub far_fade: bool,
}

/// Cells per side of the tilted map's mesh: egui interpolates UVs affinely inside a triangle,
/// the perspective is not affine, so the plane is subdivided until the error is invisible.
pub const TILT_CELLS: usize = 24;

/// The map image under the camera. Flat: a triangle fan over `outline` (cut to the image when
/// not mirroring) with per-vertex UVs from the inverse mapping, which is exact for an affine
/// map. Tilted: a screen-space grid of `TILT_CELLS`² cells over `outline` from the plane's far
/// limit down (each cut to `outline`, and to the image on the plane when not mirroring), with
/// each vertex's UV taken from the inverse projection.
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

    // The far-edge fade: alpha 0 at the plane's far limit (`FAR_MIN_SCALE`, just short of the
    // horizon), 1 at `FAR_FADE_DEPTH` more depth scale; linear in the screen row.
    let fade = m.far_fade && !cam.is_flat();
    let ramp = |y: f32| if fade { ((cam.depth_scale_at_row(y) - FAR_MIN_SCALE) / style::FAR_FADE_DEPTH).clamp(0.0, 1.0) } else { 1.0 };

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
        fan(&mut img, hub, shape, |pt| Vertex { pos: pt, uv: uv_of(pt), color: col.gamma_multiply(ramp(pt.y)) });
        if veil_a > 0 {
            fan(&mut veil, hub, shape, |pt| Vertex { pos: pt, uv: egui::epaint::WHITE_UV, color: veil_col.gamma_multiply(ramp(pt.y)) });
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
        // A screen-space grid over the outline, from the far limit (or the outline's top) down:
        // the whole view is covered by construction. Rows are spaced evenly in the log of the
        // depth scale, so each row spans the same depth ratio and the affine UV error stays
        // even; each vertex's UV comes from the exact inverse projection.
        let bounds = outline_bounds(m.outline);
        let top = bounds.top().max(cam.far_row());
        let bottom = bounds.bottom();
        let n = TILT_CELLS;
        if bottom > top {
            let k0 = cam.depth_scale_at_row(top).max(FAR_MIN_SCALE).ln();
            let k1 = (1.0 + (bottom - cam.centre.y) * cam.pitch.tan() / cam.focal).max(FAR_MIN_SCALE).ln();
            let mut rows: Vec<f32> = (0..=n).map(|j| cam.row_of_depth_scale((k0 + (k1 - k0) * j as f32 / n as f32).exp())).collect();
            rows[0] = top;
            rows[n] = bottom;
            let dx = bounds.width() / n as f32;
            let rect_outline = m.outline.len() == 4;
            for w in rows.windows(2) {
                let (y0, y1) = (w[0], w[1]);
                if y1 <= y0 {
                    continue;
                }
                for i in 0..n {
                    let x0 = bounds.left() + dx * i as f32;
                    let cell = Rect::from_min_max(pos2(x0, y0), pos2(x0 + dx, y1));
                    let quad = [cell.left_top(), cell.right_top(), cell.right_bottom(), cell.left_bottom()];
                    let shape = if rect_outline && bounds.contains_rect(cell) { quad.to_vec() } else { clip_convex(&quad, m.outline) };
                    if m.mirror {
                        add(&shape);
                        continue;
                    }
                    // Cut to the image on the plane (a projection keeps lines straight).
                    let Some(plane) = shape.iter().map(|&q| cam.unproject_offset(q).map(|o| pos2(o[0], o[1]))).collect::<Option<Vec<Pos2>>>() else { continue };
                    let cut = clip_convex(&plane, &corners);
                    if let Some(screen) = cut.iter().map(|q| cam.project_offset(q.x, q.y)).collect::<Option<Vec<Pos2>>>() {
                        add(&screen);
                    }
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

/// POI icons of one egui context: the texture plus a UV rect per category (the order of
/// `style::POI_CATS`), per race class (a `RacePin` takes its route's class) and per mascot
/// region. A category without a rect is drawn as a shape marker (D64).
#[derive(Clone, Debug)]
pub struct IconAtlas {
    pub texture: TextureId,
    pub rects: Vec<Option<Rect>>,
    pub race: HashMap<RaceClass, Rect>,
    pub mascot: HashMap<u32, Rect>,
}

fn uv_rect(uv: &[f32; 4]) -> Rect {
    Rect::from_min_max(pos2(uv[0], uv[1]), pos2(uv[2], uv[3]))
}

impl IconAtlas {
    pub fn new(texture: TextureId) -> IconAtlas {
        IconAtlas { texture, rects: vec![None; style::POI_CATS.len()], race: HashMap::new(), mascot: HashMap::new() }
    }

    /// The atlas of `icons` once uploaded as `texture`: every category shows its kind's icon
    /// (`PoiCat::icon_kind`: the current treasure chest the chest's, danger signs their own).
    pub fn from_poi_icons(texture: TextureId, icons: &PoiIcons) -> IconAtlas {
        let mut a = IconAtlas::new(texture);
        for (i, c) in style::POI_CATS.iter().enumerate() {
            a.rects[i] = c.icon_kind().and_then(|k| icons.uv.get(&k)).map(uv_rect);
        }
        a.race = icons.race.iter().map(|(c, uv)| (*c, uv_rect(uv))).collect();
        a.mascot = icons.mascot.iter().map(|(r, uv)| (*r, uv_rect(uv))).collect();
        a
    }

    /// Set the UV rect of category `id` (a `style::POI_CATS` id); unknown ids are ignored.
    #[cfg(test)]
    pub fn set(&mut self, id: &str, uv: Rect) {
        if let Some(i) = style::cat_index(id) {
            self.rects[i] = Some(uv);
        }
    }

    fn uv(&self, cat: usize) -> Option<Rect> {
        self.rects.get(cat).copied().flatten()
    }

    /// The icon of one POI: race pins by their route's class, mascots by region, the rest by
    /// category.
    fn uv_of(&self, cat: usize, item: &Poi, layers: &MapLayers) -> Option<Rect> {
        match item.kind {
            PoiKind::RacePin => layers.race_class.get(&item.n).and_then(|c| self.race.get(c)).copied().or_else(|| self.uv(cat)),
            PoiKind::Mascot => self.mascot.get(&item.n).copied().or_else(|| self.uv(cat)),
            _ => self.uv(cat),
        }
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
    /// The race selector: its picks are the lines drawn for Nearest / Near / Current (`All`
    /// ignores them) and the source of the in-race focus (D66, `cfg.race_lines.focus`).
    pub race_sel: &'a RaceSel,
    /// The game week for the current treasure chest (`poi::week_index_at`); `None` = now.
    pub week: Option<i64>,
}

/// What a [`draw_layers`] call drew (tests, perf numbers).
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerStats {
    pub chains: usize,
    pub vertices: usize,
    pub pois: usize,
    pub race_lines: usize,
    /// Gate lines drawn (speed zones, trailblazers, drift zones, speed traps).
    pub gates: usize,
    /// Road chains / runs / jump lines drawn muted (in-race focus), already counted in `chains`.
    pub muted: usize,
}

/// Which parts of [`draw_layers_parts`] to draw. In the 3D view (phase K) the roads, jump lines
/// and, since D88, the race lines with their start / finish marks are the GL scene's
/// ([`super::gl3d`], depth-tested: terrain and overpasses hide them); the POIs are still drawn
/// here, with egui, **over** the 3D (`Camera::project` follows the terrain, `k_at` sizes the
/// icons), so the 3D call sites pass [`Parts::OVER_3D`]. *Why the POIs stay:* the same code,
/// icons, fonts, clip and per-category rules as the 2D maps; the cost is that nothing hides them
/// behind a ridge (v1, design 2.1E; limit K6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parts {
    pub roads: bool,
    pub race_lines: bool,
    pub pois: bool,
}

impl Parts {
    /// Everything (what [`draw_layers`] draws).
    pub const ALL: Parts = Parts { roads: true, race_lines: true, pois: true };
    /// What stays on egui when the GL scene draws the roads and the race lines.
    #[allow(dead_code)] // phase K: the 3D call sites (K3, K4)
    pub const OVER_3D: Parts = Parts { roads: false, race_lines: false, pois: true };
}

/// Vector layers over the base image: roads (bottom to top: `style::ROAD_DRAW_ORDER`), jump
/// lines, race lines with start / finish marks, POIs. Each part is skipped when its switch is off.
pub fn draw_layers(cx: &LayerCtx, layers: &MapLayers, cfg: &MapLayerConfig) -> LayerStats {
    draw_layers_parts(cx, layers, cfg, Parts::ALL)
}

/// [`draw_layers`] restricted to `parts` (a part is drawn when both its `parts` flag and its config
/// switch are on).
pub fn draw_layers_parts(cx: &LayerCtx, layers: &MapLayers, cfg: &MapLayerConfig, parts: Parts) -> LayerStats {
    let mut st = LayerStats::default();
    // In-race focus (D66): only with a selected race line (`RaceSel::focus_line`), never on a guess.
    let focusing = cx.race_sel.focus_line().is_some_and(|l| l < layers.races.lines.len());
    let rc = &cfg.race_lines;
    let other = rc.focus.other_roads.effective(rc.route);
    // D82: "race road only" draws no road of the road layer while the focus is on.
    if parts.roads && cfg.roads.on && !(focusing && other == OtherRoads::RaceOnly) {
        let focus = (focusing && other != OtherRoads::Normal).then(|| cx.race_sel.road_focus(layers)).flatten();
        draw_roads(cx, layers, cfg, other, focus.as_deref(), &mut st);
    }
    if parts.race_lines && rc.mode != RaceLineMode::Off {
        draw_race_lines(cx, layers, cfg, &mut st);
    }
    if parts.pois && cfg.pois.on && !(focusing && cfg.race_lines.focus.hide_pois) {
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

    /// A road chain (or a run of one) projected like [`Self::polyline`], with its ends closed
    /// (D81): at [`EndPx::Ext`] (a join; the next piece's neighbouring vertex) the line runs on
    /// 1 px (at most `ext_px`) along the corner's bisector and the mitre's outer corner is noted
    /// in `caps` as a [`Deco::Wedge`]; at [`EndPx::Cap`] a round cap is noted there. The
    /// [`Deco`]s are drawn per pass.
    fn road_line(&self, pts: &[[f32; 2]], ends: [EndPx; 2], ext_px: f32, out: &mut Vec<Vec<Pos2>>, caps: &mut Vec<Deco>, scratch: &mut Vec<Pos2>) {
        scratch.clear();
        let n = pts.len();
        let mut first = 0usize;
        for i in 0..=n {
            match pts.get(i).and_then(|q| self.cam.project(q[0], q[1])) {
                Some(s) => {
                    if scratch.is_empty() {
                        first = i;
                    }
                    scratch.push(s);
                }
                None => {
                    if scratch.len() >= 2 {
                        let e = [if first == 0 { ends[0] } else { EndPx::Butt }, if i == n { ends[1] } else { EndPx::Butt }];
                        self.road_piece(scratch, e, ext_px, out, caps);
                    }
                    scratch.clear();
                }
            }
        }
    }

    fn road_piece(&self, scratch: &mut Vec<Pos2>, ends: [EndPx; 2], ext_px: f32, out: &mut Vec<Vec<Pos2>>, caps: &mut Vec<Deco>) {
        thin(scratch, style::THIN_PX);
        let m = scratch.len();
        if m < 2 {
            return;
        }
        let mut ext = [None, None];
        for (k, e) in ends.into_iter().enumerate() {
            let (p, q) = if k == 0 { (scratch[0], scratch[1]) } else { (scratch[m - 1], scratch[m - 2]) };
            match e {
                EndPx::Ext(nw) => {
                    // A last, 1 px short segment along the corner's bisector direction: egui
                    // mitres the corner onto it and ends the line square to it, i.e. along the
                    // bisector line, where the next piece ends the same way. The 1 px overlap
                    // hides the anti-aliasing seam (the later-drawn line covers it).
                    if let Some(pn) = self.cam.project(nw[0], nw[1]) {
                        let (din, dout) = ((p - q).normalized(), (pn - p).normalized());
                        let t = (din + dout).normalized();
                        let t = if t.is_finite() && (pn - p).length() > 1e-3 { t } else { din };
                        if t.is_finite() {
                            ext[k] = Some(p + t * ext_px.min(1.0));
                        }
                        // egui bevels the corner of that short segment; the rest of the mitre's
                        // outer corner, up to its tip on the bisector, is a wedge of its own.
                        if let Some(w) = mitre_wedge(din, dout) {
                            caps.push(Deco::Wedge(p, w.0, w.1));
                        }
                    }
                }
                EndPx::Cap => {
                    let d = p - q;
                    if d.length() > 1e-3 {
                        caps.push(Deco::Cap(p, d.normalized()));
                    }
                }
                EndPx::Butt => {}
            }
        }
        if let Some(x) = ext[0] {
            scratch.insert(0, x);
        }
        if let Some(x) = ext[1] {
            scratch.push(x);
        }
        match &self.corner_clip {
            Some(cc) if scratch.iter().any(|q| !cc.safe.contains(*q)) => out.extend(clip_polyline_convex(scratch, cc.poly)),
            _ => out.push(scratch.clone()),
        }
    }

    /// A [`Deco`] for a line of half-width `r`.
    fn deco(&self, d: Deco, r: f32, col: Color32) {
        if r < DECO_MIN_R {
            return;
        }
        match d {
            Deco::Cap(at, dir) => self.round_cap(at, dir, r, col),
            Deco::Wedge(at, n, tip) => {
                if self.fits(at, r * tip.length()) {
                    self.p.add(Shape::convex_polygon(vec![at, at + n * r, at + tip * r], col, Stroke::NONE));
                }
            }
        }
    }

    /// A round cap (D81): the half disc of radius `r` beyond a line end at `at` (outward `dir`),
    /// reaching 1 px back into the line so no anti-aliasing seam shows between the two. Skipped
    /// when it would poke out of the map shape.
    fn round_cap(&self, at: Pos2, dir: Vec2, r: f32, col: Color32) {
        if !self.fits(at, r) {
            return;
        }
        let side = dir.rot90();
        let back = dir * -(1.0f32).min(r);
        // Fewer arc segments on small caps (a 2 px cap is a half hexagon).
        let n = (r.ceil() as usize).clamp(3, CAP_SEGS_2D);
        let mut pts = Vec::with_capacity(n + 3);
        pts.push(at + side * r + back);
        for j in 0..=n {
            let a = std::f32::consts::PI * j as f32 / n as f32;
            pts.push(at + side * (r * a.cos()) + dir * (r * a.sin()));
        }
        pts.push(at - side * r + back);
        self.p.add(Shape::convex_polygon(pts, col, Stroke::NONE));
    }

    /// The taper factor [`Self::tapered`] gives a line at screen row `y`.
    fn taper_k(&self, y: f32, taper: bool) -> f32 {
        let cam = self.cam;
        if !taper || cam.is_flat() || cam.relief.is_some() {
            return 1.0;
        }
        let (k0, k1) = (cam.depth_scale_at_row(cam.rect.top()), cam.depth_scale_at_row(cam.rect.bottom()));
        let n = style::TAPER_BANDS as f32;
        let b = (((cam.depth_scale_at_row(y) - k0) / (k1 - k0).max(1e-3)) * n).clamp(0.0, n - 1.0) as usize;
        k0 + (b as f32 + 0.5) / n * (k1 - k0)
    }

    /// The tilt's width taper: `lines` cut into pieces of similar depth, each with the factor
    /// its width gets (the perspective at its screen row, in [`style::TAPER_BANDS`] steps).
    /// Flat view or `taper` off: every line whole with factor 1.
    fn tapered(&self, lines: Vec<Vec<Pos2>>, taper: bool) -> Vec<(f32, Vec<Pos2>)> {
        let cam = self.cam;
        // In 3D the row-based depth scale (a flat-plane formula) is wrong over hills: the egui
        // lines (race lines) keep a constant width there; the GL roads taper per vertex.
        if !taper || cam.is_flat() || cam.relief.is_some() {
            return lines.into_iter().map(|l| (1.0, l)).collect();
        }
        let (k0, k1) = (cam.depth_scale_at_row(cam.rect.top()), cam.depth_scale_at_row(cam.rect.bottom()));
        let n = style::TAPER_BANDS as f32;
        let band = |y: f32| (((cam.depth_scale_at_row(y) - k0) / (k1 - k0).max(1e-3)) * n).clamp(0.0, n - 1.0) as usize;
        let k_of = |b: usize| k0 + (b as f32 + 0.5) / n * (k1 - k0);
        let mut out = Vec::with_capacity(lines.len());
        for l in lines {
            let Some(&first) = l.first() else { continue };
            let (mut cur, mut cb) = (vec![first], usize::MAX);
            for w in l.windows(2) {
                let b = band((w[0].y + w[1].y) * 0.5);
                if cb != usize::MAX && b != cb {
                    out.push((k_of(cb), std::mem::replace(&mut cur, vec![w[0]])));
                }
                cb = b;
                cur.push(w[1]);
            }
            if cur.len() >= 2 {
                out.push((k_of(cb), cur));
            }
        }
        out
    }

    /// Does a marker of half-size `r` centred on `at` lie wholly inside the map shape? Always
    /// without a corner clip (the painter's clip rect cuts the rest); with one, a marker near a
    /// rounded corner must keep its four extremes inside the outline, else it is skipped (a half
    /// marker poking into the transparent surround is worse than none).
    fn fits(&self, at: Pos2, r: f32) -> bool {
        match &self.corner_clip {
            Some(cc) if !cc.safe.contains_rect(Rect::from_center_size(at, vec2(2.0 * r, 2.0 * r))) => {
                [vec2(-r, -r), vec2(r, -r), vec2(r, r), vec2(-r, r)].iter().all(|d| inside_convex(at + *d, cc.poly))
            }
            _ => true,
        }
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

/// How [`road_pass`] closes a road line's end (from the chain's [`End`]).
#[derive(Clone, Copy, Debug, PartialEq)]
enum EndPx {
    /// Plain (egui's butt end): a run's end inside its chain, and every muted end.
    Butt,
    /// Run on towards this world point (the next piece at an [`End::Join`]).
    Ext([f32; 2]),
    /// A round cap (a dead end, a junction, a join onto a type that is switched off).
    Cap,
}

/// Arc segments of a 2D round cap (at most).
const CAP_SEGS_2D: usize = 8;

/// Caps, mitre wedges and overpass outlines of a line narrower than twice this (px) are not
/// drawn: a cap of a 2 px line is not seen, and the island zoomed out has ~4 000 of them, which
/// cost more than all the lines (D81, measured: 5 km Dashboard 3.5 ms -> 2 ms).
const DECO_MIN_R: f32 = 1.5;

/// What a road line's end adds besides the line (D81), drawn in the casing and the fill pass
/// with that pass's half-width.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Deco {
    /// A round cap: the end point, the outward direction.
    Cap(Pos2, Vec2),
    /// The outer corner of a mitred join: the node, the line's outer normal there and the offset
    /// of the mitre's tip, both per unit of half-width. The triangle node, node + normal, tip is
    /// this line's half of the corner; the next piece draws the other half, so the two meet on
    /// the bisector.
    Wedge(Pos2, Vec2, Vec2),
}

/// The outer normal and the mitre tip (per unit of half-width) where a line arriving along `din`
/// turns into `dout` (unit vectors); `None` when it runs on straight (nothing to fill) or turns
/// so sharply that the tip would be more than [`super::mesh3d::MITER_MAX`] half-widths out
/// (left bevelled, as egui does).
fn mitre_wedge(din: Vec2, dout: Vec2) -> Option<(Vec2, Vec2)> {
    if !(din.is_finite() && dout.is_finite()) || din.dot(dout) > 0.9998 {
        return None;
    }
    let n_in = if din.rot90().dot(dout) <= 0.0 { din.rot90() } else { -din.rot90() };
    let n_out = if dout.rot90().dot(-din) <= 0.0 { dout.rot90() } else { -dout.rot90() };
    let m = (n_in + n_out).normalized();
    let f = 1.0 / m.dot(n_in).max(1e-3);
    (m.is_finite() && f <= super::mesh3d::MITER_MAX).then_some((n_in, m * f))
}

/// Which roads one [`road_pass`] draws, and how.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// Everything in its own style (no in-race focus).
    All,
    /// Only the roads along the race line, in their own style.
    Relevant,
    /// Only the other roads, in the muted look (`cfg.race_lines.focus`).
    Muted,
}

/// Roads, and with the in-race `focus` (D66) the other roads muted underneath, or not at all.
fn draw_roads(cx: &LayerCtx, layers: &MapLayers, cfg: &MapLayerConfig, other: OtherRoads, focus: Option<&RoadFocus>, st: &mut LayerStats) {
    match (focus, other) {
        (Some(f), OtherRoads::Muted) => {
            road_pass(cx, layers, cfg, Some(f), Pass::Muted, st);
            road_pass(cx, layers, cfg, Some(f), Pass::Relevant, st);
        }
        (Some(f), OtherRoads::Hidden) => road_pass(cx, layers, cfg, Some(f), Pass::Relevant, st),
        _ => road_pass(cx, layers, cfg, None, Pass::All, st),
    }
}

/// Overpasses (D81, [`super::data::Overpass`]): the upper road's stretch over the crossing once more, its
/// casing over the lower road and its fill over that, 1.5 px past the casing's ends (so they do
/// not show across the road). The stretches of one road that overlap (a ramp crossing two lanes)
/// are merged first, then drawn lowest first. Only for an upper type with a casing and a solid,
/// opaque fill (a translucent or dashed stretch would not match the line under it), both types
/// drawn in this pass, crossing at more than ~15 degrees (two roads stacked almost in parallel
/// would be cut into pieces); with the in-race focus only an upper segment of the pass's own
/// relevance.
fn draw_overpasses(cx: &LayerCtx, layers: &MapLayers, joins: &Joins, focus: Option<(&RoadFocus, bool)>, slots: &[SlotLines], taper: bool, dashes: bool) {
    let cam = cx.cam;
    let find = |slot: usize| slots.iter().find(|s| s.slot == slot);
    let rect = cam.rect.expand(40.0);
    // (slot, chain) -> [(s0, s1, y, k)] stretches in metres along the chain.
    let mut stretches: Vec<((u8, u32), [f32; 2], f32, f32)> = Vec::new();
    // Where a road lies over another almost in parallel (a flat crossing, drawn by type rank as
    // everywhere else): no stretch of that road may be drawn over there, it would show as a patch.
    let mut blocked: Vec<((u8, u32), [f32; 2])> = Vec::new();
    for ov in &joins.overpasses {
        let (Some(up), Some(low)) = (find(ov.slot as usize), find(ov.lower_slot as usize)) else { continue };
        let Some((_, extra)) = up.casing else { continue };
        if !up.fill_caps || (up.dash != DashStyle::None && dashes) {
            continue;
        }
        let Some(at) = cam.project(ov.at[0], ov.at[1]).filter(|p| rect.contains(*p)) else { continue };
        if let Some((f, relevant)) = focus {
            if seg_relevant(f, ov.slot as usize, ov.chain, ov.seg) != Some(relevant) {
                continue;
            }
        }
        let Some(ch) = layers.roads.by_type[ov.slot as usize].get(ov.chain as usize) else { continue };
        let k = cx.taper_k(at.y, taper);
        let wk = |w: f32| if k == 1.0 { w } else { (w * k).max(style::MIN_LINE_PX) };
        let (wu, wl, c) = (wk(up.w), wk(low.w), extra * k);
        if 0.5 * (wu + c) < DECO_MIN_R {
            continue; // an outline of a hair-thin line is not seen
        }
        let cos = (1.0 - ov.sin * ov.sin).max(0.0).sqrt();
        let len_px = (0.5 * (wl + low.casing.map_or(0.0, |cc| cc.1 * k)) + 0.5 * (wu + c) * cos) / ov.sin.max(0.05) + 1.5;
        let len = len_px / (cam.scale() * cam.k_at(ov.at[0], ov.at[1]).max(1e-3));
        let s = arc_at(&ch.pts, ov.seg as usize, ov.at);
        if ov.sin < OVERPASS_MIN_SIN {
            blocked.push(((ov.slot, ov.chain), [s - len, s + len]));
        } else {
            stretches.push(((ov.slot, ov.chain), [s - len, s + len], ov.y, k));
        }
    }
    if stretches.is_empty() {
        return;
    }
    // Merge the overlapping stretches of each road (the y of the highest crossing in them).
    stretches.sort_by(|a, b| a.0.cmp(&b.0).then(a.1[0].total_cmp(&b.1[0])));
    let mut merged: Vec<((u8, u32), [f32; 2], f32, f32)> = Vec::with_capacity(stretches.len());
    for st in stretches {
        match merged.last_mut() {
            Some(m) if m.0 == st.0 && st.1[0] <= m.1[1] => {
                m.1[1] = m.1[1].max(st.1[1]);
                m.2 = m.2.max(st.2);
                m.3 = m.3.min(st.3);
            }
            _ => merged.push(st),
        }
    }
    merged.retain(|m| !blocked.iter().any(|b| b.0 == m.0 && b.1[0] < m.1[1] && m.1[0] < b.1[1]));
    merged.sort_by(|a, b| a.2.total_cmp(&b.2));
    let (mut scratch, mut caps) = (Vec::new(), Vec::new());
    for ((slot, chain), [s0, s1], _, k) in merged {
        let (Some(up), Some(ch)) = (find(slot as usize), layers.roads.by_type[slot as usize].get(chain as usize)) else { continue };
        let Some((casing, extra)) = up.casing else { continue };
        let wu = if k == 1.0 { up.w } else { (up.w * k).max(style::MIN_LINE_PX) };
        // 1.5 px more fill than casing at each end, in metres at the stretch; at the chain's own
        // ends (the fill cannot go on) the casing stops short instead.
        let mid = piece_between(&ch.pts, (s0 + s1) * 0.5, (s0 + s1) * 0.5);
        let grow = mid.first().map_or(0.0, |q| 1.5 / (cam.scale() * cam.k_at(q[0], q[1]).max(1e-3)));
        let total: f32 = ch.pts.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
        let (c0, c1) = (s0.max(grow), s1.min(total - grow));
        if c1 <= c0 {
            continue;
        }
        // The fill runs on 1 px into the next piece at a join it reaches (as the line under it).
        let e = joins.get(slot as usize, chain as usize);
        let on = |end: End, reached: bool| match end {
            End::Join { next, .. } if reached => EndPx::Ext(next),
            _ => EndPx::Butt,
        };
        let fill_ends = [on(e[0], c0 - grow <= 0.0), on(e[1], c1 + grow >= total)];
        for (a, b, stroke, ends) in [(c0, c1, Stroke::new(wu + extra * k, casing), [EndPx::Butt; 2]), (c0 - grow, c1 + grow, Stroke::new(wu, up.color), fill_ends)] {
            let mut lines = Vec::new();
            cx.road_line(&piece_between(&ch.pts, a, b), ends, wu, &mut lines, &mut caps, &mut scratch);
            for l in lines {
                cx.p.add(Shape::line(l, stroke));
            }
        }
    }
}

/// Crossings flatter than this (`|sin|` of the angle, ~15 degrees) get no overpass outline.
const OVERPASS_MIN_SIN: f32 = 0.26;

/// The relevance of segment `seg` of a chain in the in-race focus (`None`: the focus has no run
/// for it).
fn seg_relevant(f: &RoadFocus, slot: usize, chain: u32, seg: u32) -> Option<bool> {
    let runs = f.runs.get(slot)?;
    let i = runs.partition_point(|r| r.chain < chain);
    runs[i..].iter().take_while(|r| r.chain == chain).find(|r| r.a <= seg && seg < r.b).map(|r| r.relevant)
}

/// Metres along a polyline to the point `at` on segment `seg`.
fn arc_at(pts: &[[f32; 2]], seg: usize, at: [f32; 2]) -> f32 {
    let d = |a: [f32; 2], b: [f32; 2]| (b[0] - a[0]).hypot(b[1] - a[1]);
    pts.windows(2).take(seg).map(|w| d(w[0], w[1])).sum::<f32>() + d(pts[seg], at)
}

/// The stretch of a polyline from `s0` to `s1` metres along it (clamped to its ends).
fn piece_between(pts: &[[f32; 2]], s0: f32, s1: f32) -> Vec<[f32; 2]> {
    let mut out = Vec::new();
    let mut acc = 0.0f32;
    let lerp = |a: [f32; 2], b: [f32; 2], f: f32| [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
    for w in pts.windows(2) {
        let l = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
        let (a, b) = (acc, acc + l);
        if b >= s0 && a <= s1 && l > 0.0 {
            if out.is_empty() {
                out.push(lerp(w[0], w[1], ((s0 - a) / l).clamp(0.0, 1.0)));
            }
            out.push(lerp(w[0], w[1], ((s1 - a) / l).clamp(0.0, 1.0)));
        }
        acc = b;
        if acc > s1 {
            break;
        }
    }
    out
}

/// One type's share of a [`road_pass`]: its look and what it draws.
struct SlotLines {
    slot: usize,
    /// Fill colour, line width at the car's row (px), dash.
    color: Color32,
    w: f32,
    dash: DashStyle,
    /// Casing colour and its extra px, if the type has a casing in this pass.
    casing: Option<(Color32, f32)>,
    /// Round caps on the fill too: only for a solid, opaque fill (a translucent cap would
    /// darken where it meets its own line; a dashed line ending in a gap would get a dot).
    fill_caps: bool,
    /// (taper factor, line).
    pieces: Vec<(f32, Vec<Pos2>)>,
    /// (taper factor, what) of the round caps and mitre wedges.
    caps: Vec<(f32, Deco)>,
}

fn road_pass(cx: &LayerCtx, layers: &MapLayers, cfg: &MapLayerConfig, focus: Option<&RoadFocus>, pass: Pass, st: &mut LayerStats) {
    let cam = cx.cam;
    let rf = &cfg.race_lines.focus;
    let muted = pass == Pass::Muted;
    let c = &cfg.roads;
    let taper = cfg.tilt.taper;
    let aabb = cam.footprint(30.0);
    // Widths are in design px (`min_px` / `max_px` / the zoom rule), scaled by the HUD's `s` after.
    let base = style::road_base_px(c, cam.scale() / cx.s) * cx.s;
    let dashes = cam.scale() / cx.s >= style::DASH_MIN_PX_PER_M;
    let joins = layers.roads.joins();
    let mut scratch: Vec<Pos2> = Vec::new();
    let mut lines: Vec<Vec<Pos2>> = Vec::new();
    // Is a slot drawn in this pass at all (for a join onto it)?
    let drawn = |slot: usize| slot == 0 || RoadType::from_index(slot as u8).and_then(|t| c.styles.get(t)).is_some_and(|s| s.on);
    let mut slots: Vec<SlotLines> = Vec::with_capacity(style::ROAD_DRAW_ORDER.len());

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
        // Muted: one faint neutral colour, solid, no casing; keeps each type's relative width.
        let (color, alpha, factor, dash, casing) = if muted { (rf.mute_color, rf.mute_alpha, factor * rf.mute_width, DashStyle::None, None) } else { (color, alpha, factor, dash, casing) };
        let w = style::line_px(base, factor);
        let casing_px = c.casing_px * cx.s;
        // A join's line runs on past the node (D81), never further than half its widest stroke.
        let ext_px = 0.5 * (w + if casing.is_some() { casing_px } else { 0.0 });
        // How this pass closes a chain end (muted roads: translucent, no casing: plain ends).
        let end_px = |e: End| match e {
            _ if muted => EndPx::Butt,
            End::Join { next, slot: ns } if pass != Pass::All || drawn(ns as usize) => EndPx::Ext(next),
            _ => EndPx::Cap,
        };
        let mut caps: Vec<Deco> = Vec::new();
        lines.clear();
        match focus {
            None => {
                for (ci, ch) in layers.roads.by_type[slot].iter().enumerate() {
                    if bbox_hits(&ch.bbox, &aabb) {
                        let e = joins.get(slot, ci);
                        cx.road_line(&ch.pts, [end_px(e[0]), end_px(e[1])], ext_px, &mut lines, &mut caps, &mut scratch);
                        st.chains += 1;
                    }
                }
            }
            Some(f) => {
                for run in f.runs[slot].iter().filter(|r| r.relevant == (pass == Pass::Relevant)) {
                    let Some(ch) = layers.roads.by_type[slot].get(run.chain as usize) else { continue };
                    if bbox_hits(&run.bbox, &aabb) {
                        if let Some(pts) = ch.pts.get(run.a as usize..=run.b as usize) {
                            // A run's own ends inside its chain (where the relevance changes) stay plain.
                            let e = joins.get(slot, run.chain as usize);
                            let a = if run.a == 0 { end_px(e[0]) } else { EndPx::Butt };
                            let b = if run.b as usize + 1 == ch.pts.len() { end_px(e[1]) } else { EndPx::Butt };
                            cx.road_line(pts, [a, b], ext_px, &mut lines, &mut caps, &mut scratch);
                            st.chains += 1;
                            st.muted += muted as usize;
                        }
                    }
                }
            }
        }
        if lines.is_empty() {
            continue;
        }
        st.vertices += lines.iter().map(Vec::len).sum::<usize>();
        let pieces = cx.tapered(std::mem::take(&mut lines), taper);
        // (taper factors stay under ~1.3 in the view: below that nothing would be drawn anyway)
        if 0.65 * (w + if casing.is_some() { casing_px } else { 0.0 }) < DECO_MIN_R {
            caps.clear();
        }
        let caps = caps
            .into_iter()
            .map(|d| {
                let (Deco::Cap(at, _) | Deco::Wedge(at, _, _)) = d;
                (cx.taper_k(at.y, taper), d)
            })
            .collect();
        let fill_caps = alpha >= 0.99 && (dash == DashStyle::None || !dashes);
        slots.push(SlotLines { slot, color: cx.c(color, alpha), w, dash, casing: casing.map(|cc| (cx.c(cc, c.casing_alpha), casing_px)), fill_caps, pieces, caps });
    }

    // Width of a piece: the full width at the car's row (k = 1), thinner towards the horizon.
    let wk = |w: f32, k: f32| if k == 1.0 { w } else { (w * k).max(style::MIN_LINE_PX) };
    // Every casing first, then every fill (D81): outlines run round the union of the roads and
    // never across another road's fill at a junction. Roads that only cross (an overpass) get
    // their outline back below.
    for sl in &slots {
        let Some((col, extra)) = sl.casing else { continue };
        for (k, l) in &sl.pieces {
            cx.p.add(Shape::line(l.clone(), Stroke::new(wk(sl.w, *k) + extra * k, col)));
        }
        for &(k, d) in &sl.caps {
            cx.deco(d, 0.5 * (wk(sl.w, k) + extra * k), col);
        }
    }
    for sl in slots.iter_mut() {
        for &(k, d) in sl.caps.iter().filter(|_| sl.fill_caps) {
            cx.deco(d, 0.5 * wk(sl.w, k), sl.color);
        }
        for (k, l) in std::mem::take(&mut sl.pieces) {
            let stroke = Stroke::new(wk(sl.w, k), sl.color);
            match style::dash_pattern(sl.dash, stroke.width).filter(|_| dashes) {
                Some((d, g)) => {
                    cx.p.extend(Shape::dashed_line(&l, stroke, d, g));
                }
                None => {
                    cx.p.add(Shape::line(l, stroke));
                }
            }
        }
    }

    if !muted {
        draw_overpasses(cx, layers, &joins, focus.map(|f| (f, pass == Pass::Relevant)), &slots, taper, dashes);
    }

    // Jump lines: take-off → landing, drawn after the chains.
    let Some(js) = c.styles.get(RoadType::Jump).filter(|s| s.on) else { return };
    for (ji, j) in layers.roads.jumps.iter().enumerate() {
        if let Some(f) = focus {
            if f.jumps.get(ji).copied().unwrap_or(false) != (pass == Pass::Relevant) {
                continue;
            }
        }
        let bb = [j[0].min(j[3]), j[1].min(j[4]), j[0].max(j[3]), j[1].max(j[4])];
        if !bbox_hits(&bb, &aabb) {
            continue;
        }
        let (Some(a), Some(b)) = (cam.project(j[0], j[1]), cam.project(j[3], j[4])) else { continue };
        st.chains += 1;
        st.muted += muted as usize;
        let k = if taper { cam.depth_scale_at_row((a.y + b.y) * 0.5) } else { 1.0 };
        let (jw, jcol, jalpha, jdash, jcasing) = if muted { (js.width * rf.mute_width, rf.mute_color, rf.mute_alpha, DashStyle::None, false) } else { (js.width, js.color, js.alpha, js.dash, js.casing) };
        let w = (base * jw * k).max(1.4 * k.min(1.0));
        let mut seg = [a, b];
        // Cut to the map shape, like the chains (a jump near a rounded corner, or anywhere
        // outside a circle, must not poke out into the transparent surround).
        if let Some(cc) = &cx.corner_clip {
            if !(cc.safe.contains(a) && cc.safe.contains(b)) {
                match clip_segment_convex(a, b, cc.poly) {
                    Some((ca, cb)) => seg = [ca, cb],
                    None => continue,
                }
            }
        }
        if jcasing {
            cx.p.add(Shape::line_segment(seg, Stroke::new(w + 1.8 * cx.s * k, cx.c(js.casing_color, c.casing_alpha))));
        }
        let stroke = Stroke::new(w, cx.c(jcol, jalpha));
        match jdash {
            super::cfg::DashStyle::None => {
                cx.p.add(Shape::line_segment(seg, stroke));
            }
            _ => {
                cx.p.extend(Shape::dashed_line(&seg, stroke, 4.0 * cx.s * k, 3.0 * cx.s * k));
            }
        }
    }
}

/// Vertex budget for the "all lines" mode (one frame).
const RACE_ALL_BUDGET: usize = 40_000;

/// The race lines (`RouteStyle::Line`: thin lines) or race roads (`RouteStyle::Road`, D80: the
/// road look along the line — every casing, then every fill, round ends, in the race colour,
/// opaque, [`style::RACE_ROAD_WIDTH`] on the roads' width rule), then the start / finish marks.
/// Not over the 3D scene (D88): that has them all in GL (`Parts::OVER_3D`).
fn draw_race_lines(cx: &LayerCtx, layers: &MapLayers, cfg: &MapLayerConfig, st: &mut LayerStats) {
    let rc = &cfg.race_lines;
    let taper = cfg.tilt.taper;
    let road = rc.route == RouteStyle::Road;
    let aabb = cx.cam.footprint(20.0);
    let lines = &layers.races.lines;
    let idx: Vec<usize> = if rc.mode == RaceLineMode::All { (0..lines.len()).collect() } else { cx.race_sel.picked().iter().copied().filter(|&i| i < lines.len()).collect() };
    let (mut scratch, mut pieces) = (Vec::new(), Vec::new());
    let mut budget = RACE_ALL_BUDGET;
    // Race roads: (colour, tapered pieces, round ends), drawn after the loop.
    let mut roads: Vec<(Color32, Vec<(f32, Vec<Pos2>)>, Vec<Pos2>)> = Vec::new();
    let mut marks: Vec<(usize, bool, bool)> = Vec::new();
    for i in idx {
        let l = &lines[i];
        if !bbox_hits(&l.bbox, &aabb) {
            continue;
        }
        let rgb = rc.color;
        // An uncertain current race (D76): only the part all candidate routes share, no finish.
        let span = if rc.mode == RaceLineMode::All { None } else { cx.race_sel.span(i).filter(|_| i < layers.races.cum.len()) };
        if rc.marks {
            marks.push((i, span.is_none_or(|sp| sp.start), span.is_none()));
        }
        pieces.clear();
        let sliced;
        let (pts, closed): (&[[f32; 2]], bool) = match span {
            Some(sp) => {
                sliced = Poly::new(&layers.races, i).slice(sp.s0, sp.s1).0;
                (&sliced, false)
            }
            None => (&l.pts, l.closed),
        };
        cx.polyline(pts, closed, &mut pieces, &mut scratch);
        let n: usize = pieces.iter().map(Vec::len).sum();
        if rc.mode == RaceLineMode::All {
            if n > budget {
                continue;
            }
            budget -= n;
        }
        st.race_lines += 1;
        st.vertices += n;
        if road {
            let ends = if closed { Vec::new() } else { [pts.first(), pts.last()].into_iter().flatten().filter_map(|q| cx.cam.project(q[0], q[1])).collect() };
            roads.push((cx.c(rgb, 1.0), cx.tapered(std::mem::take(&mut pieces), taper), ends));
        } else {
            let color = cx.c(rgb, rc.alpha);
            for (k, pc) in cx.tapered(std::mem::take(&mut pieces), taper) {
                cx.p.add(Shape::line(pc, Stroke::new((rc.width_px * cx.s * k).max(style::MIN_LINE_PX), color)));
            }
        }
    }
    if !roads.is_empty() {
        let c = &cfg.roads;
        let w = style::line_px(style::road_base_px(c, cx.cam.scale() / cx.s) * cx.s, style::RACE_ROAD_WIDTH);
        let extra = c.casing_px * cx.s;
        let casing = cx.c(c.styles.road.casing_color, c.casing_alpha);
        let wk = |k: f32| if k == 1.0 { w } else { (w * k).max(style::MIN_LINE_PX) };
        // Every casing under every fill, as the roads (D81): two race roads crossing or a circuit
        // crossing itself join like a junction.
        for (fill, pass) in [(false, 0), (true, 1)] {
            for (col, pcs, ends) in &roads {
                let col = if fill { *col } else { casing };
                for (k, l) in pcs {
                    cx.p.add(Shape::line(l.clone(), Stroke::new(wk(*k) + if fill { 0.0 } else { extra * k }, col)));
                }
                for &at in ends {
                    let k = cx.taper_k(at.y, taper);
                    let r = 0.5 * (wk(k) + if pass == 0 { extra * k } else { 0.0 });
                    if r >= DECO_MIN_R && cx.visible(at, r) && cx.fits(at, r) {
                        cx.p.circle_filled(at, r, col);
                    }
                }
            }
        }
    }
    for (i, start, finish) in marks {
        draw_race_marks(cx, &lines[i], start, finish);
    }
}

/// `start` / `finish`: which of the two marks belong with what is drawn of the line.
fn draw_race_marks(cx: &LayerCtx, l: &crate::gamedata::racelines::RaceLine, start: bool, finish: bool) {
    let (Some(&first), Some(&last)) = (l.pts.first(), l.pts.last()) else { return };
    let size = 9.0 * cx.s;
    // Marks stand upright, only shrunk by the perspective of the row they are on.
    let k = |at: Pos2| cx.cam.depth_scale_at_row(at.y).clamp(0.4, 1.5);
    if let Some(at) = cx.cam.project(first[0], first[1]).filter(|a| start && cx.visible(*a, size) && cx.fits(*a, size * 0.6)) {
        if l.circuit {
            chequer(cx, at, size * k(at));
        } else {
            cx.p.circle(at, 4.5 * cx.s * k(at), cx.c_col(style::START_DOT), Stroke::new(1.5 * cx.s * k(at), cx.c_col(style::START_DOT_OUTLINE)));
        }
    }
    if !l.circuit && finish {
        if let Some(at) = cx.cam.project(last[0], last[1]).filter(|a| cx.visible(*a, size) && cx.fits(*a, size * 0.6)) {
            chequer(cx, at, size * k(at));
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
    let aabb = cam.footprint(size / cam.scale().max(1e-6));
    let pois = &layers.pois;
    let r2 = pc.radius_m * pc.radius_m;
    let near = |it: &Poi| !pc.near_only || (it.x - cx.car.0).powi(2) + (it.z - cx.car.1).powi(2) <= r2;
    // (item, category, screen position, size factor)
    let mut vis: Vec<(usize, u8, Pos2, f32)> = Vec::new();
    pois.grid.query(&aabb, |i| {
        let i = i as usize;
        let cat = pois.cat[i];
        if cat == NO_CAT || mask & (1u64 << cat) == 0 {
            return;
        }
        let it = &pois.items[i];
        if !near(it) {
            return;
        }
        // `k_at` / `project` are the plane maths without a relief and follow the terrain with one.
        let k = cam.k_at(it.x, it.z);
        if k < style::POI_FAR_K {
            return;
        }
        let Some(at) = cam.project(it.x, it.z) else { return };
        if vis.len() < POI_BUDGET && cx.visible(at, size) {
            vis.push((i, cat, at, k.max(style::POI_MIN_K)));
        }
    });
    // Back to front: further (smaller y) first, so near icons overlap far ones.
    vis.sort_by(|a, b| a.2.y.total_cmp(&b.2.y));
    // Gate lines under every icon.
    if pc.gates {
        for &(i, cat, _, k) in &vis {
            if draw_gate(cx, &pois.items[i], cat as usize, k) {
                st.gates += 1;
            }
        }
    }
    for &(i, cat, at, k) in &vis {
        draw_poi(cx, layers, Some(&pois.items[i]), cat as usize, at, size * k);
    }
    st.pois = vis.len();

    // The current treasure chest: one of the chests, picked by the week, drawn bigger and on top.
    if let Some(cat) = style::cat_index("treasure_chest_current").filter(|c| mask & (1u64 << c) != 0) {
        let week = cx.week.unwrap_or_else(week_index_now);
        if let Some(it) = pois.current_chest(week).filter(|it| near(it)) {
            let big = size * style::CURRENT_CHEST_SCALE;
            if let Some(at) = cam.project(it.x, it.z).filter(|a| cx.visible(*a, big)) {
                draw_poi(cx, layers, None, cat, at, big * cam.k_at(it.x, it.z).max(style::POI_MIN_K));
                st.pois += 1;
            }
        }
    }
}

/// A gate's line across the road, in its category's colour (a dark casing under it), at least
/// [`style::GATE_MIN_LEN_PX`] long about its midpoint. Returns whether one was drawn.
fn draw_gate(cx: &LayerCtx, it: &Poi, cat: usize, k: f32) -> bool {
    let Some([l, r]) = it.gate else { return false };
    let (Some(a), Some(b)) = (cx.cam.project(l[0], l[1]), cx.cam.project(r[0], r[1])) else { return false };
    let (mid, d) = ((a.to_vec2() + b.to_vec2()) * 0.5, b - a);
    let min = style::GATE_MIN_LEN_PX * cx.s * k;
    let len = d.length();
    let half = if len < min {
        let dir = if len > 1e-3 { d / len } else { vec2(1.0, 0.0) };
        dir * (min * 0.5)
    } else {
        d * 0.5
    };
    let mut seg = [(mid - half).to_pos2(), (mid + half).to_pos2()];
    if let Some(cc) = &cx.corner_clip {
        if !(cc.safe.contains(seg[0]) && cc.safe.contains(seg[1])) {
            match clip_segment_convex(seg[0], seg[1], cc.poly) {
                Some((a, b)) => seg = [a, b],
                None => return false,
            }
        }
    }
    let w = style::GATE_PX * cx.s * k;
    cx.p.add(Shape::line_segment(seg, Stroke::new(w + style::GATE_CASING_PX * cx.s * k, cx.c_col(Color32::from_black_alpha(190)))));
    cx.p.add(Shape::line_segment(seg, Stroke::new(w, cx.c(style::POI_CATS[cat].color, 1.0))));
    true
}

/// One POI: its game icon when the atlas has one (`item` picks the race class / mascot region),
/// else the category's shape marker.
fn draw_poi(cx: &LayerCtx, layers: &MapLayers, item: Option<&Poi>, cat: usize, at: Pos2, size: f32) {
    if let Some(atlas) = cx.icons {
        let uv = match item {
            Some(it) => atlas.uv_of(cat, it, layers),
            None => atlas.uv(cat),
        };
        if let Some(uv) = uv {
            let quad = Rect::from_center_size(at, vec2(size, size));
            let tint = Color32::WHITE.gamma_multiply(cx.a);
            let mut m = Mesh::with_texture(atlas.texture);
            match &cx.corner_clip {
                // The quad pokes out of a rounded corner: cut it to the outline, UVs follow.
                Some(cc) if !cc.safe.contains_rect(quad) => {
                    let poly = clip_convex(&[quad.left_top(), quad.right_top(), quad.right_bottom(), quad.left_bottom()], cc.poly);
                    if poly.len() < 3 {
                        return;
                    }
                    let uv_at = |pt: Pos2| pos2(uv.min.x + (pt.x - quad.min.x) / size * uv.width(), uv.min.y + (pt.y - quad.min.y) / size * uv.height());
                    let hub = (poly.iter().fold(vec2(0.0, 0.0), |a, q| a + q.to_vec2()) / poly.len() as f32).to_pos2();
                    fan(&mut m, hub, &poly, |pt| Vertex { pos: pt, uv: uv_at(pt), color: tint });
                }
                _ => m.add_rect_with_uv(quad, uv, tint),
            }
            cx.p.add(Shape::mesh(m));
            return;
        }
    }
    let c = &style::POI_CATS[cat];
    let r = (size * 0.27).max(2.5);
    if !cx.fits(at, r * 1.4) {
        return;
    }
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
                Shape::Path(p) if !p.closed => Some(p.points.clone()),
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

    static NO_SEL: std::sync::LazyLock<RaceSel> = std::sync::LazyLock::new(RaceSel::default);

    fn ctx<'a>(p: &'a Painter, cam: &'a Camera) -> LayerCtx<'a> {
        LayerCtx { p, cam, s: 1.0, a: 1.0, car: (0.0, 0.0), corner_clip: None, icons: None, race_sel: &NO_SEL, week: Some(68) }
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
                Shape::Path(p) if !p.closed => Some(p.stroke.width),
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
        cfg.tilt.taper = false;
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
        // synthetic: house, fast travel, car meet, speed trap, barn find; fast travel is off by default since D71.
        assert_eq!(n(&cfg, &cam), 4);
        cfg.pois.categories = vec!["house".into(), "not_a_category".into()];
        assert_eq!(n(&cfg, &cam), 1);
        cfg.pois = MapLayerConfig::default().pois;
        cfg.pois.on = false;
        assert_eq!(n(&cfg, &cam), 0);
        // Hidden above max_zoom_m (10000 since D71): a 12000 m radius shows none.
        cfg.pois.on = true;
        let wide = flat_cam(rect, (0.0, 0.0), 0.0, 12000.0);
        assert_eq!(n(&cfg, &wide), 0);
        // near_only within 400 m of the car at the origin keeps house (316 m)
        // and car meet (250 m); speed trap and barn find (515 m) go.
        cfg.pois.near_only = true;
        cfg.pois.radius_m = 400.0;
        assert_eq!(n(&cfg, &cam), 2);
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
        // The other three POIs are still markers.
        assert_eq!(shapes.len(), 4);
    }

    fn poi(kind: PoiKind, name: &str, n: u32, x: f32, z: f32, gate: Option<[[f32; 2]; 2]>) -> Poi {
        Poi { kind, x, z, y: 0.0, name: name.into(), n, gate }
    }

    fn only_pois(cats: &[&str]) -> MapLayerConfig {
        let mut c = MapLayerConfig::default();
        c.roads.on = false;
        c.race_lines.mode = RaceLineMode::Off;
        c.pois.categories = cats.iter().map(|s| s.to_string()).collect();
        c
    }

    fn meshes(shapes: &[ClippedShape]) -> Vec<egui::epaint::Mesh> {
        shapes.iter().filter_map(|s| if let Shape::Mesh(m) = &s.shape { Some((**m).clone()) } else { None }).collect()
    }

    /// The HUD pill (208 x 136, radius 22): vectors that would poke out of the rounded corners
    /// are cut to the outline, and a road wholly in the cut-off corner disappears.
    #[test]
    fn corner_clip_on_the_real_pill_outline() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(208.0, 136.0));
        let outline = crate::hud::prims::rounded_points(rect, [22.0; 4]);
        let cc = CornerClip { poly: &outline, safe: rect.shrink(22.0) };
        // 1 px per metre, north up, the car in the middle: screen (x, y) = world (x - 104, 68 - y).
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 68.0);
        let mut cfg = only_roads();
        cfg.roads.styles.road.casing = false;
        let run = |layers: &MapLayers| {
            paint(rect, |p| {
                let mut c = ctx(p, &cam);
                c.corner_clip = Some(cc);
                draw_layers(&c, layers, &cfg);
            })
        };
        // Wholly inside the cut-off corner: screen (2, 2) -> (6, 6) is further than 22 px from (22, 22).
        assert!(path_points(&run(&layers_with(vec![[-102.0, 66.0], [-98.0, 62.0]], RoadType::Road))).is_empty());
        // A diagonal from the very corner into the pill: only the part inside the outline is left.
        let lines = path_points(&run(&layers_with(vec![[-104.0, 68.0], [-44.0, 8.0]], RoadType::Road)));
        assert_eq!(lines.len(), 1);
        for q in &lines[0] {
            if q.x < 22.0 && q.y < 22.0 {
                assert!((*q - pos2(22.0, 22.0)).length() <= 22.0 + 0.05, "{q:?} pokes out of the corner");
            }
        }
        assert!(lines[0].len() >= 2 && lines[0].last().unwrap().x > 40.0, "the inner part is kept: {:?}", lines[0]);
        // A road in the middle (inside the safe rect) is passed through untouched.
        let mid = path_points(&run(&layers_with(vec![[-50.0, 0.0], [50.0, 0.0]], RoadType::Road)));
        assert_eq!(mid[0], vec![pos2(54.0, 68.0), pos2(154.0, 68.0)]);
    }

    /// Icons, gate lines and markers next to a rounded corner are cut to the outline (icons) or
    /// dropped (small markers), never left poking into the transparent surround.
    #[test]
    fn pois_near_the_pill_corner_are_clipped_to_the_outline() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(208.0, 136.0));
        let outline = crate::hud::prims::rounded_points(rect, [22.0; 4]);
        let cc = CornerClip { poly: &outline, safe: rect.shrink(22.0) };
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 68.0); // 1 px/m: screen (x, y) = world (x - 104, 68 - y)
        // A speed zone in the top-left: its centre (14, 14) is inside the outline (11 px from the
        // arc centre, the radius is 22), its 32 px icon and its marker are not.
        let at = |sx: f32, sy: f32| (sx - 104.0, 68.0 - sy);
        let (x, z) = at(14.0, 14.0);
        let layers = MapLayers { pois: Arc::new(crate::maprender::data::PoiLayer::from_items([poi(PoiKind::SpeedZone, "z", 1, x, z, None)])), ..Default::default() };
        let cfg = only_pois(&["speed_zone"]);
        let mut atlas = IconAtlas::new(TextureId::Managed(2));
        atlas.set("speed_zone", Rect::from_min_max(pos2(0.25, 0.5), pos2(0.5, 0.75)));
        let run = |icons: Option<&IconAtlas>| {
            paint(rect, |p| {
                let mut c = ctx(p, &cam);
                c.corner_clip = Some(cc);
                c.icons = icons;
                draw_layers(&c, &layers, &cfg);
            })
        };
        let m = meshes(&run(Some(&atlas)));
        assert_eq!(m.len(), 1);
        for v in &m[0].vertices {
            if v.pos.x < 22.0 && v.pos.y < 22.0 {
                assert!((v.pos - pos2(22.0, 22.0)).length() <= 22.0 + 0.05, "{:?} outside the corner arc", v.pos);
            }
            // The UVs follow the cut: still inside the icon's cell.
            assert!(v.uv.x >= 0.25 - 1e-4 && v.uv.x <= 0.5 + 1e-4 && v.uv.y >= 0.5 - 1e-4 && v.uv.y <= 0.75 + 1e-4, "{:?}", v.uv);
        }
        assert!(m[0].vertices.len() > 4, "cut into a polygon");
        // Without icons the small marker would poke out at this spot: it is dropped. In the middle it stays.
        assert!(run(None).is_empty());
        let (x, z) = at(104.0, 68.0);
        let mid = MapLayers { pois: Arc::new(crate::maprender::data::PoiLayer::from_items([poi(PoiKind::SpeedZone, "z", 1, x, z, None)])), ..Default::default() };
        assert_eq!(paint(rect, |p| {
            let mut c = ctx(p, &cam);
            c.corner_clip = Some(cc);
            draw_layers(&c, &mid, &cfg);
        }).len(), 1);
    }

    /// The HUD scales everything by `s` (design px -> screen px): road widths included, however
    /// the zoom rule works, because the rule runs in design px.
    #[test]
    fn road_widths_scale_with_the_size_factor() {
        let widths = |s: f32| {
            let rect = Rect::from_min_size(Pos2::ZERO, vec2(208.0 * s, 136.0 * s));
            // Same metres on screen: the camera's px per metre grows with s.
            let cam = flat_cam(rect, (0.0, 0.0), 0.0, 300.0);
            let mut cfg = only_roads();
            cfg.roads.styles.road.casing = false;
            let layers = layers_with(vec![[0.0, -100.0], [0.0, 100.0]], RoadType::Road);
            let shapes = paint(rect, |p| {
                let mut c = ctx(p, &cam);
                c.s = s;
                draw_layers(&c, &layers, &cfg);
            });
            shapes.iter().filter_map(|s| match &s.shape { Shape::Path(p) if !p.closed => Some(p.stroke.width), _ => None }).collect::<Vec<_>>()
        };
        let (w1, w2) = (widths(1.0), widths(2.0));
        assert_eq!((w1.len(), w2.len()), (1, 1));
        assert!((w2[0] - 2.0 * w1[0]).abs() < 1e-3, "{w1:?} vs {w2:?}");
        // 300 m radius on a 136 px pill: 0.227 px/m * 10 m = 2.27 px.
        assert!((w1[0] - 2.2667).abs() < 0.01, "{w1:?}");
    }

    // ── joins (D81) ──

    fn layers_of(chains: &[(RoadType, &[[f32; 2]], f32)]) -> MapLayers {
        let mut roads = RoadLayer::default();
        for (t, pts, y) in chains {
            roads.by_type[t.index() as usize].push(Chain::new(pts.to_vec(), vec![*y; pts.len()]));
        }
        MapLayers { rev: 1, roads: Arc::new(roads), ..Default::default() }
    }

    fn solid(m: &egui::epaint::ColorMode) -> Color32 {
        match m {
            egui::epaint::ColorMode::Solid(c) => *c,
            _ => Color32::TRANSPARENT,
        }
    }

    /// The shapes tessellated into one mesh (feathering off: exact edges).
    fn tessellate(shapes: &[ClippedShape]) -> Mesh {
        let opts = egui::epaint::TessellationOptions { feathering: false, ..Default::default() };
        let mut t = egui::epaint::Tessellator::new(1.0, opts, [1, 1], vec![]);
        let mut mesh = Mesh::default();
        for s in shapes {
            t.tessellate_shape(s.shape.clone(), &mut mesh);
        }
        mesh
    }

    /// The colour of the last triangle that covers `pt` (what is on top there), if any.
    fn top_colour(mesh: &Mesh, pt: Pos2) -> Option<Color32> {
        let cross = |a: Pos2, b: Pos2, p: Pos2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        mesh.indices.chunks(3).rev().find_map(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.vertices[i as usize]);
            let (d1, d2, d3) = (cross(a.pos, b.pos, pt), cross(b.pos, c.pos, pt), cross(c.pos, a.pos, pt));
            let inside = (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0);
            (inside && cross(a.pos, b.pos, c.pos).abs() > 1e-6).then_some(a.color)
        })
    }

    /// Junctions: every casing is drawn before every fill (so no outline crosses a road at a
    /// junction), and the side road ending on the through road gets a round cap there.
    #[test]
    fn every_casing_is_drawn_before_every_fill_and_a_junction_end_gets_a_round_cap() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 50.0); // 2 px per metre: 10 px roads
        let layers = layers_of(&[(RoadType::Road, &[[-40.0, 0.0], [0.0, 0.0], [40.0, 0.0]], 10.0), (RoadType::Offroad, &[[0.0, 0.0], [0.0, 40.0]], 10.0)]);
        let cfg = only_roads();
        let shapes = paint(rect, |p| {
            draw_layers(&ctx(p, &cam), &layers, &cfg);
        });
        let st = &cfg.roads.styles;
        let casing = [st.road.casing_color.color(1.0), st.offroad.casing_color.color(1.0)];
        let colour = |s: &Shape| match s {
            Shape::Path(p) if p.closed => p.fill,
            Shape::Path(p) => solid(&p.stroke.color),
            _ => Color32::TRANSPARENT,
        };
        let is_casing: Vec<bool> = shapes.iter().map(|s| casing.contains(&colour(&s.shape))).collect();
        let first_fill = is_casing.iter().position(|c| !c).expect("fills");
        assert!(is_casing[..first_fill].iter().all(|&c| c) && is_casing[first_fill..].iter().all(|&c| !c), "{is_casing:?}");
        // Round caps: the road's two dead ends and the offroad's two ends (junction + dead end),
        // each in the casing pass and the fill pass.
        let caps = shapes.iter().filter(|s| matches!(&s.shape, Shape::Path(p) if p.closed)).count();
        assert_eq!(caps, 2 * (2 + 2));
        // At the junction no outline crosses the side road's mouth: just past the through road's
        // fill (10 px wide), where its casing band (1.4 px) runs, the side road's fill shows.
        let mesh = tessellate(&shapes);
        let c = rect.center();
        for dx in [-3.0, 0.0, 3.0] {
            assert_eq!(top_colour(&mesh, c + vec2(dx, -4.7)), Some(st.road.color.color(1.0)), "the road's fill at {dx}");
            assert_eq!(top_colour(&mesh, c + vec2(dx, -5.35)), Some(st.offroad.color.color(1.0)), "the mouth at {dx}");
        }
        // Beside the mouth the road keeps its outline.
        assert_eq!(top_colour(&mesh, c + vec2(-20.0, -5.35)), Some(casing[0]));
    }

    /// An L-corner of two chains (the screenshot of D81): mitred, the outer corner filled (it was
    /// a notch between two square ends), the colour boundary of a type change on the bisector.
    #[test]
    fn an_l_corner_of_two_chains_is_mitred_and_a_type_change_meets_on_the_bisector() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 50.0);
        let mut cfg = only_roads();
        cfg.roads.styles.road.casing = false;
        cfg.roads.styles.offroad.casing = false;
        let c = rect.center();
        let w = style::line_px(style::road_base_px(&cfg.roads, cam.scale()), 1.0);
        assert!((w - 10.0).abs() < 1e-3, "{w}");
        // East to the node, then north (screen: from the left, then up).
        let l = layers_of(&[(RoadType::Road, &[[-40.0, 0.0], [0.0, 0.0]], 10.0), (RoadType::Road, &[[0.0, 0.0], [0.0, 40.0]], 10.0)]);
        let shapes = paint(rect, |p| {
            draw_layers(&ctx(p, &cam), &l, &cfg);
        });
        let mesh = tessellate(&shapes);
        let road = cfg.roads.styles.road.color.color(1.0);
        // The outer corner (screen right-down of the node): covered up to the mitre's tip.
        assert_eq!(top_colour(&mesh, c + vec2(4.0, 4.0)), Some(road));
        assert_eq!(top_colour(&mesh, c + vec2(4.9, 4.9)), Some(road));
        assert_eq!(top_colour(&mesh, c + vec2(6.0, 6.0)), None, "no overshoot past the mitre");
        assert_eq!(top_colour(&mesh, c + vec2(6.0, 0.0)), None, "nothing past the corner to the east");
        // The same corner as a type change (road -> offroad): road on top, the boundary along the
        // bisector: road colour just before it (1 px overlap), offroad colour past it.
        let t = layers_of(&[(RoadType::Road, &[[-40.0, 0.0], [0.0, 0.0]], 10.0), (RoadType::Offroad, &[[0.0, 0.0], [0.0, 40.0]], 10.0)]);
        let mesh = tessellate(&paint(rect, |p| {
            draw_layers(&ctx(p, &cam), &t, &cfg);
        }));
        let off = cfg.roads.styles.offroad.color.color(1.0);
        // Points just off the bisector (screen y = -x + ... through the node, from upper-left to
        // lower-right): left-below of it is road, right-above is offroad.
        assert_eq!(top_colour(&mesh, c + vec2(-3.0, 1.0)), Some(road));
        assert_eq!(top_colour(&mesh, c + vec2(2.0, -4.0)), Some(off));
        assert_eq!(top_colour(&mesh, c + vec2(4.0, 4.0)), Some(road), "outer corner filled");
        assert_eq!(top_colour(&mesh, c + vec2(3.0, -4.9)), Some(off));
    }

    /// The in-race focus hides an arm of a 4-way: nothing of it is drawn, not even a stub of a
    /// cap; the other arms' caps stay inside their own width.
    #[test]
    fn a_hidden_arm_leaves_no_stub_at_the_junction() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 50.0);
        let arms: [[[f32; 2]; 2]; 4] = [[[0.0, 0.0], [-40.0, 0.0]], [[0.0, 0.0], [40.0, 0.0]], [[0.0, 0.0], [0.0, -40.0]], [[0.0, 0.0], [0.0, 40.0]]];
        let layers = layers_of(&arms.iter().map(|a| (RoadType::Road, &a[..], 10.0)).collect::<Vec<_>>());
        let mut focus = RoadFocus::default();
        let road = RoadType::Road.index() as usize;
        focus.runs[road] = (0..4).map(|c| crate::maprender::racesel::Run { chain: c, a: 0, b: 1, relevant: c != 3, bbox: layers.roads.by_type[road][c as usize].bbox }).collect();
        let mut cfg = only_roads();
        cfg.race_lines.focus.other_roads = OtherRoads::Hidden;
        let mut st = LayerStats::default();
        let shapes = paint(rect, |p| draw_roads(&ctx(p, &cam), &layers, &cfg, OtherRoads::Hidden, Some(&focus), &mut st));
        assert_eq!(st.chains, 3);
        let mesh = tessellate(&shapes);
        let c = rect.center();
        // North of the node (screen up) past the half-width + casing: nothing.
        for dy in [7.0, 10.0, 30.0] {
            assert_eq!(top_colour(&mesh, c + vec2(0.0, -dy)), None, "hidden arm at {dy} px");
        }
        assert!(top_colour(&mesh, c + vec2(0.0, 30.0)).is_some(), "the south arm is drawn");
    }

    /// An overpass (a highway 12 m over a road, no shared node) keeps its outline over the road:
    /// after all fills the highway's stretch is drawn once more, casing then fill. At grade, no.
    #[test]
    fn an_overpass_is_drawn_again_with_its_casing_over_the_lower_road() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 50.0);
        let cfg = only_roads();
        let st = &cfg.roads.styles;
        let hw_casing: Color32 = st.highway.casing_color.color(1.0);
        let count = |dy: f32| {
            let l = layers_of(&[(RoadType::Highway, &[[-40.0, 0.0], [40.0, 0.0]], 10.0 + dy), (RoadType::Road, &[[-30.0, -30.0], [30.0, 30.0]], 10.0)]);
            let shapes = paint(rect, |p| {
                draw_layers(&ctx(p, &cam), &l, &cfg);
            });
            let open: Vec<Color32> = shapes.iter().filter_map(|s| match &s.shape {
                Shape::Path(p) if !p.closed => Some(solid(&p.stroke.color)),
                _ => None,
            }).collect();
            (open.iter().filter(|c| **c == hw_casing).count(), open)
        };
        let (n, open) = count(12.0);
        assert_eq!(n, 2, "{open:?}");
        assert_eq!(open[open.len() - 2], hw_casing, "the stretch's casing, then its fill, last");
        assert_eq!(open[open.len() - 1], st.highway.color.color(1.0));
        assert_eq!(count(0.0).0, 1, "at grade: drawn once");
        // The road under it: its casing, then its fill; a road over the highway gets the stretch.
        let (n, _) = count(-12.0);
        assert_eq!(n, 1);
    }

    #[test]
    fn icon_atlas_covers_every_enabled_category_and_picks_race_and_mascot_icons() {
        let icons = crate::maprender::icontex::synthetic_icons();
        let a = IconAtlas::from_poi_icons(TextureId::Managed(1), &icons);
        let rect_of = |id: &str| a.rects[style::cat_index(id).expect(id)];
        for n in crate::maprender::cfg::POI_DEFAULT_ON {
            assert!(rect_of(n).is_some(), "default-on category {n} has no icon");
        }
        // Each rect is the table's UV of the category's icon kind; kinds without an icon stay None.
        for (i, c) in style::POI_CATS.iter().enumerate() {
            let want = c.icon_kind().and_then(|k| icons.uv.get(&k)).map(uv_rect);
            assert_eq!(a.rects[i], want, "{}", c.id);
        }
        // The current chest shows the chest's icon, a danger sign its own, the pin / mascot by item.
        assert_eq!(rect_of("treasure_chest_current"), rect_of("treasure_chest"));
        assert_ne!(rect_of("danger_sign"), rect_of("treasure_chest"));
        assert!(rect_of("landmark").is_none() && rect_of("pinata").is_none());
        let mut layers = MapLayers::synthetic();
        layers.race_class = Arc::new(HashMap::from([(7, RaceClass::Dragracing)]));
        let cat = |id: &str| style::cat_index(id).unwrap();
        let pin = |n| poi(PoiKind::RacePin, "", n, 0.0, 0.0, None);
        assert_eq!(a.uv_of(cat("race_pin"), &pin(7), &layers), Some(a.race[&RaceClass::Dragracing]));
        assert_eq!(a.uv_of(cat("race_pin"), &pin(8), &layers), rect_of("race_pin"), "unmarked route: the class-less pin icon");
        let mascot = poi(PoiKind::Mascot, "", 3, 0.0, 0.0, None);
        assert_eq!(a.uv_of(cat("mascot"), &mascot, &layers), Some(a.mascot[&3]));
    }

    /// The current treasure chest: of the chests only the one the week names is drawn, bigger,
    /// and it moves on when the week rolls.
    #[test]
    fn the_current_chest_follows_the_week_and_is_drawn_bigger() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 300.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 1000.0);
        let chest = |n: u32, x: f32| poi(PoiKind::TreasureChest, &format!("DISCOUNT_BOARD_TREASURE_CHEST_{n:03}"), n, x, 0.0, None);
        let layers = MapLayers { pois: Arc::new(crate::maprender::data::PoiLayer::from_items([chest(16, -100.0), chest(17, 100.0)])), ..Default::default() };
        let cfg = only_pois(&["treasure_chest_current"]);
        let mut atlas = IconAtlas::new(TextureId::Managed(5));
        atlas.set("treasure_chest_current", Rect::from_min_max(pos2(0.0, 0.0), pos2(0.5, 0.5)));
        let at_week = |week: i64, icons: Option<&IconAtlas>| {
            let mut st = LayerStats::default();
            let shapes = paint(rect, |p| {
                let mut c = ctx(p, &cam);
                c.week = Some(week);
                c.icons = icons;
                st = draw_layers(&c, &layers, &cfg);
            });
            (st.pois, shapes)
        };
        // Chest number = week - 52: week 68 -> chest 016 (west), week 69 -> chest 017 (east).
        for (week, west) in [(68, true), (69, false)] {
            let (n, shapes) = at_week(week, Some(&atlas));
            assert_eq!(n, 1, "week {week}");
            let m = meshes(&shapes);
            assert_eq!(m.len(), 1);
            let v = &m[0].vertices;
            let (w, cx) = (v[1].pos.x - v[0].pos.x, (v[1].pos.x + v[0].pos.x) / 2.0);
            assert!((w - 32.0 * style::CURRENT_CHEST_SCALE).abs() < 1e-3, "icon is {w} px wide");
            assert_eq!(cx < 150.0, west, "week {week}: icon at x {cx}");
        }
        // Without an atlas the shape marker stands in; a week before every chest draws none.
        assert_eq!(at_week(68, None).1.len(), 1);
        assert_eq!(at_week(10, Some(&atlas)).0, 0);
        // The plain chest category is not turned on by the current one.
        let (n, _) = at_week(68, None);
        assert_eq!(n, 1);
        // Off: nothing.
        let mut off = cfg.clone();
        off.pois.categories.clear();
        assert_eq!(paint(rect, |p| {
            let mut c = ctx(p, &cam);
            c.week = Some(68);
            draw_layers(&c, &layers, &off);
        }).len(), 0);
    }

    #[test]
    fn gate_lines_are_drawn_only_when_enabled_and_never_shorter_than_the_minimum() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(300.0, 300.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 150.0); // 1 px per metre
        let layers = MapLayers {
            pois: Arc::new(crate::maprender::data::PoiLayer::from_items([
                poi(PoiKind::SpeedTrap, "SPEEDCAMERA_07", 7, 0.0, 50.0, Some([[-8.0, 50.0], [8.0, 50.0]])),
                poi(PoiKind::SpeedZone, "z1", 1, 0.0, -50.0, Some([[0.0, -49.0], [0.0, -51.0]])), // a 2 m gate
                poi(PoiKind::House, "h", 0, 40.0, 40.0, None),
            ])),
            ..Default::default()
        };
        let mut cfg = only_pois(&["speed_trap", "speed_zone", "house"]);
        let run = |cfg: &MapLayerConfig| {
            let mut st = LayerStats::default();
            let shapes = paint(rect, |p| st = draw_layers(&ctx(p, &cam), &layers, cfg));
            (st, shapes)
        };
        let (st, shapes) = run(&cfg);
        assert_eq!((st.pois, st.gates), (3, 2));
        let segs: Vec<[Pos2; 2]> = shapes.iter().filter_map(|s| if let Shape::LineSegment { points, .. } = &s.shape { Some(*points) } else { None }).collect();
        assert_eq!(segs.len(), 4, "casing + line per gate");
        let len = |s: &[Pos2; 2]| (s[1] - s[0]).length();
        assert!((len(&segs[0]) - 16.0).abs() < 1e-3 && (len(&segs[1]) - 16.0).abs() < 1e-3, "{segs:?}"); // the 16 m trap gate
        assert!((len(&segs[2]) - style::GATE_MIN_LEN_PX).abs() < 1e-3, "the 2 m gate is stretched: {segs:?}");
        // The line's midpoint is the POI's own position, whatever the stretch.
        assert!((((segs[2][0] + segs[2][1].to_vec2()) * 0.5) - pos2(150.0, 200.0)).length() < 1e-3);
        // Gates off: no lines; a gate whose category is off draws none either.
        cfg.pois.gates = false;
        let (st, shapes) = run(&cfg);
        assert_eq!((st.pois, st.gates), (3, 0));
        assert!(!shapes.iter().any(|s| matches!(s.shape, Shape::LineSegment { .. })));
        cfg.pois.gates = true;
        cfg.pois.categories = vec!["speed_trap".into()];
        assert_eq!(run(&cfg).0.gates, 1);
    }

    #[test]
    fn tilt_taper_thins_roads_towards_the_horizon() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(208.0, 136.0));
        let cam = Camera::from_cfg(&MapLayerConfig::hud().tilt, (0.0, 0.0), 0.0, 300.0, rect);
        let layers = layers_with((0..70).map(|i| [0.0, -60.0 + 50.0 * i as f32]).collect(), RoadType::Road);
        let widths = |taper: bool, cam: &Camera| {
            let mut cfg = only_roads();
            cfg.roads.styles.road.casing = false;
            cfg.tilt.taper = taper;
            let shapes = paint(rect, |p| {
                draw_layers(&ctx(p, cam), &layers, &cfg);
            });
            let mut w: Vec<f32> = shapes.iter().filter_map(|s| match &s.shape { Shape::Path(p) if !p.closed => Some(p.stroke.width), _ => None }).collect();
            w.sort_by(f32::total_cmp);
            w
        };
        let tapered = widths(true, &cam);
        assert!(tapered.len() >= 3, "{tapered:?}");
        assert!(tapered.last().unwrap() / tapered[0] > 1.8, "near end should be much wider than far end: {tapered:?}");
        let plain = widths(false, &cam);
        assert_eq!(plain.len(), 1);
        // The widest tapered piece is at the car's row and below: about the full width.
        assert!((tapered.last().unwrap() - plain[0]).abs() < plain[0] * 0.35, "{tapered:?} vs {plain:?}");
        // Flat view: taper has nothing to do.
        let flat = flat_cam(rect, (0.0, 0.0), 0.0, 300.0);
        assert_eq!(widths(true, &flat).len(), 1);
    }

    /// Is `pt` inside one of `mesh`'s triangles (and the alpha there, nearest vertex's)?
    fn covered(mesh: &egui::epaint::Mesh, pt: Pos2) -> Option<u8> {
        let cross = |a: Pos2, b: Pos2, p: Pos2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        mesh.indices.chunks(3).find_map(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.vertices[i as usize]);
            let (d1, d2, d3) = (cross(a.pos, b.pos, pt), cross(b.pos, c.pos, pt), cross(c.pos, a.pos, pt));
            let inside = (d1 >= -1e-3 && d2 >= -1e-3 && d3 >= -1e-3) || (d1 <= 1e-3 && d2 <= 1e-3 && d3 <= 1e-3);
            let area = cross(a.pos, b.pos, c.pos).abs();
            (inside && area > 1e-6).then(|| [a, b, c].iter().min_by(|x, y| (x.pos - pt).length_sq().total_cmp(&(y.pos - pt).length_sq())).unwrap().color.a())
        })
    }

    /// The HUD pill (208 x 136, 22 px corners) at `scale`, with the HUD's tilt config at `angle`.
    fn pill_base(angle: f32, scale: f32, mirror: bool, fade: bool) -> (Camera, Vec<Pos2>, Vec<egui::epaint::Mesh>) {
        let rect = Rect::from_min_size(pos2(30.0, 40.0), vec2(208.0, 136.0) * scale);
        let outline = crate::hud::prims::rounded_points(rect.shrink(0.5 * scale), [21.5 * scale; 4]);
        let mut tilt = MapLayerConfig::hud().tilt;
        tilt.on = true;
        tilt.angle_deg = angle;
        // Near the map's centre so mirror off still has image all around.
        let cam = Camera::from_cfg(&tilt, (-409.0, -1541.0), 0.4, 300.0, rect);
        let tex = MapTex { id: TextureId::Managed(3), orig_size: [8192, 8192], winter: false };
        let shapes = paint(rect, |p| {
            draw_base(p, &BaseParams { cam: &cam, cal: MapCalibration::DEFAULT, tex, outline: &outline, mirror, look: ImageLook::FULL, a: 1.0, far_fade: fade });
        });
        (cam, outline, meshes(&shapes))
    }

    /// Points spread over the pill's inside, including right next to the rounded top corners.
    fn pill_samples(cam: &Camera, outline: &[Pos2]) -> Vec<Pos2> {
        let r = cam.rect;
        let mut pts = Vec::new();
        for ix in 0..=20 {
            for iy in 0..=20 {
                pts.push(r.min + vec2(r.width() * ix as f32 / 20.0, r.height() * iy as f32 / 20.0));
            }
        }
        // Just inside each outline vertex, towards the centre (the arcs of the corners).
        pts.extend(outline.iter().map(|&q| q + (r.center() - q).normalized() * 1.5));
        pts.retain(|&q| inside_convex(q, outline) && outline.iter().all(|&o| (o - q).length() > 0.75));
        pts
    }

    #[test]
    fn the_tilted_base_fills_the_whole_pill() {
        // The bug: the plane ended a fixed 3.2 view heights ahead, ~26 px under the pill's top at
        // 55 deg, and faded over the next 41 px. Now every point inside the pill is drawn, fully
        // opaque at the default tilt, mirrored or cut to the image, at 1x and 3x.
        for (angle, scale, mirror) in [(55.0, 1.0, true), (55.0, 3.0, true), (55.0, 1.0, false), (45.0, 1.0, true), (52.0, 2.0, true)] {
            let (cam, outline, m) = pill_base(angle, scale, mirror, true);
            assert_eq!(m.len(), 1);
            assert!(cam.far_row() < cam.rect.top(), "{angle}: horizon out of view");
            for q in pill_samples(&cam, &outline) {
                let a = covered(&m[0], q);
                assert_eq!(a, Some(255), "{angle} deg x{scale} mirror {mirror}: {q:?} (top {})", cam.rect.top());
            }
        }
        // Steep (70 deg): the horizon is in the pill. The image reaches to just under it
        // (`FAR_MIN_SCALE`) and fades there; everything below the far row is covered.
        let (cam, outline, m) = pill_base(70.0, 1.0, true, true);
        let far = cam.far_row();
        let horizon = cam.row_of_depth_scale(0.0);
        assert!(far > cam.rect.top() && far - horizon < 0.06 * cam.focal / cam.pitch.tan() + 0.01, "far {far} horizon {horizon}");
        for q in pill_samples(&cam, &outline) {
            let a = covered(&m[0], q);
            if q.y > far + 0.5 {
                assert!(a.is_some(), "{q:?} below the far row {far}");
            } else if q.y < far - 0.5 {
                assert!(a.is_none(), "{q:?} above the far row {far}");
            }
        }
    }

    #[test]
    fn the_far_edge_fades_only_near_the_horizon() {
        let (cam, _, m) = pill_base(70.0, 1.0, true, true);
        let mut seen = (false, false);
        for v in &m[0].vertices {
            // Linear in the screen row: transparent at the far limit, opaque FAR_FADE_DEPTH lower.
            let want = ((cam.depth_scale_at_row(v.pos.y) - FAR_MIN_SCALE) / style::FAR_FADE_DEPTH).clamp(0.0, 1.0);
            assert!((v.color.a() as f32 / 255.0 - want).abs() < 0.01, "alpha {} at y {}", v.color.a(), v.pos.y);
            seen = (seen.0 | (v.color.a() < 5), seen.1 | (v.color.a() == 255));
        }
        assert_eq!(seen, (true, true), "both the faded far end and the solid near part are there");
        // The fade band is a narrow strip: under 10 % of the pill's height at 70 deg.
        let band = cam.row_of_depth_scale(FAR_MIN_SCALE + style::FAR_FADE_DEPTH) - cam.far_row();
        assert!(band > 0.0 && band < 0.1 * cam.rect.height(), "{band}");
        // No fade asked for: every vertex stays opaque.
        assert!(pill_base(70.0, 1.0, true, false).2[0].vertices.iter().all(|v| v.color.a() == 255));
        // Flat with the fade on: opaque too.
        let rect = Rect::from_min_size(pos2(20.0, 10.0), vec2(300.0, 200.0));
        let outline = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
        let tex = MapTex { id: TextureId::Managed(3), orig_size: [8192, 8192], winter: false };
        let cam = flat_cam(rect, (-409.0, -6541.0), 0.4, 800.0);
        let shapes = paint(rect, |p| {
            draw_base(p, &BaseParams { cam: &cam, cal: MapCalibration::DEFAULT, tex, outline: &outline, mirror: true, look: ImageLook::FULL, a: 1.0, far_fade: true });
        });
        assert!(meshes(&shapes)[0].vertices.iter().all(|v| v.color.a() == 255));
    }

    #[test]
    fn a_tilted_dashboard_map_fills_its_rect() {
        // The Dashboard map: a plain rect outline, focal scaled to its height, tilt on.
        for (w, h, angle) in [(540.0, 540.0, 55.0), (800.0, 300.0, 55.0), (400.0, 600.0, 62.0)] {
            let rect = Rect::from_min_size(pos2(5.0, 7.0), vec2(w, h));
            let outline = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
            let mut tilt = MapLayerConfig::default().tilt;
            tilt.on = true;
            tilt.angle_deg = angle;
            let cam = Camera::from_cfg(&tilt, (-409.0, -1541.0), 1.1, 800.0, rect);
            let tex = MapTex { id: TextureId::Managed(3), orig_size: [8192, 8192], winter: false };
            let shapes = paint(rect, |p| {
                draw_base(p, &BaseParams { cam: &cam, cal: MapCalibration::DEFAULT, tex, outline: &outline, mirror: true, look: ImageLook::FULL, a: 1.0, far_fade: true });
            });
            let m = meshes(&shapes);
            let top = cam.far_row().max(rect.top());
            for ix in 0..=16 {
                for iy in 0..=16 {
                    let q = pos2(rect.left() + 0.5 + (w - 1.0) * ix as f32 / 16.0, top + 0.5 + (rect.bottom() - top - 1.0) * iy as f32 / 16.0);
                    assert!(covered(&m[0], q).is_some(), "{w}x{h} {angle}: {q:?}");
                }
            }
            if cam.far_row() < rect.top() {
                assert!(m[0].vertices.iter().all(|v| v.color.a() == 255), "{w}x{h} {angle}: no fade with the horizon out of view");
            }
        }
    }

    #[test]
    fn race_lines_follow_the_selection_and_all_mode() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0));
        let cam = flat_cam(rect, (0.0, 0.0), 0.0, 600.0);
        let layers = MapLayers::synthetic();
        let mut cfg = MapLayerConfig::default();
        cfg.race_lines.route = RouteStyle::Line;
        cfg.roads.on = false;
        cfg.pois.on = false;
        let run = |cfg: &MapLayerConfig, sel: &[usize]| {
            let mut st = LayerStats::default();
            let fixed = RaceSel::fixed(sel.to_vec(), false);
            let shapes = paint(rect, |p| {
                let mut c = ctx(p, &cam);
                c.race_sel = &fixed;
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
        let rc = crate::maprender::cfg::RaceCfg::default();
        let picked = sel.update(&layers.races, &rc, (400.0, 0.0), 0.0, true).picked().to_vec();
        assert_eq!(picked, vec![0]);
    }

    /// Layers for the in-race focus: a straight race line at x = 0 (z 0..1000) with a road
    /// beside it (relevant), a road 100 m away and a highway elsewhere (other), a jump line on
    /// each side, plus the synthetic POIs.
    fn focus_layers() -> MapLayers {
        let mut roads = RoadLayer::default();
        let mk = |pts: &[[f32; 2]]| Chain::new(pts.to_vec(), vec![0.0; pts.len()]);
        roads.by_type[RoadType::Road.index() as usize].push(mk(&[[10.0, 100.0], [10.0, 500.0]]));
        roads.by_type[RoadType::Road.index() as usize].push(mk(&[[100.0, 100.0], [100.0, 500.0]]));
        roads.by_type[RoadType::Highway.index() as usize].push(mk(&[[-200.0, 100.0], [-200.0, 500.0]]));
        roads.jumps.push([0.0, 600.0, 0.0, 0.0, 640.0, 0.0]);
        roads.jumps.push([200.0, 600.0, 0.0, 200.0, 640.0, 0.0]);
        MapLayers { roads: Arc::new(roads), races: Arc::new(super::super::data::RaceLayer::new(vec![super::super::racesel::test_line(1, 0.0, false)])), ..MapLayers::synthetic() }
    }

    #[test]
    fn in_race_focus_mutes_or_hides_other_roads_and_pois_only_with_a_selected_line() {
        use super::super::cfg::OtherRoads;
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 600.0));
        let cam = flat_cam(rect, (0.0, 300.0), 0.0, 1500.0);
        let layers = focus_layers();
        let mut cfg = MapLayerConfig::default();
        cfg.race_lines.mode = RaceLineMode::Off; // keep the picture to roads + POIs
        let run = |cfg: &MapLayerConfig, sel: &RaceSel| {
            let mut st = LayerStats::default();
            let shapes = paint(rect, |p| {
                let mut c = ctx(p, &cam);
                c.race_sel = sel;
                st = draw_layers(&c, &layers, cfg);
            });
            (st, shapes)
        };
        let in_race = RaceSel::fixed(vec![0], true);
        let not_racing = RaceSel::fixed(vec![0], false);
        let no_pick = RaceSel::fixed(vec![], true);
        // Normal look: 3 chains with casing + fill, 2 jumps with casing + dashed pieces.
        let (base, base_shapes) = run(&cfg, &not_racing);
        assert_eq!((base.chains, base.muted), (3 + 2, 0));
        assert!(base.pois > 0);
        // Same without a selected line, whatever the setting: nothing is muted on a guess.
        for sel in [&not_racing, &no_pick] {
            let (st, shapes) = run(&cfg, sel);
            assert_eq!((st, shapes.len()), (base, base_shapes.len()));
        }
        // The option Normal changes nothing even in a race; POIs are hidden (default) in all modes.
        cfg.race_lines.focus.hide_pois = false;
        cfg.race_lines.focus.other_roads = OtherRoads::Normal;
        let (st, shapes) = run(&cfg, &in_race);
        assert_eq!((st, shapes.len()), (base, base_shapes.len()));
        // Muted: everything is still there, the 2 other chains + 1 jump are muted: white,
        // alpha 0.25, no casing (one stroke instead of two).
        cfg.race_lines.focus.other_roads = OtherRoads::Muted;
        let (st, shapes) = run(&cfg, &in_race);
        assert_eq!((st.chains, st.muted), (5, 3));
        let strokes: Vec<(f32, Color32)> = shapes
            .iter()
            .filter_map(|s| if let Shape::Path(p) = &s.shape { if let egui::epaint::ColorMode::Solid(c) = p.stroke.color { Some((p.stroke.width, c)) } else { None } } else { None })
            .collect();
        let faint = Rgb::hex(0xffffff).color(0.25);
        assert_eq!(strokes.iter().filter(|(_, c)| *c == faint).count(), 2, "{strokes:?}");
        // Hidden: only the relevant chain and jump are drawn.
        cfg.race_lines.focus.other_roads = OtherRoads::Hidden;
        let (st, shapes) = run(&cfg, &in_race);
        assert_eq!((st.chains, st.muted), (2, 0));
        assert!(shapes.len() < base_shapes.len());
        // POIs: hidden in a race by default, shown again when the option is off or no race.
        cfg.race_lines.focus.hide_pois = true;
        assert_eq!(run(&cfg, &in_race).0.pois, 0);
        assert_eq!(run(&cfg, &not_racing).0.pois, base.pois);
        cfg.race_lines.focus.hide_pois = false;
        assert_eq!(run(&cfg, &in_race).0.pois, base.pois);
        // Roads off: the focus draws nothing extra.
        cfg.roads.on = false;
        assert_eq!(run(&cfg, &in_race).0.chains, 0);
    }

    #[test]
    fn in_race_focus_keeps_the_race_line_and_its_marks() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 600.0));
        let cam = flat_cam(rect, (0.0, 300.0), 0.0, 1500.0);
        let layers = focus_layers();
        let mut cfg = MapLayerConfig::default();
        cfg.race_lines.route = RouteStyle::Line;
        cfg.roads.on = false;
        assert!(cfg.race_lines.focus.hide_pois);
        let sel = RaceSel::fixed(vec![0], true);
        let mut st = LayerStats::default();
        let shapes = paint(rect, |p| {
            let mut c = ctx(p, &cam);
            c.race_sel = &sel;
            st = draw_layers(&c, &layers, &cfg);
        });
        assert_eq!((st.race_lines, st.pois), (1, 0));
        // line + start dot (circle) + finish chequer (1 backing + 9 cells)
        assert_eq!(shapes.len(), 1 + 1 + 10);
    }

    /// D80: `RouteStyle::Road` (the default) draws the race line as a road: its casing under its
    /// fill (the road width rule, x RACE_ROAD_WIDTH), round ends, opaque race colour; over the 3D
    /// scene (D88) the GL scene has all of it (lines and marks), so nothing of it is drawn here.
    #[test]
    fn the_race_line_is_drawn_as_a_road() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 600.0));
        let cam = flat_cam(rect, (0.0, 300.0), 0.0, 1500.0);
        let layers = focus_layers();
        let mut cfg = MapLayerConfig::default();
        assert_eq!(cfg.race_lines.route, RouteStyle::Road);
        let sel = RaceSel::fixed(vec![0], true);
        let run = |cfg: &MapLayerConfig, parts: Parts| {
            let mut st = LayerStats::default();
            let shapes = paint(rect, |p| {
                let mut c = ctx(p, &cam);
                c.race_sel = &sel;
                st = draw_layers_parts(&c, &layers, cfg, parts);
            });
            (st, shapes)
        };
        cfg.roads.on = false;
        let (st, shapes) = run(&cfg, Parts::ALL);
        assert_eq!(st.race_lines, 1);
        // casing + fill, 2 round ends each (casing, fill), start dot, finish chequer (1 + 9)
        assert_eq!(shapes.len(), 2 + 4 + 1 + 10);
        let strokes: Vec<(f32, Color32)> = shapes
            .iter()
            .filter_map(|s| if let Shape::Path(p) = &s.shape { if let egui::epaint::ColorMode::Solid(c) = p.stroke.color { Some((p.stroke.width, c)) } else { None } } else { None })
            .collect();
        let fill = cfg.race_lines.color.color(1.0);
        let casing = cfg.roads.styles.road.casing_color.color(cfg.roads.casing_alpha);
        assert_eq!(strokes.len(), 2, "{strokes:?}");
        assert_eq!((strokes[0].1, strokes[1].1), (casing, fill), "casing first, then the opaque race colour");
        let base = style::road_base_px(&cfg.roads, cam.scale());
        assert!((strokes[1].0 - style::line_px(base, style::RACE_ROAD_WIDTH)).abs() < 1e-3 && strokes[0].0 > strokes[1].0, "{strokes:?}");
        // Over the 3D scene (D88) the race lines and their marks are the GL scene's, whatever the
        // style or the other roads: nothing here.
        let (st, shapes) = run(&cfg, Parts::OVER_3D);
        assert_eq!((st.race_lines, shapes.len()), (0, 0));
        cfg.race_lines.focus.other_roads = OtherRoads::Normal;
        assert_eq!(run(&cfg, Parts::OVER_3D).1.len(), 0);
        cfg.race_lines.route = RouteStyle::Line;
        assert_eq!(run(&cfg, Parts::OVER_3D).1.len(), 0);
    }

    /// D82: "race road only" draws no road of the road layer in a race (2D), only the race road;
    /// outside a race the roads are back; with `RouteStyle::Line` it is `Hidden`.
    #[test]
    fn race_only_draws_no_normal_roads_in_a_race() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 600.0));
        let cam = flat_cam(rect, (0.0, 300.0), 0.0, 1500.0);
        let layers = focus_layers();
        let mut cfg = MapLayerConfig::default();
        cfg.pois.on = false;
        cfg.race_lines.focus.other_roads = OtherRoads::RaceOnly;
        let run = |cfg: &MapLayerConfig, sel: &RaceSel| {
            let mut st = LayerStats::default();
            paint(rect, |p| {
                let mut c = ctx(p, &cam);
                c.race_sel = sel;
                st = draw_layers(&c, &layers, cfg);
            });
            st
        };
        let st = run(&cfg, &RaceSel::fixed(vec![0], true));
        assert_eq!((st.chains, st.race_lines), (0, 1), "no road pieces, the race road");
        assert_eq!(run(&cfg, &RaceSel::fixed(vec![0], false)).chains, 5, "not in a race: every road");
        cfg.race_lines.route = RouteStyle::Line;
        assert_eq!(run(&cfg, &RaceSel::fixed(vec![0], true)).chains, 2, "with the line: Hidden (the corridor's road + jump)");
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
        let cases: [(&str, Vec2, f32, bool); 9] = [
            ("Dashboard 900x600 @ 5000 m", vec2(900.0, 600.0), 5000.0, false),
            ("Dashboard 900x600 @ 5000 m, roads only", vec2(900.0, 600.0), 5000.0, false),
            ("Dashboard 900x600 @ 1500 m, roads only", vec2(900.0, 600.0), 1500.0, false),
            ("Dashboard 900x600 @ 6000 m (whole island)", vec2(900.0, 600.0), 6000.0, false),
            ("Dashboard 600x400 @ 1500 m", vec2(600.0, 400.0), 1500.0, false),
            ("Dashboard 420x420 @ 5000 m", vec2(420.0, 420.0), 5000.0, false),
            ("HUD 208x136 @ 300 m tilted", vec2(208.0, 136.0), 300.0, true),
            ("HUD 208x136 @ 700 m tilted", vec2(208.0, 136.0), 700.0, true),
            ("Dashboard 900x600 @ 5000 m tilted", vec2(900.0, 600.0), 5000.0, true),
        ];
        for (label, size, zoom, tilted) in cases {
            let rect = Rect::from_min_size(Pos2::ZERO, size);
            // The HUD cases run the HUD's defaults (taper on, 32 px game icons), the Dashboard the
            // Dashboard's (a tilted one with the taper too).
            let hud = size.y < 200.0;
            let mut cfg = if hud { MapLayerConfig::hud() } else { MapLayerConfig::default() };
            cfg.tilt.on = tilted;
            if label.ends_with("roads only") {
                cfg.pois.on = false;
                cfg.race_lines.mode = RaceLineMode::Off;
            }
            let cam = Camera::from_cfg(&cfg.tilt, car, 0.6, zoom, rect);
            let atlas = layers.icons.as_ref().map(|i| IconAtlas::from_poi_icons(TextureId::Managed(1), i));
            let (mut t_build, mut t_tess) = (Vec::new(), Vec::new());
            let mut st = LayerStats::default();
            for _ in 0..40 {
                let ectx = egui::Context::default();
                let out = ectx.run(egui::RawInput::default(), |c| {
                    let p = Painter::new(c.clone(), egui::LayerId::background(), rect);
                    let t0 = std::time::Instant::now();
                    let mut c = ctx(&p, &cam);
                    c.icons = atlas.as_ref();
                    st = draw_layers(&c, &layers, &cfg);
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
                draw_base(p, &BaseParams { cam: &cam, cal, tex, outline: &outline, mirror: true, look: ImageLook::FULL, a: 1.0, far_fade: false });
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
            draw_base(p, &BaseParams { cam: &cam, cal, tex, outline: &outline, mirror: false, look, a: 1.0, far_fade: false });
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
            draw_base(p, &BaseParams { cam: &cam, cal, tex, outline: &outline, mirror: false, look: ImageLook::FULL, a: 1.0, far_fade: false });
        });
        assert_eq!(full.iter().filter(|s| matches!(s.shape, Shape::Mesh(_))).count(), 1);
    }
}
