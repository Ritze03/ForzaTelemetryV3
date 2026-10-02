#!/usr/bin/env python3
"""FH6 speed-limit-sign instances from GeoChunk0 `signs_do` pgeo cells -> speedsigns.json.   READ-ONLY on the install.

Source: Tracks/Brio/GeoChunk0.minizip (PGZP; only the 1182 `c300_signs_do*.pgeo` entries are seek-read, ~5 MB, a few seconds).  Models:
   sgn_gbl_info_speed_01_a        1659  speed-limit sign (red ring)            kind 'limit'
   sgn_gbl_info_high_speed_01_a    217  same family on a pole / expressway     kind 'limit_high'
   sgn_gbl_info_pfb_speedend_01_a  265  "end of speed limit" (white disc, slash) kind 'limit_end'
Instance (80 B): 3 x u32 sign-magnitude 16.16 pos; right f32[3]; up f32[3]; scale f32[3]; then 32 B =
   +48 u32 0x80800000 (const)  +52 0xffffffff (const)  +56 0  +60 u32 VARIANT 0..5  +64 0  +68 u32 per-instance hash  +72 0  +76 u32 256|512
VARIANT (+60) = the material variant: the model has materials SGN_GBL_INFO_SPEED_01_A_Alpha_Retro (=0) and _VARIANT_1.._5; their MatI blocks
differ only by two float params = a UV offset ((0,0) (.25,0) (.5,0) (.75,0) (0,.25) (.25,.25)) i.e. a cell of a 4-column atlas, which is
where the printed number comes from.  The atlas texture is NOT in the model's swatches (they hold the blank red-ring face) -> the km/h
value of each variant is not stored anywhere we can read: `limit_kmh` is null unless --variant-map "0=50,1=60,..." is given (needs an
in-game check; see docs).
heading_deg = direction the sign FACE looks (towards oncoming traffic): face normal = -(right x up) (the face quad sits at local z=-0.05,
the pole at z>0), 0 = +z (north), 90 = +x.  97.4 % of the signs are consistent with left-hand traffic.
road_dist_m / road_class = nearest Brio_00.nav road polyline (needs roads.json from decode_nav.py, found in --out or via --roads; scipy).
Output is Playground Games' data: write it outside the repo.

Usage: extract_speedsigns.py [--media ...] [--out DIR] [--roads roads.json] [--variant-map 0=50,1=60,...]
"""
import argparse, collections, json, math, os, re, struct, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, resolve_media
from pgzp import Pgzp

KINDS = {'sgn_gbl_info_speed_01_a': 'limit', 'sgn_gbl_info_high_speed_01_a': 'limit_high', 'sgn_gbl_info_pfb_speedend_01_a': 'limit_end'}


def sm(u):
    """sign-magnitude 16.16 fixed point"""
    v = (u & 0x7fffffff) / 65536.0
    return -v if u & 0x80000000 else v


def instances(d, model):
    """every instance of `model` (lower-case name, without _3D) in a pgeo blob -> list of dicts"""
    key = (model + '_3D').encode(); out = []
    for m in re.finditer(re.escape(key), d, re.I):
        o = m.start() - 4
        if o < 0 or struct.unpack_from('<I', d, o)[0] != len(key): continue
        q = o + 8 + len(key); cnt = struct.unpack_from('<I', d, q - 4)[0]
        for k in range(cnt):
            b = d[q + k * 80:q + (k + 1) * 80]
            if len(b) < 80: break
            x, y, z = (sm(u) for u in struct.unpack_from('<3I', b, 0))
            r = np.array(struct.unpack_from('<3f', b, 12)); u = np.array(struct.unpack_from('<3f', b, 24))
            t = struct.unpack_from('<8I', b, 48)
            out.append(dict(x=x, y=y, z=z, r=r, u=u, variant=t[3], flag=t[7], hash=t[5]))
        break
    return out


def road_index(path):
    from scipy.spatial import cKDTree
    R = json.load(open(path)); pts = []; cls = []
    for l, c in zip(R['polylines'], R['cls']):
        l = np.array(l, float)
        for a, b in zip(l[:-1], l[1:]):
            L = float(np.hypot(*(b - a)))
            if L < 1e-6: continue
            k = max(int(L / 2), 1); t = (np.arange(k) / k)[:, None]
            pts.append(a + (b - a) * t); cls += [c & 0xffff] * k
    pts = np.vstack(pts)
    return cKDTree(pts), np.array(cls)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0]); add_media_args(ap)
    ap.add_argument('--roads', help='roads.json from decode_nav.py (default: <out>/roads.json if present) - adds road_dist_m / road_class')
    ap.add_argument('--variant-map', default='', help='km/h per variant, e.g. 0=50,1=60,2=70 - only if you know it (otherwise limit_kmh = null)')
    args = ap.parse_args(); media = resolve_media(args)
    vmap = {int(a): int(b) for a, b in (p.split('=') for p in args.variant_map.split(',') if p)}
    pg = Pgzp.open(media, 0)
    out = []
    for i in pg.find(r'_signs_do[^\\]*\.pgeo$'):
        d = pg.entry(i); cell = pg.names[i].split('cellsize\\')[-1]
        for model, kind in KINDS.items():
            for s in instances(d, model):
                n = -np.cross(s['r'], s['u']); hd = math.degrees(math.atan2(n[0], n[2])) % 360
                out.append(dict(x=round(s['x'], 2), y=round(s['y'], 2), z=round(s['z'], 2), heading_deg=round(hd, 1), kind=kind, model=model,
                                variant=s['variant'], limit_kmh=vmap.get(s['variant']) if kind != 'limit_end' else None, cell=cell))
    roads = args.roads or os.path.join(args.out, 'roads.json')
    if os.path.exists(roads):
        try:
            T, C = road_index(roads)
            dd, ii = T.query(np.array([[s['x'], s['z']] for s in out]))
            for s, dist, j in zip(out, dd, ii):
                s['road_dist_m'] = round(float(dist), 1); s['road_class'] = int(C[j])
        except ImportError:
            print('scipy missing - road_dist_m skipped')
    else:
        print(f'no roads.json ({roads}) - road_dist_m skipped; run decode_nav.py first or pass --roads')
    with open(os.path.join(args.out, 'speedsigns.json'), 'w') as fh:
        json.dump(out, fh, separators=(',', ':'))
    print(len(out), 'signs;', dict(collections.Counter(s['kind'] for s in out)))
    print('by (kind, variant):', sorted(collections.Counter((s['kind'], s['variant']) for s in out).items()))
    if out and 'road_dist_m' in out[0]:
        print('road snap median %.1f m' % np.median([s['road_dist_m'] for s in out]))


if __name__ == '__main__':
    main()
