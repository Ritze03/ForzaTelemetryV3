//! FH6 overworld map tiles, read at runtime from the user's own install: the season zips
//! `<media>/UI/Textures/Data_Bound/Map_Brio_<Season>.zip` hold a tile pyramid of `swatchbin`
//! ('burG' container, BC1-compressed) files. See `docs/game-data/fh6-game-files.md` and
//! `docs/features/minimap.md`.
//!
//! Level `L` has `2^L × 2^L` tiles named `<L>-<row>-<col>.swatchbin` (row = y), 1024 px each, so
//! level 3 is the 8192² map and level 2 the 4096² one. BC1 is decoded by hand (~40 lines, no
//! crate): it is the only format the map tiles use.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::install::ci;

/// Edge length of one tile, px.
pub const TILE_PX: usize = 1024;
/// Finest pyramid level: 8 × 8 tiles = 8192² px.
pub const MAX_LEVEL: u32 = 3;

/// Why a map could not be loaded from the install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapLoadError {
    /// No Forza Horizon 6 install found (no configured folder, env override or Steam copy).
    NoInstall,
    /// The install exists but a file in it can't be opened (permissions, protected Store install).
    NotReadable(PathBuf),
    /// The install has no map zip for this season (name given).
    MissingZip(String),
    /// The zip or a tile in it isn't what was expected.
    Decode(String),
}

impl std::fmt::Display for MapLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoInstall => write!(f, "no Forza Horizon 6 install found"),
            Self::NotReadable(p) => write!(f, "can't read {}", p.display()),
            Self::MissingZip(s) => write!(f, "no map zip for {s} in the install"),
            Self::Decode(e) => write!(f, "map tile decode failed: {e}"),
        }
    }
}

impl std::error::Error for MapLoadError {}

/// `<media>/UI/Textures/Data_Bound/Map_Brio_<season>.zip` (on-disk case is mixed). `season` is
/// `Spring`, `Summer`, `Autumn` or `Winter`.
pub fn season_zip(media: &Path, season: &str) -> Option<PathBuf> {
    let dir = ci(media, "UI/Textures/Data_Bound")?;
    ci(&dir, &format!("Map_Brio_{season}.zip")).filter(|p| p.is_file())
}

/// A swatchbin ('burG' container) → `(width, height, BC1 data of the top mip)`.
///
/// Header, all little-endian u32: `0x00` magic `burG`, `0x08` header size (140), `0x0c` total
/// size (must equal the file length), `0x4c` width, `0x50` height, `0x74` pixel format (0 = BC1),
/// `0x80` top-mip byte size; the pixel data is at `[header size .. + top-mip size]`.
pub fn parse_swatchbin(b: &[u8]) -> Result<(usize, usize, &[u8]), MapLoadError> {
    let bad = |m: String| Err(MapLoadError::Decode(m));
    if b.len() < 0x84 || &b[0..4] != b"burG" {
        return bad("not a swatchbin".into());
    }
    let u = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize;
    let (hdr, total, w, h, fmt, dsz) = (u(0x08), u(0x0c), u(0x4c), u(0x50), u(0x74), u(0x80));
    if total != b.len() {
        return bad(format!("size mismatch {total} != {}", b.len()));
    }
    if fmt != 0 {
        return bad(format!("pixel format {fmt:#x} is not BC1"));
    }
    if hdr.checked_add(dsz).is_none_or(|end| end > b.len()) || dsz < (w / 4) * (h / 4) * 8 {
        return bad("truncated".into());
    }
    Ok((w, h, &b[hdr..hdr + dsz]))
}

/// RGB565 → 8-bit RGB, rounding like the Python reference (`(v * 255 + half) / max`).
#[inline]
fn rgb565(c: u16) -> [u8; 3] {
    let (r, g, b) = (((c >> 11) & 31) as u32, ((c >> 5) & 63) as u32, (c & 31) as u32);
    [((r * 255 + 15) / 31) as u8, ((g * 255 + 31) / 63) as u8, ((b * 255 + 15) / 31) as u8]
}

