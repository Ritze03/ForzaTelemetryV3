#!/usr/bin/env python3
"""Build mapping.json (POI category -> icon) from xml_symbols.json + icons.json (written by extract_icons.py).
Usage: build_icon_mapping.py [ICON_DIR]   (= the --out dir of extract_icons.py, default '.'; writes ICON_DIR/mapping.json).
Result on the Sept-2026 build: 53 categories -> 38 derived, 7 guessed, 8 none (= 45 with an icon).  Icon<->tag links come from the game's
own MapProfiles XML; the 7 guesses are listed in docs/game-data/fh6-game-files.md ("Map icons") - they need a human look.

POI category names = the `type` values of tools/fh6-extract/extract_poi.py / extract_geochunk.py / extract_races.py.
basis:
  derived = the icon is what the game's own MapProfiles XML draws for a map-element type tag whose name matches the
            category (xml_types lists the tags). Icon<->tag link is from the XML; tag<->category link is by name.
  guessed = no map-element tag matches; icon picked by file name / pins folder. Needs a human look.
  none    = nothing sensible exists (category is geometry/internal only).
"""
import json, os, sys
d = sys.argv[1] if len(sys.argv) > 1 else '.'
sym = json.load(open(os.path.join(d, 'xml_symbols.json')))
icons = {e['name'] for e in json.load(open(os.path.join(d, 'icons.json')))}
A = 'atlas/ForteMapIconSheet/'
H = 'icons/MapIcons/'


def X(tag, contains):
    """first icon the XML draws for `tag` whose name contains `contains` (case-insensitive)"""
    for g in sym.get(tag, []):
        for i in g['icons']:
            if contains.lower() in i.lower():
                return i
    raise KeyError((tag, contains))


def G(name):
    assert name in icons, name
    return name


def cat(basis, icon, xml_types=(), variants=None, note=''):
    for i in [icon, *(variants or {}).values()]:
        assert i is None or i in icons, i
    out = dict(basis=basis, icon=icon)
    if xml_types: out['xml_types'] = list(xml_types)
    if variants: out['variants'] = variants
    if note: out['note'] = note
    return out


def pr(tag, base):  # PR stunt family: default + seasonal / live (Forzathon) / evolving world / gold
    return {'seasonal': X(tag, base + '_Seasonal'), 'live': X(tag, base + '_Live'),
            'evolving_world': X(tag, base + '_EvolvingWorld'), 'gold': X(tag, base + '_Gold')}


R = lambda t, p: X(t, p)
M = {}
M['speed_trap'] = cat('derived', X('ambient_speed_trap', 'pos_prstunt_speedcamera'), ['ambient_speed_trap'], pr('ambient_speed_trap', 'Icon_PRStunt_SpeedCamera'))
M['speed_zone'] = cat('derived', X('ambient_speed_zone', 'pos_prstunt_speedzone'), ['ambient_speed_zone'], pr('ambient_speed_zone', 'Icon_PRStunt_SpeedZone'))
M['danger_sign'] = cat('derived', X('ambient_danger_sign', 'pos_prstunt_dangersign'), ['ambient_danger_sign'], pr('ambient_danger_sign', 'Icon_PRStunt_DangerSign'))
M['drift_zone'] = cat('derived', X('ambient_drift_zone', 'pos_prstunt_driftzone'), ['ambient_drift_zone'], pr('ambient_drift_zone', 'Icon_PRStunt_DriftZone'))
tb = pr('ambient_trailblazer_gate_start', 'Icon_PRStunt_Trailblazer')
tb['end'] = X('ambient_trailblazer_gate_end', 'pos_prstunt_trailblazer_end')
M['trailblazer'] = cat('derived', X('ambient_trailblazer_gate_start', 'pos_prstunt_trailblazer_start'),
                       ['ambient_trailblazer_gate_start', 'ambient_trailblazer_gate_end'], tb, 'variants.end = finish gate icon')
M['xp_board'] = cat('derived', X('xp_board', 'pos_influenceboard_notfound'), ['xp_board', 'xp_board_collected'],
                    {'collected': X('xp_board_collected', 'pos_influenceboard_found')}, 'game calls them influence boards; both states are the same "H" tile (purple = undiscovered, grey = collected)')
M['fast_travel'] = cat('guessed', G('pins/discount_board_fasttravel'), ['travel_board', 'travel_board_collected'],
                       {'collected': G('pins/discount_board_fasttravel_collected')},
                       'XML type travel_board points at atlas cells pos_fasttravelboard_* that are BLANK in ForteMapIconSheet (FH6 reuses the xp-board tile); pins/ images are the pause-menu versions')
