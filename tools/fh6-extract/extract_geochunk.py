#!/usr/bin/env python3
"""FH6 gameplay-prop world positions (speed traps, drift zones, xp boards, danger signs, ...) -> geochunk_pois.json.
READ-ONLY on the game install.  Needs numpy.   Usage: extract_geochunk.py [--media <...>/ForzaHorizon6/media] [--out ./fh6-out]
Format notes (PGZP container, .pgeo layout) and findings: docs/game-data/fh6-game-files.md.

Two sources, both telemetry space (x/z horizontal metres, y height):

A) Tracks/Brio/Ribbon_00/GameObjs.xml   (327 KB plain XML, EXACT, no decoding needed - this is the main source)
     <Obj GameplayID="SPEEDCAMERA_07_LEFT"><Pos value="x,y,z"/><Orientation>...   793 objs:
     DISCOUNT_BOARD_XP_{A,B,C}_NNN (xp boards, 100/75/25), SPEEDCAMERA_NN_{LEFT,RIGHT} (speed traps, 30),
     SPEEDCAMERAZONE_NN_{LEFT,RIGHT}_{1,2} (speed zones, 30 x 2 gates), TRAILBLAZER_NN_{LEFT,RIGHT}_{1,2} (12 x 2 gates),
     DRIFTZONEMARKER_NN_{LEFT,RIGHT}_{1,2} (20 drift zones x 2 gates), MASCOTS_REGION_R_NNN (200), ESTATE_ENTRANCE_NN (37),
     DISCOUNT_BOARD_TREASURE_CHEST_N (3), BARN_FIND_*, plus 27 ANIM_*/STADIUM_FLOOR objs sitting at 0,0,0 (ignored).
     Gate _1 / _2 = start / end of the zone is NOT verified.

B) Tracks/Brio/GeoChunk0.minizip (PGZP, 39.7 GB; only the needed entries are seek-read) -> .pgeo prop-group files:
   danger signs (ramp + construction board + cones), drift-zone marker posts (1027), plus a cross-check of the pgeo decoder
   against source A (error must be 0.000 m).  See pgzp.py for the container and `parse_pgeo` below for the .pgeo layout:
     u32 len + section name ("c200_props_do_rt_0_discount_board_xp_a_077_cellx2z45_section0")
     u32 0, u32 13, u32 15, f32 bbox_min[3], u32 0, f32 bbox_max[3], f32 1.0, then u32 5 'PROPS'/'SIGNS' block + tag string
     per model: u32 len, name ("gpy_gbl_bonusboard_01_a_3D"), u32 count, count x 80-byte instances:
       3 x u32 position, sign-magnitude 16.16 fixed point (bit31 = sign, value = (u & 0x7fffffff)/65536; x,y,z),
       6 x f32 (right + up unit vectors), 3 x f32 scale (1,1,1), 32 B undecoded.   Then a LOD block with the name minus "_3D".
   Speed traps / speed zones / trailblazers / photo spots do NOT occur in any pgeo (all 96234 grepped) -> source A only.
"""
import argparse, collections, json, math, os, re, struct, sys
import xml.etree.ElementTree as ET
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, ci, resolve_media
from pgzp import Pgzp

ap = argparse.ArgumentParser(description='Extract exact gameplay-prop positions from GameObjs.xml + GeoChunk0 pgeo files')
add_media_args(ap)
ap.add_argument('--skip-pgeo', action='store_true', help='only read Ribbon_00/GameObjs.xml (instant; skips the 40 GB GeoChunk0 file)')
ARGS = ap.parse_args()
MEDIA = resolve_media(ARGS)
TB = ci(MEDIA, 'Tracks/Brio')


# ------------------------------------------------------------------ pgeo
def sm(u):
    v = (u & 0x7fffffff) / 65536.0
    return -v if u & 0x80000000 else v


def parse_pgeo(d, stride=80):
    nl = struct.unpack_from('<I', d, 0)[0]
    p = 4 + nl + 12
    bb = struct.unpack_from('<3f', d, p) + struct.unpack_from('<3f', d, p + 16)
    lo, hi = bb[:3], bb[3:]
    models = []; o = p + 32; n = len(d)
    while o < n - 8:
        L = struct.unpack_from('<I', d, o)[0]
        if 4 <= L <= 120 and o + 8 + L <= n:
            s = d[o + 4:o + 4 + L]
            if re.fullmatch(rb'[A-Za-z0-9_\-\.\[\]]+', s):
                cnt = struct.unpack_from('<I', d, o + 4 + L)[0]; q = o + 8 + L
                if 1 <= cnt <= 100000 and q + 12 <= n:
                    x, y, z = map(sm, struct.unpack_from('<3I', d, q))
                    if lo[0] - 3 <= x <= hi[0] + 3 and lo[1] - 3 <= y <= hi[1] + 3 and lo[2] - 3 <= z <= hi[2] + 3:
                        pts = [tuple(map(sm, struct.unpack_from('<3I', d, q + k * stride))) for k in range(cnt) if q + k * stride + 12 <= n]
                        models.append((s.decode(), pts)); o = q + cnt * stride; continue
        o += 1
    return dict(bbox=bb, models=models)


