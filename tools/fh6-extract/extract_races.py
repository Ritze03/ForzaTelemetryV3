#!/usr/bin/env python3
"""FH6 race start lines / grids / finish lines for all ~170 routes (+ best-effort race names) -> races.json.   READ-ONLY on the install.

Usage: extract_races.py [--media <...>/ForzaHorizon6/media] [--out ./fh6-out] [--entity-model PATH]
Needs: numpy.  With --entity-model also scipy (Hungarian name matching).   Full notes: docs/game-data/fh6-game-files.md.

KEY FINDING: `OpenWorld/Brio/AITracks/Route<N>.nav` is not only a road graph.  After the 'WVAN' graph there is a SECOND block,
magic 'RVAN' (16 B header: 'RVAN', u32 ver=2, u32 id, u32 size), that holds the race locators of that route:
     +0   f32[4] start_line  (x,y,z,0)         +16 f32[4] finish_line   (== start_line for circuits)
     +56  u32 nrect (checkpoint-gate records, 76 B each, centre + 4 corners + id, starting at +80)
     +60  u32 npts = 14, +64 u32 251
     ...  14 x 48 B records {pos f32[3],0, dir f32[3],0, u32 idx,0,0,0} = start_line, finish_line and the 12 grid slots
          start_location_000..011; their order == the order of the NUL-terminated name pool at the end of the block (hash
          sorted), so the names must be read from that pool.
=> exact start line + 12-slot grid + heading for 169/170 routes (route 99 is a test route at 0,0).  start_line lies on the
route's own .owt racing line (<0.1 m in 168/170).  The `race_trigger_zone_rt<N>` spheres (extract_poi.py `race_pin`) are only the
MAP PIN (0-780 m from the start, median 185 m; circuits: anywhere on the loop).  .owt node 0 is just the lead-in.

EXACT race names + types (111 of the routes): `ObjectModelGame.zip` (plaintext) -> fh6careers.py: TrackInfoDataSet.InfoByRouteId links
route -> CareerRace key -> DisplayName string (EN + DE) and the type (UITheme / UseCrossCountryAI / StreetRace flyer).  Written into
extra as names{EN,DE}, type_exact (ABSENT when the files state none - nothing is inferred), type_source, career_key, ribbon; the EN name
also becomes race_name (confidence 'exact') and overrides everything below.

Older fallback for the remaining routes (the pre-ObjectModelGame approach; kept for the ~60 routes without a TrackInfo entry):
`Stripped/StringTables/EN.zip` CareerRaceCollection.str (Name column = entries >= 154).  Without --entity-model only the 3 Horizon Rush
(via `sidi_rush_*` locators), the 5 Initial-Experience routes and the 7 Horizon Chases get names.  With --entity-model (an OLDER, readable
EntityModel.zip - the current install's copy is encrypted, see docs) it also derives the event family of 88 routes
(Entities/Brio/campaign_slots.xml), 10 exact names (4 finales via ContextId + 6 via post_race_locators.xml) and ~44 more by the
heuristic "the family's names are assigned to its routes by Hungarian matching on the distance landmark -> racing line, landmark
keywords in K below" (+ a few by elimination).  Confidence is recorded per record (extra.name_confidence); treat anything but
'exact'/'locator' as a guess.
"""
import argparse, glob, json, math, os, re, struct, sys, zipfile
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, ci, resolve_media, locators, tzones
from fh6str import StringTables
from fh6owt import parse_rvan, positions as owt_positions
import fh6careers

ap = argparse.ArgumentParser(description='Extract race start lines/grids from AITracks/Route<N>.nav (RVAN block) -> races.json')
add_media_args(ap)
ap.add_argument('--entity-model', help='OPTIONAL path to an older, readable (non-encrypted) Stripped/EntityModel.zip: adds event types and more race names')
ARGS = ap.parse_args()
MEDIA = resolve_media(ARGS)
TB = ci(MEDIA, 'Tracks/Brio')
AIT = ci(MEDIA, 'OpenWorld/Brio/AITracks')
OUT = []
CAREER_NAMES_FROM = 154        # CareerRaceCollection.str: entries < 154 are descriptions, >= 154 the names (323 entries in the Sept-2026 build)


