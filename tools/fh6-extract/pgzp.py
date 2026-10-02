r"""Reader for FH6 `Tracks/Brio/GeoChunk<k>.minizip` ('PGZP' containers).  READ-ONLY; only the entries asked for are seek-read
(the files are 3.5-49 GB, never load them whole).  Format notes: docs/game-data/fh6-game-files.md ("GeoChunk / PGZP").

    pg = Pgzp.open(media, 0)                  # GeoChunk0 + the matching ChunkContentsMiniZip0.txt name list
    for i in pg.find(r'tbheightfield\\.*_square\d+\.phys$'):   # regex on the in-game path (lower-case, backslashes)
        data = pg.entry(i)                    # decoded bytes
"""
import re, struct, zlib
import numpy as np

from fh6common import ci
from lz4b import lz4_block as _py_lz4

try:                                           # optional speed-up: pip install lz4
    import lz4.block as _c_lz4

    def lz4_block(src, usize):
        return _c_lz4.decompress(bytes(src), uncompressed_size=usize)
except Exception:                              # pragma: no cover
    lz4_block = _py_lz4


def chunk_names(media, k):
    """ChunkContentsMiniZip<k>.txt -> list of in-game paths, line i == PGZP entry i.
    Lines look like `<PREZIPPED>d:\\scratch\\p4\\forte_main\\zipcache\\pc\\tracks\\brio\\scene\\...\\x.pgeo|<n>`; we strip the build-machine
    prefix and the `|n` suffix, leaving e.g. `tracks\\brio\\scene\\proc\\cellsize\\200\\3_4\\x.pgeo`."""
    p = ci(media, 'Tracks/Brio', f'ChunkContentsMiniZip{k}.txt')
    out = []
    for l in open(p, errors='replace'):
        l = l.rstrip('\n').rsplit('|', 1)[0].replace('<PREZIPPED>', '')
        out.append(l.split('zipcache\\pc\\')[-1])
    return out


class Pgzp:
    def __init__(self, path, names=None):
        self.f = open(path, 'rb')
        h = struct.unpack('<8I', self.f.read(32))
        assert h[0] == 0x505a4750 and h[1] == 101, 'not a PGZP v101 file'
        self.n, m, self.per, nseg = h[3], h[4], h[5], h[6]
        self.f.seek(32 + 4 * m)                                   # skip the u32[m] id list
        S = np.frombuffer(self.f.read(4 * (3 + 3 * self.n + 2 * nseg)), '<u4')
        assert S[0] == self.n
        self.segs = [int(S[1]) | int(S[2]) << 32]                 # absolute start offset of every segment (+ one past the end)
        body = S[3:]
        self.off = np.empty(self.n, np.uint64); self.us = np.empty(self.n, np.uint32)
        self.fl = np.empty(self.n, np.uint32); self.sg = np.empty(self.n, np.int32)
        p = k = 0
        for s in range(nseg):
            c = min(self.per, self.n - k)
            blk = body[p:p + 3 * c].reshape(-1, 3)
            self.off[k:k + c] = blk[:, 0]; self.us[k:k + c] = blk[:, 1]; self.fl[k:k + c] = blk[:, 2]; self.sg[k:k + c] = s
            p += 3 * c; k += c
            self.segs.append(int(body[p]) | int(body[p + 1]) << 32); p += 2
        self.names = names

    @classmethod
    def open(cls, media, k):
        names = chunk_names(media, k)
        pg = cls(ci(media, 'Tracks/Brio', f'GeoChunk{k}.minizip'), names)
        assert pg.n == len(names), (pg.n, len(names))
        return pg

    def find(self, regex, flags=re.I):
        """entry indices whose path matches `regex` (re.search), in on-disk order (fastest to read sequentially)."""
        rx = re.compile(regex, flags)
        idx = [i for i, n in enumerate(self.names) if rx.search(n)]
        idx.sort(key=lambda i: (int(self.sg[i]), int(self.off[i])))
        return idx

    def entry(self, r):
        s = int(self.sg[r]); o = int(self.off[r]); a = self.segs[s] + o
        # compressed size = next row's offset in the same segment, else up to the next segment start
        c = (int(self.off[r + 1]) - o) if (r + 1 < self.n and self.sg[r + 1] == s) else self.segs[s + 1] - a
        self.f.seek(a); d = self.f.read(c)
        us = int(self.us[r]); k = int(self.fl[r]) & 0xff
        if k == 0x1f: return lz4_block(d, us)                      # raw LZ4 block
        if k == 0x08: return zlib.decompress(d, -15)               # raw deflate
        return d[:us]                                              # stored (0x00)

    def close(self): self.f.close()
