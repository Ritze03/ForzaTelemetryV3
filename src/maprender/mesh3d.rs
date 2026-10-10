//! The 3D road mesh, built on the CPU (phase K, K1): every road chain resampled to <= 8 m,
//! cut at 1 km tile borders, laid out as GPU-ready vertex and index bytes with two LOD sets,
//! and the per-vertex flags of the in-race focus. Pure CPU, no GL: the renderer (K2) uploads
//! [`RoadMesh::vertices`] and the two index sets once per rebuild and draws tiles.
//!
//! # What a road is in 3D
//!
//! A **ribbon with a deck**: per *sample* (a cross-section of the road) four GPU vertices
//! `{right, left} x {top, bottom}`. The vertex shader offsets left/right by the half-width along
//! the sample's tangent normal (the width rule needs the camera, so it lives in the shader, per
//! vertex depth) and drops the bottom vertices by `deck_m / exag`, so the deck thickness is a live
//! slider with no rebuild. A segment between two samples is 8 triangles: top 2, the two walls 4,
//! the underside 2 ([`RoadMesh::idx_near`]); the far set has the top only.
//!
//! # Height rules (D51), identical to the shader and the editor's 3D page
//!
//! [`road_y`]: nodes by default (`y_node` linear between the chain vertices' own heights), or
//! the terrain drape (a switch, one uniform, no rebuild); **cross-country is always draped**
//! (user points sit at terrain height only every few tens of metres, so node mode cut up to 4.8 m
//! through hills); **jump lines are a taut string** ([`taut_string`]) over the terrain between
//! their two end points, computed here into `y_node` (2 m samples) so every mode shows the same
//! line. Every height gets the same small lift ([`LIFT_M`]) so the ribbon does not z-fight the
//! ground. *Why node heights by default:* 67 % of the highway vertices are > 5 m above the terrain
//! (elevated expressways) and a drape would bury them; the user reversed the earlier "always
//! drape" decision after seeing it (`docs/game-data/fh6-map-tooling.md`, "3D preview").
//!
//! # Layout
//!
//! * **Samples** ([`RoadMesh::samples`]) are stored piece after piece; a [`Piece`] is one chain
//!   piece inside one 1 km tile. Pieces are sorted by tile, tunnels last, so each [`Tile`] owns a
//!   contiguous range of the index buffers ([`Tile::near`] / [`Tile::far`], `[normal, tunnel]`):
//!   one `draw_elements` per visible tile (+ one for its tunnels, which are drawn on top with the
//!   depth test off, like the page does).
//! * **LOD:** the *near* set is every sample with the deck; the *far* set takes samples >= 32 m
//!   apart (jump lines all) and the top surface only: 585 k -> ~100 k triangles for the HUD's
//!   stopped zoom. The renderer chooses per tile ([`Tile::dist_to`]); both index sets reference the
//!   same vertices.
//! * **Tangents** are the bisector of the two segments at a sample, scaled by the mitre factor
//!   (D81, [`Sample::tx`]): the ribbon keeps its width through a bend, and two pieces of a chain
//!   (a tile border) meet in the same vertices.
//! * **Chain ends (D81)** follow [`RoadLayer::joins`]: at an [`End::Join`] (exactly one other
//!   piece goes on: a type change, an L-corner of two chains) the end takes the bisector with the
//!   next piece's first segment, so both ends sit on the same two vertices, a mitre with no notch
//!   or overlap; at a dead end or a junction ([`End::Junction`]) a **round cap** closes the end:
//!   [`CAP_RIM`] rim samples after the piece's own, a fan round a centre vertex, the rim wall and
//!   the underside in the near set. At a junction the arms' caps and the through road overlap at
//!   the node's one height; the renderer's two passes (every casing, then every fill,
//!   `gl3d::shaders::ROAD_VS`) make that a clean junction patch. Jump lines stay open.
//! * **`s`** is the distance along the whole chain (dashes run on across tile borders).
//! * **In-race focus (D66):** [`RoadMesh::build_rel`] makes one byte per GPU vertex, 1 for the
//!   roads along the picked race line and 0 for the others, from the same [`RoadFocus`] the 2D
//!   renderer uses. A sample is tagged with the segment it *starts*, so the flag flips half a
//!   quad (<= 4 m) away from the real boundary once the GPU interpolates it; fine at map scale.
//!
//! Sizes on the real data (8 m steps): 135 911 samples (121 267 + 2 092 caps of 7), 543 644
//! vertices = 15.2 MB, 1.02 M near triangles = 12.2 MB of indices, 2 440 pieces over 177 tiles.


use std::collections::HashMap;

use super::cfg::RoadHeight;
use super::data::{Chain, End, RoadLayer, N_TYPES};
use super::racesel::{RaceRoad, RoadFocus};
use super::terrain::Terrain;

/// Tile edge, m.
pub const TILE_M: f32 = 1000.0;
/// Resample step of the near set, m.
pub const STEP_NEAR_M: f32 = 8.0;
/// Sample spacing of the far set, m (jump lines keep all their samples).
pub const STEP_FAR_M: f32 = 32.0;
/// Resample step of jump lines, m (the taut string needs the terrain between the ends).
pub const STEP_JUMP_M: f32 = 2.0;
/// Metres every road is lifted above its height (z-fighting with the terrain).
pub const LIFT_M: f32 = 0.6;
/// Coincident chain vertices closer than this are one point.
const MIN_SEG_M: f32 = 0.05;
/// Cap of the mitre factor (`1 / cos` of half the turn) at a corner (D81): 2 = a full mitre up to
/// a 120 degree turn; a sharper corner narrows a little instead of growing a spike.
pub const MITER_MAX: f32 = 2.0;
/// Triangles of a round cap (half a disc) at a dead end or a junction (D81).
pub const CAP_SEGS: usize = 8;
/// Samples a round cap adds: its rim between the end sample's two top vertices.
pub const CAP_RIM: usize = CAP_SEGS - 1;

/// Road type slots (`RoadType::index`), see `data::N_TYPES`.
pub const SLOT_CROSSCOUNTRY: u8 = 5;
pub const SLOT_TUNNEL: u8 = 6;
pub const SLOT_JUMP: u8 = 7;
pub const SLOT_TURNAROUND: u8 = 9;

/// GPU vertices per sample: `bot * 2 + (side > 0)`: 0 = top/right, 1 = top/left, 2 = bottom/right,
/// 3 = bottom/left (`side` +1 offsets by `(-tz, tx) * half_width`).
pub const VERTS_PER_SAMPLE: usize = 4;
/// Bytes per GPU vertex: `x, z, y_node` f32 (offset 0), `side` i8 / `bot` u8 / `slot` u8 / cap
/// centre u8 (12, see [`push_vertices`]),
/// `tx, tz` f32 (16), `s` f32 (24).
pub const VERTEX_STRIDE: usize = 28;
#[allow(dead_code)] // layout documentation; the renderer binds the same offsets (`gl3d::roads`)
pub const OFFSET_POS: usize = 0;
#[allow(dead_code)]
pub const OFFSET_FLAGS: usize = 12;
#[allow(dead_code)]
pub const OFFSET_TAN: usize = 16;
#[allow(dead_code)]
pub const OFFSET_S: usize = 24;

// ── height rules ─────────────────────────────────────────────────────────────────────────────

/// The y (m) of a road point under D51's rules, as the vertex shader evaluates them (lift
/// included): cross-country is always the terrain, jump lines always `y_node` (their taut
/// string), everything else `y_node` ([`RoadHeight::Nodes`]) or the terrain ([`RoadHeight::Terrain`]).
#[allow(dead_code)] // the CPU mirror of the shader's rule (tests pin the two together)
pub fn road_y(slot: u8, y_node: f32, y_terrain: f32, mode: RoadHeight) -> f32 {
    let base = match (slot, mode) {
        (SLOT_CROSSCOUNTRY, _) => y_terrain,
        (SLOT_JUMP, _) => y_node,
        (_, RoadHeight::Nodes) => y_node,
        (_, RoadHeight::Terrain) => y_terrain,
    };
    base + LIFT_M
}

/// A jump line's height along its samples: the **upper convex hull** of the two end heights and
/// every sample's terrain height ("a string wrapped around the terrain of its two connecting
/// points", the user's words): straight where it clears the ground, resting on the terrain where
/// the ground rises above the chord (a cliff edge before the drop). `s` = distance along the line
/// (increasing), `y_terrain` = terrain under each sample, `y0` / `y1` = the end points' own
/// heights (raised to the terrain if a point sits underground). Port of `tautString` in
/// `assets/editor/preview-3d.html`.
pub fn taut_string(s: &[f32], y_terrain: &[f32], y0: f32, y1: f32) -> Vec<f32> {
    let n = s.len();
    assert_eq!(n, y_terrain.len());
    if n == 0 {
        return Vec::new();
    }
    let mut ys = y_terrain.to_vec();
    ys[0] = if y0.is_finite() { y0.max(ys[0]) } else { ys[0] };
    ys[n - 1] = if y1.is_finite() { y1.max(ys[n - 1]) } else { ys[n - 1] };
    // Monotone chain, upper hull over samples sorted by s.
    let mut hull: Vec<usize> = Vec::new();
    for k in 0..n {
        while hull.len() >= 2 {
            let (p, q) = (hull[hull.len() - 2], hull[hull.len() - 1]);
            // q on or below the line p-k: not on the upper hull.
            if (s[q] - s[p]) * (ys[k] - ys[p]) - (ys[q] - ys[p]) * (s[k] - s[p]) >= 0.0 {
                hull.pop();
            } else {
                break;
            }
        }
        hull.push(k);
    }
    let mut out = ys.clone();
    for w in hull.windows(2) {
        let (p, q) = (w[0], w[1]);
        for k in p..=q {
            out[k] = if s[q] > s[p] { ys[p] + (ys[q] - ys[p]) * (s[k] - s[p]) / (s[q] - s[p]) } else { ys[k] };
        }
    }
    out
}

// ── the mesh ─────────────────────────────────────────────────────────────────────────────────

/// One road cross-section.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub x: f32,
    pub z: f32,
    /// Node height (m, no lift): linear between the chain vertices; the terrain for cross-country;
    /// the taut string for jump lines.
    pub y_node: f32,
    /// Tangent in (x, z): unit length times the mitre factor (D81). At a corner it is the bisector
    /// of the two segments, `1 / cos(turn / 2)` long (at most [`MITER_MAX`]), so the shader's
    /// offset `(-tz, tx) * half_width` keeps the ribbon its full width through the bend; at a
    /// [`End::Join`] both chains take the same bisector, so their ends share the edge. A cap
    /// sample's tangent is unit and turned so its left vertex lies on the cap's rim.
    pub tx: f32,
    pub tz: f32,
    /// Distance along the whole chain, m.
    pub s: f32,
    pub slot: u8,
}

/// Where a sample came from, for [`RoadMesh::build_rel`]: the chain (index into
/// `RoadLayer::by_type[slot]`, or into `RoadLayer::jumps` for the jump slot) and the segment it
/// starts (the last sample of a piece belongs to the segment before it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SampleSrc {
    pub chain: u32,
    pub seg: u32,
    /// A rim sample of a round cap (it takes the provenance of the end sample it caps).
    pub cap: bool,
}

