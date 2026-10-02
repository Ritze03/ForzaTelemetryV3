#!/usr/bin/env python3
"""FH6 terrain: ELEVATION raster (from the terrain render meshes) and SURFACE-id raster (from the terrain collision meshes).
READ-ONLY on the install.   Needs numpy, Pillow (scipy optional).   Full notes + validation: docs/game-data/fh6-terrain.md.

Usage: extract_terrain.py [--media <...>/ForzaHorizon6/media] [--out ./fh6-out]
                          [--region X0,Z0,X1,Z1]   world metres (telemetry space); default = whole map (slow: ~10 min, ~3 GB RAM)
                          [--res 4]                elevation raster pixel size in metres (default 4)
                          [--surf-res 8]           surface raster pixel size in metres (default 8)
                          [--coarse]               use scene\\uberheightfield (486 low-detail files, 26 MB) instead of tbheightfield
                          [--no-elevation] [--no-surfaces]
Outputs (in --out):  elevation.npy (float32, NaN = no data; row 0 = north/z1), elevation.json (extent + pixel size), elevation.png (hillshaded),
                     surfaces.npy (uint16 dominant-by-area material id per pixel, 0xFFFF = no data), surfaces.json, surfaces.png (OUR class colours).

How it works (everything lives in Tracks/Brio/GeoChunk0.minizip, see pgzp.py):
 ELEVATION  scene\\tbheightfield\\autoterrain_x{X}_z{Z}_{cb|ul}_cluster{N}.i.modelbin  - Forza 'burG' model containers.
   File x/z = round(world/1023)*1023, so the 512 m world cell of a file is (round(X/1023)*512, round(Z/1023)*512) (== its bbox min corner; uberheightfield cells are 2048 m).
   Sections (directory of 24-byte entries: 4-char tag byte-reversed, ver, id, offset, size, size): Skel MatI Mesh IndB VLay VerB Modl;
   material/mesh names in 'emaN' chunks.  IndB/VerB have a 16 B header (u32 count, u32 bytes, u16 stride, u16 ?, u32 layout id).
   IndB = u16 indices.  First VerB: stride 8 = 3 x snorm16 position + 2 unused bytes; world = snorm/32767 * scale + centre, where
   scale/centre = the last 8 f32 of the Mesh section (sx,sy,sz,0,cx,cy,cz,0).  Mesh record: material index u16 @+2, first index u32 @+34,
   base vertex i32 @+38 (ignored - indices are absolute), index count u32 @+42.  The terrain surface is the mesh whose material name starts
   'GenMaterial_UberMap' (the other meshes in a cluster are skirts/props); 'cb' and 'ul' files are two sets of clusters that together cover
   the cells (what the difference is: unknown; ul fills holes of cb).  Several triangles per pixel -> keep the max y (bridges/overpasses win).
 SURFACES   scene\\tbheightfield\\autoterrain_x{X}_z{Z}_square{0..35}.phys - 6x6 squares of ~85 m per 512 m cell, 28259 files, 34.7 M triangles.
   u32 len + name; f32[3] bbox_min, pad; f32[3] bbox_max, pad; ...; s32 @ +64 from bbox_min = -vertexCount; vertices 10 B = 3 x u16 (x,y,z quantised
   over the bbox, ~1 mm) + 2 x u16 unknown; u32 triCount; triCount x 8 B {u8 flags, u8 materialSlot, u16 i0,i1,i2}; u32 n2 + n2 x 32 B
   undecoded; u32 ?; u32 nMat; nMat x (4 x u16) material table: cols 0-2 = per-corner material id, col 3 unknown; column 1 is used as the
   triangle's dominant id; + 8 B tail.   Ids are GLOBAL (~350 values, 56 occur on terrain).  The names behind the ids live in the ENCRYPTED
   Physics/surfaceTypes.xml, so the CLASSES below are OUR inference from where each id occurs, NOT game names.
"""
import argparse, json, os, re, struct, sys
import numpy as np
from PIL import Image, ImageDraw

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, ci, resolve_media
from pgzp import Pgzp

