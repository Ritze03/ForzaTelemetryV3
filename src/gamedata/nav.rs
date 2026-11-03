//! The open-world road graph: `<media>/OpenWorld/Brio/Freeroam/Brio_00.nav` (`WVAN` file),
//! a port of `tools/fh6-extract/decode_nav.py`. See `docs/game-data/fh6-map-tooling.md`.
//!
//! Layout (little-endian): `0x00` `"WVAN"`; `u32[14]` at `0x58` of which `[0]` = node count `N`,
//! `[1]` = road count `NR`, `[2]` = node-index list length `NA`. Nodes: `N × 48 B` at `0x90`
//! (`f32 pos[3]` = x, **y = height**, z; `f32 up[3]`; `u16` at +24 = the stable node id, unique;
//! then 20 B we ignore). Road table at `0x90 + 48N`: `NR-1` records of 24 B
//! `{u32 count, u32 flags, u64, u64}` then a final 8 B `{u32 count, u32 flags}`; class =
//! `flags & 0xffff` (4/5/6/8), `hi = flags >> 16`. Right after it `u64[NA]` node indices; road
//! `r` takes the next `count_r` of them.
//!
//! Roads are split into polylines wherever two consecutive nodes are more than 60 m apart
//! (3D distance: those are jump links, not road); pieces with fewer than 2 nodes are dropped.
//! Reference numbers (install of 2026-10): 38 473 nodes, 1 532 roads → 1 544 polylines,
//! 39 383 unique edges, 0 orphans, SHA-1 `a88c69f4…4202`.
//!
//! The file is 3.4 MB; [`Nav::load`] takes ~20 ms, so it may run anywhere but the UI frame loop.

use std::collections::HashSet;
use std::path::Path;

use sha1::{Digest, Sha1};

use super::install::ci;

/// File name in the install and in the road-type file's `nav.file`.
pub const NAV_FILE: &str = "Brio_00.nav";
/// Two consecutive road nodes further apart than this (m, 3D) are not one polyline.
pub const SPLIT_M: f32 = 60.0;

/// One polyline vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavVert {
    /// Stable node id (the `u16` at +24 of the node record); what road-type files refer to.
    pub id: u32,
    pub x: f32,
    pub z: f32,
    /// Height (the nav file's `y`).
    pub y: f32,
}

#[derive(Clone, Debug)]
pub struct Nav {
    /// SHA-1 (lower-case hex) of the whole file: identifies the nav version.
    pub sha1: String,
    /// Number of nodes in the file.
    pub nodes: usize,
    /// Road polylines in file order (the editor's edge order derives from this).
    pub polys: Vec<Vec<NavVert>>,
    /// Road class per polyline (`flags & 0xffff`).
    pub cls: Vec<u16>,
    /// `flags >> 16` per polyline.
    pub hi: Vec<u16>,
    /// Nodes that appear in no polyline: `(id, x, z)`.
    pub orphans: Vec<(u32, f32, f32)>,
}

