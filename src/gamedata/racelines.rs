//! Race driving lines from the user's FH6 install: `OpenWorld/Brio/AITracks/Route<N>.owt` (the AI's
//! racing line with a track half-width per node) plus the start / finish line of the matching
//! `Route<N>.nav` (`RVAN` block). A port of `tools/fh6-extract/fh6owt.py` and
//! `extract_racelines.py`; formats in `docs/game-data/fh6-game-files.md` ("Race lines").
//!
//! **`.owt`** (`FTWO`, little-endian): `u32[24]` header at 0 — `h[8]` = section count, `h[9]` = node
//! count, `h[11] & 0xffff` = start node, `h[21]` = kind (256 point-to-point / 257 circuit / 258
//! circuit + tail). Nodes start at `0x60 + (16 + 48 × (sections − 2) when sections > 1)`, 56 B each:
//! `f32 pos[3]` (x, height, z), `f32 A[3]` (left half-width vector, horizontal), up vector, rest
//! undecoded. The file must be exactly `offset + 56 × count + 16` (or `+ 24`) bytes, else it is
//! not trusted and the route is skipped.
//!
//! **`.nav`**: first `RVAN` tag, block at `+16`: `f32 start_line[3]` at `+0`, `f32 finish_line[3]`
//! at `+16`. A route is a circuit when start and finish are < 1 m apart (the Python's test, not
//! `h[21]`).
//!
//! The raw node list holds more than one drive (a lead-in before the start node, a tail after the
//! finish), so [`build`] trims it like the Python: point-to-point `start ..= nearest node to the
//! finish line`, circuit = one lap rotated to begin at the start node. Then it decimates to one
//! point per `step_m` metres (first and last always kept).
//!
//! Reads all 170 routes in ≈ 20 ms (warm cache, release).

use std::path::Path;

use super::install::ci;
use super::poi::{PoiKind, Pois};

/// A circuit closes when its first and last point are closer than this (m); also the Python's check.
pub const CLOSED_M: f64 = 3.0;
/// Routes without a map pin: 99 is a test route (start at 0,0), 102 and 103 start off the map.
pub const NO_PIN_ROUTES: [u32; 3] = [99, 102, 103];

/// One decoded `.owt` node.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    /// x, height, z.
    pub p: [f32; 3],
    /// Left half-width vector (horizontal): the track edges are `p ± A`. Some nodes have `A = 0`.
    pub a: [f32; 3],
}

/// A parsed `.owt`.
#[derive(Clone, Debug)]
pub struct Owt {
    pub nodes: Vec<Node>,
    /// Start node (`h[11] & 0xffff`), clamped into the node list.
    pub start: usize,
    pub sections: u32,
    /// `h[21]`: 256 point-to-point, 257 circuit, 258 circuit + tail.
    pub kind: u32,
}

/// One race route's line, trimmed to a single drive.
#[derive(Clone, Debug)]
pub struct RaceLine {
    pub route: u32,
    /// RVAN start == finish (< 1 m).
    pub circuit: bool,
    /// RVAN start line (x, height, z) — the grid, not the map pin.
    pub start: [f32; 3],
    /// RVAN finish line.
    pub finish: [f32; 3],
    /// x, z of the trimmed, decimated centre line.
    pub pts: Vec<[f32; 2]>,
    /// Height per point.
    pub y: Vec<f32>,
    /// Left half-width vector (x, z) per point: edges = `pts ± half`, width = `|half|` (may be 0).
    pub half: Vec<[f32; 2]>,
    /// Length of the whole trimmed line before decimation (m, summed in f64: f32 drifts by 7 m on
    /// the 85 km route 5555).
    pub length_m: f64,
    /// A circuit whose line closes (first and last point < [`CLOSED_M`] apart).
    pub closed: bool,
    pub n_sections: u32,
    /// min x, min z, max x, max z of `pts`.
    pub bbox: [f32; 4],
}

