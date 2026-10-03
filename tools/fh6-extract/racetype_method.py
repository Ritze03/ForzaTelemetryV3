#!/usr/bin/env python3
"""FH6 race-TYPE prediction for every route.   READ-ONLY on the install.   Method + validation numbers: docs/game-data/fh6-game-files.md
("Predicting race type and map-pin position").   Driven by extract_predictions.py; this module only holds the method.

An ORDERED list of sources, the first that answers wins (most reliable first):
    exact       fh6careers.race_types  (CareerRaceDataSet UITheme / UseCrossCountryAI / StreetRace flyer)
    event       HorizonStoryChallengeData.RouteId -> story ; CareerRaceDataSet EventType=Showcase -> wristband (analogy to the marked 8004)
    ai_family   DifficultyLevel `Route<N>_<level>` (ObjectModelGame.zip): which AI-driver family the route uses (Dirt_/Cross_/Street_/Road_/Drag_/Touge_).
                Fingerprint = the (RaceTrack, RaceStart) object guids per difficulty level vs the game's generic `<Family>_<level>` levels.  A GAME FIELD, nothing fitted.
    not_a_race  Initial-Experience tutorial drives, Horizon Chase, test / off-map routes
    copy        a route >= 30000 whose racing line lies on another route's line (>= 95 % within 3 m) is an event copy of it -> story
    line        fitted ordered rules on the racing line (surface under the line, AI-line dirt tag, circuit / length); paved point-to-point routes are then
                split road / street / touge by the route-id thousands digit -> source `id_convention` (a naming convention, NOT a game field)
Needs numpy, scipy.  Imports the repo's own readers (fh6common, fh6owt, fh6bxml, fh6careers, pgzp, classify_roads, extract_racelines).
`sample_line_surfaces` uses a process pool: call it from under `if __name__ == '__main__':`.
"""
import collections, glob, math, os, re, sys, zipfile
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

# ------------------------------------------------------------------------------------------------------------ AI family (ObjectModelGame.zip)
FAMILIES = ('Dirt', 'Cross', 'Touge', 'Drag', 'Street', 'Road')          # tie-break order; Street/Road differ in ONE level (Unbeatable)
FAMILY_TYPE = {'Dirt': 'rally', 'Cross': 'cross_country', 'Street': 'street', 'Road': 'road', 'Drag': 'drag', 'Touge': 'touge'}
FINGERPRINT_SLOTS = ('RaceTrack', 'RaceStart')


def _attr(node, k):
    return dict(node[1]).get(k)


def _objects(media, type_name):
    """yield the root bxml object of every ObjectModelGame.zip `.om.xml` of that type (cheap byte pre-filter on the string table)"""
    from fh6bxml import bxml_decode
    from fh6common import ci
    z = zipfile.ZipFile(ci(media, 'ObjectModelGame.zip'))
    for n in z.namelist():
        if n.endswith('.om.xml'):
            d = z.read(n)
            if type_name.encode() in d:
                obj = bxml_decode(d)[2][0]
                if _attr(obj, 'type') == type_name:
                    yield obj


def difficulty_levels(media):
    """-> {DifficultyName: {slot: object guid}} for every DifficultyLevel object (613 in the Oct-2026 build; 0.2 s)."""
    out = {}
    for obj in _objects(media, 'DifficultyLevel'):
        name, slots = None, {}
        for p in obj[2]:
            if p[0] != 'property':
                continue
            pid = _attr(p, 'id')
            if pid == 'DifficultyName':
                name = _attr(p, 'value'); continue
            for q in p[2]:                              # <property id=Slot><property LoadOnCreation/><property id=Id><Domain/><Object value=guid/>
                if _attr(q, 'id') == 'Id':
                    for r in q[2]:
                        if _attr(r, 'id') == 'Object':
                            slots[pid] = _attr(r, 'value')
        out[name] = slots
    return out


