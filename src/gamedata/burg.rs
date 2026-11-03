//! The game's `burG` container (swatchbin textures, `.modelbin` models) and the terrain mesh
//! inside the `tbheightfield` / `uberheightfield` models. A port of `burg_sections`,
//! `burg_names` and `terrain_mesh` from `tools/fh6-extract/extract_terrain.py`; see
//! `docs/game-data/fh6-terrain.md`.
//!
//! Container (all little-endian u32): `0x04` version, `0x08` header size, `0x0c` total size,
//! `0x10` section count; section `k` at `20 + 24k`: 4-byte tag stored **byte-reversed** (`IndB`
//! is `BdnI` on disk), version, id, data offset, data size, size2. Material names sit in
//! `emaN` (reversed `Name`) chunks of the header area, the string at chunk + 16.
//!
//! Every read is bounds-checked: a malformed model yields `None`, never a panic (the data comes
//! from the user's install, not from us).

/// A decoded terrain model: world-space vertices (x, y = height, z) and triangles into them.
pub struct Mesh {
    pub v: Vec<[f32; 3]>,
    pub t: Vec<[u32; 3]>,
}

struct Section {
    tag: [u8; 4],
    off: usize,
    size: usize,
}

fn u32at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o.checked_add(4)?).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn u16at(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o.checked_add(2)?).map(|b| u16::from_le_bytes([b[0], b[1]]))
}
fn f32at(d: &[u8], o: usize) -> Option<f32> {
    u32at(d, o).map(f32::from_bits)
}

/// The section table (`tag` already un-reversed).
fn sections(d: &[u8]) -> Option<Vec<Section>> {
    if d.get(0..4)? != b"burG" {
        return None;
    }
    let n = u32at(d, 16)? as usize;
    (0..n.min(4096))
        .map(|k| {
            let o = 20 + 24 * k;
            let mut tag = [*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?];
            tag.reverse();
            Some(Section { tag, off: u32at(d, o + 12)? as usize, size: u32at(d, o + 16)? as usize })
        })
        .collect()
}

/// Material names from the `emaN` chunks of the header area, in order.
fn names(d: &[u8], secs: &[Section]) -> Option<Vec<String>> {
    let hdr = d.get(20 + 24 * secs.len()..secs.first()?.off)?;
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= hdr.len() {
        if &hdr[i..i + 4] == b"emaN" {
            let p = (i + 16).min(hdr.len());
            let e = hdr[p..].iter().position(|&b| b == 0).map(|x| p + x);
            out.push(match e {
                Some(e) if e > p => String::from_utf8_lossy(&hdr[p..e]).into_owned(),
                _ => String::new(),
            });
            i += 4;
        } else {
            i += 1;
        }
    }
    Some(out)
}

/// Header of an `IndB` / `VerB` section: `{u32 count, u32 bytes, u16 stride, u16, u32 layout}`,
/// then `bytes` of data. Returns `(count, stride, data)`.
fn buffer<'a>(d: &'a [u8], s: &Section) -> Option<(usize, usize, &'a [u8])> {
    let (count, bytes, stride) = (u32at(d, s.off)? as usize, u32at(d, s.off + 4)? as usize, u16at(d, s.off + 8)? as usize);
    let start = s.off.checked_add(16)?;
    Some((count, stride, d.get(start..start.checked_add(bytes)?)?))
}