# whole-map extent of the telemetry coordinate system covered by the terrain (same origin as the map calibration in the docs)
X0, X1, Z0, Z1 = -12540.0, 9470.0, -11272.0, 10738.0


# ------------------------------------------------------------------------------------------------ burG model containers
def burg_sections(d):
    assert d[:4] == b'burG', 'not a burG container'
    ver, hsz, tot, n = struct.unpack_from('<IIII', d, 4)
    secs = []
    for k in range(n):
        o = 20 + 24 * k
        tag = d[o:o + 4][::-1].decode('latin1')
        v, x, off, sz, _sz2 = struct.unpack_from('<5I', d, o + 4)
        secs.append((tag, v, x, off, sz))
    return secs


def burg_names(d, secs):
    """names stored in 'emaN' chunks of the header area"""
    hdr = d[20 + 24 * len(secs):secs[0][3]]
    out = []
    for m in re.finditer(rb'emaN', hdr):
        p = m.start() + 16
        e = hdr.find(b'\0', p)
        out.append(hdr[p:e].decode('latin1') if e > p else '')
    return out


def terrain_mesh(d, want=lambda material: material.startswith('GenMaterial_UberMap')):
    """-> (world vertices Nx3 float32, triangles Mx3 int) of the meshes whose material name satisfies `want`"""
    s = burg_sections(d)
    names = burg_names(d, s)
    nmat = sum(1 for x in s if x[0] == 'MatI')
    ind = next(x for x in s if x[0] == 'IndB')
    _cnt, isz, stride, _, _ = struct.unpack_from('<IIHHI', d, ind[3])
    I = np.frombuffer(d[ind[3] + 16:ind[3] + 16 + isz], '<u2' if stride == 2 else '<u4')
    vb = next(x for x in s if x[0] == 'VerB')
    c, vsz, st, _, _lay = struct.unpack_from('<IIHHI', d, vb[3])
    assert st == 8, f'unexpected vertex stride {st}'
    B = np.frombuffer(d[vb[3] + 16:vb[3] + 16 + vsz], np.uint8).reshape(c, 8)
    P = B[:, :6].copy().view('<i2').reshape(-1, 3) / 32767.0
    mesh0 = next(x for x in s if x[0] == 'Mesh')
    g = np.array(struct.unpack_from('<8f', d, mesh0[3] + mesh0[4] - 32))
    W = (P * g[:3] + g[4:7]).astype(np.float32)
    tris = []
    for x in s:
        if x[0] != 'Mesh':
            continue
        o = x[3]
        mat = struct.unpack_from('<H', d, o + 2)[0]
        start, _base, n = struct.unpack_from('<IiI', d, o + 34)
        if mat >= nmat or mat >= len(names) or not want(names[mat]):
            continue
        t = I[start:start + n].astype(np.int64).reshape(-1, 3)
        if t.size and t.max() < c:
            tris.append(t)
    return W, (np.concatenate(tris) if tris else np.zeros((0, 3), np.int64))