impl Nav {
    /// Read `OpenWorld/Brio/Freeroam/Brio_00.nav` under `media` and parse it.
    pub fn load(media: &Path) -> Result<Nav, String> {
        let path = ci(media, &format!("OpenWorld/Brio/Freeroam/{NAV_FILE}"))
            .filter(|p| p.is_file())
            .ok_or_else(|| format!("no {NAV_FILE} in the install"))?;
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&bytes)
    }

    /// Parse the bytes of a `.nav` file.
    pub fn parse(d: &[u8]) -> Result<Nav, String> {
        if d.len() < 0x90 || &d[0..4] != b"WVAN" {
            return Err("not a WVAN nav file".into());
        }
        let u32at = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
        let f32at = |o: usize| f32::from_bits(u32at(o));
        let (n, nr, na) = (u32at(0x58) as usize, u32at(0x5c) as usize, u32at(0x60) as usize);
        let nodes_off = 0x90;
        let rt_off = nodes_off + 48 * n;
        if nr == 0 || n > 10_000_000 || nr > 10_000_000 || na > 100_000_000 {
            return Err("implausible nav header".into());
        }
        let a_off = rt_off + (nr - 1) * 24 + 8;
        if d.len() < a_off + 8 * na {
            return Err("nav file truncated".into());
        }
        let pos = |i: usize| (f32at(nodes_off + 48 * i), f32at(nodes_off + 48 * i + 4), f32at(nodes_off + 48 * i + 8)); // x, y (height), z
        let nid = |i: usize| u16::from_le_bytes([d[nodes_off + 48 * i + 24], d[nodes_off + 48 * i + 25]]) as u32;
        let mut ids = HashSet::with_capacity(n);
        if !(0..n).all(|i| ids.insert(nid(i))) {
            return Err("duplicate node ids".into());
        }
        let (cnt, flags): (Vec<usize>, Vec<u32>) = (0..nr).map(|r| (u32at(rt_off + r * 24) as usize, u32at(rt_off + r * 24 + 4))).unzip();
        if cnt.iter().sum::<usize>() != na {
            return Err("road table does not sum to the node list".into());
        }
        let mut polys: Vec<Vec<NavVert>> = Vec::new();
        let (mut cls, mut hi) = (Vec::new(), Vec::new());
        let mut used = vec![false; n];
        let mut s = 0;
        for (&c, &f) in cnt.iter().zip(&flags) {
            let mut idx = Vec::with_capacity(c);
            for k in 0..c {
                let o = a_off + 8 * (s + k);
                let i = u64::from_le_bytes(d[o..o + 8].try_into().unwrap()) as usize;
                if i >= n {
                    return Err(format!("road node index {i} out of range"));
                }
                idx.push(i);
            }
            s += c;
            let mut flush = |cur: &mut Vec<usize>| {
                if cur.len() > 1 {
                    polys.push(
                        cur.iter()
                            .map(|&i| {
                                used[i] = true;
                                let (x, y, z) = pos(i);
                                NavVert { id: nid(i), x, z, y }
                            })
                            .collect(),
                    );
                    cls.push((f & 0xffff) as u16);
                    hi.push((f >> 16) as u16);
                }
                cur.clear();
            };
            let Some(&first) = idx.first() else { continue };
            let mut cur = vec![first];
            for w in idx.windows(2) {
                let (a, b) = (pos(w[0]), pos(w[1]));
                let dist = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt();
                if dist > SPLIT_M {
                    flush(&mut cur);
                }
                cur.push(w[1]);
            }
            flush(&mut cur);
        }
        let orphans = (0..n)
            .filter(|&i| !used[i])
            .map(|i| {
                let (x, _, z) = pos(i);
                (nid(i), x, z)
            })
            .collect();
        let sha1 = Sha1::digest(d).iter().map(|b| format!("{b:02x}")).collect();
        Ok(Nav { sha1, nodes: n, polys, cls, hi, orphans })
    }

    /// Unique undirected edges `(min id, max id)` of the polylines, in first-seen order. This
    /// order is the editor's edge order and the order of `types` in the project road-type file.
    pub fn edges(&self) -> Vec<(u32, u32)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for p in &self.polys {
            for w in p.windows(2) {
                let e = (w[0].id.min(w[1].id), w[0].id.max(w[1].id));
                if seen.insert(e) {
                    out.push(e);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::install::find_media;

    /// A synthetic nav: 5 nodes, 2 roads. Road 0 = nodes 0-1-2 (class 5, 10 m apart); road 1
    /// = nodes 2-3 (class 4) which are 100 m apart -> split into two 1-node pieces -> dropped.
    /// Node 4 is in no road -> orphan. Node ids are 10, 11, 12, 13, 14.
    fn synthetic() -> Vec<u8> {
        let (n, nr, na) = (5usize, 2usize, 5usize);
        let rt_off = 0x90 + 48 * n;
        let a_off = rt_off + (nr - 1) * 24 + 8;
        let mut d = vec![0u8; a_off + 8 * na];
        d[0..4].copy_from_slice(b"WVAN");
        for (k, v) in [n as u32, nr as u32, na as u32].into_iter().enumerate() {
            d[0x58 + 4 * k..][..4].copy_from_slice(&v.to_le_bytes());
        }
        let nodes = [(0.0f32, 1.0f32, 0.0f32), (10.0, 2.0, 0.0), (20.0, 3.0, 0.0), (120.0, 3.0, 0.0), (500.0, 5.0, 500.0)];
        for (i, (x, y, z)) in nodes.into_iter().enumerate() {
            let o = 0x90 + 48 * i;
            for (k, v) in [x, y, z].into_iter().enumerate() {
                d[o + 4 * k..][..4].copy_from_slice(&v.to_le_bytes());
            }
            d[o + 24..][..2].copy_from_slice(&(10 + i as u16).to_le_bytes());
        }
        // road table: record 0 (24 B): count 3, flags 5 | 7<<16; last record (8 B): count 2, flags 4
        d[rt_off..][..4].copy_from_slice(&3u32.to_le_bytes());
        d[rt_off + 4..][..4].copy_from_slice(&(5u32 | 7 << 16).to_le_bytes());
        d[rt_off + 24..][..4].copy_from_slice(&2u32.to_le_bytes());
        d[rt_off + 28..][..4].copy_from_slice(&4u32.to_le_bytes());
        for (k, i) in [0u64, 1, 2, 2, 3].into_iter().enumerate() {
            d[a_off + 8 * k..][..8].copy_from_slice(&i.to_le_bytes());
        }
        d
    }

    #[test]
    fn synthetic_nav_splits_and_orphans() {
        let nav = Nav::parse(&synthetic()).unwrap();
        assert_eq!(nav.nodes, 5);
        assert_eq!(nav.polys.len(), 1);
        assert_eq!(nav.polys[0].iter().map(|v| v.id).collect::<Vec<_>>(), vec![10, 11, 12]);
        assert_eq!((nav.cls[0], nav.hi[0]), (5, 7));
        assert_eq!(nav.polys[0][1], NavVert { id: 11, x: 10.0, z: 0.0, y: 2.0 });
        assert_eq!(nav.edges(), vec![(10, 11), (11, 12)]);
        // nodes 3 (jump piece dropped) and 4 are in no kept polyline
        assert_eq!(nav.orphans.iter().map(|o| o.0).collect::<Vec<_>>(), vec![13, 14]);
        assert_eq!(nav.sha1.len(), 40);
    }

    #[test]
    fn malformed_nav_is_err() {
        assert!(Nav::parse(b"WVAN").is_err());
        assert!(Nav::parse(&[0u8; 0x100]).is_err());
        let mut d = synthetic();
        d.truncate(d.len() - 8);
        assert!(Nav::parse(&d).is_err());
        let mut d = synthetic();
        d[0x90 + 48 * 3 + 24..][..2].copy_from_slice(&10u16.to_le_bytes()); // duplicate id
        assert!(Nav::parse(&d).is_err());
    }

    /// Reference numbers of `decode_nav.py` on the real nav file.
    #[test]
    fn real_install_nav_reference() {
        let Some(media) = find_media(None) else {
            eprintln!("SKIP real_install_nav_reference: FH6 install not found");
            return;
        };
        let nav = Nav::load(&media).expect("nav");
        assert_eq!(nav.sha1, "a88c69f49c16e8aa86b883cd978328d737144202");
        assert_eq!(nav.nodes, 38473);
        assert_eq!(nav.polys.len(), 1544); // 1532 roads + jump splits
        assert_eq!(nav.edges().len(), 39383);
        assert!(nav.orphans.is_empty());
        let mut hist = std::collections::BTreeMap::new();
        for c in &nav.cls {
            *hist.entry(*c).or_insert(0usize) += 1;
        }
        assert_eq!(hist, [(4u16, 194usize), (5, 1091), (6, 257), (8, 2)].into_iter().collect());
        let p0 = &nav.polys[0];
        assert_eq!(p0.iter().take(7).map(|v| v.id).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5, 6, 7]);
        assert!((p0[0].x - 3097.3).abs() < 0.05 && (p0[0].z - 586.6).abs() < 0.05, "{:?}", p0[0]);
        assert!((p0[0].y - 150.96).abs() < 0.05, "{:?}", p0[0]);
    }
}
