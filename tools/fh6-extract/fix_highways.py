#!/usr/bin/env python3
"""Infer two things the road-type editor makes tedious to mark by hand, on a `fh6-road-types` v2 file (the viewer editor's "Export"):

  1. highway OVER-MARKS   - a normal road that got typed `highway` (the editor's brush paints every edge under it and its fill tool takes the *nearest* run,
                            so roads under / beside a highway were painted too)                                   ->  `road`
  2. highway TURNAROUNDS  - the short crossover strips (one nav edge, ~15-25 m) that join the two carriageways of a highway  ->  `turnaround`

READ-ONLY on the game install; works from the extractor outputs only (roads.json + terr_e/elevation.npy from decode_nav.py / extract_terrain.py).
Needs numpy (matplotlib only for --plot).  Every threshold below says what it was calibrated on; the method and its limits are in the comments of each rule
(and in the README entry of this script).

Usage:  fix_highways.py IN.json OUT.json [--work DIR] [--report PATH] [--dry-run] [--validate] [--no-overmarks] [--no-turnarounds] [--plot PNG [--bbox X0,Z0,X1,Z1]]
  IN / OUT      fh6-road-types files (v2; v1 is accepted and written as v2).  OUT may equal IN.  OUT is written in the editor's own key order, one entry
                per line (diff-friendly, like data/fh6-road-types.json); `counts` is recomputed the way the editor's exportObj() does.  Only `types`
                values and `counts` can differ from IN: points / moved / removed / added / jump_from / races are passed through untouched.
  --work DIR    extractor outputs (default ./fh6-out-work): roads.json (needs `ids` + `heights`), terr_e/elevation.npy (optional: without it the
                ground-level test of the island rule is skipped, so that rule only reports), regions.json (optional: region name column)
  --report P    review list: P.csv + P.json - one row per changed or needs-review edge (default <OUT without .json>.review.{csv,json})
  --dry-run     do everything but write OUT (the report is still written)
  --validate    also run the turnaround rule on a copy of IN whose existing turnarounds are reset to `road` and print how many it re-finds
  --plot PNG    plot of the result: red = highway->road, magenta = new turnaround, orange = needs review, thin red = other highway (needs matplotlib).
                Window = --bbox, else the box around the highway->road changes / reviews.
The script is idempotent: run on its own output it changes nothing.
"""
import argparse, collections, csv, json, math, os, sys

import numpy as np

TYPES = ['road', 'offroad', 'other', 'trail', 'crosscountry', 'tunnel', 'jump', 'highway', 'turnaround']      # editor TN[1..9]

# ------------------------------------------------------------------------------------------------------------------ thresholds
# turnaround.  The user's 12 hand-marked examples: one nav edge each, 16.5-24.1 m, both ends degree-3 nodes in the middle of a highway carriageway,
# end heights equal (<= 0.1 m), strip ~perpendicular to the carriageways.  Unmarked look-alikes: 11.6-24.6 m (nothing between 25 and 36 m).
TURN_MAX_LEN = {1: 30.0, 2: 40.0}   # m, whole strip, by number of nav edges: 1 edge (all 12 examples) or 2 edges ("one intermediate node", the user's phrase;
                                    # the only 2-edge strips between highway nodes are 36-39 m, i.e. 2 x the usual 18-20 m)
TURN_MAX_EDGES = 2
TURN_MAX_DZ = 4.0          # m height difference between the two ends (examples <= 0.1; accepted look-alikes <= 3.1)
TURN_THROUGH_DEG = 130.0   # the two highway edges at each end must be ~collinear (a carriageway passing through): angle between them >= this
TURN_CROSS_MIN = 25.0      # the strip must cross the carriageway: angle between strip and carriageway axis within [25, 155] (rejects slip-road merges)
TURN_SHORT_MAX = 30.0      # TURN_SHORT_LINK: a 1-edge nav polyline up to this long whose both ends touch a highway/tunnel edge is a turnaround, no further tests
TURN_LOOP_M = 150.0       # the ends must not be joined by <= this much highway path (a tiny loop, not two carriageways)

