//! BC7 (BPTC, "DXGI_FORMAT_BC7_UNORM") decoder, written by hand: no crate decodes it offline here
//! and the `image` crate has no BCn. Used for the FH6 map icons (swatchbin pixel format `0x09`, see
//! `docs/game-data/fh6-cars-names-icons.md`); BC1 for the map tiles lives in `tiles.rs`.
//!
//! One 128-bit block = 4×4 texels. The low bits of the block are the mode (unary: mode `k` is `k`
//! zero bits and a one), followed in LSB-first order by: partition number, rotation, index
//! selection, the endpoint channels (all R of every endpoint, then all G, B, A), the p-bits and
//! the 16 palette indices (the anchor index of each subset omits its MSB). The eight modes differ
//! in subset count, endpoint precision, p-bit style and index width ([`MODES`]).
//!
//! The partition tables (which subset a texel belongs to, and which texel is a subset's anchor)
//! are format constants. They are not derivable, so they were read out of a reference decoder
//! (Pillow's BC7 decoder fed probe blocks; `tools/fh6-extract/extract_icons.py` uses the same one)
//! and the 2-subset anchors cross-checked against the published table. A wrong entry would show
//! as a decode mismatch in the real-install test (`gamedata::icons`), which compares every icon
//! the app uses against Pillow's output.

/// Static per-mode parameters.
struct Mode {
    /// 1, 2 or 3 subsets.
    subsets: usize,
    partition_bits: u32,
    rotation_bits: u32,
    /// 1 for mode 4 (swaps which of the two index sets is colour / alpha).
    index_sel_bits: u32,
    /// Endpoint colour channel bits (before the p-bit).
    color_bits: u32,
    /// Endpoint alpha bits (0 = opaque).
    alpha_bits: u32,
    pbits: Pbits,
    /// Bits per index (of the first index set).
    index_bits: u32,
    /// Bits per index of the second set (mode 4: 3, mode 5: 2), else 0.
    index2_bits: u32,
}

#[derive(PartialEq, Clone, Copy)]
enum Pbits {
    None,
    /// One p-bit per endpoint.
    Unique,
    /// One p-bit per subset, shared by both its endpoints.
    Shared,
}

const MODES: [Mode; 8] = [
    Mode { subsets: 3, partition_bits: 4, rotation_bits: 0, index_sel_bits: 0, color_bits: 4, alpha_bits: 0, pbits: Pbits::Unique, index_bits: 3, index2_bits: 0 },
    Mode { subsets: 2, partition_bits: 6, rotation_bits: 0, index_sel_bits: 0, color_bits: 6, alpha_bits: 0, pbits: Pbits::Shared, index_bits: 3, index2_bits: 0 },
    Mode { subsets: 3, partition_bits: 6, rotation_bits: 0, index_sel_bits: 0, color_bits: 5, alpha_bits: 0, pbits: Pbits::None, index_bits: 2, index2_bits: 0 },
    Mode { subsets: 2, partition_bits: 6, rotation_bits: 0, index_sel_bits: 0, color_bits: 7, alpha_bits: 0, pbits: Pbits::Unique, index_bits: 2, index2_bits: 0 },
    Mode { subsets: 1, partition_bits: 0, rotation_bits: 2, index_sel_bits: 1, color_bits: 5, alpha_bits: 6, pbits: Pbits::None, index_bits: 2, index2_bits: 3 },
    Mode { subsets: 1, partition_bits: 0, rotation_bits: 2, index_sel_bits: 0, color_bits: 7, alpha_bits: 8, pbits: Pbits::None, index_bits: 2, index2_bits: 2 },
    Mode { subsets: 1, partition_bits: 0, rotation_bits: 0, index_sel_bits: 0, color_bits: 7, alpha_bits: 7, pbits: Pbits::Unique, index_bits: 4, index2_bits: 0 },
    Mode { subsets: 2, partition_bits: 6, rotation_bits: 0, index_sel_bits: 0, color_bits: 5, alpha_bits: 5, pbits: Pbits::Unique, index_bits: 2, index2_bits: 0 },
];

