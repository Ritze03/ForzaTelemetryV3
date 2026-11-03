#!/usr/bin/env python3
"""Extract the FH6 map icons from the user's own install to PNG (READ-ONLY on the game files).

What it does
  1. Decodes every .swatchbin in UI/Textures/HiRes/Data_Bound/Horizon_Map.zip (map icons, pins, filters,
     region images) -> <out>/png/<zip path without .swatchbin>.png  (RGBA).
  2. Reads UI.zip/MapProfiles/**/*.xml (the game's map-element definitions), crops the packed atlas sheets
     (ForteMapIconSheet etc.) into per-symbol PNGs  <out>/png/atlas/<Sheet>/<pos_key>.png , decodes the few
     icons the XML references from other Data_Bound zips -> <out>/png/ext/<Zip>/<path>.png
  3. Writes <out>/icons.json  [{name,w,h,format,source,...}]  and <out>/xml_symbols.json
     {map-element type tag: [{group, icons:[icon names]}]}  (what the game draws for each element type).
  mapping.json (POI category -> icon) is built from xml_symbols.json by build_icon_mapping.py.

Swatchbin pixel-format field: u32 LE at file offset 0x74 (all other header words are identical across formats).
    0x00 = BC1 (8 B/4x4 block)   0x09 = BC7 (16 B/block)   0x0d = RGBA8 (4 B/px, byte order R,G,B,A)
    0x07 = 16 B/block, probably BC6H (HDRImages.zip only; unverified, not decoded here)
Pixel data starts at header end (u32 @0x08, always 0x8c); top-mip size @0x80; width @0x4c, height @0x50, mips @0x54.

BC7 decoding: Pillow ('bcn' decoder; tested with 12.3.0, Image.frombytes('RGBA', (w, h), data, 'bcn', 7)) is tried first; if Pillow cannot, the
optional `texture2ddecoder` package is used; if neither works the BC7 icons are skipped with a message (BC1/RGBA8 still decode).
Needs: numpy, Pillow.   Usage: extract_icons.py [--media .../media] [--out DIR]
"""
import argparse, collections, json, os, re, struct, sys, zipfile
import xml.etree.ElementTree as ET
import numpy as np
from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, ci, resolve_media

FMT = {0x00: 'BC1', 0x09: 'BC7', 0x0D: 'RGBA8', 0x07: 'BC6H?'}


# ---------------------------------------------------------------- swatchbin
def parse_swatchbin(b):
    assert b[:4] == b'burG', 'not a swatchbin'
    hdr, total = struct.unpack_from('<II', b, 8)
    w, h, mips = struct.unpack_from('<III', b, 0x4C)
    fmt = struct.unpack_from('<I', b, 0x74)[0]
    dsz = struct.unpack_from('<I', b, 0x80)[0]
    return dict(w=w, h=h, mips=mips, fmt=fmt, data=b[hdr:hdr + dsz])


