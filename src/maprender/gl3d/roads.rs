//! The roads on the GPU: the K1 [`RoadMesh`] bytes uploaded once per mesh, a per-frame **draw
//! plan** (which tiles, near or far LOD set, tunnels last) and the **style table** that turns
//! the map's road settings into uniforms.
//!
//! Nothing here is per-frame data: vertices, the two index sets and the in-race focus flags
//! live in static buffers; a frame sets ~30 uniforms and issues one `draw_elements` per visible
//! tile (+ one for its tunnels). Style, width rule and height mode are uniforms, so changing
//! them never re-uploads anything.
//!
//! **LOD (design 5.5):** two index sets over the same vertices. The *near* set is every 8 m
//! sample with the deck (8 triangles per segment); the *far* set takes samples >= 32 m apart and
//! the top surface only. A tile uses the far set when the roads on it would be drawn at less
//! than [`FAR_PPM`] px per metre, where 32 m segments are a few px long and the deck is below
//! a pixel. *Why per tile and not per vertex:* one draw per tile, no extra attribute, and the
//! seams between a near and a far tile are as wide as a few px.

use std::sync::Arc;

use egui_glow::glow::{self, HasContext};

use super::as_bytes;
use crate::maprender::cfg::{DashStyle, OtherRoads, RaceFocusCfg, RoadsCfg};
use crate::maprender::mesh3d::{RoadMesh, SLOT_JUMP, VERTEX_STRIDE};
use crate::maprender::racesel::RoadFocus;
use crate::maprender::style;
use crate::maprender::view::Camera;
use crate::gamedata::roadtypes::RoadType;

/// Tiles whose nearest road is drawn at fewer px per metre than this use the far (32 m, top
/// only) index set. 0.1 px/m = a 32 m segment is 3.2 px.
pub const FAR_PPM: f32 = 0.1;

/// Road type slots (`RoadType::index`), 0 = edges without a type.
pub const SLOTS: usize = 10;

// ── GPU buffers ──────────────────────────────────────────────────────────────────────────────

pub struct RoadGpu {
    pub vao: glow::VertexArray,
    vbo: glow::Buffer,
    rel: glow::Buffer,
    pub ibo_near: glow::Buffer,
    pub ibo_far: glow::Buffer,
    /// The CPU mesh (tile table, rev); shared, not copied.
    pub mesh: Arc<RoadMesh>,
    /// Which focus the `rel` buffer holds: `None` = all ones.
    pub rel_focus: Option<Arc<RoadFocus>>,
}

impl RoadGpu {
    /// Upload vertices (28 B each), both index sets and an all-ones focus buffer.
    pub fn upload(gl: &glow::Context, mesh: Arc<RoadMesh>) -> Result<RoadGpu, String> {
        // SAFETY: plain GL buffer creation and uploads on the current context.
        unsafe {
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));
            let vbo = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &mesh.vertices, glow::STATIC_DRAW);
            let stride = VERTEX_STRIDE as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 16);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 1, glow::FLOAT, false, stride, 24);
            gl.enable_vertex_attrib_array(3);
            gl.vertex_attrib_pointer_i32(3, 4, glow::UNSIGNED_BYTE, stride, 12);
            let rel = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(rel));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &vec![1u8; mesh.vertex_count()], glow::DYNAMIC_DRAW);
            gl.enable_vertex_attrib_array(4);
            gl.vertex_attrib_pointer_i32(4, 1, glow::UNSIGNED_BYTE, 1, 0);
            let ibo_near = gl.create_buffer()?;
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo_near));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, as_bytes(&mesh.idx_near), glow::STATIC_DRAW);
            let ibo_far = gl.create_buffer()?;
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo_far));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, as_bytes(&mesh.idx_far), glow::STATIC_DRAW);
            gl.bind_vertex_array(None);
            Ok(RoadGpu { vao, vbo, rel, ibo_near, ibo_far, mesh, rel_focus: None })
        }
    }

    /// Replace the per-vertex focus flags (`RoadMesh::build_rel`), or reset them to all ones.
    pub fn set_rel(&mut self, gl: &glow::Context, flags: Option<&[u8]>, focus: Option<Arc<RoadFocus>>) {
        let ones;
        let data = match flags {
            Some(f) if f.len() == self.mesh.vertex_count() => f,
            _ => {
                ones = vec![1u8; self.mesh.vertex_count()];
                &ones
            }
        };
        // SAFETY: the buffer was created with exactly this size.
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.rel));
            gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, data);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        self.rel_focus = focus;
    }

    pub fn destroy(&mut self, gl: &glow::Context) {
        // SAFETY: deleting objects this struct created.
        unsafe {
            gl.delete_vertex_array(self.vao);
            for b in [self.vbo, self.rel, self.ibo_near, self.ibo_far] {
                gl.delete_buffer(b);
            }
        }
    }
}