# over-marks
OVER_SHARE_MAX = 0.30      # a nav polyline (= one road record) with <= this share of highway edges (tunnel edges not counted) is a road with brush spill
ISLAND_MAX_M = 1500.0      # an island of highway edges longer than this is never touched
GROUND_DZ = 3.0            # m above the terrain raster = "on the ground"
GROUND_MIN_FRAC = 0.8      # share of an island's nodes (that have terrain data) that must be on the ground


# ------------------------------------------------------------------------------------------------------------------ io
def ek(a, b):
    return f'{min(a, b)}-{max(a, b)}'


def load_nav(work):
    f = os.path.join(work, 'roads.json')
    if not os.path.isfile(f):
        sys.exit(f'missing {f} - run decode_nav.py (or build_viewer.py) first, or pass --work DIR')
    r = json.load(open(f))
    if not r.get('ids') or not r.get('heights'):
        sys.exit('roads.json has no node ids / heights - re-run decode_nav.py')
    pos, edges, polys = {}, {}, []
    for pi, (pl, hs, ids) in enumerate(zip(r['polylines'], r['heights'], r['ids'])):
        for p, h, i in zip(pl, hs, ids):
            pos[i] = (p[0], p[1], h)
        polys.append(list(ids))
        for k in range(len(ids) - 1):
            edges[ek(ids[k], ids[k + 1])] = (min(ids[k], ids[k + 1]), max(ids[k], ids[k + 1]), pi)
    return r, pos, edges, polys


class Terrain:
    """8 m elevation raster (extract_terrain.py): bilinear sample, NaN = no data (also when there is no raster)."""

    def __init__(self, work):
        f = os.path.join(work, 'terr_e', 'elevation.npy')
        self.E = np.load(f) if os.path.isfile(f) else None
        if self.E is not None:
            m = json.load(open(os.path.join(work, 'terr_e', 'elevation.json')))
            self.x0, self.z1, self.res = m['x0'], m['z1'], m['res_m']

    def at(self, x, z):
        if self.E is None:
            return float('nan')
        c, r = (x - self.x0) / self.res - 0.5, (self.z1 - z) / self.res - 0.5
        c0, r0 = int(math.floor(c)), int(math.floor(r))
        if c0 < 0 or r0 < 0 or c0 + 1 >= self.E.shape[1] or r0 + 1 >= self.E.shape[0]:
            return float('nan')
        fc, fr = c - c0, r - r0
        v = self.E[r0:r0 + 2, c0:c0 + 2]
        return float((v[0, 0] * (1 - fc) + v[0, 1] * fc) * (1 - fr) + (v[1, 0] * (1 - fc) + v[1, 1] * fc) * fr)


# ------------------------------------------------------------------------------------------------------------------ graph
class Net:
    """Nav graph + the user's added links / points, with the current type of every edge (game edges: ty; added links: their own type)."""

    def __init__(self, obj, pos, edges, polys, terrain):
        self.pos, self.edges, self.polys, self.terr = dict(pos), edges, polys, terrain
        for k, v in obj.get('points', {}).items():
            self.pos[int(k)] = (v[0], v[1], v[2])
        for k, v in obj.get('moved', {}).items():
            self.pos[int(k)] = (v[0], v[1], v[2])
        self.removed = set(obj.get('removed', []))
        self.ty = dict(obj['types'])                                  # game edge key -> type (absent = not set)
        self.added = {}                                               # added link key -> type or None
        for q in obj.get('added', []):
            self.added[ek(q['a'], q['b'])] = q.get('type')
        self.adj = collections.defaultdict(set)
        for k, (a, b, _) in edges.items():
            if k not in self.removed:
                self.adj[a].add(b); self.adj[b].add(a)
        for k in self.added:
            a, b = map(int, k.split('-'))
            if a in self.pos and b in self.pos:
                self.adj[a].add(b); self.adj[b].add(a)
        self.nd_cache = {}

    def type_of(self, a, b):
        k = ek(a, b)
        return self.added[k] if k in self.added else self.ty.get(k)

    def length(self, a, b):
        p, q = self.pos[a], self.pos[b]
        return math.hypot(p[0] - q[0], p[1] - q[1])

    def hw_nbrs(self, n, excl=()):
        return [m for m in self.adj[n] if m not in excl and self.type_of(n, m) == 'highway']

    def dz(self, n):
        """height above the terrain raster (NaN where it has no data)"""
        if n not in self.nd_cache:
            p = self.pos[n]
            self.nd_cache[n] = p[2] - self.terr.at(p[0], p[1])
        return self.nd_cache[n]

    def apply(self, changes):
        for k, v in changes.items():
            self.ty[k] = v[1]