# ------------------------------------------------------------------ build
OUT = []
def add(type_, name, p, source, **extra):
    r = {'type': type_, 'name': name, 'x': round(p[0], 2), 'y': round(p[1], 2), 'z': round(p[2], 2), 'source': source}
    if extra: r['extra'] = extra
    OUT.append(r)

def mid(a, b): return tuple((a[i] + b[i]) / 2 for i in range(3))
def r2(p): return [round(v, 2) for v in p]


def gameobjs():
    src = 'Tracks/Brio/Ribbon_00/GameObjs.xml'
    objs = {}
    for o in ET.parse(ci(TB, 'Ribbon_00/GameObjs.xml')).getroot().iter('Obj'):
        pos = tuple(map(float, o.find('Pos').get('value').split(',')))
        zx = o.find('Orientation/ZAxis')
        zax = tuple(map(float, zx.get('value').split(','))) if zx is not None else (0, 0, 1)
        objs[o.get('GameplayID')] = (pos, zax)
    P = lambda k: objs[k][0]
    # xp boards
    for k in sorted(objs):
        m = re.fullmatch(r'DISCOUNT_BOARD_XP_([ABC])_(\d+)', k)
        if m: add('xp_board', f'xp_{m[1].lower()}_{m[2]}', P(k), src, variant=m[1], heading_zaxis=r2(objs[k][1]))
        m = re.fullmatch(r'DISCOUNT_BOARD_TREASURE_CHEST_(\d+)', k)
        if m: add('treasure_chest', f'treasure_chest_{m[1]}', P(k), src)
        m = re.fullmatch(r'MASCOTS_REGION_(\d+)_(\d+)', k)
        if m: add('mascot', f'mascot_r{m[1]}_{m[2]}', P(k), src, region=int(m[1]))
        m = re.fullmatch(r'ESTATE_ENTRANCE_(\d+)', k)
        if m: add('estate_entrance', f'estate_entrance_{m[1]}', P(k), src)
    # speed traps: LEFT/RIGHT camera poles either side of the road; position = midpoint
    for n in sorted({re.fullmatch(r'SPEEDCAMERA_(\d+)_LEFT', k)[1] for k in objs if re.fullmatch(r'SPEEDCAMERA_(\d+)_LEFT', k)}):
        l, r = P(f'SPEEDCAMERA_{n}_LEFT'), P(f'SPEEDCAMERA_{n}_RIGHT')
        add('speed_trap', f'speed_trap_{n}', mid(l, r), src, left=r2(l), right=r2(r), width=round(math.dist(l, r), 1))
    # gate pairs: speed zones (SPEEDCAMERAZONE), trailblazers, drift zones (DRIFTZONEMARKER)
    for pref, typ in (('SPEEDCAMERAZONE', 'speed_zone'), ('TRAILBLAZER', 'trailblazer'), ('DRIFTZONEMARKER', 'drift_zone')):
        ids = sorted({re.fullmatch(pref + r'_(\d+)_LEFT_1', k)[1] for k in objs if re.fullmatch(pref + r'_(\d+)_LEFT_1', k)})
        for n in ids:
            g = {}
            for gi in (1, 2):
                l, r = P(f'{pref}_{n}_LEFT_{gi}'), P(f'{pref}_{n}_RIGHT_{gi}')
                g[gi] = mid(l, r)
                add(typ, f'{typ}_{n}_gate{gi}', g[gi], src, zone=int(n), gate=gi, left=r2(l), right=r2(r), width=round(math.dist(l, r), 1))
            OUT[-1]['extra']['length_to_gate1'] = round(math.dist(g[1], g[2]), 1)
    return objs