/// BC1 → RGBA8, written into `dst` (row stride `stride` px) with its top-left at pixel
/// (`x0`, `y0`). Alpha is 255: the 3-colour mode's fourth colour is black, not transparent.
/// `w` and `h` are multiples of 4; `data` holds `w/4 × h/4` 8-byte blocks, row-major.
pub fn decode_bc1_into(data: &[u8], w: usize, h: usize, dst: &mut [u8], stride: usize, x0: usize, y0: usize) {
    let (bw, bh) = (w / 4, h / 4);
    for by in 0..bh {
        for bx in 0..bw {
            let blk = &data[(by * bw + bx) * 8..][..8];
            let c0 = u16::from_le_bytes([blk[0], blk[1]]);
            let c1 = u16::from_le_bytes([blk[2], blk[3]]);
            let (p0, p1) = (rgb565(c0), rgb565(c1));
            let mut pal = [p0, p1, [0; 3], [0; 3]];
            for k in 0..3 {
                if c0 > c1 {
                    pal[2][k] = ((2 * p0[k] as u32 + p1[k] as u32) / 3) as u8;
                    pal[3][k] = ((p0[k] as u32 + 2 * p1[k] as u32) / 3) as u8;
                } else {
                    pal[2][k] = ((p0[k] as u32 + p1[k] as u32) / 2) as u8;
                    // pal[3] stays black
                }
            }
            let idx = u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]);
            for y in 0..4 {
                let row = ((y0 + by * 4 + y) * stride + x0 + bx * 4) * 4;
                for x in 0..4 {
                    let p = pal[((idx >> (2 * (y * 4 + x))) & 3) as usize];
                    let o = row + x * 4;
                    dst[o..o + 4].copy_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
        }
    }
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<std::fs::File>, MapLoadError> {
    let f = std::fs::File::open(path).map_err(|_| MapLoadError::NotReadable(path.to_path_buf()))?;
    zip::ZipArchive::new(f).map_err(|e| MapLoadError::Decode(format!("{}: {e}", path.display())))
}

fn read_entry<R: Read + std::io::Seek>(z: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>, MapLoadError> {
    let mut f = z.by_name(name).map_err(|e| MapLoadError::Decode(format!("{name}: {e}")))?;
    // The zip's declared size is untrusted (a corrupt zip must not OOM-abort us): cap the
    // pre-allocation and the read. Real tiles are ~525-700 KB.
    let mut v = Vec::with_capacity((f.size() as usize).min(MAX_ENTRY_PREALLOC));
    f.by_ref().take(MAX_ENTRY_BYTES + 1).read_to_end(&mut v).map_err(|e| MapLoadError::Decode(format!("{name}: {e}")))?;
    if v.len() as u64 > MAX_ENTRY_BYTES {
        return Err(MapLoadError::Decode(format!("{name}: entry larger than {} MB", MAX_ENTRY_BYTES >> 20)));
    }
    Ok(v)
}

/// Largest zip entry [`read_entry`] accepts, bytes (real tiles are ~0.5 MB).
const MAX_ENTRY_BYTES: u64 = 16 << 20;
/// Largest buffer pre-allocated from a zip entry's declared size, bytes.
const MAX_ENTRY_PREALLOC: usize = 8 << 20;

/// One whole tile row of `level`, decoded into `band` (`TILE_PX` rows of `size` px, RGBA).
fn decode_row(zip_path: &Path, level: u32, row: usize, size: usize, band: &mut [u8]) -> Result<(), MapLoadError> {
    let mut z = open_zip(zip_path)?;
    for col in 0..size / TILE_PX {
        let name = format!("{level}-{row}-{col}.swatchbin");
        let raw = read_entry(&mut z, &name)?;
        let (w, h, data) = parse_swatchbin(&raw)?;
        if (w, h) != (TILE_PX, TILE_PX) {
            return Err(MapLoadError::Decode(format!("tile {name} is {w}x{h}, expected {TILE_PX}²")));
        }
        decode_bc1_into(data, w, h, band, size, col * TILE_PX, 0);
    }
    Ok(())
}

/// One tile of the pyramid as RGB8 (`TILE_PX² × 3` bytes, row-major): what the map editor's
/// Leaflet layer serves (JPEG-encoded by `mapedit::data::tile_jpeg`). Level `L` has `2^L` rows
/// and columns; Leaflet's `{z}/{x}/{y}` is `level = z, col = x, row = y`. ~30 ms (opens the zip
/// each call, so concurrent requests don't share state).
pub fn tile_rgb(media: &Path, season: &str, level: u32, row: usize, col: usize) -> Result<Vec<u8>, MapLoadError> {
    if level > MAX_LEVEL || row >> level != 0 || col >> level != 0 {
        return Err(MapLoadError::Decode(format!("no tile {level}-{row}-{col}")));
    }
    let zip_path = season_zip(media, season).ok_or_else(|| MapLoadError::MissingZip(season.to_owned()))?;
    let name = format!("{level}-{row}-{col}.swatchbin");
    let raw = read_entry(&mut open_zip(&zip_path)?, &name)?;
    let (w, h, data) = parse_swatchbin(&raw)?;
    if (w, h) != (TILE_PX, TILE_PX) {
        return Err(MapLoadError::Decode(format!("tile {name} is {w}x{h}, expected {TILE_PX}²")));
    }
    let mut rgba = vec![0u8; TILE_PX * TILE_PX * 4];
    decode_bc1_into(data, w, h, &mut rgba, TILE_PX, 0, 0);
    let mut rgb = Vec::with_capacity(TILE_PX * TILE_PX * 3);
    for p in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&p[..3]);
    }
    Ok(rgb)
}

