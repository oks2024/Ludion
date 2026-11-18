---
description: UiRadial, the tool wheel — format, angle and selection rules, nav contract, radial helpers
---

# Tool wheels: `UiRadial`

**Scope:** `UiRadial` — up to 16 items on a ring, picked by stick angle, cursor or locked-mouse delta. It
shares `UiRect`'s canvas and pivot rule (`ludion docs screen-ui`) and `group` (`ludion docs ui-groups`). It
draws each item's label, detail line and icon; wedge fills and strokes are not drawn.

## The loop

```yaml
  - name: tools
    components:
      UiRadial:
        anchor: center
        items:
          - text: AXE
            detail: "x3"
            atlas: ui/icons.atlas.yaml   # optional icon, drawn above the label
            frame: axe
          - text: BOW
          - text: SHIELD
            enabled: false               # dimmed; aim and step skip it
```

Each frame, build a `UiRadialNav`, call `wheel.navigate(nav)`, act on the returned `RadialEvent`
(`Activated(i)` / `Cancelled`). `ludion check-scene` validates the wheel (`L1619`) and its icon atlases.

```rust
let nav = UiRadialNav {
    activate: pad.was_pressed(GamepadButton::South),
    ..UiRadialNav::from_stick(pad.left_stick()) // stick-up is +y, read as is
};
if let Some(RadialEvent::Activated(i)) = wheel.navigate(nav) { /* use item i */ }
```

Runnable: `cargo run -p ludion --example ui_radial`.

## Format

| Field | Default | Meaning |
| --- | --- | --- |
| `anchor`, `offset`, `group` | `top_left`, `[0,0]`, none | the box is `2 * radius_outer` square; the wheel centre is its centre |
| `radius_inner`, `radius_outer` | `90`, `220` | canvas units; `0 <= inner < outer` |
| `start_deg`, `gap_deg` | `0`, `2` | item 0's centre; empty degrees between sectors, `[0, 360/n)` |
| `items` | empty | `text`, `detail`, `atlas`, `frame`, `enabled (true)`; at most 16 |
| `selected`, `pointer` | none, `[0,0]` | hashed state; `navigate` writes only these two, only on change |
| `deadzone`, `hysteresis_deg` | `0.45`, `6` | `[0, 1)`; `[0, 360/n)` |
| `size`, `icon_size`, `font` | `24`, `48`, default face | label glyph height; icon edge; `fonts.yaml` face |
| `color`, `selected_color`, `disabled_color` | white, warm, grey | disabled outranks selected |
| `order`, `visible`, `outline` | `0`, `true`, none | as `UiList` |

An empty or invisible wheel draws nothing. Labels sit at the mid-radius on each item's centre angle; the
icon is `icon_size` square, `0.5 * (size + icon_size)` above that point; `detail` is `size` below at
`0.75 * size`. Icons take the item's colour as a tint.

## Rules

- **Angles are clock degrees:** 0 is 12 o'clock, clockwise positive. Item `i` is centred at
  `start_deg + i * 360/n`. An exact boundary belongs to the clockwise sector. `gap_deg` never affects
  stick selection, only drawing and `radial_hit`.
- **`navigate` order:** aim, then `step`, then `cancel` (beats `activate`), then `activate`. Aim reads
  `AimKind::Stick` (magnitude vs `deadzone`), `Cursor { dead_radius_px }` (pixels from the centre, `+y` down)
  or `Delta` (a mouse delta, `+y` down, summed into `pointer` and clamped to `radius_outer`).
  Inside the dead zone, `selected` holds.
- **Hysteresis:** the selected item is kept while the angle stays within its sector widened by
  `hysteresis_deg / 2` a side. Otherwise the pointed sector is taken only if enabled; a disabled target
  changes nothing.
- **`step`** walks `|step|` enabled items with wrap; from no selection the first step lands on the first
  enabled item.
- `Activated(i)` needs an enabled selection. A cancel or activation resets `pointer` to the centre and
  keeps `selected`. An empty wheel returns `None`; an out-of-range `selected` reads as none.
- There is no focus manager: the game picks which wheel to navigate and fills `step`, `activate` and
  `cancel` from its own bindings.
- **Helpers** (`ludion::math`): `radial_layout(&wheel, viewport)` gives the centre, radii and each item's
  `start_deg`, `sweep_deg` and `mid_px`; `radial_hit(&wheel, viewport, cursor_px)` gives the item under a
  cursor, `None` in the gap, the hole or outside, and returns disabled items. Both are ungrouped;
  `ui_hit` answers for a grouped wheel (its box).
- A `UiTween` or `UiWorldAnchor` on a radial is `L2300` and ignored: animate its `UiGroup`.
- **`L1619`** (error): non-finite number, bad radii, `gap_deg`/`hysteresis_deg` outside `[0, 360/n)`,
  `deadzone` outside `[0, 1)`, non-positive `size`/`icon_size`, or more than 16 items.

## Gotchas

- Write only on change: build `items` when the content changes, never per frame, or the world hash churns.
- A cursor offset is `+y` down, a stick `+y` up; `from_cursor` and `from_stick` already differ for that.

## Next

- `ludion docs ui-list` — the list widget this mirrors.
- `ludion docs ui-groups` — `group`, scale, opacity and clip.
- `ludion docs components-ui` — the component table.
