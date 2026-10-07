//! PGZP v101 reader for `<media>/Tracks/Brio/GeoChunk<k>.minizip`, a port of
//! `tools/fh6-extract/pgzp.py`. See `docs/game-data/fh6-game-files.md` ("PGZP").
//!
//! The archive is huge (GeoChunk0 is ~40 GB, 411 013 entries) and is **never loaded whole**:
//! [`Pgzp::open`] reads only the index (~10 MB) and the companion name list, and
//! [`Pgzp::entry`] seeks to one entry and decodes it. Entry names are not in the archive; line
//! `i` of `ChunkContentsMiniZip<k>.txt` is the in-game path of entry `i`.
//!
//! Layout: header `u32[8]` (`'PGZP'`, 101, _, `n` entries, `m` ids, `per` entries per segment,
//! `nseg` segments, _), then `u32[m]` ids, then the index `S = u32[4 + 3n + 2*nseg]`:
//! `S[0] == n`, the first segment's start as a u64 (`S[1..3]`; GeoChunk2 has `S[1] == 0` and
//! the u64 at `S[2..4]`), then per segment `per` rows of `{offset, decoded size, flags}`
//! followed by the next segment's start (u64). Entry data lives at `segment start + offset`.
//! `flags & 0xff`: `0x1f` raw LZ4 block, `0x00` stored; `0x08` (raw deflate) is **not**
//! supported (no needed entry uses it; would pull in `flate2`).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::lz4::lz4_block;

/// Largest compressed entry we are willing to read (guards a corrupt index; real terrain models are ~40 KB).
const MAX_ENTRY: i64 = 64 << 20;

pub struct Pgzp {
    path: PathBuf,
    n: usize,
    segs: Vec<u64>,
    off: Vec<u64>,
    us: Vec<u32>,
    fl: Vec<u32>,
    sg: Vec<u32>,
    names: Vec<(usize, String)>,
}

/// In-game path of one `ChunkContentsMiniZip<k>.txt` line: `<PREZIPPED>d:\...\zipcache\pc\<path>|<n>`
/// → the lower-cased `<path>` with backslashes (what the extraction scripts match on).
pub fn parse_chunk_line(line: &str) -> String {
    let l = line.trim_end_matches('\r');
    let l = l.rsplit_once('|').map_or(l, |(a, _)| a).replace("<PREZIPPED>", "");
    l.rsplit("zipcache\\pc\\").next().unwrap_or("").to_ascii_lowercase()
}

/// Parse the name list, returning `(line count, [(entry index, name) for names `keep` accepts])`.
/// Filtering while parsing avoids holding 411k strings of which a few thousand matter.
pub fn chunk_names(txt: &[u8], keep: &dyn Fn(&str) -> bool) -> (usize, Vec<(usize, String)>) {
    let mut lines: Vec<&[u8]> = txt.split(|&b| b == b'\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let sel = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            let name = parse_chunk_line(&String::from_utf8_lossy(l));
            keep(&name).then_some((i, name))
        })
        .collect();
    (lines.len(), sel)
}

