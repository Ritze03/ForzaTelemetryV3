"""Shared helpers for the fh6-extract scripts: locate the FH6 install's media/ dir, case-insensitive paths.

READ-ONLY on the game install - nothing here ever writes into it.
"""
import os, re, sys

APP_FOLDER = 'ForzaHorizon6'   # steamapps/common/<this>/media ; Steam app id 2483190
VDF_CANDIDATES = [
    '~/.local/share/Steam/steamapps/libraryfolders.vdf',
    '~/.steam/steam/steamapps/libraryfolders.vdf',
    '~/.var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/libraryfolders.vdf',   # Flatpak
    r'C:\Program Files (x86)\Steam\steamapps\libraryfolders.vdf',
    r'C:\Program Files\Steam\steamapps\libraryfolders.vdf',
]


def ci(root, *parts):
    """Case-insensitive path walk (on-disk case is mixed: 'Tracks', 'trackroutes', 'UI/Textures/Data_Bound'...).
    Each part may itself contain '/' separators."""
    p = root
    for part in '/'.join(parts).split('/'):
        if not part:
            continue
        hit = next((e for e in os.listdir(p) if e.lower() == part.lower()), None)
        if hit is None:
            raise FileNotFoundError(os.path.join(p, part))
        p = os.path.join(p, hit)
    return p


def steam_libraries():
    """Every Steam library root listed in any libraryfolders.vdf we can find (plus the Steam root itself)."""
    libs = []
    for v in VDF_CANDIDATES:
        v = os.path.expanduser(v)
        if not os.path.isfile(v):
            continue
        libs.append(os.path.dirname(os.path.dirname(v)))
        for m in re.finditer(r'"path"\s+"([^"]+)"', open(v, encoding='utf-8', errors='replace').read()):
            libs.append(m.group(1).replace('\\\\', '\\'))
    return list(dict.fromkeys(libs))


def autodetect_media():
    for lib in steam_libraries():
        try:
            return ci(lib, 'steamapps', 'common', APP_FOLDER, 'media')
        except FileNotFoundError:
            continue
    return None


def add_media_args(ap):
    ap.add_argument('--media', help='path to <ForzaHorizon6>/media (default: auto-detect via Steam libraryfolders.vdf)')
    ap.add_argument('--out', default='./fh6-out',
                    help='output dir (default ./fh6-out; keep it OUTSIDE the repo - the data is Playground Games IP)')


def resolve_media(args):
    media = args.media or autodetect_media()
    if not media or not os.path.isdir(media):
        sys.exit('FH6 media dir not found. Pass --media /path/to/steamapps/common/ForzaHorizon6/media')
    os.makedirs(args.out, exist_ok=True)
    return media


# ---- plain-text game files shared by several scripts ------------------------------------------------------------------
def rd(path):
    """text of a game file (they may start with a UTF-8 BOM)"""
    return open(path, encoding='utf-8-sig', errors='replace').read()


def locators(path):
    """`.nt` locator list -> [(name, x, y, z)].  Format: <Locator><Name value=".."/> ... <SceneTransform value._41=x _42=height _43=z/>"""
    return [(n, float(x), float(y), float(z)) for n, x, y, z in re.findall(
        r'<Name value="([^"]*)"/>.*?value\._41="([-\d.]+)" value\._42="([-\d.]+)" value\._43="([-\d.]+)"', rd(path), re.S)]


def tzones(path):
    """`.tz` trigger zones -> [(type, name, x, y, z, size_x, size_z)]"""
    res = []
    for m in re.finditer(r'<triggerzone type="(\w+)" name="([^"]*)".*?<position x="([-\d.]+)" y="([-\d.]+)" z="([-\d.]+)" />'
                         r'\s*<size x="([-\d.]+)" y="([-\d.]+)" z="([-\d.]+)"', rd(path), re.S):
        res.append((m.group(1), m.group(2), float(m.group(3)), float(m.group(4)), float(m.group(5)), float(m.group(6)), float(m.group(8))))
    return res
