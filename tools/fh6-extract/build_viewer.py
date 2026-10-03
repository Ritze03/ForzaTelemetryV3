#!/usr/bin/env python3
"""Build a LOCAL interactive map viewer (Leaflet, static files, opens from file://) of everything the fh6-extract scripts can read
from the user's own Forza Horizon 6 install.  READ-ONLY on the game; the output contains Playground Games' imagery/data, so write it
OUTSIDE the repo and never publish/commit it.

    python3 -B build_viewer.py [--media <...>/ForzaHorizon6/media] [--out ./fh6-viewer] [--work <dir>] [--seasons Spring,Summer,Autumn,Winter]
                               [--force] [--no-terrain] [--jpeg-quality 80]
    then open  <out>/index.html  in a browser (needs internet once per session for the Leaflet CDN scripts).

Self-sufficient: it runs the extractors itself into --work (default <out>-work; re-runs skip steps whose output already exists, --force
redoes them), so a first run takes ~5 min (terrain = 40 GB GeoChunk0 seek-reads; measured 270 s for one season), later runs ~10-20 s.  A layer whose extractor
fails is skipped with a warning; the rest of the viewer is still built.

Output (<out>/):  index.html (from viewer_template.html) · data/*.js (window.FH6.<name> = ..., loaded by <script>, no fetch()) ·
tiles/<Season>/<z>/<x>/<y>.jpg (the game's own tile pyramid, z 0..3 = L0..L3) · icons/*.png (game map icons, <=64 px) ·
overlays/elevation.png (hillshade; surfaces are drawn client-side from a compressed id grid in data/surfaces.js).
Coordinates everywhere: telemetry space (x, z metres).  Map px (8192 map) = ((x+12540)*0.3722, (10738-z)*0.3722); the viewer's Leaflet CRS
uses those 8192-map pixels as units (lat = -py, lng = px).
Needs: numpy, Pillow, scipy (extract_predictions; optional for extract_speedsigns).  Format notes: docs/game-data/.
"""
import argparse, base64, collections, concurrent.futures as cf, glob, json, math, os, re, shutil, subprocess, sys, threading, time, traceback, zlib
import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from fh6common import ci, autodetect_media
from fh6surfaces import SURFACES, PAVED, OFFROAD

T0 = time.time()
LOCK = threading.Lock()
SKIPPED = []          # (layer, reason)
INCLUDED = {}         # layer -> count / note

X0, Z1, SC, SIZE = -12540.0, 10738.0, 0.3722, 8192


def log(*a):
    with LOCK:
        print(f'[{time.time() - T0:6.0f}s]', *a, flush=True)


def warn(layer, why):
    SKIPPED.append((layer, str(why)))
    log(f'WARNING: layer "{layer}" skipped: {why}')


# ---------------------------------------------------------------------------------------------------------------- extractors
def run_script(name, args, log_path, outputs, force):
    """Run tools/fh6-extract/<name> unless all `outputs` already exist.  Returns True on success."""
    if not force and all(os.path.isfile(o) and os.path.getsize(o) > 0 for o in outputs):
        log(f'cached  {name} ({os.path.basename(os.path.dirname(outputs[0])) or "."})')
        return True
    os.makedirs(os.path.dirname(log_path), exist_ok=True)
    cmd = [sys.executable, '-B', os.path.join(HERE, name)] + args
    log('running', name)
    with open(log_path, 'w') as fh:
        r = subprocess.run(cmd, stdout=fh, stderr=subprocess.STDOUT)
    ok = r.returncode == 0 and all(os.path.isfile(o) and os.path.getsize(o) > 0 for o in outputs)
    if not ok:
        tail = open(log_path, errors='replace').read().strip().splitlines()[-3:]
        log(f'FAILED  {name} (exit {r.returncode}): ' + ' | '.join(tail))
    else:
        log(f'done    {name}')
    return ok