def add(type_, name, x, z, source, y=None, precision='exact', **extra):
    r = {'type': type_, 'name': name, 'x': round(float(x), 1), 'z': round(float(z), 1), 'source': source, 'precision': precision}
    if y is not None:
        r['y'] = round(float(y), 1)
    extra = {k: v for k, v in extra.items() if v is not None}
    if extra:
        r['extra'] = extra
    OUT.append(r)


# ================================================================================================ route files
def parse_nav(path):
    return parse_rvan(path)                                      # RVAN block, see fh6owt.py


def owt_line(rid):
    """driven racing line of a route: finite node positions of Route<N>.owt, ALL sections (fh6owt.read_owt; the old reader dropped
    nodes on the 6 multi-section files)"""
    return owt_positions(os.path.join(AIT, f'Route{rid}.owt'))


def dist_to_line(rid, x, z):
    p = LINE[rid]
    return float(np.min(np.hypot(p[:, 0] - x, p[:, 2] - z)))


RIDS = sorted(int(re.findall(r'\d+', os.path.basename(f))[0]) for f in glob.glob(os.path.join(AIT, 'Route*.nav')) + glob.glob(os.path.join(AIT, 'route*.nav')))
RIDS = sorted(set(RIDS))
NAV = {r: parse_nav(os.path.join(AIT, f'Route{r}.nav')) for r in RIDS}
LINE = {r: owt_line(r) for r in RIDS}

# activation trigger spheres = map pins (36 routes)
TRIG = {}
for typ, n, x, y, z, sx, sz in tzones(ci(TB, 'triggerzones/tz_race_activations/race_triggers.tz')):
    TRIG[int(re.search(r'rt(\d+)$', n).group(1))] = (x, y, z)
LM = {n: (x, z) for typ, n, x, y, z, sx, sz in tzones(ci(TB, 'triggerzones/tz_world_constraints/landmark_triggers.tz'))}

# ================================================================================================ event family / names
SLOT = {}                      # route id -> (family, CareerRaceCollectionId)     [needs --entity-model]
EM = None
if ARGS.entity_model:
    from fh6bxml import EntityModel
    EM = EntityModel(ARGS.entity_model)
    for eid, tpl, body in EM.entities('Entities/Brio/campaign_slots.xml'):
        m = re.match(r'brio_(\w+?)_(?:event_)?(\d+)$', eid)
        cid = re.search(r'CareerRaceCollectionId" value="(\d+)"', body)
        if m and cid:
            SLOT[int(m.group(2))] = (m.group(1), int(cid.group(1)))
COLL2ROUTE = {c: r for r, (f, c) in SLOT.items()}


def event_type(rid):
    if rid in SLOT:
        fam, c = SLOT[rid]
        if fam == 'circuit': return 'xc_circuit' if 80 <= c <= 87 else 'road_circuit'     # collection-id ranges split road vs cross country
        if fam == 'sprint': return 'xc_sprint' if 88 <= c <= 97 else 'road_sprint'
        if fam in ('scramble', 'trail', 'street', 'touge', 'finale', 'horizon_rush'): return fam
    if 8001 <= rid <= 8008: return 'horizon_rush'
    if 3333 <= rid <= 3337: return 'initial_experience'
    if 30100 <= rid <= 30106: return 'horizon_chase'
    return 'unknown'


ST = StringTables(MEDIA)
_cr = ST.table('CareerRaceCollection')
if len(_cr) != 323:
    print(f'WARNING: CareerRaceCollection.str has {len(_cr)} entries (323 expected) - re-check CAREER_NAMES_FROM={CAREER_NAMES_FROM}', file=sys.stderr)