/// One chain piece inside one tile: samples `first .. first + count` (>= 2), then the rim samples
/// of its round caps ([`CAP_RIM`] each, the start cap first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub first: u32,
    pub count: u32,
    pub slot: u8,
    /// Index into [`RoadMesh::tiles`].
    pub tile: u32,
    /// Round caps (D81): bit 0 at the first sample (the chain's start), bit 1 at the last.
    pub caps: u8,
}

/// How a chain end is closed in the mesh (from [`End`]).
#[derive(Clone, Copy, Debug, PartialEq)]
enum EndGeo {
    /// Left open (jump lines).
    Open,
    /// A round cap (dead end, junction).
    Cap,
    /// Mitred onto the next piece, whose neighbouring vertex this is.
    Mitre([f32; 2]),
}

impl From<End> for EndGeo {
    fn from(e: End) -> EndGeo {
        match e {
            End::Join { next, .. } => EndGeo::Mitre(next),
            End::Dead | End::Junction => EndGeo::Cap,
        }
    }
}

/// A range of an index buffer, in indices (not triangles).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IdxRange {
    pub first: u32,
    pub count: u32,
}

/// A 1 km tile of road pieces: its draw ranges and its extent.
#[derive(Clone, Debug)]
pub struct Tile {
    /// `(floor(x / TILE_M), floor(z / TILE_M))`.
    pub key: (i32, i32),
    /// `[min_x, min_z, max_x, max_z]` of the samples (not widened by the road width).
    pub bbox: [f32; 4],
    /// `[lowest, highest]` of the samples' node heights and terrain heights, m (without lift,
    /// deck or width): the vertical extent for culling in either height mode.
    pub y_range: [f32; 2],
    /// Near set (8 m, with deck): `[normal types, tunnels]`.
    pub near: [IdxRange; 2],
    /// Far set (>= 32 m, top only): `[normal types, tunnels]`.
    pub far: [IdxRange; 2],
}

impl Tile {
    /// Distance from (x, z) to the tile's box (0 inside), for choosing the LOD set.
    #[allow(dead_code)] // the renderer's LOD choice needs the nearest point, not just the distance (`gl3d::roads::plan`)
    pub fn dist_to(&self, x: f32, z: f32) -> f32 {
        let dx = (self.bbox[0] - x).max(x - self.bbox[2]).max(0.0);
        let dz = (self.bbox[1] - z).max(z - self.bbox[3]).max(0.0);
        dx.hypot(dz)
    }

    /// Conservative frustum test of the tile's box (widened by `pad_m` on every side, `y_range`
    /// scaled by `exag`) against `view_proj` — the folded matrix of [`super::view::Camera::view_proj`]
    /// (input `(x, y * exag, z, 1)`): false only when all 8 corners are outside one clip plane.
    pub fn in_frustum(&self, view_proj: &[f32; 16], exag: f32, pad_m: f32) -> bool {
        let (y0, y1) = ((self.y_range[0] - pad_m) * exag, (self.y_range[1] + pad_m) * exag);
        let mut outside = [true; 6];
        for &x in &[self.bbox[0] - pad_m, self.bbox[2] + pad_m] {
            for &z in &[self.bbox[1] - pad_m, self.bbox[3] + pad_m] {
                for &y in &[y0, y1] {
                    let v = [x, y, z, 1.0];
                    let mut c = [0.0f32; 4];
                    for (r, o) in c.iter_mut().enumerate() {
                        *o = (0..4).map(|k| view_proj[k * 4 + r] * v[k]).sum();
                    }
                    let w = c[3];
                    outside[0] &= c[0] < -w;
                    outside[1] &= c[0] > w;
                    outside[2] &= c[1] < -w;
                    outside[3] &= c[1] > w;
                    outside[4] &= c[2] < -w;
                    outside[5] &= c[2] > w;
                }
            }
        }
        !outside.iter().any(|&o| o)
    }
}

/// Everything the GL renderer needs of the roads, CPU side. Shared as `Arc<RoadMesh>`.
#[derive(Debug, Default)]
pub struct RoadMesh {
    /// `MapLayers::rev` it was built from (rebuild when it changes).
    pub rev: u64,
    /// `Terrain::rev` it was built with (heights of the cross-country and jump samples).
    pub terrain_rev: u64,
    pub samples: Vec<Sample>,
    /// Parallel to `samples`.
    pub src: Vec<SampleSrc>,
    pub pieces: Vec<Piece>,
    pub tiles: Vec<Tile>,
    /// `VERTS_PER_SAMPLE` vertices per sample, [`VERTEX_STRIDE`] bytes each, little endian.
    pub vertices: Vec<u8>,
    /// Near index set (u32): 8 triangles per segment.
    pub idx_near: Vec<u32>,
    /// Far index set (u32): top surface only, >= 32 m apart.
    pub idx_far: Vec<u32>,
}

/// The slot the race road's open stretches use in its own mesh (the `Road` slot; the renderer
/// gives every slot of the race mesh the race look).
pub const SLOT_RACE: u8 = 1;
/// A race road point this far (m) or more under the terrain is in a tunnel.
pub const RACE_TUNNEL_DEPTH_M: f32 = 4.0;

/// The race road as a road layer (D80, [`RoadMesh::race_road`]): one chain per run of open /
/// tunnel segments, consecutive runs sharing their boundary vertex (a mitred join); a closed
/// circuit ends on its first point, so its ends join too.
pub fn race_road_layer(r: &RaceRoad, terrain: &Terrain) -> RoadLayer {
    let mut l = RoadLayer::default();
    let (mut pts, mut ys) = (r.pts.clone(), r.y.clone());
    ys.resize(pts.len(), 0.0);
    if r.closed && pts.len() >= 3 {
        if pts[0] != pts[pts.len() - 1] {
            pts.push(pts[0]);
            ys.push(ys[0]);
        } else {
            let n = pts.len();
            ys[n - 1] = ys[0];
        }
    }
    let n = pts.len();
    if n < 2 {
        return l;
    }
    let under: Vec<bool> = (0..n).map(|i| known_y(ys[i]).is_some_and(|y| terrain.height(pts[i][0], pts[i][1]) - y >= RACE_TUNNEL_DEPTH_M)).collect();
    let tunnel = |s: usize| under[s] && under[s + 1];
    let mut start = 0usize;
    for s in 1..=n - 1 {
        if s == n - 1 || tunnel(s) != tunnel(start) {
            let slot = if tunnel(start) { SLOT_TUNNEL } else { SLOT_RACE } as usize;
            l.by_type[slot].push(Chain::new(pts[start..=s].to_vec(), ys[start..=s].to_vec()));
            start = s;
        }
    }
    l
}

/// A dense chain point before it is split into pieces.
#[derive(Clone, Copy)]
struct Dense {
    x: f32,
    z: f32,
    y: f32,
    seg: u32,
}

/// A nav node's height if it has one. `data::build_roads` gives nav **orphans** (nodes that are
/// on no polyline, linked in only by the road-type file's `added` links) `y = 0.0` for "no height";
/// that is 100 m below the sea, so taken literally the road would dive into the ground at every
/// such node. A height of exactly 0 (or a non-finite one) counts as unknown and falls back to the
/// terrain (K2; no real node sits at y = 0, the island is -1.8..1473 m with the sea at 100).
pub fn known_y(y: f32) -> Option<f32> {
    (y.is_finite() && y.abs() > 1e-3).then_some(y)
}

/// Resample a chain to <= `step` m: every chain vertex is kept, with equal sub-steps between
/// them. Node heights are linear between vertices (unknown ones, [`known_y`], fall back to the terrain);
/// `drape` makes every sample take the terrain height (cross-country).
fn densify(pts: &[[f32; 2]], ys: &[f32], step: f32, drape: bool, terrain: &Terrain) -> Vec<Dense> {
    let mut out: Vec<Dense> = Vec::with_capacity(pts.len() * 2);
    let node_y = |i: usize, x: f32, z: f32| {
        let y = ys.get(i).copied().unwrap_or(f32::NAN);
        if drape { terrain.height(x, z) } else { known_y(y).unwrap_or_else(|| terrain.height(x, z)) }
    };
    let mut last_seg = 0u32;
    for i in 0..pts.len().saturating_sub(1) {
        let (a, b) = (pts[i], pts[i + 1]);
        let l = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if l < MIN_SEG_M || !l.is_finite() {
            continue;
        }
        let (ya, yb) = (node_y(i, a[0], a[1]), node_y(i + 1, b[0], b[1]));
        let n = ((l / step).ceil() as usize).max(1);
        for k in 0..n {
            let f = k as f32 / n as f32;
            let (x, z) = (a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f);
            let y = if drape { terrain.height(x, z) } else { ya + (yb - ya) * f };
            out.push(Dense { x, z, y, seg: i as u32 });
        }
        last_seg = i as u32;
    }
    if let (Some(last), Some(&p)) = (out.last(), pts.last()) {
        // The chain's last vertex (unless the last segment was collapsed away and it is the same point).
        if (last.x - p[0]).hypot(last.z - p[1]) >= MIN_SEG_M {
            let y = node_y(pts.len() - 1, p[0], p[1]);
            out.push(Dense { x: p[0], z: p[1], y, seg: last_seg });
        }
    }
    out
}

fn tile_key(x: f32, z: f32) -> (i32, i32) {
    ((x / TILE_M).floor() as i32, (z / TILE_M).floor() as i32)
}

/// One piece under construction: its samples, then its caps' rim samples (`caps` as in [`Piece`]).
struct PieceBuild {
    key: (i32, i32),
    slot: u8,
    samples: Vec<Sample>,
    src: Vec<SampleSrc>,
    caps: u8,
}

fn unit(dx: f32, dz: f32) -> Option<[f32; 2]> {
    let l = dx.hypot(dz);
    (l > 1e-6 && l.is_finite()).then(|| [dx / l, dz / l])
}

/// `v` turned by `a` radians (counter-clockwise in a right-handed x/z reading: +x towards +z, so
/// the "left" normal `(-tz, tx)` of a tangent is the tangent turned by +90 degrees).
fn turn(v: [f32; 2], a: f32) -> [f32; 2] {
    let (s, c) = a.sin_cos();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c]
}

/// The tangent at a point with the incoming direction `din` and the outgoing `dout` (unit, either
/// may be missing at an open end): the bisector, scaled by the mitre factor (D81).
fn mitre(din: Option<[f32; 2]>, dout: Option<[f32; 2]>) -> [f32; 2] {
    match (din, dout) {
        (Some(a), Some(b)) => match unit(a[0] + b[0], a[1] + b[1]) {
            Some(t) => {
                let c = (t[0] * b[0] + t[1] * b[1]).max(1.0 / MITER_MAX);
                [t[0] / c, t[1] / c]
            }
            None => b, // a full reversal: no bisector
        },
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => [1.0, 0.0],
    }
}