def run_extractors(media, work, force, terrain):
    W = lambda *p: os.path.join(work, *p)
    m = ['--media', media]
    res = {}

    def chain(*steps):
        ok = True
        for key, name, extra, out_dir, outs in steps:
            if not ok:
                break
            ok = run_script(name, m + ['--out', out_dir] + extra, W('logs', key + '.log'), [os.path.join(out_dir, o) for o in outs], force)
            res[key] = ok

    def icons_chain():
        d = W('icons')
        if not force and os.path.isfile(os.path.join(d, 'mapping.json')):
            res['icons'] = True
            return
        chain(('icons', 'extract_icons.py', [], d, ['icons.json', 'xml_symbols.json']))
        if res.get('icons'):
            lp = W('logs', 'icon_mapping.log')
            with open(lp, 'w') as fh:
                r = subprocess.run([sys.executable, '-B', os.path.join(HERE, 'build_icon_mapping.py'), d], stdout=fh, stderr=subprocess.STDOUT)
            res['icons'] = r.returncode == 0 and os.path.isfile(os.path.join(d, 'mapping.json'))

    def after_nav():
        # same order as the README "typical run"; poi -> geochunk -> races each validate against the previous output if present
        chain(('poi', 'extract_poi.py', [], work, ['pois.json']),
              ('geochunk', 'extract_geochunk.py', [], work, ['geochunk_pois.json']),
              ('races', 'extract_races.py', [], work, ['races.json']))

    jobs = []
    with cf.ThreadPoolExecutor(max_workers=8) as ex:
        os.makedirs(work, exist_ok=True)
        if terrain:
            os.makedirs(W('terr_e'), exist_ok=True); os.makedirs(W('terr_s'), exist_ok=True)
            # elevation 8 m / surfaces 8 m: the viewer downsamples to <= 2752 px anyway; two processes -> they run in parallel
            jobs.append(ex.submit(chain, ('elevation', 'extract_terrain.py', ['--res', '8', '--no-surfaces'], W('terr_e'), ['elevation.npy', 'elevation.json'])))
            jobs.append(ex.submit(chain, ('surfaces', 'extract_terrain.py', ['--surf-res', '8', '--no-elevation'], W('terr_s'), ['surfaces.npy', 'surfaces.json'])))
        jobs.append(ex.submit(icons_chain))
        jobs.append(ex.submit(chain, ('names', 'extract_names.py', [], work, ['names.json', 'regions.json'])))
        jobs.append(ex.submit(chain, ('racelines', 'extract_racelines.py', [], work, ['racelines.json'])))
        # roads first (extract_speedsigns snaps to roads.json), then the rest of the chains
        rj = W('roads.json')
        if os.path.isfile(rj) and not all(k in open(rj, 'rb').read() for k in (b'"heights"', b'"ids"')):       # roads.json from before node heights / node ids were exported
            os.remove(rj)
        rc = W('races.json')
        if os.path.isfile(rc) and b'"type_exact"' not in open(rc, 'rb').read():       # races.json from before exact race names / types (ObjectModelGame.zip)
            os.remove(rc)
        chain(('roads', 'decode_nav.py', [], work, ['roads.json']))
        if terrain and res.get('roads'):
            jobs.append(ex.submit(chain, ('roadsurf', 'classify_roads.py', [], work, ['roadsurf.npz'])))
        jobs.append(ex.submit(after_nav))
        jobs.append(ex.submit(chain, ('speedsigns', 'extract_speedsigns.py', [], work, ['speedsigns.json'])))
        for j in jobs:
            try:
                j.result()
            except Exception as e:                       # a crashing job must not kill the build
                log('job crashed:', e)
    # predicted race types + map pins: needs races.json, pois.json AND racelines.json, so it runs after everything above; ~20 s
    pj = W('predictions.json')
    ins = [W('races.json'), W('pois.json'), W('racelines.json')]
    if os.path.isfile(pj) and (b'"pin_predicted"' not in open(pj, 'rb').read() or any(os.path.isfile(i) and os.path.getmtime(i) > os.path.getmtime(pj) for i in ins)):
        os.remove(pj)                                     # from before the predictions, or older than its inputs
    if all(os.path.isfile(i) for i in ins):
        chain(('predict', 'extract_predictions.py', [], work, ['predictions.json']))
    else:
        log('predictions skipped: races.json / pois.json / racelines.json missing')
    return res


# ---------------------------------------------------------------------------------------------------------------------- tiles
def _tile_job(a):
    zpath, level, row, col, dst, q = a
    if os.path.isfile(dst):
        return 0
    import zipfile
    from extract_map import parse_swatchbin, decode_bc1
    z = zipfile.ZipFile(zpath)
    w, h, mips, data = parse_swatchbin(z.read(f'{level}-{row}-{col}.swatchbin'))
    img = Image.fromarray(decode_bc1(data, w, h))
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    img.save(dst, 'JPEG', quality=q, optimize=True, progressive=True)
    return 1


def build_tiles(media, out, seasons, q, force):
    d = ci(media, 'UI/Textures/Data_Bound')
    done = []
    for s in seasons:
        try:
            zp = ci(d, f'map_brio_{s}.zip')
        except FileNotFoundError:
            warn(f'tiles {s}', 'Map_Brio zip not found'); continue
        tasks = []
        for lv in range(4):
            for r in range(1 << lv):
                for c in range(1 << lv):
                    dst = os.path.join(out, 'tiles', s, str(lv), str(c), f'{r}.jpg')   # Leaflet {z}/{x}/{y}: x = column, y = row
                    if force and os.path.isfile(dst):
                        os.remove(dst)
                    tasks.append((zp, lv, r, c, dst, q))
        try:
            with cf.ProcessPoolExecutor(max_workers=min(12, os.cpu_count() or 4)) as ex:
                n = sum(ex.map(_tile_job, tasks, chunksize=4))
            log(f'tiles {s}: {len(tasks)} ({n} new)')
            done.append(s)
        except Exception as e:
            warn(f'tiles {s}', e)
    INCLUDED['base map seasons'] = ','.join(done)
    return done


# ------------------------------------------------------------------------------------------------------------------------ data
def write_js(out, name, obj):
    os.makedirs(os.path.join(out, 'data'), exist_ok=True)
    p = os.path.join(out, 'data', name + '.js')
    with open(p, 'w') as fh:
        fh.write(f'(window.FH6=window.FH6||{{}}).{name}=' + json.dumps(obj, separators=(',', ':'), ensure_ascii=True) + ';\n')
    log(f'data/{name}.js  {os.path.getsize(p) / 1e6:.2f} MB')
    return name


def jl(path):
    return json.load(open(path))


def fl(v, n=1):
    return round(float(v), n)


