#!/usr/bin/env python3
"""Extract FH6 world-map images from the game install to PNG (read-only on the game files).

Map_Brio_<Season>.zip holds a tile pyramid: <level>-<a>-<b>.swatchbin, level L has 2^L x 2^L
tiles of 1024x1024 BC1. Level 3 = 8x8 tiles = 8192x8192 (the bundled-jpg size).
Usage: extract_map.py [--media <...>/ForzaHorizon6/media] [--out ./fh6-out] [--level 3] [--seasons Summer,...] [--swap]
Needs: numpy, Pillow.  Full notes: docs/game-data/fh6-game-files.md
"""
import argparse, os, struct, sys, zipfile
import numpy as np
from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, ci, resolve_media

TILE = 1024

def parse_swatchbin(b):
    """Header (all LE u32): 0x00 'burG' magic, 0x04 ver 0x0101, 0x08 hdr_size(140), 0x0c total size,
    0x4c width, 0x50 height, 0x54 mip count, 0x80 top-mip data size; pixel data starts at hdr_size."""
    assert b[:4] == b"burG", "not a swatchbin"
    hdr, total = struct.unpack_from("<II", b, 8)
    w, h, mips = struct.unpack_from("<III", b, 0x4C)
    dsz = struct.unpack_from("<I", b, 0x80)[0]
    assert total == len(b)
    return w, h, mips, b[hdr:hdr + dsz]

def decode_bc1(data, w, h):
    """BC1 -> RGBA uint8 (h,w,4), numpy only."""
    bw, bh = w // 4, h // 4
    blk = np.frombuffer(data, dtype=np.uint8, count=bw * bh * 8).reshape(bh, bw, 8)
    c = blk[..., :4].copy().view("<u2").reshape(bh, bw, 2)
    c0, c1 = c[..., 0].astype(np.int32), c[..., 1].astype(np.int32)
    def rgb(c):
        r = (c >> 11) & 31; g = (c >> 5) & 63; bl = c & 31
        return np.stack([(r * 255 + 15) // 31, (g * 255 + 31) // 63, (bl * 255 + 15) // 31], -1)
    p0, p1 = rgb(c0), rgb(c1)
    four = (c0 > c1)[..., None]
    p2 = np.where(four, (2 * p0 + p1) // 3, (p0 + p1) // 2)
    p3 = np.where(four, (p0 + 2 * p1) // 3, 0)
    pal = np.stack([p0, p1, p2, p3], -2)  # (bh,bw,4,3)
    idx = blk[..., 4:].copy().view("<u4").reshape(bh, bw)
    sh = (np.arange(16) * 2).astype(np.uint32)
    ix = ((idx[..., None] >> sh) & 3).reshape(bh, bw, 4, 4)  # [..., y, x]
    out = np.empty((bh, bw, 4, 4, 3), np.uint8)
    for k in range(4):
        m = ix == k
        out[m] = pal[:, :, k, :][:, :, None, None, :].repeat(4, 2).repeat(4, 3)[m]
    out = out.transpose(0, 2, 1, 3, 4).reshape(h, w, 3)
    return out

def build(zpath, level, swap=False, step=1):
    n = 1 << level
    z = zipfile.ZipFile(zpath)
    img = np.zeros((n * TILE, n * TILE, 3), np.uint8)
    for a in range(n):
        for b in range(n):
            w, h, mips, data = parse_swatchbin(z.read(f"{level}-{a}-{b}.swatchbin"))
            assert (w, h, mips) == (TILE, TILE, 1), (w, h, mips)
            row, col = (b, a) if swap else (a, b)
            img[row * TILE:(row + 1) * TILE, col * TILE:(col + 1) * TILE] = decode_bc1(data, w, h)
    return img

def main():
    ap = argparse.ArgumentParser()
    add_media_args(ap)
    ap.add_argument("--level", type=int, default=3)
    ap.add_argument("--seasons", default="Spring,Summer,Autumn,Winter")
    ap.add_argument("--swap", action="store_true", help="name is <level>-<col>-<row> instead of <row>-<col>")
    a = ap.parse_args()
    media = resolve_media(a)
    d = ci(media, "UI/Textures/Data_Bound")
    for s in a.seasons.split(","):
        zp = ci(d, f"map_brio_{s}.zip")
        img = build(zp, a.level, a.swap)
        out = os.path.join(a.out, f"map_{s.lower()}_L{a.level}.png")
        im = Image.fromarray(img); im.save(out)
        im.resize((1600, 1600), Image.LANCZOS).save(out.replace(".png", "_preview.png"))
        print(s, img.shape, out)

if __name__ == "__main__":
    main()
