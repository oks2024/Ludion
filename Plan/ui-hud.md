# Plan: HUD motion and shapes — tweens, world anchors, rotation, clipping (Neon Shroud request 0020)

> Status: **Waves 1, 2, 4a, 4b, 5a, 6a mechanized (2026-11-18)** — W6a: `UiRadial` (decisions 37–44, `L1619`,
> `radial_layout`/`radial_hit`, `docs/ui-radial.md`; labels and icons only, wedges are W6b); W5a: `outline` on `UiText`/`UiList` (decisions 31–36,
> `L1618`, `L2304`, `ui_text.wgsl`; the `ui-text-outline` golden is blessed on the RTX 3090, the macOS golden is owed on the M4);
> `UiGroup` (`group`, `fill_group`; scale, opacity,
> visibility composed through up to 8 levels; CPU only, only the component schema snapshot moved);
> W4b: `UiGroup.clip` (`none | rect | circle | rounded_rect`, an analytic per-instance clip in
> `ui_rect.wgsl`/`ui_text.wgsl`, nesting cap 2, `L2303`) and `ludion::math::ui_hit`; the `ui-groups` gallery
> golden is blessed on the RTX 3090 (the macOS golden is owed on the M4);
> `UiTween` and `UiWorldAnchor`, resolved in `extract`
> (`ui/hud.rs`; the component and schema snapshots moved once, no golden, replay or protocol hash);
> `UiRect.uv_rect` and `UiRect.slice` (nine-slice as nine emitted rects, `ui::nine_slice`; only the
> component schema snapshot moved). Waves 3, 5b and 6–7 are recorded below and not started. Owed: the
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
   `distance.scale` and `camera` (request 0017; `Plan/secondary-views.md`).
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

25. **`UiGroup.clip`**, enum `UiClipShape` (externally tagged, snake_case, the `ProbeShape` pattern): `none`
    (default, skipped when none), `rect`, `circle`, `rounded_rect: { corner_radius: f32 }` (canvas units, finite
    and >= 0, else `L1614` at `/clip/rounded_rect/corner_radius`). YAML: `clip: rect` or
    `clip: { rounded_rect: { corner_radius: 12.0 } }`.
26. **One shape, a rounded box.** Every clip is a rounded box in screen px: `rect` has radius 0. `rounded_rect`
    has radius `corner_radius * ps * S`, clamped to half the box's shorter side. `circle` is the inscribed circle:
    a centred square of side `min(w,h)` with radius side/2. The box is the group's `box_px` under its own composed
    affine (`[x*S+Ox, y*S+Oy, w*S, h*S]`, `S`/`O` the `GroupXf` scale/offset after this group composes), so a
    pop-in scale tween shrinks the clip with the content.
27. **Renderer data:** `ludion_renderer::UiClipBox { rect_px, radius_px }` and
    `UiClip { boxes: [Option<UiClipBox>; 2] }` (innermost first, `UiClip::NONE`, `Default`). Both `UiInstance`
    variants gain `clip: UiClip`. Rects carry it per instance and text per vertex, so batching is unchanged.
28. **Coverage:** `d = rounded_box_sdf(frag_px, box, r)`, `cov = clamp(0.5 - d, 0, 1)`: pixel-exact on
    integer-aligned rect edges, a one-pixel AA band otherwise. Two boxes give the `min` of the two coverages
    (an intersection). An absent box gives exactly `1.0` through `select` (a flag, never a sentinel box) and
    multiplies the alpha, so an unclipped instance is bit-identical.
29. **Nesting cap 2.** An element keeps the two innermost clipping groups in its chain. A third or more is
    dropped, with the warning `L2303` (degraded at runtime, keyed `"{key}/ui-clip-depth"`, and static from
    `scene_ui_group_diagnostics`).
30. **`ui_hit`:** `ludion::math::ui_hit(world, entity, viewport, cursor, now) -> bool` is true when the cursor is
    in the half-open bounding box of the element's emitted instances after `hud::post` and inside its clip
    (`d < 0`, the same SDF in Rust, `ui::clip_contains`). A `UiRect` is tested as its one resolved rect
    (`hud::rect_box`, shared with extract); a `UiText` by its glyph quads; a `UiList` by its panel
    `layout.background_px` (use `list_hit` for the item index, ungrouped only). An entity with several UI
    components hits if any does. False for an invisible element or group, a world anchor that hides it, a dead
    entity, a non-finite cursor or a camera error. The camera is `resolve_camera(.., None)` without
    `pixel_snap`.