# (group, EN, DE, colour, clustered, on-by-default)   -- labels are ours; keys are the `type` values of the extract scripts
CATS = {
    'race_start': ('races', 'Race start', 'Rennstart', '#ff5a4d', 0, 1),
    'race_finish': ('races', 'Race finish', 'Renn-Ziel', '#d9d9d9', 0, 0),
    'race_pin': ('races', 'Race map pin (activation circle)', 'Rennen-Kartenpin (Aktivierungskreis)', '#ff9f43', 0, 0),
    'touge_event': ('races', 'Touge event', 'Touge-Event', '#c084fc', 0, 1),
    'touge_pin': ('races', 'Touge pin', 'Touge-Pin', '#c084fc', 0, 0),
    'drag_meet': ('races', 'Drag meet', 'Drag-Treffen', '#f472b6', 0, 1),
    'drag_meet_finish': ('races', 'Drag meet finish', 'Drag-Treffen Ziel', '#f9a8d4', 0, 0),
    'horizon_chase': ('races', 'Horizon Chase start', 'Horizon-Chase-Start', '#fb7185', 0, 1),
    'ie_route': ('races', 'Initial experience route', 'Einstiegs-Route', '#94a3b8', 0, 0),
    'rush_event': ('races', 'Horizon Rush', 'Horizon Rush', '#facc15', 0, 1),
    'special_event': ('races', 'Special event (invitational / legend)', 'Sonderevent (Einladung / Legende)', '#facc15', 0, 1),
    'speed_trap': ('stunts', 'Speed trap', 'Radarfalle', '#38bdf8', 0, 0),
    'speed_zone': ('stunts', 'Speed zone gate', 'Geschwindigkeitszone (Tor)', '#38bdf8', 0, 0),
    'trailblazer': ('stunts', 'Trailblazer gate', 'Trailblazer (Tor)', '#34d399', 0, 0),
    'drift_zone': ('stunts', 'Drift zone', 'Driftzone', '#fb923c', 0, 0),
    'drift_zone_post': ('stunts', 'Drift zone marker posts', 'Driftzonen-Pfosten', '#fb923c', 1, 0),
    'danger_sign': ('stunts', 'Danger sign', 'Gefahrenschild', '#f87171', 0, 0),
    'drift_circuit_prop': ('stunts', 'Drift circuit props', 'Drift-Rundkurs-Objekte', '#fdba74', 0, 0),
    'xp_board': ('collect', 'XP board', 'XP-Tafel', '#a3e635', 1, 0),
    'mascot': ('collect', 'Mascot', 'Maskottchen', '#f0abfc', 1, 0),
    'treasure_chest': ('collect', 'Treasure chest', 'Schatztruhe', '#fde047', 0, 0),
    'treasure_chest_board': ('collect', 'Treasure chest board', 'Schatztruhen-Tafel', '#fde047', 0, 0),
    'treasure_chest_current': ('collect', 'Current season treasure chest', 'Schatztruhe der aktuellen Saison', '#fde047', 0, 1),
    'barn_find': ('collect', 'Barn find', 'Scheunenfund', '#fbbf24', 0, 1),
    'barn_find_hint': ('collect', 'Barn find hint area', 'Scheunenfund-Hinweisgebiet', '#d97706', 0, 0),
    'barn_building_cell': ('collect', 'Barn building (approx., cell centre)', 'Scheune (ungefaehr, Zellmitte)', '#b45309', 0, 0),
    'treasure_car': ('collect', 'Treasure car', 'Schatzauto', '#fde047', 0, 0),
    'pinata': ('collect', 'Pinata', 'Pinata', '#f472b6', 1, 0),
    'house': ('places', 'House', 'Haus', '#60a5fa', 0, 1),
    'estate': ('places', 'Estate arrival point', 'Anwesen (Ankunftspunkt)', '#60a5fa', 0, 0),
    'estate_entrance': ('places', 'Estate entrance', 'Anwesen (Eingang)', '#93c5fd', 0, 0),
    'festival_site': ('places', 'Festival site', 'Festivalgelaende', '#f59e0b', 0, 1),
    'fast_travel': ('places', 'Fast travel', 'Schnellreise', '#22d3ee', 0, 1),
    'car_meet': ('places', 'Car meet', 'Auto-Treffen', '#2dd4bf', 0, 1),
    'showcase': ('places', 'Showcase', 'Showcase', '#e879f9', 0, 1),
    'aftermarket_spot': ('places', 'Aftermarket car spot', 'Tuning-Auto (Standort)', '#a78bfa', 0, 0),
    'aftermarket_board': ('places', 'Aftermarket car board', 'Tuning-Auto (Tafel)', '#a78bfa', 0, 0),
    'upsell': ('places', 'Upsell / playlist car', 'Playlist-Auto', '#a78bfa', 0, 0),
    'parking_area': ('places', 'Parking area', 'Parkplatz', '#64748b', 1, 0),
    'eliminator_spawn': ('places', 'Eliminator spawn', 'Eliminator-Spawn', '#ef4444', 1, 0),
    'playground_arena': ('events', 'Playground Games arena', 'Playground-Games-Arena', '#4ade80', 0, 1),
    'hide_seek_arena': ('events', 'Hide & Seek arena', 'Versteckspiel-Arena', '#4ade80', 0, 1),
    'flag_rush_flag': ('events', 'Flag Rush flag', 'Flag-Rush-Flagge', '#4ade80', 0, 0),
    'horizon_story': ('story', 'Horizon Story', 'Horizon-Story', '#fb923c', 0, 1),
    'story_activation': ('story', 'Story activation zone', 'Story-Aktivierungszone', '#fb923c', 0, 0),
    'story_volume': ('story', 'Story volume (mean of start locators)', 'Story-Bereich (Mittel der Startpunkte)', '#fdba74', 0, 0),
    'horizon_job': ('story', 'Horizon Job', 'Horizon-Job', '#38bdf8', 0, 1),
    'job_activation': ('story', 'Job activation zone', 'Job-Aktivierungszone', '#38bdf8', 0, 0),
    'job_volume': ('story', 'Job volume (mean of start locators)', 'Job-Bereich (Mittel der Startpunkte)', '#7dd3fc', 0, 0),
    'creature_zone': ('other', 'Creature zone', 'Tierzone', '#86efac', 0, 0),
    'train_line': ('other', 'Rural train line', 'Regionalbahn-Strecke', '#e2e8f0', 0, 0),
}
GROUPS = [('races', 'Races & events', 'Rennen & Events'), ('stunts', 'Stunts (PR stunts)', 'Stunts (PR-Stunts)'),
          ('collect', 'Collectibles', 'Sammelobjekte'), ('places', 'Places & services', 'Orte & Dienste'),
          ('events', 'Arenas & games', 'Arenen & Spiele'), ('story', 'Horizon Stories & Jobs', 'Horizon-Stories & -Jobs'),
          ('other', 'Other', 'Sonstiges')]