# ------------------------------------------------------------------------------------------------ rasterisation
def raster_height(V, T, x0, z1, W, H, res, grid):
    """max-height rasterisation of triangles into `grid` (H x W, row 0 = z1 = north).  Pure numpy per triangle - slow but simple."""
    px = (V[:, 0] - x0) / res; pz = (z1 - V[:, 2]) / res; y = V[:, 1]
    xa, xb, xc = px[T[:, 0]], px[T[:, 1]], px[T[:, 2]]
    za, zb, zc = pz[T[:, 0]], pz[T[:, 1]], pz[T[:, 2]]
    ya, yb, yc = y[T[:, 0]], y[T[:, 1]], y[T[:, 2]]
    minx = np.floor(np.minimum(np.minimum(xa, xb), xc) - 0.5).astype(int); maxx = np.ceil(np.maximum(np.maximum(xa, xb), xc) - 0.5).astype(int)
    minz = np.floor(np.minimum(np.minimum(za, zb), zc) - 0.5).astype(int); maxz = np.ceil(np.maximum(np.maximum(za, zb), zc) - 0.5).astype(int)
    for k in range(len(T)):
        a0 = max(minx[k], 0); a1 = min(maxx[k], W - 1); b0 = max(minz[k], 0); b1 = min(maxz[k], H - 1)
        if a0 > a1 or b0 > b1:
            continue
        gx, gz = np.meshgrid(np.arange(a0, a1 + 1) + 0.5, np.arange(b0, b1 + 1) + 0.5)
        d = (zb[k] - zc[k]) * (xa[k] - xc[k]) + (xc[k] - xb[k]) * (za[k] - zc[k])
        if abs(d) < 1e-9:
            continue
        l1 = ((zb[k] - zc[k]) * (gx - xc[k]) + (xc[k] - xb[k]) * (gz - zc[k])) / d
        l2 = ((zc[k] - za[k]) * (gx - xc[k]) + (xa[k] - xc[k]) * (gz - zc[k])) / d
        l3 = 1 - l1 - l2
        m = (l1 >= -1e-6) & (l2 >= -1e-6) & (l3 >= -1e-6)
        if not m.any():
            continue
        h = l1 * ya[k] + l2 * yb[k] + l3 * yc[k]
        sub = grid[b0:b1 + 1, a0:a1 + 1]
        cur = sub[m]
        sub[m] = np.where(np.isnan(cur), h[m], np.maximum(cur, h[m]))


def hillshade(h, px_m, zf=3.0):
    gy, gx = np.gradient(np.nan_to_num(h, nan=100.0), px_m)
    slope = np.arctan(zf * np.hypot(gx, gy)); aspect = np.arctan2(gy, -gx)
    az = np.radians(315); alt = np.radians(40)
    return np.clip(np.sin(alt) * np.cos(slope) + np.cos(alt) * np.sin(slope) * np.cos(az - aspect), 0, 1)


def downsample_nan(a, n_w, n_h):
    """box-filter downsample that ignores NaN"""
    valid = ~np.isnan(a)
    v = np.asarray(Image.fromarray(np.where(valid, a, 0).astype(np.float32), 'F').resize((n_w, n_h), Image.BOX))
    m = np.asarray(Image.fromarray(valid.astype(np.float32), 'F').resize((n_w, n_h), Image.BOX))
    return np.where(m > 0.02, v / np.maximum(m, 1e-6), np.nan)


# ------------------------------------------------------------------------------------------------ main
def cell_of(fx, fz):
    return round(fx / 1023) * 512, round(fz / 1023) * 512


def main():
    ap = argparse.ArgumentParser(description='FH6 terrain elevation + surface-id rasters from GeoChunk0.minizip')
    add_media_args(ap)
    ap.add_argument('--region', help='X0,Z0,X1,Z1 in world metres (default: whole map)')
    ap.add_argument('--res', type=float, default=4.0, help='elevation pixel size in metres (default 4)')
    ap.add_argument('--surf-res', type=float, default=8.0, help='surface-id pixel size in metres (default 8)')
    ap.add_argument('--coarse', action='store_true', help='elevation from scene\\uberheightfield (low detail, whole island, much faster)')
    ap.add_argument('--no-elevation', action='store_true')
    ap.add_argument('--no-surfaces', action='store_true')
    args = ap.parse_args()
    media = resolve_media(args)
    rx0, rz0, rx1, rz1 = (X0, Z0, X1, Z1) if not args.region else [float(v) for v in args.region.split(',')]
    assert rx1 > rx0 and rz1 > rz0
    pg = Pgzp.open(media, 0)

    def in_region(cx, cz, size=512):
        return not (cx + size < rx0 or cx > rx1 or cz + size < rz0 or cz > rz1)

    if not args.no_elevation:
        elevation(pg, args, (rx0, rz0, rx1, rz1), in_region, media)
    if not args.no_surfaces:
        surfaces(pg, args, (rx0, rz0, rx1, rz1), in_region)


