#!/usr/bin/env python3
"""Build the LOCAL 3D map preview of the whole FH6 island: the elevation raster as a terrain mesh, the normal season map imagery draped on it,
and the road graph drawn on top.  Each road vertex carries TWO heights (page toggle "Road height"): the nav node heights (linear
interpolation along the edge; default) and the terrain-mesh drape.  READ-ONLY on the game; the output contains
Playground Games' imagery/data, so write it OUTSIDE the repo and never publish/commit it.

    python3 -B preview_3d.py --out <viewer dir> --work <viewer work dir> [--media <...>/ForzaHorizon6/media] [--road-types PATH]
                             [--seasons Spring,Summer,Autumn,Winter] [--tex-size 4096] [--decim 2] [--skirt 1000] [--step 5] [--lift 0.6]
    then open  <out>/preview-3d.html  in a browser (three.js comes from a CDN: needs internet once per session).

Needs (all produced by build_viewer.py / the extractors, nothing is re-extracted except the optional coarse hole filler):
  <work>/terr_e/elevation.npy + elevation.json   the 8 m elevation raster (extract_terrain.py)
  <work>/roads.json                              nav graph polylines + node ids (decode_nav.py)
  <out>/tiles/<Season>/3/<x>/<y>.jpg             the game's map tiles (build_viewer.py:build_tiles), level 3 = 8x8 tiles of 1024 px = the 8192 px map
  tools/fh6-extract/data/fh6-road-types.json     road types (default; --road-types PATH; version 1 and 2 of the editor export are read)
Output (<out>/):  preview-3d.html (from preview_3d.html) and preview3d/{meta,terrain,roads,tex_<Season>}.js  (window.P3D.* = ..., loaded by <script>,
no fetch(): file:// blocks it).  Notes + the why of the design choices: docs/game-data/fh6-terrain.md / README "3D preview".
"""
import argparse, base64, io, json, math, os, shutil, subprocess, sys, time, zlib
import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from fh6common import autodetect_media

T0 = time.time()
HMIN, HSTEP = -5.0, 0.025        # heights are stored as uint16 q = (h - HMIN) / HSTEP + 1 ; 0 = "no terrain here" (open sea beyond the skirt)
SEA_Y = 100.0                    # sea level in telemetry y
FLOOR_Y = 40.0                   # height of the filled sea floor beyond the island data
KNOWN_TYPES = ('road', 'highway', 'offroad', 'other', 'trail', 'crosscountry', 'tunnel', 'jump', 'turnaround')   # v2 editor list; anything else -> other
DEFAULT_TYPES = os.path.join(HERE, 'data', 'fh6-road-types.json')


def log(*a):
    print(f'[{time.time() - T0:6.1f}s]', *a, flush=True)


def b64z(a):
    return base64.b64encode(zlib.compress(np.ascontiguousarray(a).tobytes(), 9)).decode('ascii')


def write_js(path, name, obj):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w') as fh:
        fh.write(f'(window.P3D=window.P3D||{{}}).{name}=' + json.dumps(obj, separators=(',', ':'), ensure_ascii=True) + ';\n')
    log(f'{os.path.relpath(path, os.path.dirname(os.path.dirname(path)))}  {os.path.getsize(path) / 1e6:.2f} MB')


