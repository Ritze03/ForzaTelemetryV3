#!/usr/bin/env python3
"""Plot pois.json (from extract_poi.py) on a map image -> pois_on_map.png + pois_dense_on_map.png (validation aid).

Usage: plot_pois.py --map <square map image, e.g. map_summer_L3_preview.png from extract_map.py> [--out ./fh6-out]
Reads <out>/pois.json. Calibration = minimap.rs MapCalibration::DEFAULT (0.3722 px/m at 8192 px, origin x=-12540, z=10738).
Needs: Pillow.
"""
import argparse, json, os
from PIL import Image, ImageDraw, ImageFont

ap = argparse.ArgumentParser()
ap.add_argument('--map', required=True)
ap.add_argument('--out', default='./fh6-out')
args = ap.parse_args()
HERE = args.out
P = json.load(open(HERE + '/pois.json'))
Image.MAX_IMAGE_PIXELS = None
im = Image.open(args.map).convert('RGB'); W = im.size[0]; k = 0.3722 * W / 8192
def tf(x, z): return ((x + 12540) * k, (10738 - z) * k)
# (type group, colour, shape, size)
CATS = [('race_start', (255, 40, 40), 'c', 7), ('route_node0', (255, 150, 0), 't', 4), ('story_activation', (60, 200, 255), 'c', 6), ('job_activation', (0, 120, 255), 'c', 6),
        ('house', (255, 255, 0), 's', 7), ('fast_travel', (255, 255, 255), 'd', 5), ('festival_site', (255, 0, 255), 's', 9), ('barn_find', (0, 255, 100), 'd', 7),
        ('landmark', (190, 120, 255), 't', 5), ('xp_board', (255, 220, 120), 'x', 4), ('drift_zone', (255, 100, 200), 'c', 5), ('danger_sign', (255, 60, 0), 'x', 5),
        ('treasure_chest_board', (0, 255, 255), 's', 5), ('aftermarket_spot', (160, 255, 60), 'c', 3), ('showcase', (255, 128, 128), 's', 7),
        ('car_meet', (0, 200, 200), 's', 6), ('touge_event', (255, 170, 255), 'c', 5), ('rush_event', (255, 255, 150), 't', 6), ('drag_meet', (255, 200, 80), 'c', 5), ('treasure_car', (120, 255, 255), 'd', 5)]
dense = [('pinata', (255, 255, 255), 1), ('parking_area', (120, 160, 255), 1), ('eliminator_spawn', (255, 120, 120), 1), ('creature_zone', (120, 255, 120), 2)]
def shape(dr, s, x, y, c, r):
    o = (0, 0, 0)
    if s == 'c': dr.ellipse([x-r-1, y-r-1, x+r+1, y+r+1], fill=o); dr.ellipse([x-r, y-r, x+r, y+r], fill=c)
    elif s == 's': dr.rectangle([x-r-1, y-r-1, x+r+1, y+r+1], fill=o); dr.rectangle([x-r, y-r, x+r, y+r], fill=c)
    elif s == 'd': dr.polygon([(x, y-r-1), (x+r+1, y), (x, y+r+1), (x-r-1, y)], fill=c, outline=o)
    elif s == 't': dr.polygon([(x, y-r-1), (x+r+1, y+r), (x-r-1, y+r)], fill=c, outline=o)
    else: dr.line([x-r, y-r, x+r, y+r], fill=c, width=2); dr.line([x-r, y+r, x+r, y-r], fill=c, width=2)
def render(items, dn, name):
    img = im.copy(); dr = ImageDraw.Draw(img, 'RGBA')
    for t, c, r in dn:
        for p in P:
            if p['type'] == t: x, y = tf(p['x'], p['z']); dr.ellipse([x-r, y-r, x+r, y+r], fill=c + (150,))
    for t, c, s, r in items:
        for p in P:
            if p['type'] == t: x, y = tf(p['x'], p['z']); shape(dr, s, x, y, c, r)
    f = ImageFont.load_default()
    rows = [(t, c, s, r) for t, c, s, r in items] + [(t, c, 'c', 3) for t, c, r in dn]
    pw = 205; ph = 14 * len(rows) + 10
    dr.rectangle([8, 8, 8 + pw, 8 + ph], fill=(0, 0, 0, 200))
    for i, (t, c, s, r) in enumerate(rows):
        y = 18 + i * 14; shape(dr, s, 22, y + 3, c, min(r, 5)); n = sum(1 for p in P if p['type'] == t)
        dr.text((36, y - 3), f'{t} ({n})', fill=(255, 255, 255), font=f)
    img.save(HERE + '/' + name); print(name, img.size)
render(CATS, [], 'pois_on_map.png')
render(CATS, dense, 'pois_dense_on_map.png')
