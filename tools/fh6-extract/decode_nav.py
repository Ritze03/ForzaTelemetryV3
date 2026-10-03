#!/usr/bin/env python3
"""Decode FH6 OpenWorld/Brio/Freeroam/Brio_00.nav (magic 'WVAN') -> roads.json + roads.png (+ roads_on_map.png).

READ-ONLY on the game install. Needs: numpy, Pillow.
Usage: decode_nav.py [--media <...>/ForzaHorizon6/media] [--out ./fh6-out] [--nav other.nav] [--map square_map_image]

Layout (little-endian):
  0x00 'WVAN', u32 version=2, 8B id; zeros to 0x58
  0x58 u32 hdr[14]: [0]=nodes 38473, [1]=roads 1532, [2],[3]=40966 (road->node index list len),
       [4]=31139 (road-attached list len?), [5]=23 (#property names), [6]=2193 (#strings), ...
  0x90 nodes[hdr0] x 48B: f32 pos(x,y,z), f32 up(x,y,z), u16 a,u16 b,u16 c,u16 0, u32 id, u32 0, u32 l0, u32 l1
  then road table: 1531 x 24B {u32 count, u32 flags(lo16=4|5|6|8, hi16=?), u64 cum_end_in_A, u64 cum_end_in_B}
       + 8 more bytes (u32 count=4, u32 flags) for the 1532nd road (its u64s are absent)
  then A: u64[hdr2=40966] of NODE INDICES (0-based into nodes[]); road r = A[start_r : start_r+count_r]
       (junction nodes are shared between roads)
  then further tables (B etc., ~1.17MB, undecoded), then string pool (property names: road_type, road_level,
       oneway_forward, give_way, ...), ends file.
Full notes: docs/game-data/fh6-game-files.md
"""
import argparse, json, os, struct, sys
import numpy as np
from PIL import Image, ImageDraw

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, ci, resolve_media

# minimap.rs MapCalibration::DEFAULT (map px = ((x-ox)*ppm, (oz-z)*ppm), north=+z up)
PPM, OX, OZ = 0.3722, -12540.0, 10738.0
S = 4096

ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
add_media_args(ap)
ap.add_argument('--nav', help='explicit .nav file (default <media>/OpenWorld/Brio/Freeroam/Brio_00.nav)')
ap.add_argument('--map', help='square map image (e.g. map_summer_L3.png from extract_map.py) for the roads_on_map.png overlay; '
                              'any resolution, must show the whole map')
args = ap.parse_args()
media = resolve_media(args)
OUT = args.out
NAV = args.nav or ci(media, 'OpenWorld/Brio/Freeroam/Brio_00.nav')

d = open(NAV, 'rb').read()
assert d[:4] == b'WVAN'
hdr = struct.unpack_from('<14I', d, 0x58)
N, NR, NA = hdr[0], hdr[1], hdr[2]
nodes = np.frombuffer(d, dtype=[('p', '<f4', 3), ('up', '<f4', 3), ('a', '<u2'), ('b', '<u2'), ('c', '<u2'), ('d', '<u2'),
                                ('id', '<u4'), ('z', '<u4'), ('l0', '<u4'), ('l1', '<u4')], offset=0x90, count=N)
p = nodes['p']
off = 0x90 + 48 * N
rt = np.frombuffer(d, '<u4', offset=off, count=(NR - 1) * 6).reshape(-1, 6)
cnt = list(rt[:, 0].astype(int)) + [struct.unpack_from('<I', d, off + (NR - 1) * 24)[0]]
flags = list(rt[:, 1]) + [struct.unpack_from('<I', d, off + (NR - 1) * 24 + 4)[0]]
A = np.frombuffer(d, '<u8', offset=off + (NR - 1) * 24 + 8, count=NA).astype(np.int64)
assert sum(cnt) == NA
roads = []; s = 0
for c, f in zip(cnt, flags):
    roads.append((A[s:s + c], int(f))); s += c
print('nodes', N, 'roads', NR, 'A', NA, 'bounds x', p[:, 0].min(), p[:, 0].max(), 'z', p[:, 2].min(), p[:, 2].max(),
      'y', p[:, 1].min(), p[:, 1].max())

# --- export: split a road where consecutive nodes are >60 m apart (a few listed entries are jump links)
polys, attrs = [], []
for idx, f in roads:
    cur = [idx[0]]
    for a, b in zip(idx[:-1], idx[1:]):
        if np.linalg.norm(p[a] - p[b]) > 60:
            if len(cur) > 1: polys.append(cur); attrs.append(f)
            cur = []
        cur.append(b)
    if len(cur) > 1: polys.append(cur); attrs.append(f)
json.dump({'note': 'polylines of [x,z] world metres (same space as telemetry PositionX/Z); heights = node y per polyline vertex; cls = u32 flags of road record (lo16 kind 4/5/6/8, hi16 unknown id)',
           'polylines': [[[round(float(p[i][0]), 1), round(float(p[i][2]), 1)] for i in q] for q in polys],
           'heights': [[round(float(p[i][1]), 1) for i in q] for q in polys],
           'cls': [a & 0xffff for a in attrs], 'hi': [a >> 16 for a in attrs]},
          open(OUT + '/roads.json', 'w'), separators=(',', ':'))
print('polylines', len(polys))

COL = {4: (255, 255, 255), 5: (90, 200, 255), 6: (255, 190, 60), 8: (255, 80, 80)}

def render(img, tf, w):
    dr = ImageDraw.Draw(img)
    for q, f in sorted(zip(polys, attrs), key=lambda t: -(t[1] & 0xffff)):
        pts = [tf(p[i][0], p[i][2]) for i in q]
        dr.line(pts, fill=COL.get(f & 0xffff, (200, 200, 200)), width=w)

# plain render (north up, same convention as the map)
x0, x1, z0, z1 = p[:, 0].min() - 50, p[:, 0].max() + 50, p[:, 2].min() - 50, p[:, 2].max() + 50
k = (S - 1) / max(x1 - x0, z1 - z0)
im = Image.new('RGB', (S, S), (12, 14, 18))
render(im, lambda x, z: ((x - x0) * k, (z1 - z) * k), 2)
im.save(OUT + '/roads.png')

# overlay on map using the project's calibration (optional)
if args.map:
    try:
        Image.MAX_IMAGE_PIXELS = None
        m = Image.open(args.map).convert('RGB')
        f = S / 8192   # calibration is in 8192-px map space; any square image of the full map is rescaled to S
        m = m.resize((S, S)); m = Image.blend(m, Image.new('RGB', (S, S)), 0.35)
        render(m, lambda x, z: ((x - OX) * PPM * f, (OZ - z) * PPM * f), 2)
        m.save(OUT + '/roads_on_map.png')
    except Exception as e:
        print('overlay failed', e)