cr_names = [s for k, s in _cr[CAREER_NAMES_FROM:]]
NAME = {}                      # rid -> (name, confidence, evidence)
CAND = {}

if EM:
    # (1) ground truth: post_race_locators.xml names the exit locator after the race
    for eid, tpl, body in EM.entities('Entities/Brio/post_race_locators.xml'):
        m = re.match(r'race_(\d+)$', eid); ln = re.search(r'LocatorName" value="([^"]*)"', body)
        if not (m and ln): continue
        rid = COLL2ROUTE.get(int(m.group(1)))
        nm = re.sub(r'_custom_exit_locator$', '', ln.group(1))
        cand = [n for n in cr_names if re.sub(r'[^a-z0-9]+', '_', n.lower()).strip('_') == nm]
        if rid and cand:
            NAME[rid] = (cand[0], 'exact', f'post_race_locators race_{m.group(1)} -> {ln.group(1)}')
    # (2) finales: ContextId "The<Name>RaceEventActivation"
    _slots = {e: b for e, t, b in EM.entities('Entities/Brio/campaign_slots.xml')}
    for rid, (fam, c) in SLOT.items():
        if fam != 'finale': continue
        ctx = re.search(r'ContextId" value="The(\w+?)RaceEventActivation"', _slots.get(f'brio_finale_event_{rid}', ''))
        if ctx and 'The ' + ctx.group(1) in cr_names:
            NAME[rid] = ('The ' + ctx.group(1), 'exact', f'campaign_slots ContextId The{ctx.group(1)}RaceEventActivation')

    # (3) heuristic: the names of one family -> its routes, by distance(landmark keyword, route racing line), Hungarian assignment.
    #     The 'seaside_*' landmark strings look swapped (Hokubu/Sekibe) so both keywords are listed for the Scramble/Circuit names.
    from scipy.optimize import linear_sum_assignment
    K = {
     'road_circuit': {'Shimanoyama Circuit': ['drift_circuit'], 'Hokubu Circuit': ['seaside_offroad_circuit'], 'Irokawa Circuit': ['spaceport'],
       'Edamame Circuit': [], 'Electric Town Circuit': ['akihabara'], 'Highway Circuit': [], 'Soni Circuit': ['mountain_circuit'],
       'Legend Island Circuit': ['legend_island_circuit'], 'Daikoku Circuit': ['daikoku_parking_lot'], 'Narai-Juku Circuit': ['narai_juku_hot_springs'],
       'Shirakawa Circuit': ['shirakawa_go'], 'Festival Nishi': ['festival_site'], 'Frost and Furious': ['snow_monster_forest', 'ski_resort']},
     'road_sprint': {'Tateyama Kurobe Sprint': ['bandai_azuma_snow_corridor'], 'Satta Sprint': ['satta_pass'], 'Tokyo Railway Sprint': ['tokyo_railway_station'],
       'Seaside Park Sprint': ['seaside_park'], 'Ito Sprint': [], 'Shikisai Sprint': ['shikisai_no_oka'], 'Festival Sprint': ['festival_site'],
       'Coastline Sprint': [], 'Shimanoyama Sprint': [], 'Venus Sprint': ['venus_line']},
     'xc_circuit': {'Snow Forest Cross Country Circuit': ['snow_monster_forest'], 'Nangan Cross Country Circuit': [], 'Legend Island Cross Country Circuit': ['legend_island_circuit'],
       'City Docks Cross Country Circuit': ['docks'], 'Naruo Cross Country Circuit': ['golf_course'], 'Oka Cross Country Circuit': [],
       'Stadium Cross Country Circuit': ['stadium'], 'Edogawa Cross Country Circuit': ['edogawa_baseball_stadium']},
     'xc_sprint': {'Izu Cross Country': ['izu_skyline'], 'Soni Highlands Cross Country': ['soni_highlands'], 'Shimanoyama Cross Country': [], 'Takashiro Cross Country': [],
       'Yahikoyama Cross Country': ['yahikoyama_panorama_ropeway'], 'Tateyama Alpine Cross Country': ['bandai_azuma_snow_corridor'],
       'Shinjuku Gyoen Cross Country': ['shinjuku_national_garden'], 'Ruriko-ji Cross Country': ['ruriko_ji_temple'], 'Temple Cross Country': ['temple_of_nachi_falls'],
       'Wind Farm Cross Country': ['wind_farm']},
     'scramble': {'Sunflower Scramble': ['sunflower_farm'], 'Hirosaki Scramble': ['hirosaki_castle'], 'Ine Scramble': ['ine'], 'Taiyaki Scramble': [],
       'Sotoyama Scramble': ['ski_resort'], 'Sekibe Scramble': ['seaside_circuit', 'sekibe_kaijo_bridge'], 'Horizon Stadium Scramble': ['stadium'],
       'Chiheisen Scramble': [], 'Bamboo Forest Scramble': ['arashiyama_bamboo_forest'], 'Kawazu Nanadaru Scramble': ['loop_bridge']},
     'trail': {'Cherry Field Trail': ['cherry_fields'], 'Airfield Trail': ['airfield'], 'Nukabira Trail': ['lake_nukabira'], 'Legend Island Trail': ['legend_island_circuit'],
       'Waterfall Trail': ['temple_of_nachi_falls'], 'Oyashirazu Trail': ['oyashirazu_cliff_tunnels'], 'Takashiro Trail': [], 'Kinkaku-ji Trail': ['kinkaku_ji_temple'],
       'Hokubu Trail': [], 'Ito Trail': []},
     'street': {'Nachi Run': ['temple_of_nachi_falls'], 'Sunflower Charge': ['sunflower_farm'], 'Matsumi Climb': ['matsumi_great_bridge'], 'Costa Rocosa': [],
       'Norikura Descent': ['norikura_skyline'], 'Okishinaimura Run': ['okishinaimura'], 'Kita Ine': ['ine'], 'Rainbow Bridge Descent': ['rainbow_bridge'],
       'Tokyo City Docks Charge': ['docks'], 'River Descent': [], 'Minami Chase': ['minami_awa_double_road'], 'Yoshinoyama Charge': ['yoshinoyama'],
       'Cedar Run': ['cedar_lane'], 'Daikoku Chase': ['daikoku_parking_lot'], 'Pier Pressure': [], 'Hokubu Ascent': []},
     'touge': {'Mt. Haruna': ['mt_haruna'], 'Norikura Skyline': ['norikura_skyline'], 'Bandai Azuma': ['bandai_azuma_skyline'], 'Hakone Nanamagari': ['hakone_nanamagari'],
       'Arashiyama Takao': ['arashiyama_bamboo_forest']},
    }
    NEUTRAL = 1500.0
    for et, names in K.items():
        routes = [r for r in RIDS if event_type(r) == et and r not in NAME]
        used = {v[0] for v in NAME.values()}
        nm = [n for n in names if n not in used and n in cr_names]
        if not routes or not nm: continue
        C = np.full((len(nm), len(routes)), NEUTRAL)          # cost = metres from landmark to racing line (names without keyword: neutral)
        for i, n in enumerate(nm):
            for j, r in enumerate(routes):
                ks = [k for k in names[n] if k in LM]
                if ks: C[i, j] = min(min(dist_to_line(r, *LM[k]) for k in ks), 3000.0)
        ri, cj = linear_sum_assignment(C)
        left_n = set(range(len(nm))); left_r = set(range(len(routes)))
        for i, j in zip(ri, cj):
            if C[i, j] <= 1000:
                NAME[routes[j]] = (nm[i], 'landmark<=400m' if C[i, j] <= 400 else 'landmark<=1000m', f'closest keyword landmark {C[i, j]:.0f} m from racing line')
                left_n.discard(i); left_r.discard(j)
        if len(left_n) == 1 and len(left_r) == 1:
            NAME[routes[left_r.pop()]] = (nm[left_n.pop()], 'elimination', 'last unassigned name/route of the family')
        else:
            for j in left_r: CAND[routes[j]] = [nm[i] for i in sorted(left_n)]

