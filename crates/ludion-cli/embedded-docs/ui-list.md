---
description: UiList, the selectable-menu widget — format, the cursor/activation contract, windowing
---

# Selectable lists: `UiList`

**Scope:** `UiList` — a vertical stack of selectable text lines with one cursor, the engine's one
menu surface (a purchase list, a pause menu, a dialogue choice). It shares `UiRect`'s anchor/offset
model and pivot rule; the canvas coordinate model, `UiRect` and `UiText` themselves are
`ludion docs screen-ui`.

`UiList` is its own panel: no separate frame entity. Like `UiRect` and `UiText` it has a `visible` flag.

## Format

```yaml
  - name: shop-menu
    components:
      UiList:
        anchor: center
        title: WARES
        items:
          - text: "SALVE (30 XP)"
          - text: "RELIC (120 XP)"
            enabled: false          # unaffordable — dimmed, and the cursor skips over it
          - text: "LEAVE"
        selected: 0
        max_visible: 0          # 0 = unlimited (default)
        size: 28.0
        font: body              # optional — a face named in fonts.yaml (`ludion docs fonts`)
        line_spacing: 6.0
        padding: 20.0
        color:
          - 0.9
          - 0.9
          - 0.85
          - 1.0
        selected_color:
          - 1.0
          - 0.85
          - 0.2
          - 1.0
        disabled_color:
          - 0.4
          - 0.4
          - 0.4
          - 1.0
        background:
          - 0.05
          - 0.05
          - 0.08
          - 0.85
        order: 0
```

- `title` draws above the items and is never selectable; each item is `{ text, enabled }` — an
  authored item defaults to `enabled: true`. `size` is the glyph cell height (as `UiText.size`);
  `line_spacing` is the gap *between* lines, so `0.0` stacks them exactly like a multi-line `UiText`.
  `padding` grows `background` outward from the measured title+items block on all four sides without
  moving the text — width is the longest line's advance width (glyph count × `size` on the embedded face), height is
  `lines * size + (lines - 1) * line_spacing`.
- **An empty `items` vec draws nothing at all — background and title included**, the same convention
  as `UiText`'s empty string carried one step further. `visible: false` (default `true`) hides the
  whole list too, orthogonal to `items`: it is the authored way to keep a list that exists from the
  start off until it is wanted.
- `background` follows the same alpha convention as everywhere else: alpha `0` (the default) draws no
  panel. When it does draw, the panel and every line share the list's `order`, and the panel is
  emitted first, so it is always behind its own text.

## Rules