def ai_families(media):
    """-> {route id: {family, type, score, levels, runner_up, margin, scores}}.  A family scores 1 per `Route<N>_<level>` level whose (RaceTrack, RaceStart)
    guids equal those of the generic `<Family>_<level>`; best wins, margin = best - second (Road vs Street: 1 level, every other pair >= 4)."""
    gen, routes = {f: {} for f in FAMILIES}, collections.defaultdict(dict)
    for n, s in difficulty_levels(media).items():
        m = re.match(r'(Dirt|Cross|Street|Road|Drag|Touge)_(\w+)$', n or '')
        if m:
            gen[m.group(1)][m.group(2)] = s
        m = re.match(r'Route(\d+)_(\w+)$', n or '')
        if m:
            routes[int(m.group(1))][m.group(2)] = s
    out = {}
    for rid, lv in sorted(routes.items()):
        sc = {f: sum(all(s.get(k) == g.get(k) for k in FINGERPRINT_SLOTS) for lvl, s in lv.items() for g in [gen[f].get(lvl)] if g) for f in FAMILIES}
        order = sorted(FAMILIES, key=lambda f: (-sc[f], FAMILIES.index(f)))
        best, second = order[0], order[1]
        out[rid] = (dict(family=best, type=FAMILY_TYPE[best], score=sc[best], levels=len(lv), runner_up=second, margin=sc[best] - sc[second], scores=sc) if sc[best]
                    else dict(family=None, type=None, score=0, levels=len(lv), runner_up=None, margin=0, scores=sc))
    return out


def story_routes(media):
    """-> {route id: challenge Type} from HorizonStoryChallengeData.RouteId (RouteId 0 = the Horizon Jobs, skipped)"""
    out = {}
    for obj in _objects(media, 'HorizonStoryChallengeData'):
        pr = {_attr(p, 'id'): _attr(p, 'value') for p in obj[2] if p[0] == 'property'}
        if int(pr.get('RouteId') or 0):
            out[int(pr['RouteId'])] = pr.get('Type')
    return out


def showcase_routes(media):
    """-> {route id: UITheme} for CareerRaceDataSet entries with EventType == 'Showcase' (8004 showcase_mech, 8005 showcase_planes)."""
    import fh6careers as fc
    from fh6common import ci
    z = zipfile.ZipFile(ci(media, 'ObjectModelGame.zip'))
    cr = fc._table(z, fc.OM_CAREERRACE, 'Data')
    by = {rid: int(dict(v[2][0][1])['value']) for rid, v in fc._maps(z, fc.OM_TRACKINFO)['InfoByRouteId']}
    return {rid: (cr[k].get('UITheme') or '') for rid, k in by.items() if k in cr and cr[k].get('EventType') == 'Showcase'}


# ------------------------------------------------------------------------------------------------------------ line features
DIRT_TAGS = range(272, 275)        # .owt node tag[0] 272-274: the AI-line "dirt" class (UNDECODED field; empirical: 60 % of rally lines, 0-3 % of road/street/touge)


def owt_tag_shares(media):
    """-> {route id: {t_0, t_dirt, tags_all_zero...}}: share (by length) of the per-node tag[0] along the TRIMMED racing line."""
    from fh6common import ci
    from fh6owt import read_owt, parse_rvan
    from extract_racelines import trim
    ait = ci(media, 'OpenWorld/Brio/AITracks')
    out = {}
    for f in glob.glob(os.path.join(ait, 'Route*.owt')):
        rid = int(re.findall(r'\d+', os.path.basename(f))[0])
        a, h = read_owt(f)
        rv = parse_rvan(os.path.join(ait, f'Route{rid}.nav'))
        p = a['p'].astype(float); A = a['a'].astype(float)
        idx, _, _ = trim(p, h, np.array(rv['B']), math.dist(rv['A'], rv['B']) < 1.0)
        idx = idx[np.isfinite(p[idx]).all(1) & np.isfinite(A[idx]).all(1)]
        t = a['tag'][idx][:, 0].astype(int); pp = p[idx]
        seg = np.hypot(*np.diff(pp[:, [0, 2]], axis=0).T); w = np.r_[seg, 0] / 2 + np.r_[0, seg] / 2
        W = w.sum() or 1.0
        out[rid] = dict(t_0=float(w[t == 0].sum() / W), t_dirt=float(w[np.isin(t, list(DIRT_TAGS))].sum() / W))
    return out