/// A season's whole map at pyramid `level` (0..=[`MAX_LEVEL`]): RGBA8 pixels and the side length
/// in px (`1024 << level`). One thread per tile row, each with its own `ZipArchive` (~60 ms for
/// level 3 against ~250 ms serial, release build). Heavy: call off the UI thread.
pub fn load_mosaic(media: &Path, season: &str, level: u32) -> Result<(Vec<u8>, usize), MapLoadError> {
    if level > MAX_LEVEL {
        return Err(MapLoadError::Decode(format!("no map level {level}")));
    }
    let zip_path = season_zip(media, season).ok_or_else(|| MapLoadError::MissingZip(season.to_owned()))?;
    let size = TILE_PX << level;
    let mut out = vec![0u8; size * size * 4];
    std::thread::scope(|s| {
        let handles: Vec<_> = out
            .chunks_mut(TILE_PX * size * 4) // one tile row each
            .enumerate()
            .map(|(row, band)| {
                let zip_path = &zip_path;
                s.spawn(move || decode_row(zip_path, level, row, size, band))
            })
            .collect();
        for h in handles {
            h.join().map_err(|_| MapLoadError::Decode("tile decoder thread panicked".into()))??;
        }
        Ok::<(), MapLoadError>(())
    })?;
    Ok((out, size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::install::find_media;

    /// A swatchbin with the given header fields and `dsz` bytes of zeroed data.
    fn fake(magic: &[u8; 4], fmt: u32, total_delta: i64, dsz: u32) -> Vec<u8> {
        let mut b = vec![0u8; 140 + dsz as usize];
        b[0..4].copy_from_slice(magic);
        let mut put = |o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(8, 140);
        put(0x4c, 8);
        put(0x50, 8);
        put(0x74, fmt);
        put(0x80, dsz);
        let total = (140 + dsz as i64 + total_delta) as u32;
        b[12..16].copy_from_slice(&total.to_le_bytes());
        b
    }

    fn block(c0: u16, c1: u16, idx: u32) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[0..2].copy_from_slice(&c0.to_le_bytes());
        b[2..4].copy_from_slice(&c1.to_le_bytes());
        b[4..8].copy_from_slice(&idx.to_le_bytes());
        b
    }

    fn px(buf: &[u8], x: usize, y: usize) -> [u8; 4] {
        let o = (y * 4 + x) * 4;
        [buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]
    }

    #[test]
    fn bc1_four_colour_block() {
        // c0 = pure red (0xF800) > c1 = pure blue (0x001F): 4-colour mode.
        // Texel order (x,y) = bit 2*(y*4+x); pick 0,1,2,3 in the first row.
        let blk = block(0xF800, 0x001F, 0b11_10_01_00);
        let mut out = vec![0u8; 4 * 4 * 4];
        decode_bc1_into(&blk, 4, 4, &mut out, 4, 0, 0);
        assert_eq!(px(&out, 0, 0), [255, 0, 0, 255]);
        assert_eq!(px(&out, 1, 0), [0, 0, 255, 255]);
        assert_eq!(px(&out, 2, 0), [170, 0, 85, 255]); // (2*red + blue) / 3
        assert_eq!(px(&out, 3, 0), [85, 0, 170, 255]); // (red + 2*blue) / 3
        assert_eq!(px(&out, 0, 1), [255, 0, 0, 255]); // idx 0 everywhere else
    }

    #[test]
    fn bc1_three_colour_block_p3_is_black() {
        // c0 <= c1: 3-colour mode, p2 = midpoint, p3 = opaque black (not transparent).
        let blk = block(0x001F, 0xF800, 0b11_10_00_00);
        let mut out = vec![0u8; 4 * 4 * 4];
        decode_bc1_into(&blk, 4, 4, &mut out, 4, 0, 0);
        assert_eq!(px(&out, 0, 0), [0, 0, 255, 255]);
        assert_eq!(px(&out, 2, 0), [127, 0, 127, 255]); // (blue + red) / 2
        assert_eq!(px(&out, 3, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn bc1_writes_at_offset_with_stride() {
        // A 4x4 block placed at (4, 4) of an 8x8 buffer touches nothing else.
        let blk = block(0xF800, 0x001F, 0);
        let mut out = vec![0u8; 8 * 8 * 4];
        decode_bc1_into(&blk, 4, 4, &mut out, 8, 4, 4);
        assert_eq!(&out[(4 * 8 + 4) * 4..][..4], &[255, 0, 0, 255]);
        assert_eq!(&out[(3 * 8 + 4) * 4..][..4], &[0, 0, 0, 0]);
        assert_eq!(&out[(4 * 8 + 3) * 4..][..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn swatchbin_header_checks() {
        let ok = fake(b"burG", 0, 0, 32); // 8x8 BC1 = 4 blocks = 32 bytes
        let (w, h, data) = parse_swatchbin(&ok).expect("valid");
        assert_eq!((w, h, data.len()), (8, 8, 32));
        assert!(matches!(parse_swatchbin(&fake(b"Gurb", 0, 0, 32)), Err(MapLoadError::Decode(_))));
        assert!(matches!(parse_swatchbin(&fake(b"burG", 0, 4, 32)), Err(MapLoadError::Decode(_)))); // total != len
        assert!(matches!(parse_swatchbin(&fake(b"burG", 7, 0, 32)), Err(MapLoadError::Decode(_)))); // not BC1
        assert!(matches!(parse_swatchbin(&fake(b"burG", 0, 0, 16)), Err(MapLoadError::Decode(_)))); // too little data
        assert!(matches!(parse_swatchbin(&ok[..100]), Err(MapLoadError::Decode(_)))); // shorter than a header
    }

    /// Decode a single tile of the real install as RGBA (1024²).
    fn real_tile(media: &Path, season: &str, level: u32, row: usize, col: usize) -> Vec<u8> {
        let zip = season_zip(media, season).expect("season zip");
        let raw = read_entry(&mut open_zip(&zip).unwrap(), &format!("{level}-{row}-{col}.swatchbin")).unwrap();
        let (w, h, data) = parse_swatchbin(&raw).unwrap();
        assert_eq!((w, h), (TILE_PX, TILE_PX));
        let mut out = vec![0u8; w * h * 4];
        decode_bc1_into(data, w, h, &mut out, w, 0, 0);
        out
    }

    fn rgb_at(tile: &[u8], x: usize, y: usize) -> [u8; 3] {
        let o = (y * TILE_PX + x) * 4;
        [tile[o], tile[o + 1], tile[o + 2]]
    }

    /// Reference pixels from the Python decoder (`tools/fh6-extract/extract_map.py`).
    #[test]
    fn real_install_reference_pixels() {
        let Some(media) = find_media(None) else {
            eprintln!("SKIP real_install_reference_pixels: FH6 install not found");
            return;
        };
        // (season, level, row, col, x, y, expected rgb)
        let cases: [(&str, u32, usize, usize, usize, usize, [u8; 3]); 11] = [
            ("Summer", 3, 4, 4, 10, 20, [41, 61, 33]),
            ("Summer", 3, 0, 0, 0, 0, [52, 62, 33]),
            ("Summer", 3, 7, 7, 1023, 1023, [52, 71, 76]),
            ("Summer", 2, 1, 1, 512, 512, [58, 69, 41]),
            ("Summer", 3, 3, 5, 300, 700, [68, 89, 93]),
            ("Spring", 3, 0, 0, 0, 0, [49, 61, 33]),
            ("Spring", 3, 4, 4, 10, 20, [58, 69, 41]),
            ("Autumn", 3, 0, 0, 0, 0, [165, 138, 82]),
            ("Autumn", 3, 3, 5, 300, 700, [99, 97, 90]),
            ("Winter", 3, 0, 0, 0, 0, [132, 154, 189]),
            ("Winter", 3, 3, 5, 300, 700, [99, 105, 115]),
        ];
        for (season, level, row, col, x, y, want) in cases {
            let tile = real_tile(&media, season, level, row, col);
            assert_eq!(rgb_at(&tile, x, y), want, "{season} L{level} r{row} c{col} px({x},{y})");
        }
    }

    /// Whole level-2 mosaic (4096²): placement is row-major (row = y), tile (r, c) lands at
    /// (c*1024, r*1024). Ignored by default (16 tiles are slow in a debug build):
    /// `cargo test --release -- --ignored real_install_mosaic`.
    #[test]
    #[ignore = "slow in debug builds; run with --release -- --ignored"]
    fn real_install_mosaic_level2() {
        let Some(media) = find_media(None) else {
            eprintln!("SKIP real_install_mosaic_level2: FH6 install not found");
            return;
        };
        let t = std::time::Instant::now();
        let (rgba, size) = load_mosaic(&media, "Summer", 2).expect("level 2");
        eprintln!("L2 mosaic in {:?}", t.elapsed());
        assert_eq!(size, 4096);
        assert_eq!(rgba.len(), 4096 * 4096 * 4);
        // Summer L2 tile (row 1, col 1) px (512, 512) is [58,69,41] -> mosaic px (1536, 1536).
        let o = (1536 * size + 1536) * 4;
        assert_eq!(&rgba[o..o + 4], &[58, 69, 41, 255]);
        assert!(matches!(load_mosaic(&media, "Nonsense", 2), Err(MapLoadError::MissingZip(_))));
    }
}