impl Pgzp {
    /// Open `path` (the `.minizip`) with its name list `names_path`; keep only names `keep`
    /// accepts. Errors if it isn't a PGZP v101 file or the name count differs from the entry count.
    pub fn open(path: &Path, names_path: &Path, keep: &dyn Fn(&str) -> bool) -> Result<Self, String> {
        let io = |e: std::io::Error| format!("{}: {e}", path.display());
        let txt = std::fs::read(names_path).map_err(|e| format!("{}: {e}", names_path.display()))?;
        let (nlines, names) = chunk_names(&txt, keep);
        drop(txt);
        let mut f = File::open(path).map_err(io)?;
        let mut h = [0u8; 32];
        f.read_exact(&mut h).map_err(io)?;
        let hu = |i: usize| u32::from_le_bytes([h[4 * i], h[4 * i + 1], h[4 * i + 2], h[4 * i + 3]]);
        if hu(0) != 0x505a_4750 || hu(1) != 101 {
            return Err(format!("{}: not a PGZP v101 file", path.display()));
        }
        let (n, m, per, nseg) = (hu(3) as usize, hu(4) as usize, hu(5) as usize, hu(6) as usize);
        if per == 0 || n > 50_000_000 || m > 50_000_000 || nseg > 50_000_000 {
            return Err(format!("{}: implausible PGZP header", path.display()));
        }
        f.seek(SeekFrom::Start(32 + 4 * m as u64)).map_err(io)?;
        let mut raw = vec![0u8; 4 * (4 + 3 * n + 2 * nseg)];
        f.read_exact(&mut raw).map_err(io)?;
        let s: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        if s[0] as usize != n {
            return Err(format!("{}: PGZP index count mismatch", path.display()));
        }
        let p = if s[1] == 0 { 2 } else { 1 };
        let mut segs = vec![s[p] as u64 | (s[p + 1] as u64) << 32];
        let body = &s[p + 2..];
        let (mut off, mut us, mut fl, mut sg) = (vec![0u64; n], vec![0u32; n], vec![0u32; n], vec![0u32; n]);
        let (mut q, mut k) = (0usize, 0usize);
        for sgi in 0..nseg {
            let c = per.min(n - k);
            if q + 3 * c + 2 > body.len() {
                return Err(format!("{}: PGZP index truncated", path.display()));
            }
            for j in 0..c {
                off[k + j] = body[q + 3 * j] as u64;
                us[k + j] = body[q + 3 * j + 1];
                fl[k + j] = body[q + 3 * j + 2];
                sg[k + j] = sgi as u32;
            }
            q += 3 * c;
            k += c;
            segs.push(body[q] as u64 | (body[q + 1] as u64) << 32);
            q += 2;
        }
        if nlines != n {
            return Err(format!("{}: {nlines} names for {n} entries", names_path.display()));
        }
        Ok(Self { path: path.to_path_buf(), n, segs, off, us, fl, sg, names })
    }

    /// Number of entries in the archive.
    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// `(entry index, lower-case in-game path)` of the entries `keep` accepted in [`Pgzp::open`].
    pub fn names(&self) -> &[(usize, String)] {
        &self.names
    }

    /// A fresh read handle on the archive (one per thread: reads `seek` then `read_exact`).
    pub fn open_file(&self) -> Result<File, String> {
        File::open(&self.path).map_err(|e| format!("{}: {e}", self.path.display()))
    }

    /// Entry indices ordered by (segment, offset) = the fastest, most sequential read order.
    pub fn sorted(&self, mut idx: Vec<usize>) -> Vec<usize> {
        idx.sort_by_key(|&i| (self.sg[i], self.off[i]));
        idx
    }