31. **`TextOutline { width, color }`** is in `ludion-components` (the shape `Plan/font-assets.md` decision 15
    reserved). `outline` on `UiText` and `UiList` is `#[serde(default, skip_serializing_if = "Option::is_none")]`.
    A `width` that is non-finite or below 0, or a non-finite `color`, is `L1618` at `/outline/width` or
    `/outline/color`. A width of `0` or a colour alpha of `0` draws nothing.
32. **Texels, resolved in extract** (`ui::resolve_outline`). `ppt` (screen px per atlas texel) is
    `size * ps * units_per_em / (em_px * line_units)`, invariant under a group scale. The width in texels is
    `t = width * ps / ppt`. SDF: `t` clamps to `spread_px - 1/ppt`. Bitmap: `k = clamp(round(t), 1, 4)` whole
    source pixels (at most 81 taps). Either clamp records the degraded warning `L2304`, keyed
    `"{key}/ui-outline-width"`, naming the face (its name, or `embedded`) and the widest width in canvas units.
33. **Renderer data:** `GlyphOutline { color, texels }` rides `UiInstance::Text.outline`, carried per vertex so
    batching is unchanged; `FontDraw` gains `spread_px` (0 for bitmap). The SDF edge is
    `e = 0.5 - texels / (2 * spread_px)`.
34. **Quads.** SDF quads already pad by the spread and are not grown. A bitmap quad with an outline grows by `k`
    texels per side in uv and pixels; the original glyph uv rect rides flat and every sample outside it reads 0,
    so a neighbouring atlas glyph never bleeds in.
35. **Shader composite** (fill over outline, straight alpha): `af = color.a * fill_cov`,
    `ao = outline.a * out_cov`, `a = af + ao * (1 - af)`, `rgb = (color.rgb * af + outline.rgb * ao * (1 - af)) / a`,
    output alpha times the clip. SDF `out_cov` is the same smoothstep at `e`; bitmap `out_cov` is the max of
    masked samples over the `(2k+1)^2` taps. An unoutlined vertex returns the previous expression through an early
    `if` on a flat flag after every uniform-flow sample, so text without an outline renders byte-identically.
36. **Group opacity** multiplies the outline alpha (`hud.rs::multiply_alpha`); tweens of `color` leave it alone.

37. **`UiRadial`** is a registered component in `ludion-components` (`radial.rs`), hashed on sim entities like
    `UiList`. Fields: `anchor`, `offset`, `group`; `radius_inner` (90), `radius_outer` (220); `start_deg` (0),
    `gap_deg` (2); `items: [{text, detail, atlas, frame, enabled}]`; `selected: Option<u32>`, `pointer: [f32; 2]`;
    `deadzone` (0.45), `hysteresis_deg` (6); `size` (24), `font`, `icon_size` (48); `color`, `selected_color`,
    `disabled_color`; `order`, `visible`, `outline`. Optional fields skip-serialize at their default. The box is a
    `2·radius_outer` square under the `UiRect` pivot rule.
38. **Angle convention:** clock degrees, 0 = 12 o'clock, clockwise positive. Up-positive `(x, y)` is
    `θ = atan2(x, y)` in degrees wrapped to `[0, 360)`; a cursor offset (+Y down) is read as `(x, −y)`. Item `i` is
    centred at `start_deg + i·span`, `span = 360/n`; the sector is `floor(rem_euclid(θ − start + span/2, 360) / span)`,
    so an exact boundary goes to the clockwise sector. `gap_deg` is drawing and hit-test only.
39. **`navigate(UiRadialNav) -> Option<RadialEvent>`** (`Activated(u32)`, `Cancelled`), in order: aim (`Stick`
    magnitude vs `deadzone`; `Cursor { dead_radius_px }`; `Delta` accumulates into `pointer`, clamped to
    `radius_outer`), deadzone hold, hysteresis (the selected sector widened by `hysteresis_deg/2` a side), a disabled
    target changes nothing; `step` walks enabled items with wrap, the first step from none landing on the first
    enabled item; `cancel` beats `activate`; either resets `pointer`, `selected` is kept. An empty wheel does nothing;
    an out-of-range `selected` reads as none. Maths is `ludion_core::math` only.
40. **`UiRadialNav { aim, aim_kind, step, activate, cancel }`**, `AimKind::{Stick, Cursor { dead_radius_px }, Delta}`,
    constructors `from_stick` (stick-up is +y, as `left_stick()` reports), `from_cursor`, `from_delta` (+y down). The
    game fills `step`/`activate`/`cancel`: no engine focus manager.