/// What [`load_all`] found.
#[derive(Clone, Debug, Default)]
pub struct RaceLines {
    /// Sorted by route id.
    pub lines: Vec<RaceLine>,
    /// Routes that were skipped (id + reason): no `.nav`, bad `.owt` layout, no `RVAN`, no usable node.
    pub skipped: Vec<String>,
}

fn u32at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn f32at(d: &[u8], o: usize) -> f32 {
    f32::from_bits(u32at(d, o))
}

/// Parse the bytes of a `Route<N>.owt`. `Err` (with the reason) when the layout does not add up.
pub fn parse_owt(b: &[u8]) -> Result<Owt, String> {
    if b.len() < 0x60 || &b[0..4] != b"FTWO" {
        return Err("not an FTWO file".into());
    }
    let h: Vec<u32> = (0..24).map(|i| u32at(b, i * 4)).collect();
    let (sections, count) = (h[8], h[9] as usize);
    let off = 0x60 + if sections <= 1 { 0 } else { 16 + 48 * (sections as usize - 2) };
    let body = count.saturating_mul(56).saturating_add(off);
    if ![16usize, 24].iter().any(|t| body.saturating_add(*t) == b.len()) {
        return Err(format!("layout does not add up (offset {off}, {count} nodes, {} bytes)", b.len()));
    }
    let nodes = (0..count)
        .map(|i| {
            let o = off + i * 56;
            Node { p: [f32at(b, o), f32at(b, o + 4), f32at(b, o + 8)], a: [f32at(b, o + 12), f32at(b, o + 16), f32at(b, o + 20)] }
        })
        .collect::<Vec<_>>();
    let start = ((h[11] & 0xffff) as usize).min(count.saturating_sub(1));
    Ok(Owt { nodes, start, sections, kind: h[21] })
}

/// `(start_line, finish_line)` (x, height, z each) of the `RVAN` block of a `Route<N>.nav`.
pub fn parse_rvan(b: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    let i = b.windows(4).position(|w| w == b"RVAN")?;
    let blk = i + 16;
    if blk + 32 > b.len() {
        return None;
    }
    Some(([f32at(b, blk), f32at(b, blk + 4), f32at(b, blk + 8)], [f32at(b, blk + 16), f32at(b, blk + 20), f32at(b, blk + 24)]))
}

fn dist3(a: [f32; 3], b: [f32; 3]) -> f64 {
    let d = |i: usize| f64::from(a[i]) - f64::from(b[i]);
    (d(0) * d(0) + d(1) * d(1) + d(2) * d(2)).sqrt()
}

