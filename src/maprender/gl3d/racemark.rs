//! The race lines' start / finish marks in the 3D scene (D88, D102): a flat bar lying **across the
//! race road** at the line's own height, like the paint of a start / finish line, **depth-tested**
//! so terrain and decks hide it like the roads (the 2D marks were egui shapes over the picture
//! and showed through hills).
//!
//! * a sprint's start: a green bar with a dark outline (the 2D green dot's colours);
//! * a circuit's start and a sprint's finish: a chequered bar, two rows of white / dark cells
//!   (the 2D flag).
//!
//! *Why a bar across the road, after posts (#231):* the posts stood upright and read as small
//! things next to the road; the user expects the line a race has on the road ("the finish line is
//! turned by 90 degrees, at least it isn't wide enough"). A flat quad is only a hairline from a
//! very flat camera, but the 3D view is a map view (angle <= 75 deg), and the bar is the same
//! few vertices.
//!
//! The geometry is **world-sized**, so a mark is one quad (6 vertices) of static data
//! ([`vertices`]: the point, the race direction, the climb, the corner), and the vertex shader
//! ([`shaders::MARK_VS`]) works out the size from the road's own width rule, the same as the race
//! road's ribbon (`ROAD_VS`): the road's drawn width at that depth (the fill plus the casing) plus
//! [`OVERHANG_PX`] each side, and a depth along the road of [`BAR_DEPTH_M`], at least
//! [`MIN_DEPTH_PX`] on the screen so it does not vanish when zoomed out. The chequer is procedural
//! in the fragment shader (cells of half the depth, filtered by `fwidth`, so cells under a pixel
//! average to grey instead of shimmering).
//!
//! Built per frame (a few hundred marks in the worst case, `All`: 170 lines; ~60 floats a mark, a
//! microsecond each) and streamed into one buffer; cheaper than keeping a cache coherent.
//!
//! Marks whose line height is 4 m or more under the terrain (a tunnel start) are drawn without
//! the depth test, like the road tunnels, instead of being buried.

use egui_glow::glow::{self, HasContext};

use crate::maprender::mesh3d::{known_y, RACE_TUNNEL_DEPTH_M};
use crate::maprender::racesel::{MarkKind, RaceMark};
use crate::maprender::terrain::Terrain;

/// The bar's depth along the road in metres (two rows of cells of half of it).
pub const BAR_DEPTH_M: f32 = 4.0;
/// ... and at least this many screen px (design px, x the size factor) deep: zoomed out, the
/// bar is wider than 4 m of road is long.
pub const MIN_DEPTH_PX: f32 = 8.0;
/// The bar reaches this much beyond the road's casing on each side (design px).
pub const OVERHANG_PX: f32 = 1.5;
/// Floats per vertex: `aP` (3), `aD` (3), `aI` (4), `aKind` (1).
pub const MARK_FLOATS: usize = 11;

/// The corners of the quad as two triangles: (across, along), each -1 or +1.
const CORNERS: [[f32; 2]; 6] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

/// Is mark `m` in a tunnel (4 m or more under the terrain, like the race road's tunnel stretches)?
pub fn in_tunnel(m: &RaceMark, terrain: &Terrain) -> bool {
    known_y(m.y).is_some_and(|y| terrain.height(m.at[0], m.at[1]) - y >= RACE_TUNNEL_DEPTH_M)
}

/// The quads of the marks in the open (`tunnel` false) or in a tunnel (true), 6 vertices of
/// [`MARK_FLOATS`] each: the point `x, y, z` (the line's height, or the ground's when unknown),
/// the race direction `x, z` and climb, `[height known 1 / 0, inset, corner across, corner along]`,
/// and the kind (0 = sprint start, 1 = chequered).
pub fn vertices(marks: &[RaceMark], terrain: &Terrain, tunnel: bool) -> Vec<f32> {
    let mut v = Vec::new();
    for m in marks.iter().filter(|m| in_tunnel(m, terrain) == tunnel) {
        let known = known_y(m.y);
        let y = known.unwrap_or_else(|| terrain.height(m.at[0], m.at[1]));
        let kind = (m.kind == MarkKind::Chequered) as u8 as f32;
        for c in CORNERS {
            v.extend([m.at[0], y, m.at[1], m.dir[0], m.dir[1], m.slope, known.is_some() as u8 as f32, m.inset, c[0], c[1], kind]);
        }
    }
    v
}

/// The streamed vertex buffer of the marks.
pub struct MarkGpu {
    pub vao: glow::VertexArray,
    pub vbo: glow::Buffer,
}

impl MarkGpu {
    pub fn new(gl: &glow::Context) -> Result<MarkGpu, String> {
        // SAFETY: plain buffer setup on the current context.
        unsafe {
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));
            let vbo = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = (MARK_FLOATS * 4) as i32;
            for (loc, n, off) in [(0u32, 3, 0), (1, 3, 12), (2, 4, 24), (3, 1, 40)] {
                gl.enable_vertex_attrib_array(loc);
                gl.vertex_attrib_pointer_f32(loc, n, glow::FLOAT, false, stride, off);
            }
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            Ok(MarkGpu { vao, vbo })
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

    fn mark(at: [f32; 2], y: f32, kind: MarkKind) -> RaceMark {
        RaceMark { at, y, kind, dir: [1.0, 0.0], slope: 0.1, inset: 0.5 }
    }

    #[test]
    fn marks_are_quads_and_tunnel_marks_are_apart() {
        let t = Terrain::synthetic();
        let g = t.height(0.0, 0.0);
        let marks = [mark([0.0, 0.0], g + 1.0, MarkKind::SprintStart), mark([0.0, 0.0], 0.0, MarkKind::Chequered), mark([0.0, 0.0], g - 10.0, MarkKind::Chequered)];
        let open = vertices(&marks, &t, false);
        let under = vertices(&marks, &t, true);
        assert_eq!(open.len(), 2 * 6 * MARK_FLOATS, "two quads in the open");
        assert_eq!(under.len(), 6 * MARK_FLOATS, "one in the tunnel");
        // the sprint start: the line's height, known, kind 0; the corners are the four of a quad
        let v = &open[..MARK_FLOATS];
        assert_eq!((v[1], v[6], v[10]), (g + 1.0, 1.0, 0.0));
        let corners: std::collections::BTreeSet<(i32, i32)> = open[..6 * MARK_FLOATS].chunks(MARK_FLOATS).map(|v| (v[8] as i32, v[9] as i32)).collect();
        assert_eq!(corners.len(), 4);
        // an unknown height (0) takes the terrain and says so
        let v = &open[6 * MARK_FLOATS..7 * MARK_FLOATS];
        assert_eq!((v[1], v[6], v[10]), (g, 0.0, 1.0));
    }
}
