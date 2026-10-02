"""Terrain surface-id -> name table for the 54 material ids that occur on FH6's terrain (`.phys` collision meshes, see fh6-terrain.md).

The game's own id -> name table (`Physics/surfaceTypes.xml`) is ENCRYPTED in the install, so these names are OURS.  Each entry says how
sure we are (plan decision D9: "use your best guess, but mark that they are reasoned"):

    'confirmed'  the user drove to a spot of this id and looked / checked it in-game (survey, 2026-10-02)
    'seen'       the user saw the spot in the game but it is not reachable / outside the playable map (visual check only)
    'reasoned'   our guess from where the id occurs (next to roads, coast, snow line, forest), megatexture-palette co-occurrence and the
                 neighbouring confirmed ids - NOT checked in-game.  Never present these as the game's names.

Names deliberately stay generic (the in-game surface vocabulary of `Audio/AudioSurfacesInfo.xml` - Asphalt_Smooth, Dirt_Gravel,
Dirt_Forest, Snow_Compact_Road, ... - is a good target for a later mapping; it is not id-numbered).  Area = km^2 of terrain triangles
of that id over the whole island (230.4 km^2 total).  Ids not listed do not occur on the terrain (the game has ~350 global ids).

    from fh6surfaces import SURFACES, surface_name, surface_status
"""

CONFIRMED, SEEN, REASONED = 'confirmed', 'seen', 'reasoned'

