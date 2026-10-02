#!/usr/bin/env python3
"""FH6 CarOrdinal -> car name, from the plaintext install only (no gamedb, no key, no lookup table).  READ-ONLY on the install.

Chain (all files plaintext):
  ordinal -> MediaName : every `media/Cars/<MediaName>.zip` (plain deflate zip, 671 of them) contains
                         `Scene/animations/Mojo/clip/carclips_<ordinal>.clipd`.  Only the zip CENTRAL DIRECTORY is read (~0.4 s for all).
  ordinal -> name      : `Stripped/StringTables/<LANG>.zip` -> `Data_Car.str`
                         strhash('IDS_ModelShort_<ordinal>')   = full name incl. make (+ year for ~238/638): "Mazda RX-7 '92"
                         strhash('IDS_DisplayName_<ordinal>')  = model only: "RX-7 Type R"
                         (the gamedb column names are swapped vs their content: ModelShort is the LONG text.)
  Localised: `--lang DE` etc. (the 24 shipped languages).  No separate make field exists in the readable files (MakeID -> List_CarMake
  lives in the encrypted gamedb); the make is only inside the full name.  The year of the MediaName suffix is unreliable (558/638).
UNVERIFIED: that the telemetry packet's CarOrdinal equals this id (validated 638/638 against the decrypted July gamedb's Data_Car.Id,
33 newer cars resolve too, but a live check against a running game is still pending).

Output: cars.json  [{ordinal, media_name, name, model, [lang -> ...]}]  sorted by ordinal.
Usage: extract_cars.py [--media ...] [--out DIR] [--lang EN[,DE,...]|all] [--ordinal 4144 ...]
Output is Playground Games data (names + folder ids): write it outside the repo; an app must derive it at runtime from the user's install.
"""
import argparse, glob, json, os, re, sys, zipfile

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
    out = []
    for o in sorted(om):
        full, model = name_of(tabs[first], o)
        rec = dict(ordinal=o, media_name=om[o], name=full, model=model)
        if len(langs) > 1:
            rec['names'] = {l: dict(zip(('name', 'model'), name_of(tabs[l], o))) for l in langs}
        out.append(rec)
    with open(os.path.join(args.out, 'cars.json'), 'w', encoding='utf-8') as fh:
        json.dump(out, fh, ensure_ascii=False, indent=1)
    zips = len(glob.glob(os.path.join(ci(media, 'Cars'), '*.zip')))
    named = sum(1 for r in out if r['name'])
    print(f'{zips} car zips -> {len(out)} ordinals ({len(set(om.values()))} distinct MediaNames); full name for {named}, model-only for {sum(1 for r in out if r["model"])}; '
          f'languages: {",".join(langs)} -> {args.out}/cars.json')
    for o in args.ordinal or []:
        r = next((r for r in out if r['ordinal'] == o), None)
        print(o, r and (r['media_name'], r['name'], r['model']))


if __name__ == '__main__':
    main()