// ── the draw plan ────────────────────────────────────────────────────────────────────────────

/// One `draw_elements`: a range of an index set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    pub far: bool,
    pub first: u32,
    pub count: u32,
}

#[derive(Debug, Default)]
pub struct Plan {
    /// Roads of the visible tiles, depth tested.
    pub normal: Vec<Draw>,
    /// Their tunnels, drawn afterwards on top (depth test off).
    pub tunnel: Vec<Draw>,
    pub tiles_near: usize,
    pub tiles_far: usize,
    pub triangles: usize,
}

/// Which tiles to draw, with which LOD set. `max_px` = the widest a road may get in viewport
/// px (for the culling margin), `ppp` = pixels per point.
pub fn plan(mesh: &RoadMesh, cam: &Camera, ppp: f32, max_px: f32) -> Plan {
    let mut out = Plan::default();
    let vp = cam.view_proj(ppp);
    let exag = cam.exag();
    let (car_x, car_z) = (cam.view.car_x, cam.view.car_z);
    // Camera depth of the tile's nearest point to the car, and with it px per metre / size factor.
    let (scale, focal) = (cam.view.scale, cam.focal);
    for t in &mesh.tiles {
        let (nx, nz) = (car_x.clamp(t.bbox[0], t.bbox[2]), car_z.clamp(t.bbox[1], t.bbox[3]));
        let ym = 0.5 * (t.y_range[0] + t.y_range[1]);
        let (ppm, k) = match cam.project3(nx, ym, nz) {
            Some((_, cz)) => (scale * focal / cz, focal / cz),
            None => (f32::MAX, 4.0), // at or behind the eye: the near set
        };
        // Everything on the tile is fainter than the far fade's end: skip it.
        if k < 0.05 && ppm < FAR_PPM {
            continue;
        }
        let pad = 3.0 + max_px / (ppm.max(1e-4) * ppp.max(1e-3)).max(1e-3);
        if !t.in_frustum(&vp, exag, pad.min(2000.0)) {
            continue;
        }
        let far = ppm < FAR_PPM;
        let ranges = if far { t.far } else { t.near };
        for (i, r) in ranges.iter().enumerate() {
            if r.count == 0 {
                continue;
            }
            let d = Draw { far, first: r.first, count: r.count };
            out.triangles += r.count as usize / 3;
            if i == 0 { out.normal.push(d) } else { out.tunnel.push(d) }
        }
        if far { out.tiles_far += 1 } else { out.tiles_near += 1 }
    }
    out
}

// ── style table ──────────────────────────────────────────────────────────────────────────────

/// Everything the road shader takes from the map's settings. Pure data, unit-tested.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleTable {
    /// Per slot: rgb, alpha (0 = off).
    pub a: [[f32; 4]; SLOTS],
    /// Per slot: casing rgb, width factor.
    pub b: [[f32; 4]; SLOTS],
    /// Per slot: dash m, gap m (0 = solid), casing on (1 / 0), draw rank (0 = bottom).
    pub c: [[f32; 4]; SLOTS],
    /// Width rule in viewport px: metres per... see `RoadsCfg`: `[metres, min_px, max_px, casing_px]`.
    pub rw: [f32; 4],
    /// In-race focus: `[mode (0 none, 1 muted, 2 hidden), muted alpha, muted width factor, 0]`.
    pub focus: [f32; 4],
    pub mute_rgb: [f32; 3],
    pub casing_alpha: f32,
}

fn rgb(c: crate::maprender::cfg::Rgb) -> [f32; 3] {
    [c.0[0] as f32 / 255.0, c.0[1] as f32 / 255.0, c.0[2] as f32 / 255.0]
}