/// Interpolation weights (out of 64) for 2-, 3- and 4-bit indices.
const WEIGHTS2: [u32; 4] = [0, 21, 43, 64];
const WEIGHTS3: [u32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const WEIGHTS4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

fn weight(bits: u32, idx: u32) -> u32 {
    match bits {
        2 => WEIGHTS2[idx as usize],
        3 => WEIGHTS3[idx as usize],
        _ => WEIGHTS4[idx as usize],
    }
}

/// 2-subset partitions: bit `i` = subset (0/1) of texel `i` (row-major).
const PART2: [u16; 64] = [
    0xcccc, 0x8888, 0xeeee, 0xecc8, 0xc880, 0xfeec, 0xfec8, 0xec80,
    0xc800, 0xffec, 0xfe80, 0xe800, 0xffe8, 0xff00, 0xfff0, 0xf000,
    0xf710, 0x008e, 0x7100, 0x08ce, 0x008c, 0x7310, 0x3100, 0x8cce,
    0x088c, 0x3110, 0x6666, 0x366c, 0x17e8, 0x0ff0, 0x718e, 0x399c,
    0xaaaa, 0xf0f0, 0x5a5a, 0x33cc, 0x3c3c, 0x55aa, 0x9696, 0xa55a,
    0x73ce, 0x13c8, 0x324c, 0x3bdc, 0x6996, 0xc33c, 0x9966, 0x0660,
    0x0272, 0x04e4, 0x4e40, 0x2720, 0xc936, 0x936c, 0x39c6, 0x639c,
    0x9336, 0x9cc6, 0x817e, 0xe718, 0xccf0, 0x0fcc, 0x7744, 0xee22,
];

/// 3-subset partitions: bits `2i..2i+2` = subset (0/1/2) of texel `i`.
const PART3: [u32; 64] = [
    0xaa685050, 0x6a5a5040, 0x5a5a4200, 0x5450a0a8, 0xa5a50000, 0xa0a05050, 0x5555a0a0, 0x5a5a5050,
    0xaa550000, 0xaa555500, 0xaaaa5500, 0x90909090, 0x94949494, 0xa4a4a4a4, 0xa9a59450, 0x2a0a4250,
    0xa5945040, 0x0a425054, 0xa5a5a500, 0x55a0a0a0, 0xa8a85454, 0x6a6a4040, 0xa4a45000, 0x1a1a0500,
    0x0050a4a4, 0xaaa59090, 0x14696914, 0x69691400, 0xa08585a0, 0xaa821414, 0x50a4a450, 0x6a5a0200,
    0xa9a58000, 0x5090a0a8, 0xa8a09050, 0x24242424, 0x00aa5500, 0x24924924, 0x24499224, 0x50a50a50,
    0x500aa550, 0xaaaa4444, 0x66660000, 0xa5a0a5a0, 0x50a050a0, 0x69286928, 0x44aaaa44, 0x66666600,
    0xaa444444, 0x54a854a8, 0x95809580, 0x96969600, 0xa85454a8, 0x80959580, 0xaa141414, 0x96960000,
    0xaaaa1414, 0xa05050a0, 0xa0a5a5a0, 0x96000000, 0x40804080, 0xa9a8a9a8, 0xaaaaaa44, 0x2a4a5254,
];

/// Anchor texel of subset 1 in the 2-subset partitions (subset 0's anchor is always texel 0).
const ANCHOR2: [u8; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15,
    15, 15, 15, 15, 15, 15, 15, 15,
    15, 2, 8, 2, 2, 8, 8, 15,
    2, 8, 2, 2, 8, 8, 2, 2,
    15, 15, 6, 8, 2, 8, 15, 15,
    2, 8, 2, 2, 2, 15, 15, 6,
    6, 2, 6, 8, 15, 15, 2, 2,
    15, 15, 15, 15, 15, 2, 2, 15,
];

/// Anchor texels of subsets 1 and 2 in the 3-subset partitions.
const ANCHOR3_1: [u8; 64] = [
    3, 3, 15, 15, 8, 3, 15, 15,
    8, 8, 6, 6, 6, 5, 3, 3,
    3, 3, 8, 15, 3, 3, 6, 10,
    5, 8, 8, 6, 8, 5, 15, 15,
    8, 15, 3, 5, 6, 10, 8, 15,
    15, 3, 15, 5, 15, 15, 15, 15,
    3, 15, 5, 5, 5, 8, 5, 10,
    5, 10, 8, 13, 15, 12, 3, 3,
];
const ANCHOR3_2: [u8; 64] = [
    15, 8, 8, 3, 15, 15, 3, 8,
    15, 15, 15, 15, 15, 15, 15, 8,
    15, 8, 15, 3, 15, 8, 15, 8,
    3, 15, 6, 10, 15, 15, 10, 8,
    15, 3, 15, 10, 10, 8, 9, 10,
    6, 15, 8, 15, 3, 6, 6, 8,
    15, 3, 15, 15, 15, 15, 15, 15,
    15, 15, 15, 15, 3, 15, 15, 8,
];

/// Subset of texel `i` under `partition` for a mode with `subsets` subsets.
fn subset_of(subsets: usize, partition: usize, i: usize) -> usize {
    match subsets {
        1 => 0,
        2 => ((PART2[partition] >> i) & 1) as usize,
        _ => ((PART3[partition] >> (2 * i)) & 3) as usize,
    }
}

/// Whether texel `i` is the anchor of its subset (its index has one bit less).
fn is_anchor(subsets: usize, partition: usize, i: usize) -> bool {
    match subsets {
        1 => i == 0,
        2 => i == 0 || i == ANCHOR2[partition] as usize,
        _ => i == 0 || i == ANCHOR3_1[partition] as usize || i == ANCHOR3_2[partition] as usize,
    }
}

/// Decode one 16-byte block into 16 RGBA texels (row-major). A reserved mode (first byte 0)
/// decodes to transparent black, as the specification says.
pub fn decode_block(block: &[u8; 16]) -> [[u8; 4]; 16] {
    let v = u128::from_le_bytes(*block);
    let mode_n = v.trailing_zeros() as usize;
    if mode_n >= 8 {
        return [[0; 4]; 16];
    }
    let m = &MODES[mode_n];
    let mut pos = mode_n as u32 + 1;
    let mut get = |n: u32| -> u32 {
        let r = ((v >> pos) & ((1u128 << n) - 1)) as u32;
        pos += n;
        r
    };
    let partition = get(m.partition_bits) as usize;
    let rotation = get(m.rotation_bits);
    let index_sel = get(m.index_sel_bits);
    let n_ep = m.subsets * 2;
    let mut ep = [[0u32; 4]; 6];
    for ch in 0..3 {
        for e in ep.iter_mut().take(n_ep) {
            e[ch] = get(m.color_bits);
        }
    }
    if m.alpha_bits > 0 {
        for e in ep.iter_mut().take(n_ep) {
            e[3] = get(m.alpha_bits);
        }
    }
    let mut p = [0u32; 6];
    match m.pbits {
        Pbits::Unique => p.iter_mut().take(n_ep).for_each(|b| *b = get(1)),
        Pbits::Shared => {
            for s in 0..m.subsets {
                let b = get(1);
                p[2 * s] = b;
                p[2 * s + 1] = b;
            }
        }
        Pbits::None => {}
    }
    // Expand every endpoint to 8 bits (p-bit appended below the channel bits, then the top bits
    // replicated into the low ones).
    let has_p = m.pbits != Pbits::None;
    let expand = |val: u32, bits: u32, pb: u32| -> u32 {
        let (val, prec) = if has_p { ((val << 1) | pb, bits + 1) } else { (val, bits) };
        (val << (8 - prec)) | (val >> (2 * prec - 8))
    };
    let mut pts = [[0u32; 4]; 6];
    for e in 0..n_ep {
        for ch in 0..3 {
            pts[e][ch] = expand(ep[e][ch], m.color_bits, p[e]);
        }
        pts[e][3] = if m.alpha_bits > 0 { expand(ep[e][3], m.alpha_bits, p[e]) } else { 255 };
    }
    // Indices. The first set uses `index_bits`, anchors one bit less; modes 4/5 have a second set
    // (a single subset, so only texel 0 is an anchor there too).
    let mut idx1 = [0u32; 16];
    for (i, ix) in idx1.iter_mut().enumerate() {
        let bits = m.index_bits - u32::from(is_anchor(m.subsets, partition, i));
        *ix = get(bits);
    }
    let mut idx2 = idx1;
    if m.index2_bits > 0 {
        for (i, ix) in idx2.iter_mut().enumerate() {
            *ix = get(m.index2_bits - u32::from(i == 0));
        }
    }
    // Which set / width drives colour and alpha. Mode 4 with index_sel = 1 swaps them.
    let (c_idx, c_bits, a_idx, a_bits) = if m.index2_bits == 0 {
        (&idx1, m.index_bits, &idx1, m.index_bits)
    } else if index_sel == 0 {
        (&idx1, m.index_bits, &idx2, m.index2_bits)
    } else {
        (&idx2, m.index2_bits, &idx1, m.index_bits)
    };
    let mut out = [[0u8; 4]; 16];
    for (i, o) in out.iter_mut().enumerate() {
        let s = subset_of(m.subsets, partition, i);
        let (e0, e1) = (pts[2 * s], pts[2 * s + 1]);
        let (cw, aw) = (weight(c_bits, c_idx[i]), weight(a_bits, a_idx[i]));
        let mut px = [0u32; 4];
        for ch in 0..3 {
            px[ch] = (e0[ch] * (64 - cw) + e1[ch] * cw + 32) >> 6;
        }
        px[3] = (e0[3] * (64 - aw) + e1[3] * aw + 32) >> 6;
        match rotation {
            1 => px.swap(0, 3),
            2 => px.swap(1, 3),
            3 => px.swap(2, 3),
            _ => {}
        }
        *o = [px[0] as u8, px[1] as u8, px[2] as u8, px[3] as u8];
    }
    out
}

/// Decode a `w × h` BC7 image into `dst` (RGBA8, row stride `stride` px) with its top-left at
/// (`x0`, `y0`). `w` and `h` need not be multiples of 4 (icons are e.g. 257 px): `data` holds
/// `ceil(w/4) × ceil(h/4)` blocks, row-major, and texels past the edge are dropped. `data` must be
/// that long (the caller checks, see `tiles::parse_swatch`).
pub fn decode_bc7_into(data: &[u8], w: usize, h: usize, dst: &mut [u8], stride: usize, x0: usize, y0: usize) {
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * 16;
            let blk: &[u8; 16] = data[off..off + 16].try_into().unwrap();
            let px = decode_block(blk);
            for (i, p) in px.iter().enumerate() {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < w && y < h {
                    let o = ((y0 + y) * stride + x0 + x) * 4;
                    dst[o..o + 4].copy_from_slice(p);
                }
            }
        }
    }
}

