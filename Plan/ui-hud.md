# Plan: HUD motion and shapes — tweens, world anchors, rotation, clipping (Neon Shroud request 0020)

> Status: **Waves 1, 2, 3a, 3b, 4a, 4b, 5a, 6a, 6b mechanized (2026-11-21)** — W6b: `fill`/`selected_fill`/`stroke` wedges on
> `UiRadial` (decisions 62–66; the `ui-radial` golden is blessed on the RTX 3090, the macOS golden is owed on the M4); W3b: `UiShape` (`line circle ring arc wedge rounded_rect`, fill and
> stroke, decisions 53–61, `L1621`, `ui_shape.wgsl`, `ludion::math::ui_shape_hit`, `docs/ui-shapes.md`; the `ui-shapes` golden is blessed on
> the RTX 3090, the macOS golden is owed on the M4); W3a: `rotation` on `UiRect`/`UiText`/`UiGroup`
> (decisions 45–52, `L1620`, `UiTweenField::Rotation`, `ui_rect_corners`/`ui_point_in`, rotated clip and hit; the `ui-rotation`
> golden is blessed on the RTX 3090, the macOS golden is owed on the M4); W6a: `UiRadial` (decisions 37–44, `L1619`,
> `radial_layout`/`radial_hit`, `docs/ui-radial.md`; labels and icons, wedges came in W6b); W5a: `outline` on `UiText`/`UiList` (decisions 31–36,
> `L1618`, `L2304`, `ui_text.wgsl`; the `ui-text-outline` golden is blessed on the RTX 3090, the macOS golden is owed on the M4);
> `UiGroup` (`group`, `fill_group`; scale, opacity,
> visibility composed through up to 8 levels; CPU only, only the component schema snapshot moved);
> W4b: `UiGroup.clip` (`none | rect | circle | rounded_rect`, an analytic per-instance clip in
> `ui_rect.wgsl`/`ui_text.wgsl`, nesting cap 2, `L2303`) and `ludion::math::ui_hit`; the `ui-groups` gallery
> golden is blessed on the RTX 3090 (the macOS golden is owed on the M4);
> `UiTween` and `UiWorldAnchor`, resolved in `extract`
> (`ui/hud.rs`; the component and schema snapshots moved once, no golden, replay or protocol hash);
> `UiRect.uv_rect` and `UiRect.slice` (nine-slice as nine emitted rects, `ui::nine_slice`; only the
> component schema snapshot moved); W3c-1: filled `polygon` (decisions 67–73, `L2305`, `ui_polygon.wgsl`; the `ui-polygons`
> golden is blessed on the RTX 3090 and its macOS twin on the M4, r2455); W3c-2: polygon strokes and `polyline` (decisions 74–80, `UiLineJoin`,
> `push_ui_stroke_verts` on the polygon pipeline; the `ui-strokes` golden is blessed on the RTX 3090 and its macOS twin on the M4, r2455). W3c-3: `dash` (decisions 81–87; the `ui-dashes` golden is owed on the RTX 3090 and its macOS twin on the M4). Waves 5b and 7 are recorded below and not started. Owed: the
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
   while the 3D scene is blended and can lead by up to one tick. **Accepted by the owner 2026-11-19**; blend keys in W7, trigger: visible
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

45. **Field.** `rotation: f32` on `UiRect`, `UiText`, `UiGroup`: degrees, skipped when zero. Positive is clockwise on the
    +Y-down canvas: `R(θ) = [[c, −s], [s, c]]` applied to `(x, y)`, `θ = deg.to_radians()`. A non-finite value is
    `L1620` at `/rotation`. `UiList` and `UiRadial` take no `rotation`; they rotate only through a group.
46. **Pivot.** An element rotates about the centre of the bounds of its emitted instances (`hud::bounds`, the pivot
    `scale` uses), after its tween scale and world-anchor translation. A nine-slice's nine pieces share it. Text pivots
    on the glyph quads' bounds. A group rotates about the centre of its `box_px`.
47. **Tween.** `UiTweenField::Rotation` (arity 1, degrees) is added to the authored rotation, like `offset`. Supported on
    `UiRect`, `UiText`, `UiGroup`; on a list or radial it is `L2300`.