def sample_line_surfaces(media, racelines, jobs=12):
    """Exact terrain `.phys` triangle id under every racing-line point (5 m spacing), height-matched like classify_roads.  -> (off, id, dy) arrays."""
    from concurrent.futures import ProcessPoolExecutor
    from pgzp import Pgzp
    from extract_terrain import cell_of
    import classify_roads as cr
    xs, zs, ys, off = [], [], [], [0]
    for r in racelines:
        p = np.array(r['points']); xs.append(p[:, 0]); zs.append(p[:, 1]); ys.append(np.array(r['y'])); off.append(off[-1] + len(p))
    X, Z, Y = [np.concatenate(v).astype(np.float64) for v in (xs, zs, ys)]
    pg = Pgzp.open(media, 0)
    cx = np.floor(X / 512).astype(int) * 512; cz = np.floor(Z / 512).astype(int) * 512
    want = set()
    for a, b in set(zip(cx.tolist(), cz.tolist())):
        want |= {(a + i, b + j) for i in (-512, 0, 512) for j in (-512, 0, 512)}
    rx = re.compile(cr.PAT); sel = []
    for i in pg.find(cr.PAT):
        m = rx.search(pg.names[i])
        if cell_of(int(m.group(1)), int(m.group(2))) in want:
            sel.append(i)
    pg.close()
    chunks = [sel[k::jobs] for k in range(jobs)]
    with ProcessPoolExecutor(jobs) as ex:
        res = list(ex.map(cr.work, [(media, c, X, Z, Y) for c in chunks]))
    bid = np.full(len(X), cr.NONE_ID, np.uint16); bdy = np.full(len(X), cr.NONE_DY, np.int32)
    for i_, d_, _n in res:
        b = (np.abs(d_) < np.abs(bdy)) & (d_ != cr.NONE_DY); bid[b] = i_[b]; bdy[b] = d_[b]
    return np.array(off), bid, bdy


def surface_shares(racelines, off, ids, dy, max_dy_dm=150):
    """-> {route id: {s_off, s_none}}: share of line points whose terrain triangle (within 15 m of the line height) is off-road / missing
    (kind table: fh6surfaces.SURFACES; water and unknown ids count as neither)."""
    from fh6surfaces import SURFACES
    kind = np.full(65536, 3, np.int8)
    for i, (_, _, k) in SURFACES.items():
        kind[i] = {'paved': 0, 'offroad': 1, 'water': 2, 'other': 3}[k]
    out = {}
    for q, r in enumerate(racelines):
        sid, d = ids[off[q]:off[q + 1]], dy[off[q]:off[q + 1]]
        ok = (sid != 65535) & (np.abs(d) <= max_dy_dm)
        kk = np.where(ok, kind[sid], 4)
        out[r['route_id']] = dict(s_off=float((kk == 1).mean()), s_none=float((kk == 4).mean()))
    return out


def duplicates(racelines, tol=3.0):
    """-> {route id: (partner route id, share of this line's points within `tol` m of the partner's line)} for the best partner (0, 0.0 if none)."""
    from scipy.spatial import cKDTree
    allp = np.vstack([np.array(r['points']) for r in racelines])
    allr = np.concatenate([[r['route_id']] * len(r['points']) for r in racelines])
    tree = cKDTree(allp); out = {}
    for r in racelines:
        P = np.array(r['points']); cnt = collections.Counter()
        dd, ii = tree.query(P, k=8, distance_upper_bound=tol)
        for row_d, row_i in zip(dd, ii):
            for o in {int(allr[iv]) for dv, iv in zip(row_d, row_i) if np.isfinite(dv)} - {r['route_id']}:
                cnt[o] += 1
        out[r['route_id']] = (cnt.most_common(1)[0][0], cnt.most_common(1)[0][1] / len(P)) if cnt else (0, 0.0)
    return out