ICON_ALIAS = {'horizon_chase': 'horizon_chase_start', 'treasure_chest_current': 'treasure_chest'}
EXTRA_ICON = {'eliminator_spawn': 'ext/Eliminator/Map/EliminatorActivationMapIcon'}
# categories whose pois.json 'cell' (approximate) records are superseded by the exact geochunk_pois.json ones
SUPERSEDED = {'xp_board', 'drift_zone', 'danger_sign', 'drift_circuit_prop'}
DROP_POI = {'route_node0', 'landmark', 'map_region'}     # route_node0 = lead-in node; landmarks / regions come from names.json


def build_icons(work, out, used_cats):
    """-> ({category: icon key}, {key: relative file}, race variants {circuit, p2p})"""
    d = os.path.join(work, 'icons')
    mp = jl(os.path.join(d, 'mapping.json'))['categories']
    cat_icon, files = {}, {}

    def add(name):
        if not name:
            return None
        src = os.path.join(d, 'png', name + '.png')
        if not os.path.isfile(src):
            return None
        key = name.replace('/', '__')
        dst = os.path.join(out, 'icons', key + '.png')
        if key not in files:
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            im = Image.open(src).convert('RGBA')
            im.thumbnail((64, 64), Image.LANCZOS)
            im.save(dst, optimize=True)
            files[key] = 'icons/' + key + '.png'
        return key

    for c in used_cats:
        m = mp.get(ICON_ALIAS.get(c, c))
        nm = m.get('icon') if m else None
        if c in EXTRA_ICON:
            nm = EXTRA_ICON[c]
        k = add(nm)
        if k:
            cat_icon[c] = {'key': k, 'basis': (m or {}).get('basis', 'ours') if c not in EXTRA_ICON else 'ours'}
    race = {}
    v = mp.get('race_start', {}).get('variants', {})
    race['circuit'] = add(v.get('asphalt_circuit'))
    race['p2p'] = add(v.get('asphalt_p2p'))
    for k, nm in v.items():                    # every variant by its mapping.json name (road editor: the icon per race type)
        race.setdefault(k, add(nm))
    # editor race types without a race-pin icon of their own: the game's own symbols (user's pick, 2026-10-03)
    #   story     = the Horizon Story map marker (the `horizon_story` category icon: the yellow arch the per-story logos sit on)
    #   wristband = the orange wristband from the pause-menu art (mapping.json has no wristband map pin; `icons/MapIcons/wristband_objective` is a green "!" objective shield)
    race['story'] = add((mp.get('horizon_story') or {}).get('icon') or 'atlas/ForteMapIconSheet/pos_story_background')
    race['wristband'] = add('pins/Wristbands/wristband_orange')
    return cat_icon, files, race


def build_pois(work, out):
    P = jl(os.path.join(work, 'pois.json')) if os.path.isfile(os.path.join(work, 'pois.json')) else []
    G = jl(os.path.join(work, 'geochunk_pois.json')) if os.path.isfile(os.path.join(work, 'geochunk_pois.json')) else []
    R = jl(os.path.join(work, 'races.json')) if os.path.isfile(os.path.join(work, 'races.json')) else []
    if not P: warn('POIs (pois.json)', 'missing')
    if not G: warn('GameObjs/GeoChunk POIs (geochunk_pois.json)', 'missing')
    if not R: warn('race starts (races.json)', 'missing')
    geo_types = {g['type'] for g in G}
    PR = jl(os.path.join(work, 'predictions.json')) if os.path.isfile(os.path.join(work, 'predictions.json')) else None
    if PR:
        c = PR['check']
        log(f"predictions: self-check vs {c['marks']} project marks: full pipeline {c['full']}, hand-only {c['hand_only']}, AI family alone {c['ai_family_alone']}; "
            f"misses {[(m[0], m[1], m[2]) for m in c['misses']]}")
        log('  type sources: ' + str(dict(collections.Counter(v['type_source'] for v in PR['routes'].values()))) + '; pin methods: ' +
            str(dict(collections.Counter(v['pin_predicted']['method'] for v in PR['routes'].values()))))
    else:
        warn('predicted race types / pins (predictions.json)', 'missing - the viewer shows only exact data and your marks')
    srcs, sidx = [], {}
    cats, lines = {}, {}
    for rec in P + G + R:
        t = rec['type']
        if t in DROP_POI:
            continue
        if rec.get('precision') == 'cell' and t in SUPERSEDED and t in geo_types:
            continue
        ex = dict(rec.get('extra') or {})
        if PR and t in ('race_start', 'ie_route', 'horizon_chase') and str(ex.get('route_id')) in PR['routes']:
            ex.update({k: v for k, v in PR['routes'][str(ex['route_id'])].items() if k != 'type_exact'})       # PREDICTIONS (type_predicted, type_*, pin_predicted), kept apart from the exact fields
        if rec.get('precision') not in (None, 'exact'):
            ex['precision'] = rec['precision'] + ' (approximate)'
        if t == 'train_line' and 'polyline' in ex:
            lines[t] = [[fl(a), fl(b)] for a, b in ex.pop('polyline')]
        s = rec.get('source', '')
        if s not in sidx:
            sidx[s] = len(srcs); srcs.append(s)
        cats.setdefault(t, []).append([fl(rec['x'], 2), fl(rec['z'], 2), fl(rec['y'], 1) if rec.get('y') is not None else None,
                                       rec.get('name', ''), sidx[s], ex or None])
    # current season's chest = the highest-numbered treasure-chest board over both sources (user observation, 2026-10-03)
    best = None
    for rec in P + G:
        m = re.search(r'TREASURE_CHEST_(\d+)$|^treasure_chest_(\d+)$', rec.get('name', ''), re.I)
        if m and rec['type'] in ('treasure_chest_board', 'treasure_chest') and (best is None or int(m[1] or m[2]) > best[0]):
            best = (int(m[1] or m[2]), rec)
    if best:
        n, rec = best
        cats['treasure_chest_current'] = [[fl(rec['x'], 2), fl(rec['z'], 2), fl(rec['y'], 1) if rec.get('y') is not None else None, '', sidx[rec.get('source', '')], {'board': n}]]
        log(f'current season treasure chest: board {n} at x={rec["x"]} z={rec["z"]}')
    return cats, srcs, lines


