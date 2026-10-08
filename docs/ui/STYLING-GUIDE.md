# Styling Guide

How settings-style UI is laid out in ForzaTelemetryV3. Follow this for every tab
that shows grouped controls (Backfire, Automatic Gearbox, Co-Op, Power Curve, …)
so they stay visually consistent. The reusable helpers all live in `src/theme.rs`.

Chrome colours are referenced by role token (`ACCENT`, `PANEL`, `TEXT_DIM`, …) —
never hard-code hex at call sites.

## Panes: everything stays in its container (the layout rule)

**Every page column is its own pane, every category card is its own pane, and every
row inside a card is laid out in its card's width. Nothing may draw outside its pane.**

- **Page columns** — split a tab with **`theme::columns(ui, n, |cols| …)`**, never
  `ui.columns`. Each column is exactly `1/n` of the page (minus the 8px gaps), clipped
  to its own slot, and the row allocates exactly the page width.
- **Cards** — **`theme::card`** fills its column's width exactly, lays its body out in
  the card's inner width and clips it there. A too-wide row is cut at the card edge; it
  can never widen the card or paint into the next column.
- **Rows** — use stack / row layouts that size from `ui.available_width()`, never
  absolute coordinates or fixed pixel widths. When the pane is narrow a row must
  **shrink, wrap or stack**: slider rails shrink (the spinner keeps its reserved width),
  labels wrap, a row of choices wraps onto further lines (`theme::radio_group`). Label |
  control halves inside a card also use `theme::columns(ui, 2, …)` (`slider_row`,
  `checkbox_row_with` and the tabs' `control_row`s already do).
- **Custom-drawn widgets** (the Overlay tab's Layout grid and position pickers, …)
  take their size from `ui.available_width()` (a `.min(cap)` is fine) and paint only
  inside the rect they allocated — never a fixed size that can exceed the pane.
- The styled checkbox / radio (`mark_ui`) never sizes itself wider than the space the row
  has: a long label wraps instead.

The clip is the safety net, not the layout: if something gets cut at a pane edge, fix
its row so it fits (wrap / stack / shrink), don't widen the pane. The guarantee is per
pane: an overflowing row can't touch the next card or column, but *later rows in the
same card* are still laid out in the widened space egui gave it (a slider row after it
pushes its spinner past the edge, where it's clipped away).

*Why:* egui grows a `Ui` — and every `Ui` around it — to fit a widget that is wider
than the space it was given (`Region::expand_to_include_rect` widens `max_rect`, not just
`min_rect`), and plain `ui.columns` neither clips nor holds its width. So one too-wide
row made its whole card wider than its column and drew over the neighbouring column, and
every following row in that card was laid out in the widened rect. That was the Overlay
tab's Drift Counter bug: at three columns its *Position + Gain / Total score* radios were
wider than the right half, the card grew ~140px into the right column and its *Gain chip
interval* spinner disappeared under the Notifications card when the window was resized.
The user asked for "every column its own pane, every category its own pane", sized with
stack/row layouts, so that dynamic sizing happens only *inside* a pane and nothing is
placed freely in one big area.

Verified by `ui::test_render` (test-only): it runs a page in a headless `egui::Context`
with the app's fonts and theme, asserts nothing paints across a column, and with
`FORZA_UI_SNAPSHOT_DIR=<dir>` rasterises the frame to `<dir>/<name>.png` so a layout can
be looked at without opening a window — see `overlay_tab`'s `card_pages_stay_inside_their_panes` and `map_pages_stay_inside_their_panes`
(800 / 1100 / 1235 px). Dashboard modules are the exception: they're free-placed on the
Dashboard canvas by design (see *Dashboard modules* below).

## Categories (cards)

A **category** is a bordered card with a blue uppercase title that groups related
controls (see @docs/meta/TERMINOLOGY.md). Render one with:

```rust
crate::theme::card(ui, tr("RPM Range"), |ui| {
    // controls…
});
```

- **A pane** — the card is exactly as wide as its column and its body is clipped to the
  card's inner width (see *Panes* above). Don't `set_width` / `set_min_width` inside a
  card to make room for a wide row; make the row fit.
- **Title** — `theme::section_label(text)`: the accent colour (`ACCENT`), uppercased,
  size 12, bold. `card` draws it for you; when a title is needed outside a card
  (e.g. Power Curve chart headers) call `section_label` directly.
- **Uniform 8px spacing.** Cards are separated by exactly **8px** — vertically
  between stacked cards and horizontally between columns. `card` emits the 8px
  trailing gap itself; to keep it exact the caller MUST zero the container's
  vertical item spacing before stacking cards:

  ```rust
  ui.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
  ```

  Without the zero, egui adds its own spacing on top and the gaps balloon. `card`
  sets its own comfortable inner spacing, independent of that outer zero.
- **Do NOT `add_space(8.0)` above the first card.** The `CentralPanel` already
  supplies an 8px inner margin on every side, so a leading `add_space` *doubles*
  the top gap (~16px) while the sides stay at 8 — the first card then floats too
  far from the tab bar. Let the panel margin be the top gap so it matches the
  left/right inset.

## Two-column tab split

Tabs are split into two equal halves with an 8px gap:

```rust
ui.spacing_mut().item_spacing.x = 8.0; // inter-column gap
theme::columns(ui, 2, |cols| { // contained panes, never ui.columns
    // left half: controls (usually inside a vertical ScrollArea)
    // right half: live view, or left empty to keep controls narrow
});
```

The Automatic Gearbox uses controls | live-view. A tab with only one column of
controls (e.g. Backfire) should NOT reserve an empty second column — that leaves an
ugly dead half. Instead cap the content width so it reads as a settings panel and
still shrinks on a narrow window:

```rust
egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
    // At most half the tab (never dominates a slim window), capped at 520px.
    ui.set_max_width((ui.available_width() * 0.5).min(520.0));
    // cards…
});
```

## Control rows

Inside a card, lay controls out as **two-column rows**: a label in the left half, the
control in the right half. This keeps every row's control edge aligned.

### Slider rows

```rust
theme::slider_row(ui, tr("Packet Buffer Size"), &mut value, 0..=500, 10.0, 0, " ms");
```

`slider_row<N: Numeric>(ui, label, value, range, step, decimals, suffix)` draws the
label in the left half and, in the right half, a slider rail followed by a
**fixed-width value spinner**. Returns the combined response (`.changed()`).
It is generic over the numeric type (f32, u32, u64, …).

### Reserved spinner width (important)

The value spinner is always **`theme::VALUE_W` (72px)** wide — enough for the widest
value (`"100.0%"`). This is deliberate: a `DragValue` sizes to its text, so without a
reserved width it grows and shoves the row sideways as the value gains a digit
(e.g. `9.0%` → `100.0%`). Always reserve room for the **highest possible value**.

The spinner is **pinned to the right** of the row and the slider fills whatever
space is left (via a `right_to_left` layout), so as the window narrows the slider
shrinks but the value spinner never clips.

When you place a bare spinner yourself (not via `slider_row`), do the same:

```rust
ui.add_sized(
    [theme::VALUE_W, ui.spacing().interact_size.y],
    egui::DragValue::new(&mut value).range(0.0..=20000.0).speed(50.0),
);
```

Percentages show one decimal (`.fixed_decimals(1)` / the `decimals` arg) so a value
never jumps between `50%` and `50.0%`.

### Checkboxes — always the styled one

**Every checkbox in the app uses the custom styled checkbox, never a raw
`egui::Checkbox`.** The style: an 18px rounded box that is **accent-filled with a white
check** when on and a hairline steel **outline** when off, followed by the label, with
the *whole row* a single click target and a subtle hover wash. It's defined once in
`theme.rs` (`checkbox_ui`) and reached through these entry points — pick by context:

```rust
theme::checkbox_row(ui, &mut flag, tr("Enabled"));                 // category: half-width min
theme::checkbox_row_with(ui, &mut flag, tr("Dynamic…"), |ui| {     // + control on the right
    // e.g. a ComboBox in place of a slider
});
theme::styled_checkbox(ui, &mut flag, tr("Show grid"));            // content-sized (cog Mini-Settings)
theme::styled_checkbox_enabled(ui, &mut flag, label, enabled);     // + greyed/disabled when !enabled
```

- **`checkbox_row` / `checkbox_row_with`** — category cards. Span **at least the left
  half** (so short ones read a uniform width, with a control beginning where a slider's
  rail would) but never wider than the card — a long label extends to one line and only
  wraps if it would overflow. This clamp keeps a long checkbox (e.g. a "Test mode…" label)
  from dragging its card wider than the rest.
- **`styled_checkbox`** — content-sized; used where there's no two-column row, e.g. the
  cog **Mini-Settings** popup.
- **`styled_checkbox_enabled`** — same, plus an `enabled` flag: when false the row is
  dimmed (dim outline/check/label) and ignores clicks. Used for the Settings → Profiles
  **export/import tree**, where groups absent from a pasted JSON are shown greyed. The
  tree indents children with a leading `ui.add_space(16.0)` before the styled box.

All return the checkbox response (wrap in a tooltip `hover(...)` if needed). If you need a
checkbox anywhere, reach for one of these — do **not** hand-roll `egui::Checkbox::new`,
so the accent-box look stays uniform across the app.

### Radio buttons — the matching styled radio

For a one-of-N choice, use **`theme::styled_radio`**, never `ui.radio_value`. It shares the
checkbox's renderer (`mark_ui`), so it reads as the same family: the same 18px mark, accent
fill, hover wash and whole-row click target — but drawn as a **circle with a white centre
dot** when selected (hairline circle when not). Sets `*current = value` on click.

```rust
ui.horizontal(|ui| {                                              // group like radio_value
    theme::styled_radio(ui, &mut app.config.use_mph, false, "km/h");
    theme::styled_radio(ui, &mut app.config.use_mph, true,  "mph");
});
```

`current: &mut T` for any `T: PartialEq` (an enum, a `bool`, …); `value` is the option this
button represents. Content-sized — stack them or lay them out in a `ui.horizontal`. As with
checkboxes: do **not** fall back to `egui::RadioButton` / `ui.radio_value`, so the circle
matches the accent-box checkbox everywhere.

**Options with longer labels — `theme::radio_group`.** When the options might not fit
side by side (a card's right half in a narrow column), use the wrapping group instead of a
`ui.horizontal`: the options sit in one line while they fit and stack onto further lines
when they don't, so the row never leaves its pane (the Overlay tab's Drift Counter
*Style*).

```rust
theme::radio_group(ui, &mut o.drift_style, &[(DriftStyle::PositionGain, tr("Position + Gain")),
                                             (DriftStyle::Total, tr("Total score"))]);
```

**Column-aligning several radio rows** (e.g. the Display card's unit toggles — Speed unit,
Tire temp unit, Boost/pressure): each row is content-sized on its own, so a shorter first
label ("bar") leaves its second button sitting further left than a longer one ("km/h"), and
the rows don't read as columns. Fix it with **`theme::styled_radio_w`**, which takes an
explicit minimum width for the mark + label — give the *first* button of every row the same
`col_w`, measured from the widest first-position label, and leave the second button
content-sized:

```rust
let col_w = radio_col_width(ui, "km/h"); // BOX + GAP + widest label's text width
theme::styled_radio_w(ui, &mut app.config.use_mph, false, "km/h", col_w);
theme::styled_radio(ui, &mut app.config.use_mph, true, "mph"); // starts at the same x every row
```

`radio_col_width` (`settings.rs`) measures `BOX (18) + GAP (7) + text width` the same way
`mark_ui` sizes a content-sized mark, so the column is exactly as wide as it needs to be —
no magic-number padding.

### Confirm / input modals

Destructive or name-entry profile actions (New / Duplicate / Rename / Delete) use a **modal**:
a dim full-screen backdrop `egui::Area` (`Order::Middle`) that swallows clicks, plus a
centered `egui::Window` (`Order::Foreground`) holding the message or text field and a
primary + `secondary_button("Cancel")` pair — `danger_button` for the primary when the action
is destructive. Enter confirms, Esc or a backdrop click cancels. See
`profile_dialog_modal` in `settings.rs` (pattern borrowed from the Ritz launcher).

### No trailing colons on labels

Field and section labels do **not** end in a colon — write `tr("Listen port")`, not
`tr("Listen port:")`. *Why:* the colon was applied inconsistently across tabs (some rows
had it, most didn't); dropping it everywhere is the one consistent convention. The
control sits in the right column, so the colon adds nothing. This applies to slider,
combobox, checkbox, and text-input row labels alike.

### Row labels — use `theme::row_label`

Draw the left-column label of a two-column row with **`theme::row_label(ui, label)`**,
not a bare `ui.label`:

```rust
theme::columns(ui, 2, |c| {
    theme::row_label(&mut c[0], tr("Player color"));   // vertically centred, no letter-spread
    c[1].horizontal(|ui| { /* slider / combobox / spinner */ });
});
```

It solves two column gotchas at once:

- **Vertical alignment.** Columns top-align each column, so a bare label sits at
  the row's top edge while the taller control beside it (~`interact_size.y`) is
  centred — the label then floats above the control. `row_label` allocates the label a
  row of the standard control height and centres it, so the two line up. This is why
  every label + control row (and `styled_checkbox`, which now also occupies the standard
  control height) reads as one horizontal band.
- **Letter-spreading.** The columns' *justified* layout spreads a wrapping label's
  letters across the line; `row_label`'s `left_to_right` sub-layout wraps normally.

`slider_row` / `setting_row` and the gearbox tuning rows all build their label through
`row_label`, so fixing alignment is a one-place change, not a per-row edit.

### Comboboxes / other controls

For a plain label + control row, mirror `slider_row`'s split: `theme::columns(ui, 2, …)` with
`theme::row_label` in the left half and the control filling the right half via
`.width(ui.available_width())`.

### Segmented control — `theme::segmented`

For choosing exactly one of a few short, mutually exclusive options (e.g. the Co-Op
Session card's `Trystero | Cloudflare` transport). Not for long lists (use a combobox)
or on/off flags (use a checkbox).

```rust
let mut t = cfg.transport;
if theme::segmented(ui, &mut t, &[(A, tr("One")), (B, tr("Two"))]) { /* changed */ }
```

Look: a fully-rounded pill container (`FIELD` fill, `BORDER` stroke, 3px inner margin)
of equal-width segments. The selected segment is a smaller pill with `SEL` fill, a 1px
`SELBD` stroke and `TEXT`; unselected segments are `TEXT_DIM` on the container with a
`HOV` wash and a hand cursor on hover. It fills the available width and returns `true`
on the frame the selection changes. Wrap it in `ui.add_enabled_ui(..)` to lock it
(segments then show no hover/cursor). The Co-Op tab additionally sits it in a small
inner `Frame` (`WELL` fill, `BORDER` stroke, 8px radius) at the top of the card.

Why `SEL`/`SELBD`: they are the theme's accent-derived selection tints (also used by
page pills and text selection), so the control follows the accent rather than
hard-coding a colour.

## Right-bound value preview

The right edge of a row is the **value preview**: a spinner for numbers, or a small
fixed-width swatch/badge for non-numeric values (e.g. the Co-Op colour swatch sits
where a spinner would, to the right of the hue slider). Reserve a fixed width for it
so rows stay aligned.

## Text inputs

Text fields fill the available width (`.desired_width(ui.available_width())`) rather
than a fixed pixel width, so they don't get clipped inside a narrow card. Pin a
trailing button to the right with a `right_to_left` layout and let the field take the
rest.

## No helper text under options

Do not put explanatory text under (or beside) an individual option. Put it in a
**tooltip** on the control or its label, and keep helper text rare.

```rust
// checkbox / button / any control: attach to the returned Response
theme::checkbox_row(ui, &mut o.speed_hold, tr("Update speed only every 0.5 s"))
    .on_hover_text(tr("Calmer to read. Gear and revs stay live."));
// a row whose control is a closure: tooltip on the label (control_row_tip in the tabs)
control_row_tip(ui, tr("Listen port"), tr("Avoid ports 5200–5300 (used by the game)."), |ui| { /* … */ });
```

If the control already has a tooltip, merge the texts into one.

**Tooltips only where needed.** Add one only when the label alone doesn't explain the
control: units, side effects, interactions with other settings, non-obvious defaults. A
self-explanatory control ("North up", "Enabled", "Always on top") gets **no tooltip** — a
missing tooltip is fine. Never add one just to restate the label.

**Exception:** short text the user must see without hovering stays: a warning
(`DANGER` / `WARN`, e.g. a key bound twice, "HUD is hidden"), a runtime status (overlay
state, permission missing, a Test result, a live readout such as the backfire RPM range).
These describe a state, not an option.

**Why:** the user found text under every option bloated the UI (the Overlay tab in
particular). Tooltips keep the layout compact and the explanation is one hover away.
Tooltips everywhere are noise too, so only what needs explaining gets one.

## Dashboard modules

Dashboard modules don't use cards or rows: each fits its cell. Paint text (no wrapping
labels), scale all fonts uniformly to the pane, no minimum size, title truncates. Helpers
and the rationale: `docs/features/dashboard.md` → *Module sizing standard*.

## Fonts

The app renders in Geist Mono. Values/readouts stay monospace so columns line up;
fixed labels use the same face today. All user-facing strings go through `tr(...)`.