def extract_features(media, racelines, races, surf=None):
    """Everything predict() needs, one dict per route id.  racelines = racelines.json (list), races = races.json (list; the route record kind + name are used),
    surf = optional (off, ids, dy) from sample_line_surfaces (computed here when None - needs the process-pool guard)."""
    import fh6careers
    surf = surf or sample_line_surfaces(media, racelines)
    S = surface_shares(racelines, *surf); Tg = owt_tag_shares(media); D = duplicates(racelines)
    AI, ST, SC, exact = ai_families(media), story_routes(media), showcase_routes(media), fh6careers.race_types(media)
    kinds = {r['extra']['route_id']: r for r in races if r['type'] in ('race_start', 'ie_route', 'horizon_chase')}
    F = {}
    for r in racelines:
        rid = r['route_id']; rec = kinds.get(rid)
        f = dict(rid=rid, circuit=bool(r['circuit']), len=float(r['length_m']), **S[rid], **Tg[rid])
        f['dup_with'], f['dup_frac'] = D[rid]
        f['record'] = rec['type'] if rec else None
        f['name'] = ((rec or {}).get('extra', {}).get('names') or {}).get('EN') or (rec or {}).get('extra', {}).get('race_name')
        f['type_exact'] = (exact.get(rid) or {}).get('type_exact')
        f['ai'], f['story_challenge'], f['showcase'] = AI.get(rid), ST.get(rid), SC.get(rid)
        F[rid] = f
    return F


# ------------------------------------------------------------------------------------------------------------ fitted line rules (the fallback)
RACE_TYPES = ('road', 'street', 'rally', 'cross_country', 'touge', 'drag')


def _thr(x, t, sign=1):
    """threshold th maximising the accuracy of the rule (sign*x >= sign*th) == t  (midpoints between sorted distinct values)"""
    x = np.asarray(x, float); t = np.asarray(t, bool); v = np.unique(x)
    if len(v) < 2:
        return float(v[0])
    return float(max(((np.mean(((x * sign) >= (th * sign)) == t), th) for th in (v[:-1] + v[1:]) / 2), key=lambda p: p[0])[1])


def _leaf(f, P):
    """which rule fires -> leaf name (the order IS the rule list)"""
    if f['s_off'] >= P['off_surface']:
        return 'offroad_dirt' if f['t_dirt'] >= P['dirt_tag'] else 'offroad_cross'
    if not f['circuit'] and f['len'] <= P['drag_len']:
        return 'drag_short'
    if f['circuit']:
        return 'paved_circuit'
    k = f['rid'] // 1000
    return f'paved_p2p_id{k}' if P['use_id'] and k in (2, 4, 5) else 'paved_p2p'


def fit_line_rules(feats, labels, use_id=True, alpha=0.5):
    """feats = feature dicts, labels = race types (RACE_TYPES) -> params incl. the per-leaf class distribution (Laplace-smoothed over all six types).
    Rules (first match wins): s_off >= off_surface -> off-road (t_dirt >= dirt_tag -> rally, else cross_country); point-to-point and len <= drag_len -> drag;
    circuit -> road; paved point-to-point -> street / road / touge undecidable from the line: leaf distribution (+ the route-id thousands digit as a hint)."""
    y = np.array(labels); off = np.isin(y, ['rally', 'cross_country'])
    P = {'off_surface': _thr([f['s_off'] for f in feats], off)}
    P['dirt_tag'] = _thr([f['t_dirt'] for f, k in zip(feats, off) if k], [yy == 'rally' for yy, k in zip(y, off) if k])
    pd = [(f['len'], yy == 'drag') for f, yy, o in zip(feats, y, off) if not o and not f['circuit']]
    P['drag_len'] = _thr([a for a, _ in pd], [b for _, b in pd], sign=-1)
    P['use_id'] = use_id
    leaves = collections.defaultdict(collections.Counter)
    for f, yy in zip(feats, y):
        leaves[_leaf(f, P)][yy] += 1
    P['leaf'] = {}
    for k, c in leaves.items():
        tot = sum(c.values()) + alpha * len(RACE_TYPES)
        P['leaf'][k] = {t: (c[t] + alpha) / tot for t in RACE_TYPES}
    return P


