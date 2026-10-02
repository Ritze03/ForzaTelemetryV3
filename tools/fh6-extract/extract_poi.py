#!/usr/bin/env python3
"""Extract FH6 points of interest (world x/z, telemetry space) from the game install -> pois.json.

READ-ONLY on the install. Needs: numpy.
Usage: extract_poi.py [--media <...>/ForzaHorizon6/media] [--out ./fh6-out]   (full notes: docs/game-data/fh6-game-files.md)
Sources (all under <media>/):
  Tracks/Brio/trackroutes/*.nt            XML <Locator><Name/><SceneTransform value._41/_42/_43 = world x/y/z>
  Tracks/Brio/triggerzones/*/*.tz         XML <triggerzone type name><position x y z/>... (sphere/box/mesh)
  Stripped/gs/brio/gameobjs.xml           XML <Obj GameplayID><Pos value="x,y,z"/>
  OpenWorld/Brio/AITracks/Route<N>.owt    binary 'FTWO': u32 count @0x24, nodes @0x60, 56 B each, f32 x,y,z first
  Tracks/Brio/ChunkContentsMiniZip*.txt   file lists of the geometry streams; proc-cell pgeo file names carry the
                                          200 m (etc.) grid cell "scene\\proc\\cellsize\\<S>\\<i>_<j>\\..." ->
                                          cell centre ((i+.5)S, (j+.5)S)  [APPROXIMATE, +-0.7*S]
Every record: {"type","name","x","z","y"?,"source","precision":"exact"|"cell","extra"?}
"""
import argparse, json, os, re, struct, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, ci, resolve_media

ap = argparse.ArgumentParser(description='Extract FH6 points of interest -> pois.json')
add_media_args(ap)
_args = ap.parse_args()
MEDIA = resolve_media(_args)
HERE = _args.out
TB = ci(MEDIA, 'Tracks/Brio')          # on-disk case is mixed; ci() makes this portable
OUT = []


def add(type_, name, x, z, source, y=None, precision='exact', **extra):
    r = {'type': type_, 'name': name, 'x': round(float(x), 1), 'z': round(float(z), 1), 'source': source, 'precision': precision}
    if y is not None: r['y'] = round(float(y), 1)
    if extra: r['extra'] = extra
    OUT.append(r)


def rd(p): return open(p, encoding='utf-8-sig', errors='replace').read()


def locators(path):
    t = rd(path)
    return [(n, float(x), float(y), float(z)) for n, x, y, z in re.findall(
        r'<Name value="([^"]*)"/>.*?value\._41="([-\d.]+)" value\._42="([-\d.]+)" value\._43="([-\d.]+)"', t, re.S)]


def tzones(path):
    t = rd(path); res = []
    for m in re.finditer(r'<triggerzone type="(\w+)" name="([^"]*)".*?<position x="([-\d.]+)" y="([-\d.]+)" z="([-\d.]+)" />\s*<size x="([-\d.]+)" y="([-\d.]+)" z="([-\d.]+)"', t, re.S):
        res.append((m.group(1), m.group(2), float(m.group(3)), float(m.group(4)), float(m.group(5)), float(m.group(6)), float(m.group(8))))
    return res


# ---------------------------------------------------------------- race activation triggers (36) + route nodes (170)
SRC = 'Tracks/Brio/triggerzones/tz_race_activations/race_triggers.tz'
for typ, n, x, y, z, sx, sz in tzones(ci(TB, 'triggerzones/tz_race_activations/race_triggers.tz')):
    rid = int(re.search(r'rt(\d+)$', n).group(1))
    add('race_start', f'route {rid}', x, z, SRC, y, route=rid, radius=sx)


def owt_nodes(path):
    d = open(path, 'rb').read()
    n = struct.unpack_from('<I', d, 0x24)[0]
    a = np.frombuffer(d, dtype=np.dtype([('p', '<f4', 3), ('r', 'u1', 44)]), offset=0x60, count=n)['p']
    return a[np.isfinite(a).all(axis=1)]