def build_meta_cats(cats, cat_icon):
    meta = {}
    for t, rows in cats.items():
        g, en, de, col, cl, on = CATS.get(t, ('other', t.replace('_', ' ').capitalize(), t.replace('_', ' ').capitalize(), '#9ca3af', 0, 0))
        meta[t] = dict(group=g, en=en, de=de, color=col, cluster=bool(cl), on=bool(on), n=len(rows), icon=(cat_icon.get(t) or {}).get('key'),
                       icon_basis=(cat_icon.get(t) or {}).get('basis'))
    return meta


def build_names(work):
    n = jl(os.path.join(work, 'names.json'))
    rg = jl(os.path.join(work, 'regions.json'))
    lm = []
    for l in n['landmarks']:
        lm.append(dict(slug=l['slug'], x=fl(l['x']), z=fl(l['z']), r=fl(l.get('radius', 0)), names=l['names'], src=l.get('source', ''),
                       parent=bool(l.get('name_is_parent'))))
    regs = [dict(id=r['id'], names=r['names'], long=r.get('names_long_form'), outline=[[fl(a), fl(b)] for a, b in r['outline']],
                 km2=r.get('area_km2'), src=r.get('source', '')) for r in rg]
    return dict(languages=n['languages'], landmarks=lm, regions=regs)


ELEV_DM = 30           # road node more than 3.0 m above/below the nearest terrain triangle -> elevated / underground -> unknown
MINRUN_M = 40          # surface runs shorter than this are absorbed by the longer neighbour (flicker, seams between collision squares)
KIND_NAMES = ['paved', 'offroad', 'unknown']
WHY = ['no collision mesh', 'elevated / tunnel', 'water or unidentified id']       # why a sample is unknown


def smooth_runs(code, step, minrun=MINRUN_M):
    """per-sample kind codes (0 paved, 1 off-road, 2 unknown) -> ([[kind, first sample, n samples]], smoothed codes); runs shorter than `minrun`
    metres are merged into the longer neighbour (equal neighbours -> that kind), shortest first."""
    def rle(c):
        ch = np.nonzero(np.diff(c))[0] + 1
        st = np.r_[0, ch]; en = np.r_[ch, len(c)]
        return [[int(c[x]), int(x), int(y - x)] for x, y in zip(st, en)]
    code = np.asarray(code).copy(); runs = rle(code); mn = max(1, int(round(minrun / step)))
    while len(runs) > 1:
        k = min(range(len(runs)), key=lambda j: runs[j][2])
        if runs[k][2] >= mn:
            break
        l = runs[k - 1] if k > 0 else None; r = runs[k + 1] if k + 1 < len(runs) else None
        tgt = (l if (l[0] == r[0] or l[2] >= r[2]) else r) if (l and r) else (l or r)
        code[runs[k][1]:runs[k][1] + runs[k][2]] = tgt[0]
        runs = rle(code)
    return runs, code


