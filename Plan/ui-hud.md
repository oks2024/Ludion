# Plan: HUD motion and shapes — tweens, world anchors, rotation, clipping (Neon Shroud request 0020)

> Status: **Wave 1 mechanized (2026-11-18)** — `UiTween` and `UiWorldAnchor`, resolved in `extract`
> (`ui/hud.rs`; the component and schema snapshots moved once, no golden, replay or protocol hash).
> Waves 2–7 are recorded below and not started. Human look owed: a live sitting on a world-anchored
> marker (jitter under `present_interpolation`, decision 8).

Neon Shroud request 0020 asks for rotation, shapes, clipping, tweens and world-anchored HUD
elements. This plan is the design of record. The fired triggers it answers: UI animation
(`Plan/phase-14-ui.md` deferred list), clipping and nine-slice (`Plan/ui-widgets.md` deferred list).

## Decisions

1. **Extract-only, per element.** The pass edits the element before it is resolved: the offset, size,
   colour and opacity tracks change a local copy of `UiRect`/`UiText`/`UiList`, and opacity multiplies
   the alpha of every colour. After the element is resolved, the pass changes its emitted instances:
   scale about the element's resolved box centre, then the world-anchor translation. No
   `RenderScene`/`UiInstance` shape changes. The box is the union of the element's emitted rects and
   glyph quads.
2. **Tween fields in W1** are `opacity`(1), `offset`(2), `scale`(1), `size`(2) and `color`(4); the
   number is the `from`/`to` arity. `opacity` and `offset` work on all three elements; `scale` and
   `color` on `UiRect`/`UiText`; `size` on `UiRect` only. An unsupported pairing is skipped with the
   degraded warning `L2300`, keyed per entity. It is not a scene error: a cross-component check would
   need `scene_check.rs`.
3. **Timing:** `start: u64` is a frame stamp, `duration` seconds (`>= 0`, finite).
   `t = (frame - start) * seconds_per_tick / duration`. Before `start` the track holds `from`; `once`
   holds `to` from `t >= 1`; `loop` uses `fract(t)`; `ping_pong` mirrors on odd periods;
   `duration == 0` is `to` from `start`. Arithmetic only. Tracks apply in list order; a later track
   on the same field wins.
4. **Clock:** `clock: real | game`, default `real` — the sim frame, which runs under a `GameClock`
   hold. `game` is the gameplay frame. Neither is wall time; `sim.pause` stops both. This is the
   exception to game-clock.md's "every stamp is a gameplay frame" rule, stated in the docs.
5. **`UiEase` is its own serde enum:** `linear smooth step in_quad out_quad in_out_quad in_cubic
   out_cubic in_out_cubic out_back` (`out_back` c1 = 1.70158). `smooth` is the clip evaluator's
   smoothstep. The clip `Ease` is hand-parsed with 3 variants, so the two are not unified;
   trigger: a clip asking for a cubic ease. Every ease is exactly 0 at 0 and 1 at 1.
6. **World-anchor placement:** the element's `anchor` becomes its pivot on the projected point and
   `offset` still adds in canvas units, as a translation delta
   `projected_px - canvas_anchor_point_px(anchor, viewport)` over every emitted instance. A `fill: true`
   rect ignores the anchor and raises `L2300`.
7. **Clamp rule (pinned):** the inset rect is the viewport inset by `margin * pixel_scale`; a
   degenerate inset places the point at the viewport centre, never on screen. The view-space point is
   `v = view * p`; the point is behind when `clip.w <= 1e-6` (perspective only). In front: the
   `world_to_pixel` formula; outside the inset the vector from the centre is scaled until it meets the
   inset. Behind: direction `(v.x, -v.y)`, a near-zero direction is `(0, 1)` (straight down), clamped
   the same way. `distance = sqrt(v . v)` through `ludion_core::math::sqrt`.
8. **Resolved against the camera as drawn** (after pixel-snap) and the target's world pose plus
   `world_offset`. A dead target or one with no `Transform` draws nothing, silently. **Known gap:** UI
   is never keyed in `BlendData`, so under `present_interpolation` an anchor follows the newest tick
   while the 3D scene is blended and can lead by up to one tick. Proposed for W7; trigger: visible
   jitter at a live sitting.
9. **`UiWorldAnchor` fields:** `target` (required entity ref; presentation to sim is allowed, the
   reverse is `L1036`), `world_offset`, `show: always | on_screen | off_screen`,
   `behind: clamp | hide`, `margin` (canvas units, default 48), `fade: [near, far]` metres (alpha 1 at
   `near`, 0 at `far`, multiplied into alpha). Deferred: `rotate_toward` (needs rotation, W3),
   `distance.scale` and `camera` (request 0017).
10. **Validation:** a `UiTween` arity mismatch, non-finite value or negative `duration` is `L1610`; a
    `UiWorldAnchor` with non-finite numbers, a negative `margin` or a `fade` without `0 <= near < far`
    is `L1611`. Both are errors with JSON pointers into the track or field.
11. **Pure twins** for sim logic and tests: `ludion_engine::ui::ui_tween_sample`,
    `place_world_anchor`, `canvas_anchor_px`, and the facade `ludion::math::world_anchor`.

## Waves

- **W1 — `UiTween`, `UiWorldAnchor`, resolved in extract.** CPU-only; the pass lives in
  `ludion-engine/src/ui/hud.rs`, called from the UI phase of `extract`. Docs `ui-tweens`,
  `ui-world-anchor`; example `ui_world_anchor`. No existing golden, replay or hash moves: a world
  without the components extracts byte-identically.
- **W2 — `uv_rect` and nine-slice.** CPU-only (nine emitted rects).
- **W3 — rotation and `UiShape`.** Needs the renderer: a new instance and `ui_shape.wgsl`; waits for
  the renderer lock of the extract wedge.
- **W4 — `UiGroup`:** origin, scale, opacity and visibility; then the fragment clip and `math::ui_hit`,
  which walks groups, tweens and anchors.
- **W5 — text outline and shadow** (SDF, and the dilation fallback for the bitmap face).
- **W6 — `UiRadial` and `UiRadialNav`,** with the 1/2/8-thread replay and the snapshot test.
- **W7 — anchor blend keys under interpolation, `rotate_toward`, `distance.scale`, `camera`.**

## Proposed — owner ruling owed

Request amendments 1–7; the 12-o'clock clockwise angle convention (W3); the centre pivot for scale;
the `real` default clock; the separate `UiEase`; the interpolation gap (decision 8).

## Out of scope (triggers recorded)

`UiShape`, rotation, `UiGroup`/clip, nine-slice, `uv_rect`, text outline/shadow, `UiRadial`,
`math::ui_hit`, anchor blending, `rotate_toward`, `distance.scale`, the anchor `camera` field, gallery
goldens (a pinned box owes them), world `Text` outline (trigger: a request for it; see
`Plan/font-assets.md` for the SDF outline path).
