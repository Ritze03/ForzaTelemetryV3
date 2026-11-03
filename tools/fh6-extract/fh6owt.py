"""Readers for the race-route files `OpenWorld/Brio/AITracks/Route<N>.owt` (racing line, magic 'FTWO') and the `RVAN` block of
`Route<N>.nav` (start line, finish line, 12-slot grid).  READ-ONLY.  Format notes: docs/game-data/fh6-game-files.md ("Race lines").

.owt layout (little endian):
  header u32[24] at 0:  [8] section count (1 normally; 4-5 in routes 132, 281, 351, 1181, 1281, 8008)   [9] node count
                        [11] low u16 = START NODE INDEX (the node the RVAN start_line sits on; 170/170 match)
                        [20] = node count again, except multi-section files   [21] 256 point-to-point / 257 circuit / 258 circuit with tail
  nodes at 0x60 + (0 if h[8] == 1 else 16 + 48 * (h[8] - 2)), h[9] x 56 B, then a 16 or 24 B tail (offset + count*56 + tail == file size).
  node: +0 f32[3] pos (x, height, z)   +12 f32[3] LEFT half-width vector (length 2.5-12.5 m; left edge = p + A, right edge = p - A)
        +24 f32[3] up (unit)   +36 i16,i16 (+40 copy) UNDECODED   +44 u16[4] run-constant tag UNDECODED   +52 u32 16 every 5-10 nodes UNDECODED
"""
import re, struct
import numpy as np

NODE = np.dtype([('p', '<f4', 3), ('a', '<f4', 3), ('u', '<f4', 3), ('s', '<i2', 2), ('s3', '<u2'), ('z', '<u2'), ('tag', '<u2', 4), ('flag', '<u4')])
assert NODE.itemsize == 56


def read_owt(path):
    """-> (nodes: structured array of NODE, header: tuple of 24 u32).  Raises AssertionError if the layout does not add up."""
    b = open(path, 'rb').read()
    assert b[:4] == b'FTWO', 'not an FTWO file'
    h = struct.unpack_from('<24I', b, 0)
    off = 0x60 + (0 if h[8] == 1 else 16 + 48 * (h[8] - 2))
    cnt = h[9]
    assert off + cnt * 56 + 16 in (len(b), len(b) - 8), (path, off, cnt, len(b))
    return np.frombuffer(b, NODE, offset=off, count=cnt), h


def start_node(h):
    return h[11] & 0xffff


def positions(path):
    """finite node positions (N,3) of a route's racing line (all sections; some routes have NaN nodes)"""
    a, _ = read_owt(path)
    p = a['p'].astype(float)
    return p[np.isfinite(p).all(axis=1)]


def parse_rvan(path):
    """RVAN block of a Route<N>.nav -> dict(A=start_line xyz, B=finish_line xyz, pts={name: (x,y,z,dx,dy,dz)}, gates=[centre xyz...]).
    Block: 'RVAN', u32 ver=2, u32 id, u32 size, then +0 start_line f32[4], +16 finish_line f32[4], +56 u32 nrect (76 B gate records from +80),
    +60 u32 npts=14, +64 u32 251, ..., 14 x 48 B {pos f32[3],0, dir f32[3],0, u32 idx,0,0,0}, NUL-terminated name pool (hash sorted =
    same order as the 14 records)."""
    b = open(path, 'rb').read()
    i = b.find(b'RVAN')
    sz = struct.unpack_from('<I', b, i + 12)[0]
    blk = b[i + 16:i + 16 + sz]
    A = struct.unpack_from('<3f', blk, 0); B = struct.unpack_from('<3f', blk, 16)
    nrect, npts, magic = struct.unpack_from('<3I', blk, 56)
    assert npts == 14 and magic == 251, (path, npts, magic)
    toks = re.findall(rb'[A-Za-z_0-9]+', blk[-(14 * 20 + 16):])[-npts:]
    p = blk.rfind(b'\0'.join(toks) + b'\0') - npts * 48
    pts = {}
    for k, tk in enumerate(toks):
        o = p + k * 48
        x, y, z = struct.unpack_from('<3f', blk, o); dx, dy, dz = struct.unpack_from('<3f', blk, o + 16)
        pts[tk.decode()] = (x, y, z, dx, dy, dz)
    gates = []
    for k in range(nrect):
        o = 80 + k * 76
        if o + 76 > p:
            break
        gates.append(struct.unpack_from('<3f', blk, o))
    return dict(A=A, B=B, pts=pts, gates=gates)
