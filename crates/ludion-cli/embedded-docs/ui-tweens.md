---
description: UiTween: closed-form opacity, offset, scale, size and colour tracks on a screen-space element
---

# UI tweens

**Scope:** animating a `UiRect`, `UiText` or `UiList` without writing per-tick state. Placement is `ludion docs screen-ui`; following a 3D target is `ludion docs ui-world-anchor`.

## The loop

Add `UiTween` to the element's entity (a presentation entity is the usual home), `ludion check-scene`, `ludion render`. `extract` samples every track at the current frame; nothing is written to the world, so an element with no tween draws exactly as before.

```yaml
components:
  UiRect: { anchor: center, size: [200, 40] }
  UiTween:
    tracks:
      - { field: opacity, from: [0.0], to: [1.0], duration: 0.25, ease: out_quad }
      - { field: offset, from: [0, 24], to: [0, 0], duration: 0.25, ease: out_back }
```

## Format

| Track field | Default | Notes |
|---|---|---|
| `field` | required | `opacity` (1 number), `offset` (2), `scale` (1), `rotation` (1, degrees), `size` (2), `color` (4) |
| `from`, `to` | required | exactly the field's arity, finite (`L1610`) |
| `start` | `0` | frame stamp on the track's `clock`; before it the track holds `from` |
| `duration` | `1` | seconds, `>= 0` (`L1610`); `0` is `to` from `start` on |
| `ease` | `linear` | `linear smooth step in_quad out_quad in_out_quad in_cubic out_cubic in_out_cubic out_back` |
| `repeat` | `once` | `once` holds `to`; `loop` restarts; `ping_pong` mirrors on odd periods |
| `clock` | `real` | `real` is the sim frame, `game` the gameplay frame |

`opacity` multiplies the alpha of every colour the element draws (a list's panel included); `offset` adds to the element's `offset`; `size` and `color` replace the authored value; `scale` scales the drawn element about the centre of its box; `rotation` adds degrees (clockwise on screen) to the authored `rotation`. Tracks apply in list order, so a later track on the same field wins.

## Rules

- **Support:** `opacity` and `offset` work on every element; `scale` on `UiRect`, `UiText` and `UiGroup`; `rotation` on the same three (a list or radial takes none: tween its group); `color` on `UiRect` and `UiText`; `size` on `UiRect` and `UiGroup`. An unsupported track is skipped and reported once per entity as the degraded warning `L2300`.
- **The default clock is `real`**, not the gameplay clock: it keeps running while a `GameClock` hold freezes the game, so a pause menu can fade in over a frozen picture. `clock: game` stands still during a hold. This is the one stamp that does not want `ctx.game_frame` (`ludion docs game-clock`). Neither clock is wall time; `sim.pause` stops both.
- The value is exactly `from` at `start` and exactly `to` at `start + duration` for every ease. Pure sim logic and tests sample the same function: `ludion::math::ui_tween_sample`.

## Next

- `ludion docs ui-world-anchor` — pin an element to a 3D target.
- `ludion docs screen-ui` — the canvas and anchors a tween moves within.
- `ludion docs game-clock` — the two clocks.
- `ludion docs sequences` — a ui key writes a `UiTween` on a timeline.
