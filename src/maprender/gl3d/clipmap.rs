//! The terrain: a geometry clipmap displaced in the vertex shader from one `R16UI` height texture.
//!
//! * **Heights** are the K1 [`HeightGrid`] verbatim (`h = q * 0.1 - 10`), 15 MB for the island, read with
//!   `texelFetch` and bilinear-ed by hand: exact, portable (ES 3.0 has no filterable integer
//!   textures), and the same arithmetic as [`HeightGrid::height`], so what the roads and the
//!   markers follow on the CPU is what is drawn.
//! * **Levels** `l = 0..7` are `M` x `M` cells of `8 * 2^l` m around the car, one static
//!   `(M+1)^2` vertex grid reused by all of them (the shader turns grid indices into raster pixels
//!   with `uBase + ij * uStride`). Level 0 is the full grid; the others are rings whose hole is
//!   the next finer level's extent. *Why this and not a quadtree:* the geometry is static, the
//!   per-frame cost is 7 draws with 5 uniforms each, independent of zoom, and the cell size
//!   grows with the distance (~distance / 32), which is what a perspective camera needs.
//! * **Alignment rule** that makes the ring holes a 3 x 3 set: level `l`'s centre is the car's
//!   raster pixel rounded to a multiple of `2^(l+1)`, so the finer level's centre differs by
//!   -1, 0 or +1 cells of level `l` and its extent is exactly `M/2` cells wide: the hole is
//!   `[M/4 + d, 3M/4 + d)` and no gap or overlap appears ([`levels`], tested).
//! * **Cracks:** the odd vertices on a level's outer edge take the mean of their even
//!   neighbours (`uHasCoarser` in the shader), so the edge lies exactly on the coarser level's
//!   linear edge; no skirts.
//! * **Geomorphing:** a level re-centres in whole cells, so without it the ring strip that
//!   switches from the finer to the coarser level jumps in height and shading normal (hills
//!   "pop" while driving). The vertex shader instead blends every vertex of a level towards
//!   the coarser level's surface over its outer cells (`alpha = clamp((d - 24) / 7, 0, 1)`,
//!   `d` = Chebyshev distance from the car in level cells, from `uCarPx`). Where rings meet
//!   `alpha` is exactly 1 (a level's outer boundary is >= 31 cells out), which subsumes the
//!   odd-edge rule above. See `docs/features/minimap.md`.
//!
//! Level 6 spans `64 * 64 = 4096` raster pixels > the 2752 px island, so the island is always
//! fully covered; beyond the raster the heights are clamped to the edge (the sea).

use egui_glow::glow::{self, HasContext};

use super::as_bytes;
use crate::maprender::terrain::HeightGrid;

/// Cells per side of a level.
pub const M: i32 = 64;
/// Number of levels.
pub const LEVELS: usize = 7;
/// Index buffers: 1 full grid (level 0) + 9 ring variants (hole offset -1, 0, 1 cells in x and z).
pub const VARIANTS: usize = 10;

/// Tests only: the world (x, z) the terrain levels are placed for, instead of the camera's car
/// (the pop regression test renders a frame with the previous frame's levels).
#[cfg(test)]
pub static LOD_CAR: std::sync::Mutex<Option<[f64; 2]>> = std::sync::Mutex::new(None);

/// What one level draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelDraw {
    /// Raster pixel (column, row) of grid vertex (0, 0).
    pub base: [i64; 2],
    /// Raster pixels per cell.
    pub stride: i64,
    /// Which index buffer: 0 = full grid, else `1 + (dz + 1) * 3 + (dx + 1)`.
    pub variant: usize,
    /// Is there a coarser level around this one (its outer edge is snapped to it)?
    pub has_coarser: bool,
}

