#!/usr/bin/env python3
"""Predict the in-game MAP PIN of an FH6 race route.   Inputs: races.json + pois.json (our own extracts).   Method + validation: docs/game-data/fh6-game-files.md
("Predicting race type and map-pin position").   Driven by extract_predictions.py.

Rules, in order:
  1. exact   the route has a `race_trigger_zone_rt<N>` sphere (pois.json `race_pin`) or a `sidi_touge_event_<N>` locator (races.json `touge_pin`)  -> that position
  2. event   a Horizon special-event route (id 8001..8099) without a pin: its pin is the special-event POI (rush_event / showcase / special_event) that no other
             route's exact pin already sits on, nearest to the start line, within 600 m
  3. start   everything else -> the RVAN start line.  Why: the game only needs an extra trigger sphere where the pin is NOT at the start line, so a route
             without a sphere has its pin at the start (0-41 m along the road, median 5 m, on 55 of 55 routes checked against an independent in-game-captured pin set)
Nothing here predicts a DISPLACED pin geometrically: no hypothesis tested gets below ~150 m (leave-one-out), so a displaced pin is only knowable from rule 1.
"""
import math

EVENT_TYPES = ('rush_event', 'showcase', 'special_event')
EVENT_ROUTES = range(8001, 8100)
EXPECTED_ERR = {'exact': 0, 'event_poi': 20, 'start': 18, 'start_unverified': 25, 'start_story_job': 30, 'start_no_pin': None}    # metres; 'start' = p90 of the 55 checked routes


class Ctx:
    def __init__(self, races, pois):
        self.exact, self.start, self.rtype = {}, {}, {}
        for p in pois:
            if p['type'] == 'race_pin':
                self.exact[p['extra']['route']] = (p['x'], p['z'])
        for r in races:
            e = r.get('extra', {})
            if r['type'] == 'touge_pin':
                self.exact.setdefault(e['route_id'], (r['x'], r['z']))
            elif 'route_id' in e and r['type'] in ('race_start', 'ie_route', 'horizon_chase'):
                self.start[e['route_id']] = (r['x'], r['z']); self.rtype[e['route_id']] = r['type']
        self.events = [(p['name'], (p['x'], p['z'])) for p in pois if p['type'] in EVENT_TYPES]
        self.taken = {n for n, c in self.events for q in self.exact.values() if math.dist(c, q) < 60}      # event POIs an exact pin already sits on


def _kind_of_start(rid, ctx):
    if rid in (99, 102, 103) or ctx.rtype.get(rid) in ('ie_route', 'horizon_chase'):
        return 'start_no_pin'                                  # test / off-map / tutorial / chase: no map race pin
    if 11000 <= rid < 30100:
        return 'start_story_job'                               # story-chapter / job routes: the start sits on the story activation
    if rid in (128, 4301, 8008):
        return 'start_unverified'                              # real races with neither a sphere nor an independent position
    return 'start'


def predict_pin(rid, ctx):
    """-> dict(x, z, method, expected_err_m, note); method in exact | event_poi | start | start_unverified | start_story_job | start_no_pin | none"""
    if rid in ctx.exact:
        x, z = ctx.exact[rid]
        return dict(x=x, z=z, method='exact', expected_err_m=0, note='race_trigger_zone sphere / touge locator')
    s = ctx.start.get(rid)
    if s is None:
        return dict(x=None, z=None, method='none', expected_err_m=None, note='no start line')
    if rid in EVENT_ROUTES:
        c = sorted((math.dist(s, p), n, p) for n, p in ctx.events if n not in ctx.taken)
        if c and c[0][0] <= 600:
            return dict(x=c[0][2][0], z=c[0][2][1], method='event_poi', expected_err_m=EXPECTED_ERR['event_poi'], note=f'special-event POI {c[0][1]} ({c[0][0]:.0f} m from the start line)')
    k = _kind_of_start(rid, ctx)
    return dict(x=s[0], z=s[1], method=k, expected_err_m=EXPECTED_ERR[k], note='RVAN start line')