def angle(u, v):
    nu, nv = math.hypot(*u), math.hypot(*v)
    if nu == 0 or nv == 0:
        return 0.0
    c = max(-1.0, min(1.0, (u[0] * v[0] + u[1] * v[1]) / (nu * nv)))
    return math.degrees(math.acos(c))


# ------------------------------------------------------------------------------------------------------------------ rule: turnarounds
# Why this shape: the game's nav stores each crossover as its OWN tiny road record (a 2-node polyline) hung between two nodes in the middle of the two
# carriageways, so "short strip between two highway nodes" is a clean topological test - no geometry guessing about the median needed.
def hw_reach(net, src, dst, limit, skip):
    """is dst within `limit` metres of highway-typed path from src (not using the edges in `skip`)?"""
    best = {src: 0.0}
    st = [src]
    while st:
        n = st.pop()
        for m in net.hw_nbrs(n):
            if ek(n, m) in skip:
                continue
            d = best[n] + net.length(n, m)
            if d <= limit and d < best.get(m, 1e18):
                best[m] = d
                st.append(m)
    return dst in best


def end_ok(net, n, strip_nodes, strip_dir):
    """node n (an end of the strip) must be a through-point of a highway carriageway: exactly 2 other edges, both highway, ~collinear, crossed by the strip.
    Returns (ok, reason)."""
    others = [m for m in net.adj[n] if m not in strip_nodes]
    if len(others) != 2:
        return False, f'end degree {len(others) + 1}'
    bad = sorted({str(net.type_of(n, m)) for m in others if net.type_of(n, m) != 'highway'})
    if bad:
        return False, 'end on ' + '/'.join(bad) + ' (not highway)'
    p = net.pos[n]
    u = [(net.pos[m][0] - p[0], net.pos[m][1] - p[1]) for m in others]
    if angle(u[0], u[1]) < TURN_THROUGH_DEG:
        return False, 'end not a straight carriageway'
    axis = (u[0][0] - u[1][0], u[0][1] - u[1][1])
    a = angle(axis, strip_dir)
    if not TURN_CROSS_MIN <= a <= 180 - TURN_CROSS_MIN:
        return False, 'strip not crossing the carriageway'
    return True, ''


def find_turnarounds(net):
    """-> (changes {edge key: (old, 'turnaround', code, text)}, review [(key, old, code, text)])"""
    changes, review = {}, []
    for pi, ids in enumerate(net.polys):
        ne = len(ids) - 1
        if ne < 1 or ne > TURN_MAX_EDGES or ids[0] == ids[-1]:
            continue
        ks = [ek(ids[i], ids[i + 1]) for i in range(ne)]
        if any(k in net.removed for k in ks):
            continue
        olds = [net.ty.get(k) for k in ks]
        if all(o == 'turnaround' for o in olds) or any(o not in (None, 'road', 'highway', 'turnaround') for o in olds):
            continue                                                   # done already / offroad, tunnel, other, jump ...: the user chose that on purpose
        a, b = ids[0], ids[-1]
        # pre-filter: both ends touch a highway at all (otherwise it is just a short road: not a candidate, not worth a review row)
        if not (net.hw_nbrs(a, ids[1:2]) and net.hw_nbrs(b, ids[-2:-1])):
            continue
        reasons = []
        L = sum(net.length(ids[i], ids[i + 1]) for i in range(ne))
        if L > TURN_MAX_LEN[ne]:
            reasons.append(f'strip {L:.0f} m long')
        if any(len(net.adj[n]) != 2 for n in ids[1:-1]):
            reasons.append('branching intermediate node')
        pa, pb = net.pos[a], net.pos[b]
        if abs(pa[2] - pb[2]) > TURN_MAX_DZ:
            reasons.append(f'ends {abs(pa[2] - pb[2]):.1f} m apart in height')
        sn = set(ids)
        sdir = (pb[0] - pa[0], pb[1] - pa[1])
        for n, lab in ((a, 'A'), (b, 'B')):
            ok, why = end_ok(net, n, sn - {n}, sdir)
            if not ok:
                reasons.append(f'{lab}: {why}')
        if not reasons and hw_reach(net, a, b, TURN_LOOP_M, set(ks)):
            reasons.append('ends joined by a short highway path')
        if not reasons:
            for k, o in zip(ks, olds):
                if o != 'turnaround':
                    changes[k] = (o, 'turnaround', 'TURN_CROSSOVER', 'crossover strip between two highway carriageways')
        else:
            for k, o in zip(ks, olds):
                review.append((k, o, 'REVIEW_TURNAROUND', 'turnaround? ' + '; '.join(reasons)))
    return changes, review