/// The seven level placements for a car at raster position `car_px` (fractional pixels, column /
/// row of the car's world position).
pub fn levels(car_px: [f64; 2]) -> [LevelDraw; LEVELS] {
    let centre = |l: usize| {
        let st = (2i64 << l) as f64;
        [(car_px[0] / st).round() as i64 * st as i64, (car_px[1] / st).round() as i64 * st as i64]
    };
    let mut out = [LevelDraw { base: [0, 0], stride: 1, variant: 0, has_coarser: false }; LEVELS];
    for (l, o) in out.iter_mut().enumerate() {
        let stride = 1i64 << l;
        let c = centre(l);
        let variant = if l == 0 {
            0
        } else {
            let f = centre(l - 1);
            let (dx, dz) = (((f[0] - c[0]) / stride) as i32, ((f[1] - c[1]) / stride) as i32);
            1 + ((dz + 1) * 3 + (dx + 1)) as usize
        };
        *o = LevelDraw { base: [c[0] - (M as i64 / 2) * stride, c[1] - (M as i64 / 2) * stride], stride, variant, has_coarser: l + 1 < LEVELS };
    }
    out
}

/// The hole of ring variant `v` (1..10) in grid cells: `[x0, x1) x [z0, z1)`.
pub fn hole(variant: usize) -> [i32; 4] {
    let v = variant as i32 - 1;
    let (dx, dz) = (v % 3 - 1, v / 3 - 1);
    [M / 4 + dx, 3 * M / 4 + dx, M / 4 + dz, 3 * M / 4 + dz]
}

/// Static GL objects of the clipmap (one grid VBO, ten index buffers, the VAO).
pub struct Clipmap {
    pub vao: glow::VertexArray,
    vbo: glow::Buffer,
    /// `(buffer, index count)` per variant.
    pub ibo: Vec<(glow::Buffer, i32)>,
}

impl Clipmap {
    pub fn new(gl: &glow::Context) -> Result<Clipmap, String> {
        // SAFETY: plain GL object creation and uploads on the current context.
        unsafe {
            let vao = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vao));
            let n = (M + 1) as usize;
            let mut vb: Vec<f32> = Vec::with_capacity(n * n * 2);
            for j in 0..n {
                for i in 0..n {
                    vb.push(i as f32);
                    vb.push(j as f32);
                }
            }
            let vbo = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&vb), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
            let mut ibo = Vec::with_capacity(VARIANTS);
            for variant in 0..VARIANTS {
                let h = if variant == 0 { [-1, -1, -1, -1] } else { hole(variant) };
                let mut idx: Vec<u32> = Vec::new();
                for j in 0..M {
                    for i in 0..M {
                        if variant != 0 && i >= h[0] && i < h[1] && j >= h[2] && j < h[3] {
                            continue;
                        }
                        let a = (j * (M + 1) + i) as u32;
                        let (b, c) = (a + 1, a + (M + 1) as u32);
                        idx.extend_from_slice(&[a, c, b, b, c, c + 1]);
                    }
                }
                let b = gl.create_buffer()?;
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(b));
                gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, as_bytes(&idx), glow::STATIC_DRAW);
                ibo.push((b, idx.len() as i32));
            }
            gl.bind_vertex_array(None);
            Ok(Clipmap { vao, vbo, ibo })
        }
    }

    pub fn destroy(&mut self, gl: &glow::Context) {
        // SAFETY: deleting objects this struct created, on their context.
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            for (b, _) in self.ibo.drain(..) {
                gl.delete_buffer(b);
            }
        }
    }
}

/// The height raster on the GPU.
pub struct HeightTex {
    pub tex: glow::Texture,
    pub size: [i32; 2],
    /// `x0, z1, res` of the grid.
    pub geo: [f32; 3],
    /// The `Terrain::rev` it was made from.
    pub rev: u64,
}

