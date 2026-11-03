#!/usr/bin/env python3
"""Sample the TERRAIN surface id under every road of the nav graph (roads.json from decode_nav.py) -> roadsurf.npz.
READ-ONLY on the install.   Needs numpy.   Method + limits: docs/game-data/fh6-terrain.md ("Surface under the roads").

Usage: classify_roads.py [--media <...>/ForzaHorizon6/media] [--out ./fh6-out]   (--out must hold roads.json with `heights`, i.e. a current decode_nav.py run)
                         [--step 4]   sample spacing along a polyline in metres      [--jobs 8]

Every polyline is densified to --step metres (node heights interpolated).  For each sample the terrain collision triangles
(`tbheightfield/autoterrain_x{X}_z{Z}_square{0..35}.phys`, same decode as extract_terrain.parse_phys) that contain the sample in x/z are
looked up; of those the one whose height is closest to the road node height wins (so a bridge deck that carries its own collision beats the
ground under it, and ground far below/above the road is recognisable).  The result is the EXACT triangle id, not the 8 m dominant-by-area raster
(which can pick the verge - see the docs).
Output roadsurf.npz:  step, off (n_poly+1 sample offsets), x, z, y (float32 sample position + interpolated node height), id (uint16 id of the best
triangle, 65535 = no collision triangle under the sample), dy (int16 decimetres, triangle height - road height, 32767 = none), n (uint8 number of
candidate triangles).  Thresholds (elevated / minimum run) are applied later in build_viewer.py so they can be tuned without re-reading the 40 GB file.
"""
import argparse, json, os, re, sys, time
import numpy as np
from concurrent.futures import ProcessPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, resolve_media
from pgzp import Pgzp
from extract_terrain import parse_phys, cell_of

NONE_ID, NONE_DY = 65535, 32767
PAT = r'tbheightfield\\autoterrain_x(-?\d+)_z(-?\d+)_square(\d+)\.phys$'


def densify(poly, hs, step):
    P = np.asarray(poly, float); h = np.asarray(hs, float)
    d = np.r_[0, np.cumsum(np.hypot(*np.diff(P, axis=0).T))]
    t = np.unique(np.r_[np.arange(0, d[-1], step), d[-1]])
    return np.interp(t, d, P[:, 0]), np.interp(t, d, P[:, 1]), np.interp(t, d, h)


def sample_file(P, T, ids, X, Z, Y, cand):
    """-> for each sample index in `cand`: (best id, best dy in dm, n candidate triangles); barycentric point-in-triangle, chunked."""
    A, B, C = P[T[:, 0]], P[T[:, 1]], P[T[:, 2]]
    x1, z1, x2, z2, x3, z3 = A[:, 0], A[:, 2], B[:, 0], B[:, 2], C[:, 0], C[:, 2]
    det = (z2 - z3) * (x1 - x3) + (x3 - x2) * (z1 - z3)
    ok = np.abs(det) > 1e-9
    det = np.where(ok, det, 1.0)
    out = []
    for c0 in range(0, len(cand), 256):
        c = cand[c0:c0 + 256]
        px, pz = X[c][:, None], Z[c][:, None]
        l1 = ((z2 - z3) * (px - x3) + (x3 - x2) * (pz - z3)) / det
        l2 = ((z3 - z1) * (px - x3) + (x1 - x3) * (pz - z3)) / det
        l3 = 1 - l1 - l2
        e = -1e-4
        m = (l1 >= e) & (l2 >= e) & (l3 >= e) & ok
        ty = l1 * A[:, 1] + l2 * B[:, 1] + l3 * C[:, 1]
        dy = np.where(m, ty - Y[c][:, None], np.inf)
        k = np.abs(dy).argmin(1)
        r = np.arange(len(c))
        has = m[r, k]
        out.append((np.where(has, ids[k], NONE_ID), np.where(has, np.round(np.where(has, dy[r, k], 0) * 10), NONE_DY), m.sum(1)))
    return [np.concatenate(v) for v in zip(*out)]