/// Decode only the rectangle (`rx`, `ry`, `rw`, `rh`) of a BC7 image `w` px wide into tightly
/// packed RGBA8 (`rw * rh * 4` bytes). All four values must be multiples of 4 (block aligned),
/// and `data` must cover the rectangle. Used to cut one cell out of the 2048×1024 icon sheet
/// without decoding the other ~90 % of it.
pub fn decode_bc7_region(data: &[u8], w: usize, rx: usize, ry: usize, rw: usize, rh: usize) -> Vec<u8> {
    debug_assert!(rx % 4 == 0 && ry % 4 == 0 && rw % 4 == 0 && rh % 4 == 0);
    let bw = w.div_ceil(4);
    let mut out = vec![0u8; rw * rh * 4];
    for by in 0..rh / 4 {
        for bx in 0..rw / 4 {
            let off = ((ry / 4 + by) * bw + rx / 4 + bx) * 16;
            let px = decode_block(data[off..off + 16].try_into().unwrap());
            for (i, p) in px.iter().enumerate() {
                let o = ((by * 4 + i / 4) * rw + bx * 4 + i % 4) * 4;
                out[o..o + 4].copy_from_slice(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A region decode equals the same rectangle cut out of the full decode.
    #[test]
    fn region_matches_full_decode() {
        // 16x12 image = 4x3 blocks of pseudo-random bytes (all eight modes show up)
        let mut s = 0x1234_5678u32;
        let mut data = vec![0u8; 12 * 16];
        for b in data.iter_mut() {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            *b = (s >> 24) as u8;
        }
        for (i, blk) in data.chunks_mut(16).enumerate() {
            blk[0] = (blk[0] & 0xf0) | (1 << (i % 8).min(7)); // force a different mode per block
        }
        let mut full = vec![0u8; 16 * 12 * 4];
        decode_bc7_into(&data, 16, 12, &mut full, 16, 0, 0);
        let part = decode_bc7_region(&data, 16, 4, 4, 8, 8);
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(part[(y * 8 + x) * 4..][..4], full[((y + 4) * 16 + x + 4) * 4..][..4], "({x},{y})");
            }
        }
    }

    /// LSB-first bit writer for building test blocks.
    #[derive(Default)]
    struct W {
        v: u128,
        n: u32,
    }
    impl W {
        fn put(&mut self, val: u32, bits: u32) -> &mut Self {
            self.v |= ((val as u128) & ((1u128 << bits) - 1)) << self.n;
            self.n += bits;
            self
        }
        fn block(&self) -> [u8; 16] {
            assert_eq!(self.n, 128, "a block is exactly 128 bits");
            self.v.to_le_bytes()
        }
    }

    fn texels(b: &W) -> [[u8; 4]; 16] {
        decode_block(&b.block())
    }

    /// Mode 6 (1 subset, RGBA 7.7.7.7 + unique p-bits, 4-bit indices): the simplest full block.
    #[test]
    fn mode6_endpoints_and_interpolation() {
        let mut w = W::default();
        w.put(1 << 6, 7); // mode 6
        // per channel (e0, e1): R (0, 127), G (127, 0), B (0, 127), A (127, 0); p-bits e0 = 1, e1 = 0.
        for (a, b) in [(0, 127), (127, 0), (0, 127), (127, 0)] {
            w.put(a, 7).put(b, 7);
        }
        w.put(1, 1).put(0, 1);
        // indices: the anchor (texel 0) has 3 bits, the rest 4. texel 0 -> 0, texel 1 -> 15, texel 2 -> 7.
        w.put(0, 3).put(15, 4).put(7, 4);
        for _ in 3..16 {
            w.put(0, 4);
        }
        let t = texels(&w);
        // 7 bits + p-bit = 8 bits exactly: e0 = (1, 255, 1, 255), e1 = (254, 0, 254, 0)
        assert_eq!(t[0], [1, 255, 1, 255]);
        assert_eq!(t[1], [254, 0, 254, 0]);
        // texel 2: 4-bit index 7 = weight 30
        let rb = ((1 * 34 + 254 * 30 + 32) >> 6) as u8;
        let ga = ((255 * 34 + 32) >> 6) as u8;
        assert_eq!(t[2], [rb, ga, rb, ga]);
        assert_eq!(t[3], t[0]);
    }

    /// Mode 3 (2 subsets, partition 0 = left two columns / right two columns): RGB 7.7.7 + unique
    /// p-bits, 2-bit indices with the anchor of subset 1 at texel 15.
    #[test]
    fn mode3_two_subsets_partition0() {
        let mut w = W::default();
        w.put(1 << 3, 4).put(0, 6); // mode 3, partition 0
        // R, G, B of the endpoints [s0e0, s0e1, s1e0, s1e1]
        for ch in 0..3 {
            for e in 0..4 {
                let v = match (ch, e) {
                    (0, 1) => 127, // subset 0: red 0 -> 127
                    (1, 2) => 127, // subset 1: green 127 -> 0
                    _ => 0,
                };
                w.put(v, 7);
            }
        }
        for _ in 0..4 {
            w.put(0, 1); // p-bits
        }
        // indices: texels 0 and 15 are anchors (1 bit), the others 2 bits; texel 0 -> 0, everything else all ones.
        for i in 0..16 {
            w.put(if i == 0 { 0 } else { 3 }, if i == 0 || i == 15 { 1 } else { 2 });
        }
        let t = texels(&w);
        assert_eq!(t[0], [0, 0, 0, 255]);
        assert_eq!(t[1], [254, 0, 0, 255], "subset 0, index 3 = e1");
        assert_eq!(t[2], [0, 0, 0, 255], "subset 1, index 3 = e1 (green 0)");
        let g15 = ((254 * (64 - 21) + 32) >> 6) as u8; // anchor: 1-bit index 1 = weight 21
        assert_eq!(t[15], [0, g15, 0, 255]);
    }

    /// Mode 5 with rotation 3 (A <-> B swapped after decode); separate colour and alpha indices.
    #[test]
    fn mode5_rotation_swaps_alpha_and_blue() {
        let mut w = W::default();
        w.put(1 << 5, 6).put(3, 2); // mode 5, rotation 3
        // colour endpoints (7 bits, 127 -> 255): R (127, 0), G (0, 0), B (0, 127); alpha (8 bits): 200, 0.
        for (a, b) in [(127, 0), (0, 0), (0, 127)] {
            w.put(a, 7).put(b, 7);
        }
        w.put(200, 8).put(0, 8);
        // colour indices (2 bits, texel 0 anchor 1 bit): texel 0 -> 0, texel 1 -> 3, others 0
        w.put(0, 1).put(3, 2);
        for _ in 2..16 {
            w.put(0, 2);
        }
        // alpha indices: texels 0, 1 -> 0, texel 2 -> 3
        w.put(0, 1).put(0, 2).put(3, 2);
        for _ in 3..16 {
            w.put(0, 2);
        }
        let t = texels(&w);
        // before rotation: (255,0,0,200), (0,0,255,200), (255,0,0,0); rotation 3 swaps B and A
        assert_eq!(t[0], [255, 0, 200, 0]);
        assert_eq!(t[1], [0, 0, 200, 255]);
        assert_eq!(t[2], [255, 0, 0, 0]);
    }

    /// Mode 4 index selection: with the bit set, colour uses the 3-bit set and alpha the 2-bit one.
    #[test]
    fn mode4_index_selection_swaps_the_sets() {
        let build = |sel: u32| {
            let mut w = W::default();
            w.put(1 << 4, 5).put(0, 2).put(sel, 1); // mode 4, no rotation
            // colour (5 bits): e0 = 0, e1 = 31 (-> 255); alpha (6 bits): e0 = 0, e1 = 63 (-> 255)
            for _ in 0..3 {
                w.put(0, 5).put(31, 5);
            }
            w.put(0, 6).put(63, 6);
            // 2-bit set: texel 0 (anchor, 1 bit) -> 0, texel 1 -> 3, rest 0
            w.put(0, 1).put(3, 2);
            for _ in 2..16 {
                w.put(0, 2);
            }
            // 3-bit set: texel 0 (2 bits) -> 0, texel 1 -> 4, rest 0
            w.put(0, 2).put(4, 3);
            for _ in 2..16 {
                w.put(0, 3);
            }
            texels(&w)
        };
        let w37 = ((255 * 37 + 32) >> 6) as u8;
        let a = build(0)[1]; // colour from the 2-bit set (full), alpha from the 3-bit set (weight 37)
        assert_eq!((a[0], a[3]), (255, w37));
        let b = build(1)[1]; // swapped
        assert_eq!((b[0], b[3]), (w37, 255));
    }

    /// Mode 2 (3 subsets): partition 0 has anchors at texels 0, 3 and 15.
    #[test]
    fn mode2_three_subsets_and_anchors() {
        let mut w = W::default();
        w.put(1 << 2, 3).put(0, 6); // mode 2, partition 0
        // R of the 6 endpoints (5 bits): every subset goes 0 -> 31 (255); G, B zero
        for e in 0..6 {
            w.put(if e % 2 == 0 { 0 } else { 31 }, 5);
        }
        for _ in 0..12 {
            w.put(0, 5);
        }
        // 2-bit indices, anchors (0, 3, 15) 1 bit; all ones everywhere
        for i in 0..16 {
            w.put(u32::MAX, if matches!(i, 0 | 3 | 15) { 1 } else { 2 });
        }
        let t = texels(&w);
        let anchor = ((255 * 21 + 32) >> 6) as u8; // 1-bit index 1 = weight 21
        assert_eq!((t[0][0], t[3][0], t[15][0]), (anchor, anchor, anchor));
        assert_eq!((t[1][0], t[8][0]), (255, 255), "non-anchor texels read index 3");
        assert_eq!(t[0][3], 255);
    }

    /// Modes 0, 1 and 7: endpoint expansion with their p-bit styles.
    #[test]
    fn modes_0_1_7_endpoint_expansion() {
        // mode 0: 4-bit 0b1000 + p-bit 1 = 0b10001 -> 8 bit 0b10001_100 = 140
        let mut w = W::default();
        w.put(1, 1).put(0, 4);
        for _ in 0..18 {
            w.put(0b1000, 4);
        }
        for _ in 0..6 {
            w.put(1, 1);
        }
        for i in 0..16 {
            w.put(0, if matches!(i, 0 | 3 | 15) { 2 } else { 3 }); // anchors of 3-subset partition 0
        }
        assert_eq!(texels(&w)[5], [140, 140, 140, 255]);
        // mode 1: 6-bit 0b100000 + shared p-bit 1 = 0b1000001 -> 8 bit 0b10000011 = 131
        let mut w = W::default();
        w.put(1 << 1, 2).put(0, 6);
        for _ in 0..12 {
            w.put(0b100000, 6);
        }
        w.put(1, 1).put(1, 1);
        for i in 0..16 {
            w.put(0, if i == 0 || i == 15 { 2 } else { 3 });
        }
        assert_eq!(texels(&w)[5], [131, 131, 131, 255]);
        // mode 7: 5-bit 0b10000 + p-bit 0 = 0b100000 -> 8 bit 0b10000010 = 130, alpha too
        let mut w = W::default();
        w.put(1 << 7, 8).put(0, 6);
        for _ in 0..16 {
            w.put(0b10000, 5);
        }
        for _ in 0..4 {
            w.put(0, 1);
        }
        for i in 0..16 {
            w.put(0, if i == 0 || i == 15 { 1 } else { 2 });
        }
        assert_eq!(texels(&w)[5], [130, 130, 130, 130]);
    }

    #[test]
    fn reserved_mode_is_transparent_black() {
        assert_eq!(decode_block(&[0; 16]), [[0; 4]; 16]);
    }

    /// Texels past a non-multiple-of-4 size are dropped, and the destination offset / stride apply.
    #[test]
    fn decode_into_clips_and_offsets() {
        // Four mode-6 blocks, each flat via e0 == e1 and index 0: R = 2 * (k * 8) (7 bits + p-bit 0).
        let mut data = Vec::new();
        for k in 0..4u32 {
            let mut w = W::default();
            w.put(1 << 6, 7);
            for ch in 0..4 {
                let v = if ch == 0 { k * 8 } else { 0 };
                w.put(v, 7).put(v, 7);
            }
            w.put(0, 2); // p-bits
            for i in 0..16 {
                w.put(0, if i == 0 { 3 } else { 4 });
            }
            data.extend_from_slice(&w.block());
        }
        // 6x6 image = 2x2 blocks, written at (1, 1) of an 8-wide buffer
        let mut dst = vec![9u8; 8 * 8 * 4];
        decode_bc7_into(&data, 6, 6, &mut dst, 8, 1, 1);
        let px = |x: usize, y: usize| dst[(y * 8 + x) * 4];
        assert_eq!(px(1, 1), 0); // block 0
        assert_eq!(px(5, 1), 16); // block 1 (image x = 4)
        assert_eq!(px(1, 5), 32); // block 2
        assert_eq!(px(6, 6), 48); // image texel (5, 5), block 3
        assert_eq!(px(7, 7), 9, "image texel (6,6) is outside the 6x6 image");
        assert_eq!(px(0, 0), 9);
    }
}