# ---------------------------------------------------------------------------------------------------------------------- terrain
def coarse_raster(media, cache):
    """The coarse uberheightfield raster (same extent/res as the detailed one; ~40 s) - only used to fill the few holes in the detailed one."""
    npy = os.path.join(cache, 'elevation.npy')
    if not os.path.isfile(npy):
        if not media:
            return None
        os.makedirs(cache, exist_ok=True)
        log('running extract_terrain.py --coarse (hole filler, ~40 s, cached)')
        r = subprocess.run([sys.executable, '-B', os.path.join(HERE, 'extract_terrain.py'), '--media', media, '--out', cache, '--coarse', '--res', '8',
                            '--no-surfaces'], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        if r.returncode or not os.path.isfile(npy):
            log('coarse hole filler FAILED (holes will be interpolated):', (r.stderr or '').strip().splitlines()[-1:] )
            return None
    return np.load(npy)


def fill_holes(E, C):
    """E: float32 raster, NaN = no data.  Returns (filled raster, sea mask).  Sea = big NaN regions (open water beyond the island data); the rest are
    holes (a missing 512 m cell, thin streaks): filled from the coarse raster where it has data, else interpolated from the rim."""
    from scipy import ndimage as ndi
    from scipy.interpolate import griddata
    nan = np.isnan(E)
    lab, n = ndi.label(nan)
    sizes = ndi.sum(nan, lab, range(1, n + 1))
    sea = np.isin(lab, [i + 1 for i, s in enumerate(sizes) if s > 20000])
    holes = nan & ~sea
    E = E.copy()
    if C is not None and C.shape == E.shape:
        use = holes & ~np.isnan(C)
        E[use] = C[use]
        log(f'holes: {int(holes.sum())} px, {int(use.sum())} filled from the coarse raster')
        holes &= np.isnan(E)
    if holes.any():
        lab, n = ndi.label(holes)
        for i, sl in enumerate(ndi.find_objects(lab)):
            r0, r1 = max(sl[0].start - 3, 0), min(sl[0].stop + 3, E.shape[0]); c0, c1 = max(sl[1].start - 3, 0), min(sl[1].stop + 3, E.shape[1])
            sub = E[r0:r1, c0:c1]; m = lab[r0:r1, c0:c1] == i + 1
            known = ~np.isnan(sub)
            if known.sum() < 3:
                continue
            ky, kx = np.nonzero(known); hy, hx = np.nonzero(m)
            v = griddata((ky, kx), sub[known], (hy, hx), method='linear')
            bad = np.isnan(v)
            if bad.any():
                v[bad] = griddata((ky, kx), sub[known], (hy[bad], hx[bad]), method='nearest')
            sub[hy, hx] = v
        log(f'holes: {int(holes.sum())} px interpolated from their rim')
    return E, sea


def build_terrain(work, out, coarse, decim, skirt):
    from scipy import ndimage as ndi
    d = os.path.join(work, 'terr_e')
    E = np.load(os.path.join(d, 'elevation.npy')).astype(np.float32)
    ej = json.load(open(os.path.join(d, 'elevation.json')))
    res, x0, z1 = float(ej['res_m']), float(ej['x0']), float(ej['z1'])
    H_, W_ = E.shape
    E, sea = fill_holes(E, coarse)
    # Open sea beyond the data: nearest data height, decaying to the sea floor over `skirt` m (a smooth skirt, so the mesh has no cliff and no pit);
    # further out nothing is meshed (q = 0) and the page's sea plane covers it.
    keep = np.ones(E.shape, bool)
    if sea.any():
        dist, (iy, ix) = ndi.distance_transform_edt(sea, return_indices=True)
        dm = dist * res
        w = np.clip(1 - dm / max(skirt, 1e-6), 0, 1) ** 2 if skirt > 0 else np.zeros_like(dm)
        near = E[iy, ix]
        E = np.where(sea, FLOOR_Y + (near - FLOOR_Y) * w, E).astype(np.float32)
        keep = ~sea | (dm <= skirt)
    rows, cols = np.where(keep)
    r0, r1, c0, c1 = rows.min(), rows.max() + 1, cols.min(), cols.max() + 1
    r0 -= r0 % decim; c0 -= c0 % decim
    r1 = min(H_, -(-r1 // decim) * decim); c1 = min(W_, -(-c1 // decim) * decim)
    r1 -= (r1 - r0) % decim; c1 -= (c1 - c0) % decim
    E, keep = E[r0:r1, c0:c1], keep[r0:r1, c0:c1]
    nz, nx = (r1 - r0) // decim, (c1 - c0) // decim
    blk = E.reshape(nz, decim, nx, decim).mean((1, 3))                  # block mean = anti-aliased decimation
    kept = keep.reshape(nz, decim, nx, decim).any((1, 3))
    q = np.clip(np.round((blk - HMIN) / HSTEP) + 1, 1, 65535)
    q = np.where(kept, q, 0).astype('<u2')
    step = decim * res
    # vertex (i, j): x = mx0 + i*step, z = mz0 - j*step  (pixel centres of the decimated grid; row 0 = north)
    mx0 = x0 + (c0 + decim / 2) * res; mz0 = z1 - (r0 + decim / 2) * res
    log(f'terrain mesh {nx}x{nz} verts @ {step:g} m ({int(kept.sum())} of {nx * nz} cells kept), data y {np.nanmin(blk):.1f}..{np.nanmax(blk):.1f}')
    meta = dict(nx=int(nx), nz=int(nz), mx0=mx0, mz0=mz0, step=step, X0=x0, Z1=z1, W=W_ * res, H=H_ * res, hmin=HMIN, hstep=HSTEP, sea=SEA_Y,
                floor=FLOOR_Y, data=b64z(q))
    write_js(os.path.join(out, 'preview3d', 'terrain.js'), 'terrain', meta)
    return q, meta


def deq(q):
    return np.where(q == 0, FLOOR_Y, HMIN + (q.astype(np.float64) - 1) * HSTEP)


def tri_height(Hm, meta, x, z):
    """Height of the terrain MESH at world (x, z): the page triangulates every cell as (i,j)(i,j+1)(i+1,j) + (i+1,j)(i,j+1)(i+1,j+1), i.e. the diagonal
    runs (i+1,j)-(i,j+1); interpolating on those exact triangles makes the road sit on the rendered surface, not on a bilinear patch that differs by a
    metre or two on steep 16 m cells."""
    fx = (np.asarray(x) - meta['mx0']) / meta['step']; fz = (meta['mz0'] - np.asarray(z)) / meta['step']
    fx = np.clip(fx, 0, meta['nx'] - 1.000001); fz = np.clip(fz, 0, meta['nz'] - 1.000001)
    i, j = fx.astype(int), fz.astype(int); a, b = fx - i, fz - j
    h00, h10, h01, h11 = Hm[j, i], Hm[j, i + 1], Hm[j + 1, i], Hm[j + 1, i + 1]
    lo = a + b <= 1
    return np.where(lo, h00 + a * (h10 - h00) + b * (h01 - h00), h11 + (1 - a) * (h01 - h11) + (1 - b) * (h10 - h11))


def bilinear_height(Hm, meta, x, z):
    fx = (np.asarray(x) - meta['mx0']) / meta['step']; fz = (meta['mz0'] - np.asarray(z)) / meta['step']
    fx = np.clip(fx, 0, meta['nx'] - 1.000001); fz = np.clip(fz, 0, meta['nz'] - 1.000001)
    i, j = fx.astype(int), fz.astype(int); a, b = fx - i, fz - j
    return (Hm[j, i] * (1 - a) * (1 - b) + Hm[j, i + 1] * a * (1 - b) + Hm[j + 1, i] * (1 - a) * b + Hm[j + 1, i + 1] * a * b)


# ----------------------------------------------------------------------------------------------------------------------- textures
def build_textures(out, seasons, size, quality):
    done = []
    for s in seasons:
        base = os.path.join(out, 'tiles', s, '3')
        if not os.path.isdir(base):
            log(f'texture {s}: {base} missing (run build_viewer.py first) - skipped')
            continue
        canvas = Image.new('RGB', (8192, 8192))
        try:
            for c in range(8):
                for r in range(8):
                    canvas.paste(Image.open(os.path.join(base, str(c), f'{r}.jpg')).convert('RGB'), (c * 1024, r * 1024))
        except Exception as e:
            log(f'texture {s}: tile missing/unreadable ({e}) - skipped'); continue
        if size < 8192:
            canvas = canvas.resize((size, size), Image.LANCZOS)
        buf = io.BytesIO()
        canvas.save(buf, 'JPEG', quality=quality, optimize=True, progressive=True)
        uri = 'data:image/jpeg;base64,' + base64.b64encode(buf.getvalue()).decode('ascii')
        p = os.path.join(out, 'preview3d', f'tex_{s}.js')
        with open(p, 'w') as fh:
            fh.write(f'(window.P3D=window.P3D||{{}}).tex_{s}={json.dumps(uri)};\n')
        log(f'tex_{s}.js  {size}x{size} jpeg q{quality}  {os.path.getsize(p) / 1e6:.2f} MB')
        done.append(s)
    return done


# ------------------------------------------------------------------------------------------------------------------------- roads
def load_types(path):
    """Road-type file (editor export v1 or v2) -> dict(types, added, removed, points, moved) with every type normalised."""
    t = json.load(open(path))
    if t.get('format') != 'fh6-road-types':
        raise ValueError(f'{path} is not an fh6-road-types file')
    norm = lambda v: v if v in KNOWN_TYPES else 'other'
    return dict(version=t.get('version', 1),
                types={k: norm(v) for k, v in (t.get('types') or {}).items()},
                added=[(int(e['a']), int(e['b']), norm(e.get('type'))) for e in (t.get('added') or [])],
                removed=set(t.get('removed') or []),
                points={int(k): v for k, v in (t.get('points') or {}).items()},
                moved={int(k): v for k, v in (t.get('moved') or {}).items()})


def build_roads(work, out, Hm, meta, types_path, step, lift):
    r = json.load(open(os.path.join(work, 'roads.json')))
    if 'ids' not in r:
        raise RuntimeError('roads.json has no node ids - re-run decode_nav.py')
    T = load_types(types_path)
    pos = {}                                                   # node id -> (x, z)
    for pl, ids in zip(r['polylines'], r['ids']):
        for p, i in zip(pl, ids):
            pos[int(i)] = (float(p[0]), float(p[1]))
    for i, v in T['points'].items():
        pos[i] = (float(v[0]), float(v[1]))
    for i, v in T['moved'].items():
        pos[i] = (float(v[0]), float(v[1]))
    ny = {}                                                    # node id -> nav node height (roads.json heights; v2 user points / moved nodes: their stored y)
    for hs, ids in zip(r['heights'], r['ids']):
        for h, i in zip(hs, ids):
            ny[int(i)] = float(h)
    for src in (T['points'], T['moved']):
        for i, v in src.items():
            if len(v) > 2 and v[2] is not None:
                ny[i] = float(v[2])
    key = lambda a, b: f'{min(a, b)}-{max(a, b)}'
    seen, runs = set(), []                                     # runs: (type, [node ids])
    miss = 0
    for ids in r['ids']:
        cur = None
        for a, b in zip(ids[:-1], ids[1:]):
            k = key(a, b)
            if k in seen or k in T['removed'] or a not in pos or b not in pos:
                cur = None; continue
            seen.add(k)
            ty = T['types'].get(k)
            if ty is None:
                ty = 'other'; miss += 1                        # edge not painted in the project data
            if cur is not None and cur[0] == ty and cur[1][-1] == a:
                cur[1].append(b)
            else:
                cur = (ty, [a, b]); runs.append(cur)
    nadd = 0
    for a, b, ty in T['added']:
        k = key(a, b)
        if k in seen or k in T['removed'] or a not in pos or b not in pos:
            continue
        seen.add(k); runs.append((ty, [a, b])); nadd += 1
    log(f'roads: {len(seen)} edges ({miss} unpainted -> other, {nadd} added links, {len(T["removed"])} removed), {len(runs)} runs; types file v{T["version"]}')
    per, samples, nofb = {}, [], 0
    for ty, ids in runs:
        P = np.array([pos[i] for i in ids], float)
        d = np.r_[0, np.cumsum(np.hypot(*np.diff(P, axis=0).T))]
        if d[-1] < 0.5:
            continue
        t = np.unique(np.r_[d, np.arange(0, d[-1], step)])                          # the original vertices + a uniform step
        x = np.interp(t, d, P[:, 0]); z = np.interp(t, d, P[:, 1])
        y = tri_height(Hm, meta, x, z) + lift
        hn = np.array([ny.get(i, np.nan) for i in ids])           # node heights; a node without one (user point without y) falls back to the terrain
        bad = np.isnan(hn)
        if bad.any():
            nofb += 1
            hn[bad] = (tri_height(Hm, meta, P[bad, 0], P[bad, 1]))
        yn = np.interp(t, d, hn) + lift                           # linear along the edge between the two nodes' heights
        per.setdefault(ty, []).append(np.stack([x, y, z, t, yn], 1).astype('<f4'))
        samples.append((ty, ids, d, t))
    out_t, stats = {}, {}
    for ty, arrs in per.items():
        starts = np.r_[0, np.cumsum([len(a) for a in arrs])].astype('<u4')
        xyzs = np.concatenate(arrs)
        km = sum(a[-1, 3] for a in arrs) / 1000
        # xyz t stay float32 (4 columns); the node-height column is a uint16 quantised like the terrain (0.025 m): float32 would add ~0.4 MB for no visible gain
        yq = np.clip(np.round((xyzs[:, 4].astype(np.float64) - HMIN) / HSTEP), 0, 65535).astype('<u2')
        out_t[ty] = dict(runs=b64z(starts), n=int(len(xyzs)), xyzs=b64z(np.ascontiguousarray(xyzs[:, :4])), yn=b64z(yq), km=round(float(km), 1))
        stats[ty] = (len(arrs), len(xyzs), km)
    write_js(os.path.join(out, 'preview3d', 'roads.js'), 'roads', dict(types=out_t, lift=lift, step=step, hmin=HMIN, hstep=HSTEP))
    if nofb:
        log(f'roads: {nofb} runs had a node without a height (fell back to the terrain there)')
    for ty, (n, m, km) in sorted(stats.items()):
        log(f'    {ty:13s} {n:5d} runs {m:7d} samples {km:8.1f} km')
    return per, samples, ny


def numeric_check(per, samples, ny, Hm, meta, lift, out, rng=np.random.default_rng(7)):
    """End-to-end check on the WRITTEN roads.js (decoded again): 5 random road samples must satisfy
         terrain y == mesh-triangle height + lift                      (exact, float32 rounding)
         node y    == linear interpolation of the two bracketing nav nodes' heights + lift   (0.0125 m quantisation)
       The node interpolation is recomputed here from the raw node ids / heights, independently of the build loop.  Also prints how far the node heights
       are from the terrain (sinking / floating) and the steepest node-to-node grades (suspect node heights)."""
    txt = open(os.path.join(out, 'preview3d', 'roads.js')).read()
    R = json.loads(txt[txt.index(').roads=') + 8: txt.rindex(';')])
    dec = lambda b64, dt: np.frombuffer(zlib.decompress(base64.b64decode(b64)), dt)
    print('numeric check (5 random road samples): type x z | y terrain | tri+lift | y node | interp node+lift | err')
    worst_t = worst_n = 0.0
    idx_in_type = {}
    runs_of = {}
    for k, (ty, ids, d, t) in enumerate(samples):
        runs_of.setdefault(ty, []).append(k)
    for k in rng.choice(len(samples), 5, replace=False):
        ty, ids, d, t = samples[k]
        j = runs_of[ty].index(k)
        D = R['types'][ty]
        starts = dec(D['runs'], '<u4'); xyzs = dec(D['xyzs'], '<f4').reshape(-1, 4); yq = dec(D['yn'], '<u2')
        a, b = starts[j], starts[j + 1]
        m = int(rng.integers(a, b))
        x, y, z, tt = xyzs[m]
        yn = R['hmin'] + float(yq[m]) * R['hstep']
        # independent interpolation: bracketing nodes of distance tt
        seg = min(max(int(np.searchsorted(d, tt, side='right')) - 1, 0), len(ids) - 2)
        f = (tt - d[seg]) / (d[seg + 1] - d[seg])
        h0 = ny.get(ids[seg]); h1 = ny.get(ids[seg + 1])
        exp = (h0 + (h1 - h0) * f + lift) if h0 is not None and h1 is not None else float('nan')
        tri = float(tri_height(Hm, meta, x, z)) + lift
        worst_t = max(worst_t, abs(y - tri)); worst_n = max(worst_n, abs(yn - exp))
        print(f'    {ty:12s} {x:9.1f} {z:9.1f} | {y:9.3f} | {tri:9.3f} | {yn:9.3f} | {exp:9.3f} | {yn - exp:+.4f}')
    log(f'numeric check: max |terrain y - (tri + lift)| = {worst_t:.4f} m (float32 rounding); max |node y - interpolated node height - lift| = {worst_n:.4f} m (0.0125 = uint16 step/2)')
    # how node heights relate to the mesh, over every sample (diagnostic for the report)
    allx = np.concatenate([a[:, 0] for v in per.values() for a in v]).astype(float); allz = np.concatenate([a[:, 2] for v in per.values() for a in v]).astype(float)
    ally = np.concatenate([a[:, 4] for v in per.values() for a in v]).astype(float) - lift
    dn = ally - tri_height(Hm, meta, allx, allz)
    log(f'node y - terrain(tri) over all samples: median {np.median(dn):+.2f} m, p1 {np.percentile(dn, 1):+.2f}, p99 {np.percentile(dn, 99):+.2f}; '
        f'{100 * np.mean(dn < -1):.1f}% sink >1 m below the mesh, {100 * np.mean(dn < -3):.1f}% >3 m, {100 * np.mean(dn > 3):.1f}% float >3 m above')
    worst = []
    for ty, ids, d, t in samples:
        if ty == 'tunnel':
            continue
        for q in range(len(ids) - 1):
            a, b = ids[q], ids[q + 1]
            if a in ny and b in ny and d[q + 1] - d[q] > 0.5:
                worst.append((abs(ny[b] - ny[a]) / (d[q + 1] - d[q]), ty, a, b, ny[a], ny[b], d[q + 1] - d[q]))
    worst.sort(reverse=True)
    log('steepest node-to-node grades (non-tunnel): ' + '; '.join(f'{g * 100:.0f}% {ty} {a}->{b} ({ha:.1f}->{hb:.1f} m over {ln:.1f} m)' for g, ty, a, b, ha, hb, ln in worst[:5]))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--media', help='<ForzaHorizon6>/media (only used for the optional coarse hole filler; default: auto-detect via Steam)')
    ap.add_argument('--out', default='./fh6-viewer', help='the map viewer output dir (default ./fh6-viewer; tiles/ must exist) - keep it OUTSIDE the repo')
    ap.add_argument('--work', help='extractor outputs (default <out>-work): needs terr_e/elevation.npy and roads.json')
    ap.add_argument('--road-types', default=DEFAULT_TYPES, help='fh6-road-types.json (editor export v1/v2); default: the project data')
    ap.add_argument('--seasons', default='Spring,Summer,Autumn,Winter')
    ap.add_argument('--tex-size', type=int, default=4096, choices=(2048, 4096, 8192), help='texture size per season (default 4096: 5.4 m/texel; 8192 = full map res, needs a GPU with 8192 textures)')
    ap.add_argument('--jpeg-quality', type=int, default=82)
    ap.add_argument('--decim', type=int, default=2, choices=(1, 2, 4), help='terrain mesh = raster / decim (default 2 -> 16 m vertices)')
    ap.add_argument('--skirt', type=float, default=1000, help='metres of smooth skirt beyond the island data (0 = hard crop)')
    ap.add_argument('--step', type=float, default=5.0, help='road resample step in metres')
    ap.add_argument('--lift', type=float, default=0.6, help='metres the road centreline sits above the terrain')
    ap.add_argument('--no-coarse', action='store_true', help='do not fetch the coarse raster to fill holes (interpolate them instead)')
    a = ap.parse_args()
    out = os.path.abspath(a.out); work = os.path.abspath(a.work or out.rstrip('/') + '-work')
    repo = os.path.abspath(os.path.join(HERE, '..', '..'))
    if os.path.commonpath([out, repo]) == repo:
        sys.exit(f'refusing to write game data inside the repo ({out}); choose --out outside {repo}')
    for f in (os.path.join(work, 'terr_e', 'elevation.npy'), os.path.join(work, 'roads.json')):
        if not os.path.isfile(f):
            sys.exit(f'missing {f} - run build_viewer.py first (it extracts elevation and the nav graph)')
    os.makedirs(os.path.join(out, 'preview3d'), exist_ok=True)
    coarse = None if a.no_coarse else coarse_raster(a.media or autodetect_media(), os.path.join(out, 'preview3d', 'cache', 'terr_c'))
    q, meta = build_terrain(work, out, coarse, a.decim, a.skirt)
    Hm = deq(q.reshape(meta['nz'], meta['nx']))
    seasons = build_textures(out, [s for s in a.seasons.split(',') if s], a.tex_size, a.jpeg_quality)
    per, samples, ny = build_roads(work, out, Hm, meta, a.road_types, a.step, a.lift)
    numeric_check(per, samples, ny, Hm, meta, a.lift, out)
    write_js(os.path.join(out, 'preview3d', 'meta.js'), 'meta', dict(seasons=seasons, default='Summer' if 'Summer' in seasons else (seasons[0] if seasons else None),
                                                                      tex_size=a.tex_size, built=time.strftime('%Y-%m-%d %H:%M')))
    shutil.copyfile(os.path.join(HERE, 'preview_3d.html'), os.path.join(out, 'preview-3d.html'))
    log('wrote', os.path.join(out, 'preview-3d.html'))


if __name__ == '__main__':
    main()
