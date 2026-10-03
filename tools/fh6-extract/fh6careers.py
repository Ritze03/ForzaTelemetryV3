"""FH6 exact race type + exact race name per route, from `ObjectModelGame.zip` (PLAINTEXT, 7121 `source/ScribbleData/<id>.om.xml` BXML files).  READ-ONLY.

Three data sets (file ids are stable across the Sept/Oct-2026 builds we have seen):
  TrackInfoDataSet               13260499414882115191  two maps: `Data` (112 entries, key = CareerRace id: RouteId, RibbonConfig P2P/Circuit/Playground,
                                                       UseCrossCountryAI, DisplayName = `CareerTrackInfo.IDS_DisplayName_<guid>` -> EN/DE string table
                                                       = exact race name) and `InfoByRouteId` (111 entries, route id -> key into Data)
  CareerRaceDataSet              16066973273702787567  only 35 keys (5-11, 20-43, 60-63): RaceMode, UITheme (asphalt_series, touge_series, drag_racing,
                                                       mixed_surface_series, showcase_*), CustomEntityName
  RaceCollectionUIOverridesMap   6322925578581225131   key -> flyer texture; `Backgrounds\\Custom\\StreetRace.png` = street race

Type rule (EXACT FIELDS ONLY - nothing is inferred from geometry, surface or the name; user's call 2026-10-03):
  UITheme asphalt_series -> road,  touge_series -> touge,  drag_racing -> drag,  mixed_surface_series -> rally,
  StreetRace flyer -> street,  UseCrossCountryAI=True -> cross_country, except the 5 Initial-Experience tutorial drives 3333-3337 (they carry the
  flag but are not cross-country races -> no type).  Everything else (scramble/trail keys 64-79, Horizon Rush/showcase/finale, Playground,
  routes without a TrackInfo entry) gets type None.
route -> key is TrackInfoDataSet.InfoByRouteId (one key per route: 2091 -> 39, although Data[4048] also names route 2091).
"""
import zipfile

from fh6bxml import bxml_decode
from fh6common import ci
from fh6str import StringTables

OM_TRACKINFO, OM_CAREERRACE, OM_FLYERS = '13260499414882115191', '16066973273702787567', '6322925578581225131'
THEME = {'asphalt_series': 'road', 'touge_series': 'touge', 'drag_racing': 'drag', 'mixed_surface_series': 'rally'}
INITIAL_EXPERIENCE = range(3333, 3338)


def _maps(z, fid):
    """om.xml -> {property id of a top-level map: [(int key, value node)]}; one file can hold several maps (TrackInfoDataSet: `Data` + `InfoByRouteId`)"""
    obj = bxml_decode(z.read(f'source/ScribbleData/{fid}.om.xml'))[2][0]
    return {dict(p[1]).get('id'): [(int(next(dict(a)['value'] for nm, a, _ in e[2] if nm == 'key')), next(x for x in e[2] if x[0] == 'value'))
                                   for e in p[2] if e[0] == 'map_element'] for p in obj[2] if p[0] == 'property'}


def _table(z, fid, prop):
    """map `prop` of an om.xml -> {int key: {property id: value}} (only the plain `value=` properties of each element)"""
    return {k: {dict(c[1]).get('id'): dict(c[1]).get('value') for c in v[2] if c[0] == 'property'} for k, v in _maps(z, fid)[prop]}


def race_types(media, langs=('EN', 'DE')):
    """-> {route id: {type_exact (str|None), type_source (str|None), career_key, ribbon, names {lang: str}}} for the 111 routes of TrackInfoDataSet"""
    z = zipfile.ZipFile(ci(media, 'ObjectModelGame.zip'))
    ti, cr = _table(z, OM_TRACKINFO, 'Data'), _table(z, OM_CAREERRACE, 'Data')
    fl = _table(z, OM_FLYERS, 'RaceCollectionUIOverridesMap')
    by_route = {rid: int(dict(v[2][0][1])['value']) for rid, v in _maps(z, OM_TRACKINFO)['InfoByRouteId']}      # route -> TrackInfo/CareerRace key
    st = {l: StringTables(media, l) for l in langs}
    res = {}
    for rid, key in sorted(by_route.items()):
        t = ti[key]
        if t.get('RouteId') != str(rid):
            print(f'WARNING: InfoByRouteId says route {rid} -> key {key} but Data[{key}].RouteId={t.get("RouteId")}')
        typ = src = None
        theme = (cr.get(key) or {}).get('UITheme')
        if theme in THEME:
            typ, src = THEME[theme], f'CareerRaceDataSet[{key}].UITheme={theme}'
        elif t.get('UseCrossCountryAI') == 'True' and rid not in INITIAL_EXPERIENCE:
            typ, src = 'cross_country', f'TrackInfoDataSet[{key}].UseCrossCountryAI=True'
        elif 'Custom\\StreetRace.png' in (fl.get(key) or {}).get('FlyerTexturePath', ''):
            typ, src = 'street', f'RaceCollectionUIOverridesMap[{key}].FlyerTexturePath=StreetRace.png'
        res[rid] = dict(type_exact=typ, type_source=src, career_key=key, ribbon=t.get('RibbonConfig'),
                        names={l: s.ids(t['DisplayName']) for l, s in st.items()})
    return res
