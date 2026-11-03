#!/usr/bin/env python3
"""FH6 area / landmark / map-region names in every shipped language + region outlines.  READ-ONLY on the install.

Outputs (in --out): names.json (landmarks + regions + unplaced landmark strings + stunt-name tables), regions.json (outlines).
Result on the Sept-2026 build: 24 languages; landmarks 75/75 named (54 via IDS_Area_Discovered_<slug>, 17 via ALT_ID, 2 by name/position,
and 2 of the 75 borrow their parent's name - flagged `name_is_parent`); 10 regions with outlines (no self-intersections, tile the island).
Output is Playground Games text/data: write it outside the repo.  Needs numpy.  Usage: extract_names.py [--media ...] [--out DIR]
"""
import argparse, collections, json, math, os, re, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, ci, resolve_media, rd, locators, tzones
from fh6str import StringTables, strhash

ap = argparse.ArgumentParser(); add_media_args(ap); A = ap.parse_args()
MEDIA = resolve_media(A); OUT = A.out
TB = ci(MEDIA, 'Tracks/Brio')

langs = sorted(f[:-4] for f in os.listdir(ci(MEDIA, 'Stripped/StringTables')) if f.endswith('.zip'))
ST = {l: StringTables(MEDIA, l) for l in langs}
TAB = {}   # (lang, table) -> {key: text}
def tab(l, t):
    if (l, t) not in TAB: TAB[(l, t)] = dict(ST[l].table(t))
    return TAB[(l, t)]
def names_for_key(table, key):
    return {l: tab(l, table)[key] for l in langs if key in tab(l, table)}

# ---------------------------------------------------------------- landmarks (75 trigger zones)
# Landmarks.str key = strhash('IDS_Area_Discovered_<id>').  <id> == tz slug for 54/75; for 17 more the id differs (ALT_ID)
# (found by hash search over word combinations / the map-profile landmark type names, each verified by exact hash):
ALT_ID = {
    'arashiyama_bamboo_forest': 'bamboo_forest', 'cedar_lane': 'cedar_avenue', 'daikoku_parking_lot': 'daikoku_parking_area',
    'ginkgo_avenue': 'gingko_avenue', 'golf_course': 'naruo_golf_course', 'hotel_area_06': 'hotel',
    'inakadate_tanbo_rice_art': 'inakadate_rice_art', 'kinkaku_ji_temple': 'kinkakuji_temple', 'meoto_iwa': 'meoto_iwa_rock',
    'narai_juku_hot_springs': 'hot_springs', 'okuibuki_parking_lot': 'okuibuki_parking_area', 'oyashirazu_cliff_tunnels': 'cliff_tunnels',
    'ruriko_ji_temple': 'rurikoji_temple', 'shinjuku_national_garden': 'shinjuku_gyoen_national_garden',
    'tatsumi_parking_lot': 'tatsumi_parking_area', 'temple_of_nachi_falls': 'nachi_falls', 'yahikoyama_panorama_ropeway': 'yahikoyama',
}
# no hash-derivable id: matched by name/position instead (the string exists in Landmarks.str; its exact id is unknown)
NAME_MATCH = {   # slug -> (English text, why)
    'tokyo_railway_station': ('Tokyo Railway Station', 'slug == English text'),
    'mountain_circuit': ('Soni Circuit', 'tz at (2746,4938); Soni Circuit race start at (2788,4991)'),
}
# sub-zones with no string of their own: label them with the parent landmark (flagged)
PARENT = {'bandai_azuma_snow_corridor': 'bandai_azuma_skyline', 'festival_site_parking_lot': 'festival_site'}

LM_EN = tab('EN', 'Landmarks')
by_text = {}
for k, s in LM_EN.items(): by_text.setdefault(s, k)
lm_tz = tzones(ci(TB, 'triggerzones/tz_world_constraints/landmark_triggers.tz'))
LM_SRC = 'Tracks/Brio/triggerzones/tz_world_constraints/landmark_triggers.tz'
records, used_keys, key_of = [], set(), {}
for _, slug, x, y, z, sx, sz in lm_tz:
    key = src = None
    for cand, note in ((slug, 'IDS_Area_Discovered_' + slug), (ALT_ID.get(slug), 'IDS_Area_Discovered_' + str(ALT_ID.get(slug)))):
        if cand and strhash('IDS_Area_Discovered_' + cand) in LM_EN:
            key, src = strhash('IDS_Area_Discovered_' + cand), 'Landmarks.' + note; break
    if key is None and slug in NAME_MATCH:
        key, src = by_text[NAME_MATCH[slug][0]], f'Landmarks.str text match ({NAME_MATCH[slug][1]}); id unknown'
    if key is not None:
        used_keys.add(key); key_of[slug] = key
    records.append(dict(slug=slug, kind='landmark', x=round(x, 1), z=round(z, 1), radius=sx, key=key, source=src, _tz=LM_SRC))
