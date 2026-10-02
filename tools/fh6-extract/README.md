# fh6-extract

Research scripts that read **map imagery, roads and POIs** from the user's own Forza Horizon 6
install. Format details, validation and unknowns: [`docs/game-data/fh6-game-files.md`](../../docs/game-data/fh6-game-files.md).

- **Read-only** on the game install.
- Output is Playground Games' data: write it **outside the repo** (default `./fh6-out` is relative
  to your cwd — run from a scratch dir or pass `--out`). Never commit it.
- Deps: `python3`, `numpy`, `Pillow` (`scipy` optional, only speeds up `roaddist.py`).

All scripts take `--media <...>/steamapps/common/ForzaHorizon6/media` (default: auto-detect via
Steam's `libraryfolders.vdf`, see `fh6common.py`) and `--out DIR` (default `./fh6-out`).

| Script | Output | Notes |
|---|---|---|
| `extract_map.py [--level 3] [--seasons Summer,Winter]` | `map_<season>_L<level>.png` + `_preview.png` | BC1 swatchbin tiles → PNG. L3 = 8192², L2 = 4096² (fast; ~25 MB PNG). |
| `decode_nav.py [--map IMG]` | `roads.json`, `roads.png`, `roads_on_map.png` | `Brio_00.nav` road graph. `--map` (optional) = full-map image from `extract_map.py` for the overlay. |
| `extract_poi.py` | `pois.json` | ~5.5k POIs from `.nt`/`.tz`/`gameobjs.xml`/`.owt`. |
| `roaddist.py` | stdout | Per-type distance to nearest road (needs `roads.json` + `pois.json` in `--out`). |
| `plot_pois.py --map IMG` | `pois_on_map.png`, `pois_dense_on_map.png` | Needs `pois.json` in `--out`. |

Typical run:

```
python3 extract_map.py --out /tmp/fh6 --level 2 --seasons Summer
python3 decode_nav.py  --out /tmp/fh6 --map /tmp/fh6/map_summer_L2.png
python3 extract_poi.py --out /tmp/fh6
python3 roaddist.py    --out /tmp/fh6
python3 plot_pois.py   --out /tmp/fh6 --map /tmp/fh6/map_summer_L2_preview.png
```

Coordinates in `roads.json` / `pois.json` are telemetry space (`PositionX`/`PositionZ` metres).