/// The [`CAP_RIM`] rim samples of a round cap on `end` (`start`: the chain's first sample, so the
/// cap points backwards). Rim direction `j` = the outward direction turned by
/// `90 - 180 j / CAP_SEGS` degrees: from the end sample's vertex on the left of the outward
/// direction round to the one on its right. A rim sample's tangent is set so its *left* vertex
/// (side +1) lies on the rim; its right vertices are unused.
fn cap_rim(end: &Sample, start: bool) -> impl Iterator<Item = Sample> + '_ {
    let t = unit(end.tx, end.tz).unwrap_or([1.0, 0.0]);
    let f = if start { [-t[0], -t[1]] } else { t };
    (1..CAP_SEGS).map(move |j| {
        let dir = turn(f, std::f32::consts::FRAC_PI_2 - std::f32::consts::PI * j as f32 / CAP_SEGS as f32);
        let tan = turn(dir, -std::f32::consts::FRAC_PI_2);
        Sample { tx: tan[0], tz: tan[1], ..*end }
    })
}

/// Samples of a dense chain with tangents and distance along, cut into one piece per tile (a piece
/// ends with the first point of the next tile, which also starts the next piece). `ends` closes the
/// chain's two ends: mitred onto the next piece, a round cap, or open.
fn pieces_of(dense: &[Dense], slot: u8, chain: u32, ends: [EndGeo; 2], out: &mut Vec<PieceBuild>) {
    let n = dense.len();
    if n < 2 {
        return;
    }
    let mut s = vec![0.0f32; n];
    for i in 1..n {
        s[i] = s[i - 1] + (dense[i].x - dense[i - 1].x).hypot(dense[i].z - dense[i - 1].z);
    }
    let sample = |i: usize| {
        let p = dense[i];
        let din = if i > 0 {
            unit(p.x - dense[i - 1].x, p.z - dense[i - 1].z)
        } else if let EndGeo::Mitre(q) = ends[0] {
            unit(p.x - q[0], p.z - q[1])
        } else {
            None
        };
        let dout = if i + 1 < n {
            unit(dense[i + 1].x - p.x, dense[i + 1].z - p.z)
        } else if let EndGeo::Mitre(q) = ends[1] {
            unit(q[0] - p.x, q[1] - p.z)
        } else {
            None
        };
        let [tx, tz] = mitre(din, dout);
        Sample { x: p.x, z: p.z, y_node: p.y, tx, tz, s: s[i], slot }
    };
    let mut start = 0usize;
    let mut cur = tile_key(dense[0].x, dense[0].z);
    let mut flush = |a: usize, b: usize, key: (i32, i32)| {
        if b <= a {
            return;
        }
        let mut samples: Vec<Sample> = (a..=b).map(sample).collect();
        // The last sample of a piece (the next tile's first point) belongs to the segment before it.
        let mut src: Vec<SampleSrc> = (a..=b).map(|i| SampleSrc { chain, seg: dense[if i == b && b > a { b - 1 } else { i }].seg, cap: false }).collect();
        let mut caps = 0u8;
        for (bit, at, k) in [(1u8, a == 0 && ends[0] == EndGeo::Cap, 0usize), (2, b == n - 1 && ends[1] == EndGeo::Cap, b - a)] {
            if at {
                let (end, end_src) = (samples[k], src[k]);
                let rim: Vec<Sample> = cap_rim(&end, bit == 1).collect();
                src.extend(std::iter::repeat_n(SampleSrc { cap: true, ..end_src }, rim.len()));
                samples.extend(rim);
                caps |= bit;
            }
        }
        out.push(PieceBuild { key, slot, samples, src, caps });
    };
    for i in 1..n {
        let t = tile_key(dense[i].x, dense[i].z);
        if t != cur {
            flush(start, i, cur);
            start = i;
            cur = t;
        }
    }
    flush(start, n - 1, cur);
}

/// Indices of the samples the far set keeps (first, then >= `step` m after the last kept, last).
fn far_subset(samples: &[Sample], step: f32) -> Vec<usize> {
    let n = samples.len();
    if step <= 0.0 {
        return (0..n).collect();
    }
    let mut keep = vec![0usize];
    let mut last_s = samples[0].s;
    for (i, sm) in samples.iter().enumerate().take(n - 1).skip(1) {
        if sm.s - last_s >= step {
            keep.push(i);
            last_s = sm.s;
        }
    }
    keep.push(n - 1);
    keep
}

impl RoadMesh {
    /// Build the mesh of `roads` over `terrain`. Turnarounds are never drawn (D52); the jump slot's
    /// chains are the `jumps` list. ~15 ms release on the real island (30 ms debug).
    pub fn build(roads: &RoadLayer, terrain: &Terrain, rev: u64) -> RoadMesh {
        let mut builds: Vec<PieceBuild> = Vec::new();
        let joins = roads.joins();
        for slot in 0..N_TYPES {
            let sl = slot as u8;
            if sl == SLOT_TURNAROUND || sl == SLOT_JUMP {
                continue;
            }
            for (ci, ch) in roads.by_type[slot].iter().enumerate() {
                let dense = densify(&ch.pts, &ch.y, STEP_NEAR_M, sl == SLOT_CROSSCOUNTRY, terrain);
                pieces_of(&dense, sl, ci as u32, joins.get(slot, ci).map(EndGeo::from), &mut builds);
            }
        }
        for (ji, j) in roads.jumps.iter().enumerate() {
            let dense = jump_chain(j, terrain);
            pieces_of(&dense, SLOT_JUMP, ji as u32, [EndGeo::Open; 2], &mut builds);
        }
        // Tile order, tunnels last within a tile (a stable sort keeps the type order otherwise).
        builds.sort_by_key(|b| (b.key, b.slot == SLOT_TUNNEL));

        let mut mesh = RoadMesh { rev, terrain_rev: terrain.rev, ..Default::default() };
        let total: usize = builds.iter().map(|b| b.samples.len()).sum();
        mesh.samples.reserve(total);
        mesh.src.reserve(total);
        mesh.vertices.reserve(total * VERTS_PER_SAMPLE * VERTEX_STRIDE);
        for b in &builds {
            let tile = match mesh.tiles.last() {
                Some(t) if t.key == b.key => mesh.tiles.len() as u32 - 1,
                _ => {
                    mesh.tiles.push(Tile { key: b.key, bbox: [f32::MAX, f32::MAX, f32::MIN, f32::MIN], y_range: [f32::MAX, f32::MIN], near: Default::default(), far: Default::default() });
                    mesh.tiles.len() as u32 - 1
                }
            };
            let t = &mut mesh.tiles[tile as usize];
            for sm in &b.samples {
                t.bbox = [t.bbox[0].min(sm.x), t.bbox[1].min(sm.z), t.bbox[2].max(sm.x), t.bbox[3].max(sm.z)];
                let yt = terrain.height(sm.x, sm.z);
                t.y_range = [t.y_range[0].min(sm.y_node.min(yt)), t.y_range[1].max(sm.y_node.max(yt))];
            }
            let count = (b.samples.len() - b.caps.count_ones() as usize * CAP_RIM) as u32;
            mesh.pieces.push(Piece { first: mesh.samples.len() as u32, count, slot: b.slot, tile, caps: b.caps });
            mesh.samples.extend_from_slice(&b.samples);
            mesh.src.extend_from_slice(&b.src);
        }
        for (sm, src) in mesh.samples.iter().zip(&mesh.src) {
            push_vertices(&mut mesh.vertices, sm, src.cap);
        }
        mesh.build_indices();
        mesh
    }

    /// The race line as a road of its own (D80): the same ribbon, deck, mitred joins and round
    /// caps as the roads, along the line's own points and heights. Stretches more than
    /// [`RACE_TUNNEL_DEPTH_M`] under the terrain go into the tunnel slot, so the renderer draws
    /// them like the road tunnels (last, without the depth test: seen through the hill); the rest
    /// is slot [`SLOT_RACE`]. A closed circuit closes on itself (one mitred join, no ends).
    /// A few hundred samples per km; the 85 km route 5555 builds in a few ms.
    #[cfg(test)]
    pub fn race_road(r: &RaceRoad, terrain: &Terrain) -> RoadMesh {
        RoadMesh::race_roads(std::slice::from_ref(r), terrain)
    }

    /// Several race lines as one mesh (D88: every line the race-line mode draws, up to all 170 =
    /// ~1 000 km, is one mesh, one upload, one draw plan).
    pub fn race_roads(lines: &[RaceRoad], terrain: &Terrain) -> RoadMesh {
        let mut layer = RoadLayer::default();
        for r in lines {
            let l = race_road_layer(r, terrain);
            for (slot, chains) in l.by_type.into_iter().enumerate() {
                layer.by_type[slot].extend(chains);
            }
        }
        RoadMesh::build(&layer, terrain, 0)
    }

    /// Fill `idx_near` / `idx_far` and the tiles' ranges from the pieces.
    fn build_indices(&mut self) {
        let mut piece_i = 0usize;
        for ti in 0..self.tiles.len() {
            let first = piece_i;
            while piece_i < self.pieces.len() && self.pieces[piece_i].tile as usize == ti {
                piece_i += 1;
            }
            for tunnel in [false, true] {
                let (n0, f0) = (self.idx_near.len() as u32, self.idx_far.len() as u32);
                for p in &self.pieces[first..piece_i] {
                    if (p.slot == SLOT_TUNNEL) != tunnel {
                        continue;
                    }
                    let base = p.first as usize;
                    let samples = &self.samples[base..base + p.count as usize];
                    for i in 0..samples.len() - 1 {
                        near_quad(&mut self.idx_near, (base + i) as u32, (base + i + 1) as u32);
                    }
                    let keep = far_subset(samples, if p.slot == SLOT_JUMP { 0.0 } else { STEP_FAR_M });
                    for w in keep.windows(2) {
                        top_quad(&mut self.idx_far, (base + w[0]) as u32, (base + w[1]) as u32);
                    }
                    let mut rim = (base + p.count as usize) as u32;
                    for (bit, end) in [(1u8, base), (2, base + p.count as usize - 1)] {
                        if p.caps & bit != 0 {
                            cap_tris(&self.samples, &self.src, end as u32, rim, bit == 1, &mut self.idx_near, &mut self.idx_far);
                            rim += CAP_RIM as u32;
                        }
                    }
                }
                let t = &mut self.tiles[ti];
                t.near[tunnel as usize] = IdxRange { first: n0, count: self.idx_near.len() as u32 - n0 };
                t.far[tunnel as usize] = IdxRange { first: f0, count: self.idx_far.len() as u32 - f0 };
            }
        }
    }

    pub fn vertex_count(&self) -> usize {
        self.samples.len() * VERTS_PER_SAMPLE
    }

    /// Triangles of the near / far set.
    #[allow(dead_code)] // tests and diagnostics
    pub fn triangles(&self) -> (usize, usize) {
        (self.idx_near.len() / 3, self.idx_far.len() / 3)
    }