def elevation(pg, args, region, in_region, media):
    rx0, rz0, rx1, rz1 = region
    if args.coarse:
        pat = r'uberheightfield\\autouberlod_x(-?\d+)_z(-?\d+)_cluster(\d+)\.i\.modelbin$'
    else:
        pat = r'tbheightfield\\autoterrain_x(-?\d+)_z(-?\d+)_(cb|ul)_cluster(\d+)\.i\.modelbin$'
    rx = re.compile(pat)
    sel = []
    for i in pg.find(pat):
        m = rx.search(pg.names[i])
        cx, cz = cell_of(int(m.group(1)), int(m.group(2)))
        if in_region(cx, cz, 2048 if args.coarse else 512):
            sel.append((i, m.group(3) if not args.coarse else 'ul'))
    W = int(np.ceil((rx1 - rx0) / args.res)); H = int(np.ceil((rz1 - rz0) / args.res))
    print(f'elevation: {len(sel)} model files, raster {W}x{H} @ {args.res} m')
    grids = {'cb': np.full((H, W), np.nan, np.float32), 'ul': np.full((H, W), np.nan, np.float32)}
    ntri = 0
    for n, (i, kind) in enumerate(sel):
        d = pg.entry(i)
        V, T = terrain_mesh(d)
        if len(T):
            raster_height(V, T, rx0, rz1, W, H, args.res, grids[kind]); ntri += len(T)
        if n % 200 == 0:
            print(f'  {n}/{len(sel)} files, {ntri} triangles', flush=True)
    g = np.where(np.isnan(grids['cb']), grids['ul'], grids['cb'])          # cb first, ul fills the holes
    print(f'elevation: {ntri} terrain triangles, valid {np.mean(~np.isnan(g)) * 100:.1f}% of pixels, y {np.nanmin(g):.1f}..{np.nanmax(g):.1f}')
    np.save(os.path.join(args.out, 'elevation.npy'), g)
    json.dump({'x0': rx0, 'z0': rz0, 'x1': rx0 + W * args.res, 'z1': rz1, 'res_m': args.res, 'width': W, 'height': H,
               'note': 'row 0 = north (z1); pixel (c,r) covers x = x0 + c*res .. , z = z1 - r*res ..; sea level ~ y 100'},
              open(os.path.join(args.out, 'elevation.json'), 'w'))
    validate_elevation(media, g, rx0, rz1, args.res)
    render_elevation(g, args)


def nav_nodes(media):
    d = open(ci(media, 'OpenWorld/Brio/Freeroam/Brio_00.nav'), 'rb').read()
    n = struct.unpack_from('<I', d, 0x58)[0]
    a = np.frombuffer(d, np.uint8, count=n * 48, offset=0x90).reshape(n, 48)
    return a[:, :12].copy().view('<f4').reshape(n, 3)


def validate_elevation(media, g, x0, z1, res):
    """compare with the 38473 road-node heights (expect: median |dy| ~0.17 m, p90 ~11 m = bridges/tunnels, coverage 98% on the full map)"""
    P = nav_nodes(media)
    ix = ((P[:, 0] - x0) / res).astype(int); iz = ((z1 - P[:, 2]) / res).astype(int)
    inside = (ix >= 0) & (ix < g.shape[1]) & (iz >= 0) & (iz < g.shape[0])
    h = g[iz[inside], ix[inside]]
    ok = ~np.isnan(h)
    if inside.sum() == 0:
        return
    dy = np.abs(P[inside, 1][ok] - h[ok])
    print(f'validation vs road nodes: {inside.sum()} nodes in region, covered {ok.mean() * 100:.1f}%, median |dy| {np.median(dy):.2f} m, '
          f'p90 {np.percentile(dy, 90):.1f} m' if ok.any() else 'validation: no road node covered')


