#!/usr/bin/env python3
"""Build the LOCAL 3D map preview of the whole FH6 island: the elevation raster as a terrain mesh, the normal season map imagery draped on it,
and the road graph drawn on top.  The roads are shipped as a plain EDGE LIST (nodes + typed edges, preview3d/roads.js); the page turns it into ribbons itself
(the same code draws the live state of the editor, which postMessage's its edge list into the page).  Each road vertex gets TWO heights (page toggle "Road
height"): the nav node heights (linear interpolation along the edge; default) and the terrain-mesh drape.  READ-ONLY on the game; the output contains
Playground Games' imagery/data, so write it OUTSIDE the repo and never publish/commit it.

    python3 -B preview_3d.py --out <viewer dir> --work <viewer work dir> [--media <...>/ForzaHorizon6/media] [--road-types PATH]
                             [--seasons Spring,Summer,Autumn,Winter] [--tex-size 4096] [--decim 2] [--skirt 1000] [--step 5] [--lift 0.6]
    then open  <out>/preview-3d.html  in a browser (three.js comes from a CDN: needs internet once per session).

Needs (all produced by build_viewer.py / the extractors, nothing is re-extracted except the optional coarse hole filler):
  <work>/terr_e/elevation.npy + elevation.json   the 8 m elevation raster (extract_terrain.py)
  <work>/roads.json                              nav graph polylines + node ids (decode_nav.py)
  <out>/tiles/<Season>/3/<x>/<y>.jpg             the game's map tiles (build_viewer.py:build_tiles), level 3 = 8x8 tiles of 1024 px = the 8192 px map
  assets/map/fh6-road-types.json                 road types (default; --road-types PATH; version 1 and 2 of the editor export are read)
Output (<out>/):  preview-3d.html (from assets/editor/preview-3d.html) and lib/ and preview3d/{meta,terrain,roads,tex_<Season>}.js (roads = the edge list)  (window.P3D.* = ..., loaded by <script>,
no fetch(): file:// blocks it).  Notes + the why of the design choices: docs/game-data/fh6-terrain.md / README "3D preview".
"""
import argparse, base64, io, json, math, os, shutil, subprocess, sys, time, zlib
import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
EDITOR = os.path.join(HERE, '..', '..', 'assets', 'editor')      # index.html + preview-3d.html + lib/ (shared with the app)
sys.path.insert(0, HERE)
from fh6common import autodetect_media

T0 = time.time()
HMIN, HSTEP = -5.0, 0.025        # heights are stored as uint16 q = (h - HMIN) / HSTEP + 1 ; 0 = "no terrain here" (open sea beyond the skirt)
SEA_Y = 100.0                    # sea level in telemetry y
FLOOR_Y = 40.0                   # height of the filled sea floor beyond the island data
KNOWN_TYPES = ('road', 'highway', 'offroad', 'other', 'trail', 'crosscountry', 'tunnel', 'jump', 'turnaround')   # v2 editor list; anything else -> other
DEFAULT_TYPES = os.path.join(HERE, '..', '..', 'assets', 'map', 'fh6-road-types.json')


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
                added=[(int(e['a']), int(e['b']), norm(e.get('type')) if e.get('type') else 'unset') for e in (t.get('added') or [])],
                removed=set(t.get('removed') or []),
                points={int(k): v for k, v in (t.get('points') or {}).items()},
                moved={int(k): v for k, v in (t.get('moved') or {}).items()})


TN = ('unset', 'road', 'offroad', 'other', 'trail', 'crosscountry', 'tunnel', 'jump', 'highway', 'turnaround')    # the editor's type index order (0 = not set); the page gets the names


def build_roads(work, out, Hm, meta, types_path, step, lift):
    """Writes preview3d/roads.js = the road EDGE LIST (nodes [x, z, nav height], edges [a, b, type index]); the page chains / resamples / drapes it in JS
    (the same code that draws the editor's live state).  Returns (nodes, edges, node ids) for numeric_check."""
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
    tix = {n: k for k, n in enumerate(TN)}
    seen, edges = set(), []                                    # edges: (a id, b id, type index)
    miss = 0
    for ids in r['ids']:
        for a, b in zip(ids[:-1], ids[1:]):
            k = key(a, b)
            if k in seen or k in T['removed'] or a not in pos or b not in pos:
                continue
            seen.add(k)
            ty = T['types'].get(k)
            if ty is None:
                ty = 'unset'; miss += 1                        # edge not painted: own type (red in the 3D 'Type colours' style, grey like Other in 'Map look')
            edges.append((a, b, tix[ty]))
    nadd = 0
    for a, b, ty in T['added']:
        k = key(a, b)
        if k in seen or k in T['removed'] or a not in pos or b not in pos:
            continue
        seen.add(k); edges.append((a, b, tix[ty])); nadd += 1
    used = sorted({i for a, b, _ in edges for i in (a, b)})
    ix = {i: k for k, i in enumerate(used)}
    nodes = np.array([[pos[i][0], pos[i][1], ny.get(i, np.nan)] for i in used], '<f4')      # a node without a height (user point without y) falls back to the terrain in the page
    E = np.array([[ix[a], ix[b], t] for a, b, t in edges], '<u4')
    L = np.hypot(nodes[E[:, 0], 0].astype(float) - nodes[E[:, 1], 0], nodes[E[:, 0], 1].astype(float) - nodes[E[:, 1], 1])
    km = {TN[t]: round(float(L[E[:, 2] == t].sum()) / 1000, 1) for t in range(len(TN)) if (E[:, 2] == t).any()}
    log(f'roads: {len(edges)} edges ({miss} unpainted -> unset, {nadd} added links, {len(T["removed"])} removed), {len(used)} nodes; types file v{T["version"]}')
    write_js(os.path.join(out, 'preview3d', 'roads.js'), 'roads', dict(nodes=b64z(nodes), edges=b64z(E), tn=list(TN), n_nodes=int(len(used)), n_edges=int(len(E)), lift=lift, step=step, km=km))
    nofb = int(np.isnan(nodes[:, 2]).sum())
    if nofb:
        log(f'roads: {nofb} nodes have no height (the page puts them on the terrain)')
    for ty, k in sorted(km.items()):
        log(f'    {ty:13s} {int((E[:, 2] == tix[ty]).sum()):6d} edges {k:8.1f} km')
    return nodes, E, used