/// Trim and decimate one route (port of `extract_racelines.py:trim/decimate`). `step_m <= 0` keeps
/// every node. `None` when no finite node is left.
pub fn build(route: u32, owt: &Owt, start_line: [f32; 3], finish_line: [f32; 3], step_m: f32) -> Option<RaceLine> {
    let n = owt.nodes.len();
    if n == 0 {
        return None;
    }
    let circuit = dist3(start_line, finish_line) < 1.0;
    let st = owt.start.min(n - 1);
    let finite = |v: &[f32; 3]| v.iter().all(|c| c.is_finite());
    // non-finite nodes count as "very far away" for the searches and are dropped from the output
    let pp = |i: usize| if finite(&owt.nodes[i].p) { owt.nodes[i].p } else { [1e9; 3] };
    let idx: Vec<usize> = if !circuit {
        let (mut best, mut bi) = (f64::MAX, st);
        for i in st..n {
            let d = dist3(pp(i), finish_line);
            if d < best {
                best = d;
                bi = i;
            }
        }
        (st..=bi).collect()
    } else {
        // one lap: the first local minimum after the start node within 2.6 m of node 0 closes it
        let d0 = |i: usize| dist3(pp(i), pp(0));
        let j = (st.max(1)..n).find(|&i| d0(i) <= 2.6 && d0(i) <= d0(i - 1) && (i == n - 1 || d0(i) <= d0(i + 1))).unwrap_or(n - 1);
        (st..=j).chain(0..=st).collect()
    };
    let idx: Vec<usize> = idx.into_iter().filter(|&i| finite(&owt.nodes[i].p) && finite(&owt.nodes[i].a)).collect();
    if idx.is_empty() {
        return None;
    }
    let length_m: f64 = idx.windows(2).map(|w| dist3(owt.nodes[w[0]].p, owt.nodes[w[1]].p)).sum();

    // keep a node once `step_m` metres (horizontal) have accumulated since the last kept one
    let mut keep = vec![0usize];
    let mut acc = 0.0f64;
    for k in 1..idx.len() {
        let (a, b) = (owt.nodes[idx[k - 1]].p, owt.nodes[idx[k]].p);
        acc += (f64::from(b[0] - a[0]).powi(2) + f64::from(b[2] - a[2]).powi(2)).sqrt();
        if step_m <= 0.0 || acc >= f64::from(step_m) {
            keep.push(k);
            acc = 0.0;
        }
    }
    if keep.last() != Some(&(idx.len() - 1)) {
        keep.push(idx.len() - 1);
    }
    let node = |k: usize| &owt.nodes[idx[k]];
    let pts: Vec<[f32; 2]> = keep.iter().map(|&k| [node(k).p[0], node(k).p[2]]).collect();
    let y: Vec<f32> = keep.iter().map(|&k| node(k).p[1]).collect();
    let half: Vec<[f32; 2]> = keep.iter().map(|&k| [node(k).a[0], node(k).a[2]]).collect();
    let closed = circuit && {
        let (a, b) = (keep[0], keep[keep.len() - 1]);
        dist3(node(a).p, node(b).p) < CLOSED_M
    };
    let mut bbox = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for p in &pts {
        bbox = [bbox[0].min(p[0]), bbox[1].min(p[1]), bbox[2].max(p[0]), bbox[3].max(p[1])];
    }
    Some(RaceLine { route, circuit, start: start_line, finish: finish_line, pts, y, half, length_m, closed, n_sections: owt.sections, bbox })
}