def render_elevation(g, args, maxpx=1600):
    N = min(maxpx, max(g.shape))
    nw = max(1, round(N * g.shape[1] / max(g.shape))); nh = max(1, round(N * g.shape[0] / max(g.shape)))
    h = downsample_nan(g, nw, nh)
    px_m = args.res * g.shape[1] / nw
    stops = [(0, (40, 80, 110)), (95, (60, 110, 120)), (100, (70, 120, 80)), (200, (120, 150, 80)), (400, (190, 180, 110)),
             (700, (150, 120, 90)), (1000, (200, 200, 200)), (1500, (255, 255, 255))]
    xs = [s[0] for s in stops]
    hh = np.nan_to_num(h, nan=-1)
    rgb = np.stack([np.interp(hh, xs, [s[1][c] for s in stops]) for c in range(3)], -1)
    img = np.clip(rgb * (0.35 + 0.65 * hillshade(h, px_m))[..., None], 0, 255)
    img[np.isnan(h)] = (18, 20, 26)
    im = Image.fromarray(img.astype(np.uint8))
    ImageDraw.Draw(im).text((8, 8), 'FH6 terrain elevation (telemetry y), hillshade x3, sea level ~100 m', fill=(230, 230, 230))
    im.save(os.path.join(args.out, 'elevation.png'))
    print('elevation.png', im.size)


# ------------------------------------------------------------------------------------------------ surfaces
# OUR grouping of terrain material ids by where they occur (NOT game names - the id -> name table is in the encrypted Physics/surfaceTypes.xml).
CLASSES = [  # name, colour, ids, paint-as-road-priority
    ('Asphalt road', (255, 255, 255), [9, 286], True),
    ('Road B: urban/dust? (ids 8,10)', (230, 140, 40), [8, 10], True),
    ('Road shoulder / verge (assumed)', (190, 190, 120), [280, 281, 242, 26], True),
    ('Concrete / pavement', (170, 170, 210), [27], True),
    ('Snow road', (120, 200, 255), [40], True),
    ('Water / flat lake+sea floor', (40, 110, 200), [207], False),
    ('Sand / seabed / shore', (235, 215, 120), [36, 39, 208, 60, 331], False),
    ('Snow / alpine rock', (235, 240, 250), [345, 211, 41, 328, 183], False),
    ('Riverbank stones / rock', (150, 120, 100), [230], False),
    ('Dirt / farmland / gravel', (180, 110, 60), [7, 56, 19, 279, 43], False),
    ('Forest floor / dense vegetation', (30, 110, 45), [31, 20, 340, 336, 339, 23, 46, 53, 342, 17, 239], False),
    ('Grass / meadow / lawn', (130, 190, 60), [22, 346], False),
    ('Other / unclassified id', (200, 60, 160), [], False),
]


def parse_phys(d):
    n = struct.unpack_from('<I', d, 0)[0]; o = 4 + n
    lo = np.array(struct.unpack_from('<3f', d, o)); hi = np.array(struct.unpack_from('<3f', d, o + 16))
    nv = -struct.unpack_from('<i', d, o + 64)[0]
    assert nv > 0
    vo = o + 68
    U = np.frombuffer(d, '<u2', count=5 * nv, offset=vo).reshape(nv, 5)
    p = vo + 10 * nv
    nt = struct.unpack_from('<I', d, p)[0]; p += 4
    T = np.frombuffer(d, np.uint8, count=8 * nt, offset=p).reshape(nt, 8)
    mat = T[:, 1]; idx = T[:, 2:].copy().view('<u2').reshape(nt, 3)
    p += 8 * nt
    n2 = struct.unpack_from('<I', d, p)[0]; p += 4 + 32 * n2
    _a, nm = struct.unpack_from('<II', d, p); p += 8
    M = np.frombuffer(d, '<u2', count=4 * nm, offset=p).reshape(nm, 4)
    P = lo + U[:, :3] / 65535.0 * (hi - lo)
    return P, idx, M[mat]


