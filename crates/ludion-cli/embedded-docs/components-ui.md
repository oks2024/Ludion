---
description: The UiRect/UiText/UiList screen-space UI and world-space Text component table
---

# UI components

**Scope:** the fields and defaults of the screen-space UI primitives and world `Text`, split out of
`ludion docs components` (registry rules and drift safety live there). The wider topics — the
reference canvas, anchors, draw order, and the selectable-menu widget's cursor/activation/windowing —
are `ludion docs screen-ui` and `ludion docs ui-list`.

| Component | Fields (default) |
|---|---|
| `UiRect` | `anchor` (`Anchor::TopLeft`), `offset: [f32;2]` (`[0,0]`, canvas units, **+Y down**) — the reference-canvas placement `UiText`/`UiList` also use, `size: [f32;2]` (`[100,100]`; a non-positive axis is `L1118`), `texture: Option<String>` (none → flat-colored quad) + `atlas: Option<String>` + `frame: Option<String>` (set together, never beside `texture`: `L1151`), `color: [f32;4]` (white), `order: i32` (`0`), `visible: bool` (`true`), `uv_rect: Option<[f32;4]>` (none — the `[u0,v0,u1,v1]` sub-rect sampled), `slice: Option<UiSlice>` (none — nine-slice `{border: [l,t,r,b] texels, scale (1), center (stretch|hidden)}`; wins over `uv_rect`; bad numbers `L1612`, ignored `L1613`, clamped `L2301`; see `ludion docs ui-nine-slice`) — a screen-space colored or textured rect, anchored to one of the canvas's nine points. See `ludion docs screen-ui` |
| `UiText` | `anchor` (`Anchor::TopLeft`), `offset: [f32;2]` (`[0,0]`, canvas units, **+Y down**), `text: String` (`""`), `size: f32` (`24.0`, glyph cell height; a non-positive value is `L1118`), `color: [f32;4]` (white), `order: i32` (`0`), `wrap_width: f32` (`0.0` = no wrap — greedy word-wrap otherwise, breaking at spaces and tabs and after `-`/`—` (never at U+00A0), hard-breaking an overlong word; `\n`, `\r\n` and `\r` are paragraph breaks), `font: Option<String>` (none → the table's `default`, else the embedded 8×8 face; `L1254` if unnamed in `fonts.yaml`), `visible: bool` (`true`) — a run of screen-space text, sharing `UiRect`'s coordinate model and pivot rule. See `ludion docs screen-ui` |
| `UiList` | `anchor`/`offset`/`title`/`items: [{text, enabled}]` (empty — draws nothing)/`selected: u32`/`max_visible: u32` (`0` = unlimited — windows the drawn items around `selected`)/`size: f32` (`24.0`, `L1118`)/`line_spacing`/`padding: f32` (`0.0`/`16.0`, `>=0`)/`color`/`selected_color`/`disabled_color`/`background: [f32;4]`/`font: Option<String>` (as `UiText`)/`order: i32`/`visible: bool` (`true`) — the selectable-menu list; screen-space only. See `ludion docs ui-list` |
| `UiTween` | `tracks: [{field, from, to, start (0), duration (1 s), ease (linear), repeat (once), clock (real)}]` — closed-form opacity/offset/scale/size/colour animation of a `UiRect`/`UiText`/`UiList` on the same entity; arity, non-finite or negative `duration` is `L1610`, an unsupported field `L2300`. See `ludion docs ui-tweens` |
| `UiWorldAnchor` | `target: EntityId` (required), `world_offset: [f32;3]`, `show` (`always`/`on_screen`/`off_screen`), `behind` (`clamp`/`hide`), `margin` (`48`), `fade: Option<[near, far]>` — pins the element to a 3D target and clamps it to the screen edge; a bad number is `L1611`. See `ludion docs ui-world-anchor` |
| `Text` | `text: String` (`""`), `size: f32` (`1.0`), `color: [f32;4]` (white; RGB `>=0`, alpha `0..=1`: `L1263`), `font: Option<String>` (as `UiText`) |

**World `Text` draws on top of the scene with no depth test, in scene order** — a wall in front of a label does not hide it, and two overlapping labels stack by entity order, not by distance. Fog, beams and DoF use the label's own distance, so a label seen through a wall is fogged as far as it actually is.

## Next

- `ludion docs components` — declaring/registering your own components, and the core scene/render set.
- `ludion docs components-fx` — the content and effect components (`Sprite`, `Tilemap`, particles, patterns).
- `ludion docs screen-ui` — the reference canvas, anchors, and draw order for `UiRect`/`UiText`.
- `ludion docs ui-list` — `UiList`: cursor, activation, and windowing.
- `ludion docs ui-tweens`, `ludion docs ui-world-anchor` — HUD motion components.
- `ludion docs fonts` — baked faces and the `font` field.
