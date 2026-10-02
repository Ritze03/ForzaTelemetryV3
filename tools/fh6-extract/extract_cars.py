#!/usr/bin/env python3
"""FH6 CarOrdinal -> car name, from the plaintext install only (no gamedb, no key, no lookup table).  READ-ONLY on the install.

Chain (all files plaintext):
  ordinal -> MediaName : every `media/Cars/<MediaName>.zip` (plain deflate zip, 671 of them) contains
                         `Scene/animations/Mojo/clip/carclips_<ordinal>.clipd`.  Only the zip CENTRAL DIRECTORY is read (~0.4 s for all).
  ordinal -> name      : `Stripped/StringTables/<LANG>.zip` -> `Data_Car.str`
                         strhash('IDS_ModelShort_<ordinal>')   = full name incl. make (+ year for ~238/638): "Mazda RX-7 '92"
                         strhash('IDS_DisplayName_<ordinal>')  = model only: "RX-7 Type R"
                         (the gamedb column names are swapped vs their content: ModelShort is the LONG text.)
  Localised: `--lang DE` etc. (the 24 shipped languages).  No MakeID exists in the readable files (MakeID -> List_CarMake lives in the
  encrypted gamedb) and ModelShort does NOT always contain the make (4277 "#21 Civic WTAC", 2574 "M12S Warthog CST"), so the make is a
  HEURISTIC (`derive_makes`; make names = List_CarMake.str, keys IDS_DisplayName_<MakeID>): (a) a make name that prefixes ModelShort /
  DisplayName (longest, whole word, after an optional "#nn " tag); else (b) the majority make of the MediaName's prefix (`HON` -> Honda)
  over the (a) cars; else (c) the make whose letters contain the prefix as a subsequence (most letters on word starts, then fewest skipped letters; ties -> none).
  The Rust port in src/gamedata/cars.rs must stay identical.  The year of the MediaName suffix is unreliable (558/638).
VERIFIED live: the telemetry packet's CarOrdinal equals this id (4277 -> HON_21_CivicWTA_92, Civic WTAC).

Output: cars.json  [{ordinal, media_name, name, model, make, display, [lang -> ...]}]  sorted by ordinal.
`display` = "Make + model": no make -> the game's name; text already starts with the make -> as is; "#nn " tag -> "#nn Make <DisplayName
rest>" ("#21 Honda Hardrace/JDMYard Civic WTAC"); else "Make ModelShort".
Usage: extract_cars.py [--media ...] [--out DIR] [--lang EN[,DE,...]|all] [--ordinal 4144 ...]
Output is Playground Games data (names + folder ids): write it outside the repo; an app must derive it at runtime from the user's install.
"""
import argparse, collections, glob, json, os, re, sys, zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fh6common import add_media_args, ci, resolve_media
from fh6str import StringTables, strhash


def ordinal_map(media):
    """{ordinal: MediaName} from the zip central directories of media/Cars/*.zip"""
    out = {}
    for f in glob.glob(os.path.join(ci(media, 'Cars'), '*.zip')):
        try:
            names = zipfile.ZipFile(f).namelist()
        except zipfile.BadZipFile:
            continue
        for n in names:
            m = re.search(r'carclips_(\d+)\.clipd$', n)
            if m:
                out[int(m.group(1))] = os.path.basename(f)[:-4]
    return out


def car_names(media, lang='EN'):
    """{key: text} of Data_Car.str in that language"""
    return dict(StringTables(media, lang).table('Data_Car'))


def name_of(table, ordinal):
    """-> (full name incl. make, model-only name); None where the table has no entry"""
    return table.get(strhash(f'IDS_ModelShort_{ordinal}')), table.get(strhash(f'IDS_DisplayName_{ordinal}'))


def _split_tag(s):
    m = re.match(r'#\d+ ', s)
    return (s[:m.end() - 1], s[m.end():].lstrip()) if m else (None, s)


def starts_with_make(s, make):
    s = _split_tag(s)[1].lower(); m = make.lower()
    return s.startswith(m) and not (len(s) > len(m) and s[len(m)].isalnum())


def make_names(media, lang='EN'):
    """make names of List_CarMake.str (ids recovered by hashing IDS_DisplayName_<n>; the table also holds placeholder text)"""
    tab = dict(StringTables(media, lang).table('List_CarMake'))
    return sorted({tab[h] for n in range(2000) if (h := strhash(f'IDS_DisplayName_{n}')) in tab and tab[h].strip()})