_ai = ci(MEDIA, 'OpenWorld/Brio/AITracks')
for f in sorted((os.path.join(_ai, e) for e in os.listdir(_ai) if re.fullmatch(r'Route\d+\.owt', e, re.I)),
                key=lambda s: int(re.findall(r'\d+', os.path.basename(s))[0])):
    rid = int(re.findall(r'\d+', os.path.basename(f))[0])
    p = owt_nodes(f)
    closed = bool(np.hypot(*(p[0] - p[-1])[[0, 2]]) < 5)
    add('route_node0', f'route {rid}', p[0][0], p[0][2], f'OpenWorld/Brio/AITracks/Route{rid}.owt', p[0][1],
        route=rid, circuit=closed, nodes=int(len(p)),
        end=None if closed else [round(float(p[-1][0]), 1), round(float(p[-1][2]), 1)])

# ---------------------------------------------------------------- route0.nt: the big named-locator list (371)
SRC = 'Tracks/Brio/trackroutes/route0.nt'
for n, x, y, z in locators(ci(TB, 'trackroutes/route0.nt')):
    l = n.lower()
    if l.startswith('barn_finds_cinematic_'): add('barn_find', n[21:], x, z, SRC, y)
    elif l.startswith('barn_finds_anna_hint_'): add('barn_find_hint', n[21:], x, z, SRC, y)
    elif l.startswith('player_house_') and l.endswith('_root_locator'): add('house', n[13:-13], x, z, SRC, y)
    elif l.endswith('fast_travel_locator'): add('fast_travel', n[:-20], x, z, SRC, y)
    elif l in ('festival_root_locator', 'legend_island_outpost_root_locator'): add('festival_site', n[:-13], x, z, SRC, y)
    elif l.startswith('estate_') and l != 'estate_fast_travel_locator': add('estate', n, x, z, SRC, y)
    elif re.match(r'carmeet_\w+_locator$', l) and 'character' not in l and 'parking' not in l: add('car_meet', n[:-8], x, z, SRC, y)
    elif re.match(r'drag_meet_\d+_activation', l): add('drag_meet', n[:-11], x, z, SRC, y)
    elif re.match(r'drag_meet_\d+_finish_line', l): add('drag_meet_finish', n[:-12], x, z, SRC, y)
    elif re.match(r'sidi_aftermarket_board_\d+$', l): add('aftermarket_board', n[14:], x, z, SRC, y)
    elif re.match(r'sidi_aftermarket_\d+$', l): add('aftermarket_spot', n[14:], x, z, SRC, y)
    elif re.match(r'sidi_treasurecar_\d+_spawn$', l): add('treasure_car', n[7:-6], x, z, SRC, y)
    elif l.startswith('sidi_touge_event'): add('touge_event', n[17:], x, z, SRC, y)
    elif l.startswith('sidi_showcase'): add('showcase', n[6:], x, z, SRC, y)
    elif l.startswith('sidi_hj_'): add('horizon_job', n[8:], x, z, SRC, y)
    elif l.startswith('sidi_hs_'): add('horizon_story', n[8:], x, z, SRC, y)
    elif l.startswith('sidi_rush_'): add('rush_event', n[5:], x, z, SRC, y)
    elif l.startswith('sidi_invitational') or l.startswith('sidi_legendevent'): add('special_event', n[5:], x, z, SRC, y)
    elif l.startswith('sidi_upsell') and not l.endswith('_exit'): add('upsell', n[5:], x, z, SRC, y)

# ---------------------------------------------------------------- other trackroutes/*.nt
for fn, typ, pat in [('pinata_locators', 'pinata', None), ('eliminator_locators', 'eliminator_spawn', None), ('parkingareas', 'parking_area', None)]:
    for n, x, y, z in locators(ci(TB, f'trackroutes/{fn}.nt')):
        add(typ, n, x, z, f'Tracks/Brio/trackroutes/{fn}.nt', y)
for fn in ('job_challenges_startend_locations', 'bucket_challenges_startend_locations'):
    groups = {}
    for n, x, y, z in locators(ci(TB, f'trackroutes/{fn}.nt')):
        m = re.match(r'(VOL_\w+?)_start(\d+)$', n)
        if m: groups.setdefault(m.group(1), []).append((x, y, z))
    for g, v in groups.items():
        v = np.array(v)
        typ = 'story_volume' if g.startswith('VOL_HS') else 'job_volume'
        add(typ, g[4:], v[:, 0].mean(), v[:, 2].mean(), f'Tracks/Brio/trackroutes/{fn}.nt', v[:, 1].mean(), starts=len(v))
