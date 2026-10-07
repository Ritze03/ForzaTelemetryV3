# Contributing

Thanks for helping. Bugs and feature requests: open a [GitHub issue](https://github.com/Ritze03/ForzaTelemetryV3/issues), free form.

## Road types for the map

The app can show which kind of road each piece of the Forza Horizon 6 map is (road, offroad, highway, trail, tunnel, ...). Those labels are hand-checked and live in one file, `assets/map/fh6-road-types.json`. If you found a wrong road type or fixed some, you can send your version back.

### 1. Edit and export

1. In the app, open **Setup → Map data** and press **Open map editor** (it needs your own Forza Horizon 6 install; the map is built from it on your computer).
2. Check the roads and fix what is wrong. **Preview** mode and the **3D** view show how it will look.
3. Press **Save**. The app uses your edits right away. Then press **Export** to download the file `fh6-road-types.json`.

### 2. Send it

**Pull request (preferred).** Fork the repository, replace `assets/map/fh6-road-types.json` with your exported file and open a pull request. The file has one entry per line, so the diff shows only the entries you changed.

**New to pull requests?** A full file is fine too, either way:

- On GitHub, fork the repository, open the `assets/map` folder, choose **Add file → Upload files**, drop in your exported `fh6-road-types.json` (the file name must stay `fh6-road-types.json`) and commit it.
- Or open an issue and attach the exported file to it.

### What is in the file

Only road ids, their types and the points you placed by hand. It holds no game data, and your Forza files are never uploaded: the app reads them from your install on your computer.