- **`max_visible` windows a long list — default `0` is unlimited.** Set above `0`, and once the list
  holds more items than that, only `max_visible` are drawn, in a window recomputed every frame from
  `selected`: the cursor is kept centred (`start = selected − max_visible/2`) and clamped at both
  ends, so there is no scroll state to desync — it's pure arithmetic, `ui_list_window(&list) ->
  Range<usize>` is the exact function extract uses, and a game can call the same one at
  `ludion::math::ui_list_window` (so can `measure_ui_list` and `resolve_ui_list`). `navigate` doesn't change: the cursor still walks every item, wraps at the ends and skips
  disabled ones — the window just follows it. The measured block follows the window too: height
  counts the title plus the *windowed* items only, but width still runs over the title and every
  item, visible or not, so the panel never changes width while scrolling. Nothing marks a hidden item
  above or below the window — there is no scroll cue — a list that needs one draws its own, typically
  a `UiText` driven from `ui_list_window`.
- **The cursor and the activation are two different kinds of thing.** `selected` is an ordinary
  hashed component field: it survives frames, rides a snapshot and a scene switch, and shows in a
  golden. Activation is **the return value of `UiList::navigate`**, never a field — a value that
  lives one call gets no clearing protocol to forget. A `selected` past the item count is clamped
  when drawn, never a diagnostic (a list a game shrinks mid-frame must not be able to author an
  error); an authored `selected` sitting on a disabled item draws in `disabled_color`, which
  outranks `selected_color`.
- **Pointer hit-testing** is `ludion::math::list_hit(face, list, viewport, cursor) -> Option<u32>` —
  `face` is the list's resolved `&FontFace` (`world.resource::<FontLibrary>().cloned().unwrap_or_default()`, then `.face(list.font.as_deref())`);
  `cursor` in render-target pixels (origin top-left, `+Y` down, what `mouse().position()` reports),
  `viewport` the extent the session renders at. Read that extent from the world —
  `world.resource::<RenderExtent>().copied().unwrap_or_default().size()` — never a constant: a host's
  `--render-size`/`LUDION_RENDER_SIZE` or a client's `render.size` moves it, and a hard-coded
  `(1280, 720)` then resolves a click against pixels nothing is drawn at. (`RenderExtent` is host
  state — never hashed, never snapshotted — and a recording's mouse events are render-target pixels,
  so a session recorded at one extent is replayed at that same extent.) It walks the identical
  geometry `extract` draws, so a hit and the drawn line never disagree (a list with `visible: false`
  draws nothing and never hits: `None`), and it returns the item under
  the cursor **whether or not that item is enabled** — the same refusal split as everywhere else in
  `UiList`: the hit test only answers what is under the cursor, and the caller decides whether a
  disabled item's hit counts.
- **Driving the cursor** is a two-step call, forced by the same borrow discipline every system uses:
  read `InputState`, drop the borrow, then query `&mut UiList`.

  ```rust,ignore
  use ludion::prelude::*;

  fn drive_menu(world: &mut World, _ctx: &SystemCtx) {
      let nav = {
          let input = world.resource::<InputState>().unwrap();
          UiNav::from_input(input)
      };
      for (list,) in world.query::<(&mut UiList,)>() {
          if let Some(index) = list.navigate(nav) {
              // `index` is the activated item — apply it to your own game state here.
          }
      }
  }
  ```

  `UiNav::from_input` reads fixed default bindings: `Key::Up`/`GamepadButton::DpadUp` moves to the
  previous item, `Key::Down`/`GamepadButton::DpadDown` to the next, `Key::Enter`/`Key::Space`/
  `GamepadButton::South` activates the item under the cursor, and `Key::Escape`/`GamepadButton::East`
  sets `back` (leave the menu; `navigate` ignores it, the game acts on it). It reads **discrete edges
  only** (`was_pressed`, never `is_held`) — one press is one step, and a held direction does not
  repeat. A game wanting different keys builds a `UiNav { prev, next, activate, back }` by hand (the
  fields are public), or defines the reserved actions `ui_prev`, `ui_next`, `ui_activate` and
  `ui_back` in its input map and calls `UiNav::from_actions(&actions, input)`: each field reads its
  action's `pressed`, and a field whose action is undefined keeps the `from_input` default, so a
  rebind reaches every menu.
- **`next`/`prev` wrap at the ends and skip disabled items.** Stepping past the last item lands on
  the first and vice versa; an item with `enabled: false` is never a stepping destination. A list
  whose items are all disabled keeps its cursor exactly where it was and `navigate` never returns
  `Some` for it, no matter how many times `activate` fires.
- **Exactly one list is navigated per frame — the engine has no focus manager.** There is no group
  id, no tab order, and no engine-owned system anywhere in a game's schedule: the game's own phase
  state decides which `UiList` is the live one, and calls `navigate` on that entity only. Two
  simultaneously-navigable lists on one screen is out of scope for `UiList` as shipped.
- **Write only on change — this is the rule, not a suggestion.** `items` is variable-length data
  inside a hashed component: rebuilding it from fresh `String`s every frame churns the `WorldHash`
  for no reason, even while the menu just sits there. Build `items` when the menu's *content*
  changes (a new price, a newly-unlocked ware) and leave the field alone every other frame. The
  cursor field moves only through `navigate`, so an untouched menu already costs nothing; the
  `items` rebuild is the one write a game has to gate itself.

## Gotchas

- `line_spacing` and `padding` carry an ordinary schema lower bound of `0.0` — a negative value is
  `L1014` at the field. `size` itself must be strictly positive and finite — `L1118`, the same rule
  `UiRect.size` and `UiText.size` carry; see `ludion docs screen-ui`.
- Without `font`, the list uses the `default` face of `fonts.yaml`, else the embedded 8×8 face, whose glyph set is: lowercase folds to uppercase, an unmapped
  character renders blank — the same rule for `UiList` items as for `UiText`. A named face has its
  own charset (`ludion docs fonts`).
- An item or title is always one row: a line break inside it draws as a space in measurement,
  `list_hit` and drawing alike. Multi-line content goes in separate items.
- A `Transform` on a UI entity is simply ignored — screen space has no world placement.

## Next

- `ludion docs screen-ui` — `UiRect`/`UiText`: the canvas, anchors, draw order, and text wrapping.
- `ludion docs fonts` — the `font` field: baked faces and proportional measurement.
- `ludion docs systems` — the system contract and the determinism rules.
- `ludion docs components` — declaring components, and the built-in visual set.