def numeric_check(nodes, E, used, Hm, meta, out):
    """Checks the WRITTEN roads.js (decoded again): the edge list round-trips (node count, edge count, km per type), every edge end exists, and diagnostics of the nav node
    heights against the terrain mesh (sinking / floating) and the steepest node-to-node grades (suspect heights).  The page's own P3D_DEBUG.check() verifies the
    resampled geometry (terrain drape = mesh triangle height + lift, node height = interpolation of the edge's two nodes + lift)."""
    txt = open(os.path.join(out, 'preview3d', 'roads.js')).read()
    R = json.loads(txt[txt.index(').roads=') + 8: txt.rindex(';')])
    dec = lambda b64, dt: np.frombuffer(zlib.decompress(base64.b64decode(b64)), dt)
    n2 = dec(R['nodes'], '<f4').reshape(-1, 3); e2 = dec(R['edges'], '<u4').reshape(-1, 3)
    ok = n2.shape == nodes.shape and e2.shape == E.shape and np.array_equal(e2, E) and np.array_equal(n2[:, :2], nodes[:, :2]) and np.allclose(n2[:, 2], nodes[:, 2], equal_nan=True)
    ok = ok and int(e2[:, :2].max()) < len(n2) and set(np.unique(e2[:, 2])) <= set(range(len(R['tn'])))
    log(f'numeric check: roads.js round-trip {"OK" if ok else "FAILED"} ({len(n2)} nodes, {len(e2)} edges)')
    if not ok:
        raise SystemExit('roads.js does not match what was built')
    x = nodes[:, 0].astype(float); z = nodes[:, 1].astype(float); h = nodes[:, 2].astype(float)
    have = ~np.isnan(h)
    dn = h[have] - tri_height(Hm, meta, x[have], z[have])
    log(f'node y - terrain(tri) over all nodes: median {np.median(dn):+.2f} m, p1 {np.percentile(dn, 1):+.2f}, p99 {np.percentile(dn, 99):+.2f}; '
        f'{100 * np.mean(dn < -1):.1f}% sink >1 m below the mesh, {100 * np.mean(dn < -3):.1f}% >3 m, {100 * np.mean(dn > 3):.1f}% float >3 m above')
    tun = R['tn'].index('tunnel')
    a, b = E[:, 0], E[:, 1]
    L = np.hypot(x[a] - x[b], z[a] - z[b]); both = have[a] & have[b] & (E[:, 2] != tun) & (L > 0.5)
    g = np.abs(h[a] - h[b])[both] / L[both]
    sel = np.nonzero(both)[0][np.argsort(-g)[:5]]
    log('steepest node-to-node grades (non-tunnel): ' + '; '.join(f'{100 * abs(h[a[e]] - h[b[e]]) / L[e]:.0f}% {R["tn"][E[e, 2]]} {used[a[e]]}->{used[b[e]]} ({h[a[e]]:.1f}->{h[b[e]]:.1f} m over {L[e]:.1f} m)' for e in sel))


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
    ap.add_argument('--step', type=float, default=5.0, help='road resample step in metres (written into roads.js; the page resamples)')
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
    nodes, E, used = build_roads(work, out, Hm, meta, a.road_types, a.step, a.lift)
    numeric_check(nodes, E, used, Hm, meta, out)
    write_js(os.path.join(out, 'preview3d', 'meta.js'), 'meta', dict(seasons=seasons, default='Summer' if 'Summer' in seasons else (seasons[0] if seasons else None),
                                                                      tex_size=a.tex_size, built=time.strftime('%Y-%m-%d %H:%M')))
    shutil.copyfile(os.path.join(EDITOR, 'preview-3d.html'), os.path.join(out, 'preview-3d.html'))      # the single copy of the page (also served by the app)
    shutil.copytree(os.path.join(EDITOR, 'lib'), os.path.join(out, 'lib'), dirs_exist_ok=True)      # three.min.js (embedded, no CDN); the editor's build_viewer.py copies the same folder
    log('wrote', os.path.join(out, 'preview-3d.html'))


if __name__ == '__main__':
    main()