41. **`L1619`** (error): a non-finite number, `radius_inner < 0` or `radius_outer ≤ radius_inner`, `gap_deg` or
    `hysteresis_deg` outside `[0, span)`, `deadzone` outside `[0, 1)`, `size`/`icon_size` not positive and finite, more
    than 16 items. The span is checked only with items; one item is allowed.
42. **`radial_layout`/`radial_hit`** (`ui/radial.rs`, re-exported through `ludion::math`): pure, ungrouped.
    `radial_layout` gives the centre, radii and per item `{ start_deg, sweep_deg, mid_px }`; `radial_hit` the item whose
    sector (gap excluded, via `UiRadial::sector_hit`) and annulus `[r_in, r_out)` hold the cursor, disabled items
    included. `ui_hit` tests an ungrouped wheel with `radial_hit` and a grouped one by its group-placed box.
43. **Extract drawing:** per item an optional icon (`icon_size` square, `0.5·(size+icon_size)` above `mid_px`, from
    the atlas frame, tinted by the item colour), the `text` centred on `mid_px`, the `detail` centred `size` below at
    `0.75·size`. The colour is `disabled_color`, else `selected_color`, else `color`; `outline` applies to both lines.
    The set shares the radial's `(order, EntityId)` key and goes through `hud::post` (`HudElement::Radial`) with
    `group`. A `UiTween` or `UiWorldAnchor` on a radial is `L2300`: animate its group.
44. **Hash:** `navigate` writes only `selected` and `pointer`, only on change; an idle wheel is bit-identical, and a
    world without `UiRadial` extracts and hashes as before.

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
- **W5a — text outline** (decisions 31–36): SDF from the field, bitmap by in-shader dilation; `ui-text-outline` golden.
- **W5b — text shadow.**
- Order, proposed: W6a → W3 → W6b.
- **W6a — `UiRadial`, `UiRadialNav`, `radial_layout`/`radial_hit`** (decisions 37–44): CPU only, no renderer change;
  item text, detail and icons through the existing UI instances; the 1/2/8-thread replay and the snapshot test
  (`tests/ui_radial.rs`); `L1619`; docs `ui-radial`; example `ui_radial`. Only the component schema snapshot moved.
- **W6b — wedge fills and strokes** on W3's shape pipeline, after W3: `fill`, `selected_fill`, `stroke` fields and the
  `ui-radial` gallery golden. A rotated wheel is a rotated `UiGroup` (W3).
- **W7 — anchor blend keys under interpolation, `rotate_toward`, `distance.scale`, `camera`.**

## Proposed — owner ruling owed

Request amendments 1–7; the 12-o'clock clockwise angle convention (W3); the centre pivot for scale;
the `real` default clock; the separate `UiEase`; the interpolation gap (decision 8); W2: `uv_rect` beside `slice` warns and `slice` wins, stretch-only
edges, the proportional texel clamp rather than an error; W4a: the 8-level cap, `order` global (no block sort),
a world anchor only on the outermost group, `fill` ignoring `group`; W5a: the SDF max (spread less 1 px) with clamp-and-warn rather than an error, the bitmap dilation as a square kernel rounded with a minimum of 1, the cap of 4 source px, no outline on world `Text`, the shadow split to W5b; W4b: the 2-clip cap (innermost kept), circle
= the inscribed circle, the 1 px AA band, `ui_hit` on a list tests the panel, `ui_hit` ignores `pixel_snap`.
W6a: the clock-angle convention, the clockwise boundary rule, the gap ignored by stick selection, cancel beating activate,
the pointer reset on close, the 16-item cap and allowing one item, the first step from none landing on the first enabled item,
a delta aim read +y down, the icon placement and the `icon_size` and `detail` sizes (fields the request does not name), icons tinted
by the item colour, the W6a → W3 → W6b order.
Request differences: no rotation or polygon in `ui-groups` (W3); `ui_hit` does not return list items; W6a draws no wedge or stroke (W6b)
and adds `icon_size`, `selected_color`, `visible` and `outline`.

## Out of scope (triggers recorded)

`UiShape`, rotation, nine-slice edge tiling (trigger: frame art that visibly breaks
when stretched), slicing `UiList`/`UiText`, text shadow (W5b), `UiRadial` wedge and stroke drawing (W6b), sub-wheels, item-level `ui_hit`,
anchor blending, `rotate_toward`, `distance.scale`, the anchor `camera` field, gallery
goldens (a pinned box owes them), world `Text` outline (trigger: a request for it; see
`Plan/font-assets.md` for the SDF outline path).
