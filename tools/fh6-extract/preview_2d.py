#!/usr/bin/env python3
"""Vanilla-style 2D road preview: a standalone local page that shows ONLY the roads of the FH6 island - transparent background, frameless,
no map imagery, no panels - like the in-game map.  A preview of what the road data allows.  READ-ONLY; the output contains game data
(road geometry), so write it OUTSIDE the repo and never publish/commit it.

    python3 -B preview_2d.py [--out ./fh6-viewer] [--work <out>-work] [--road-types PATH]
    then open  <out>/preview-2d.html  in a browser (file://, no CDN, no fetch).

Inputs: <work>/roads.json (nav graph from decode_nav.py / build_viewer.py - geometry) + the road-type file (default
assets/map/fh6-road-types.json; ids only, format `fh6-road-types` v1 or v2 - e.g. a fresh export from the viewer's road editor).
Output: <out>/preview-2d.html (data inlined, ~1 MB).

Edge model (same as the road editor): an *edge* = two consecutive nav nodes of a roads.json polyline, key "<min id>-<max id>".  Per-type draw lists
are built here as polylines (consecutive same-type edges of a game polyline are chained, so dashes run on); `added` links (any two points) are
single segments.  v2 extras: `points` (user nodes, ids >= 1000000), `moved` (position override per node id), `removed` (edges dropped),
`jump_from` (take-off node of a jump edge; the other end is the landing).  Unpainted game edges are drawn as 'unset' (faint dashed grey).
Coordinates in the page data are integer decimetres (x east, z north).
`turnaround` edges (AI cross-connections that are not on the in-game map) are kept in the page data and counted, but the page does not draw them
unless the debug key T is pressed; `highway` is drawn like road, wider.
"""
import argparse, json, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
CANON = os.path.join(HERE, '..', '..', 'assets', 'map', 'fh6-road-types.json')
TYPES = ('road', 'highway', 'offroad', 'other', 'trail', 'crosscountry', 'tunnel', 'jump', 'turnaround')


def load_types(path):
    o = json.load(open(path))
    if o.get('format') != 'fh6-road-types' or o.get('version') not in (1, 2):
        sys.exit(f'{path}: not a fh6-road-types v1/v2 file (format={o.get("format")!r}, version={o.get("version")!r})')
    o.setdefault('types', {}); o.setdefault('added', [])
    for k in ('points', 'moved', 'jump_from'):
        o.setdefault(k, {})                                     # v2 only; absent in v1
    o.setdefault('removed', [])
    for e in o['added']:
        e['type'] = e.get('type') or 'unset'                    # editor v2 may export type:null / no type
    bad = {t for t in list(o['types'].values()) + [e['type'] for e in o['added']] if t not in TYPES + ('unset',)}
    if bad:
        sys.exit(f'{path}: unknown edge type(s) {sorted(bad)}')
    return o