def _subsequence_make(prefix, makes):
    letters = [c.lower() for c in prefix if c.isascii() and c.isalpha()]
    if len(letters) < 2:
        return None
    best, tie = None, False
    for m in makes:
        ml = m.lower()   # (char, starts a word) - "Gordon Murray Automotive": GMA hits three word starts
        mc = [(c, i == 0 or not ml[i - 1].isalnum()) for i, c in enumerate(ml) if c.isalnum()]
        if not mc or mc[0][0] != letters[0]:
            continue
        li = last = hits = 0
        for i, (c, start) in enumerate(mc):
            if li < len(letters) and c == letters[li]:
                li += 1; last = i; hits += start
        if li < len(letters):
            continue
        score = (-hits, last + 1 - len(letters))   # most word-start hits, then fewest skipped letters
        if best is None or score < best[0]:
            best, tie = (score, m), False
        elif score == best[0]:
            tie = True
    return None if tie or best is None else best[1]


def derive_makes(om, tab, makes, stats=None):
    """{ordinal: make} - the 3-step heuristic described in the module docstring (stats: optional Counter of a/b/c hits)"""
    out, votes = {}, {}
    pre = lambda o: om[o].split('_')[0].upper()
    for o in om:
        for s in name_of(tab, o):
            hits = [m for m in makes if s and starts_with_make(s, m)]
            if hits:
                out[o] = max(hits, key=len)
                votes.setdefault(pre(o), {}).setdefault(out[o], 0)
                votes[pre(o)][out[o]] += 1
                if stats is not None: stats['a'] += 1
                break
    cache = {}
    for o in om:
        if o in out:
            continue
        p = pre(o)
        if p not in cache:
            v = votes.get(p)
            cache[p] = ('b', min(v, key=lambda m: (-v[m], m))) if v else ('c', _subsequence_make(p, makes))
        step, m = cache[p]
        if m:
            out[o] = m
            if stats is not None: stats[step] += 1
    return out


def compose_display(make, full, model, fallback=''):
    base = full or model or fallback
    if not make:
        return base
    tag, rest = _split_tag(base)
    if tag:
        mt, mr = _split_tag(model) if model else (None, None)
        if mt == tag:
            rest = mr
        return f'{tag} {rest}' if starts_with_make(rest, make) else f'{tag} {make} {rest}'
    return base if starts_with_make(base, make) else f'{make} {base}'


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[1]); add_media_args(ap)
    ap.add_argument('--lang', default='EN', help="language code(s), comma separated, or 'all' (default EN; see media/Stripped/StringTables/)")
    ap.add_argument('--ordinal', type=int, nargs='*', help='only print these ordinals (still writes cars.json for all)')
    args = ap.parse_args(); media = resolve_media(args)
    if args.lang.lower() == 'all':
        langs = sorted(f[:-4] for f in os.listdir(ci(media, 'Stripped/StringTables')) if f.endswith('.zip'))
    else:
        langs = [l.strip().upper() for l in args.lang.split(',')]
    om = ordinal_map(media)
    tabs = {l: car_names(media, l) for l in langs}
    first = langs[0]
    stats = collections.Counter()
    mk = derive_makes(om, tabs[first], make_names(media, first), stats)
    out = []
    for o in sorted(om):
        full, model = name_of(tabs[first], o)
        rec = dict(ordinal=o, media_name=om[o], name=full, model=model, make=mk.get(o),
                   display=compose_display(mk.get(o), full, model, om[o]))
        if len(langs) > 1:
            rec['names'] = {l: dict(zip(('name', 'model'), name_of(tabs[l], o))) for l in langs}
        out.append(rec)
    with open(os.path.join(args.out, 'cars.json'), 'w', encoding='utf-8') as fh:
        json.dump(out, fh, ensure_ascii=False, indent=1)
    zips = len(glob.glob(os.path.join(ci(media, 'Cars'), '*.zip')))
    named = sum(1 for r in out if r['name'])
    print(f'make found for {len(mk)}/{len(out)} (heuristic steps a/b/c: {stats["a"]}/{stats["b"]}/{stats["c"]})')
    print(f'{zips} car zips -> {len(out)} ordinals ({len(set(om.values()))} distinct MediaNames); full name for {named}, model-only for {sum(1 for r in out if r["model"])}; '
          f'languages: {",".join(langs)} -> {args.out}/cars.json')
    for o in args.ordinal or []:
        r = next((r for r in out if r['ordinal'] == o), None)
        print(o, r and (r['media_name'], r['name'], r['model'], r['make'], r['display']))


if __name__ == '__main__':
    main()