def decode_bc1(data, w, h):
    bw, bh = (w + 3) // 4, (h + 3) // 4
    blk = np.frombuffer(data, np.uint8, bw * bh * 8).reshape(bh, bw, 8)
    c = blk[..., :4].copy().view('<u2').reshape(bh, bw, 2).astype(np.int32)

    def rgb(c):
        r, g, bl = (c >> 11) & 31, (c >> 5) & 63, c & 31
        return np.stack([(r * 255 + 15) // 31, (g * 255 + 31) // 63, (bl * 255 + 15) // 31], -1)
    p0, p1 = rgb(c[..., 0]), rgb(c[..., 1])
    four = (c[..., 0] > c[..., 1])[..., None]
    p2 = np.where(four, (2 * p0 + p1) // 3, (p0 + p1) // 2)
    p3 = np.where(four, (p0 + 2 * p1) // 3, 0)
    pal = np.stack([p0, p1, p2, p3], -2)
    alpha = np.stack([np.full((bh, bw), 255), np.full((bh, bw), 255), np.full((bh, bw), 255),
                      np.where(four[..., 0], 255, 0)], -1)  # index 3 transparent in 3-colour mode
    idx = blk[..., 4:].copy().view('<u4').reshape(bh, bw)
    ix = ((idx[..., None] >> (np.arange(16) * 2).astype(np.uint32)) & 3).reshape(bh, bw, 4, 4)
    rgbo = np.empty((bh, bw, 4, 4, 3), np.uint8)
    ao = np.empty((bh, bw, 4, 4), np.uint8)
    for k in range(4):
        m = ix == k
        rgbo[m] = np.broadcast_to(pal[:, :, k, None, None, :], (bh, bw, 4, 4, 3))[m]
        ao[m] = np.broadcast_to(alpha[:, :, k, None, None], (bh, bw, 4, 4))[m]
    rgba = np.concatenate([rgbo, ao[..., None]], -1).transpose(0, 2, 1, 3, 4).reshape(bh * 4, bw * 4, 4)
    return rgba[:h, :w]


def decode(sw):
    w, h, f, d = sw['w'], sw['h'], sw['fmt'], sw['data']
    if f == 0x00:
        return Image.fromarray(decode_bc1(d, w, h), 'RGBA')
    if f == 0x09:
        try:                                                   # Pillow's own BCn decoder (no extra dependency)
            return Image.frombytes('RGBA', (w, h), d, 'bcn', 7)       # d = ceil(w/4)*ceil(h/4)*16 bytes (w,h need not be multiples of 4)
        except Exception as e:
            err = e
        try:                                                   # optional fallback: pip install texture2ddecoder
            import texture2ddecoder
            return Image.frombytes('RGBA', (w, h), texture2ddecoder.decode_bc7(d, w, h), 'raw', 'BGRA')
        except ImportError:
            raise ValueError(f'BC7 needs Pillow with the bcn decoder (got: {err}) or the texture2ddecoder package')
    if f == 0x0D:
        return Image.frombytes('RGBA', (w, h), d[:w * h * 4], 'raw', 'RGBA')
    raise ValueError(f'unsupported swatchbin format 0x{f:x}')


# ---------------------------------------------------------------- MapProfiles XML
def load_xml(uizip):
    z = zipfile.ZipFile(uizip)
    trees = {}
    for n in z.namelist():
        if n.startswith('MapProfiles/') and n.endswith('.xml'):
            try:
                trees[os.path.basename(n)] = ET.fromstring(z.read(n))
            except ET.ParseError:
                pass  # MapIncludeDebugBackgroundTileLayout.xml is not well-formed (debug only)
    return trees


def build_resources(trees):
    res = collections.defaultdict(list)
    for f, r in trees.items():
        for e in r.iter('Resource'):
            k = e.get('key') or e.get('Key')
            if k:
                res[k.lower()].append((f, dict(e.attrib)))
    return res


def build_templates(trees):
    t = {}
    for r in trees.values():
        for e in r.iter('Resource'):
            k = e.get('key') or e.get('Key')
            if k and len(e):
                t[k.lower()] = e
    return t


def best(res, key):
    """Resource by key; prefer non-colourblind files, then the Default colourblind file (= normal colours)."""
    L = res.get((key or '').lower())
    if not L:
        return None
    pr = lambda x: 0 if 'Colorblind' not in x[0] else (1 if x[0] == 'MapIncludeColorblindModeDefault.xml' else 2)
    return sorted(L, key=pr)[0][1]


def type_tags(group):
    """positive `type` filters of a Group, plus 'rf:<key>' for ResourceFilters (e.g. rf:is_campaign_asphalt_p2p)"""
    out = []

    def walk(e, neg):
        for c in e:
            if c.tag == 'Filter' and c.get('tag') == 'type' and not neg:
                out.append(c.get('value'))
            elif c.tag == 'ResourceFilter' and c.get('resource_key'):
                out.append('rf:' + c.get('resource_key'))
            elif c.tag == 'NotFilter':
                continue
            else:
                walk(c, neg)
    walk(group, False)
    return out


# ---------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[1])
    add_media_args(ap)
    a = ap.parse_args()
    media = resolve_media(a)
    bound = ci(media, 'UI/Textures/HiRes/Data_Bound')
    hm_path = os.path.join(bound, 'Horizon_Map.zip')
    hm = zipfile.ZipFile(hm_path)
    hm_rel = os.path.relpath(hm_path, media)
    png = os.path.join(a.out, 'png')
    entries, sheets, skipped = [], {}, []

    def save(name, im):
        p = os.path.join(png, name + '.png')
        os.makedirs(os.path.dirname(p), exist_ok=True)
        im.save(p)

    # 1. every swatchbin in Horizon_Map.zip
    for n in sorted(hm.namelist()):
        if not n.endswith('.swatchbin'):
            continue
        name = n[:-len('.swatchbin')]
        sw = parse_swatchbin(hm.read(n))
        try:
            im = decode(sw)
        except Exception as e:
            skipped.append(n)
            if len(skipped) <= 3:
                print('SKIP', n, e)
            continue
        save(name, im)
        if name.endswith('Sheet') or 'IconSheet' in name:
            sheets[name.split('/')[-1]] = im
        entries.append(dict(name=name, w=sw['w'], h=sw['h'], format=FMT.get(sw['fmt'], hex(sw['fmt'])),
                            source=f'{hm_rel}:{n}', kind='file'))
    have = {e['name'].lower(): e['name'] for e in entries}

    # 2. XML symbols
    trees = load_xml(ci(media, 'UI.zip'))
    res = build_resources(trees)
    tmpl = build_templates(trees)
    ext_zips = {}

    def file_icon(fp):
        """'Data_Bound\\Horizon_Map\\icons\\...\\X.png' -> icon name (decoding it first if it lives in another zip)"""
        parts = fp.replace('\\', '/').split('/')
        assert parts[0].lower() == 'data_bound'
        if parts[1].lower() == 'horizon_map':
            nm = '/'.join(parts[2:]).rsplit('.', 1)[0]
            return have.get(nm.lower())
        nm = 'ext/' + parts[1] + '/' + '/'.join(parts[2:]).rsplit('.', 1)[0]
        if nm.lower() in have:
            return have[nm.lower()]
        try:
            zp = ci(bound, parts[1] + '.zip')
            z = ext_zips.setdefault(zp, zipfile.ZipFile(zp))
            inner = next(n for n in z.namelist() if n.lower() == '/'.join(parts[2:]).rsplit('.', 1)[0].lower() + '.swatchbin')
        except (StopIteration, OSError):
            return None
        sw = parse_swatchbin(z.read(inner))
        save(nm, decode(sw))
        entries.append(dict(name=nm, w=sw['w'], h=sw['h'], format=FMT.get(sw['fmt'], hex(sw['fmt'])),
                            source=f'{os.path.relpath(zp, media)}:{inner}', kind='file'))
        have[nm.lower()] = nm
        return nm

    def atlas_crop(sheet, poskey, slots):
        pos = best(res, poskey)
        if pos is None or 'x' not in pos:
            return None
        sx, sy = int(slots['x_slots']), int(slots['y_slots'])
        im = sheets[sheet]
        cw, ch = im.width // sx, im.height // sy
        x, y = int(pos['x']), int(pos['y'])
        nm = f'atlas/{sheet}/{poskey}'
        if nm.lower() not in have:
            crop = im.crop((x * cw, y * ch, (x + 1) * cw, (y + 1) * ch))
            if crop.getchannel('A').getextrema()[1] < 8:  # cell is blank in this sheet (symbol lives elsewhere)
                print('  empty atlas cell, skipped:', nm)
                have[nm.lower()] = None
                return None
            save(nm, crop)
            entries.append(dict(name=nm, w=cw, h=ch, format='crop', source=f'{hm_rel}:icons/MapIcons/{sheet}.swatchbin',
                                kind='atlas', cell=[x, y], slots=[sx, sy]))
            have[nm.lower()] = nm
        return have.get(nm.lower())

    def atlas_icons(texres, poskey, slotkey):
        """[(icon name, restrict-to-type-tags or None)]. x,y are in units of (sheet / slots). A pos key may be a
        conditional resource (<Value resource_key=.. ><Filter tag="type" value=..>), expanded to one crop per type."""
        tex = best(res, texres) if texres and not texres.endswith('.png') else None
        texpath = (tex or {}).get('value') or texres
        sheet = os.path.basename((texpath or '').replace('\\', '/')).rsplit('.', 1)[0]
        slots = best(res, slotkey)
        if sheet not in sheets or not poskey or not slots or 'x_slots' not in slots:
            print('  unresolved atlas ref', texres, poskey, slotkey, slots)
            return []
        out = []
        cond = tmpl.get(poskey.lower())
        if cond is not None and cond.findall('Value'):
            for v in cond.findall('Value'):
                tags = [f.get('value') for f in v.iter('Filter') if f.get('tag') == 'type']
                ic = atlas_crop(sheet, v.get('resource_key'), slots)
                if ic and tags:
                    out.append((ic, tags))
        else:
            ic = atlas_crop(sheet, poskey, slots)
            if ic:
                out.append((ic, None))
        return out

    sym = collections.defaultdict(list)
    for fname, r in trees.items():
        for g in r.iter('Group'):
            tags = type_tags(g)
            icons, restricted = [], collections.defaultdict(list)
            for s in g.findall('Symbolizer'):
                ics = []
                ta, ap_, asl, tx = s.find('TextureAtlas'), s.find('AtlasPos'), s.find('AtlasSlots'), s.find('Texture')
                t = tmpl.get((s.get('resource_key') or '').lower())  # template symbolizer supplies default slots/texture
                if t is not None:
                    asl = asl if asl is not None else t.find('AtlasSlots')
                    tx = tx if tx is not None else t.find('Texture')
                if ta is not None:
                    d = best(res, ta.get('resource_key'))
                    if d and d.get('filepath'):
                        ics = [(file_icon(d['filepath']), None)]
                elif ap_ is not None and asl is not None and tx is not None:
                    ics = atlas_icons(tx.get('resource_key') or tx.get('value'), ap_.get('resource_key'), asl.get('resource_key'))
                elif tx is not None:
                    d = best(res, tx.get('resource_key')) or {}
                    p = tx.get('value') or d.get('value') or d.get('filepath') or ''
                    if p.lower().startswith('data_bound'):
                        ics = [(file_icon(p), None)]
                for ic, only in ics:
                    if not ic or (not ic.startswith('atlas/') and ('IconSheet' in ic or ic.endswith('Sheet'))):  # whole-sheet refs are not symbols
                        continue
                    if only:
                        for t_ in only:
                            restricted[t_].append(ic)
                    elif ic not in icons:
                        icons.append(ic)
            for t_ in dict.fromkeys(tags):
                if icons:
                    sym[t_].append(dict(group=g.get('name'), file=fname, icons=icons))
            for t_, ic in restricted.items():
                sym[t_].append(dict(group=g.get('name'), file=fname, icons=list(dict.fromkeys(ic))))
    json.dump(entries, open(os.path.join(a.out, 'icons.json'), 'w'), indent=1)
    json.dump(sym, open(os.path.join(a.out, 'xml_symbols.json'), 'w'), indent=1)
    print(f'{len(entries)} icons ({sum(e["kind"] == "file" for e in entries)} files, '
          f'{sum(e["kind"] == "atlas" for e in entries)} atlas crops); {len(sym)} element types with icons')
    if skipped:
        print(f'{len(skipped)} files skipped (undecodable format - BC6H, or BC7 without a decoder): {skipped[:5]}')


if __name__ == '__main__':
    main()