# thresholds fitted on all 85 race-typed marks of the project data (extract_predictions.py --refit re-fits them after a game update)
LINE_RULES = {
    'off_surface': 0.1241, 'dirt_tag': 0.1478, 'drag_len': 1843.7, 'use_id': True,
    'leaf': {
        'offroad_dirt':  {'road': 0.0208, 'street': 0.0208, 'rally': 0.8958, 'cross_country': 0.0208, 'touge': 0.0208, 'drag': 0.0208},
        'offroad_cross': {'road': 0.0238, 'street': 0.0238, 'rally': 0.0238, 'cross_country': 0.881, 'touge': 0.0238, 'drag': 0.0238},
        'paved_circuit': {'road': 0.7188, 'street': 0.0938, 'rally': 0.0312, 'cross_country': 0.0938, 'touge': 0.0312, 'drag': 0.0312},
        'paved_p2p_id2': {'road': 0.8077, 'street': 0.0385, 'rally': 0.0385, 'cross_country': 0.0385, 'touge': 0.0385, 'drag': 0.0385},
        'paved_p2p_id4': {'road': 0.0278, 'street': 0.8611, 'rally': 0.0278, 'cross_country': 0.0278, 'touge': 0.0278, 'drag': 0.0278},
        'paved_p2p_id5': {'road': 0.0625, 'street': 0.0625, 'rally': 0.0625, 'cross_country': 0.0625, 'touge': 0.6875, 'drag': 0.0625},
        'drag_short':    {'road': 0.0833, 'street': 0.0833, 'rally': 0.0833, 'cross_country': 0.0833, 'touge': 0.0833, 'drag': 0.5833},
    },
}


def predict_line(f, P=None):
    P = P or LINE_RULES
    leaf = _leaf(f, P)
    dist = P['leaf'].get(leaf)
    if dist is None:                                   # unseen id-hint leaf -> generic paved p2p
        leaf = 'paved_p2p'; dist = P['leaf'].get(leaf) or {'road': 0.5, 'street': 0.5}
    order = sorted(dist.items(), key=lambda kv: -kv[1])
    why = {'offroad_dirt': f"off-road terrain under {f['s_off']:.0%} of the line (>= {P['off_surface']:.0%}) and AI-line dirt tag on {f['t_dirt']:.0%} (>= {P['dirt_tag']:.0%})",
           'offroad_cross': f"off-road terrain under {f['s_off']:.0%} of the line (>= {P['off_surface']:.0%}), AI-line dirt tag only {f['t_dirt']:.0%} (< {P['dirt_tag']:.0%})",
           'drag_short': f"point-to-point, {f['len']:.0f} m (<= {P['drag_len']:.0f} m)",
           'paved_circuit': 'paved circuit'}.get(leaf)
    if why is None:
        why = (f"paved point-to-point; route id {f['rid']} = {f['rid'] // 1000}xxx by the id convention (not a game field)" if leaf.startswith('paved_p2p_id')
               else 'paved point-to-point: road / street / touge cannot be told apart from the line')
    return order[0][0], order[0][1], why, order[:3], leaf