/// Build the table for a view whose car-plane scale is `scale` points per metre; `s` = the size
/// factor (HUD design -> screen, 1 on the Dashboard), `ppp` = pixels per point (the widths are
/// in viewport px). `focus` = the in-race look when a race line is picked.
pub fn style_table(roads: &RoadsCfg, focus: Option<&RaceFocusCfg>, scale: f32, s: f32, ppp: f32) -> StyleTable {
    let mut t = StyleTable {
        a: [[0.0; 4]; SLOTS],
        b: [[0.0, 0.0, 0.0, 1.0]; SLOTS],
        c: [[0.0; 4]; SLOTS],
        rw: [0.0; 4],
        focus: [0.0; 4],
        mute_rgb: [1.0; 3],
        casing_alpha: roads.casing_alpha,
    };
    let px = ppp * s;
    t.rw = if roads.scale_with_zoom {
        [roads.metres, roads.min_px * px, roads.max_px.max(roads.min_px) * px, roads.casing_px * px]
    } else {
        // A fixed width: metres 0 makes the clamp return `min`.
        [0.0, roads.base_px * px, roads.base_px * px, roads.casing_px * px]
    };
    // The 2D rule at the car (design px), for the dash length that grows with the line.
    let base_car = style::road_base_px(roads, scale / s) * s;
    let dashes = scale / s >= style::DASH_MIN_PX_PER_M;
    for (rank, &slot) in style::ROAD_DRAW_ORDER.iter().enumerate() {
        let (color, alpha, factor, dash, casing) = if slot == 0 {
            (style::UNSET_COLOR, style::UNSET_ALPHA, style::UNSET_WIDTH, DashStyle::Dashed, None)
        } else {
            let Some(st) = RoadType::from_index(slot as u8).and_then(|ty| roads.styles.get(ty)) else { continue };
            if !st.on {
                continue; // alpha 0 = not drawn
            }
            (st.color, st.alpha, st.width, st.dash, st.casing.then_some(st.casing_color))
        };
        fill(&mut t, slot, rank, color, alpha, factor, dash, casing, base_car, scale, s, dashes);
    }
    if let Some(st) = RoadType::from_index(SLOT_JUMP).and_then(|ty| roads.styles.get(ty)).filter(|st| st.on) {
        fill(&mut t, SLOT_JUMP as usize, style::ROAD_DRAW_ORDER.len(), st.color, st.alpha, st.width, st.dash, st.casing.then_some(st.casing_color), base_car, scale, s, dashes);
        // 2D draws jump lines with their own dash lengths: 4 s on, 3 s off.
        if st.dash != DashStyle::None && dashes {
            t.c[SLOT_JUMP as usize][0] = 4.0 * s / scale.max(1e-6);
            t.c[SLOT_JUMP as usize][1] = 3.0 * s / scale.max(1e-6);
        }
    }
    if let Some(f) = focus {
        t.focus = [
            match f.other_roads {
                OtherRoads::Normal => 0.0,
                OtherRoads::Muted => 1.0,
                // RaceOnly with the race road: the scene skips the road mesh altogether
                // (`Gl3d::render`); without one it is Hidden.
                OtherRoads::Hidden | OtherRoads::RaceOnly => 2.0,
            },
            f.mute_alpha,
            f.mute_width,
            0.0,
        ];
        t.mute_rgb = rgb(f.mute_color);
    }
    t
}

/// The style table of the race road's own mesh (D80, `RoadMesh::race_road`): every slot in the
/// race colour (opaque; its tunnel stretches at the road tunnels' alpha), solid, cased in the
/// `Road` type's casing colour, [`style::RACE_ROAD_WIDTH`] wide under the roads' width rule; no
/// focus muting, one rank.
pub fn race_table(roads: &RoadsCfg, color: crate::maprender::cfg::Rgb, scale: f32, s: f32, ppp: f32) -> StyleTable {
    let mut t = style_table(roads, None, scale, s, ppp);
    let c = rgb(color);
    let casing = rgb(roads.styles.road.casing_color);
    let tunnel_alpha = roads.styles.tunnel.alpha.clamp(0.3, 1.0);
    for slot in 0..SLOTS {
        let alpha = if slot == crate::maprender::mesh3d::SLOT_TUNNEL as usize { tunnel_alpha } else { 1.0 };
        t.a[slot] = [c[0], c[1], c[2], alpha];
        t.b[slot] = [casing[0], casing[1], casing[2], style::RACE_ROAD_WIDTH];
        t.c[slot] = [0.0, 0.0, 1.0, 0.0];
    }
    t
}

