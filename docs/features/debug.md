# Debug

The last top-level tab (bug icon, `icons::BUG` = fa-bug U+F188). It shows every field of
the latest received `ForzaPacket` (`app.telemetry.latest`) raw, as a live, scrollable,
monospaced **name → value** grid inside one "Raw Telemetry" card, plus a **Copy** button
that puts the same `name: value` lines on the clipboard (handy for bug reports). With no
packet yet it shows "No telemetry yet".

## Where it lives

- `src/ui/debug_tab.rs` — `show()` and `fields(pkt)`.
- Wired in `src/app.rs` like every tab: `Tab::Debug`, `tab_title`, `max_pill_width`'s
  `TABS`, the tab bar's `right` array (first entry, so right-to-left layout puts it
  rightmost), and the `CentralPanel` dispatch. No mini-settings page.

## How the fields are enumerated

`fields()` formats the packet with its derived `{:#?}` and turns each `name: value,` line
into a row, in struct declaration order. Floats (a value containing `.`, `e`, `NaN` or
`inf`) are re-printed with 3 decimals; integers as-is. Field names are the Rust field
names and stay untranslated (they're data).

*Why:* the user asked for a "super simple" page with all raw readings. A hand-written list
of ~85 fields would go stale the first time `packet.rs` changes; the Debug output can't
drift. A unit test checks there's one row per field.