def work(args):
    media, idxs, X, Z, Y = args
    pg = Pgzp.open(media, 0)
    bid = np.full(len(X), NONE_ID, np.uint16); bdy = np.full(len(X), NONE_DY, np.int32); bn = np.zeros(len(X), np.int32)
    for i in idxs:
        P, T, M = parse_phys(pg.entry(i))
        lo, hi = P.min(0), P.max(0)
        cand = np.nonzero((X >= lo[0] - .01) & (X <= hi[0] + .01) & (Z >= lo[2] - .01) & (Z <= hi[2] + .01))[0]
        if not len(cand):
            continue
        ids, dy, n = sample_file(P, T, M[:, 1], X, Z, Y, cand)
        better = (np.abs(dy) < np.abs(bdy[cand])) & (dy != NONE_DY)
        bid[cand[better]] = ids[better]; bdy[cand[better]] = dy[better]; bn[cand] += n
    return bid, bdy, bn


def main():
    ap = argparse.ArgumentParser(description='surface id under every nav road -> roadsurf.npz')
    add_media_args(ap)
    ap.add_argument('--step', type=float, default=4.0)
    ap.add_argument('--jobs', type=int, default=8)
    args = ap.parse_args()
    media = resolve_media(args)
    rj = json.load(open(os.path.join(args.out, 'roads.json')))
    assert 'heights' in rj, 'roads.json has no node heights - re-run decode_nav.py'
    xs, zs, ys, off = [], [], [], [0]
    for pl, hs in zip(rj['polylines'], rj['heights']):
        x, z, y = densify(pl, hs, args.step)
        xs.append(x); zs.append(z); ys.append(y); off.append(off[-1] + len(x))
    X, Z, Y = [np.concatenate(v).astype(np.float64) for v in (xs, zs, ys)]
    print(f'classify_roads: {len(rj["polylines"])} polylines, {len(X)} samples @ {args.step} m')
    pg = Pgzp.open(media, 0)
    # 512 m cells that hold samples; a sample on a cell edge is found via the squares' bbox test (squares overlap by their own bbox only)
    cx = np.floor(X / 512).astype(int) * 512; cz = np.floor(Z / 512).astype(int) * 512
    want = set()
    for a, b in set(zip(cx.tolist(), cz.tolist())):
        want |= {(a + i, b + j) for i in (-512, 0, 512) for j in (-512, 0, 512)}
    sel = []
    rx = re.compile(PAT)
    for i in pg.find(PAT):
        m = rx.search(pg.names[i])
        if cell_of(int(m.group(1)), int(m.group(2))) in want:
            sel.append(i)
    pg.close()
    print(f'  {len(sel)} .phys files in {len(want)} cells', flush=True)
    t0 = time.time()
    chunks = [sel[k::args.jobs] for k in range(args.jobs)]       # every worker sees all samples and keeps its own best
    with ProcessPoolExecutor(args.jobs) as ex:
        res = list(ex.map(work, [(media, c, X, Z, Y) for c in chunks]))
    bid = np.full(len(X), NONE_ID, np.uint16); bdy = np.full(len(X), NONE_DY, np.int32); bn = np.zeros(len(X), np.int32)
    for i_, d_, n_ in res:
        better = (np.abs(d_) < np.abs(bdy)) & (d_ != NONE_DY)
        bid[better] = i_[better]; bdy[better] = d_[better]; bn += n_
    print(f'  sampled in {time.time() - t0:.0f} s; no triangle: {np.mean(bid == NONE_ID) * 100:.1f}% of samples')
    np.savez_compressed(os.path.join(args.out, 'roadsurf.npz'), step=args.step, off=np.array(off), x=X.astype(np.float32), z=Z.astype(np.float32),
                        y=Y.astype(np.float32), id=bid, dy=np.clip(bdy, -32768, 32767).astype(np.int16), n=np.minimum(bn, 255).astype(np.uint8))


if __name__ == '__main__':
    main()
