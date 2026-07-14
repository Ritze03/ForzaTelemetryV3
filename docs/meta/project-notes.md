# Project Notes

Cross-cutting domain facts and scope decisions that don't belong to a single feature.

## Domain notes

- **Shift indicator** uses *measured* max RPM (not `EngineMaxRpm`, which is a display
  limit); calibrated per car. Defaults: 91% low warning / 99% shift. See [[dashboard]].
- **Presets / mini-settings**: a preset is a subset of `AppConfig`. A new mini-setting
  that should travel with export/presets must be added to `MINISETTINGS_KEYS` in
  `config.rs`. See [[presets]] and [[state-and-config]].
- **FPS limiter** renders independently of packet rate. See [[settings]].
- MiniMap and Co-Op have deeper design notes in the auto-memory index. See [[minimap]]
  and [[coop]].

## Not in V3

Deliberately excluded (localhost works natively, so the old workarounds are gone):

- Data relay to another IP/port.
- Loopback companion app / `.bat` scripts.
- Android APK web server.
- Imgur screenshot upload.
