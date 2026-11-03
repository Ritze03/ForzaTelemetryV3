//! LZ4 *block* decoder (no frame format), a port of `tools/fh6-extract/lz4b.py`. The game's
//! PGZP archives ([`super::pgzp`]) store entries as raw blocks with the decoded size kept next
//! to them, so that is all that's needed: ~30 lines instead of a new crate (`lz4_flex`).

/// Decode one raw LZ4 block to (at most) `usize_` bytes. Stops at the end of `src` or once
/// `usize_` bytes are out, like the Python reference (the last sequence of a block is
/// literals-only). Errors on a truncated stream or a match offset that reaches before the start.
pub fn lz4_block(src: &[u8], usize_: usize) -> Result<Vec<u8>, &'static str> {
    let mut out: Vec<u8> = Vec::with_capacity(usize_.min(64 << 20));
    let (mut i, n) = (0usize, src.len());
    // Length extension: 15 in the token nibble, then bytes added up until one is not 255.
    let ext = |i: &mut usize, mut len: usize| -> Result<usize, &'static str> {
        loop {
            let b = *src.get(*i).ok_or("LZ4: truncated length")?;
            *i += 1;
            len += b as usize;
            if b != 255 {
                return Ok(len);
            }
        }
    };
    while i < n && out.len() < usize_ {
        let t = src[i];
        i += 1;
        let mut ll = (t >> 4) as usize;
        if ll == 15 {
            ll = ext(&mut i, ll)?;
        }
        out.extend_from_slice(src.get(i..i + ll).ok_or("LZ4: truncated literals")?);
        i += ll;
        if i >= n || out.len() >= usize_ {
            break;
        }
        let lo = *src.get(i).ok_or("LZ4: truncated offset")? as usize;
        let hi = *src.get(i + 1).ok_or("LZ4: truncated offset")? as usize;
        i += 2;
        let mut ml = (t & 15) as usize;
        if ml == 15 {
            ml = ext(&mut i, ml)?;
        }
        ml += 4;
        let off = lo | hi << 8;
        if off == 0 || off > out.len() {
            return Err("LZ4: bad match offset");
        }
        let s = out.len() - off;
        if off >= ml {
            out.extend_from_within(s..s + ml);
        } else {
            // Overlapping match (run-length style): copy byte by byte.
            for k in 0..ml {
                let b = out[s + k];
                out.push(b);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_only() {
        // token 0x50: 5 literals, no match (end of block)
        assert_eq!(lz4_block(&[0x50, b'h', b'e', b'l', b'l', b'o'], 5).unwrap(), b"hello");
    }

    #[test]
    fn overlapping_match() {
        // "ab" literal then a match of length 4+2 at offset 2 -> "abababab"
        let src = [0x22, b'a', b'b', 0x02, 0x00];
        assert_eq!(lz4_block(&src, 8).unwrap(), b"abababab");
        // run of one byte: literal 'x', match offset 1, length 4+11 = 15 -> 16 x's
        let src = [0x1b, b'x', 0x01, 0x00];
        assert_eq!(lz4_block(&src, 16).unwrap(), vec![b'x'; 16]);
    }

    #[test]
    fn long_literal_run_uses_length_extension() {
        // 15 + 255 + 10 = 280 literals
        let mut src = vec![0xf0, 255, 10];
        src.extend((0..280u32).map(|i| i as u8));
        let out = lz4_block(&src, 280).unwrap();
        assert_eq!(out.len(), 280);
        assert_eq!(out[279], 279u32 as u8);
    }

    #[test]
    fn rejects_bad_streams() {
        assert!(lz4_block(&[0x50, b'h'], 5).is_err()); // truncated literals
        assert!(lz4_block(&[0x11, b'a', 0x05, 0x00], 9).is_err()); // offset 5 > 1 byte of history
        assert!(lz4_block(&[0x11, b'a', 0x00, 0x00], 9).is_err()); // offset 0
    }
}