def build(work, road_types):
    r = json.load(open(os.path.join(work, 'roads.json')))
    if 'ids' not in r:
        sys.exit('roads.json has no node ids - re-run decode_nav.py')
    rt = load_types(road_types)
    key = lambda a, b: f'{min(a, b)}-{max(a, b)}'
    dm = lambda v: round(v * 10)
    pos = {}                                                    # node id -> (x, z) metres
    for ids, pl in zip(r['ids'], r['polylines']):
        for i, p in zip(ids, pl):
            pos[i] = (p[0], p[1])
    for i, x, z in r.get('orphans', []):
        pos[i] = (x, z)
    for i, p in rt['points'].items():
        pos[int(i)] = (p[0], p[1])
    for i, p in rt['moved'].items():
        pos[int(i)] = (p[0], p[1])                              # overrides game nav nodes
    removed, jump_from = set(rt['removed']), {k: int(v) for k, v in rt['jump_from'].items()}
    lines = {t: [] for t in TYPES + ('unset',) if t != 'jump'}  # type -> list of flat [x,z,x,z..] polylines (dm)
    jumps = []                                                  # [x1,z1,x2,z2] take-off -> landing (dm)
    counts = {t: 0 for t in TYPES + ('unset',)}
    seen, n_removed = set(), 0
    st = dict(cur=[], t=None)

    def flush():
        if len(st['cur']) >= 4:
            lines[st['t']].append(st['cur'])
        st['cur'], st['t'] = [], None

    def add_jump(a, b, k):
        s = jump_from.get(k, a)                                 # take-off; the other end is the landing
        e = b if s == a else a
        jumps.append([dm(pos[s][0]), dm(pos[s][1]), dm(pos[e][0]), dm(pos[e][1])])

    for ids in r['ids']:
        flush()
        for a, b in zip(ids, ids[1:]):
            k = key(a, b); seen.add(k)
            if k in removed:
                n_removed += 1; flush(); continue
            t = rt['types'].get(k, 'unset')
            counts[t] += 1
            if t == 'jump':
                flush(); add_jump(a, b, k); continue
            if t != st['t']:
                flush(); st['t'] = t
            if not st['cur']:
                st['cur'] += [dm(pos[a][0]), dm(pos[a][1])]
            st['cur'] += [dm(pos[b][0]), dm(pos[b][1])]
    flush()
    n_stale = sum(1 for k in rt['types'] if k not in seen and k not in removed)
    n_added = 0
    for e in rt['added']:
        a, b, t = int(e['a']), int(e['b']), e['type']
        if a not in pos or b not in pos:
            print(f'WARNING: added link {a}-{b} references an unknown node - skipped'); continue
        n_added += 1; counts[t] += 1
        if t == 'jump':
            add_jump(a, b, key(a, b))
        else:
            lines[t].append([dm(pos[a][0]), dm(pos[a][1]), dm(pos[b][0]), dm(pos[b][1])])
    xs = [p[0] for p in pos.values()]; zs = [p[1] for p in pos.values()]
    return dict(lines=lines, jumps=jumps, counts=counts, bounds=[dm(min(xs)), dm(min(zs)), dm(max(xs)), dm(max(zs))],
                src=dict(road_types=os.path.basename(road_types), version=rt['version'], nodes=len(pos), added=n_added,
                         removed=n_removed, stale=n_stale, user_points=len(rt['points']), moved=len(rt['moved'])))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--out', default='./fh6-viewer', help='output dir (default ./fh6-viewer) - keep it OUTSIDE the repo and do not publish it')
    ap.add_argument('--work', help='extractor outputs (default <out>-work); needs roads.json')
    ap.add_argument('--road-types', default=CANON, help='fh6-road-types v1/v2 file (default: the project canonical file)')
    a = ap.parse_args()
    out = os.path.abspath(a.out)
    work = os.path.abspath(a.work or out.rstrip('/') + '-work')
    repo = os.path.abspath(os.path.join(HERE, '..', '..'))
    if os.path.commonpath([out, repo]) == repo:
        sys.exit(f'refusing to write game data inside the repo ({out}); choose --out outside {repo}')
    if not os.path.isfile(os.path.join(work, 'roads.json')):
        sys.exit(f'{work}/roads.json missing - run build_viewer.py (or decode_nav.py) first')
    d = build(work, a.road_types)
    html = open(os.path.join(HERE, 'preview_2d.html'), encoding='utf-8').read()
    assert '/*__DATA__*/null' in html
    html = html.replace('/*__DATA__*/null', json.dumps(d, separators=(',', ':')))
    os.makedirs(out, exist_ok=True)
    p = os.path.join(out, 'preview-2d.html')
    open(p, 'w', encoding='utf-8').write(html)
    c = d['counts']
    print('edges drawn: ' + ', '.join(f'{t} {c[t]}' for t in c if c[t]) + f'   (total {sum(c.values())})')
    print('source:', json.dumps(d['src']))
    print(f'{p}  {os.path.getsize(p) / 1e6:.2f} MB')


if __name__ == '__main__':
    main()
