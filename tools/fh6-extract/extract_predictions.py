#!/usr/bin/env python3
"""FH6 predicted race TYPE + predicted map-PIN position for every route -> predictions.json.   READ-ONLY on the install.

Usage: extract_predictions.py [--media <...>/ForzaHorizon6/media] [--out DIR] [--canon assets/map/fh6-road-types.json] [--refit]
--out must hold races.json, pois.json and racelines.json (the extract_races / extract_poi / extract_racelines outputs); takes ~20 s (terrain collision sampling
under every racing line).  The methods live in racetype_method.py (type) and pin_method.py (pin); validation + caveats: docs/game-data/fh6-game-files.md
("Predicting race type and map-pin position").

Output predictions.json: {"version", "routes": {route id: {type_predicted, type_kind, type_confidence, type_source, type_why, type_alternatives, pin_predicted,
type_exact}}, "check": {...accuracy against the project marks...}}.  Predictions are PREDICTIONS: build_viewer.py keeps them apart from the exact game-file data and from the
user's marks, and nothing here ever writes into fh6-road-types.json.  The self-check against the marks is printed (and stored in "check") on every run.
--refit  re-fits the line-rule thresholds on the marks (leave-one-out accuracy printed) and prints the new LINE_RULES; nothing is written.
"""
import argparse, collections, json, os, sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from fh6common import add_media_args, resolve_media
import racetype_method as rm
import pin_method as pm

VERSION = 1


def acc(pairs):
    return f'{sum(a == b for a, b in pairs)}/{len(pairs)}'


def check(F, lab):
    """the ported predictor against the project's hand marks (in-sample for the line thresholds; every other step has no fitted parameter)"""
    exact = {r for r in lab if F[r]['type_exact']}
    full = {r: rm.predict(F[r])['type'] for r in lab}
    ai = [(lab[r], F[r]['ai']['type']) for r in sorted(lab) if F[r]['ai'] and F[r]['ai']['family']]
    miss = [(r, lab[r], full[r], rm.predict(F[r])['source']) for r in sorted(lab) if full[r] != lab[r]]
    c = dict(marks=len(lab), exact_covered=len(exact), full=acc([(lab[r], full[r]) for r in lab]), hand_only=acc([(lab[r], full[r]) for r in lab if r not in exact]),
             ai_family_alone=acc(ai), misses=miss)
    print(f'self-check vs {len(lab)} marks ({len(exact)} with an exact type): full pipeline {c["full"]}, hand-only {c["hand_only"]}, AI family alone {c["ai_family_alone"]}')
    print('  misses:', ', '.join(f'{r} marked {a} predicted {b} ({s})' for r, a, b, s in miss) or 'none')
    return c


def refit(F, lab):
    race = [r for r in sorted(lab) if lab[r] in rm.RACE_TYPES]
    for use_id in (False, True):
        ok = 0
        for i in race:
            tr = [r for r in race if r != i]
            P = rm.fit_line_rules([F[r] for r in tr], [lab[r] for r in tr], use_id)
            ok += rm.predict_line(F[i], P)[0] == lab[i]
        print(f'line rules leave-one-out, use_id={use_id}: {ok}/{len(race)}')
    P = rm.fit_line_rules([F[r] for r in race], [lab[r] for r in race], True)
    print('LINE_RULES =', json.dumps({k: (round(v, 4) if isinstance(v, float) else v) for k, v in P.items() if k != 'leaf'}))
    for k, v in P['leaf'].items():
        print(f"  {k}: {({a: round(b, 4) for a, b in v.items()})}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0]); add_media_args(ap)
    ap.add_argument('--canon', default=os.path.join(HERE, '..', '..', 'assets', 'map', 'fh6-road-types.json'), help="the project's hand marks (only used for the self-check)")
    ap.add_argument('--refit', action='store_true')
    a = ap.parse_args(); media = resolve_media(a)
    J = lambda n: json.load(open(os.path.join(a.out, n)))
    races, pois, racelines = J('races.json'), J('pois.json'), J('racelines.json')
    lab = {int(k): v for k, v in json.load(open(a.canon))['races'].items()}
    F = rm.extract_features(media, racelines, races)
    print(f'features for {len(F)} routes')
    if a.refit:
        return refit(F, lab)
    ctx = pm.Ctx(races, pois)
    out = {}
    for rid in sorted(F):
        f = F[rid]; p = rm.predict(f)
        out[str(rid)] = dict(type_predicted=p['type'], type_kind=p['kind'], type_confidence=p['confidence'], type_source=p['source'], type_why=p['why'],
                             type_alternatives=p['alternatives'], pin_predicted=pm.predict_pin(rid, ctx), type_exact=f['type_exact'])
        if p.get('copy_of'):
            out[str(rid)]['copy_of'] = p['copy_of']
    # story ids with no HorizonStoryChallengeData entry that sit between ones that have it (an id gap, e.g. 11016): possibly an omitted story finale
    st = {r for r in F if F[r]['story_challenge']}
    for r in sorted(F):
        if 11000 <= r < 12000 and not F[r]['story_challenge'] and any(s in st for s in (r - 1, r + 1)):
            out[str(r)]['story_gap'] = True
    c = check(F, lab)
    n = collections.Counter(v['type_source'] for v in out.values()); pins = collections.Counter(v['pin_predicted']['method'] for v in out.values())
    print('type sources:', dict(n)); print('pin methods :', dict(pins))
    with open(os.path.join(a.out, 'predictions.json'), 'w') as fh:
        json.dump(dict(version=VERSION, routes=out, check=c), fh, separators=(',', ':'), default=float)
    print(f'{len(out)} routes -> {a.out}/predictions.json')


if __name__ == '__main__':
    main()