def geochunk(objs):
    pg = Pgzp.open(MEDIA, 0)
    names = pg.names
    pat = re.compile(r'_rt_0__tag_dangersign_bm_|__tag_dangersign_bm_|tag_drift_zone_|driftzonemarker_|discount_board_|tag_time_attack_drift_circuit')
    sel = [i for i in pg.find(r'\.pgeo$') if pat.search(names[i])]   # pg.find() sorts into on-disk order (fast sequential reads)
    src0 = 'Tracks/Brio/GeoChunk0.minizip'
    db = collections.defaultdict(list)                            # dangersign NN -> [(model, pts, cellpath)]
    cmp = collections.defaultdict(list)
    for r in sel:
        d = pg.entry(r); P = parse_pgeo(d)
        leaf = names[r].split('cellsize\\')[-1]
        src = f'{src0}#{leaf}'
        m = re.search(r'tag_dangersign_bm_(\d+)', leaf)
        if m:
            for model, pts in P['models']: db[m[1]].append((model, pts, src)); 
            continue
        m = re.search(r'tag_drift_zone_(\d+)', leaf)
        if m:
            for model, pts in P['models']:
                for k, p in enumerate(pts): add('drift_zone_post', f'zone_{m[1]}_post', p, src, zone=int(m[1]))
            continue
        m = re.search(r'driftzonemarker_(\d+)_(left|right)_(\d)', leaf)
        if m:
            p = P['models'][0][1][0]
            cmp[f'DRIFTZONEMARKER_{m[1]}_{m[2].upper()}_{m[3]}'].append(p); continue
        m = re.search(r'discount_board_xp_a_(\d+)', leaf)
        if m:
            cmp[f'DISCOUNT_BOARD_XP_A_{m[1]}'].append(P['models'][0][1][0]); continue
        m = re.search(r'discount_board_treasure_chest_(\d+)', leaf)
        if m:
            cmp[f'DISCOUNT_BOARD_TREASURE_CHEST_{int(m[1])}'].append(P['models'][0][1][0]); continue
        m = re.search(r'tag_time_attack_drift_circuit_(\w+?)_?\.pgeo|tag_time_attack_drift_circuit_(\w+?)__cell', leaf)
        pts = [p for _, ps in P['models'] for p in ps]
        c = tuple(float(np.mean([p[i] for p in pts])) for i in range(3))
        kind = re.search(r'circuit_(horizon|grassroutes)', leaf)[1]
        add('drift_circuit_prop', f'{kind}', c, src, instances=len(pts), models=sorted({mm for mm, _ in P['models']}))
    # danger signs: group centre = mean of construction boards (the sign), fallback cones, fallback ramp
    for nn in sorted(db):
        grp = collections.defaultdict(list)
        for model, pts, src in db[nn]: grp[model.replace('_3D', '')] += [(p, src) for p in pts]
        def cen(ps): return tuple(float(np.mean([p[i] for p, _ in ps])) for i in range(3))
        board = next((v for k, v in grp.items() if 'constructionbrd' in k), None)
        ramp = next((v for k, v in grp.items() if 'rush_ramp' in k), None)
        cones = next((v for k, v in grp.items() if 'cone' in k), None)
        main = board or cones or ramp
        add('danger_sign', f'bm_{nn}', cen(main), main[0][1], models={k: len(v) for k, v in grp.items()},
            ramp=[r2(p) for p, _ in (ramp or [])], boards=[r2(p) for p, _ in (board or [])], cones=len(cones or []))
    # --- validation of the pgeo decoder against the exact GameObjs positions
    errs = []
    for k, ps in cmp.items():
        if k in objs:
            for p in ps: errs.append(math.dist(p, objs[k][0]))
    return errs, len(sel)


def validate_cells(path):
    """optional: compare against the old cell-centre records of extract_poi.py's pois.json (if present in --out)"""
    if not os.path.exists(path): return
    old = json.load(open(path)); ok = tot = 0; worst = 0; miss = []
    # old records: type drift_zone / danger_sign / xp_board with centre x,z and extra.cell (cell size S)
    new = collections.defaultdict(list)
    for r in OUT: new[r['type']].append(r)
    for o in old:
        if o['precision'] != 'cell' or o['type'] not in ('drift_zone', 'danger_sign', 'xp_board'): continue
        S = o['extra']['cell']; cx, cz = o['x'], o['z']
        cand = new[o['type']] + (new['drift_zone_post'] if o['type'] == 'drift_zone' else []) + (new['drift_circuit_prop'] if False else [])
        d = min(max(abs(c['x'] - cx), abs(c['z'] - cz)) for c in cand)   # Chebyshev distance to nearest exact point
        tot += 1; ok += d <= S / 2 + 0.5; worst = max(worst, d)
        if d > S / 2 + 0.5: miss.append((o['type'], o['name'], round(d)))
    print(f'cell validation: {ok}/{tot} old cell-centre records have an exact point inside their cell (worst {worst:.0f} m)', miss[:5])




if __name__ == '__main__':
    objs = gameobjs()
    print(f'GameObjs.xml: {len(objs)} objects with a GameplayID -> {len(OUT)} records')
    if not ARGS.skip_pgeo:
        errs, nsel = geochunk(objs)
        errs = np.array(errs)
        print(f'decoded {nsel} pgeo entries; pgeo-vs-GameObjs position error over {len(errs)} matched objects: '
              f'max {errs.max():.3f} m, median {np.median(errs):.3f} m')
        validate_cells(os.path.join(ARGS.out, 'pois.json'))
    out = os.path.join(ARGS.out, 'geochunk_pois.json')
    json.dump(OUT, open(out, 'w'), indent=0)
    print(len(OUT), 'records ->', out)
    print(dict(collections.Counter(r['type'] for r in OUT)))