SURFACES = {
    # id: (name, status)                                           # area km2 ; evidence
    31:  ('Forest floor', CONFIRMED),                              # confirmed in-game | 61.82 km2 ; user: forest floor (palette: forest litter / needles)
    22:  ('Grass', CONFIRMED),                                     # confirmed in-game | 37.92 km2 ; user: grass
    207: ('Shallow water / puddle', CONFIRMED),                    # confirmed in-game | 36.58 km2 ; user: shallow water / puddle
    20:  ('Forest floor (brownish)', CONFIRMED),                   # confirmed in-game | 13.73 km2 ; user: forest floor, brownish variant
    345: ('Snowy forest floor', CONFIRMED),                        # confirmed in-game | 8.04 km2 ; user: snowy forest floor between trees (NOT alpine rock)
    340: ('Dead grass / dry brown ground', CONFIRMED),             # confirmed in-game | 7.97 km2 ; user: dead grass / dry brown ground (screenshot)
    36:  ('Seabed (under water)', CONFIRMED),                      # confirmed in-game | 7.53 km2 ; user: water, seabed under the water
    9:   ('Asphalt', CONFIRMED),                                   # confirmed in-game | 7.20 km2 ; user: asphalt, parking area
    230: ('Packed gravel / dirt', CONFIRMED),                      # confirmed in-game | 6.45 km2 ; user: packed gravel/dirt beside a puddle, docks/industrial (NOT riverbank stones)
    39:  ('Seabed / shore', CONFIRMED),                            # confirmed in-game | 6.13 km2 ; user: seabed / shore
    336: ('Forest floor', CONFIRMED),                              # confirmed in-game | 4.71 km2 ; user: forest floor
    211: ('Snow', SEEN),                                           # seen only (not reachable) | 3.64 km2 ; user: snow, outside the playable map, visual only
    342: ('Bamboo forest floor', CONFIRMED),                       # confirmed in-game | 3.56 km2 ; user: bamboo forest floor
    7:   ('Gravel / stones', CONFIRMED),                           # confirmed in-game | 3.23 km2 ; user: gravel/stones on an alpine lake shore (NOT a rice field, despite the palette)
    56:  ('Ploughed field', CONFIRMED),                            # confirmed in-game | 3.11 km2 ; user: ploughed field
    41:  ('Snow / alpine', SEEN),                                  # seen only (not reachable) | 3.01 km2 ; user: snow/alpine, outside the playable map, looks right
    339: ('Forest floor', CONFIRMED),                              # confirmed in-game | 1.85 km2 ; user: forest floor
    19:  ('Forest clearing (felled trees)', CONFIRMED),            # confirmed in-game | 1.57 km2 ; user: forest clearing with felled trees
    8:   ('Asphalt (elevated deck)', CONFIRMED),                   # confirmed in-game | 1.49 km2 ; user: asphalt on the elevated Tokyo expressway interchange (bridge deck)
    23:  ('Grass (festival site)', SEEN),                          # seen only (not reachable) | 1.26 km2 ; user: festival-site grass, seen but not reachable
    27:  ('Concrete', SEEN),                                       # seen only (not reachable) | 1.21 km2 ; user: concrete, industrial area, seen but not reachable
    208: ('Sand / shore', REASONED),                               # reasoned (not checked in-game) | 0.92 km2 ; spot at y 96 m (sea level is ~100); sibling of the confirmed seabed/shore ids 36, 39
    280: ('Dirt track or road verge (weak)', REASONED),            # reasoned (not checked in-game) | 0.89 km2 ; lies on roads (0 m from a road); 281 turned out to be a dirt track, so "verge" is doubtful
    279: ('Dirt / farm track', REASONED),                          # reasoned (not checked in-game) | 0.81 km2 ; earlier class guess "dirt / farmland"; no other hint
    46:  ('Forest floor (steep)', REASONED),                       # reasoned (not checked in-game) | 0.80 km2 ; earlier class guess "forest"; only on steep ground (no flat patch to visit)
    53:  ('Forest floor', REASONED),                               # reasoned (not checked in-game) | 0.71 km2 ; earlier class guess "forest"; small patches
    40:  ('Snow road', REASONED),                                  # reasoned (not checked in-game) | 0.56 km2 ; on alpine roads; megatexture palette road_alp_snw_flat
    43:  ('Dirt / gravel', REASONED),                              # reasoned (not checked in-game) | 0.49 km2 ; earlier class guess "dirt / farmland / gravel"; no other hint
    328: ('Snow / alpine rock (weak)', REASONED),                  # reasoned (not checked in-game) | 0.49 km2 ; earlier class guess "snow", but the spot is at y 121 m (near sea level) - doubtful
    281: ('Packed dirt track', CONFIRMED),                         # confirmed in-game | 0.47 km2 ; user: packed dirt track at the junction of a rallycross-style dirt circuit (NOT road verge)
    60:  ('Sand / shore', REASONED),                               # reasoned (not checked in-game) | 0.44 km2 ; spot at y 101 m (sea level); sibling of 36, 39, 208
    183: ('Snow / alpine rock', REASONED),                         # reasoned (not checked in-game) | 0.40 km2 ; earlier class guess "snow / alpine rock"; spot at y 524 m next to a parking area
    331: ('Sand / shore', REASONED),                               # reasoned (not checked in-game) | 0.27 km2 ; spot at y 102 m (sea level); sibling of 36, 39, 208
    239: ('Vegetation / undergrowth (weak)', REASONED),            # reasoned (not checked in-game) | 0.26 km2 ; earlier class guess "forest / vegetation"; nothing more specific
    286: ('Asphalt (dark rural)', REASONED),                       # reasoned (not checked in-game) | 0.19 km2 ; on rural roads; megatexture palette road_gen_asp_darkrural_g
    17:  ('Unknown (vegetation?)', REASONED),                      # reasoned (not checked in-game) | 0.17 km2 ; next to a bridge landmark; earlier class guess "forest"; no real evidence
    346: ('Lawn / golf course grass', REASONED),                   # reasoned (not checked in-game) | 0.16 km2 ; earlier class guess "lawn / golf"; spot beside a car-parking area
    10:  ('Asphalt (urban variant)', REASONED),                    # reasoned (not checked in-game) | 0.11 km2 ; on a job-route road (10 m from it); sibling of confirmed asphalt 8 - assumed asphalt variant
    242: ('Road verge / dirt (weak)', REASONED),                   # reasoned (not checked in-game) | 0.05 km2 ; beside roads; see 280
    26:  ('Unknown (road shoulder?)', REASONED),                   # reasoned (not checked in-game) | 0.05 km2 ; beside a parking area; no real evidence
    199: ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.034 km2 ; too small to survey, no evidence
    236: ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.022 km2
    18:  ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.022 km2
    107: ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.022 km2
    178: ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.017 km2
    282: ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.014 km2
    29:  ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.011 km2
    283: ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.011 km2
    28:  ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.006 km2
    32:  ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.004 km2
    229: ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.004 km2
    171: ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.002 km2
    293: ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.002 km2
    238: ('Unknown', REASONED),                                    # reasoned (not checked in-game) | 0.001 km2
}


def surface_name(i):
    return SURFACES[i][0] if i in SURFACES else None


def surface_status(i):
    return SURFACES[i][1] if i in SURFACES else None

