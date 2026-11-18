---
description: UiGroup: a screen-space box whose scale, opacity and visibility compose onto the UI inside it
---

# UI groups

**Scope:** a radar, a tool wheel, a meter and glyph that move as one. Placement is `ludion docs screen-ui`; motion is `ludion docs ui-tweens` and `ludion docs ui-world-anchor`.

## The loop

Spawn a `UiGroup`, then give each `UiRect`, `UiText`, `UiList` (or another `UiGroup`) `group: <that entity>`, then `ludion render`. The group draws nothing: it is a box resolved like a `UiRect` (`anchor`, `offset`, `size`), and a grouped element anchors against that box instead of the canvas. `Center` is the box's centre, `bottom_right` its corner.

```yaml
components:
  UiGroup: { anchor: top_right, offset: [-24, 24], size: [360, 360], opacity: 0.9 }
```

A child names it by entity ref: `UiRect: { anchor: center, size: [16, 16], group: { index: 4, generation: 0 } }`. A rect with `fill_group: true` covers the whole box, as `fill` covers the canvas.

## Fields

| Field | Default | Meaning |
|---|---|---|
| `anchor`, `offset`, `size` | `top_left`, `[0, 0]`, `[100, 100]` | the box, as for `UiRect` (`size` axis `<= 0` is `L1118`) |
| `scale` | `1` | uniform scale about the box centre, composed onto every descendant, `> 0` (`L1614`) |
| `opacity` | `1` | multiplied into every descendant's alpha, `0..=1` (`L1614`) |
| `visible` | `true` | `false` removes every descendant |
| `group` | none | the outer group this one nests in |

## Rules

- **Nesting composes:** scales and opacities multiply, boxes nest, `visible` is the conjunction. A chain is at most 8 groups: a deeper one, or a `group` cycle, is refused by `check-scene` (`L1616`, `L1615`).
- **`order` stays global.** A group never reorders its children; they sort by `(order, entity)` with everything else.
- **`UiTween` and `UiWorldAnchor` work on a group** (offset, size, scale, opacity; no `color`): a tween scale moves every child, a world anchor moves the whole group. Anchor only the outermost group: a `UiWorldAnchor` on a grouped element or a nested group is skipped (`L2300`).
- **`fill: true` ignores `group`** and `fill_group` (`L1617`); `fill_group` without `group` is also `L1617`.
- **A `group` that names a dead entity, an entity with no `UiGroup`, or a chain past 8** draws the element ungrouped and reports the degraded warning `L2302` once. `check-scene` reports a static reference to an entity without `UiGroup` as `L2302`.
- A grouped element's own `UiTween` still applies inside the box, before the group's scale.

## See also

- `ludion docs screen-ui` — the canvas, anchors and `order`.
- `ludion docs ui-tweens`, `ludion docs ui-world-anchor` — HUD motion.
- `crates/ludion/examples/ui_groups.rs` — a radar group with a pop-in tween.