    /// The per-vertex in-race focus flags (D66): one byte per GPU vertex, **1 = relevant** (the
    /// road runs along the picked race line: draw in its own style) and **0 = other** (draw muted
    /// or hide, `RaceFocusCfg`). Upload with `buffer_sub_data` when the focus changes (new picked
    /// line / road rev; ~0.2 ms for the island). Without a focus the renderer uses an all-1
    /// buffer or no flag at all. `focus` must come from the road layer this mesh was built from;
    /// a sample the focus does not know counts as relevant.
    pub fn build_rel(&self, focus: &RoadFocus) -> Vec<u8> {
        // Per slot: where each chain's runs start in `focus.runs[slot]` (runs are in chain order).
        let index: Vec<HashMap<u32, (usize, usize)>> = focus
            .runs
            .iter()
            .map(|runs| {
                let mut m: HashMap<u32, (usize, usize)> = HashMap::new();
                for (i, r) in runs.iter().enumerate() {
                    m.entry(r.chain).and_modify(|e| e.1 = i + 1).or_insert((i, i + 1));
                }
                m
            })
            .collect();
        let mut out = vec![1u8; self.vertex_count()];
        let rel_of = |slot: u8, chain: u32, seg: u32| -> bool {
            if slot == SLOT_JUMP {
                return focus.jumps.get(chain as usize).copied().unwrap_or(true);
            }
            let k = (slot as usize).min(N_TYPES - 1);
            match index[k].get(&chain) {
                // Segment k lies in the run with a <= k < b (points a..=b).
                Some(&(lo, hi)) => focus.runs[k][lo..hi].iter().find(|r| r.a <= seg && seg < r.b).map_or(true, |r| r.relevant),
                None => true,
            }
        };
        for (i, (sm, src)) in self.samples.iter().zip(&self.src).enumerate() {
            let mut relevant = rel_of(sm.slot, src.chain, src.seg);
            // A sample on a node joins the segment before it and the one it starts: it is
            // relevant only if both are, so the flag never reaches into an irrelevant stretch
            // (a side road's stub) from the relevant side by interpolation across the quad.
            if relevant && i > 0 && sm.slot != SLOT_JUMP {
                let (ps, pr) = (&self.samples[i - 1], &self.src[i - 1]);
                if ps.slot == sm.slot && pr.chain == src.chain && pr.seg < src.seg && !src.cap && !pr.cap {
                    relevant = rel_of(sm.slot, src.chain, pr.seg);
                }
            }
            if !relevant {
                out[i * VERTS_PER_SAMPLE..(i + 1) * VERTS_PER_SAMPLE].fill(0);
            }
        }
        out
    }
}

/// The dense samples of one jump line (take-off -> landing, `[x, z, y, x, z, y]`): 2 m steps with
/// the taut string as their height.
fn jump_chain(j: &[f32; 6], terrain: &Terrain) -> Vec<Dense> {
    let pts = [[j[0], j[1]], [j[3], j[4]]];
    let mut dense = densify(&pts, &[j[2], j[5]], STEP_JUMP_M, false, terrain);
    if dense.len() < 2 {
        return Vec::new();
    }
    let mut s = vec![0.0f32; dense.len()];
    for i in 1..dense.len() {
        s[i] = s[i - 1] + (dense[i].x - dense[i - 1].x).hypot(dense[i].z - dense[i - 1].z);
    }
    let yt: Vec<f32> = dense.iter().map(|d| terrain.height(d.x, d.z)).collect();
    let ys = taut_string(&s, &yt, known_y(j[2]).unwrap_or(f32::NAN), known_y(j[5]).unwrap_or(f32::NAN));
    for (d, y) in dense.iter_mut().zip(ys) {
        d.y = y;
    }
    dense
}

/// The four GPU vertices of a sample. A cap's rim sample (`cap`) marks its right vertices (unused
/// on the rim) as the cap's **centre** (pad byte 1: the shader puts them on the sample's point
/// with `side` 0, so the fill's edge fade runs radially in the cap).
fn push_vertices(out: &mut Vec<u8>, sm: &Sample, cap: bool) {
    for bot in 0..2u8 {
        for side in [-1i8, 1i8] {
            out.extend_from_slice(&sm.x.to_le_bytes());
            out.extend_from_slice(&sm.z.to_le_bytes());
            out.extend_from_slice(&sm.y_node.to_le_bytes());
            out.extend_from_slice(&[side as u8, bot, sm.slot, (cap && side < 0) as u8]);
            out.extend_from_slice(&sm.tx.to_le_bytes());
            out.extend_from_slice(&sm.tz.to_le_bytes());
            out.extend_from_slice(&sm.s.to_le_bytes());
        }
    }
}

/// Vertex index of sample `s`: `bot` 0/1, `left` = side +1.
#[inline]
fn vi(s: u32, bot: u32, left: u32) -> u32 {
    s * VERTS_PER_SAMPLE as u32 + bot * 2 + left
}

/// Top surface of the segment `a -> b`: two triangles facing +y (counter-clockwise seen from
/// above in a right-handed x/y/z reading of the world coordinates; the renderer's matrix flips
/// handedness, so choose the culling face by looking, or draw both).
fn top_quad(idx: &mut Vec<u32>, a: u32, b: u32) {
    let (al, ar, bl, br) = (vi(a, 0, 1), vi(a, 0, 0), vi(b, 0, 1), vi(b, 0, 0));
    idx.extend_from_slice(&[al, bl, ar, bl, br, ar]);
}

/// Position of GPU vertex `v` relative to its sample's point for a half-width of 1 and a deck of
/// 1 (what the shader makes of it, up to scale): for choosing a cap triangle's winding.
fn unit_pos(samples: &[Sample], src: &[SampleSrc], v: u32) -> [f32; 3] {
    let i = (v / VERTS_PER_SAMPLE as u32) as usize;
    let sm = &samples[i];
    let k = v % VERTS_PER_SAMPLE as u32;
    let side = if k % 2 == 1 { 1.0 } else if src[i].cap { 0.0 } else { -1.0 };
    let t = unit(sm.tx, sm.tz).unwrap_or([1.0, 0.0]);
    [-t[1] * side, -((k / 2) as f32), t[0] * side]
}

/// Append the triangle `v` facing `out`, with the winding of [`top_quad`] (`(b - a) x (c - a)`
/// points out of the deck).
fn tri_out(idx: &mut Vec<u32>, samples: &[Sample], src: &[SampleSrc], v: [u32; 3], out: [f32; 3]) {
    let [a, b, c] = v.map(|v| unit_pos(samples, src, v));
    let (e1, e2) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
    let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
    if n[0] * out[0] + n[1] * out[1] + n[2] * out[2] >= 0.0 {
        idx.extend_from_slice(&v);
    } else {
        idx.extend_from_slice(&[v[0], v[2], v[1]]);
    }
}

/// A round cap (D81) on sample `end` with its rim samples `rim .. rim + CAP_RIM` ([`cap_rim`]):
/// the top as a fan round the centre vertex (the first rim sample's right vertex, see
/// [`push_vertices`]; near and far set), and in the near set the rim wall and the underside, so
/// a dead end's deck is closed. All its vertices sit on the end's point, so the shader gives them
/// the same half-width: the cap is exactly as wide as the ribbon.
fn cap_tris(samples: &[Sample], src: &[SampleSrc], end: u32, rim: u32, start: bool, near: &mut Vec<u32>, far: &mut Vec<u32>) {
    // The rim from the end vertex on the left of the outward direction round to the one on its
    // right (outward = backwards at a start cap: there the left one is the right-side vertex).
    let (a_left, b_left) = if start { (0, 1) } else { (1, 0) };
    let mut ring: Vec<u32> = Vec::with_capacity(CAP_RIM + 2);
    ring.push(vi(end, 0, a_left));
    ring.extend((0..CAP_RIM as u32).map(|j| vi(rim + j, 0, 1)));
    ring.push(vi(end, 0, b_left));
    let bot = |v: u32| v + 2; // the same sample's bottom vertex on the same side
    let centre = vi(rim, 0, 0);
    for w in ring.windows(2) {
        tri_out(near, samples, src, [centre, w[0], w[1]], [0.0, 1.0, 0.0]);
        tri_out(far, samples, src, [centre, w[0], w[1]], [0.0, 1.0, 0.0]);
        tri_out(near, samples, src, [bot(centre), bot(w[0]), bot(w[1])], [0.0, -1.0, 0.0]);
        let (p, q) = (unit_pos(samples, src, w[0]), unit_pos(samples, src, w[1]));
        let out = [p[0] + q[0], 0.0, p[2] + q[2]];
        tri_out(near, samples, src, [w[0], bot(w[0]), w[1]], out);
        tri_out(near, samples, src, [w[1], bot(w[0]), bot(w[1])], out);
    }
}

/// The whole deck of the segment `a -> b`: top 2, left wall 2, right wall 2, underside 2
/// triangles, every face outward (same handedness convention as [`top_quad`]).
fn near_quad(idx: &mut Vec<u32>, a: u32, b: u32) {
    top_quad(idx, a, b);
    let (alt, alb, blt, blb) = (vi(a, 0, 1), vi(a, 1, 1), vi(b, 0, 1), vi(b, 1, 1));
    let (art, arb, brt, brb) = (vi(a, 0, 0), vi(a, 1, 0), vi(b, 0, 0), vi(b, 1, 0));
    // Left wall (side +1), outward = the left normal.
    idx.extend_from_slice(&[alt, alb, blt, blt, alb, blb]);
    // Right wall (side -1).
    idx.extend_from_slice(&[art, brt, arb, brt, brb, arb]);
    // Underside, facing down.
    idx.extend_from_slice(&[alb, arb, blb, blb, arb, brb]);
}