# Initial experience: templates initial_experience_section_{one..four}_route -> Route3333..3336, city tour -> Route3337 (names from IE* entries of the table)
for rid, nm in ((3333, 'IE Drive Section 1'), (3334, 'IE Drive Section 2'), (3335, 'IE Drive Section 3'), (3336, 'IE Drive Section 4'), (3337, 'IE City Tour')):
    NAME[rid] = (nm, 'exact', 'route-id convention (templates_initial_experience.xml section_one..four -> Route3333..3336, city tour -> 3337)')
for i, rid in enumerate(range(30100, 30107)):
    NAME[rid] = (f'Horizon Chase {i + 1}', 'exact', 'route 30100+n-1 = ChaseEventID n (horizon_chase.xml)')

# Horizon Rush: the sidi_rush_* locators (route0.nt) sit next to the 3 rush starts
RUSH_TXT = {'docks': 'Tokyo City Docks', 'ski': 'Sotoyama Ski Resort', 'spaceport': 'Irokawa Space Center'}
_rush = {n[10:]: (x, z) for n, x, y, z in locators(ci(TB, 'trackroutes/route0.nt')) if n.lower().startswith('sidi_rush_')}
for rid in (8001, 8002, 8003):
    if rid not in NAV or not _rush: continue
    A = NAV[rid]['A']
    k = min(_rush, key=lambda q: math.hypot(_rush[q][0] - A[0], _rush[q][1] - A[2]))
    if k in RUSH_TXT:
        NAME[rid] = (f'Horizon Rush: {RUSH_TXT[k]}', 'locator', f'nearest sidi_rush_{k} {math.hypot(_rush[k][0]-A[0], _rush[k][1]-A[2]):.0f} m')