for rg in ('canyon', 'city', 'east_coast', 'festival', 'highlands', 'legend_island', 'north_plains', 'snowy_mountains', 'south_coast', 'south_plains'):
    v = np.array([(x, y, z) for _, x, y, z in locators(ci(TB, f'trackroutes/map_region_{rg}.nt'))])
    add('map_region', rg, v[:, 0].mean(), v[:, 2].mean(), f'Tracks/Brio/trackroutes/map_region_{rg}.nt', v[:, 1].mean(), points=len(v),
        note='Arena_NNN locators = outline points of the region; x/z here is their centroid')

# ---------------------------------------------------------------- trigger zones
SRC = 'Tracks/Brio/triggerzones/tz_world_constraints/landmark_triggers.tz'
for typ, n, x, y, z, sx, sz in tzones(ci(TB, 'triggerzones/tz_world_constraints/landmark_triggers.tz')):
    add('landmark', n, x, z, SRC, y, shape=typ, size=[sx, sz])
for typ, n, x, y, z, sx, sz in tzones(ci(TB, 'triggerzones/tz_creatures/creatures_all.tz')):
    add('creature_zone', n, x, z, 'Tracks/Brio/triggerzones/tz_creatures/creatures_all.tz', y)
for typ, n, x, y, z, sx, sz in tzones(ci(TB, 'triggerzones/tz_bucket_challenges/tz_horizonstories.tz')):
    if n.endswith('activation_zone'):
        add('story_activation' if n.startswith('HS_') else 'job_activation', n[:-16], x, z,
            'Tracks/Brio/triggerzones/tz_bucket_challenges/tz_horizonstories.tz', y, radius=sx)

# ---------------------------------------------------------------- gameobjs.xml (exact)
t = rd(ci(MEDIA, 'Stripped/gs/brio/gameobjs.xml'))
seen = set()
for gid, pos in re.findall(r'<Obj GameplayID="([^"]*)"[^>]*>\s*<Pos value="([^"]*)"', t):
    x, y, z = map(float, pos.split(','))
    if gid.startswith('DISCOUNT_BOARD_TREASURE_CHEST'):
        k = (gid, round(x), round(z))
        if k not in seen: seen.add(k); add('treasure_chest_board', gid, x, z, 'Stripped/gs/brio/gameobjs.xml', y)

# ---------------------------------------------------------------- proc-cell tags in file lists (approximate: cell centre)
cells = []
for i in range(4):
    for line in open(ci(TB, f'ChunkContentsMiniZip{i}.txt'), errors='replace'):
        m = re.search(r'scene\\proc\\cellsize\\(\d+)\\(-?\d+)_(-?\d+)\\([^\\|]*\.pgeo)', line)
        if m: cells.append((int(m.group(1)), int(m.group(2)), int(m.group(3)), m.group(4), f'Tracks/Brio/ChunkContentsMiniZip{i}.txt'))
for s, ci, cj, nm, src in cells:
    cx, cz = (ci + .5) * s, (cj + .5) * s
    if (m := re.search(r'driftzonemarker_(\d+)_(left|right)_(\d)', nm)):
        if m.group(2) == 'left':
            add('drift_zone', f'zone {m.group(1)} marker {m.group(3)}', cx, cz, src, precision='cell', zone=int(m.group(1)), cell=s,
                note='left_1/left_2 = the two gate markers of drift zone NN')
    elif (m := re.search(r'tag_dangersign_bm_(\d+)', nm)): add('danger_sign', f'bm_{m.group(1)}', cx, cz, src, precision='cell', cell=s)
    elif (m := re.search(r'discount_board_xp_a_?(\d+)', nm)): add('xp_board', f'xp_board_{m.group(1)}', cx, cz, src, precision='cell', cell=s)
    elif (m := re.search(r'tag_time_attack_drift_circuit_(\w+?)_?\.pgeo', nm)): add('drift_circuit_prop', m.group(1), cx, cz, src, precision='cell', cell=s)
    elif (m := re.search(r'barn_find_(\w+?)\.pgeo', nm)): add('barn_building_cell', m.group(1), cx, cz, src, precision='cell', cell=s)

json.dump(OUT, open(f'{HERE}/pois.json', 'w'), indent=0, separators=(',', ':'))
from collections import Counter
for k, v in Counter(r['type'] for r in OUT).most_common(): print(f'{v:6d}  {k}')
print('total', len(OUT), '->', f'{HERE}/pois.json')