48. **Renderer data.** `ludion_renderer::UiRotation { radians, pivot_px }` (`NONE` = zero) on both `UiInstance` variants.
    Geometry stays axis-aligned; the drawn point is `pivot + R(radians)(p − pivot)`. Rects rotate in the `ui_rect.wgsl`
    vertex stage, text on the CPU in `push_ui_text_verts`, both only when `radians != 0`, so an unrotated vertex is
    bit-identical. `UiClipBox` gains `radians` (about its own centre); the fragment SDF evaluates in the box frame
    behind the same flag.
49. **Group composition.** `GroupXf` is a similarity `p' = S·R(Θ)·p + O`. Composing group `g` (scale `s`, rotation `θ`,
    centre `c`) under parent `A_p`: `S = S_p·s`, `Θ = Θ_p + θ`, `O = A_p(c) − S·R(Θ)·c`. When `Θ_p` and `θ` are both
    exactly 0 the pre-rotation formula runs unchanged (the general one is not bit-equal; `ui-groups` depends on it).
50. **Applying a rotated group to an instance.** With `P` the instance's pivot if rotated, else its bounds centre:
    `P' = A(P)`, every point maps `x' = P' + S·(x − P)`, `radians += Θ`, `pivot = P'`. When `Θ == 0` the existing
    path runs unchanged.
51. **Clip box under rotation.** When `Θ != 0` the clip box is centred on `A(centre(box_px))`, sized `[w·S, h·S]`, with
    `radians = Θ`; radius and circle rules (decision 26) are unchanged. When `Θ == 0` the existing formula runs.
52. **Hit tests.** `ui_hit` inverse-rotates the cursor about the instance's pivot before the half-open bounds test; the
    clip test uses the rotated SDF twin. `ui_rect_corners(&UiRect, viewport)` (TL, TR, BR, BL) and
    `ui_point_in(&UiRect, viewport, cursor)` are ungrouped and untweened. All trig goes through
    `ludion_core::math::sin_cos`, and a rotation of exactly 0 takes no trig.
53. **`UiShape` fields** (`ludion-components/src/ui_shape.rs`, the `radial.rs` pattern): `anchor` (default top_left), `offset`
    (canvas units, +Y down), `group`; `shape: UiShapeType` (default a circle of radius 16); `fill: Option<[f32; 4]>` and
    `stroke: Option<UiStroke { width, color }>`, skipped when none; `rotation` (degrees, skipped when 0, `L1620`), `order`,
    `visible`. There is no `fill_group` field. A shape with neither fill nor stroke draws nothing, silently.
54. **`UiShapeType`** is `#[serde(tag = "type", rename_all = "snake_case")]`, mirroring `GroundMarkerShape`; the request wrote
    `kind:`, which in Ludion names a document kind. `line { from, to, cap: butt|round }`; `circle { radius }`;
    `ring { radius, thickness, start_deg = 0, sweep_deg = 360 }` (the band `[radius − thickness, radius]`);
    `arc { radius, start_deg, sweep_deg }`; `wedge { radius, inner_radius = 0, start_deg, sweep_deg }`;
    `rounded_rect { size, corner_radius = 0 }`, centred on the origin. Angles are clock degrees (decision 38).
55. **Origin rule.** The shape's local `(0, 0)` sits at `canvas_anchor_px(anchor) + offset·ps`; inside a group it moves by
    `group_delta` like every element. Every length is in canvas units × `pixel_scale`. Scale (tween or group) and rotation
    pivot on the **origin**, not a bounds centre — a documented difference from decision 46.
56. **Stroke** is centred on the shape's edge. `line` and `arc` are stroke-only: their geometry is the stroke band, drawn in
    `stroke.color` (an arc becomes a sector `[r − w/2, r + w/2]`, a line a segment of half-width `w/2`). A `fill` on them, or a
    missing `stroke`, is `L1621`.
57. **`L1621 UI_SHAPE_INVALID`** (error, at the field pointer): non-finite numbers; `radius ≤ 0`; `thickness ≤ 0` or `> radius`;
    `inner_radius < 0` or `≥ radius`; `sweep_deg` outside `[0, 360]`; a negative `size` component or `corner_radius`; a stroke
    `width ≤ 0`; a `fill` on, or no `stroke` for, `line`/`arc`. `sweep_deg = 0` and a zero-length butt line are valid and draw nothing.
