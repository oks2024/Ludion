# Plan: HUD motion and shapes — tweens, world anchors, rotation, clipping (Neon Shroud request 0020)

> Status: **Waves 1, 2, 4a mechanized (2026-11-18)** — `UiGroup` (`group`, `fill_group`; scale, opacity,
> visibility composed through up to 8 levels; CPU only, only the component schema snapshot moved);
> `UiTween` and `UiWorldAnchor`, resolved in `extract`
> (`ui/hud.rs`; the component and schema snapshots moved once, no golden, replay or protocol hash);
> `UiRect.uv_rect` and `UiRect.slice` (nine-slice as nine emitted rects, `ui::nine_slice`; only the
> component schema snapshot moved). Waves 3, 4b and 5–7 are recorded below and not started. Owed: the
> `ui-nine-slice` gallery golden on a pinned box. Human look owed: a live sitting on a world-anchored
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
12. **Sampling fields.** `UiRect.uv_rect: Option<[f32; 4]>` is `[u0, v0, u1, v1]`, top-left origin, in the
    texture's or the atlas region's own 0..1 space. `UiRect.slice: Option<UiSlice>`, `UiSlice { border:
    [f32; 4], scale: f32, center: UiSliceCenter }`: `border` is `[left, top, right, bottom]` in source
    texels of the texture or region; `scale` is canvas units per source texel for the corners (default
    `1.0`); `center` is `stretch | hidden` (default `stretch`). Both fields are skipped when absent, so a
    rect without them serializes, snapshots and hashes byte-identically.
13. **`uv_rect` mapping:** the sampled uv is `lerp(region_uv, uv_rect)` per axis; `region_uv` is `FULL_UV`
    for a loose texture and the atlas region's uv otherwise.
14. **Nine-slice geometry** (pure fn, decision 17). If `l + r > rw`, `l` and `r` scale by `rw / (l + r)`;
    likewise `t + b` against `rh` (`clamped`). Pixel corners are `l·scale·ps` (likewise `t r b`); if
    `lp + rp > w` both scale by `w / (lp + rp)`, likewise vertically (`clamped`); the uv split is
    unchanged by the pixel clamp. Edges are computed once and shared by neighbours:
    `xs = [x, x+lp, x+w−rp, x+w]`, `us = [u0, u0+(l/rw)(u1−u0), u1−(r/rw)(u1−u0), u1]`, `ys`/`vs`
    likewise. Pieces are emitted row-major; `center: hidden` skips the centre; a zero-width or
    zero-height piece is skipped. `extract`'s stable `(order, id)` sort keeps the emission order.
15. **Region size in texels** is `((uv[2]−uv[0])·tex.width).round()`, likewise height, from the resolved
    texture.
16. **Diagnostics.** `L1612` (error): a `uv_rect` value non-finite or outside `[0, 1]`, or `u0 ≥ u1` /
    `v0 ≥ v1`; a negative or non-finite `slice.border` value; a `slice.scale` non-finite or `≤ 0`.
    `L1613` (warning, static): `uv_rect` beside `slice` (slice wins), or either field on a rect with
    neither `texture` nor `atlas`. `L2301` (degraded warning, keyed per entity): a border exceeds its
    region, or the corners exceed the rect at the authored scale; drawn clamped.
17. **Pure twins:** `ludion_engine::ui::nine_slice(rect_px, uv, region_texels, &UiSlice, pixel_scale) ->
    NineSlice` and `ludion_engine::ui::sub_uv(region, uv_rect)`, re-exported from `ludion::math`.

18. **`UiGroup` fields:** `anchor`, `offset`, `size` (defaults as `UiRect`: top_left, `[0,0]`, `[100,100]`),
    `scale: f32` (default 1, skipped at 1), `opacity: f32` (default 1, skipped at 1), `visible: bool` (default
    true, skipped when true), `group: Option<EntityId>` (nesting). It draws nothing. `rotation` arrives with W3,
    `clip` with W4b (both recorded as request differences).
19. **`group` field** on `UiRect`, `UiText`, `UiList` and `UiGroup`: an optional entity ref, skipped when none.
    The scene range check, `scene.save` remap and the presentation rule (`L1036`) come free from the marker.
    `UiRect.fill_group: bool` (skipped when false) covers the group's box, as `fill` covers the canvas.