def build_roads(work):
    r = jl(os.path.join(work, 'roads.json'))
    out = dict(cls=r['cls'], lines=[[v for p in pl for v in (fl(p[0]), fl(p[1]))] for pl in r['polylines']])
    npz = os.path.join(work, 'roadsurf.npz')
    if not os.path.isfile(npz):
        warn('road surfaces', 'roadsurf.npz missing (classify_roads.py failed or --no-terrain) - roads are coloured by nav class only')
        return out
    d = np.load(npz); step = float(d['step']); off = d['off']
    ids, dy, SX, SZ = d['id'].astype(int), d['dy'].astype(int), d['x'], d['z']      # load once (npz members decompress on every access)
    kind_of = np.full(65536, 2, np.int8)
    for i, (_, _, k) in SURFACES.items():
        kind_of[i] = 0 if k == PAVED else 1 if k == OFFROAD else 2
    tri = ids != 65535
    elev = tri & (np.abs(dy) > ELEV_DM)
    valid = tri & ~elev                                            # a trustworthy surface id under the road
    raw = np.where(valid, kind_of[ids], 2)                         # 0 paved / 1 off-road / 2 unknown
    why = np.where(~tri, 0, np.where(elev, 1, 2))                  # reason for unknown samples
    used = set()
    km = {}                                                        # nav class -> [paved, offroad, unknown] km
    wkm = [0.0, 0.0, 0.0]                                          # unknown km by dominant reason
    roads = []

    def top(v, k):
        u, c = np.unique(v, return_counts=True); o = np.argsort(-c)[:k]
        return [[int(u[j]), int(round(100 * c[j] / max(len(v), 1)))] for j in o]
    for q in range(len(r['cls'])):
        a, b = int(off[q]), int(off[q + 1]); n = b - a
        pl = np.asarray(r['polylines'][q], float); length = float(np.hypot(*np.diff(pl, axis=0).T).sum()); w = length / max(n - 1, 1)
        runs, code = smooth_runs(raw[a:b], step)
        vi = ids[a:b][valid[a:b]]
        used.update(int(i) for i in np.unique(vi))
        sm = [round(100 * float((code == k).mean())) for k in range(3)]
        rr = []
        for kd, s0, ln in runs:
            sl = slice(a + s0, a + s0 + ln); e = min(s0 + ln + 1, n)          # +1: a run ends on the first sample of the next one (no visual gap)
            ix = np.unique(np.r_[np.arange(s0, e, 4), e - 1])
            pts = [fl(v, 1) for j in ix for v in (SX[a + j], SZ[a + j])]
            wy = np.bincount(why[sl][raw[sl] == 2], minlength=3)
            rr.append([kd, int(round(ln * w)), top(ids[sl][valid[sl]], 3), int(wy.argmax()) if kd == 2 and wy.sum() else None, pts])
            km.setdefault(r['cls'][q], [0.0, 0.0, 0.0])[kd] += ln * w / 1000
            if kd == 2 and wy.sum():
                wkm[int(wy.argmax())] += ln * w / 1000
        roads.append([sm, top(vi, 4), rr])
    out['surf'] = dict(kinds=KIND_NAMES, why=WHY, minrun=MINRUN_M, elev_m=ELEV_DM / 10, roads=roads,
                       ids={str(i): (list(SURFACES[i]) if i in SURFACES else ['Unknown id %d' % i, 'unknown', 'other']) for i in sorted(used)})
    log('road surface kinds after smoothing (km): paved / off-road / unknown')
    for c in sorted(km):
        v = km[c]; t_ = sum(v)
        log(f'    nav class {c}: {v[0]:7.1f} / {v[1]:7.1f} / {v[2]:6.1f}   ({t_:.1f} km; {100 * v[0] / t_:.0f}% / {100 * v[1] / t_:.0f}% / {100 * v[2] / t_:.0f}%)')
    tot = [sum(km[c][k] for c in km) for k in range(3)]
    log(f'    all        : {tot[0]:7.1f} / {tot[1]:7.1f} / {tot[2]:6.1f}   ({sum(tot):.1f} km); unknown by dominant reason: ' +
        ', '.join(f'{WHY[k]} {wkm[k]:.1f} km' for k in range(3)))
    return out


def build_roaded(work):
    """Road editor data (data/roaded.js): node ids per polyline vertex + a prefill kind per EDGE (consecutive vertices) from the surface samples.
    Per-edge kind = majority of the (smoothed) 4 m samples that lie on that node-to-node stretch: '0' paved -> Road, '1' off-road -> Offroad, '2' unknown -> not set."""
    r = jl(os.path.join(work, 'roads.json'))
    if 'ids' not in r:
        raise RuntimeError('roads.json has no node ids - re-run decode_nav.py')
    npz = os.path.join(work, 'roadsurf.npz')
    pre = None
    if os.path.isfile(npz):
        d = np.load(npz); step = float(d['step']); off = d['off']
        ids, dy = d['id'].astype(int), d['dy'].astype(int)
        kind_of = np.full(65536, 2, np.int8)
        for i, (_, _, k) in SURFACES.items():
            kind_of[i] = 0 if k == PAVED else 1 if k == OFFROAD else 2
        tri = ids != 65535
        raw = np.where(tri & ~(tri & (np.abs(dy) > ELEV_DM)), kind_of[ids], 2)
        pre, tot = [], [0, 0, 0]
        for q, pl in enumerate(r['polylines']):
            a, b = int(off[q]), int(off[q + 1])
            code = smooth_runs(raw[a:b], step)[1]
            P = np.asarray(pl, float); dd = np.r_[0, np.cumsum(np.hypot(*np.diff(P, axis=0).T))]
            t = np.unique(np.r_[np.arange(0, dd[-1], step), dd[-1]])                      # same sample positions as classify_roads.densify
            assert len(t) == b - a, 'roadsurf.npz does not match roads.json - re-run classify_roads.py'
            lo = np.searchsorted(t, dd[:-1], 'left'); hi = np.searchsorted(t, dd[1:], 'left')
            s = []
            for k in range(len(dd) - 1):
                j = code[lo[k]:hi[k]] if hi[k] > lo[k] else code[[min(len(t) - 1, int(round((dd[k] + dd[k + 1]) / 2 / step)))]]
                kd = int(np.bincount(j, minlength=3).argmax()); s.append(str(kd)); tot[kd] += (dd[k + 1] - dd[k]) / 1000
            pre.append(''.join(s))
        log(f'road editor prefill (km): Road {tot[0]:.1f} / Offroad {tot[1]:.1f} / not set {tot[2]:.1f}')
    else:
        warn('road editor', 'roadsurf.npz missing - the prefill button will have nothing to apply')
    return dict(nav=r['nav'], ids=r['ids'], pre=pre, orphans=r['orphans'])


CANON = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'data', 'fh6-road-types.json')


def build_canon():
    """The project's hand-classified road + race types (tools/fh6-extract/data/fh6-road-types.json, ids only) -> data/canon.js.  The editor starts from it
    when the browser has no saved work of its own; 'Reset to project data' loads it."""
    o = jl(CANON)
    assert o.get('format') == 'fh6-road-types' and o.get('version') == 1, 'unexpected canonical file format'
    return o


def build_racelines(work):
    rl = jl(os.path.join(work, 'racelines.json'))
    out = []
    for r in rl:
        pts = r['points']
        out.append(dict(id=r['route_id'], circuit=bool(r['circuit']), len=r['length_m'], hdg=r.get('start_heading_deg'), hw=r.get('half_width_range'),
                        p=[v for q in pts for v in (fl(q[0]), fl(q[1]))], l=[v for q in r['left'] for v in (fl(q[0]), fl(q[1]))]))
    return out