/// The triangles of a terrain model whose material name satisfies `want` (the ground meshes
/// are the `GenMaterial_UberMap*` ones). `None` if the data isn't the expected layout (not a
/// `burG`, no index/vertex buffer, vertex stride != 8).
///
/// Vertices are `3 × i16` snorm + 2 spare bytes: `world = snorm / 32767 * scale + centre`, with
/// `(sx, sy, sz, 0, cx, cy, cz, 0)` the last 8 f32 of the first `Mesh` section. Each `Mesh`
/// section: material `u16` at +2, first index `u32` at +34, index count `u32` at +42; a mesh
/// whose indices reach past the vertex buffer is skipped.
pub fn terrain_mesh(d: &[u8], want: &dyn Fn(&str) -> bool) -> Option<Mesh> {
    let secs = sections(d)?;
    let names = names(d, &secs)?;
    let nmat = secs.iter().filter(|s| &s.tag == b"MatI").count();
    let (istride, ibytes) = {
        let s = secs.iter().find(|s| &s.tag == b"IndB")?;
        let (_, stride, data) = buffer(d, s)?;
        (stride, data)
    };
    let idx = |k: usize| -> Option<u32> {
        if istride == 2 {
            u16at(ibytes, 2 * k).map(u32::from)
        } else {
            u32at(ibytes, 4 * k)
        }
    };
    let (vcount, vstride, vbytes) = buffer(d, secs.iter().find(|s| &s.tag == b"VerB")?)?;
    if vstride != 8 || vbytes.len() < vcount.checked_mul(8)? {
        return None;
    }
    let mesh0 = secs.iter().find(|s| &s.tag == b"Mesh")?;
    let gbase = (mesh0.off + mesh0.size).checked_sub(32)?;
    let mut g = [0f64; 8];
    for (k, v) in g.iter_mut().enumerate() {
        *v = f32at(d, gbase + 4 * k)? as f64;
    }
    let v: Vec<[f32; 3]> = (0..vcount)
        .map(|k| {
            let p = |j: usize| i16::from_le_bytes([vbytes[8 * k + 2 * j], vbytes[8 * k + 2 * j + 1]]) as f64 / 32767.0;
            [(p(0) * g[0] + g[4]) as f32, (p(1) * g[1] + g[5]) as f32, (p(2) * g[2] + g[6]) as f32]
        })
        .collect();
    let mut t = Vec::new();
    for s in secs.iter().filter(|s| &s.tag == b"Mesh") {
        let mat = u16at(d, s.off + 2)? as usize;
        let (start, n) = (u32at(d, s.off + 34)? as usize, u32at(d, s.off + 42)? as usize);
        if mat >= nmat || mat >= names.len() || !want(&names[mat]) {
            continue;
        }
        let tris: Option<Vec<[u32; 3]>> = (0..n / 3)
            .map(|k| Some([idx(start + 3 * k)?, idx(start + 3 * k + 1)?, idx(start + 3 * k + 2)?]))
            .collect();
        if let Some(tris) = tris {
            if tris.iter().all(|q| q.iter().all(|&i| (i as usize) < vcount)) {
                t.extend(tris);
            }
        }
    }
    Some(Mesh { v, t })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn put32(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// A tiny hand-built terrain model: one material `name`, 4 vertices spanning x,z in
    /// [-10, 10] at heights 0..30 (scale 10, centre 0 / 15 / 0), 2 triangles (u16 indices).
    pub(crate) fn tiny_model(name: &str, bad_index: bool) -> Vec<u8> {
        // sections: MatI, IndB, VerB, Mesh
        let nsec = 4;
        let hdr_end = 20 + 24 * nsec; // 116
        let name_chunk = hdr_end; // "emaN" + 12 spare + name\0
        let name_len = 16 + name.len() + 1;
        let mat_off = (hdr_end + name_len + 3) & !3;
        let ind_off = mat_off + 16;
        let ind_bytes = 6 * 2;
        let ver_off = ind_off + 16 + ind_bytes;
        let ver_bytes = 4 * 8;
        let mesh_off = ver_off + 16 + ver_bytes;
        let mesh_size = 46 + 32; // fields + trailing 8 f32
        let total = mesh_off + mesh_size;
        let mut d = vec![0u8; total];
        d[0..4].copy_from_slice(b"burG");
        put32(&mut d, 4, 0x101);
        put32(&mut d, 8, hdr_end as u32);
        put32(&mut d, 12, total as u32);
        put32(&mut d, 16, nsec as u32);
        for (k, (tag, off, size)) in [(b"MatI", mat_off, 16), (b"IndB", ind_off, 16 + ind_bytes), (b"VerB", ver_off, 16 + ver_bytes), (b"Mesh", mesh_off, mesh_size)].into_iter().enumerate() {
            let o = 20 + 24 * k;
            let mut t = *tag;
            t.reverse();
            d[o..o + 4].copy_from_slice(&t);
            put32(&mut d, o + 12, off as u32);
            put32(&mut d, o + 16, size as u32);
        }
        d[name_chunk..name_chunk + 4].copy_from_slice(b"emaN");
        d[name_chunk + 16..name_chunk + 16 + name.len()].copy_from_slice(name.as_bytes());
        // IndB: count 6, bytes 12, stride 2
        put32(&mut d, ind_off, 6);
        put32(&mut d, ind_off + 4, ind_bytes as u32);
        d[ind_off + 8..ind_off + 10].copy_from_slice(&2u16.to_le_bytes());
        let last = if bad_index { 9u16 } else { 3 };
        for (k, i) in [0u16, 1, 2, 2, 1, last].into_iter().enumerate() {
            d[ind_off + 16 + 2 * k..][..2].copy_from_slice(&i.to_le_bytes());
        }
        // VerB: 4 vertices, stride 8: snorm (+-32767 -> +-1)
        put32(&mut d, ver_off, 4);
        put32(&mut d, ver_off + 4, ver_bytes as u32);
        d[ver_off + 8..ver_off + 10].copy_from_slice(&8u16.to_le_bytes());
        for (k, (x, y, z)) in [(-32767i16, -32767i16, -32767i16), (32767, 0, -32767), (-32767, 32767, 32767), (32767, 32767, 32767)].into_iter().enumerate() {
            for (j, c) in [x, y, z].into_iter().enumerate() {
                d[ver_off + 16 + 8 * k + 2 * j..][..2].copy_from_slice(&c.to_le_bytes());
            }
        }
        // Mesh: material 0, first index 0, 6 indices; trailing scale (10,15,10,0) and centre (0,15,0,0)
        d[mesh_off + 2..mesh_off + 4].copy_from_slice(&0u16.to_le_bytes());
        put32(&mut d, mesh_off + 34, 0);
        put32(&mut d, mesh_off + 42, 6);
        for (k, f) in [10.0f32, 15.0, 10.0, 0.0, 0.0, 15.0, 0.0, 0.0].into_iter().enumerate() {
            put32(&mut d, mesh_off + mesh_size - 32 + 4 * k, f.to_bits());
        }
        d
    }

    #[test]
    fn tiny_model_decodes() {
        let d = tiny_model("GenMaterial_UberMap_grass", false);
        let m = terrain_mesh(&d, &|n| n.starts_with("GenMaterial_UberMap")).expect("mesh");
        assert_eq!(m.v.len(), 4);
        assert_eq!(m.t, vec![[0, 1, 2], [2, 1, 3]]);
        assert_eq!(m.v[0], [-10.0, 0.0, -10.0]);
        assert_eq!(m.v[3], [10.0, 30.0, 10.0]);
        assert_eq!(m.v[1], [10.0, 15.0, -10.0]);
    }

    #[test]
    fn material_filter_and_bad_index() {
        let d = tiny_model("Other", false);
        let m = terrain_mesh(&d, &|n| n.starts_with("GenMaterial_UberMap")).expect("mesh");
        assert!(m.t.is_empty());
        // an index past the vertex buffer drops that mesh (as the Python reference does)
        let d = tiny_model("GenMaterial_UberMap_x", true);
        assert!(terrain_mesh(&d, &|_| true).unwrap().t.is_empty());
    }

    #[test]
    fn malformed_is_none_not_panic() {
        assert!(terrain_mesh(b"nope", &|_| true).is_none());
        let d = tiny_model("GenMaterial_UberMap_x", false);
        for cut in [10, 30, 130, d.len() - 10] {
            let _ = terrain_mesh(&d[..cut], &|_| true); // must not panic
        }
    }
}
