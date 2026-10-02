//! FH6 `.str` string tables (port of `tools/fh6-extract/fh6str.py`).
//!
//! Layout (little endian): 0x98-byte header, `u32` count at 0x94, then `count` x (`u32` key,
//! `u32` offset), then NUL-terminated UTF-8 strings (offset relative to the end of the key table).
//! Key = `strhash(identifier)`.

use std::collections::HashMap;

/// `h = 0xFFFFFFFF; for each byte c: h = rotl32(h ^ c, 7)`.
pub fn strhash(s: &str) -> u32 {
    let mut h = u32::MAX;
    for c in s.bytes() {
        h = (h ^ c as u32).rotate_left(7);
    }
    h
}

/// Parse a `.str` file into `key -> text`. Malformed entries are skipped, never panic.
pub fn parse_str(d: &[u8]) -> HashMap<u32, String> {
    let u32_at = |o: usize| -> Option<u32> {
        d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut out = HashMap::new();
    let Some(n) = u32_at(0x94) else { return out };
    let n = n as usize;
    let base = 0x98usize.saturating_add(n.saturating_mul(8));
    for i in 0..n {
        let (Some(k), Some(o)) = (u32_at(0x98 + 8 * i), u32_at(0x98 + 8 * i + 4)) else { break };
        let start = base.saturating_add(o as usize);
        let Some(tail) = d.get(start..) else { continue };
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        out.insert(k, String::from_utf8_lossy(&tail[..end]).into_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strhash_known_values() {
        // Reference values from tools/fh6-extract/fh6str.py (Python port of the same algorithm).
        assert_eq!(strhash(""), 0xFFFF_FFFF);
        assert_eq!(strhash("a"), ((0xFFFF_FFFFu32 ^ 0x61).rotate_left(7)));
        assert_eq!(strhash("IDS_ModelShort_4144"), 0xd9b5_317e);
        assert_eq!(strhash("IDS_DisplayName_4277"), 0xb165_c7b8);
    }

    #[test]
    fn parse_synthetic_table() {
        let mut d = vec![0u8; 0x98];
        d[0x94..0x98].copy_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&7u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&9u32.to_le_bytes());
        d.extend_from_slice(&3u32.to_le_bytes());
        d.extend_from_slice(b"ab\0Cd\0");
        let t = parse_str(&d);
        assert_eq!(t[&7], "ab");
        assert_eq!(t[&9], "Cd");
        assert!(parse_str(&[1, 2, 3]).is_empty());
    }
}