# exact race names + exact race types from ObjectModelGame.zip (plaintext; see fh6careers.py) - these override every guess above
try:
    EXACT = fh6careers.race_types(MEDIA)
except (FileNotFoundError, KeyError, zipfile.BadZipFile) as e:
    EXACT = {}
    print(f'WARNING: ObjectModelGame.zip not usable ({type(e).__name__}: {e}) - no exact race names / types', file=sys.stderr)
for rid, x in EXACT.items():
    if x['names'].get('EN'):
        NAME[rid] = (x['names']['EN'], 'exact', f'TrackInfoDataSet[{x["career_key"]}].DisplayName (ObjectModelGame.zip)')

# ================================================================================================ emit
SRC_NAV = 'OpenWorld/Brio/AITracks/Route{}.nav (RVAN block) + owt'
on_line = []
for rid in RIDS:
    n = NAV[rid]; A, B = n['A'], n['B']
    if abs(A[0]) < 1 and abs(A[2]) < 1:
        continue                                                # route 99: test route, start at origin
    et = event_type(rid)
    circuit = math.hypot(A[0] - B[0], A[2] - B[2]) < 5
    line = LINE[rid]
    length = float(np.sum(np.hypot(*np.diff(line[:, [0, 2]], axis=0).T)))
    sl = n['pts'].get('start_line'); d = sl[3:6] if sl else None
    heading = round(math.degrees(math.atan2(d[0], d[2])), 1) if d else None          # 0 = +z (north), 90 = +x
    grid = [[round(n['pts'][f'start_location_{i:03d}'][0], 1), round(n['pts'][f'start_location_{i:03d}'][2], 1)] for i in range(12)]
    nm = NAME.get(rid)
    sure = nm and nm[1] in ('exact', 'landmark<=400m', 'locator')
    typ = {'initial_experience': 'ie_route', 'horizon_chase': 'horizon_chase'}.get(et, 'race_start')
    on_line.append(dist_to_line(rid, A[0], A[2]))
    ex = dict(route_id=rid, event_type=et, circuit=circuit, length_m=round(length), heading_deg=heading, grid=grid,
              finish=None if circuit else [round(B[0], 1), round(B[2], 1)],
              collection_id=SLOT[rid][1] if rid in SLOT else None,
              race_name=nm[0] if sure else None, race_name_guess=nm[0] if nm and not sure else None,
              name_confidence=nm[1] if nm else None, name_evidence=nm[2] if nm else None, name_candidates=CAND.get(rid))
    xe = EXACT.get(rid)
    if xe:                                                      # exact fields only; type_exact is absent when the game files state no type
        ex.update(type_exact=xe['type_exact'], type_source=xe['type_source'], career_key=xe['career_key'], ribbon=xe['ribbon'], names=xe['names'])
    if rid in TRIG:
        t = TRIG[rid]
        ex['activation'] = [round(t[0], 1), round(t[2], 1)]
        ex['activation_dist_m'] = round(math.hypot(t[0] - A[0], t[2] - A[2]))
    add(typ, nm[0] if nm else f'route {rid}', A[0], A[2], SRC_NAV.format(rid), A[1], **ex)
    if not circuit and typ == 'race_start':
        add('race_finish', (nm[0] + ' (finish)') if nm else f'route {rid} finish', B[0], B[2], SRC_NAV.format(rid), B[1], route_id=rid, event_type=et)

