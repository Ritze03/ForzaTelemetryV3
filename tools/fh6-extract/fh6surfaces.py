"""Terrain surface-id -> name + kind table for the 54 material ids that occur on FH6's terrain (`.phys` collision meshes, see fh6-terrain.md).

The game's own id -> name table (`Physics/surfaceTypes.xml`) is ENCRYPTED in the install, so these names are OURS.  Each entry says how
sure we are (plan decision D9: "use your best guess, but mark that they are reasoned"):

    'confirmed'  the user drove to a spot of this id and looked / checked it in-game (survey, 2026-10-02)
    'seen'       the user saw the spot in the game but it is not reachable / outside the playable map (visual check only)
    'reasoned'   our guess from where the id occurs (next to roads, coast, snow line, forest), megatexture-palette co-occurrence and the
                 neighbouring confirmed ids - NOT checked in-game.  Never present these as the game's names.

Names deliberately stay generic (the in-game surface vocabulary of `Audio/AudioSurfacesInfo.xml` - Asphalt_Smooth, Dirt_Gravel,
Dirt_Forest, Snow_Compact_Road, ... - is a good target for a later mapping; it is not id-numbered).  Area = km^2 of terrain triangles
of that id over the whole island (230.4 km^2 total).  Ids not listed do not occur on the terrain (the game has ~350 global ids).

Third field = KIND, what a ROAD over this id counts as (classify_roads.py / the viewer's road layer): paved (asphalt, concrete), offroad (dirt, gravel,
grass, sand, snow, forest floor ...), water (puddle / sea- or shore-bed: says nothing about the road itself -> unknown) or other (unknown id).  The kind carries
the SAME confidence as the status of the entry (a 'reasoned' entry has a reasoned kind); the per-entry comment after `| KIND` says why and flags weak ones.

    from fh6surfaces import SURFACES, surface_name, surface_status, surface_kind
"""

CONFIRMED, SEEN, REASONED = 'confirmed', 'seen', 'reasoned'
PAVED, OFFROAD, WATER, OTHER = 'paved', 'offroad', 'water', 'other'