def find_short_links(net):
    """The user's simple rule (TURN_SHORT_LINK), at EDGE level: an edge <= TURN_SHORT_MAX m whose both end nodes also touch some other edge typed highway or
    tunnel is a turnaround when EITHER it is a nav polyline of its own (2 nodes; old type road / unset / highway / other) OR (inside a longer polyline;
    old type road / unset / other only) every other edge at both ends is highway / tunnel / turnaround, so a ramp or street that merely touches the
    highway is left alone.  (Short highway edges inside a carriageway polyline are the carriageway itself, hence excluded.)
    Why: the game has such point-to-point links from carriageway to carriageway everywhere; the strict crossover rule above (collinear carriageways, crossing
    angle, height, no short loop, no tunnel ends) rejected ~22 of them and the user wanted every one marked.  -> {edge key: (old, 'turnaround', code, text)}"""
    HT = ('highway', 'tunnel')
    one = {ek(ids[0], ids[1]) for ids in net.polys if len(ids) == 2 and ids[0] != ids[1]}
    changes = {}
    for k, (a, b, _) in net.edges.items():
        o = net.ty.get(k)
        if k in net.removed or o == 'turnaround' or net.length(a, b) > TURN_SHORT_MAX:
            continue
        if o not in ((None, 'road', 'highway', 'other') if k in one else (None, 'road', 'other')):
            continue
        ends = ((a, b), (b, a))
        if not all(any(m != o2 and net.type_of(n, m) in HT for m in net.adj[n]) for n, o2 in ends):
            continue
        if k not in one and not all(net.type_of(n, m) in HT + ('turnaround',) for n, o2 in ends for m in net.adj[n] if m != o2):
            continue
        changes[k] = (o, 'turnaround', 'TURN_SHORT_LINK', 'short link between two highway/tunnel edges')
    return changes


# ------------------------------------------------------------------------------------------------------------------ rule: over-marks
# Why NOT "stacked in 2D with a height difference": the city has real double-deck highways (nav roads #800/#801: two full carriageway pairs 9 m apart, the lower
# one on the terrain, joined by their own crossovers) and 10-layer interchanges, so ~750 highway edges have a stacked highway partner and almost all are
# genuine.  What the user's brush/fill mistakes leave behind is a different, topological signature: a road that is only PARTLY or only LOCALLY highway.
def polyline_of(net):
    pm = {}
    for pi, ids in enumerate(net.polys):
        for i in range(len(ids) - 1):
            pm[ek(ids[i], ids[i + 1])] = pi
    return pm


def network_components(net):
    """connected components of the edges typed highway / tunnel / turnaround (game edges + added links), through shared nodes.
    A real highway is one big connected piece (tunnels, crossovers and the user's added links keep it together); a street painted by mistake is an island."""
    nadj = collections.defaultdict(list)
    keys = [k for k, t in net.ty.items() if t in ('highway', 'tunnel', 'turnaround') and k in net.edges and k not in net.removed]
    keys += [k for k, t in net.added.items() if t in ('highway', 'tunnel', 'turnaround')]
    for k in keys:
        a, b = map(int, k.split('-'))
        nadj[a].append((b, k)); nadj[b].append((a, k))
    seen, comps = set(), []
    for n in nadj:
        if n in seen:
            continue
        seen.add(n); st = [n]; nodes = [n]; ks = set()
        while st:
            x = st.pop()
            for m, k in nadj[x]:
                ks.add(k)
                if m not in seen:
                    seen.add(m); st.append(m); nodes.append(m)
        comps.append((nodes, ks))
    return comps