M['treasure_chest'] = cat('guessed', X('treasure_hunt', 'Icon_Treasure_Discovered'), ['treasure_hunt', 'treasure_hunt_collected'],
                          {'collected': X('treasure_hunt_collected', 'Icon_Treasure_Collected'), 'radius': X('treasure_hunt_region', 'Icon_Treasure_Radius_Icon')},
                          'icon is the Treasure Hunt chest; repo type treasure_chest = DISCOUNT_BOARD_TREASURE_CHEST_N objects, link by name only')
M['treasure_chest_board'] = dict(M['treasure_chest'])
M['mascot'] = cat('derived', None, [f'mascot_region_{n}' for n in range(1, 10)],
                  {f'region_{n}': X(f'mascot_region_{n}', '') for n in range(1, 10)} |
                  {f'region_{n}_collected': X(f'mascot_region_{n}_collected', '') for n in range(1, 10)},
                  'one mascot per region 1..9 (Ramen, Dango, Omurice, Curry, Matcha, Kakigori, Edamame, Onigiri, Tempura) -> pick variants.region_<mascot.region>')
M['house'] = cat('derived', X('player_house', 'Icon_PlayerHouse_Default'), ['player_house'],
                 {'owned': X('player_house', 'Icon_PlayerHouse_Owned'), 'home': X('player_house', 'Icon_PlayerHouse_Home')})
M['estate'] = cat('derived', X('player_house', 'Icon_Estate_Default'), ['player_house'],
                  {'owned': X('player_house', 'Icon_Estate_Owned'), 'home': X('player_house', 'Icon_Estate_Home'), 'activation': G('ext/Estate/Activation/EstateActivationMapIcon')},
                  'estate_activation XML type draws ext/.../EstateActivationMapIcon (which actually shows Hide&Seek art - leftover asset)')
M['estate_entrance'] = dict(M['estate'])
M['festival_site'] = cat('derived', X('festival_site', 'Icon_HorizonFestival_Main'), ['festival_site'],
                         {'legend_island': X('festival_site', 'LegendIsland')})
M['barn_find'] = cat('derived', G(H + 'BarnFind'), ['barn_find', 'barn_find_collected'],
                     {'gift': G(H + 'BarnFind_Gift'), 'gold': G(H + 'BarnFind_Gold'), 'radius_icon': G(H + 'Barnfind_Radius_Icon'), 'radius_mask': G(H + 'Barnfind_Radius_Mask')})
M['barn_find_hint'] = cat('derived', G(H + 'Barnfind_Radius_Icon'), ['barn_find_region'], note='search-radius badge (barn_find_region draws Barnfind_Radius_Icon inside the radius ellipse)')
M['photo_spot'] = cat('derived', X('photochallenge_region', 'Photo_Radius_Icon'), ['photochallenge_region'],
                      {'radius_mask': X('photochallenge_region', 'Photo_Radius_Mask')})
M['car_meet'] = cat('derived', X('car_meet_location', 'Icon_Car_Meet'), ['car_meet_location'],
                    {'seasonal': G(H + 'Icon_Car_Meet_Seasonal'), 'evolving_world': G(H + 'Icon_Car_Meet_EvolvingWorld')})
M['drag_meet'] = cat('derived', G(H + 'HorizonLife/Icon_DragEvent'), ['ambient_drag_event'],
                     {'gold': G(H + 'HorizonLife/Icon_DragEvent_Gold'), 'seasonal': G(H + 'HorizonLife/Icon_DragEvent_Seasonal'), 'evolving_world': G(H + 'HorizonLife/Icon_DragEvent_EvolvingWorld')})
M['time_attack'] = cat('derived', G(H + 'HorizonLife/Icon_Time_Attack'), ['ambient_time_attack'],
                       {'gold': G(H + 'HorizonLife/Icon_Time_Attack_Gold'), 'seasonal': G(H + 'HorizonLife/Icon_Time_Attack_Seasonal'), 'evolving_world': G(H + 'HorizonLife/Icon_Time_Attack_EvolvingWorld')})
M['drift_attack'] = cat('derived', G(H + 'HorizonLife/Icon_Drift_Attack'), ['ambient_drift_attack'],
                        {'gold': G(H + 'HorizonLife/Icon_Drift_Attack_Gold'), 'seasonal': G(H + 'HorizonLife/Icon_Drift_Attack_Seasonal'), 'evolving_world': G(H + 'HorizonLife/Icon_Drift_Attack_EvolvingWorld')})
