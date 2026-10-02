# fh6-extract

Research scripts that read **map imagery, roads, POIs, race starts and terrain** from the user's own
Forza Horizon 6 install. Format details, validation and unknowns:
[`docs/game-data/fh6-game-files.md`](../../docs/game-data/fh6-game-files.md) and
[`docs/game-data/fh6-terrain.md`](../../docs/game-data/fh6-terrain.md).

- **Read-only** on the game install.
- Output is Playground Games' data: write it **outside the repo** (default `./fh6-out` is relative
  to your cwd — run from a scratch dir or pass `--out`). Never commit it.
- Deps: `python3`, `numpy`, `Pillow`. `scipy` is optional (speeds up `roaddist.py`; required only for
  the race-name heuristic with `--entity-model`). `lz4` (pip) is optional and speeds up PGZP reads.

All scripts take `--media <...>/steamapps/common/ForzaHorizon6/media` (default: auto-detect via
Steam's `libraryfolders.vdf`, see `fh6common.py`) and `--out DIR` (default `./fh6-out`).

| Script | Output | Notes |
|---|---|---|
| `extract_map.py [--level 3] [--seasons Summer,Winter]` | `map_<season>_L<level>.png` + `_preview.png` | BC1 swatchbin tiles → PNG. L3 = 8192², L2 = 4096² (fast; ~25 MB PNG). |
| `decode_nav.py [--map IMG]` | `roads.json`, `roads.png`, `roads_on_map.png` | `Brio_00.nav` road graph. `--map` (optional) = full-map image from `extract_map.py` for the overlay. |
| `extract_poi.py [--entity-model ZIP]` | `pois.json` | ~5.6k POIs from `.nt`/`.tz`/`gameobjs.xml`/`.owt`/route files (+ arenas, car meets, train line). `race_pin` = race map pin (NOT the start). `--entity-model` adds the creator-dump-only categories (photo spots, time attacks, chase starts…). |
| `extract_geochunk.py [--skip-pgeo]` | `geochunk_pois.json` | exact positions: `Ribbon_00/GameObjs.xml` (793 objects: speed traps/zones, drift zones, XP boards, …) + GeoChunk0 `.pgeo` (danger signs, drift-zone posts). Takes ~1 s. |
| `extract_races.py [--entity-model ZIP]` | `races.json` | start line, 12-slot grid, heading, finish for 169 routes from the `RVAN` block of `AITracks/Route<N>.nav`; names for ~15 routes (more with `--entity-model`). Optional validation vs `roads.json` if it is in `--out`. |
| `extract_terrain.py [--region X0,Z0,X1,Z1] [--res 4] [--surf-res 8] [--coarse] [--no-elevation] [--no-surfaces]` | `elevation.npy/.json/.png`, `surfaces.npy/.json/.png` | terrain height raster + surface-id raster from GeoChunk0. Full island: minutes and a few GB RAM — use `--region` (world metres) for a quick test, e.g. `--region -87,1460,1413,2960`. `--coarse` = whole-island low-detail elevation (~40 s). Surface class colours are **our** guesses, not game names. |
| `roaddist.py` | stdout | Per-type distance to nearest road (needs `roads.json` + `pois.json` in `--out`). |
| `plot_pois.py --map IMG` | `pois_on_map.png`, `pois_dense_on_map.png` | Needs `pois.json` in `--out`. |

Library modules (imported, not run): `fh6common.py` (install detection, case-insensitive paths, `.nt` / `.tz`
readers), `pgzp.py` + `lz4b.py` (reader for the 40 GB `GeoChunk*.minizip` PGZP containers — seek-reads single
entries), `fh6str.py` (string-table reader + key hash), `fh6bxml.py` (binary-XML decoder, only for `--entity-model`).

**`--entity-model PATH`** = an *older, readable* `Stripped/EntityModel.zip`. The one in the current install is
encrypted, so the data behind it is "creator dump only" and **not loadable from a user install** — the flag exists so
the research can be re-run, not as an app feature.

Typical run:

```
python3 extract_map.py     --out /tmp/fh6 --level 2 --seasons Summer
python3 decode_nav.py      --out /tmp/fh6 --map /tmp/fh6/map_summer_L2.png
python3 extract_poi.py     --out /tmp/fh6
python3 extract_geochunk.py --out /tmp/fh6
python3 extract_races.py   --out /tmp/fh6
python3 extract_terrain.py --out /tmp/fh6 --region -87,1460,1413,2960
python3 roaddist.py        --out /tmp/fh6
python3 plot_pois.py       --out /tmp/fh6 --map /tmp/fh6/map_summer_L2_preview.png
```

Coordinates in every output are telemetry space (`PositionX`/`PositionZ` metres, `y` = height).