def on_ground(net, nodes):
    """(fraction of the nodes WITH terrain data that are within GROUND_DZ of it, number of such nodes)"""
    d = [net.dz(n) for n in nodes]
    d = [v for v in d if not math.isnan(v)]
    if not d:
        return 0.0, 0
    return sum(1 for v in d if v < GROUND_DZ) / len(d), len(d)


def find_overmarks(net):
    """-> (changes {edge key: (old, 'road', code, text)}, review [(key, old, code, text)])"""
    changes, review = {}, []
    pm = polyline_of(net)
    # per nav polyline: its highway edges / its non-highway non-tunnel edges (tunnel edges are neither for nor against: a highway road can end in a tunnel mouth)
    stat = {}
    for pi, ids in enumerate(net.polys):
        ks = [k for k in (ek(ids[i], ids[i + 1]) for i in range(len(ids) - 1)) if k not in net.removed]
        stat[pi] = ([k for k in ks if net.ty.get(k) == 'highway'], [k for k in ks if net.ty.get(k) not in ('highway', 'tunnel')])
    # (1) brush spill: a nav road that is mostly NOT highway but has a few highway edges (a highway stroke ended on / crossed it).
    #     Why per nav road: it is the game's own unit of "one road"; the user paints whole carriageways, so a road whose highway share is tiny was hit by accident
    #     (outside the city every highway road is 100 % highway - this rule never fires there).
    for pi, (hw, rest) in stat.items():
        if not hw or not rest:
            continue
        share = len(hw) / (len(hw) + len(rest))
        if share <= OVER_SHARE_MAX:
            for k in hw:
                changes[k] = ('highway', 'road', 'OVR_SPILL', f'brush spill: only {len(hw)} of {len(hw) + len(rest)} edges of nav road #{pi} are highway')
        elif share < 0.7:
            for k in hw:
                review.append((k, 'highway', 'REVIEW_HALF_MARKED', f'half-marked nav road #{pi}: {len(hw)} highway / {len(rest)} other edges - which half is right?'))
        else:                                                         # mostly highway with a hole: an UNDER-mark (the crossovers of a carriageway hang off the hole), not touched
            for k in rest:
                if net.ty.get(k) in (None, 'road', 'other'):
                    review.append((k, net.ty.get(k), 'REVIEW_HIGHWAY_GAP', f'gap in highway nav road #{pi} ({len(hw)} of {len(hw) + len(rest)} edges are highway) - highway under-mark?'))
    # (2) islands: highway pieces joined to no other highway / tunnel / turnaround / added link, lying on the terrain = a street that got painted over.
    for nodes, ks in network_components(net):
        hk = [k for k in ks if net.ty.get(k) == 'highway']
        if not hk or len(hk) != len(ks):
            continue                                                  # touches a tunnel / crossover / added link: part of a real network
        L = sum(net.length(*map(int, k.split('-'))) for k in hk)
        if L > ISLAND_MAX_M:
            continue
        pls = {pm[k] for k in hk}
        other_hw = sum(1 for p in pls for k in stat[p][0] if k not in ks)
        frac, nd = on_ground(net, nodes)
        if other_hw:                                                  # a stray piece of a long highway road, cut off by a gap
            for k in hk:
                if k not in changes:
                    review.append((k, 'highway', 'REVIEW_CUT_OFF_PIECE', f'isolated piece ({L:.0f} m) of a longer highway nav road - cut off by non-highway edges'))
        elif nd >= 3 and frac >= GROUND_MIN_FRAC:
            for k in hk:
                changes.setdefault(k, ('highway', 'road', 'OVR_ISLAND', f'isolated highway piece ({L:.0f} m) on the ground, joined to no other highway/tunnel/turnaround'))
        else:
            for k in hk:
                if k not in changes:
                    review.append((k, 'highway', 'REVIEW_ISLAND', f'isolated highway piece ({L:.0f} m), joined to no other highway/tunnel/turnaround ({"elevated" if nd else "no terrain data"})'))
    return changes, [r for r in review if r[0] not in changes]