#[allow(clippy::too_many_arguments)]
fn fill(t: &mut StyleTable, slot: usize, rank: usize, color: crate::maprender::cfg::Rgb, alpha: f32, factor: f32, dash: DashStyle, casing: Option<crate::maprender::cfg::Rgb>, base_car: f32, scale: f32, s: f32, dashes: bool) {
    let _ = s;
    let c = rgb(color);
    t.a[slot] = [c[0], c[1], c[2], alpha.clamp(0.0, 1.0)];
    let cc = casing.map_or([0.0; 3], rgb);
    t.b[slot] = [cc[0], cc[1], cc[2], factor];
    let (d, g) = match style::dash_pattern(dash, style::line_px(base_car, factor)).filter(|_| dashes) {
        Some((d, g)) => (d / scale.max(1e-6), g / scale.max(1e-6)),
        None => (0.0, 0.0),
    };
    t.c[slot] = [d, g, casing.is_some() as u8 as f32, rank as f32];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maprender::cfg::MapLayerConfig;

    #[test]
    fn style_table_follows_the_road_settings() {
        let mut cfg = RoadsCfg::default();
        cfg.styles.offroad.on = false;
        let t = style_table(&cfg, None, 0.4, 1.0, 1.5);
        let road = RoadType::Road.index() as usize;
        let (hw, off) = (RoadType::Highway.index() as usize, RoadType::Offroad.index() as usize);
        assert_eq!(t.a[road][3], 1.0);
        assert_eq!(t.a[off][3], 0.0, "a type switched off has alpha 0");
        assert!((t.a[road][0] - 0x38 as f32 / 255.0).abs() < 1e-6, "road is 0x38bdf8");
        assert_eq!(t.b[road][3], 1.0, "width factor of 'road'");
        assert_eq!(t.c[road][2], 1.0, "casing on");
        // widths in viewport px: x ppp
        assert_eq!(t.rw, [10.0, 1.5, 15.0, 1.4 * 1.5]);
        // draw order ranks: asphalt on top of the unpaved types
        assert!(t.c[road][3] > t.c[RoadType::Trail.index() as usize][3]);
        assert!(t.c[hw][3] > t.c[road][3]);
        // trail is dashed; with 0.4 pt/m the dashes are (px / scale) m long
        let trail = RoadType::Trail.index() as usize;
        assert!(t.c[trail][0] > 0.0 && t.c[trail][1] > 0.0 && t.c[road][0] == 0.0);
        // jump: 4 / 3 design px, in metres
        let j = SLOT_JUMP as usize;
        assert!((t.c[j][0] - 4.0 / 0.4).abs() < 1e-4 && (t.c[j][1] - 3.0 / 0.4).abs() < 1e-4, "{:?}", t.c[j]);
        // sub-pixel dashes are drawn solid
        let far = style_table(&cfg, None, 0.01, 1.0, 1.0);
        assert_eq!(far.c[trail][0], 0.0);
        // fixed width mode
        let fixed = RoadsCfg { scale_with_zoom: false, base_px: 4.0, ..RoadsCfg::default() };
        let t = style_table(&fixed, None, 0.4, 2.0, 1.0);
        assert_eq!((t.rw[1], t.rw[2]), (8.0, 8.0));
        // the unset slot is the grey dashed one
        assert!((t.a[0][3] - style::UNSET_ALPHA).abs() < 1e-6);
        // turnaround never drawn
        assert_eq!(t.a[9][3], 0.0);
    }

    #[test]
    fn focus_modes_map_to_the_shader_switch() {
        let cfg = MapLayerConfig::default();
        let mut f = cfg.race_lines.focus;
        f.other_roads = OtherRoads::Muted;
        assert_eq!(style_table(&cfg.roads, Some(&f), 0.4, 1.0, 1.0).focus[0], 1.0);
        f.other_roads = OtherRoads::Hidden;
        assert_eq!(style_table(&cfg.roads, Some(&f), 0.4, 1.0, 1.0).focus[0], 2.0);
        f.other_roads = OtherRoads::Normal;
        assert_eq!(style_table(&cfg.roads, Some(&f), 0.4, 1.0, 1.0).focus[0], 0.0);
        assert_eq!(style_table(&cfg.roads, None, 0.4, 1.0, 1.0).focus[0], 0.0);
    }
}