// ── tests ────────────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::roadtypes::RoadType;
    use crate::maprender::data::Chain;
    use crate::maprender::racesel::{test_line, Run};

    fn slot(t: RoadType) -> usize {
        t.index() as usize
    }

    fn chain(pts: Vec<[f32; 2]>, y: f32) -> Chain {
        let n = pts.len();
        Chain::new(pts, vec![y; n])
    }

    /// A straight chain along +x from x0 to x1 at z.
    fn line(x0: f32, x1: f32, z: f32, y0: f32, y1: f32) -> Chain {
        Chain::new(vec![[x0, z], [x1, z]], vec![y0, y1])
    }

    fn decode(v: &[u8], i: usize) -> ([f32; 3], i8, u8, u8, [f32; 2], f32) {
        let b = &v[i * VERTEX_STRIDE..(i + 1) * VERTEX_STRIDE];
        let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        ([f(0), f(4), f(8)], b[12] as i8, b[13], b[14], [f(16), f(20)], f(24))
    }

    #[test]
    fn slot_constants_match_the_road_types() {
        assert_eq!(slot(RoadType::Crosscountry), SLOT_CROSSCOUNTRY as usize);
        assert_eq!(slot(RoadType::Tunnel), SLOT_TUNNEL as usize);
        assert_eq!(slot(RoadType::Jump), SLOT_JUMP as usize);
        assert_eq!(slot(RoadType::Turnaround), SLOT_TURNAROUND as usize);
    }

    #[test]
    fn road_y_follows_the_d51_rules() {
        use RoadHeight::{Nodes, Terrain as Ter};
        let (node, ter) = (130.0, 100.0);
        assert_eq!(road_y(1, node, ter, Nodes), node + LIFT_M);
        assert_eq!(road_y(1, node, ter, Ter), ter + LIFT_M);
        assert_eq!(road_y(8, node, ter, Nodes), node + LIFT_M, "highway: bridges float");
        // Cross-country is always the ground, jump lines always their own string.
        assert_eq!(road_y(SLOT_CROSSCOUNTRY, node, ter, Nodes), ter + LIFT_M);
        assert_eq!(road_y(SLOT_JUMP, node, ter, Ter), node + LIFT_M);
    }

    #[test]
    fn taut_string_is_the_upper_hull_of_the_ends_and_the_ground() {
        // Flat ground at 100, ends at 110 and 105: a straight chord.
        let s: Vec<f32> = (0..11).map(|i| i as f32 * 10.0).collect();
        let flat = vec![100.0; 11];
        let y = taut_string(&s, &flat, 110.0, 105.0);
        for (i, v) in y.iter().enumerate() {
            assert!((v - (110.0 - 0.05 * s[i])).abs() < 1e-4, "{i}: {v}");
        }
        // A cliff edge at s = 70 rising to 130 pokes above the chord: the string rests on it,
        // straight on either side, never below the ground, ends at the node heights.
        let mut ground = vec![100.0; 11];
        ground[7] = 130.0;
        let y = taut_string(&s, &ground, 110.0, 105.0);
        assert_eq!((y[0], y[10]), (110.0, 105.0));
        assert!(y.iter().zip(&ground).all(|(a, g)| a >= g), "{y:?}");
        assert!((y[7] - 130.0).abs() < 1e-4);
        assert!((y[3] - (110.0 + (130.0 - 110.0) * 30.0 / 70.0)).abs() < 1e-4, "{}", y[3]);
        // Concave: slopes never increase.
        let slopes: Vec<f32> = y.windows(2).map(|w| (w[1] - w[0]) / 10.0).collect();
        assert!(slopes.windows(2).all(|w| w[1] <= w[0] + 1e-5), "{slopes:?}");
        // An end under the ground is lifted onto it.
        let up = taut_string(&s, &ground, 50.0, 105.0);
        assert_eq!(up[0], 100.0);
        // Degenerate input.
        assert!(taut_string(&[], &[], 0.0, 0.0).is_empty());
        assert_eq!(taut_string(&[0.0], &[7.0], 3.0, 9.0).len(), 1);
    }

    #[test]
    fn resampling_keeps_nodes_bounds_the_step_and_interpolates_node_heights() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        // 3 vertices, one 100 m and one 30 m segment, heights 10 -> 20 -> 20.
        roads.by_type[1].push(Chain::new(vec![[-500.0, -700.0], [-400.0, -700.0], [-370.0, -700.0]], vec![10.0, 20.0, 20.0]));
        let m = RoadMesh::build(&roads, &t, 1);
        assert_eq!(m.pieces.len(), 1);
        // Both ends are dead ends: a round cap each (D81), its rim samples after the ribbon's.
        assert_eq!(m.pieces[0].caps, 3);
        assert_eq!(m.samples.len(), 18 + 2 * CAP_RIM);
        assert!(m.src[18..].iter().all(|r| r.cap) && m.src[..18].iter().all(|r| !r.cap));
        let s = &m.samples[..18];
        // 100 m -> 13 steps of 7.69 m, 30 m -> 4 steps of 7.5, plus the end: 18 samples.
        assert_eq!(s.len(), 18);
        for w in s.windows(2) {
            let d = (w[1].x - w[0].x).hypot(w[1].z - w[0].z);
            assert!(d <= STEP_NEAR_M + 1e-3 && d > 1.0, "step {d}");
            assert!((w[1].s - w[0].s - d).abs() < 1e-3, "s is the distance along");
        }
        // The middle vertex (x = -400) is a sample, and the heights are linear around it.
        let mid = s.iter().find(|p| (p.x - -400.0).abs() < 1e-3).expect("node kept");
        assert!((mid.y_node - 20.0).abs() < 1e-4);
        assert!((s[0].y_node - 10.0).abs() < 1e-4 && (s[17].y_node - 20.0).abs() < 1e-4);
        assert!(s.windows(2).take(13).all(|w| w[1].y_node >= w[0].y_node - 1e-5));
        // Tangents are unit and point along +x.
        assert!(s.iter().all(|p| (p.tx - 1.0).abs() < 1e-5 && p.tz.abs() < 1e-5 && p.slot == 1));
        assert!((s[17].s - 130.0).abs() < 1e-3);
        // Provenance: the first 13 samples start segment 0, the rest segment 1; the end belongs to segment 1.
        assert_eq!(m.src[..18].iter().map(|x| x.seg).collect::<Vec<_>>(), [vec![0; 13], vec![1; 5]].concat());
    }

    #[test]
    fn coincident_vertices_and_tiny_chains_are_skipped() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        roads.by_type[1].push(Chain::new(vec![[0.0, 0.0], [0.0, 0.0]], vec![0.0, 0.0])); // nothing
        roads.by_type[1].push(Chain::new(vec![[0.0, 0.0], [0.01, 0.0], [20.0, 0.0], [20.0, 0.0]], vec![1.0; 4])); // duplicates
        roads.by_type[9].push(line(0.0, 50.0, 50.0, 0.0, 0.0)); // turnaround: never
        let m = RoadMesh::build(&roads, &t, 1);
        assert_eq!(m.pieces.len(), 1);
        let ribbon = &m.samples[..m.pieces[0].count as usize];
        assert!(ribbon.windows(2).all(|w| (w[1].x - w[0].x).hypot(w[1].z - w[0].z) > 0.04));
        assert!(m.samples.iter().all(|p| p.slot == 1));
        assert!((ribbon.last().unwrap().x - 20.0).abs() < 1e-4);
        assert_eq!(RoadMesh::build(&RoadLayer::default(), &t, 1).vertices.len(), 0);
    }

    #[test]
    fn tiles_split_chains_at_1_km_borders_without_losing_length_or_a_gap() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        // 2.4 km along x at z = 100: crosses the borders at x = 0 and x = 1000 and 2000 from -700.
        roads.by_type[1].push(chain(vec![[-700.0, 100.0], [1700.0, 100.0]], 5.0));
        roads.by_type[1].push(chain(vec![[100.0, -900.0], [100.0, 900.0], [400.0, 900.0]], 5.0)); // other tiles too
        let m = RoadMesh::build(&roads, &t, 1);
        // Chain 0: tiles x -1, 0, 1 (x in -700..0, 0..1000, 1000..1700); chain 1 lies in tiles (0, -1) and (0, 0).
        let keys: Vec<(i32, i32)> = m.tiles.iter().map(|t| t.key).collect();
        assert_eq!(keys, vec![(-1, 0), (0, -1), (0, 0), (1, 0)], "sorted by tile, one tile entry each");
        assert!(m.pieces.windows(2).all(|w| w[0].tile <= w[1].tile));
        // Length: pieces of chain 0 add up to the chain (each segment is in exactly one piece).
        let seg_len = |p: &Piece| (0..p.count as usize - 1).map(|i| {
            let (a, b) = (m.samples[p.first as usize + i], m.samples[p.first as usize + i + 1]);
            (b.x - a.x).hypot(b.z - a.z)
        }).sum::<f32>();
        let total: f32 = m.pieces.iter().filter(|p| m.src[p.first as usize].chain == 0).map(seg_len).sum();
        assert!((total - 2400.0).abs() < 0.05, "{total}");
        // Pieces meet: a piece ends where the next starts, with the same s.
        let c0: Vec<&Piece> = m.pieces.iter().filter(|p| m.src[p.first as usize].chain == 0 && m.samples[p.first as usize].z == 100.0 && p.count > 2 && m.samples[p.first as usize].x < 1800.0).collect();
        let mut by_x = c0.clone();
        by_x.sort_by(|a, b| m.samples[a.first as usize].x.total_cmp(&m.samples[b.first as usize].x));
        for w in by_x.windows(2) {
            let (end, next) = (m.samples[(w[0].first + w[0].count - 1) as usize], m.samples[w[1].first as usize]);
            assert_eq!((end.x, end.z, end.s), (next.x, next.z, next.s));
            assert_eq!((end.tx, end.tz), (next.tx, next.tz), "same tangent on both sides of the border");
        }
        // Every sample of a piece but its last lies in the piece's tile; the last is the border point.
        for p in &m.pieces {
            let key = m.tiles[p.tile as usize].key;
            for i in 0..p.count as usize - 1 {
                let sm = m.samples[p.first as usize + i];
                assert_eq!(tile_key(sm.x, sm.z), key);
            }
        }
        // Tile bbox covers its samples.
        for tile in &m.tiles {
            for p in m.pieces.iter().filter(|p| m.tiles[p.tile as usize].key == tile.key) {
                for sm in &m.samples[p.first as usize..(p.first + p.count) as usize] {
                    assert!(sm.x >= tile.bbox[0] && sm.x <= tile.bbox[2] && sm.z >= tile.bbox[1] && sm.z <= tile.bbox[3]);
                }
            }
        }
    }

    #[test]
    fn crosscountry_is_always_draped_and_jumps_are_a_taut_string_over_the_ground() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        // Cross-country over the 220 m hill at (-300, 200) with node heights far from the ground.
        roads.by_type[5].push(Chain::new(vec![[-500.0, 200.0], [-100.0, 200.0]], vec![0.0, 0.0]));
        // A road with the same nodes keeps them.
        roads.by_type[1].push(Chain::new(vec![[-500.0, 210.0], [-100.0, 210.0]], vec![50.0, 50.0]));
        // A jump from one side of the hill to the other, ends at 120 m (the hill is 320 m).
        roads.jumps.push([-500.0, 200.0, 120.0, -100.0, 200.0, 120.0]);
        let m = RoadMesh::build(&roads, &t, 1);
        let (mut cc, mut road, mut jump) = (0, 0, 0);
        for sm in &m.samples {
            let ground = t.height(sm.x, sm.z);
            match sm.slot {
                5 => {
                    cc += 1;
                    assert!((sm.y_node - ground).abs() < 1e-3, "cross-country at {},{}: {} vs {ground}", sm.x, sm.z, sm.y_node);
                }
                1 => {
                    road += 1;
                    assert_eq!(sm.y_node, 50.0, "a road keeps its node heights (underground here)");
                }
                7 => {
                    jump += 1;
                    assert!(sm.y_node >= ground - 1e-3, "jump below the ground at {},{}: {} vs {ground}", sm.x, sm.z, sm.y_node);
                }
                _ => unreachable!(),
            }
        }
        assert!(cc > 40 && road > 40 && jump > 190, "{cc} {road} {jump}");
        // The jump line: 2 m steps, the ends keep their height (raised onto the ground if below it),
        // the middle rests on the hill top (~320 m), concave.
        let js: Vec<&Sample> = m.samples.iter().filter(|s| s.slot == 7).collect();
        assert!((js[0].s - 0.0).abs() < 1e-6 && js.windows(2).all(|w| (w[1].s - w[0].s) <= STEP_JUMP_M + 1e-3));
        let ground0 = t.height(-500.0, 200.0);
        assert!((js[0].y_node - 120.0f32.max(ground0)).abs() < 1e-3 && (js.last().unwrap().y_node - 120.0f32.max(t.height(-100.0, 200.0))).abs() < 1e-3);
        let top = js.iter().map(|s| s.y_node).fold(f32::MIN, f32::max);
        assert!(top > 300.0, "{top}");
        let slopes: Vec<f32> = js.windows(2).map(|w| (w[1].y_node - w[0].y_node) / (w[1].s - w[0].s)).collect();
        assert!(slopes.windows(2).all(|w| w[1] <= w[0] + 1e-3), "not concave");
        // Jump chains are numbered by the jump list and carry the jump slot into the vertices.
        assert!(m.src.iter().zip(&m.samples).filter(|(_, s)| s.slot == 7).all(|(r, _)| r.chain == 0 && r.seg == 0));
    }

    /// K1 note, handled in K2: nav orphans carry `y = 0` (`data::build_roads`), 100 m below the sea;
    /// a node-height road must not dive to 0 at such a node - the node counts as height-less and
    /// takes the terrain, with its neighbours' heights linear to it.
    #[test]
    fn orphan_nodes_without_a_height_take_the_terrain_not_zero() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        // Over the 220 m hill: real node heights 300 at both ends, an orphan (y = 0) in the middle.
        roads.by_type[1].push(Chain::new(vec![[-400.0, 200.0], [-300.0, 200.0], [-200.0, 200.0]], vec![300.0, 0.0, 300.0]));
        // A chain made only of orphans lies on the ground.
        roads.by_type[3].push(Chain::new(vec![[300.0, -200.0], [400.0, -200.0]], vec![0.0, 0.0]));
        // A jump whose take-off is an orphan: the string starts on the ground there, not at 0.
        roads.jumps.push([-500.0, -500.0, 0.0, -400.0, -500.0, 120.0]);
        let m = RoadMesh::build(&roads, &t, 1);
        assert!(m.samples.iter().all(|s| s.y_node > 50.0), "no sample dives to 0: min {}", m.samples.iter().map(|s| s.y_node).fold(f32::MAX, f32::min));
        let mid = m.samples.iter().find(|s| s.slot == 1 && (s.x - -300.0).abs() < 1e-3).expect("orphan node kept");
        assert!((mid.y_node - t.height(-300.0, 200.0)).abs() < 1e-3, "{} vs ground {}", mid.y_node, t.height(-300.0, 200.0));
        // (Between two orphan nodes the height is linear between their terrain heights.)
        let orphans: Vec<_> = m.samples.iter().filter(|s| s.slot == 3).collect();
        for s in [orphans[0], orphans[orphans.len() - 1]] {
            assert!((s.y_node - t.height(s.x, s.z)).abs() < 1e-3);
        }
        assert!(orphans.iter().all(|s| s.y_node > 90.0));
        let j0 = m.samples.iter().find(|s| s.slot == 7).unwrap();
        assert!((j0.y_node - t.height(j0.x, j0.z)).abs() < 1e-3, "jump starts on the ground: {}", j0.y_node);
        assert_eq!(known_y(0.0), None);
        assert_eq!(known_y(f32::NAN), None);
        assert_eq!(known_y(-1.8), Some(-1.8));
    }

    #[test]
    fn gpu_vertices_have_the_documented_layout() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        roads.by_type[8].push(line(10.0, 60.0, 30.0, 12.5, 12.5));
        let m = RoadMesh::build(&roads, &t, 5);
        assert_eq!(m.rev, 5);
        assert_eq!(m.terrain_rev, t.rev);
        assert_eq!(m.vertices.len(), m.samples.len() * VERTS_PER_SAMPLE * VERTEX_STRIDE);
        assert_eq!(VERTEX_STRIDE, 28);
        // Sample 3: its four vertices (top right, top left, bottom right, bottom left).
        let sm = m.samples[3];
        let want = [(-1, 0), (1, 0), (-1, 1), (1, 1)];
        for (k, (side, bot)) in want.into_iter().enumerate() {
            let (pos, sd, b, sl, tan, s) = decode(&m.vertices, 3 * VERTS_PER_SAMPLE + k);
            assert_eq!((pos, sd, b, sl, tan, s), ([sm.x, sm.z, sm.y_node], side, bot, 8, [sm.tx, sm.tz], sm.s));
        }
    }

    /// Positions of a GPU vertex for a ribbon of half-width `hw` and deck `deck`: what the vertex
    /// shader computes in `Nodes` mode (y exaggeration 1).
    fn pos(m: &RoadMesh, v: u32, hw: f32, deck: f32) -> [f32; 3] {
        let (p, side, bot, _, tan, _) = decode(&m.vertices, v as usize);
        // A cap's centre vertex (pad byte 1) sits on the point.
        let side = if m.vertices[v as usize * VERTEX_STRIDE + 15] == 1 { 0.0 } else { side as f32 };
        [p[0] + side * hw * -tan[1], p[2] - bot as f32 * deck, p[1] + side * hw * tan[0]]
    }

    fn normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
        let (e1, e2) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]]
    }

    /// Every face of the deck points outward: top up, underside down, each wall away from the
    /// road's axis; the winding is the same all along the chain (also around a corner).
    #[test]
    fn near_set_faces_point_outward_with_a_consistent_winding() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        roads.by_type[1].push(Chain::new(vec![[0.0, 0.0], [120.0, 0.0], [120.0, 90.0], [20.0, 90.0]], vec![40.0, 45.0, 45.0, 30.0]));
        let m = RoadMesh::build(&roads, &t, 1);
        // 8 triangles per segment, then the two round caps of the dead ends: 4 x CAP_SEGS each.
        let cap_idx = 4 * CAP_SEGS * 3;
        let segs = (m.idx_near.len() - 2 * cap_idx) / 24;
        assert_eq!(segs, m.pieces[0].count as usize - 1);
        assert_eq!(m.idx_near.len(), segs * 24 + 2 * cap_idx);
        let (hw, deck) = (3.0, 4.0);
        for q in 0..segs {
            let tris = &m.idx_near[q * 24..(q + 1) * 24];
            let (a, b) = (m.samples[q], m.samples[q + 1]);
            for (k, tri) in tris.chunks(3).enumerate() {
                assert!(tri.iter().all(|&i| (i as usize) < m.vertex_count()));
                let n = normal(pos(&m, tri[0], hw, deck), pos(&m, tri[1], hw, deck), pos(&m, tri[2], hw, deck));
                let left = [-(a.tz + b.tz), 0.0, a.tx + b.tx]; // the +side direction
                match k {
                    0 | 1 => assert!(n[1] > 0.0, "top faces up (seg {q} tri {k}): {n:?}"),
                    2 | 3 => assert!(n[0] * left[0] + n[2] * left[2] > 0.0 && n[1].abs() < 1e-3, "left wall outward (seg {q} tri {k}): {n:?}"),
                    4 | 5 => assert!(n[0] * left[0] + n[2] * left[2] < 0.0 && n[1].abs() < 1e-3, "right wall outward (seg {q} tri {k}): {n:?}"),
                    _ => assert!(n[1] < 0.0, "underside faces down (seg {q} tri {k}): {n:?}"),
                }
            }
        }
        // The caps: tops up, undersides down, the rim walls away from the end point.
        let mut seen = [0usize; 3];
        for tri in m.idx_near[segs * 24..].chunks(3) {
            let v = [0, 1, 2].map(|k| pos(&m, tri[k], hw, deck));
            let n = normal(v[0], v[1], v[2]);
            let centre = m.samples[tri[0] as usize / VERTS_PER_SAMPLE];
            let mid = [(v[0][0] + v[1][0] + v[2][0]) / 3.0 - centre.x, (v[0][2] + v[1][2] + v[2][2]) / 3.0 - centre.z];
            let (top, bottom) = (v.iter().all(|p| p[1] == centre.y_node), v.iter().all(|p| p[1] == centre.y_node - deck));
            if top {
                assert!(n[1] > 0.0, "cap top faces up: {n:?}");
                seen[0] += 1;
            } else if bottom {
                assert!(n[1] < 0.0, "cap underside faces down: {n:?}");
                seen[1] += 1;
            } else {
                assert!(n[0] * mid[0] + n[2] * mid[1] > 0.0 && n[1].abs() < 1e-3, "cap wall outward: {n:?} at {mid:?}");
                seen[2] += 1;
            }
        }
        assert_eq!(seen, [2 * CAP_SEGS, 2 * CAP_SEGS, 4 * CAP_SEGS]);
        // The far set (top only) uses the same winding.
        for tri in m.idx_far.chunks(3) {
            let n = normal(pos(&m, tri[0], hw, deck), pos(&m, tri[1], hw, deck), pos(&m, tri[2], hw, deck));
            assert!(n[1] > 0.0);
        }
    }

    #[test]
    fn lod_sets_tunnels_and_tile_ranges() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        // One tile: a 400 m road, a 400 m tunnel and a 60 m jump line; a second tile with a road.
        roads.by_type[1].push(line(100.0, 500.0, 100.0, 10.0, 10.0));
        roads.by_type[6].push(line(100.0, 500.0, 200.0, 10.0, 10.0));
        roads.jumps.push([100.0, 300.0, 10.0, 160.0, 300.0, 10.0]);
        roads.by_type[1].push(line(1100.0, 1200.0, 100.0, 10.0, 10.0));
        let m = RoadMesh::build(&roads, &t, 1);
        assert_eq!(m.tiles.len(), 2);
        let t0 = &m.tiles[0];
        // Index ranges are contiguous, cover both sets exactly, and the tunnel's are separate.
        let (n_all, f_all) = (m.idx_near.len() as u32, m.idx_far.len() as u32);
        let ends = |t: &Tile, near: bool| {
            let r = if near { &t.near } else { &t.far };
            (r[0].first, r[0].first + r[0].count, r[1].first, r[1].first + r[1].count)
        };
        let (a, b, c, d) = ends(t0, true);
        // Every road end is a dead end with a round cap (near: 4 x CAP_SEGS triangles, far: the
        // top's CAP_SEGS); jump lines have none.
        let (cap_near, cap_far) = (4 * CAP_SEGS as u32 * 3, CAP_SEGS as u32 * 3);
        assert_eq!((a, b), (0, 24 * (50 + 30) + 2 * cap_near), "road 50 segments + jump 30 segments");
        assert_eq!(b, c, "normal then tunnel");
        assert_eq!(d - c, 24 * 50 + 2 * cap_near, "400 m tunnel = 50 segments of 8 m");
        let (_, e, _, g) = ends(&m.tiles[1], true);
        assert_eq!(g, n_all);
        assert_eq!(e - m.tiles[1].near[0].first, 24 * 13 + 2 * cap_near);
        let (fa, fb, fc, fd) = ends(t0, false);
        assert_eq!(fa, 0);
        assert_eq!(fb, fc);
        assert_eq!(ends(&m.tiles[1], false).3, f_all);
        // Far tunnel: 400 m / 32 m = 12 kept steps + the end -> 13 quads of 6 indices.
        assert_eq!((fd - fc - 2 * cap_far) / 6, 13, "far tunnel quads");
        // The jump keeps all its samples in the far set (31 samples of 2 m -> 30 quads); the road
        // is thinned to ~32 m.
        let jump_far = (fb - fa - 2 * cap_far) / 6 - 13;
        assert_eq!(jump_far, 30, "jump far quads {jump_far}");
        // The far set is much lighter than the near one (top only, 4x fewer samples).
        let (nt, ft) = m.triangles();
        assert!(ft * 6 < nt, "{ft} vs {nt}");
        // Far samples are >= 32 m apart except at the end.
        let tunnel_piece = m.pieces.iter().find(|p| p.slot == 6).unwrap();
        let keep = far_subset(&m.samples[tunnel_piece.first as usize..(tunnel_piece.first + tunnel_piece.count) as usize], STEP_FAR_M);
        let ss: Vec<f32> = keep.iter().map(|&i| m.samples[tunnel_piece.first as usize + i].s).collect();
        assert!(ss.windows(2).take(ss.len() - 2).all(|w| w[1] - w[0] >= STEP_FAR_M - 1e-3));
        assert_eq!((ss[0], *ss.last().unwrap()), (0.0, 400.0));
        // All indices valid.
        assert!(m.idx_near.iter().chain(&m.idx_far).all(|&i| (i as usize) < m.vertex_count()));
        // Tile distance and extents.
        assert_eq!(t0.dist_to(300.0, 150.0), 0.0);
        assert!((t0.dist_to(0.0, 100.0) - 100.0).abs() < 1e-3);
        assert!(t0.y_range[0] <= 10.0 && t0.y_range[1] >= 10.0);
    }

    #[test]
    fn rel_flags_follow_the_focus_runs_and_jumps() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        // Chain 0: 11 vertices, 10 segments of 40 m; the focus calls segments 0..4 relevant, 4..10 not.
        roads.by_type[1].push(Chain::new((0..11).map(|i| [i as f32 * 40.0, 100.0]).collect(), vec![5.0; 11]));
        // Chain 1 is wholly not relevant (a run the focus builder emits for far chains).
        roads.by_type[1].push(line(0.0, 80.0, 300.0, 5.0, 5.0));
        roads.jumps.push([0.0, 400.0, 5.0, 40.0, 400.0, 5.0]);
        roads.jumps.push([0.0, 450.0, 5.0, 40.0, 450.0, 5.0]);
        let m = RoadMesh::build(&roads, &t, 1);
        let mut focus = RoadFocus::default();
        let bb = [0.0; 4];
        focus.runs[1] = vec![
            Run { chain: 0, a: 0, b: 4, relevant: true, bbox: bb },
            Run { chain: 0, a: 4, b: 10, relevant: false, bbox: bb },
            Run { chain: 1, a: 0, b: 1, relevant: false, bbox: bb },
        ];
        focus.jumps = vec![true, false];
        let rel = m.build_rel(&focus);
        assert_eq!(rel.len(), m.vertex_count());
        for (i, (sm, src)) in m.samples.iter().zip(&m.src).enumerate() {
            let want = match (sm.slot, src.chain) {
                (1, 0) => src.seg < 4,
                (1, 1) => false,
                (7, 0) => true,
                (7, 1) => false,
                _ => unreachable!(),
            };
            for k in 0..VERTS_PER_SAMPLE {
                assert_eq!(rel[i * VERTS_PER_SAMPLE + k], want as u8, "sample {i} ({:?} {src:?})", sm.slot);
            }
        }
        // Both values occur, and an empty focus (default runs, no jumps) means everything relevant.
        assert!(rel.contains(&0) && rel.contains(&1));
        assert!(m.build_rel(&RoadFocus::default()).iter().all(|&b| b == 1));
    }

    /// A side road joining the route: its flags are 0 right up to the junction node, even when
    /// the relevant segment follows the irrelevant one (no interpolated stub on the stub side).
    #[test]
    fn rel_flag_of_a_node_sample_is_relevant_only_if_both_its_segments_are() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        roads.by_type[1].push(Chain::new((0..11).map(|i| [i as f32 * 40.0, 100.0]).collect(), vec![5.0; 11]));
        let m = RoadMesh::build(&roads, &t, 1);
        let mut focus = RoadFocus::default();
        let bb = [0.0; 4];
        // Segments 0..4 are the side road (not relevant), 4..10 run along the route.
        focus.runs[1] = vec![Run { chain: 0, a: 0, b: 4, relevant: false, bbox: bb }, Run { chain: 0, a: 4, b: 10, relevant: true, bbox: bb }];
        let rel = m.build_rel(&focus);
        let node = m.samples.iter().zip(&m.src).position(|(_, s)| s.seg == 4).expect("the sample at node 4");
        assert_eq!(rel[node * VERTS_PER_SAMPLE], 0, "the junction node belongs to the stub side");
        let later = m.samples.iter().zip(&m.src).position(|(_, s)| s.seg == 5).unwrap();
        assert_eq!(rel[later * VERTS_PER_SAMPLE], 1);
    }

    /// The 3D flags use the same relevance: an overpass above the route is muted / hidden.
    #[test]
    fn rel_flags_drop_an_overpass_above_the_route() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        roads.by_type[1].push(Chain::new((0..=20).map(|i| [3.0, i as f32 * 50.0]).collect(), vec![50.0; 21]));
        roads.by_type[1].push(Chain::new((0..=20).map(|i| [3.0, i as f32 * 50.0]).collect(), vec![60.0; 21]));
        let mut l = test_line(1, 0.0, false);
        l.y = vec![50.0; l.pts.len()];
        let focus = RoadFocus::build(&roads, &l);
        let m = RoadMesh::build(&roads, &t, 1);
        let rel = m.build_rel(&focus);
        for (i, src) in m.src.iter().enumerate() {
            assert_eq!(rel[i * VERTS_PER_SAMPLE] == 1, src.chain == 0, "sample {i} (chain {})", src.chain);
        }
    }

    /// The same, with the real focus builder: roads along a race line are relevant, a crossing
    /// road far away is not.
    #[test]
    fn rel_flags_from_the_real_focus_builder() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        let along = 1usize;
        roads.by_type[along].push(Chain::new((0..=20).map(|i| [0.0, i as f32 * 50.0]).collect(), vec![5.0; 21]));
        roads.by_type[along].push(Chain::new((0..=20).map(|i| [400.0, i as f32 * 50.0]).collect(), vec![5.0; 21]));
        roads.jumps.push([-5.0, 100.0, 5.0, 5.0, 300.0, 5.0]); // both ends on the line's corridor
        roads.jumps.push([395.0, 100.0, 5.0, 405.0, 300.0, 5.0]);
        let focus = RoadFocus::build(&roads, &test_line(1, 0.0, false));
        let m = RoadMesh::build(&roads, &t, 1);
        let rel = m.build_rel(&focus);
        for (i, (sm, src)) in m.samples.iter().zip(&m.src).enumerate() {
            let want = src.chain == 0; // the road and the jump line on the race line's corridor
            assert_eq!(rel[i * VERTS_PER_SAMPLE] == 1, want, "sample {i} at x {} (chain {})", sm.x, src.chain);
        }
        assert!(focus.relevant_segments > 0);
    }

    /// D81: two pieces meeting at a degree-2 node (an L-corner of two chains, a type change) end
    /// in the very same vertices, the mitre's corners: no notch, no overlap, no cap; dead ends get
    /// a round cap.
    #[test]
    fn joined_ends_share_their_mitred_vertices_and_dead_ends_get_a_cap() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        roads.by_type[1].push(Chain::new(vec![[0.0, 100.0], [100.0, 100.0]], vec![50.0; 2])); // east
        roads.by_type[1].push(Chain::new(vec![[100.0, 100.0], [100.0, 200.0]], vec![50.0; 2])); // then north: an L
        roads.by_type[8].push(Chain::new(vec![[100.0, 200.0], [100.0, 300.0]], vec![50.0; 2])); // on as highway
        let m = RoadMesh::build(&roads, &t, 1);
        let at = |slot: u8, chain: u32, x: f32, z: f32| {
            let i = (0..m.samples.len()).find(|&i| {
                let (s, r) = (m.samples[i], m.src[i]);
                s.slot == slot && r.chain == chain && !r.cap && (s.x - x).abs() < 1e-3 && (s.z - z).abs() < 1e-3
            });
            i.expect("sample") as u32
        };
        let (hw, deck) = (1.0, 2.0);
        let top = |i: u32| (pos(&m, vi(i, 0, 1), hw, deck), pos(&m, vi(i, 0, 0), hw, deck));
        // The L: both ends carry the bisector, sqrt 2 long; left = the inner corner, right = the outer.
        let (a, b) = (at(1, 0, 100.0, 100.0), at(1, 1, 100.0, 100.0));
        let (sa, sb) = (m.samples[a as usize], m.samples[b as usize]);
        assert_eq!((sa.tx, sa.tz), (sb.tx, sb.tz));
        assert!((sa.tx - 1.0).abs() < 1e-5 && (sa.tz - 1.0).abs() < 1e-5, "{sa:?}");
        assert_eq!(top(a), top(b));
        let (l, r) = top(a);
        assert!((l[0] - 99.0).abs() < 1e-4 && (l[2] - 101.0).abs() < 1e-4 && (r[0] - 101.0).abs() < 1e-4 && (r[2] - 99.0).abs() < 1e-4, "{l:?} {r:?}");
        // The type change: straight on, unit tangent, the same vertices.
        let (c, d) = (at(1, 1, 100.0, 200.0), at(8, 0, 100.0, 200.0));
        assert_eq!(top(c), top(d));
        // Caps only on the two dead ends.
        let caps: Vec<(u8, u8)> = m.pieces.iter().map(|p| (p.slot, p.caps)).collect();
        assert_eq!(caps, vec![(1, 1), (1, 0), (8, 2)]);
        // A cap is exactly as wide as its road: every rim vertex hw from the end's point.
        let p = m.pieces[0];
        for j in 0..CAP_RIM as u32 {
            let v = pos(&m, vi(p.first + p.count + j, 0, 1), hw, deck);
            assert!(((v[0] - 0.0).hypot(v[2] - 100.0) - hw).abs() < 1e-4 && v[0] < 0.0, "rim {j}: {v:?}");
        }
    }

    /// D81 + D66: an arm hidden by the in-race focus takes its round cap with it (no stub at the
    /// junction), the other arms keep theirs.
    #[test]
    fn a_hidden_arm_takes_its_cap_with_it() {
        let t = Terrain::synthetic();
        let mut roads = RoadLayer::default();
        let o = [500.0, 500.0];
        for d in [[-1.0, 0.0], [1.0, 0.0], [0.0, -1.0], [0.0, 1.0]] {
            roads.by_type[1].push(Chain::new(vec![o, [o[0] + 80.0 * d[0], o[1] + 80.0 * d[1]]], vec![50.0; 2]));
        }
        let m = RoadMesh::build(&roads, &t, 1);
        assert!(m.pieces.iter().all(|p| p.caps == 3), "a junction cap and a dead-end cap each");
        let mut focus = RoadFocus::default();
        let bb = [0.0; 4];
        focus.runs[1] = (0..4).map(|c| Run { chain: c, a: 0, b: 1, relevant: c != 3, bbox: bb }).collect();
        let rel = m.build_rel(&focus);
        let mut cap_samples = [0; 4];
        for (i, src) in m.src.iter().enumerate() {
            assert_eq!(rel[i * VERTS_PER_SAMPLE] == 1, src.chain != 3, "sample {i} {src:?}");
            cap_samples[src.chain as usize] += src.cap as usize;
        }
        assert_eq!(cap_samples, [2 * CAP_RIM; 4]);
    }

    #[test]
    fn frustum_test_keeps_tiles_in_view_and_drops_the_rest() {
        use crate::maprender::view::{Camera, Relief};
        use egui::{pos2, vec2, Rect};
        let tr = std::sync::Arc::new(Terrain::synthetic());
        let mut roads = RoadLayer::default();
        roads.by_type[1].push(line(100.0, 300.0, 100.0, 120.0, 120.0)); // tile (0, 0)
        roads.by_type[1].push(line(5100.0, 5300.0, 100.0, 120.0, 120.0)); // tile (5, 0): 5 km east
        roads.by_type[1].push(line(-5900.0, -5700.0, 100.0, 120.0, 120.0)); // tile (-6, 0): 6 km west
        let m = RoadMesh::build(&roads, &tr, 1);
        assert_eq!(m.tiles.len(), 3);
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(208.0, 136.0));
        let cam = Camera::new(200.0, 100.0, 0.0, 300.0, rect, Camera::tilt_centre(rect, 0.85), 45f32.to_radians(), 200.0).with_relief(Relief::new(tr, 1.0, 120.0));
        let vp = cam.view_proj(1.0);
        let vis: Vec<bool> = m.tiles.iter().map(|t| t.in_frustum(&vp, cam.exag(), 30.0)).collect();
        assert_eq!(m.tiles.iter().map(|t| t.key).collect::<Vec<_>>(), vec![(-6, 0), (0, 0), (5, 0)]);
        assert_eq!(vis, vec![false, true, false]);
        // Looking east (yaw so that +x is ahead) finds the far east tile at a long zoom instead.
        let cam2 = Camera::new(4900.0, 100.0, std::f32::consts::FRAC_PI_2, 600.0, rect, Camera::tilt_centre(rect, 0.85), 45f32.to_radians(), 200.0).with_relief(Relief::new(std::sync::Arc::new(Terrain::synthetic()), 1.0, 120.0));
        let vp2 = cam2.view_proj(1.0);
        let keys: Vec<(i32, i32)> = m.tiles.iter().filter(|t| t.in_frustum(&vp2, 1.0, 30.0)).map(|t| t.key).collect();
        assert!(keys.contains(&(5, 0)), "{keys:?}");
    }

    /// The real island (skipped without an install): the design's sizes, every index valid,
    /// the jump lines resting on the ground, the cross-country draped.
    #[test]
    fn real_install_mesh() {
        use crate::gamedata::roadtypes::RoadTypes;
        use crate::maprender::data::GameData;
        use std::path::Path;
        let Some(media) = crate::gamedata::install::find_media(None) else {
            eprintln!("SKIP real_install_mesh: FH6 install not found");
            return;
        };
        let g = GameData::load(&media).expect("game data");
        let cur = RoadTypes::current_with(RoadTypes::project(), &RoadTypes::project_sha1(), Path::new("/nonexistent/road-types.json"), &g.nav);
        let layers = g.layers(&cur, 1);
        let cache = crate::gamedata::terrain::cache_dir();
        let terrain = Terrain::load_in(&cache, &media, &|_| {}).expect("terrain");
        let t0 = std::time::Instant::now();
        let m = RoadMesh::build(&layers.roads, &terrain, 1);
        let ms = t0.elapsed().as_millis();
        let (nt, ft) = m.triangles();
        eprintln!(
            "real road mesh: {ms} ms; {} samples, {} pieces, {} tiles, {} vertices = {:.1} MB, near {} tris ({:.1} MB), far {} tris",
            m.samples.len(), m.pieces.len(), m.tiles.len(), m.vertex_count(), m.vertices.len() as f64 / 1e6, nt, m.idx_near.len() as f64 * 4e-6, ft
        );
        // Design numbers (prototype, 1 816 chains incl. 18 jump chains): 121 267 samples, 485 068
        // vertices, 2 440 pieces, 2.85 M near indices. The Rust layer has 1 798 chains + 18 jumps.
        // (Pinned like `data::real_install_layers`: the project road-type file and the install are fixed.)
        // D81 adds 2 092 round caps (dead ends and junctions; the joins of two pieces are mitred):
        // 7 rim samples, 32 near and 8 far triangles each.
        let caps: u32 = m.pieces.iter().map(|p| p.caps.count_ones()).sum();
        assert_eq!(caps, 2_092);
        assert_eq!((m.samples.len(), m.pieces.len(), m.tiles.len()), (121_267 + 2_092 * CAP_RIM, 2_440, 177));
        assert_eq!(m.vertex_count(), 485_068 + 2_092 * CAP_RIM * VERTS_PER_SAMPLE);
        assert_eq!(m.vertices.len(), m.vertex_count() * VERTEX_STRIDE);
        assert_eq!(nt, 950_616 + 2_092 * 4 * CAP_SEGS, "8 triangles per segment, 4 x CAP_SEGS per cap");
        assert_eq!(ft, 54_176 + 2_092 * CAP_SEGS);
        assert!(ft * 4 < nt, "far set must be much lighter: {ft} vs {nt}");
        assert!(m.idx_near.iter().chain(&m.idx_far).all(|&i| (i as usize) < m.vertex_count()));
        // Tangents: unit times the mitre factor (D81), never longer than MITER_MAX.
        assert!(m.samples.iter().all(|s| s.x.is_finite() && s.z.is_finite() && s.y_node.is_finite() && (1.0 - 1e-3..=MITER_MAX + 1e-3).contains(&s.tx.hypot(s.tz))));
        // No turnaround, every other drawn slot present.
        assert!(m.samples.iter().all(|s| s.slot != SLOT_TURNAROUND));
        for sl in [1u8, 2, 3, 4, 5, 6, 7, 8] {
            assert!(m.samples.iter().any(|s| s.slot == sl), "slot {sl} missing");
        }
        // Piece ranges tile the sample array exactly, tiles cover the pieces, tunnels come last in a tile.
        let mut next = 0u32;
        for p in &m.pieces {
            assert_eq!(p.first, next);
            assert!(p.count >= 2);
            next += p.count + p.caps.count_ones() * CAP_RIM as u32;
        }
        assert_eq!(next as usize, m.samples.len());
        // Cross-country is on the ground; jump lines never below it, and some bend over the ground.
        let mut max_dev = 0.0f32;
        for s in &m.samples {
            let ground = terrain.height(s.x, s.z);
            if s.slot == SLOT_CROSSCOUNTRY {
                assert!((s.y_node - ground).abs() < 1e-3);
            }
            if s.slot == SLOT_JUMP {
                assert!(s.y_node >= ground - 1e-2, "jump below ground at {},{}", s.x, s.z);
            }
        }
        let mut bent = 0;
        for (ji, j) in layers.roads.jumps.iter().enumerate() {
            let samples: Vec<&Sample> = m.samples.iter().zip(&m.src).filter(|(s, r)| s.slot == SLOT_JUMP && r.chain as usize == ji).map(|(s, _)| s).collect();
            assert!(samples.len() >= 2, "jump {ji} has no samples");
            // The straight chord between the end heights (raised onto the ground like the string does).
            let len = (j[3] - j[0]).hypot(j[4] - j[1]);
            let (y0, y1) = (j[2].max(terrain.height(j[0], j[1])), j[5].max(terrain.height(j[3], j[4])));
            let dev = samples.iter().map(|s| s.y_node - (y0 + (y1 - y0) * s.s / len)).fold(0.0f32, f32::max);
            max_dev = max_dev.max(dev);
            bent += (dev > 0.5) as usize;
        }
        eprintln!("jump lines: {} chains, {bent} bend > 0.5 m over the chord, max {max_dev:.1} m", layers.roads.jumps.len());
        assert_eq!(layers.roads.jumps.len(), 18);
        // The design scout measured the same: 12 of the 18 bend > 0.5 m, the largest 13.4 m.
        assert_eq!(bent, 12);
        assert!((max_dev - 13.4).abs() < 0.2, "max bend {max_dev}");
        // Tile extents contain their samples and heights are plausible.
        assert!(m.tiles.iter().all(|t| t.y_range[0] > -50.0 && t.y_range[1] < 1600.0 && t.bbox[0] <= t.bbox[2]));
        // The focus flags work on the real thing: a line along the first road gives both values.
        let l0 = layers.races.lines.first().unwrap();
        let focus = RoadFocus::build(&layers.roads, l0);
        let rel = m.build_rel(&focus);
        assert_eq!(rel.len(), m.vertex_count());
        let ones = rel.iter().filter(|&&b| b == 1).count();
        assert!(ones > 0 && ones < rel.len() / 2, "relevant vertices {ones} of {}", rel.len());
    }

    /// D80: the race road's own mesh: open stretches in the race slot, a stretch under the hill
    /// in the tunnel slot, mitred where they meet (no caps there), round caps at the two ends; a
    /// closed circuit is one loop with no caps at all.
    #[test]
    fn the_race_road_mesh_splits_tunnels_and_closes_circuits() {
        let t = Terrain::synthetic();
        // West to east through the big hill at (-300, 200), 150 m high inside it.
        let pts: Vec<[f32; 2]> = (0..=90).map(|i| [-800.0 + i as f32 * 10.0, 205.0]).collect();
        let y: Vec<f32> = pts.iter().map(|p| t.height(p[0], p[1]).min(150.0) + 0.5).collect();
        let r = RaceRoad { pts: pts.clone(), y: y.clone(), closed: false };
        let l = race_road_layer(&r, &t);
        let (open, tun) = (&l.by_type[SLOT_RACE as usize], &l.by_type[SLOT_TUNNEL as usize]);
        assert_eq!((open.len(), tun.len()), (2, 1), "open, tunnel, open");
        assert!(tun[0].pts.iter().zip(&tun[0].y).all(|(p, y)| t.height(p[0], p[1]) - y >= RACE_TUNNEL_DEPTH_M - 1e-3));
        let ends: Vec<[End; 2]> = (0..2).map(|c| l.joins().get(SLOT_RACE as usize, c)).collect();
        assert!(matches!(ends[0], [End::Dead, End::Join { .. }]) && matches!(ends[1], [End::Join { .. }, End::Dead]), "{ends:?}");
        let m = RoadMesh::race_road(&r, &t);
        let caps: u32 = m.pieces.iter().map(|p| p.caps.count_ones()).sum();
        assert_eq!(caps, 2, "round caps at the two ends only");
        assert!(m.samples.iter().all(|s| s.slot == SLOT_RACE || s.slot == SLOT_TUNNEL));
        assert!(m.triangles().0 > 0 && m.tiles.iter().any(|t| t.near[1].count > 0), "a tunnel range to draw last");
        // The deck follows the line's own heights (not the terrain): inside the hill it is 150.5 m.
        assert!(m.samples.iter().filter(|s| s.slot == SLOT_TUNNEL).all(|s| (s.y_node - 150.5).abs() < 0.6));
        // A closed circuit on flat ground: one loop, mitred onto itself.
        let sq: Vec<[f32; 2]> = (0..40).map(|i| {
            let a = i as f32 / 40.0 * std::f32::consts::TAU;
            [300.0 + 200.0 * a.cos(), -300.0 + 200.0 * a.sin()]
        }).collect();
        let ys: Vec<f32> = sq.iter().map(|p| t.height(p[0], p[1]) + 0.5).collect();
        let ring = RaceRoad { pts: sq, y: ys, closed: true };
        let m = RoadMesh::race_road(&ring, &t);
        assert_eq!(m.pieces.iter().map(|p| p.caps.count_ones()).sum::<u32>(), 0, "a loop has no ends");
        let l = race_road_layer(&ring, &t);
        assert!(matches!(l.joins().get(SLOT_RACE as usize, 0), [End::Join { .. }, End::Join { .. }]));
    }
}
