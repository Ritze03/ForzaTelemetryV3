# Styling Guide

How settings-style UI is laid out in ForzaTelemetryV3. Follow this for every tab
that shows grouped controls (Backfire, Automatic Gearbox, Co-Op, Power Curve, …)
so they stay visually consistent. The reusable helpers all live in `src/theme.rs`.

Chrome colours are referenced by role token (`ACCENT`, `PANEL`, `TEXT_DIM`, …) —
never hard-code hex at call sites.

## Categories (cards)

A **category** is a bordered card with a blue uppercase title that groups related
controls (see @docs/meta/TERMINOLOGY.md). Render one with:

```rust
crate::theme::card(ui, tr("RPM Range"), |ui| {
    // controls…
});
```

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
ui.columns(2, |cols| {
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
theme::slider_row(ui, tr("Buffer:"), &mut value, 0..=500, 10.0, 0, " ms");
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

### Checkbox rows

Checkboxes in a category span **at least the left half** (so short ones read a
uniform width, with a control beginning where a slider's rail would) but never wider
than the card — a long label extends to fit on one line and only wraps if it would
otherwise overflow. This clamp is what keeps a long checkbox (e.g. a "Test mode…"
label) from dragging its card wider than the rest:

```rust
theme::checkbox_row(ui, &mut flag, tr("Enabled"));                 // half-width checkbox
theme::checkbox_row_with(ui, &mut flag, tr("Dynamic…"), |ui| {     // + control on the right
    // e.g. a ComboBox in place of a slider
});
```

Both return the checkbox response (wrap in a tooltip `hover(...)` if needed).
`theme::styled_checkbox` (content-sized) is still used for the cog **Mini-Settings**
popup, which is not a category.

### Row labels — use `theme::row_label`

Draw the left-column label of a two-column row with **`theme::row_label(ui, label)`**,
not a bare `ui.label`:

```rust
ui.columns(2, |c| {
    theme::row_label(&mut c[0], tr("Color"));   // vertically centred, no letter-spread
    c[1].horizontal(|ui| { /* slider / combobox / spinner */ });
});
```

It solves two `ui.columns` gotchas at once:

- **Vertical alignment.** `ui.columns` top-aligns each column, so a bare label sits at
  the row's top edge while the taller control beside it (~`interact_size.y`) is
  centred — the label then floats above the control. `row_label` allocates the label a
  row of the standard control height and centres it, so the two line up. This is why
  every label + control row (and `styled_checkbox`, which now also occupies the standard
  control height) reads as one horizontal band.
- **Letter-spreading.** `ui.columns`' *justified* layout spreads a wrapping label's
  letters across the line; `row_label`'s `left_to_right` sub-layout wraps normally.

`slider_row` / `setting_row` and the gearbox tuning rows all build their label through
`row_label`, so fixing alignment is a one-place change, not a per-row edit.

### Comboboxes / other controls

For a plain label + control row, mirror `slider_row`'s split: `ui.columns(2, …)` with
`theme::row_label` in the left half and the control filling the right half via
`.width(ui.available_width())`.

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

## Fonts

The app renders in Geist Mono. Values/readouts stay monospace so columns line up;
fixed labels use the same face today. All user-facing strings go through `tr(...)`.