def build_signs(work):
    s = jl(os.path.join(work, 'speedsigns.json'))
    kinds = {'limit': 0, 'limit_high': 1, 'limit_end': 2}
    return dict(kinds=list(kinds), rows=[[fl(a['x'], 2), fl(a['z'], 2), fl(a['y'], 1), fl(a['heading_deg']), kinds.get(a['kind'], 0), a['variant'],
                                          a.get('limit_kmh'), a.get('road_dist_m'), a.get('road_class'), a.get('model', ''), a.get('cell', '')] for a in s])


# --------------------------------------------------------------------------------------------------------------------- terrain
RAMP = [(100, (52, 92, 138)), (102, (70, 120, 100)), (140, (96, 140, 78)), (250, (150, 170, 86)), (450, (186, 168, 104)), (700, (150, 118, 92)),
        (950, (190, 186, 182)), (1250, (255, 255, 255))]


def hex_rgb(c):
    return tuple(int(c[i:i + 2], 16) for i in (1, 3, 5))


def b64z(a):
    return base64.b64encode(zlib.compress(np.ascontiguousarray(a).tobytes(), 9)).decode('ascii')


def build_elevation(work, out):
    d = os.path.join(work, 'terr_e')
    E = np.load(os.path.join(d, 'elevation.npy')).astype(np.float32)
    ej = jl(os.path.join(d, 'elevation.json'))
    res = ej['res_m']
    ok = np.isfinite(E)
    Ef = np.where(ok, E, 100.0)
    gy, gx = np.gradient(Ef * 2.5, res)                          # 2.5x vertical exaggeration
    az, alt = math.radians(315), math.radians(45)               # light from the north-west
    slope = np.arctan(np.hypot(gx, gy)); aspect = np.arctan2(gy, -gx)
    shade = np.sin(alt) * np.cos(slope) + np.cos(alt) * np.sin(slope) * np.cos(az - aspect)
    shade = np.clip(0.35 + 0.75 * shade, 0.15, 1.2)
    xs = [p[0] for p in RAMP]
    col = np.stack([np.interp(Ef, xs, [p[1][i] for p in RAMP]) for i in range(3)], -1)
    rgb = np.clip(col * shade[..., None], 0, 255).astype(np.uint8)
    img = np.dstack([rgb, np.where(ok, 255, 0).astype(np.uint8)])
    os.makedirs(os.path.join(out, 'overlays'), exist_ok=True)
    dst = os.path.join(out, 'overlays', 'elevation.png')
    Image.fromarray(img, 'RGBA').save(dst, optimize=True)
    log(f'overlays/elevation.png {os.path.getsize(dst) / 1e6:.1f} MB {img.shape[1]}x{img.shape[0]}')
    q = E[::2, ::2]                                              # hover lookup grid: 2x coarser, int16 decimetres, -32768 = no data
    qi = np.where(np.isfinite(q), np.round(q * 10), -32768).astype('<i2')
    return dict(img=dict(file='overlays/elevation.png', x0=ej['x0'], z1=ej['z1'], res=res, w=ej['width'], h=ej['height']),
                grid=dict(x0=ej['x0'], z1=ej['z1'], res=res * 2, w=qi.shape[1], h=qi.shape[0], data=b64z(qi)),
                zmin=float(np.nanmin(E)), zmax=float(np.nanmax(E)), ramp=[[a, list(b)] for a, b in RAMP])


def build_surfaces(work):
    d = os.path.join(work, 'terr_s')
    S = np.load(os.path.join(d, 'surfaces.npy'))
    sj = jl(os.path.join(d, 'surfaces.json'))
    ids = [int(i) for i in np.unique(S) if i != 0xFFFF]
    counts = {i: int((S == i).sum()) for i in ids}
    ids.sort(key=lambda i: -counts[i])
    try:
        from extract_terrain import CLASSES
    except Exception:
        CLASSES = []
    base = {}
    for name, c, cids, _ in CLASSES:
        for k, i in enumerate(cids):
            base[i] = (c, k, len(cids))
    import colorsys
    colors = []
    for n, i in enumerate(ids):
        c, k, m = base.get(i, ((200, 60, 160), n, 6))
        h, s, v = colorsys.rgb_to_hsv(*[x / 255 for x in c])
        h = (h + (k - m / 2) * 0.018) % 1.0
        v = min(1.0, max(0.35, v * (0.78 + 0.22 * ((k * 5) % 3) / 2)))
        s = min(1.0, s * (0.85 + 0.15 * (k % 2)))
        colors.append([int(round(x * 255)) for x in colorsys.hsv_to_rgb(h, s, v)])
    lut = np.full(65536, 255, np.uint8)
    for n, i in enumerate(ids):
        lut[i] = n
    grid = lut[S]
    res = sj['res_m']
    names = sj.get('names', {})
    px_km2 = res * res / 1e6
    return dict(x0=sj['x0'], z1=sj['z1'], res=res, w=sj['width'], h=sj['height'], ids=ids,
                names=[(names.get(str(i)) or {}).get('name', f'Unknown id {i}') for i in ids],
                status=[(names.get(str(i)) or {}).get('status', 'unknown') for i in ids],
                colors=colors, km2=[round(counts[i] * px_km2, 2) for i in ids], data=b64z(grid))