# ------------------------------------------------------------------------------------------------------------------ counts (the editor's exportObj)
def jsnum(v):
    """JS-style number: 207 not 207.0"""
    return int(v) if float(v).is_integer() else v


def recount(obj, net, types):
    km = [0.0] * 10                                                   # [not_set, road, ...]
    for k, (a, b, _) in net.edges.items():
        if k in net.removed:
            continue
        t = types.get(k)
        km[TYPES.index(t) + 1 if t else 0] += net.length(a, b) / 1000
    for k, t in net.added.items():
        a, b = map(int, k.split('-'))
        if a in net.pos and b in net.pos:
            km[TYPES.index(t) + 1 if t else 0] += net.length(a, b) / 1000
    r1 = lambda v: jsnum(math.floor(v * 10 + 0.5) / 10)
    kmo = {t: r1(km[i + 1]) for i, t in enumerate(TYPES)}
    kmo['not_set'] = r1(km[0])
    old = obj.get('counts', {})
    return {'km': kmo,
            'edges': {'total': len(net.edges), 'painted': len(types), 'added': len(obj.get('added', [])), 'removed': len(obj.get('removed', []))},
            'points': len(obj.get('points', {})), 'moved': len(obj.get('moved', {})),
            'races': {'marked': len(obj.get('races', {})), 'total': old.get('races', {}).get('total', 0)}}


# ------------------------------------------------------------------------------------------------------------------ output
def dumps(o):
    return json.dumps(o, separators=(',', ':'), ensure_ascii=False)


def write_v2(path, obj, types, counts):
    """the editor's key order, one entry per line (same layout as data/fh6-road-types.json)"""
    L = ['{"format":"fh6-road-types","version":2,', '"nav":' + dumps(obj['nav']) + ',']

    def block(name, items, last=False):
        L.append(f'"{name}":' + '{')
        L.extend(dumps(k) + ':' + dumps(v) + (',' if i < len(items) - 1 else '') for i, (k, v) in enumerate(items))
        L.append('},')
    block('types', list(types.items()))
    L.append('"added":[')
    L.extend(dumps(q) + (',' if i < len(obj.get('added', [])) - 1 else '') for i, q in enumerate(obj.get('added', [])))
    L.append('],')
    block('points', list(obj.get('points', {}).items()))
    block('moved', list(obj.get('moved', {}).items()))
    L.append('"removed":' + dumps(obj.get('removed', [])) + ',')
    block('jump_from', list(obj.get('jump_from', {}).items()))
    L.append('"races":{')
    rs = list(obj.get('races', {}).items())
    L.extend(dumps(k) + ':' + dumps(v) + (',' if i < len(rs) - 1 else '') for i, (k, v) in enumerate(rs))
    L.append('},')
    L.append('"counts":' + dumps(counts) + '}')
    with open(path, 'w', encoding='utf-8') as f:
        f.write('\n'.join(L) + '\n')


def load_regions(work):
    f = os.path.join(work, 'regions.json')
    if not os.path.isfile(f):
        return []
    return [(r['names'].get('EN', r['id']), r['outline']) for r in json.load(open(f))]


def region_of(regions, x, z):
    for name, poly in regions:
        inside, j = False, len(poly) - 1
        for i in range(len(poly)):
            xi, zi, xj, zj = poly[i][0], poly[i][1], poly[j][0], poly[j][1]
            if (zi > z) != (zj > z) and x < (xj - xi) * (z - zi) / (zj - zi) + xi:
                inside = not inside
            j = i
        if inside:
            return name
    return ''


def make_rows(net, changes, review, regions, polys_of):
    rows = []
    for kind, items in (('change', [(k, v[0], v[1], v[2], v[3]) for k, v in changes.items()]),
                        ('review', [(k, o, '', c, w) for k, o, c, w in review])):
        for k, old, new, code, why in items:
            a, b = map(int, k.split('-'))
            pa, pb = net.pos[a], net.pos[b]
            x, z, y = (pa[0] + pb[0]) / 2, (pa[1] + pb[1]) / 2, (pa[2] + pb[2]) / 2
            rows.append(dict(kind=kind, edge=k, old=old or 'not_set', new=new, code=code, reason=why, x=round(x, 1), z=round(z, 1), y=round(y, 1),
                             length_m=round(net.length(a, b), 1), nav_road=polys_of.get(k, ''), region=region_of(regions, x, z)))
    rows.sort(key=lambda r: (r['kind'], r['code'], r['nav_road'] if r['nav_road'] != '' else -1, r['x']))
    return rows