# ------------------------------------------------------------------------------------------------------------ the ordered predictor
# confidence per source = precision against the project marks, Laplace-smoothed (hits + 1) / (n + 2)
AI_CONF = {'Dirt': 0.957, 'Cross': 0.95, 'Touge': 0.857, 'Drag': 0.80, 'Street': 0.941, 'Road': 0.88}
COPY_CONF = 0.80          # 3 of 3 marked event copies (30002-30004) are story
STORY_CONF = 0.90         # 2 of 2 marked story-challenge routes (11017, 11044) are story; the other 47 are unmarked
SHOWCASE_CONF = 0.70      # 1 marked Showcase route (8004 -> wristband); 8005 is the same EventType
# Series finales shown with the street (purple) icon in-game. User rule 2026-10-03: "Goliath is technically a road race, but it has the purple icon, so everyone
# considers it a street race, especially since for every race type there's one final race, so it should be purple."  5555 The Goliath: AI family Road (6/6), Street 5/6.
FINALE_ICON = {5555: 'street'}
SOURCES = ('exact', 'finale_icon', 'event', 'ai_family', 'not_a_race', 'copy', 'line')


def predict(f, sources=SOURCES, P=None):
    """One feature dict -> dict(type, kind, confidence, source, why, alternatives).  type in road|street|rally|cross_country|touge|drag|story|wristband|none,
    kind in race|event|not_a_race, source in exact|finale_icon|event|ai_family|not_a_race|copy|line|id_convention.  `sources` switches steps off (self-check tests ai_family alone)."""
    def out(t, kind, c, src, why, alt=()):
        return dict(type=t, kind=kind, confidence=round(c, 3), source=src, why=why, alternatives=[[a, round(b, 3)] for a, b in alt])
    if 'exact' in sources and f.get('type_exact'):
        return out(f['type_exact'], 'race', 1.0, 'exact', 'CareerRaceDataSet / TrackInfoDataSet field')
    if 'finale_icon' in sources and f['rid'] in FINALE_ICON:
        return out(FINALE_ICON[f['rid']], 'race', 1.0, 'finale_icon', 'series finale; shown with the street (purple) icon in-game — user rule 2026-10-03')
    if 'event' in sources:
        if f.get('story_challenge'):
            return out('story', 'event', STORY_CONF, 'event', f"HorizonStoryChallengeData.RouteId ({f['story_challenge']})")
        if f.get('showcase'):
            return out('wristband', 'event', SHOWCASE_CONF, 'event', f"CareerRaceDataSet EventType=Showcase ({f['showcase']}), same as the marked wristband 8004")
    ai = f.get('ai')
    if 'ai_family' in sources and ai and ai['family']:
        c = AI_CONF[ai['family']] if ai['margin'] > 0 else 0.5      # margin 0 = Street/Road tie (the route lacks the one level that tells them apart)
        return out(ai['type'], 'race', c, 'ai_family', f"AI family {ai['family']}_*: {ai['score']}/{ai['levels']} difficulty levels match (runner-up {ai['runner_up']}, margin {ai['margin']})",
                   [(FAMILY_TYPE[ai['runner_up']], 1 - c)])
    if 'not_a_race' in sources:
        if f.get('record') == 'ie_route':
            return out('none', 'not_a_race', 0.95, 'not_a_race', 'Initial-Experience tutorial drive (ie_route)')
        if f.get('record') == 'horizon_chase':
            return out('none', 'not_a_race', 0.8, 'not_a_race', 'Horizon Chase route')
        if f.get('s_none', 0) >= 0.95 and f.get('t_0', 0) >= 0.99:
            return out('none', 'not_a_race', 0.8, 'not_a_race', 'no terrain collision under the line and every AI-line tag is 0 (test / off-map route)')
    if 'copy' in sources and f['rid'] >= 30000 and f.get('dup_frac', 0) >= 0.95 and f['dup_with'] not in (0, f['rid']):
        r = out('story', 'event', COPY_CONF, 'copy', f"racing line lies {f['dup_frac']:.0%} on route {f['dup_with']} (event copy; the marked copies 30002-30004 are story)")
        r['copy_of'] = f['dup_with']
        return r
    if 'line' in sources:
        t, p, why, alt, leaf = predict_line(f, P)
        return out(t, 'race', p, 'id_convention' if leaf.startswith('paved_p2p_id') else 'line', why, alt[1:])
    return out(None, None, 0.0, None, 'no source applies')