def surfaces(pg, args, region, in_region):
    rx0, rz0, rx1, rz1 = region
    pat = r'tbheightfield\\autoterrain_x(-?\d+)_z(-?\d+)_square(\d+)\.phys$'
    rx = re.compile(pat)
    sel = []
    for i in pg.find(pat):
        m = rx.search(pg.names[i])
        cx, cz = cell_of(int(m.group(1)), int(m.group(2)))
        if in_region(cx, cz):
            sel.append(i)
    res = args.surf_res
    W = int(np.ceil((rx1 - rx0) / res)); H = int(np.ceil((rz1 - rz0) / res))
    print(f'surfaces: {len(sel)} .phys files, raster {W}x{H} @ {res} m')
    keys = []; areas = []; ntri = 0
    for n, i in enumerate(sel):
        P, T, ids = parse_phys(pg.entry(i))
        A = P[T[:, 0]]; B = P[T[:, 1]]; C = P[T[:, 2]]
        ar = 0.5 * np.linalg.norm(np.cross(B - A, C - A), axis=1)
        cen = (A + B + C) / 3
        px = ((cen[:, 0] - rx0) / res).astype(np.int64); pz = ((rz1 - cen[:, 2]) / res).astype(np.int64)
        ok = (px >= 0) & (px < W) & (pz >= 0) & (pz < H)
        keys.append(((pz[ok] * W + px[ok]) * 1024 + ids[ok, 1].astype(np.int64))); areas.append(ar[ok]); ntri += len(T)
        if n % 5000 == 0:
            print(f'  {n}/{len(sel)} files', flush=True)
    if not keys:
        print('surfaces: nothing in region'); return
    key = np.concatenate(keys); ar = np.concatenate(areas)
    uk, inv = np.unique(key, return_inverse=True)
    w = np.bincount(inv, weights=ar)
    pix = uk // 1024; mid = uk % 1024
    order = np.lexsort((w, pix))                       # per pixel: ascending weight -> last one wins (dominant by area)
    pix, mid = pix[order], mid[order]
    last = np.r_[pix[1:] != pix[:-1], True]
    S = np.full(W * H, 0xFFFF, np.uint16); S[pix[last]] = mid[last]
    S = S.reshape(H, W)
    np.save(os.path.join(args.out, 'surfaces.npy'), S)
    ids_present = np.unique(mid)
    json.dump({'x0': rx0, 'z0': rz0, 'x1': rx0 + W * res, 'z1': rz1, 'res_m': res, 'width': W, 'height': H, 'nodata': 0xFFFF,
               'triangles': ntri, 'ids_present': ids_present.tolist(),
               'note': 'dominant (by area) material id per pixel; ids are the game\'s global physics-material ids, names unknown (encrypted table). '
                       'Class grouping in extract_terrain.CLASSES is our own inference.'}, open(os.path.join(args.out, 'surfaces.json'), 'w'))
    print(f'surfaces: {ntri} triangles, {len(ids_present)} distinct ids, ids = {ids_present.tolist()}')
    render_surfaces(S, args)


def render_surfaces(S, args):
    K = len(CLASSES)
    lut = np.full(1025, K - 1, np.int32)
    for k, (n, c, ids, _) in enumerate(CLASSES):
        for i in ids:
            lut[i] = k
    cls = lut[np.minimum(S, 1024)]
    img = np.zeros(S.shape + (3,), np.uint8)
    for k, (n, c, ids, _) in enumerate(CLASSES):
        img[cls == k] = c
    img[S == 0xFFFF] = (18, 20, 26)
    im = Image.fromarray(img)
    dr = ImageDraw.Draw(im)
    if min(S.shape) >= 400:                              # legend only if the raster is big enough not to be covered by it
        dr.rectangle([6, 6, 330, 10 + 18 * (K + 1)], fill=(18, 20, 26))
        y = 10
        for k, (n, c, ids, _) in enumerate(CLASSES):
            dr.rectangle([12, y + 3, 26, y + 13], fill=c)
            dr.text((34, y + 2), f'{n} [{",".join(map(str, ids[:5]))}{"..." if len(ids) > 5 else ""}]', fill=(235, 235, 235)); y += 18
        dr.text((12, y + 2), 'class names are OURS (inferred), not the game\'s', fill=(160, 160, 160))
    else:
        for k, (n, c, ids, _) in enumerate(CLASSES):
            print(f'  colour {c}: {n} {ids}')
    im.save(os.path.join(args.out, 'surfaces.png'))
    print('surfaces.png', im.size)


if __name__ == '__main__':
    main()