# -------------------------------------------------------------------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--media', help='<ForzaHorizon6>/media (default: auto-detect via Steam)')
    ap.add_argument('--out', default='./fh6-viewer', help='output dir (default ./fh6-viewer) - keep it OUTSIDE the repo and do not publish it')
    ap.add_argument('--work', help='extractor outputs + logs (default <out>-work); re-used between runs')
    ap.add_argument('--seasons', default='Spring,Summer,Autumn,Winter')
    ap.add_argument('--jpeg-quality', type=int, default=80)
    ap.add_argument('--force', action='store_true', help='re-run extractors and re-encode tiles even if output exists')
    ap.add_argument('--no-terrain', action='store_true', help='skip elevation + surfaces (the slow part)')
    a = ap.parse_args()
    media = a.media or autodetect_media()
    if not media or not os.path.isdir(media):
        sys.exit('FH6 media dir not found. Pass --media /path/to/steamapps/common/ForzaHorizon6/media')
    out = os.path.abspath(a.out)
    work = os.path.abspath(a.work or out.rstrip('/') + '-work')
    repo = os.path.abspath(os.path.join(HERE, '..', '..'))
    for p in (out, work):
        if os.path.commonpath([p, repo]) == repo:
            sys.exit(f'refusing to write game data inside the repo ({p}); choose --out/--work outside {repo}')
    os.makedirs(out, exist_ok=True)
    log('media:', media); log('out  :', out); log('work :', work)

    res = run_extractors(media, work, a.force, not a.no_terrain)
    seasons = build_tiles(media, out, [s for s in a.seasons.split(',') if s], a.jpeg_quality, a.force)
    W = lambda *p: os.path.join(work, *p)
    scripts, meta = [], dict(calib=dict(x0=X0, z1=Z1, s=SC, size=SIZE), seasons=seasons, groups=GROUPS, built=time.strftime('%Y-%m-%d %H:%M'))

    def layer(name, fn, count=lambda o: len(o)):
        try:
            o = fn()
            INCLUDED[name] = count(o)
            return o
        except Exception as e:
            warn(name, f'{type(e).__name__}: {e}')
            log(traceback.format_exc().strip().splitlines()[-3])
            return None

    # POIs + icons
    pois = layer('POIs / races / stunts', lambda: build_pois(W(), out), lambda o: sum(len(v) for v in o[0].values()))
    if pois:
        cats, srcs, lines = pois
        ci_, files, race_icons = {}, {}, {}
        if res.get('icons') or os.path.isfile(W('icons', 'mapping.json')):
            r = layer('map icons', lambda: build_icons(W(), out, list(cats)), lambda o: len(o[1]))
            if r:
                ci_, files, race_icons = r
        else:
            warn('map icons', 'extract_icons failed - POIs use coloured circles')
        meta['cats'] = build_meta_cats(cats, ci_)
        meta['icons'] = files
        meta['race_icons'] = race_icons
        scripts.append(write_js(out, 'poi', dict(cats=cats, sources=srcs, lines=lines)))
        INCLUDED['POI categories'] = ', '.join(f'{k}:{len(v)}' for k, v in sorted(cats.items(), key=lambda kv: -len(kv[1])))
    nm = layer('landmarks + regions', lambda: build_names(W()), lambda o: f"{len(o['landmarks'])} landmarks, {len(o['regions'])} regions")
    if nm: scripts.append(write_js(out, 'names', nm))
    rd_ = layer('roads', lambda: build_roads(W()), lambda o: f"{len(o['lines'])} polylines" + (', surface kinds for all' if 'surf' in o else ', no surface data'))
    if rd_: scripts.append(write_js(out, 'roads', rd_))
    re_ = layer('road editor', lambda: build_roaded(W()), lambda o: f"{sum(len(q) - 1 for q in o['ids'])} edges" + (', prefill' if o['pre'] else ', no prefill'))
    if re_: scripts.append(write_js(out, 'roaded', re_))
    cn = layer('project road/race types', build_canon, lambda o: f"{len(o['types'])} painted edges, {len(o['added'])} added links, {len(o['races'])} race marks")
    if cn: scripts.append(write_js(out, 'canon', cn))
    rl = layer('race lines', lambda: build_racelines(W()), lambda o: f'{len(o)} routes, {sum(len(r["p"]) // 2 for r in o)} points')
    if rl: scripts.append(write_js(out, 'racelines', rl))
    sg = layer('speed signs', lambda: build_signs(W()), lambda o: len(o['rows']))
    if sg: scripts.append(write_js(out, 'signs', sg))
    if not a.no_terrain:
        el = layer('elevation overlay', lambda: build_elevation(W(), out), lambda o: f"{o['img']['w']}x{o['img']['h']} px, {o['zmin']:.0f}..{o['zmax']:.0f} m")
        if el: scripts.append(write_js(out, 'elevation', el))
        sf = layer('surfaces overlay', lambda: build_surfaces(W()), lambda o: f"{len(o['ids'])} ids, {o['w']}x{o['h']} px")
        if sf: scripts.append(write_js(out, 'surfaces', sf))
    scripts.insert(0, write_js(out, 'meta', meta))

    tpl = open(os.path.join(HERE, 'viewer_template.html'), encoding='utf-8').read()
    tags = '\n'.join(f'<script charset="utf-8" src="data/{s}.js"></script>' for s in scripts)
    open(os.path.join(out, 'index.html'), 'w', encoding='utf-8').write(tpl.replace('<!--@DATA_SCRIPTS@-->', tags))

    total = sum(os.path.getsize(os.path.join(dp, f)) for dp, _, fs in os.walk(out) for f in fs)
    log('=' * 60)
    log(f'viewer: {os.path.join(out, "index.html")}   total {total / 1e6:.1f} MB   build {time.time() - T0:.0f} s')
    for k, v in INCLUDED.items():
        log(f'  included  {k}: {v}')
    for k, v in SKIPPED:
        log(f'  SKIPPED   {k}: {v}')


if __name__ == '__main__':
    main()
