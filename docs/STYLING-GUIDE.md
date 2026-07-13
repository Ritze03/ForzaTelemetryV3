# Styling Guide

How settings-style UI is laid out in ForzaTelemetryV3. Follow this for every tab
that shows grouped controls (Backfire, Automatic Gearbox, Co-Op, Power Curve, …)
so they stay visually consistent. The reusable helpers all live in `src/theme.rs`.

Chrome colours are referenced by role token (`ACCENT`, `PANEL`, `TEXT_DIM`, …) —
never hard-code hex at call sites.

## Categories (cards)

A **category** is a bordered card with a blue uppercase title that groups related
controls (see @docs/TERMINOLOGY.md). Render one with:

```rust
crate::theme::card(ui, tr("RPM Range"), |ui| {
    // controls…
});
```

- **Title** — `theme::section_label(text)`: the accent colour (`ACCENT`), uppercased,
  size 12, bold. `card` draws it for you; when a title is needed outside a card
  (e.g. Power Curve chart headers) call `section_label` directly.
- **Uniform 8px spacing.** Cards are separated by exactly **8px** — vertically
  between stacked cards, at the top of the stack, and horizontally between columns.
  `card` emits the 8px trailing gap itself; to keep it exact the caller MUST zero
  the container's vertical item spacing before stacking cards:

  ```rust
  ui.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
  ui.add_space(8.0);                     // 8px above the first card
  ```

  Without the zero, egui adds its own spacing on top and the gaps balloon. `card`
  sets its own comfortable inner spacing, independent of that outer zero.

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
    ui.set_max_width(520.0); // comfortable settings width; fills when narrower
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

Checkboxes in a category span the **left half** (not sized to their label), so a
control to their right begins where a slider's rail would:

```rust
theme::checkbox_row(ui, &mut flag, tr("Enabled"));                 // half-width checkbox
theme::checkbox_row_with(ui, &mut flag, tr("Dynamic…"), |ui| {     // + control on the right
    // e.g. a ComboBox in place of a slider
});
```

Both return the checkbox response (wrap in a tooltip `hover(...)` if needed).
`theme::styled_checkbox` (content-sized) is still used for the cog **Mini-Settings**
popup, which is not a category.

### Comboboxes / other controls

For a plain label + control row, mirror `slider_row`'s split: `ui.columns(2, …)` with
the label in the left half (vertically centred against the control) and the control
filling the right half via `.width(ui.available_width())`.

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