58. **Renderer data** (`ludion-renderer/src/scene.rs`, beside `UiRotation`): `UiShapeGeometry` in px relative to the origin, +Y down
    — `Sector { radius_outer, radius_inner, start, sweep }` (clock radians; `sweep ≥ 2π` is a full annulus or disc),
    `Segment { a, b, half_width, round }`, `RoundedBox { half_size, radius }` — with `local_bounds(stroke_width) -> [l, t, r, b]`
    padded by `stroke_width/2`. `UiInstance::Shape { origin_px, geometry, fill, stroke: Option<UiShapeStroke { width_px, color }>,
    clip, rotation }`; when rotated `rotation.pivot_px == origin_px`.
59. **Coverage** (the shader and its Rust twin agree): fill `cf = clamp(0.5 − d, 0, 1)`; stroke `cs = clamp(0.5 − (|d| − w/2), 0, 1)`
    behind `w > 0`; stroke over fill, premultiplied: `as = stroke.a·cs`, `af = fill.a·cf`, `rgb = stroke.rgb·as + fill.rgb·af·(1 − as)`,
    `a = as + af·(1 − as)`; then multiplied by `clip_cov` (decision 28). The blend is `premultiplied_blend`, as in `ui_rect`.
60. **Batching.** A run of consecutive `Shape` instances is one instanced draw (`UiBatch::Shapes`), no texture bind group; a kind
    change closes the run (decision 4), so the order is unchanged.
61. **Hit.** `ui_shape_hit(&UiShape, viewport, cursor)` is pure (ungrouped, untweened; the cursor is turned back about the
    origin): true when `d < 0` with a fill, or `|d| < w/2` with a stroke; a stroke-only kind is true when `d < 0` on its band.
    `ui_hit` tests a `UiShape` through `hud::post` the same way plus `clip_contains`. The SDF twin uses `ludion_core::math` only.

**Proposed — owner ruling owed (W3b):** `type:` rather than `kind:`; the origin pivot for scale and rotation; the centred stroke;
`rounded_rect` centred on the origin; `ring`/`wedge`/`arc` defaulting to a full sweep; `fill` on `line`/`arc` as an error; no
`sweep_deg` tween field (a game writes `sweep_deg`; trigger: a request for one). Request differences: `type:`;
`polyline`/`polygon`/`dash` are W3c; `ring` and `wedge` stay two kinds with different radius fields though they draw the same sector.