def write_report(base, rows, summary):
    with open(base + '.csv', 'w', newline='', encoding='utf-8') as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0].keys()) if rows else ['kind'])
        w.writeheader(); w.writerows(rows)
    with open(base + '.json', 'w', encoding='utf-8') as f:
        json.dump({'summary': summary, 'rows': rows}, f, indent=1, ensure_ascii=False)


def plot(path, net, rows, bbox):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    from matplotlib.collections import LineCollection
    sel = [r for r in rows if r['code'] != 'TURN_CROSSOVER'] or rows
    if bbox is None:
        xs, zs = [r['x'] for r in sel], [r['z'] for r in sel]
        bbox = (min(xs) - 400, min(zs) - 400, max(xs) + 400, max(zs) + 400) if xs else (-9000, -9500, 7000, 9500)
    x0, z0, x1, z1 = bbox
    seg = collections.defaultdict(list)
    for k, (a, b, _) in net.edges.items():
        pa, pb = net.pos[a], net.pos[b]
        if max(pa[0], pb[0]) < x0 or min(pa[0], pb[0]) > x1 or max(pa[1], pb[1]) < z0 or min(pa[1], pb[1]) > z1:
            continue
        seg['hw' if net.ty.get(k) == 'highway' else ('tn' if net.ty.get(k) == 'turnaround' else 'o')].append([pa[:2], pb[:2]])
    fig, ax = plt.subplots(figsize=(16, max(6, 16 * (z1 - z0) / (x1 - x0))), dpi=90)
    ax.add_collection(LineCollection(seg['o'], colors='#c8c8c8', linewidths=0.5))
    ax.add_collection(LineCollection(seg['hw'], colors='#a33', linewidths=1.0, alpha=0.55))
    col = {('change', 'road'): '#e00000', ('change', 'turnaround'): '#e000e0', ('review', ''): '#ff8c00'}
    lw = {('change', 'road'): 3.5, ('change', 'turnaround'): 2.5, ('review', ''): 2.5}
    drawn = collections.defaultdict(list)
    for r in rows:
        a, b = map(int, r['edge'].split('-'))
        drawn[(r['kind'], r['new'])].append([net.pos[a][:2], net.pos[b][:2]])
    for key in (('review', ''), ('change', 'turnaround'), ('change', 'road')):
        if drawn.get(key):
            ax.add_collection(LineCollection(drawn[key], colors=col[key], linewidths=lw[key], zorder=5))
    ax.set_xlim(x0, x1); ax.set_ylim(z0, z1); ax.set_aspect('equal')
    ax.set_title('red = highway -> road   magenta = new turnaround   orange = needs review   (thin dark red = other highway)')
    fig.savefig(path, bbox_inches='tight')