M['treasure_car'] = cat('derived', X('treasure_car', 'TreasureCar'), ['treasure_car', 'treasure_car_collected'],
                        {'collected': X('treasure_car', 'TreasureCar_Collected')})
M['aftermarket_spot'] = cat('derived', X('aftermarket_car', 'AftermarketCar'), ['aftermarket_car', 'aftermarket_car_discovered'])
M['aftermarket_board'] = dict(M['aftermarket_spot'])
M['community_gift_shop'] = cat('derived', X('community_gift_event_shop_activation', 'Icon_Market'), ['community_gift_event_shop_activation'],
                               {'smashable': X('community_gift_smashable', 'Icon_Gift')})
M['playground_arena'] = cat('derived', X('playground_games_arena', 'Icon_PlaygroundGames_default'), ['playground_games_arena'],
                            {'seasonal': X('playground_games_arena', 'Icon_PlaygroundGames_Seasonal'), 'seasonal_active': G(H + 'PlaygroundGames/Icon_PlaygroundGames_Seasonal_Active')})
M['horizon_story'] = cat('derived', X('horizon_story', 'pos_story_background'), ['horizon_story'],
                         {k: X('horizon_story', v) for k, v in {'gold': 'Story_Background_Gold', 'seasonal': 'Story_Seasonal_Background', 'icons_of_japan': 'Icons_of_Japan',
                                                                'day_trips': 'Day_Trips', 'drift_club': 'Drift_Club', 'redline_magazine': 'Redline_Magazine',
                                                                'yujis_auto': 'Yujis_Auto', 'renewal': 'Renewal_Story', 'test_track': 'Icon_TestTrack'}.items()},
                         'background tile; the story-specific glyph is drawn on top (variants)')
M['story_activation'] = dict(M['horizon_story'])
M['horizon_job'] = cat('derived', X('horizon_job', 'Jobs_Background'), ['horizon_job'],
                       {k: X('horizon_job', v) for k, v in {'gold': 'Jobs_Background_Gold', 'seasonal': 'Jobs_Seasonal_Background', 'advertising': 'Advertising_Job',
                                                            'taxi': 'Taxi_Job', 'delivery': 'Delivery_Job'}.items()} | {'dropoff': X('job_dropoff', 'job_dropoff'), 'food_pickup': X('food_pickup', 'food_pickup')})
M['job_volume'] = dict(M['horizon_job'])
M['story_volume'] = dict(M['horizon_story'])
M['showcase'] = cat('derived', G(H + 'Showcases/Icon_Showcase_Mech'), ['showcase_mech', 'showcase_plane'],
                    {'planes': G(H + 'Showcases/Icon_Showcase_Planes'), 'mech': G(H + 'Showcases/Icon_Showcase_Mech')},
                    'also Showcases/*_Gold|_Locked|_Seasonal|_EvolvingWorld; other showcases (monster truck, jet ski, canyon train, russian dolls) have only pins/Showcase_Icons/*')
M['rush_event'] = cat('derived', G(H + 'Rush/Icon_Rush_Docks'), ['rf:is_horizon_rush'],
                      {'docks': G(H + 'Rush/Icon_Rush_Docks'), 'ski': G(H + 'Rush/Icon_Rush_Ski'), 'rocket': G(H + 'Rush/Icon_Rush_Rocket')},
                      'Horizon Rush = Docks / Ski / Rocket; each also _Gold, _Locked, _Seasonal, _Evolving')
M['horizon_chase_start'] = cat('derived', G(H + 'Icon_HorizonChase'), ['horizon_chase_location'])
M['hide_seek_arena'] = cat('guessed', G(H + 'HideSeek/event_brand'), ['hide_seek_spawn_location'],
                           {'seeker': G(H + 'HideSeek/seeker_icon'), 'hider': G(H + 'HideSeek/hider_icon'), 'hider_find': G(H + 'HideSeek/hiderfind_icon')})
M['upsell'] = cat('derived', X('festival_pass_upsell_car', 'PlaylistCar'), ['festival_pass_upsell_car', 'car_pack_upsell_car'],
                  {'car_pack': X('car_pack_upsell_car', 'CarPackCar'), 'inactive': X('festival_pass_upsell_car', 'PlaylistCar_Inactive')})
M['special_event'] = cat('guessed', G(H + 'Icon_HallOfFame'), ['hall_of_fame_activation', 'community_challenge_activation', 'horizon_tour', 'stunt_party', 'evolving_world_location'],
                         {'hall_of_fame': G(H + 'Icon_HallOfFame'), 'super7': X('community_challenge_activation', 'Icon_Super7_ChallengeCard'), 'horizon_tour': G(H + 'Icon_Horizon_Tour'),
                          'stunt_party': G(H + 'Icon_HorizonArcade'), 'evolving_world': G(H + 'Icon_Evolving_World'), 'event_lab': G(H + 'Icon_EventLab'), 'midnight_battle': G(H + 'Icon_Midnight_Battle'), 'test_track': G(H + 'Icon_TestTrack')},
                         'catch-all: repo special_event has no sub-kind field here; pick by what it is')
