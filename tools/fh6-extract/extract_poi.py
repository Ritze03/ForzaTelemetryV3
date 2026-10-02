#!/usr/bin/env python3
"""Extract FH6 points of interest (world x/z, telemetry space) from the game install -> pois.json.

READ-ONLY on the install. Needs: numpy.
Usage: extract_poi.py [--media <...>/ForzaHorizon6/media] [--out ./fh6-out] [--entity-model OLD_EntityModel.zip]
(full notes: docs/game-data/fh6-game-files.md)
Besides the sources below it also reads: route40001/40900/4004x/4005x.nt (extra car meets, upsell pins), route30xx/8100-8105.nt (arena outlines ->
centroids), Freeroam/Ambient_RuralTrain_RuralLine.owcp (train polyline) and the `_FR_FLAG_` objects of gameobjs.xml.  `--entity-model` adds the
creator-dump-only categories (photo spots, time attacks, ...).  `race_pin` = race map pin, NOT the start line (see extract_races.py).
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
import argparse, json, math, os, re, struct, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, ci, resolve_media, rd, locators, tzones
from fh6str import StringTables

ap = argparse.ArgumentParser(description='Extract FH6 points of interest -> pois.json')
add_media_args(ap)
ap.add_argument('--entity-model', help='OPTIONAL older, readable Stripped/EntityModel.zip (the current install\'s copy is encrypted): adds photo spots, '
                'time attacks, horizon chases, backstage passes, ... = "creator-dump only" categories (see docs)')
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


# ---------------------------------------------------------------- race activation pins (36) + route nodes (170)
# NOTE: these spheres are the MAP PIN of a race, NOT its start line (0-780 m away, median 185 m).  Exact start lines/grids for
# ~170 routes come from extract_races.py (type `race_start` in races.json).  This type used to be called `race_start` here.
SRC = 'Tracks/Brio/triggerzones/tz_race_activations/race_triggers.tz'
for typ, n, x, y, z, sx, sz in tzones(ci(TB, 'triggerzones/tz_race_activations/race_triggers.tz')):
    rid = int(re.search(r'rt(\d+)$', n).group(1))
    add('race_pin', f'route {rid}', x, z, SRC, y, route=rid, radius=sx)


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
    # every `sidi_` prefix is 5 chars (an earlier version sliced n[14:] / n[7:-6] / n[6:] and produced 'et_001', 'easurecar_001', 'howcase_mech')
    elif re.match(r'sidi_aftermarket_board_\d+$', l): add('aftermarket_board', n[5:], x, z, SRC, y)
    elif re.match(r'sidi_aftermarket_\d+$', l): add('aftermarket_spot', n[5:], x, z, SRC, y)
    elif re.match(r'sidi_treasurecar_\d+_spawn$', l): add('treasure_car', n[5:-6], x, z, SRC, y)
    elif l.startswith('sidi_touge_event'): add('touge_event', n[17:], x, z, SRC, y)
    elif l.startswith('sidi_showcase'): add('showcase', n[5:], x, z, SRC, y)
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
    elif '_FR_FLAG_' in gid:
        add('flag_rush_flag', gid, x, z, 'Stripped/gs/brio/gameobjs.xml', y)

# ---------------------------------------------------------------- more car meets / upsell pins hidden in the small route files
# (carmeet_* in route40001/40900 were never read by the first version of this script)
for fn, disp in (('route40001', 'Evolving World car meet'), ('route40900', 'Hokubu car meet')):
    for n, x, y, z in locators(ci(TB, f'trackroutes/{fn}.nt')):
        if re.match(r'carmeet_\w+_locator$', n) and 'character' not in n and 'parking' not in n:
            add('car_meet', n[:-8], x, z, f'Tracks/Brio/trackroutes/{fn}.nt', y, display=disp)
_ups = {}                      # the series-4/5 upsell pins share one spot -> merged
for fn in ('route40900', 'route40041', 'route40042', 'route40043', 'route40044', 'route40051', 'route40052', 'route40053', 'route40054'):
    for n, x, y, z in locators(ci(TB, f'trackroutes/{fn}.nt')):
        if re.match(r'sidi_upsell_\w+$', n) and not n.endswith('_exit'):
            _ups.setdefault((round(x), round(z)), [x, y, z, []])[3].append(n[5:])
for (rx, rz), (x, y, z, ns) in _ups.items():
    add('upsell', ns[0] if len(ns) == 1 else 'upsell_series4/5 (car pack showcase spot)', x, z, 'Tracks/Brio/trackroutes/route400xx.nt', y, locators=ns)

# ---------------------------------------------------------------- mini-game arenas (outline points Arena_NNN -> centroid)
LM = {n: (x, z) for typ, n, x, y, z, sx, sz in tzones(ci(TB, 'triggerzones/tz_world_constraints/landmark_triggers.tz'))}


def centroid(fn):
    v = np.array([(x, y, z) for n, x, y, z in locators(ci(TB, f'trackroutes/{fn}.nt')) if re.match(r'Arena_\d+', n)])
    return v.mean(0), len(v)


# Playground Games arenas: 3 areas x 3 route files (team King / Survival / Flag Rush; 12 start_location_NN + finish_line_NN locators each).
# Area identified by the nearest landmark (docks / ski_resort / spaceport).
_gpl = {'docks': ('Tokyo City Docks', ('route3001', 'route3002', 'route3003')), 'spaceport': ('Irokawa Space Center', ('route3011', 'route3012', 'route3013')),
        'ski': ('Sotoyama Ski Resort', ('route3021', 'route3022', 'route3023'))}
_gpl_pos = {'docks': LM['docks'], 'ski': LM['ski_resort'], 'spaceport': LM['spaceport']}
_done = set()
for fn in sum((v[1] for v in _gpl.values()), ()):
    c, n = centroid(fn)
    k = min(_gpl_pos, key=lambda q: math.hypot(_gpl_pos[q][0] - c[0], _gpl_pos[q][1] - c[2]))
    if k in _done: continue
    _done.add(k)
    add('playground_arena', f'Playground Games arena: {_gpl[k][0]}', c[0], c[2], f'Tracks/Brio/trackroutes/{fn}.nt', c[1],
        route_files=list(_gpl[k][1]), outline_points=n, note='centroid of the Arena_NNN outline points')
# Hide & Seek arenas: the area names are the file names Stripped/gs/tracks/brio/scene/gameplay_locations_boundries/hideseek/gpl_hideseek_<name>.i.zip;
# the name <-> route file assignment below is INFERRED from geography (arena_01, city_east, city_west, east_coast, spaceport, west_coast).
for fn, area in {'route8100': 'arena_01', 'route8101': 'spaceport', 'route8102': 'city_east', 'route8103': 'west_coast', 'route8104': 'east_coast', 'route8105': 'city_west'}.items():
    c, n = centroid(fn)
    add('hide_seek_arena', f'Hide & Seek arena: {area}', c[0], c[2], f'Tracks/Brio/trackroutes/{fn}.nt', c[1], route_id=int(fn[5:]), outline_points=n,
        area_name_basis='inferred from position')

# ---------------------------------------------------------------- rural train line: Freeroam/Ambient_RuralTrain_RuralLine.owcp
# magic 'PCWO', 0x20-byte header, then 16-byte points {x, y, z, 0} (505 of them)
_b = open(ci(MEDIA, 'OpenWorld/Brio/Freeroam/Ambient_RuralTrain_RuralLine.owcp'), 'rb').read()
_p = np.frombuffer(_b, dtype='<f4', offset=0x20, count=(len(_b) - 0x20) // 16 * 4).reshape(-1, 4)[:, :3]
_p = _p[np.isfinite(_p).all(1) & (np.abs(_p[:, 0]) < 20000)]
add('train_line', 'Rural train line (ambient train path)', _p[0][0], _p[0][2], 'OpenWorld/Brio/Freeroam/Ambient_RuralTrain_RuralLine.owcp', _p[0][1],
    points=len(_p), polyline=[[round(float(q[0]), 1), round(float(q[2]), 1)] for q in _p[::5]], note='x/z = first point; polyline sampled every 5th point')

# ---------------------------------------------------------------- OPTIONAL: creator-dump-only categories (--entity-model PATH)
# Stripped/EntityModel.zip is ENCRYPTED in the current install; these come from an older readable copy (BXML).  Not loadable from a user install.
if _args.entity_model:
    from fh6bxml import EntityModel, xyz
    EM = EntityModel(_args.entity_model)
    ST = StringTables(MEDIA)
    OLD = ' (old build, creator dump)'

    def _pos(body, key='TriggerZonePosition'):
        m = re.search(rf'id="{key}" value="([^"]*)"', body)
        return xyz(m.group(1)) if m else None
    for eid, tpl, body in EM.entities('Entities/Brio/horizon_chase.xml'):           # 7 helicopter chases
        p = re.search(r'RouteStartTeleportPoint" value="([^"]*)"', body)
        if p:
            x, y, z = xyz(p.group(1))
            add('horizon_chase_start', eid, x, z, 'Entities/Brio/horizon_chase.xml' + OLD, y, route_id=int(re.search(r'RouteId" value="(\d+)"', body).group(1)))
    for eid, tpl, body in EM.entities('Entities/Brio/entities_photo_challenge_landmarks.xml'):   # 37 photo spots
        p = _pos(body)
        if p: add('photo_spot', re.sub(r'_trigger$', '', eid), p[0], p[2], 'Entities/Brio/entities_photo_challenge_landmarks.xml' + OLD, p[1])
    TA_NAMES = {'time_attack_02': 'Hokubu Time Attack', 'time_attack_03': 'Soni Time Attack', 'time_attack_04': 'Sekibe Time Attack', 'time_attack_05': 'Legend Island Time Attack'}
    for eid, tpl, body in EM.entities('Entities/Brio/time_attack.xml'):              # 4 leaderboard boards
        p = _pos(body)
        if p:
            add('time_attack', TA_NAMES.get(eid, eid), p[0], p[2], 'Entities/Brio/time_attack.xml' + OLD, p[1], entity=eid,
                name_basis='TimeAttack.str names matched to the nearest landmark circuit (inferred)')
    for path, ent, typ, nm in (('Entities/Brio/entities_offline_freeroam.xml', 'backstage_passes_activation', 'backstage_passes', 'Backstage Passes (rare car dealership)'),
                               ('Entities/Brio/entities_offline_freeroam.xml', 'labyrinth_matchmaking_activation', 'labyrinth_entrance', 'Labyrinth matchmaking activation'),
                               ('Entities/Brio/entities_track_global.xml', 'initial_experience_activator', 'ie_activator', 'Initial experience activator')):
        for eid, tpl, body in EM.entities(path):
            p = _pos(body, 'Position') if eid == ent else None
            if p: add(typ, nm, p[0], p[2], path + OLD, p[1])
    m = re.search(r'community_gift_event_shop_activation_base.*?id="Position" value="([^"]*)"', EM.xml('Templates/templates_community_gift_event.xml'), re.S)
    if m:
        p = xyz(m.group(1)); add('community_gift_shop', 'Community gift event shop', p[0], p[2], 'Templates/templates_community_gift_event.xml' + OLD, p[1])
    for eid, tpl, body in EM.entities('Entities/Brio/legend_island.xml'):
        p = _pos(body)
        if p: add('legend_island_gate', eid, p[0], p[2], 'Entities/Brio/legend_island.xml' + OLD, p[1])
    # readable English names of the 75 tz landmarks (DiscoveredTitleStringID -> Landmarks.str) ; two strings look swapped (see docs)
    _lm = {}
    for eid, tpl, body in EM.entities('Entities/Brio/landmark_areas.xml'):
        s = re.search(r'DiscoveredTitleStringID" value="([^"]*)"', body); tz = re.search(r'id="TriggerZoneName" value="([^"]*)"', body)
        if s: _lm[tz.group(1) if tz else eid] = ST.ids(s.group(1))
    for r in OUT:
        if r['type'] == 'landmark' and _lm.get(r['name']):
            r['extra']['display_name'] = _lm[r['name']]
            if r['name'] in ('seaside_circuit', 'seaside_offroad_circuit'):
                r['extra']['note'] = 'string looks swapped with the other seaside_* landmark (race evidence: Hokubu Circuit near (2830,2700), Sekibe near (2500,-5000))'


# ---------------------------------------------------------------- proc-cell tags in file lists (approximate: cell centre)
cells = []
for i in range(4):
    for line in open(ci(TB, f'ChunkContentsMiniZip{i}.txt'), errors='replace'):
        m = re.search(r'scene\\proc\\cellsize\\(\d+)\\(-?\d+)_(-?\d+)\\([^\\|]*\.pgeo)', line)
        if m: cells.append((int(m.group(1)), int(m.group(2)), int(m.group(3)), m.group(4), f'Tracks/Brio/ChunkContentsMiniZip{i}.txt'))
for s, cell_i, cell_j, nm, src in cells:         # (do not name a loop variable `ci` - it is the case-insensitive path helper)
    cx, cz = (cell_i + .5) * s, (cell_j + .5) * s
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