/// Read every `Route<N>.owt` (+ `.nav`) under `<media>/OpenWorld/Brio/AITracks`, decimated to
/// `step_m` metres (≤ 0 keeps every node). A route with a missing/odd file is skipped and listed in
/// [`RaceLines::skipped`]; `Err` only when the folder is missing or not a single route was readable.
pub fn load_all(media: &Path, step_m: f32) -> Result<RaceLines, String> {
    let dir = ci(media, "OpenWorld/Brio/AITracks").filter(|p| p.is_dir()).ok_or_else(|| "no OpenWorld/Brio/AITracks folder in the install".to_string())?;
    let mut ids: Vec<u32> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .filter_map(|e| {
            let l = e.file_name().to_string_lossy().to_ascii_lowercase();
            l.strip_prefix("route")?.strip_suffix(".owt")?.parse().ok()
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let mut out = RaceLines::default();
    for id in ids {
        let read = |ext: &str| -> Result<Vec<u8>, String> {
            let p = ci(&dir, &format!("Route{id}.{ext}")).filter(|p| p.is_file()).ok_or_else(|| format!("Route{id}.{ext} missing"))?;
            std::fs::read(&p).map_err(|e| format!("Route{id}.{ext}: {e}"))
        };
        let line = (|| {
            let owt = parse_owt(&read("owt")?).map_err(|e| format!("Route{id}.owt: {e}"))?;
            let (s, f) = parse_rvan(&read("nav")?).ok_or_else(|| format!("Route{id}.nav: no RVAN block"))?;
            build(id, &owt, s, f, step_m).ok_or_else(|| format!("Route{id}.owt: no finite node"))
        })();
        match line {
            Ok(l) => out.lines.push(l),
            Err(e) => out.skipped.push(e),
        }
    }
    if out.lines.is_empty() {
        return Err(format!("no race line could be read ({} route(s) skipped)", out.skipped.len()));
    }
    Ok(out)
}

/// Where a [`RacePinPos`] comes from, best first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PinSource {
    /// The `race_trigger_zone_rt<N>` sphere: the exact in-game map pin.
    Sphere,
    /// The `sidi_touge_event_<N>` locator (used when there is no sphere).
    Touge,
    /// The RVAN start line: not the pin, but a validated prediction (the pin is within 41 m of it
    /// for the 55 routes where both are known).
    StartLine,
}

/// A race's map pin position.
#[derive(Clone, Copy, Debug)]
pub struct RacePinPos {
    pub route: u32,
    pub x: f32,
    pub z: f32,
    pub y: f32,
    pub source: PinSource,
}

/// One pin per route in `lines` except [`NO_PIN_ROUTES`]: sphere, else touge locator, else start
/// line. Tutorial routes 3333–3337 and Horizon Chase routes 30100+ are kept — the renderer or the
/// user decides whether to draw them.
///
/// Why this order: the in-game pins match the spheres exactly while the start lines are offset by
/// up to 780 m; for the routes with neither source the start line is the best known guess.
pub fn race_pins(pois: &Pois, lines: &[RaceLine]) -> Vec<RacePinPos> {
    let find = |kind: PoiKind, route: u32| pois.of(kind).find(|p| p.n == route).map(|p| (p.x, p.z, p.y));
    lines
        .iter()
        .filter(|l| !NO_PIN_ROUTES.contains(&l.route))
        .map(|l| {
            let (x, z, y, source) = if let Some((x, z, y)) = find(PoiKind::RacePin, l.route) {
                (x, z, y, PinSource::Sphere)
            } else if let Some((x, z, y)) = find(PoiKind::TougeEvent, l.route) {
                (x, z, y, PinSource::Touge)
            } else {
                (l.start[0], l.start[2], l.start[1], PinSource::StartLine)
            };
            RacePinPos { route: l.route, x, z, y, source }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::install::find_media;

    /// An `.owt`: `sections` sections, nodes `(x, y, z, a_x, a_z)`, start node, trailer of 16 or 24 B.
    fn owt_bytes(sections: u32, start: u32, nodes: &[(f32, f32, f32, f32, f32)], trailer: usize) -> Vec<u8> {
        let mut h = [0u32; 24];
        h[8] = sections;
        h[9] = nodes.len() as u32;
        h[11] = start | 0xabcd_0000; // the high half is not part of the node index
        h[21] = 256;
        let mut b = Vec::new();
        b.extend_from_slice(b"FTWO");
        for v in &h[1..] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.resize(0x60 + if sections <= 1 { 0 } else { 16 + 48 * (sections as usize - 2) }, 0);
        for &(x, y, z, ax, az) in nodes {
            for v in [x, y, z, ax, 0.0, az] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(&[0u8; 32]);
        }
        b.resize(b.len() + trailer, 0);
        b
    }

    fn rvan_bytes(start: [f32; 3], finish: [f32; 3]) -> Vec<u8> {
        let mut b = vec![0u8; 40];
        b.extend_from_slice(b"RVAN");
        b.extend_from_slice(&[0u8; 12]);
        for v in start.iter().chain(&[0.0]).chain(&finish).chain(&[0.0]) {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }

    /// Straight line along x, one node per metre, half-width vector (0, 0, 3).
    fn line(n: usize) -> Vec<(f32, f32, f32, f32, f32)> {
        (0..n).map(|i| (i as f32, 100.0, 0.0, 0.0, 3.0)).collect()
    }

    #[test]
    fn parse_owt_layouts() {
        for (sections, trailer) in [(1, 16), (1, 24), (0, 16), (2, 16), (5, 24)] {
            let o = parse_owt(&owt_bytes(sections, 3, &line(10), trailer)).unwrap();
            assert_eq!(o.nodes.len(), 10, "sections {sections}");
            assert_eq!(o.start, 3, "the high half of h[11] is masked");
            assert_eq!(o.nodes[7].p, [7.0, 100.0, 0.0]);
            assert_eq!(o.nodes[7].a, [0.0, 0.0, 3.0]);
        }
        // start node clamped into the list
        assert_eq!(parse_owt(&owt_bytes(1, 99, &line(4), 16)).unwrap().start, 3);
    }

    #[test]
    fn parse_owt_size_mismatch_and_garbage() {
        let mut b = owt_bytes(1, 0, &line(10), 16);
        b.pop();
        assert!(parse_owt(&b).is_err(), "one byte short");
        let mut b = owt_bytes(1, 0, &line(10), 16);
        b.push(0);
        assert!(parse_owt(&b).is_err(), "one byte long");
        // header says 3 sections but the data is laid out for 1
        let mut b = owt_bytes(1, 0, &line(10), 16);
        b[8 * 4..8 * 4 + 4].copy_from_slice(&3u32.to_le_bytes());
        assert!(parse_owt(&b).is_err());
        let mut b = owt_bytes(1, 0, &line(10), 16);
        b[9 * 4..9 * 4 + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_owt(&b).is_err(), "absurd count must not overflow");
        assert!(parse_owt(b"FTWO").is_err());
        let mut b = owt_bytes(1, 0, &line(10), 16);
        b[0] = b'X';
        assert!(parse_owt(&b).is_err(), "bad magic");
    }

    #[test]
    fn rvan_found_and_truncated() {
        let b = rvan_bytes([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]);
        assert_eq!(parse_rvan(&b), Some(([1.0, 2.0, 3.0], [4.0, 5.0, 6.0])));
        assert_eq!(parse_rvan(&b[..b.len() - 4]), None);
        assert_eq!(parse_rvan(&[0u8; 64]), None);
    }

    #[test]
    fn point_to_point_trim_cuts_lead_in_and_tail() {
        // start node 4, finish line at x = 30: lead-in 0..4 and tail 31.. are cut
        let owt = parse_owt(&owt_bytes(1, 4, &line(60), 16)).unwrap();
        let l = build(7, &owt, [4.0, 100.0, 0.0], [30.2, 100.0, 0.0], 0.0).unwrap();
        assert!(!l.circuit && !l.closed);
        assert_eq!(l.pts.first(), Some(&[4.0, 0.0]));
        assert_eq!(l.pts.last(), Some(&[30.0, 0.0]));
        assert_eq!(l.pts.len(), 27, "step 0 keeps every node");
        assert!((l.length_m - 26.0).abs() < 1e-9, "{}", l.length_m);
        assert_eq!(l.y.len(), 27);
        assert_eq!(l.half[3], [0.0, 3.0]);
        assert_eq!(l.bbox, [4.0, 0.0, 30.0, 0.0]);
    }

    #[test]
    fn decimate_keeps_first_and_last() {
        let owt = parse_owt(&owt_bytes(1, 0, &line(23), 16)).unwrap();
        let l = build(1, &owt, [0.0, 100.0, 0.0], [22.0, 100.0, 0.0], 5.0).unwrap();
        let xs: Vec<f32> = l.pts.iter().map(|p| p[0]).collect();
        assert_eq!(xs, vec![0.0, 5.0, 10.0, 15.0, 20.0, 22.0], "every >= 5 m, the last node is always kept");
        assert!((l.length_m - 22.0).abs() < 1e-9, "length is of the undecimated line");
        // a single-node line is one point
        let owt = parse_owt(&owt_bytes(1, 0, &line(1), 16)).unwrap();
        let l = build(1, &owt, [0.0, 100.0, 0.0], [50.0, 100.0, 0.0], 5.0).unwrap();
        assert_eq!(l.pts.len(), 1);
    }

    #[test]
    fn circuit_is_trimmed_to_one_lap_and_rotated() {
        // a square lap of 40 nodes (10 m sides), start node 10, then a 5-node tail that overshoots
        let mut nodes = Vec::new();
        for i in 0..40 {
            let (s, k) = (i / 10, (i % 10) as f32);
            let (x, z) = [(k, 0.0), (10.0, k), (10.0 - k, 10.0), (0.0, 10.0 - k)][s];
            nodes.push((x, 0.0, z, 1.0, 0.0));
        }
        nodes.push((0.0, 0.0, 0.2, 1.0, 0.0)); // back at node 0 (within 2.6 m, a local minimum)
        for i in 1..5 {
            nodes.push((i as f32 * 3.0, 0.0, -3.0, 1.0, 0.0)); // lead-out tail
        }
        let owt = parse_owt(&owt_bytes(1, 10, &nodes, 24)).unwrap();
        let l = build(3, &owt, [10.0, 0.0, 0.0], [10.0, 0.0, 0.0], 0.0).unwrap();
        assert!(l.circuit);
        assert_eq!(l.pts.len(), (10..=40).count() + (0..=10).count(), "start..=closing node, then 0..=start");
        assert_eq!(l.pts[0], [10.0, 0.0], "begins at the start node");
        assert_eq!(*l.pts.last().unwrap(), [10.0, 0.0], "and returns to it");
        assert!(l.closed);
    }

    #[test]
    fn non_finite_nodes_are_dropped() {
        let mut nodes = line(10);
        nodes[4].0 = f32::NAN;
        nodes[6].4 = f32::INFINITY; // non-finite A also drops the node
        let owt = parse_owt(&owt_bytes(1, 0, &nodes, 16)).unwrap();
        let l = build(1, &owt, [0.0, 100.0, 0.0], [9.0, 100.0, 0.0], 0.0).unwrap();
        assert_eq!(l.pts.len(), 8);
        assert!(l.pts.iter().all(|p| p[0].is_finite()) && l.length_m.is_finite());
        // all nodes bad -> no line
        let nodes = vec![(f32::NAN, 0.0, 0.0, 0.0, 0.0); 3];
        let owt = parse_owt(&owt_bytes(1, 0, &nodes, 16)).unwrap();
        assert!(build(1, &owt, [0.0; 3], [5.0; 3], 5.0).is_none());
        let empty = parse_owt(&owt_bytes(1, 0, &[], 16)).unwrap();
        assert!(build(1, &empty, [0.0; 3], [5.0; 3], 5.0).is_none());
    }

    #[test]
    fn zero_half_width_vectors_are_kept() {
        let nodes: Vec<_> = (0..5).map(|i| (i as f32, 0.0, 0.0, 0.0, 0.0)).collect();
        let owt = parse_owt(&owt_bytes(1, 0, &nodes, 16)).unwrap();
        let l = build(1, &owt, [0.0; 3], [4.0, 0.0, 0.0], 0.0).unwrap();
        assert!(l.half.iter().all(|h| *h == [0.0, 0.0]));
    }

    #[test]
    fn load_all_skips_and_counts_bad_routes() {
        let root = crate::gamedata::tempdir("racelines");
        let dir = root.join("openworld/BRIO/aitracks");
        std::fs::create_dir_all(&dir).unwrap();
        let put = |name: &str, b: &[u8]| std::fs::write(dir.join(name), b).unwrap();
        // 1: good; 2: no .nav; 3: .owt layout broken; 4: .nav without RVAN
        put("Route1.owt", &owt_bytes(1, 0, &line(20), 16));
        put("Route1.nav", &rvan_bytes([0.0, 100.0, 0.0], [19.0, 100.0, 0.0]));
        put("ROUTE2.OWT", &owt_bytes(1, 0, &line(20), 16));
        put("Route3.owt", &owt_bytes(1, 0, &line(20), 16)[..200]);
        put("Route3.nav", &rvan_bytes([0.0; 3], [1.0; 3]));
        put("Route4.owt", &owt_bytes(1, 0, &line(20), 16));
        put("Route4.nav", &[0u8; 100]);
        put("Route5.txt", b"ignored");
        let r = load_all(&root, 5.0).unwrap();
        assert_eq!(r.lines.iter().map(|l| l.route).collect::<Vec<_>>(), vec![1]);
        assert_eq!(r.skipped.len(), 3, "{:?}", r.skipped);
        assert!(r.skipped[0].contains("Route2.nav missing"), "{:?}", r.skipped);
        assert!(r.skipped[1].contains("Route3.owt"), "{:?}", r.skipped);
        assert!(r.skipped[2].contains("no RVAN"), "{:?}", r.skipped);
        // nothing readable at all -> Err
        std::fs::remove_file(dir.join("Route1.nav")).unwrap();
        assert!(load_all(&root, 5.0).is_err());
        assert!(load_all(&root.join("nope"), 5.0).is_err());
    }

    #[test]
    fn pin_priority_and_exclusions() {
        use crate::gamedata::poi::Poi;
        let mk = |route: u32, start: [f32; 3]| RaceLine {
            route,
            circuit: false,
            start,
            finish: start,
            pts: vec![[start[0], start[2]]],
            y: vec![start[1]],
            half: vec![[0.0, 0.0]],
            length_m: 0.0,
            closed: false,
            n_sections: 1,
            bbox: [0.0; 4],
        };
        let poi = |kind, x, n| Poi { kind, x, z: x + 1.0, y: 5.0, name: String::new(), n, gate: None };
        let pois = Pois {
            items: vec![poi(PoiKind::RacePin, 10.0, 1), poi(PoiKind::TougeEvent, 20.0, 1), poi(PoiKind::TougeEvent, 30.0, 2)],
            ..Default::default()
        };
        let lines = [mk(1, [100.0, 7.0, 200.0]), mk(2, [100.0, 7.0, 200.0]), mk(3, [100.0, 7.0, 200.0]), mk(99, [0.0; 3]), mk(102, [8000.0; 3]), mk(103, [9000.0; 3])];
        let pins = race_pins(&pois, &lines);
        let got: Vec<_> = pins.iter().map(|p| (p.route, p.x, p.z, p.y, p.source)).collect();
        assert_eq!(
            got,
            vec![(1, 10.0, 11.0, 5.0, PinSource::Sphere), (2, 30.0, 31.0, 5.0, PinSource::Touge), (3, 100.0, 200.0, 7.0, PinSource::StartLine)]
        );
    }

    /// Python reference (`extract_racelines.py --step 5` and `--step 0`) on the install of 2026-10.
    /// A game update changes these files: these are *today's* numbers, so on a mismatch re-run the
    /// Python, sanity-check the diff, then update here and in `docs/game-data/fh6-game-files.md`.
    #[test]
    fn real_install_race_lines_reference() {
        let Some(media) = find_media(None) else {
            eprintln!("SKIP real_install_race_lines_reference: FH6 install not found");
            return;
        };
        let t0 = std::time::Instant::now();
        let r = load_all(&media, 5.0).expect("race lines");
        eprintln!("load_all(5 m): {:?}", t0.elapsed());
        assert!(r.skipped.is_empty(), "{:?}", r.skipped);
        let lines = &r.lines;
        assert_eq!(lines.len(), 170);
        assert!(lines.windows(2).all(|w| w[0].route < w[1].route), "sorted by route id");
        let circuits: Vec<_> = lines.iter().filter(|l| l.circuit).collect();
        assert_eq!((lines.len() - circuits.len(), circuits.len()), (127, 43), "point-to-point / circuits");
        assert!(circuits.iter().all(|l| l.closed), "43/43 circuits close < 3 m");
        assert_eq!(lines.iter().map(|l| l.pts.len()).sum::<usize>(), 171_564);
        assert_eq!(lines.iter().filter(|l| !l.circuit).map(|l| l.pts.len()).sum::<usize>(), 130_126);
        assert_eq!(circuits.iter().map(|l| l.pts.len()).sum::<usize>(), 41_438);
        let multi: Vec<u32> = lines.iter().filter(|l| l.n_sections != 1).map(|l| l.route).collect();
        assert_eq!(multi, [132, 281, 351, 1181, 1281, 8008]);

        let by = |id: u32| lines.iter().find(|l| l.route == id).unwrap_or_else(|| panic!("no route {id}"));
        let total_km = lines.iter().map(|l| l.length_m).sum::<f64>() / 1000.0;
        assert!((total_km - 1033.2).abs() < 0.2, "{total_km}");
        assert!((by(5555).length_m - 85_400.0).abs() < 100.0, "{}", by(5555).length_m);
        assert!((by(11045).length_m - 170.4).abs() < 1.0, "{}", by(11045).length_m);
        let longest = lines.iter().max_by(|a, b| a.length_m.total_cmp(&b.length_m)).unwrap();
        assert_eq!(longest.route, 5555);

        // point-to-point lines end at the finish line; every line starts at its start line (not test route 99)
        for l in lines.iter().filter(|l| !l.circuit) {
            let e = l.pts.last().unwrap();
            let gap = dist3([e[0], *l.y.last().unwrap(), e[1]], l.finish);
            assert!(gap < 3.0, "route {} ends {gap} m from its finish line", l.route);
        }
        for l in lines.iter().filter(|l| l.route != 99) {
            let b = l.pts[0];
            let gap = dist3([b[0], l.y[0], b[1]], l.start);
            assert!(gap < 1.0, "route {} starts {gap} m from its start line", l.route);
        }
        assert_eq!(by(99).start, [0.0, 0.0, 0.0]);
        assert!(by(102).start[0] > 8000.0 && by(103).start[2] > 19000.0, "off-map routes");
        for l in lines.iter().filter(|l| !NO_PIN_ROUTES.contains(&l.route)) {
            assert!(l.bbox[0] > -8100.0 && l.bbox[2] < 6400.0 && l.bbox[1] > -9600.0 && l.bbox[3] < 9300.0, "route {} {:?}", l.route, l.bbox);
        }
        assert!(lines.iter().flat_map(|l| &l.half).all(|h| h[0].is_finite() && h[1].is_finite()));
        let max_half = lines.iter().flat_map(|l| &l.half).map(|h| h[0].hypot(h[1])).fold(0.0f32, f32::max);
        assert!((24.0..26.0).contains(&max_half), "{max_half}");

        // every node (step 0)
        let t0 = std::time::Instant::now();
        let all = load_all(&media, 0.0).expect("race lines at step 0");
        eprintln!("load_all(0): {:?}", t0.elapsed());
        assert_eq!(all.lines.iter().map(|l| l.pts.len()).sum::<usize>(), 514_463);
    }

    /// Race pins: 36 spheres + 1 touge-only locator + the start line for the rest, minus the routes
    /// that have no pin; every pin and every sphere / touge route has an `.owt`.
    #[test]
    fn real_install_race_pins() {
        let Some(media) = find_media(None) else {
            eprintln!("SKIP real_install_race_pins: FH6 install not found");
            return;
        };
        let pois = Pois::load(&media).expect("pois");
        let lines = load_all(&media, 5.0).expect("race lines").lines;
        let routes: std::collections::HashSet<u32> = lines.iter().map(|l| l.route).collect();
        for p in pois.of(PoiKind::RacePin).chain(pois.of(PoiKind::TougeEvent)) {
            assert!(routes.contains(&p.n), "{:?} {} has no .owt", p.kind, p.n);
        }
        let pins = race_pins(&pois, &lines);
        assert_eq!(pins.len(), 167, "170 routes minus 99, 102, 103");
        let n = |s| pins.iter().filter(|p| p.source == s).count();
        assert_eq!((n(PinSource::Sphere), n(PinSource::Touge), n(PinSource::StartLine)), (36, 1, 130));
        let pin = |r: u32| pins.iter().find(|p| p.route == r).unwrap();
        // user's examples (docs "Race map pins"): sphere pin, not the start line
        assert!((pin(1281).x + 2155.4).abs() < 0.1 && (pin(1281).z + 4744.1).abs() < 0.1, "{:?}", pin(1281));
        assert_eq!(pin(5411).source, PinSource::Touge);
        assert!(pins.iter().all(|p| p.x.is_finite() && p.z.is_finite() && (p.x != 0.0 || p.z != 0.0)));
    }
}