for r in records:
    if r['key'] is None and r['slug'] in PARENT:
        r['key'] = key_of[PARENT[r['slug']]]; r['source'] = f"parent landmark '{PARENT[r['slug']]}' (no own string)"; r['name_is_parent'] = True
for r in records:
    r['names'] = names_for_key('Landmarks', r['key']) if r['key'] is not None else {}
    r['source'] = (r['source'] or 'UNRESOLVED') + ' ; position: ' + r.pop('_tz')
    r['key'] = f"0x{r['key']:08x}" if r['key'] is not None else None

# Landmarks.str strings that belong to no tz slug (extra map landmarks without a trigger position) + UI strings
unplaced = []
for k, s in LM_EN.items():
    if k in used_keys: continue
    unplaced.append(dict(key=f'0x{k:08x}', kind='landmark_unplaced' if not re.search(r'[{}]|^VIEW$|Discovered|BEAUTY', s) else 'ui_string',
                         names=names_for_key('Landmarks', k)))

# ---------------------------------------------------------------- map regions
SLUGS = ['canyon', 'city', 'east_coast', 'festival', 'highlands', 'legend_island', 'north_plains', 'snowy_mountains', 'south_coast', 'south_plains']
# slug -> English short name in MapRegion.str.  Evidence (all verified below): the 9 regions' mascots
# (GameObjs MASCOTS_REGION_<n>_*, polygon membership 100%) + the food named in ChallengeData ("Smash a Ramen Mascot in the Ohtani Region"):
# 1 ramen=Ohtani=festival, 2 =city(Tokyo City), 3 omurice=Sotoyama=snowy_mountains, 4 curry rice=Shimanoyama=canyon, 5 matcha=Takashiro=highlands,
# 6 kakigori=Hokubu=north_plains, 7 edamame=Minamino=south_plains, 8 =east_coast(Ito; Ito Airfield lies in it), 9 tempura=Nangan=south_coast.
REGION_NAME = dict(canyon='Shimanoyama', city='Tokyo City', east_coast='Ito', festival='Ohtani', highlands='Takashiro',
                   legend_island='Legend Island', north_plains='Hokubu', snowy_mountains='Sotoyama', south_coast='Nangan', south_plains='Minamino')
MASCOT_REGION = {1: 'festival', 2: 'city', 3: 'snowy_mountains', 4: 'canyon', 5: 'highlands', 6: 'north_plains', 7: 'south_plains', 8: 'east_coast', 9: 'south_coast'}
MR_EN = tab('EN', 'MapRegion')
def mr_keys(text):   # all MapRegion keys whose English text == text
    return [k for k, s in MR_EN.items() if s == text]

def pip(p, poly):
    x, z = p; c = False; n = len(poly)
    for i in range(n):
        x1, z1 = poly[i]; x2, z2 = poly[(i + 1) % n]
        if (z1 > z) != (z2 > z) and x < (x2 - x1) * (z - z1) / (z2 - z1) + x1: c = not c
    return c