# touge 5411 has no trigger sphere: its map pin is a locator in route0.nt (the start line is the race_start record of route 5411)
for n, x, y, z in locators(ci(TB, 'trackroutes/route0.nt')):
    m = re.match(r'sidi_touge_event_(\d+)$', n.lower())
    if m and int(m.group(1)) == 5411:
        add('touge_pin', f'touge {m.group(1)}', x, z, 'Tracks/Brio/trackroutes/route0.nt', y, route_id=5411)

out = os.path.join(ARGS.out, 'races.json')
json.dump(OUT, open(out, 'w'), indent=0, separators=(',', ':'))

# ================================================================================================ summary
from collections import Counter
for k, v in Counter(r['type'] for r in OUT).most_common():
    print(f'{v:5d}  {k}')
print('total', len(OUT), '->', out)
rs = [r for r in OUT if r['type'] == 'race_start']
ol = np.array(on_line)
print(f'start line to own racing line: median {np.median(ol):.2f} m, <0.1 m in {int((ol < 0.1).sum())}/{len(ol)}')
act = [r['extra']['activation_dist_m'] for r in rs if 'activation_dist_m' in r['extra']]
if act:
    print(f'activation pin to start line: n={len(act)} median {np.median(act):.0f} m max {max(act)} m')
print('event types:', dict(Counter(r['extra']['event_type'] for r in rs)))
tc = Counter(r['extra'].get('type_exact') for r in OUT if r['type'] in ('race_start', 'ie_route', 'horizon_chase') and 'career_key' in r['extra'])
print('exact types:', dict(tc), '| records with a TrackInfo entry:', sum(tc.values()), 'of', len(EXACT), 'routes')
print('names:', dict(Counter((r['extra'].get('name_confidence') or 'none') for r in rs + [r for r in OUT if r['type'] in ('ie_route', 'horizon_chase')])))
roads = os.path.join(ARGS.out, 'roads.json')
if os.path.exists(roads):                                      # optional validation against decode_nav.py output
    pts = []
    for q in json.load(open(roads))['polylines']:
        q = np.array(q)
        for a, c in zip(q[:-1], q[1:]):
            k = max(1, int(np.hypot(*(c - a)) // 10)); pts.append(a + (c - a) * np.linspace(0, 1, k, endpoint=False)[:, None])
    pts = np.vstack(pts)
    dd = [float(np.min(np.hypot(pts[:, 0] - r['x'], pts[:, 1] - r['z']))) for r in rs]
    print(f'race_start n={len(rs)} start line to nearest nav road: median {np.median(dd):.1f} m, <15 m {np.mean(np.array(dd) < 15) * 100:.0f}%')