    /// Read and decode entry `r` with an open handle (see [`Pgzp::open_file`]).
    pub fn entry(&self, f: &mut File, r: usize) -> Result<Vec<u8>, String> {
        if r >= self.n {
            return Err(format!("PGZP entry {r} out of range"));
        }
        let io = |e: std::io::Error| format!("{}: {e}", self.path.display());
        let s = self.sg[r] as usize;
        let o = self.off[r];
        let a = self.segs[s] + o;
        // Size = distance to the next entry in the same segment, else to the next segment's
        // start; the very last entry runs to the end of the file.
        let mut c = if r + 1 < self.n && self.sg[r + 1] as usize == s {
            self.off[r + 1] as i64 - o as i64
        } else {
            self.segs[s + 1] as i64 - a as i64
        };
        if c <= 0 {
            c = f.seek(SeekFrom::End(0)).map_err(io)? as i64 - a as i64;
        }
        if !(0..=MAX_ENTRY).contains(&c) {
            return Err(format!("PGZP entry {r}: implausible size {c}"));
        }
        f.seek(SeekFrom::Start(a)).map_err(io)?;
        let mut d = vec![0u8; c as usize];
        f.read_exact(&mut d).map_err(io)?;
        let us = self.us[r] as usize;
        match self.fl[r] & 0xff {
            0x1f => lz4_block(&d, us).map_err(|e| format!("PGZP entry {r}: {e}")),
            0x00 => {
                d.truncate(us);
                Ok(d)
            }
            other => Err(format!("PGZP entry {r}: unsupported compression flag {other:#x}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_line_parsing() {
        let l = "<PREZIPPED>d:\\scratch\\x\\zipcache\\pc\\Scene\\TBHeightfield\\AutoTerrain_X0_Z0_cb_cluster1.i.modelbin|1234\r";
        assert_eq!(parse_chunk_line(l), "scene\\tbheightfield\\autoterrain_x0_z0_cb_cluster1.i.modelbin");
        assert_eq!(parse_chunk_line("plain"), "plain");
    }

    #[test]
    fn name_filtering_keeps_indices_and_counts_all_lines() {
        let txt = b"<PREZIPPED>a\\zipcache\\pc\\Foo\\A.bin|1\n<PREZIPPED>a\\zipcache\\pc\\Bar\\B.bin|2\nC.bin|3\n";
        let (n, sel) = chunk_names(txt, &|s| s.starts_with("bar"));
        assert_eq!(n, 3);
        assert_eq!(sel, vec![(1, "bar\\b.bin".to_owned())]);
    }

    /// A hand-built one-segment archive: entry 0 stored, entry 1 an LZ4 block, entry 2 stored
    /// and last (runs to the end of the file); two ids; names filtered to the ".x" files.
    #[test]
    fn synthetic_archive_roundtrip() {
        let dir = crate::gamedata::tempdir("pgzp");
        let (n, m, per, nseg) = (3u32, 2u32, 512u32, 1u32);
        let words = 4 + 3 * n as usize + 2 * nseg as usize;
        let data_start = 32 + 4 * m as u64 + 4 * words as u64;
        let e0 = b"hello".to_vec();
        let e1 = vec![0x22, b'a', b'b', 0x02, 0x00]; // LZ4 -> "abababab"
        let e2 = b"tail!!".to_vec();
        let offs = [0u32, e0.len() as u32, (e0.len() + e1.len()) as u32];
        let mut file: Vec<u8> = Vec::new();
        for v in [0x505a_4750u32, 101, 0, n, m, per, nseg, 0] {
            file.extend(v.to_le_bytes());
        }
        file.extend([7u32, 8].iter().flat_map(|v| v.to_le_bytes()));
        let mut s: Vec<u32> = vec![n, data_start as u32, 0]; // S[0] = n, then the u64 segment start
        for (o, us, fl) in [(offs[0], 5u32, 0u32), (offs[1], 8, 0x1f), (offs[2], 6, 0)] {
            s.extend([o, us, fl]);
        }
        s.extend([0, 0]); // next segment start (past the last entry; unused for the last entry)
        s.push(0); // pad to 4 + 3n + 2*nseg words
        assert_eq!(s.len(), words);
        file.extend(s.iter().flat_map(|v| v.to_le_bytes()));
        file.extend(&e0);
        file.extend(&e1);
        file.extend(&e2);
        let (pz, txt) = (dir.join("t.minizip"), dir.join("t.txt"));
        std::fs::write(&pz, &file).unwrap();
        std::fs::write(&txt, "<PREZIPPED>z\\zipcache\\pc\\a.x|1\nb.y|2\nC.X|3\n").unwrap();

        let g = Pgzp::open(&pz, &txt, &|n| n.ends_with(".x")).unwrap();
        assert_eq!(g.len(), 3);
        assert_eq!(g.names(), &[(0, "a.x".to_owned()), (2, "c.x".to_owned())]);
        let mut f = g.open_file().unwrap();
        assert_eq!(g.entry(&mut f, 0).unwrap(), b"hello");
        assert_eq!(g.entry(&mut f, 1).unwrap(), b"abababab");
        assert_eq!(g.entry(&mut f, 2).unwrap(), b"tail!!");
        assert!(g.entry(&mut f, 3).is_err());
        assert_eq!(g.sorted(vec![2, 0, 1]), vec![0, 1, 2]);
        // wrong name count -> error
        std::fs::write(&txt, "only.x\n").unwrap();
        assert!(Pgzp::open(&pz, &txt, &|_| true).is_err());
        // wrong magic -> error
        std::fs::write(&pz, vec![0u8; 64]).unwrap();
        assert!(Pgzp::open(&pz, &txt, &|_| true).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