impl HeightTex {
    pub fn upload(gl: &glow::Context, grid: &HeightGrid, rev: u64, max_texture: i32) -> Result<HeightTex, String> {
        if grid.w as i32 > max_texture || grid.h as i32 > max_texture {
            return Err(format!("the {} x {} height raster does not fit the GPU's {max_texture} px textures", grid.w, grid.h));
        }
        // SAFETY: plain GL texture upload on the current context; `q` is `w * h` u16.
        unsafe {
            let tex = gl.create_texture()?;
            let align = gl.get_parameter_i32(glow::UNPACK_ALIGNMENT);
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 2);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::R16UI as i32,
                grid.w as i32,
                grid.h as i32,
                0,
                glow::RED_INTEGER,
                glow::UNSIGNED_SHORT,
                glow::PixelUnpackData::Slice(Some(as_bytes(&grid.q))),
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, align);
            // Integer textures are not filterable: NEAREST only (a mipmap filter would be incomplete).
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            gl.active_texture(glow::TEXTURE0);
            Ok(HeightTex { tex, size: [grid.w as i32, grid.h as i32], geo: [grid.x0 as f32, grid.z1 as f32, grid.res as f32], rev })
        }
    }

    pub fn destroy(&self, gl: &glow::Context) {
        // SAFETY: deleting a texture this struct created.
        unsafe { gl.delete_texture(self.tex) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cells of `l` that are drawn (outside its hole), as raster boxes `[x0, z0, x1, z1)`.
    fn covered(l: &LevelDraw, v: usize) -> Vec<[i64; 4]> {
        let mut out = vec![];
        for j in 0..M as i64 {
            for i in 0..M as i64 {
                if v != 0 {
                    let h = hole(v);
                    if i >= h[0] as i64 && i < h[1] as i64 && j >= h[2] as i64 && j < h[3] as i64 {
                        continue;
                    }
                }
                let (x, z) = (l.base[0] + i * l.stride, l.base[1] + j * l.stride);
                out.push([x, z, x + l.stride, z + l.stride]);
            }
        }
        out
    }

    #[test]
    fn rings_tile_the_plane_without_gaps_or_overlap_for_any_car_position() {
        // Every level's cells, rasterised into a coverage count over the finest level's pixels
        // of a window around the car: each pixel inside the outermost level's extent must be
        // covered exactly once.
        for (cx, cz) in [(1376.0, 1376.0), (1376.4, 1376.4), (1377.9, 1375.1), (1400.5, 1390.49), (3.2, 2745.0), (-50.0, -50.0), (1000.0, 1000.0)] {
            let lv = levels([cx, cz]);
            let win = 512i64; // the inner 4 levels' worth, enough to see every hole variant at work
            let (c0, c1) = ((cx as i64) - win / 2, (cz as i64) - win / 2);
            let mut count = vec![0u8; (win * win) as usize];
            for (l, d) in lv.iter().enumerate() {
                for b in covered(d, d.variant) {
                    for z in b[1].max(c1)..b[3].min(c1 + win) {
                        for x in b[0].max(c0)..b[2].min(c0 + win) {
                            count[((z - c1) * win + (x - c0)) as usize] += 1;
                        }
                    }
                }
                assert_eq!(d.has_coarser, l + 1 < LEVELS);
            }
            assert!(count.iter().all(|&n| n == 1), "car ({cx}, {cz}): gaps or overlaps ({} gaps, {} overlaps)", count.iter().filter(|&&n| n == 0).count(), count.iter().filter(|&&n| n > 1).count());
        }
    }

    #[test]
    fn the_coarsest_level_spans_the_whole_island_raster() {
        let l = levels([1376.0, 1376.0]);
        let top = l[LEVELS - 1];
        let ext = M as i64 * top.stride;
        assert!(ext >= 4096 && top.base[0] <= 0 && top.base[0] + ext >= 2752, "{top:?}");
    }

    #[test]
    fn variants_are_the_nine_hole_offsets() {
        for v in 1..VARIANTS {
            let h = hole(v);
            assert_eq!((h[1] - h[0], h[3] - h[2]), (M / 2, M / 2));
            assert!((-1..=1).contains(&(h[0] - M / 4)) && (-1..=1).contains(&(h[2] - M / 4)));
        }
        // All nine distinct.
        let mut hs: Vec<[i32; 4]> = (1..VARIANTS).map(hole).collect();
        hs.sort();
        hs.dedup();
        assert_eq!(hs.len(), 9);
    }
}