# races: surface/circuit variants -> atlas cells of ForteMapIconSheet, taken from the campaign-event groups
rv = {}
for key, tag, pos in [('asphalt_p2p', 'rf:is_campaign_asphalt_p2p', 'asphalt_p2p_default'), ('asphalt_circuit', 'rf:is_campaign_asphalt_circuit', 'asphalt_circuit_default'),
                      ('crosscountry_p2p', 'rf:is_campaign_crosscountry_p2p', 'crosscountry_p2p_default'), ('crosscountry_circuit', 'rf:is_campaign_crosscountry_circuit', 'crosscountry_circuit_default'),
                      ('mixedsurface_p2p', 'rf:is_campaign_mixedsurface_p2p', 'mixedsurface_p2p_default'), ('mixedsurface_circuit', 'rf:is_campaign_mixedsurface_circuit', 'mixedsurface_circuit_default'),
                      ('dragracing', 'rf:is_campaign_dragrace', 'dragracing_default'), ('streetracing', 'rf:is_streetrace', 'streetracing_default'),
                      ('touge', 'rf:is_touge', 'touge_default'), ('midnight_battle', 'rf:is_midnightbattle', 'Icon_Midnight_Battle')]:
    rv[key] = X(tag, pos)
M['race_start'] = cat('derived', rv['asphalt_circuit'], [t for t in sym if t.startswith('rf:is_campaign_') or t in ('rf:is_streetrace', 'rf:is_touge', 'rf:is_midnightbattle')], rv,
                      'the map draws race pins by event class (ResourceFilter, not a `type` tag): pick variants.<surface>_<p2p|circuit>; default icon is only a fallback. '
                      'Also per class: <class>_finish (CampaignObjective/*) and Icon_RaceEvents_*_SeasonalChampionship/_EvolvingWorld variants')
M['race_pin'] = dict(M['race_start'])
M['race_finish'] = cat('derived', G(H + 'CampaignObjective/asphalt_finish'), ['rf:is_event_finished_not_complete'],
                       {'asphalt': G(H + 'CampaignObjective/asphalt_finish'), 'cross_country': G(H + 'CampaignObjective/cross_country_finish'), 'dirt': G(H + 'CampaignObjective/dirt_finish'),
                        'drag': G(H + 'CampaignObjective/drag_finish'), 'street': G(H + 'CampaignObjective/street_scene_finish'), 'touge': G(H + 'CampaignObjective/touge_finish'),
                        'milestone': G(H + 'CampaignObjective/milestone_finish')}, 'trophy badge drawn over a finished event')
M['touge_event'] = cat('derived', rv['touge'], ['rf:is_touge'])
M['touge_pin'] = dict(M['touge_event'])
M['ie_route'] = cat('guessed', G(H + 'Milestones/Icon_Milestone_InitialDrive'), ['initial_drive'])
M['drag_meet_finish'] = cat('guessed', G(H + 'CampaignObjective/drag_finish'), [])
# landmarks: one text-label icon per landmark, key = XML type tag landmark_{place|biome|town}_<slug>
M['landmark'] = cat('derived', None, [t for t in sym if t.startswith('landmark_')],
                    {t: sym[t][0]['icons'][0] for t in sym if t.startswith('landmark_')},
                    'per-landmark icon (name badge incl. text). Slug = lower-snake of the landmark name; repo landmark names need to be slugged to match variants keys')
for k in ('map_region', 'creature_zone', 'drift_zone_post', 'drift_circuit_prop', 'flag_rush_flag', 'train_line', 'legend_island_gate', 'barn_building_cell'):
    M[k] = cat('none', None, note='no map symbol (geometry / internal)')
json.dump(dict(_meta=dict(generated_by='build_mapping.py', icon_names='keys of icons.json (path inside Horizon_Map.zip without .swatchbin, or atlas/<Sheet>/<pos key>, or ext/<Zip>/...)',
                          basis='derived = icon taken from the game MapProfiles XML for a matching map-element type tag; guessed = chosen by name; none = no icon'),
               categories=M), open(os.path.join(d, 'mapping.json'), 'w'), indent=1)
import collections
print(collections.Counter(v['basis'] for v in M.values()), len(M))