SURFACES = {
    # id: (name, status)                                           # area km2 ; evidence
    31:  ('Forest floor', CONFIRMED, OFFROAD),                 # confirmed in-game | 61.82 km2 ; user: forest floor (palette: forest litter / needles) | KIND offroad
    22:  ('Grass', CONFIRMED, OFFROAD),                        # confirmed in-game | 37.92 km2 ; user: grass | KIND offroad
    207: ('Shallow water / puddle', CONFIRMED, WATER),         # confirmed in-game | 36.58 km2 ; user: shallow water / puddle | KIND water: puddle / shallow water: a road sample here says nothing about the road surface -> unknown
    20:  ('Forest floor (brownish)', CONFIRMED, OFFROAD),      # confirmed in-game | 13.73 km2 ; user: forest floor, brownish variant | KIND offroad
    345: ('Snowy forest floor', CONFIRMED, OFFROAD),           # confirmed in-game | 8.04 km2 ; user: snowy forest floor between trees (NOT alpine rock) | KIND offroad
    340: ('Dead grass / dry brown ground', CONFIRMED, OFFROAD), # confirmed in-game | 7.97 km2 ; user: dead grass / dry brown ground (screenshot) | KIND offroad
    36:  ('Seabed (under water)', CONFIRMED, WATER),           # confirmed in-game | 7.53 km2 ; user: water, seabed under the water | KIND water
    9:   ('Asphalt', CONFIRMED, PAVED),                        # confirmed in-game | 7.20 km2 ; user: asphalt, parking area | KIND paved: asphalt
    230: ('Packed gravel / dirt', CONFIRMED, OFFROAD),         # confirmed in-game | 6.45 km2 ; user: packed gravel/dirt beside a puddle, docks/industrial (NOT riverbank stones) | KIND offroad: packed gravel/dirt
    39:  ('Seabed / shore', CONFIRMED, WATER),                 # confirmed in-game | 6.13 km2 ; user: seabed / shore | KIND water: shore/seabed: unknown for roads
    336: ('Forest floor', CONFIRMED, OFFROAD),                 # confirmed in-game | 4.71 km2 ; user: forest floor | KIND offroad
    211: ('Snow', SEEN, OFFROAD),                              # seen only (not reachable) | 3.64 km2 ; user: snow, outside the playable map, visual only | KIND offroad
    342: ('Bamboo forest floor', CONFIRMED, OFFROAD),          # confirmed in-game | 3.56 km2 ; user: bamboo forest floor | KIND offroad
    7:   ('Gravel / stones', CONFIRMED, OFFROAD),              # confirmed in-game | 3.23 km2 ; user: gravel/stones on an alpine lake shore (NOT a rice field, despite the palette) | KIND offroad: gravel
    56:  ('Ploughed field', CONFIRMED, OFFROAD),               # confirmed in-game | 3.11 km2 ; user: ploughed field | KIND offroad
    41:  ('Snow / alpine', SEEN, OFFROAD),                     # seen only (not reachable) | 3.01 km2 ; user: snow/alpine, outside the playable map, looks right | KIND offroad
    339: ('Forest floor', CONFIRMED, OFFROAD),                 # confirmed in-game | 1.85 km2 ; user: forest floor | KIND offroad
    19:  ('Forest clearing (felled trees)', CONFIRMED, OFFROAD), # confirmed in-game | 1.57 km2 ; user: forest clearing with felled trees | KIND offroad
    8:   ('Asphalt (elevated deck)', CONFIRMED, PAVED),        # confirmed in-game | 1.49 km2 ; user: asphalt on the elevated Tokyo expressway interchange (bridge deck) | KIND paved: asphalt; also on 73 km of ordinary-looking nav roads (not only the expressway); 61% of those road samples have a second terrain layer below them, i.e. road decks (bridges / ramps) carry their own collision
    23:  ('Grass (festival site)', SEEN, OFFROAD),             # seen only (not reachable) | 1.26 km2 ; user: festival-site grass, seen but not reachable | KIND offroad
    27:  ('Concrete', SEEN, PAVED),                            # seen only (not reachable) | 1.21 km2 ; user: concrete, industrial area, seen but not reachable | KIND paved: concrete
    208: ('Sand / shore', REASONED, OFFROAD),                  # reasoned (not checked in-game) | 0.92 km2 ; spot at y 96 m (sea level is ~100); sibling of the confirmed seabed/shore ids 36, 39 | KIND offroad: sand; a road on a beach counts as off-road
    280: ('Dirt track or road verge (weak)', REASONED, OFFROAD), # reasoned (not checked in-game) | 0.89 km2 ; lies on roads (0 m from a road); 281 turned out to be a dirt track, so "verge" is doubtful | KIND offroad: WEAK: 81 km of nav roads, neighbours are vegetation/dirt ids (17, 239, 279, 282), rarely 9; sibling of confirmed dirt 281
    279: ('Dirt / farm track', REASONED, OFFROAD),             # reasoned (not checked in-game) | 0.81 km2 ; earlier class guess "dirt / farmland"; no other hint | KIND offroad: dirt track family (279-283)
    46:  ('Forest floor (steep)', REASONED, OFFROAD),          # reasoned (not checked in-game) | 0.80 km2 ; earlier class guess "forest"; only on steep ground (no flat patch to visit) | KIND offroad
    53:  ('Forest floor', REASONED, OFFROAD),                  # reasoned (not checked in-game) | 0.71 km2 ; earlier class guess "forest"; small patches | KIND offroad
    40:  ('Snow road', REASONED, OFFROAD),                     # reasoned (not checked in-game) | 0.56 km2 ; on alpine roads; megatexture palette road_alp_snw_flat | KIND offroad: AMBIGUOUS: a snow-covered road may be asphalt underneath; counted off-road (snow track), per the user's "snow track" wording
    43:  ('Dirt / gravel', REASONED, OFFROAD),                 # reasoned (not checked in-game) | 0.49 km2 ; earlier class guess "dirt / farmland / gravel"; no other hint | KIND offroad
    328: ('Snow / alpine rock (weak)', REASONED, OFFROAD),     # reasoned (not checked in-game) | 0.49 km2 ; earlier class guess "snow", but the spot is at y 121 m (near sea level) - doubtful | KIND offroad
    281: ('Packed dirt track', CONFIRMED, OFFROAD),            # confirmed in-game | 0.47 km2 ; user: packed dirt track at the junction of a rallycross-style dirt circuit (NOT road verge) | KIND offroad: confirmed dirt track
    60:  ('Sand / shore', REASONED, OFFROAD),                  # reasoned (not checked in-game) | 0.44 km2 ; spot at y 101 m (sea level); sibling of 36, 39, 208 | KIND offroad: sand
    183: ('Snow / alpine rock', REASONED, OFFROAD),            # reasoned (not checked in-game) | 0.40 km2 ; earlier class guess "snow / alpine rock"; spot at y 524 m next to a parking area | KIND offroad
    331: ('Sand / shore', REASONED, OFFROAD),                  # reasoned (not checked in-game) | 0.27 km2 ; spot at y 102 m (sea level); sibling of 36, 39, 208 | KIND offroad: sand
    239: ('Vegetation / undergrowth (weak)', REASONED, OFFROAD), # reasoned (not checked in-game) | 0.26 km2 ; earlier class guess "forest / vegetation"; nothing more specific | KIND offroad
    286: ('Asphalt (dark rural)', REASONED, PAVED),            # reasoned (not checked in-game) | 0.19 km2 ; on rural roads; megatexture palette road_gen_asp_darkrural_g | KIND paved: road palette asphalt
    17:  ('Unknown (vegetation?)', REASONED, OFFROAD),         # reasoned (not checked in-game) | 0.17 km2 ; next to a bridge landmark; earlier class guess "forest"; no real evidence | KIND offroad: WEAK: 1100 road samples, mostly next to 280 - assumed dirt road through vegetation
    346: ('Lawn / golf course grass', REASONED, OFFROAD),      # reasoned (not checked in-game) | 0.16 km2 ; earlier class guess "lawn / golf"; spot beside a car-parking area | KIND offroad
    10:  ('Asphalt (urban variant)', REASONED, PAVED),         # reasoned (not checked in-game) | 0.11 km2 ; on a job-route road (10 m from it); sibling of confirmed asphalt 8 - assumed asphalt variant | KIND paved: asphalt variant; 97% of the airfield/oddity nav class 8
    242: ('Road verge / dirt (weak)', REASONED, OFFROAD),      # reasoned (not checked in-game) | 0.05 km2 ; beside roads; see 280 | KIND offroad: WEAK, see 280
    26:  ('Unknown (road shoulder?)', REASONED, OTHER),        # reasoned (not checked in-game) | 0.05 km2 ; beside a parking area; no real evidence | KIND other
    199: ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.034 km2 ; too small to survey, no evidence | KIND other
    236: ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.022 km2 | KIND other
    18:  ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.022 km2 | KIND other
    107: ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.022 km2 | KIND other
    178: ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.017 km2 | KIND other
    282: ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.014 km2 | KIND other: on roads next to 280 but no evidence -> unknown
    29:  ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.011 km2 | KIND other
    283: ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.011 km2 | KIND other
    28:  ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.006 km2 | KIND other
    32:  ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.004 km2 | KIND other
    229: ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.004 km2 | KIND other
    171: ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.002 km2 | KIND other
    293: ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.002 km2 | KIND other
    238: ('Unknown', REASONED, OTHER),                         # reasoned (not checked in-game) | 0.001 km2 | KIND other
}


def surface_name(i):
    return SURFACES[i][0] if i in SURFACES else None


def surface_status(i):
    return SURFACES[i][1] if i in SURFACES else None


def surface_kind(i):
    return SURFACES[i][2] if i in SURFACES else OTHER

