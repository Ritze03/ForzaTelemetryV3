"""Pure-Python LZ4 *block* decoder (no frame header) - used by pgzp.py for GeoChunk entries with flags&0xff == 0x1f.

Slow-ish (~tens of MB/s at best) but dependency-free; if the optional `lz4` package is installed pgzp.py uses that instead.
"""


def lz4_block(src, usize=None):
    out = bytearray()
    i = 0
    n = len(src)
    while i < n and (usize is None or len(out) < usize):
        t = src[i]; i += 1
        ll = t >> 4
        if ll == 15:
            while True:
                b = src[i]; i += 1; ll += b
                if b != 255:
                    break
        out += src[i:i + ll]; i += ll
        if i >= n or (usize is not None and len(out) >= usize):
            break
        off = src[i] | (src[i + 1] << 8); i += 2
        ml = t & 15
        if ml == 15:
            while True:
                b = src[i]; i += 1; ml += b
                if b != 255:
                    break
        ml += 4
        if off == 0 or off > len(out):
            raise ValueError('bad LZ4 offset')
        s = len(out) - off
        if off >= ml:
            out += out[s:s + ml]
        else:                                   # overlapping match: byte-wise copy
            for k in range(ml):
                out.append(out[s + k])
    return bytes(out)