def selfint(p):
    def o(a, b, c): return (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    n, bad = len(p), 0
    for i in range(n):
        for j in range(i + 2, n):
            if i == 0 and j == n - 1: continue
            a, b, c, d = p[i], p[(i + 1) % n], p[j], p[(j + 1) % n]
            if o(a, b, c) * o(a, b, d) < 0 and o(c, d, a) * o(c, d, b) < 0: bad += 1
    return bad

regions, regions_out = [], []
POLY = {}
for slug in SLUGS:
    src = f'Tracks/Brio/trackroutes/map_region_{slug}.nt'
    L = sorted(((n, x, z) for n, x, y, z in locators(ci(TB, f'trackroutes/map_region_{slug}.nt'))), key=lambda t: int(t[0].split('_')[1]))   # Arena_NNN index order
    pts = [(round(x, 1), round(z, 1)) for _, x, z in L]
    if len(pts) > 2 and pts[0] == pts[-1]: pts = pts[:-1]   # north_plains repeats its first point as the last
    POLY[slug] = np.array(pts)
for slug in SLUGS:
    pts = POLY[slug]; en = REGION_NAME[slug]
    # MapRegion.str layout: first 11 entries = long form ("Ito Region"/"Region Ito"; Tokyo City, Legend Island have no suffix), last 9 + 2nd Legend Island = short form
    order = list(MR_EN.items())
    short_k = [k for k, s in order[10:] if s == en] or [order[10][0]]
    long_k = [k for k, s in order[:10] if s in (en + ' Region', en)]
    ks = long_k + short_k
    first = lambda k: {l: tab(l, 'MapRegion')[k] for l in langs if k in tab(l, 'MapRegion')}
    names, names_region = first(short_k[0]), first(long_k[0])
    area = abs(sum(pts[i][0] * pts[(i + 1) % len(pts)][1] - pts[(i + 1) % len(pts)][0] * pts[i][1] for i in range(len(pts)))) / 2
    c = pts.mean(0)
    regions_out.append(dict(id=slug, names=names, names_long_form=names_region, keys=[f'0x{k:08x}' for k in ks],
                            outline=[[float(a), float(b)] for a, b in pts], n_points=len(pts), area_km2=round(area / 1e6, 2),
                            self_intersections=selfint(pts), source=f'Tracks/Brio/trackroutes/map_region_{slug}.nt (Arena_NNN locators, index order)'))
    regions.append(dict(slug=slug, kind='map_region', x=round(float(c[0]), 1), z=round(float(c[1]), 1), names=names, names_long_form=names_region,
                        source='MapRegion.str (EN-text match; id unknown) ; position: centroid of outline'))

# verification of slug -> name via mascots
mt = rd(ci(TB, 'Ribbon_00/GameObjs.xml')); chk = collections.defaultdict(collections.Counter)
for k, _, x, z in [(a, 0, b, d) for a, b, _, d in re.findall(r'<Obj GameplayID="MASCOTS_REGION_(\d+)_\d+">\s*<Pos value="([-\d.eE]+),([-\d.eE]+),([-\d.eE]+)"', mt)]:
    hit = next((s for s in SLUGS if pip((float(x), float(z)), POLY[s])), None); chk[int(k)][hit] += 1
mascot_ok = all(set(chk[n]) == {s} for n, s in MASCOT_REGION.items())
print('mascot-region verification:', {n: dict(c) for n, c in sorted(chk.items())}, '=> consistent' if mascot_ok else '=> MISMATCH')

# ---------------------------------------------------------------- stunt / road name tables (no position link in the files -> names only)
STUNT = {}
for t in ('SpeedZone', 'SpeedTrap', 'DriftZone', 'DangerSign', 'Trailblazer', 'TimeAttack', 'DriftAttack'):
    en = tab('EN', t)
    STUNT[t] = [dict(key=f'0x{k:08x}', names=names_for_key(t, k)) for k in en]

json.dump(dict(languages=langs, landmarks=records, regions=regions, landmarks_unplaced=unplaced, stunt_name_tables=STUNT,
               notes=dict(mascot_region_check='consistent' if mascot_ok else 'MISMATCH',
                          seaside_circuit_swap='NOT swapped: seaside_circuit (2606,2805) = Hokubu Circuit race start (2827,2696); seaside_offroad_circuit (2676,-5095) = Sekibe Scramble (2496,-5065)')),
          open(os.path.join(OUT, 'names.json'), 'w'), ensure_ascii=False, indent=1)
json.dump(regions_out, open(os.path.join(OUT, 'regions.json'), 'w'), ensure_ascii=False, indent=1)
res = [r for r in records if r['names']]
print(f'langs={len(langs)} landmarks resolved {len(res)}/{len(records)}; unresolved:', [r['slug'] for r in records if not r['names']],
      '; parent-labelled:', [r['slug'] for r in records if r.get('name_is_parent')], '; regions', len(regions_out), '; unplaced', len(unplaced))
