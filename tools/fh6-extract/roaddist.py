#!/usr/bin/env python3
"""Validation: distance from each POI type to the nearest decoded road (roads.json + pois.json from the other scripts).

Usage: roaddist.py [--out ./fh6-out]     Needs: numpy (scipy optional, speeds it up).
Exact-position types (race_start, house, fast_travel...) should sit on/near roads (race_start median ~2.8 m);
'precision: cell' types are only cell-centre approximations and will not.
"""
import argparse, collections, json
import numpy as np
try:
    from scipy.spatial import cKDTree as KD
except Exception:
    KD = None

ap = argparse.ArgumentParser()
ap.add_argument('--out', default='./fh6-out')
OUT = ap.parse_args().out
R = json.load(open(OUT + '/roads.json'))['polylines']
pts = []
for q in R:
    q = np.array(q)
    for a, b in zip(q[:-1], q[1:]):
        n = max(1, int(np.hypot(*(b - a)) // 10))
        pts.append(a + (b - a) * np.linspace(0, 1, n, endpoint=False)[:, None])
pts = np.vstack(pts); print('road samples', len(pts), 'scipy', KD is not None)
P = json.load(open(OUT + '/pois.json'))
if KD:
    t = KD(pts); d, _ = t.query(np.array([[p['x'], p['z']] for p in P]))
else:
    d = np.array([np.min(np.hypot(pts[:, 0] - p['x'], pts[:, 1] - p['z'])) for p in P])
by = collections.defaultdict(list)
for p, dd in zip(P, d): by[p['type']].append(dd)
for k, v in sorted(by.items()):
    v = np.array(v); print(f'{k:22s} n={len(v):5d} median={np.median(v):7.1f} p90={np.percentile(v, 90):7.1f} <15m={np.mean(v < 15) * 100:4.0f}% <40m={np.mean(v < 40) * 100:4.0f}%')