# ------------------------------------------------------------------------------------------------------------------ main
def run_rules(net, over=True, turn=True):
    changes, review = {}, []
    if over:
        c, r = find_overmarks(net)
        changes.update(c); review += r
        net.apply(c)                                                   # the turnaround rule sees the corrected highway set
    if turn:
        c, r = find_turnarounds(net)
        changes.update(c); review += r
        net.apply(c)
        c = find_short_links(net)
        changes.update(c)
        net.apply(c)
        review = [x for x in review if x[0] not in changes]            # a review row for an edge the short-link rule now converted is moot
    return changes, review


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0], formatter_class=argparse.RawDescriptionHelpFormatter, epilog='\n'.join(__doc__.split('\n')[2:]))
    ap.add_argument('infile'); ap.add_argument('outfile')
    ap.add_argument('--work', default='./fh6-out-work', help='extractor outputs: roads.json, terr_e/elevation.npy, regions.json (default ./fh6-out-work)')
    ap.add_argument('--report', help='review list base path (writes .csv + .json); default <outfile minus .json>.review')
    ap.add_argument('--dry-run', action='store_true', help='do not write OUT')
    ap.add_argument('--validate', action='store_true', help="reset the file's own turnarounds to road on a copy and report how many the rule re-finds")
    ap.add_argument('--no-overmarks', action='store_true'); ap.add_argument('--no-turnarounds', action='store_true')
    ap.add_argument('--plot', help='PNG of the changes (needs matplotlib)'); ap.add_argument('--bbox', help='X0,Z0,X1,Z1 window for --plot')
    a = ap.parse_args()

    obj = json.load(open(a.infile, encoding='utf-8'))
    if obj.get('format') != 'fh6-road-types' or obj.get('version') not in (1, 2):
        sys.exit('not a fh6-road-types v1/v2 file')
    work = os.path.abspath(a.work)
    nav, pos, edges, polys = load_nav(work)
    if obj.get('nav') and (obj['nav'].get('sha1') != nav['nav'].get('sha1') or obj['nav'].get('nodes') != nav['nav'].get('nodes')):
        sys.exit(f"the file was made for another nav file ({obj['nav']}) than {work}/roads.json ({nav['nav']})")
    unknown = [k for k in obj['types'] if k not in edges]
    bad = [k for k, v in obj['types'].items() if v not in TYPES]
    if unknown or bad:
        sys.exit(f'{len(unknown)} type keys are no nav edge ({unknown[:3]}), {len(bad)} have an unknown type ({bad[:3]})')
    terrain = Terrain(work)
    if terrain.E is None:
        print('note: no terr_e/elevation.npy - the ground-level island rule only reports (changes nothing)')

    old_counts = recount(obj, Net(obj, pos, edges, polys, terrain), obj['types'])
    if obj.get('counts') and obj['counts'] != old_counts:
        print('note: the recomputed counts of the INPUT differ from its own `counts` block:\n  file  ', dumps(obj['counts']), '\n  mine  ', dumps(old_counts))

    if a.validate:
        o2 = json.loads(json.dumps(obj))
        ex = [k for k, v in o2['types'].items() if v == 'turnaround']
        for k in ex:
            o2['types'][k] = 'road'
        ch, _ = find_turnarounds(Net(o2, pos, edges, polys, terrain))
        miss = [k for k in ex if k not in ch]
        print(f'validate: the turnaround rule re-finds {len(ex) - len(miss)} of the file\'s {len(ex)} turnarounds' + (f' (missed {miss})' if miss else '') +
              f'; it proposes {len(ch) - (len(ex) - len(miss))} more besides them')

    net = Net(obj, pos, edges, polys, terrain)
    changes, review = run_rules(net, not a.no_overmarks, not a.no_turnarounds)
    types = dict(obj['types'])
    for k, v in changes.items():
        types[k] = v[1]
    counts = recount(obj, net, types)

    regions = load_regions(work)
    rows = make_rows(net, changes, review, regions, polyline_of(net))
    by = collections.Counter(r['code'] for r in rows)
    km = collections.defaultdict(float)
    for k, v in changes.items():
        km[v[2]] += net.length(*map(int, k.split('-'))) / 1000
    summary = {'input': os.path.basename(a.infile), 'changes': len(changes), 'review': len(review), 'by_code': dict(by),
               'km_by_code': {k: round(v, 3) for k, v in km.items()}, 'counts': counts,
               'thresholds': {k: v for k, v in globals().items() if k.isupper() and k.startswith(('TURN_', 'OVER_', 'ISLAND_', 'GROUND_'))}}
    base = a.report or (a.outfile[:-5] if a.outfile.endswith('.json') else a.outfile) + '.review'
    write_report(base, rows, summary)
    print(f'{len(changes)} edges changed ({sum(1 for v in changes.values() if v[1] == "road")} highway->road, {sum(1 for v in changes.values() if v[1] == "turnaround")} -> turnaround), '
          f'{len(review)} need review; by code {dict(by)}; report {base}.csv/.json')
    if a.plot:
        plot(a.plot, net, rows, tuple(map(float, a.bbox.split(','))) if a.bbox else None)
        print('plot', a.plot)
    if a.dry_run:
        print('dry run: nothing written')
        return
    write_v2(a.outfile, obj, types, counts)
    print('wrote', a.outfile, 'counts', dumps(counts))


if __name__ == '__main__':
    main()
