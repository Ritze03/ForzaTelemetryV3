# Recording & Replay

Records every incoming telemetry packet to a file for later analysis, and exports a
recording to CSV for a spreadsheet or pandas. There is **no in-app playback** — a
recording can't be fed back into the dashboard to watch it live again; export to CSV is
the only way to use one after the fact.

## How it works

- The control lives in the **Recording** card on the [[settings]] page.
- **Record** starts a new file at `<app data dir>/recordings/rec-<unix-timestamp>.ftr`.
  While recording, every packet the app receives is appended as-is (its raw 324-byte wire
  form, prefixed with an elapsed-time and length header) — the button becomes
  **Stop Recording** and shows a live packet count, plus a red "Recording live
  telemetry to a file…" notice.
- Recording is a manual, per-session toggle — it isn't tied to any config setting, doesn't
  auto-start, and doesn't resume after restarting the app.
- The `.ftr` file format is simple and append-only: repeated
  `[u32 LE elapsed_ms][u16 LE len][len bytes of packet]` records, one per received
  packet.

## Using it

1. Open **Settings** → **Recording**.
2. Click **Record** to start; click **Stop Recording** to finish. The file is flushed
   and closed automatically.
3. Existing recordings appear in a dropdown (newest first, named by their timestamp).
   Pick one and click **Export CSV** to write a `.csv` file next to it, or click the
   delete (×) button to remove both the `.ftr` and any exported `.csv`.

## CSV export

`Export CSV` reads back the `.ftr` file's packets and writes one row per packet with a
practical subset of fields for analysis: `t_ms, speed_kmh, rpm, gear, accel, brake,
steer, power_ps, torque_nm, boost_psi, fuel, pos_x, pos_y, pos_z, yaw, lat_g, long_g,
vert_g, tire_fl, tire_fr, tire_rl, tire_rr, cur_lap, last_lap`. On success a green
message shows the saved path; on failure it shows the error.

## Notes

- Recordings live under the app's data directory (`recordings/`, alongside `config.json`)
  — there's no setting to change that folder.
- Because capture only happens while packets arrive, a recording only contains data from
  moments you were actively driving (the game itself doesn't send packets from menus,
  pauses, or replays — see the packet format notes).