62. **Wedge fields** on `UiRadial`, all `Option`, skipped when none: `fill` (every item's wedge colour), `selected_fill` (the
    selected item's wedge; falls back to `fill`) and `stroke` (`UiStroke`, on every drawn wedge). There is no `disabled_fill`: a
    disabled item's wedge uses `fill`, and its text keeps `disabled_color`.
63. **Geometry.** Item `i`'s wedge is a `UiShapeGeometry::Sector`: the layout's inner and outer radii, `start`/`sweep` the layout's
    `start_deg`/`sweep_deg` in radians (the gap is already off; a sweep of 360 or more is `TAU`), origin the wheel centre, `rotation`
    and `clip` `NONE` for `hud::post` and the group to apply, the stroke width in canvas units times `pixel_scale`. An item draws a
    wedge when its fill colour or `stroke` is `Some`. Built by `shape::sector_instance`.
64. **Order.** `radial_draw` emits each item's wedge before that item's icon and text, all under the radial's `(order, EntityId)`
    key; the stable sort puts each label over its own wedge. A stroke can overlap the neighbouring wedge at a small `gap_deg`; the
    author widens the gap.
65. **Validation (`L1619`).** A non-finite `fill`/`selected_fill` channel, and a stroke `width` that is non-finite or not above 0,
    are errors at their pointer. `ui_shape::stroke_problems` is the one check, shared with `UiShape` (`L1621`).
66. **Hash and hit.** The fields are authored, never written by `navigate`, and skip-serialize when absent, so an existing wheel's
    `WorldHash` and replay content hash do not move. `radial_hit` already tests the gapped sector and annulus the wedge draws.
    Group rotation turns a wedge about the wheel centre (the `Shape` arm of `rotate_into_group`).

**Proposed — owner ruling owed (W6b):** no `disabled_fill`; `selected_fill` falling back to `fill`; the stroke on every wedge rather
than a selected-only stroke; overlapping strokes at small gaps left to `gap_deg`. Request differences: none beyond the fallbacks.

67. **`polygon { points: [[x, y], …] }`**: canvas units relative to the origin, +Y down, either winding. Filled in W3c-1; W3c-2 lets it
    take a `stroke` too (decision 74). A polygon with neither `fill` nor `stroke`
    draws nothing, as every shape does. `UiShapeType` loses `Copy` (still `Clone`), because `points` is a `Vec`.
68. **Validation `L1621`**: a non-finite point is reported at `/shape/points/<i>/<j>`; more than 256 points at `/shape/points`,
    naming the cap. Fewer than 3 points is **not** an error: a game writes `points` live (wall-clipped cones), so too few is a
    runtime degeneracy (decision 70).
69. **Triangulation** (`ludion-engine/src/ui/polygon.rs`, `triangulate(&[[f32; 2]]) -> Result<Vec<[u16; 3]>, PolygonFault>`),
    deterministic, pure IEEE arithmetic (no transcendentals, orientation tests in `f64`): drop consecutive duplicate points
    (including last == first); reject fewer than 3 points, `|signed area| <= 1e-6`, and two non-adjacent edges that intersect or
    touch; ear clipping in point order from a cursor, wrapping round, clipping the first strictly convex corner whose triangle
    holds no other remaining vertex (inside or on it), then continuing from the next vertex. A collinear corner is removed without
    a triangle. A full lap with no ear is `NoEar`. Indices refer to the deduplicated points, which the instance carries.
    `PolygonFault { TooFew { needed }, ZeroArea, SelfIntersecting { a, b }, NoEar }`.
70. **`L2305 UI_POLYGON_DEGENERATE`** (Warning): extract records it in `degraded` keyed `"{offender_key}/ui-polygon"` (the `L2304`
    latch pattern), so each entity reports once. It names the fault, and the edge indices for a self-intersection. The polygon
    draws nothing that frame.
71. **Renderer data.** `UiInstance::Polygon { origin_px, points, triangles, fill, clip, rotation }`: `points` are px relative to
    `origin_px`, already scaled; `triangles` index `points` and are invariant under translation, uniform scale and rotation, so
    `hud` maps only the points; when rotated, `rotation.pivot_px == origin_px` (decision 55).
72. **The feathered fringe is built in the renderer, at the final pixel size** (`render/ui_polygon.rs`), so a group scale never
    widens it. With `s` the sign of the signed area and `n` the unit outward edge normals, each vertex's miter is
    `m = (n_prev + n_next)/2` divided by `max(dot(m, m), 0.25)` (capped at 2×); `inner = p − 0.5·m` has coverage 1 and
    `outer = p + 0.5·m` coverage 0; the interior is `triangles` over the inner points; the fringe is a quad per edge. The vertex
    colour is premultiplied `fill.rgb·fill.a·cov, fill.a·cov`; rotation turns positions on the CPU about the pivot; the clip is
    packed per vertex. The fragment returns `color · clip_cov` under `premultiplied_blend`, so the 0.5-coverage contour is the
    authored outline, the contour the hit test uses.
73. **Hit.** `ui_shape_hit` and `ui_hit` treat a polygon as an even-odd point-in-polygon test on the deduplicated points in
    origin-relative px, after `unrotate`; `ui_hit` adds `clip_contains`. A degenerate polygon never hits.

**Proposed — owner ruling owed (W3c-1):** the W3c-1/2/3 split; `L2305` as a once-per-entity warning rather than per change;
fewer than 3 points as a runtime warning, not a field error; the miter fringe capped at 2× (a 2× cap holds the miter for corners of
60° and wider; sharper tips are cut back, so the GPU agreement test uses ~78° tips); a polygon stroke as `L1621` until W3c-2. Request differences: `type: polygon`; no stroke or dash yet; `polyline` is
W3c-2.

74. **`UiLineJoin { miter (default), round, bevel }`** (`ui_shape.rs`, snake_case, skipped when miter). `polygon` gains `join`; its
    `stroke` is now allowed (lifts decision 67's `L1621`). A polygon with fill and stroke draws the stroke over the fill; a
    stroke alone draws the outline.
75. **`polyline { points, closed = false, join = miter, cap = butt }`** (`cap` is W3b's `UiLineCap`; skipped at default).
    Stroke-only like `line`: a `fill` or a missing `stroke` is `L1621`; `closed` ignores `cap`. At most 256 finite points
    (`L1621`, same pointers as polygon). Self-crossing is allowed. After dropping consecutive duplicates (and a repeated closing
    point when `closed`), fewer than 2 points draws nothing and warns `L2305` once per entity, as a degenerate polygon does:
    `PolygonFault::TooFew { needed }` is 3 for a polygon, 2 for a polyline.
76. **Polygon degeneracy is unchanged:** a polygon (fill and/or stroke) is still triangulated and its fault draws nothing and
    warns `L2305` once (decision 70). Triangles are kept only when it has a fill.
77. **Renderer data.** `UiInstance::Polygon` is the one tessellated-path instance: `{ origin_px, points, closed, triangles,
    fill: Option<[f32; 4]>, stroke: Option<UiPathStroke>, clip, rotation }`, with `UiPathStroke { width_px, color, join:
    UiPathJoin, round_cap }` and `UiPathJoin { Miter, Round, Bevel }` in `ludion-renderer/src/scene.rs` (the renderer never names
    component types). A polygon is `closed: true`; a polyline has empty `triangles` and no fill.
78. **Stroke tessellation** (`render/ui_polygon.rs`, `push_ui_stroke_verts`), the fill's vertex format and pipeline (no shader
    change), CPU-rotated like the fill. Half width `h = w/2`; a stroke under 1 px draws at `w = 1` with its alpha scaled by `w`.
    Per segment a strip of four rails, coverage 0, 1, 1, 0 at `±(h + 0.5)`, `±(h − 0.5)`, so the 0.5 contour is the stroke edge.
    Miter keeps the shared point while the miter vector is at most 4 (SVG's limit), else bevels; bevel and round fan the outer
    side from one edge normal to the other (round: `n = clamp(ceil(θ / (2·acos(1 − 0.25/(h + 0.5)))), 1, 32)`); the inner side
    uses the miter point clamped to the shorter adjacent segment. A butt end is flush with a ±0.5 px fringe along the segment; a
    round cap is a half-disc fan. Fill vertices first, then stroke, in one instance. Translucent self-crossings darken where they
    cross (accepted, documented).
79. **Hit** (`ui/polygon.rs`, `stroke_contains`): within `h` of a segment, the outer join wedge (round: within `h` of the vertex;
    bevel: the triangle; miter within the limit: the quad), round caps within `h` of an end. A shape hits when the fill test or the
    stroke test does; `ui_hit` adds `clip_contains`. A stroke under 1 px hits as 1 px.
80. **Tweens and groups.** Scale multiplies the points and `width_px`; opacity multiplies fill and stroke alpha; a colour tween
    replaces `fill` (the stroke colour on a stroke-only `polyline`, like `line`/`arc`).

**Proposed — owner ruling owed (W3c-2):** miter as the default join; the fixed miter limit 4 (no field; trigger: a request);
`polyline` `cap` reusing `UiLineCap`; the thin-stroke alpha rule; translucent self-crossings darkening; W3c-3 (dashes) kept
separate. Request differences: `cap` on polyline (the request lists only `join`).

81. **`UiDash { on: f32, off: f32 }`** (`ui_shape.rs`, `Copy`). It is a `UiShape` field `dash: Option<UiDash>`, skipped when none,
    placed after `stroke`, as a sibling of `stroke` like the request. It is in canvas units along the stroke's centre line.
    `UiRadial` wedge strokes take no dash (trigger: a request).
82. **Validation `L1621`**: a non-finite `on`/`off` at `/dash/on`, `/dash/off`; `on ≤ 0` at `/dash/on`; `off < 0` at `/dash/off`; a
    `dash` with no `stroke` at `/dash`. `off == 0` is valid and draws the stroke solid, by the undashed path.
83. **Pattern.** Dashes start "on" at the path's first point and run along it: the start of a closed path is not joined to the end
    of the last dash (SVG's rule). Dash ends take the stroke's cap: `line`/`polyline` `cap: round` gives round dash ends; a closed
    path and every analytic kind but `line` give butt ends. Corners inside one dash keep their join. Start points: `line` at `from`;
    `polygon`/`polyline` at `points[0]`; `circle` at 12 o'clock, clockwise; `arc` at `start_deg`; a ring or wedge outline at the outer
    arc's `start_deg`. A partial ring or wedge runs clockwise along the outer arc, inward along the end radial edge, back along the
    inner arc (or through the centre when `inner_radius` is 0), then out along the start edge. A full-sweep ring or wedge is two
    closed circles, outer then inner (no inner circle at `inner_radius` 0). `rounded_rect` starts at the top edge's left end
    `(−hw + r, −hh)`, clockwise.
84. **Flattening** (engine `ui/dash.rs`, px, before tweens' scale and groups): an arc of radius `r` (px) and turn `θ` becomes
    `n = clamp(ceil(θ / (2·acos(1 − 0.1/r))), 4, 512)` segments, `n = 4` when `r ≤ 0.1`; the trig is `ludion_core::math`. Analytic
    outlines use round joins, which is what the SDF stroke draws at a corner. A group or tween scale magnifies the facets (0.4 px at
    2×; accepted, documented).
85. **Renderer data.** `UiPathStroke` gains `dash: Option<[f32; 2]>`, `[on_px, off_px]`. A dashed analytic shape emits its fill as
    today's `UiInstance::Shape` with `stroke: None` (skipped when it has no fill), then one stroke-only `UiInstance::Polygon` per
    outline (empty `triangles`, `fill: None`, the same `origin_px`, `join: Round`, `round_cap` for a round `line`), so stroke over
    fill holds through the stable sort. A dashed polygon or polyline is today's one instance with `stroke.dash` set.
    `push_ui_stroke_verts` splits a dashed path with `ludion_renderer::ui_dash_runs` and strokes each run open with `dash: None`;
    `ui_dash_runs` is the one split, shared with the hit test.
86. **Density cap.** When the dashes would number more than 1024 (all outlines' length / `(on + off)` in px), extract draws the stroke
    solid by the undashed path and records the degraded warning `L2306 UI_DASH_TOO_DENSE` once per entity, keyed
    `"{offender_key}/ui-dash"`, naming the count and the limit. The count does not change under scale.
87. **Hit, tweens, groups.** A dashed stroke hits only on a dash: `path_contains` runs `stroke_contains` (open, with the dash cap)
    on each run of `ui_dash_runs`; the gaps miss and the fill still hits. Group and tween scale multiply `dash` with `width_px`.
    Opacity and the colour tween act in `pre_shape` on the component, before the split, so every instance sees them.

**Proposed — owner ruling owed (W3c-3):** the dash on `UiShape`, not inside `UiStroke`, so radial wedges take none; no dash
`offset`/phase field (marching ants; trigger: a request); the closed-path seam not joined; round joins on flattened analytic
outlines; the 1024-dash cap with a solid fallback; facets under scale; gaps not hit. Request differences: none.

## Waves

- **W1 — `UiTween`, `UiWorldAnchor`, resolved in extract.** CPU-only; the pass lives in
  `ludion-engine/src/ui/hud.rs`, called from the UI phase of `extract`. Docs `ui-tweens`,
  `ui-world-anchor`; example `ui_world_anchor`. No existing golden, replay or hash moves: a world
  without the components extracts byte-identically.
- **W2 — `uv_rect` and nine-slice.** CPU-only: extract emits nine ordinary `UiInstance::Rect`s in place of
  one (`ui/hud.rs::emit_rect`); no renderer change. Docs `ui-nine-slice`; example `ui_nine_slice`. A rect
  without the fields extracts byte-identically.
- **W3a — rotation** (decisions 45–52): `rotation` on `UiRect`/`UiText`/`UiGroup`, the rotated clip, hit agreement,
  `ui_rect_corners`/`ui_point_in`; the `ui-rotation` golden. Unrotated UI renders byte-identically.
- **W3b — `UiShape` analytic kinds** `line circle ring arc wedge rounded_rect`, fill and stroke, `UiInstance::Shape`,
  `ui_shape.wgsl`, `ui_shape_hit`, the `ui-shapes` golden.
- **W3c-1 — filled `polygon`** by ear clipping with a feathered fringe and a degenerate warning (decisions 67–73, `L2305`,
  `ui_polygon.wgsl`, the `ui-polygons` golden).
- **W3c-2 — strokes as tessellated paths** on the same pipeline: a polygon `stroke`, and `polyline { points, closed, join, cap }`
  (decisions 74–80, `L2305` for both kinds, the `ui-strokes` golden).
- **W3c-3 — dashes** (decisions 81–87, `L2306`, the `ui-dashes` golden): `dash: { on, off }` on any `UiShape` stroke.
- **W4a — `UiGroup`:** origin, scale, opacity and visibility, CPU only (decisions 18-24;
  `ui/group.rs`, `hud.rs::post`). Docs `ui-groups`; example `ui_groups`. An element without `group`
  extracts byte-identically.
- **W4b — the fragment clip and `math::ui_hit`:** a `UiInstance` field on both variants,
  `ui_rect.wgsl`/`ui_text.wgsl`, the `ui-groups` gallery golden; `ui_hit` walks groups, tweens, anchors
  and the clip.
- **W5a — text outline** (decisions 31–36): SDF from the field, bitmap by in-shader dilation; `ui-text-outline` golden.
- **W5b — text shadow.**
- Order, accepted by the owner 2026-11-19: W6a → W3a → W3b → W6b.
- **W6a — `UiRadial`, `UiRadialNav`, `radial_layout`/`radial_hit`** (decisions 37–44): CPU only, no renderer change;
  item text, detail and icons through the existing UI instances; the 1/2/8-thread replay and the snapshot test
  (`tests/ui_radial.rs`); `L1619`; docs `ui-radial`; example `ui_radial`. Only the component schema snapshot moved.
- **W6b — wedge fills and strokes** on W3b's shape pipeline (decisions 62–66): `fill`, `selected_fill`, `stroke` fields and the
  `ui-radial` gallery golden. CPU only, no renderer change. A rotated wheel is a rotated `UiGroup` (W3a).
- **W7 — anchor blend keys under interpolation, `rotate_toward`, `distance.scale`, `camera`.**

## Owner rulings (2026-11-19)

Accepted. Request amendments 1–7; the 12-o'clock clockwise angle convention (W3); the centre pivot for scale;
the `real` default clock; the separate `UiEase`; the interpolation gap (decision 8); W2: `uv_rect` beside `slice` warns and `slice` wins, stretch-only
edges, the proportional texel clamp rather than an error; W4a: the 8-level cap, `order` global (no block sort),
a world anchor only on the outermost group, `fill` ignoring `group`; W5a: the SDF max (spread less 1 px) with clamp-and-warn rather than an error, the bitmap dilation as a square kernel rounded with a minimum of 1, the cap of 4 source px, no outline on world `Text`, the shadow split to W5b; W4b: the 2-clip cap (innermost kept), circle
= the inscribed circle, the 1 px AA band, `ui_hit` on a list tests the panel, `ui_hit` ignores `pixel_snap`.
Accepted. W6a: the clock-angle convention, the clockwise boundary rule, the gap ignored by stick selection, cancel beating activate,
the pointer reset on close, the 16-item cap and allowing one item, the first step from none landing on the first enabled item,
a delta aim read +y down, the icon placement and the `icon_size` and `detail` sizes (fields the request does not name), icons tinted
by the item colour, the W6a → W3 → W6b order.
Accepted. W3a: the clockwise-degrees convention (shared with decision 38), the centre pivot (request open question 2), the text pivot at
the glyph-bounds centre rather than the measured block, the additive rotation tween, no `rotation` field on `UiList`/`UiRadial`,
the W3a/W3b/W3c split. W3a request differences: `UiShape` and its rotation are W3b, the tween adds to the authored rotation,
there is no rotation field on lists.
Accepted. Request differences: no polygon in `ui-groups` (W3c); `ui_hit` does not return list items; W6a draws no wedge or stroke (W6b, since shipped)
and adds `icon_size`, `selected_color`, `visible` and `outline`.

## Out of scope (triggers recorded)

a dash phase/offset, dashes on `UiRadial` strokes (trigger: a request), a `sweep_deg` tween, rotation as a field on lists and radials, nine-slice edge tiling (trigger: frame art that visibly breaks
when stretched), slicing `UiList`/`UiText`, text shadow (W5b), a wedge `disabled_fill` or per-item colour, sub-wheels, item-level `ui_hit`,
anchor blending, `rotate_toward`, `distance.scale`, the anchor `camera` field, gallery
goldens (a pinned box owes them), world `Text` outline (trigger: a request for it; see
`Plan/font-assets.md` for the SDF outline path).
