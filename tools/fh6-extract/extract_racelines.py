#!/usr/bin/env python3
"""FH6 race driving lines + track width from OpenWorld/Brio/AITracks/Route<N>.owt -> racelines.json.  READ-ONLY on the install.

Layout of the .owt (header, node record, what is still undecoded): fh6owt.py docstring and docs/game-data/fh6-game-files.md ("Race lines").
Per record: the line is TRIMMED to one drive,
  point-to-point : nodes[start .. finish], finish = node nearest to the RVAN finish_line at/after the start node (127 routes; all end < 3 m off)
  circuit        : one lap, rotated to begin at the start node: first local-min node after start within 2.6 m of node 0 closes it
                   (43 close < 3 m; regular circuits: the last node; 6 files with a lead-out tail are cut at that node)
then decimated to every --step metres (default 5; 0 = keep every node).
  left/right     = p +/- A (A = the node's half-width vector, length 2.5-12.5 m): the track edges.
Routes 102/103 lie off the playable map, route 99 is a test route (start at 0,0) - they are kept and flagged in `note`.
Output is Playground Games' data: write it outside the repo.

Usage: extract_racelines.py [--media <...>/ForzaHorizon6/media] [--out DIR] [--step 5]
"""
import argparse, glob, json, math, os, re, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, ci, resolve_media
from fh6owt import read_owt, start_node, parse_rvan


def trim(p, h, F, circuit):
    """-> (index array (may wrap), start_idx, finish_idx)"""
    n = len(p); st = start_node(h)
    pp = np.where(np.isfinite(p).all(1)[:, None], p, 1e9)
    if not circuit:
        dF = np.linalg.norm(pp - F, axis=1); dF[:st] = 1e9
        fin = int(dF.argmin())
        return np.arange(st, fin + 1), st, fin
    d0 = np.linalg.norm(pp - pp[0], axis=1)
    j = n - 1
    for i in range(max(st, 1), n):
        if d0[i] <= 2.6 and d0[i] <= d0[i - 1] and (i == n - 1 or d0[i] <= d0[i + 1]):
            j = i; break
    return np.concatenate([np.arange(st, j + 1), np.arange(0, st + 1)]), st, j


def decimate(xz, step):
    keep = [0]; acc = 0.0
    for i in range(1, len(xz)):
        acc += math.hypot(*(xz[i] - xz[i - 1]))
        if acc >= step:
            keep.append(i); acc = 0.0
    if keep[-1] != len(xz) - 1: keep.append(len(xz) - 1)
    return keep


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0]); add_media_args(ap)
    ap.add_argument('--step', type=float, default=5.0, help='decimation step in metres (default 5; 0 = every node)')
    args = ap.parse_args(); media = resolve_media(args)
    ait = ci(media, 'OpenWorld/Brio/AITracks')
    out = []
    for f in sorted(glob.glob(os.path.join(ait, 'Route*.owt')), key=lambda s: int(re.findall(r'\d+', os.path.basename(s))[0])):
        rid = int(re.findall(r'\d+', os.path.basename(f))[0])
        a, h = read_owt(f)
        rv = parse_rvan(os.path.join(ait, f'Route{rid}.nav'))
        S, F, pts = rv['A'], rv['B'], rv['pts']
        circuit = math.dist(S, F) < 1.0
        p = a['p'].astype(float); A = a['a'].astype(float)
        idx, st, fin = trim(p, h, np.array(F), circuit)
        idx = idx[np.isfinite(p[idx]).all(1) & np.isfinite(A[idx]).all(1)]
        P = p[idx]; L = A[idx]
        length = float(np.linalg.norm(np.diff(P, axis=0), axis=1).sum())
        k = decimate(P[:, [0, 2]], args.step) if args.step > 0 else list(range(len(P)))
        P = P[k]; L = L[k]
        hw = np.linalg.norm(L[:, [0, 2]], axis=1)
        sl = pts.get('start_location_000')
        t = P[1] - P[0]; hd_line = math.degrees(math.atan2(t[0], t[2]))                      # 0 = +z (north), 90 = +x
        hd_grid = math.degrees(math.atan2(sl[3], sl[5])) if sl else None
        rec = dict(route_id=rid, circuit=bool(circuit), length_m=round(length, 1), n_nodes=int(len(a)), n_sections=int(h[8]),
                   start_node=int(st), finish_node=int(fin), lead_in_nodes=int(st) if not circuit else 0,
                   start=[round(S[0], 2), round(S[1], 2), round(S[2], 2)], finish=[round(F[0], 2), round(F[1], 2), round(F[2], 2)],
                   start_heading_deg=round(hd_grid, 1) if hd_grid is not None else None, line_heading_deg=round(hd_line, 1),
                   closed=bool(circuit and math.dist(P[0], P[-1]) < 3.0),
                   half_width_range=[round(float(hw.min()), 1), round(float(hw.max()), 1)],
                   points=np.round(P[:, [0, 2]], 1).tolist(), y=np.round(P[:, 1], 1).tolist(), half_width=np.round(hw, 1).tolist(),
                   left=np.round(P[:, [0, 2]] + L[:, [0, 2]], 1).tolist(), right=np.round(P[:, [0, 2]] - L[:, [0, 2]], 1).tolist())
        if rid == 99: rec['note'] = 'test route (RVAN start at 0,0)'
        if rid in (102, 103): rec['note'] = 'off the playable map'
        out.append(rec)
    with open(os.path.join(args.out, 'racelines.json'), 'w') as fh:
        json.dump(out, fh, separators=(',', ':'))
    nc = sum(r['circuit'] for r in out)
    print(f'{len(out)} routes ({len(out) - nc} point-to-point, {nc} circuits), {sum(len(r["points"]) for r in out)} points -> {args.out}/racelines.json')
    print('closed circuits:', sum(r['closed'] for r in out), '/', nc, '; multi-section files:', [r['route_id'] for r in out if r['n_sections'] != 1])


if __name__ == '__main__':
    main()