20. **Resolution = canvas resolve + a translation.** Resolving against a box `B = [bx, by, bw, bh]` px instead of
    the canvas differs only by `group_delta(anchor, B, viewport) = [bx + f.x*(bw - cw), by + f.y*(bh - ch)]`
    with `f` the anchor fraction and `cw`, `ch` the canvas in px. No resolve function changes: the element
    resolves and emits as today, then its instances move by `group_delta` (`ludion::math::group_delta`).
21. **Group transform.** Each group resolves to a box in one shared layout space (`resolve_ui_rect` with the
    tween's offset and size, plus `group_delta` against the parent's box; a top-level group's `UiWorldAnchor`
    adds its translation) and an affine `p' = p*scale + offset`:
    `affine(G) = affine(parent) o scale_about(centre(box), G.scale * tween scale)`. `opacity(G)` is the
    product down the chain, `visible(G)` the conjunction. A child moves by `group_delta` into the box, takes its
    own tween scale, then `affine(G)` and `opacity(G)`; an invisible group removes its instances.
22. **Order and composition.** `order` stays global; a group never reorders children. A `UiWorldAnchor` on a
    grouped element or a nested group is skipped with `L2300` (anchor the outermost group). `fill: true`
    ignores `group` and `fill_group` (`L1617`). A `color` tween track on a group is `L2300`.
23. **Depth and cycles.** A chain is at most 8 groups. A cycle is `L1615` and a deeper chain `L1616` (errors,
    from `check-scene`). At runtime the element draws ungrouped with the degraded warning `L2302` (a dead
    entity, one without `UiGroup`, a cycle or a chain past 8). `check-scene` also reports a static `group`
    ref to an entity without `UiGroup` as `L2302`.
24. **Validation codes:** `L1614` (error) `UiGroup.scale` non-finite or <= 0, `opacity` non-finite or outside
    `[0,1]`; `size` reuses `L1118`. `L1617` (warning) `fill_group` without `group`, or `fill` beside
    `group`/`fill_group`.

## Waves

- **W1 — `UiTween`, `UiWorldAnchor`, resolved in extract.** CPU-only; the pass lives in
  `ludion-engine/src/ui/hud.rs`, called from the UI phase of `extract`. Docs `ui-tweens`,
  `ui-world-anchor`; example `ui_world_anchor`. No existing golden, replay or hash moves: a world
  without the components extracts byte-identically.
- **W2 — `uv_rect` and nine-slice.** CPU-only: extract emits nine ordinary `UiInstance::Rect`s in place of
  one (`ui/hud.rs::emit_rect`); no renderer change. Docs `ui-nine-slice`; example `ui_nine_slice`. A rect
  without the fields extracts byte-identically.
- **W3 — rotation and `UiShape`.** Needs the renderer: a new instance and `ui_shape.wgsl`; waits for
  the renderer lock of the extract wedge.
- **W4a — `UiGroup`:** origin, scale, opacity and visibility, CPU only (decisions 18-24;
  `ui/group.rs`, `hud.rs::post`). Docs `ui-groups`; example `ui_groups`. An element without `group`
  extracts byte-identically.
- **W4b — the fragment clip and `math::ui_hit`:** a `UiInstance` field on both variants,
  `ui_rect.wgsl`/`ui_text.wgsl`, the `ui-groups` gallery golden; `ui_hit` walks groups, tweens, anchors
  and the clip.
- **W5 — text outline and shadow** (SDF, and the dilation fallback for the bitmap face).
- **W6 — `UiRadial` and `UiRadialNav`,** with the 1/2/8-thread replay and the snapshot test.
- **W7 — anchor blend keys under interpolation, `rotate_toward`, `distance.scale`, `camera`.**

## Proposed — owner ruling owed

Request amendments 1–7; the 12-o'clock clockwise angle convention (W3); the centre pivot for scale;
the `real` default clock; the separate `UiEase`; the interpolation gap (decision 8); W2: `uv_rect` beside `slice` warns and `slice` wins, stretch-only
edges, the proportional texel clamp rather than an error; W4a: the 8-level cap, `order` global (no block sort),
a world anchor only on the outermost group, `fill` ignoring `group`.

## Out of scope (triggers recorded)

`UiShape`, rotation, the group `clip` (W4b), nine-slice edge tiling (trigger: frame art that visibly breaks
when stretched), slicing `UiList`/`UiText`, text outline/shadow, `UiRadial`,
`math::ui_hit`, anchor blending, `rotate_toward`, `distance.scale`, the anchor `camera` field, gallery
goldens (a pinned box owes them), world `Text` outline (trigger: a request for it; see
`Plan/font-assets.md` for the SDF outline path).
