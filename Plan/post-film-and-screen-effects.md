# Plan: film post and screen effects

> Status: **PLANNED** (Neon Shroud request 0019, `engine-requests/0019-post-anamorphic-lut-autoexposure-ui-effects.md`,
> read-only; audited at engine r2369, re-checked at Lore 2399). Six additions, all engine-owned and off by default:
> anamorphic streaks, `.cube` LUT grading, metered exposure, animated grain, an authorable halation colour, and
> `ScreenEffect` presets scoped to a UI region, the whole screen or a secondary view. Related: `Plan/post-process-pipeline.md`
> (the chain, decisions 2, 5, 9–14), `Plan/light-exposure.md` (request 0013), `Plan/rain-and-wet.md` decision 7 (the time
> precedent), `Plan/secondary-views.md` (request 0017), `Plan/engine-gap-map.md` "Exposure".

## Why

An external game's art direction asks for a 1982 film look (grain that moves, a halation hue it picks, anamorphic
streaks), a hue-selective grade that lift/gamma/gain and a global saturation cannot express, exposure that holds a dark
but legible image across a bright-to-dim walk, and CRT or analogue-video treatment on three screen elements only (a
radar feed, terminals, a first-person overlay), never on the world. The engine has none of these: bloom is a symmetric
pyramid, halation's hue is a constant, grain is static, exposure is manual, the grade is per-channel, and the UI has no
effect stage. Game shaders stay absent (`dev-docs/ludion-shader-bindgen.md` decision 1, Pillar 6), so every addition is
an engine preset with numeric fields: text-authorable, `--set`/`--sweep`-able, goldenable.

## Decisions

### Common

1. **Absent is identical.** Every new `PostProcess` field is serde-defaulted at its neutral value and skipped when
   serialized at it (`skip_serializing_if`, the `ssr_*` precedent), so committed scenes keep their canonical text and
   `WorldHash`. A neutral field is **skipped by a uniform branch or an absent pass, never computed with identity
   constants** (pipeline decision 9). A scene that authors none of them keeps today's `dump-graph` and pixels. No change
   here touches `replay_content_hash`, the wire `Command`/snapshot schema or `WorldHash`; a design that would is recorded
   as an owner item instead.
2. **Ranges ride the schema.** Scalars carry schemars ranges (`L1014`); colour triples reject a negative element or a
   zero Rec.709 luma through `PostProcess::validate_semantics` with the existing `L1130`. New codes come from
   `L2390`–`L2399` (render and asset time) and `L1760`–`L1769` (scene validation); the request's `L214x` numbers were
   placeholders.
3. **Time without history.** Animated stages (grain, `ScreenEffect`) read the game clock that extract already has
   (`now`, from `GameClock`), carried in the effect's own render data, not as a `RenderScene` field: request 0018
   settled the same question that way (`Plan/rain-and-wet.md` decision 7). A held clock renders the same frame twice;
   nothing reads a previous frame, so pipeline decision 5 is not engaged.
4. **Budgets.** New `[post_groups]` `lens` (the anamorphic chain), `exposure` (the meter) and `screen_fx`. Their
   ceilings are calibrated on the two pinned machines only; a machine that is not pinned measures nothing
   authoritative.

### A — anamorphic streaks

5. **Fields.** `anamorphic` `[0, 1]` (default 0, the gate), `anamorphic_length` `[0, 1]` (default 0.35),
   `anamorphic_tint` linear RGB (default `[0.55, 0.7, 1.0]`), `anamorphic_threshold: Option<f32>` `>= 0` (default
   `null` = `bloom_threshold`).
6. **The chain.** A horizontal-only pyramid off `bloom_down_1` (the quarter-extent level, already Karis-averaged, so a
   lone bright pixel cannot streak as a firefly): `lens_down_1..5` halve the width at a constant height of
   `output_height >> 2`, then `lens_up_4..0` walk back, each blending `mix(fine, upsampled coarse, anamorphic_length)`.
   Every level is sized from the **output** extent, so the streak is a fixed fraction of the frame at any render scale
   (the screen-relative rule). `anamorphic_length` is therefore the pyramid's stretch, not an exact half-length: it is
   monotone in length, and the docs record the measured half-length at the default. This differs from the request's
   "fraction of frame width"; an exact length would need a per-value graph or a per-pixel loop.
7. **Threshold.** With `anamorphic_threshold` set, `lens_down_1` subtracts `max(t − bloom_threshold, 0)` per channel
   from the already-thresholded field, the bright pass's own arithmetic. A value at or below `bloom_threshold` acts as
   `null` (the chain cannot see below the field it reads); the docs say so.
8. **Composite.** Step 2 of the pinned order gains `+ streak × anamorphic × tint / luma(tint)`, beside halation:
   scene-referred, before the medium, luma-normalized so `anamorphic` is a strength on `bloom_strength`'s scale. The
   streak texture is a new **optional input** of `composite` (the fixed-slot rule in `fullscreen_effect`), bound to the
   1×1 stand-in and never read when the chain is absent.
9. **Gate.** `PostChain.anamorphic` is `anamorphic > 0`; the ten `lens_*` passes and their targets exist exactly when it
   is on, declared after every existing conditional chain so no existing resource list moves. No quality switch: bloom
   has none and the chain costs a few quarter-height passes.

### E — halation colour

10. **`halation_color: Option<[f32; 3]>`**, linear RGB, default `null`. `null` keeps today's constant
    (`1.0, 0.35, 0.12`) through the **unchanged** shader path, so a default scene is byte-identical by construction.
    A set value is normalized by Rec.709 luma in the shader, so `halation` stays a strength and only the hue moves.
    This reverses the recorded "not authorable" call (rustdoc and `docs/post-processing.md`): the stock is the art
    direction's choice, and the default stays the engine's stock.

### B — `.cube` LUT grading

11. **Field.** `lut: Option<{ path, strength }>`: `path` relative to the content root, `strength` `[0, 1]` (default
    1.0). A missing or unreadable file is the existing `L1101`. `PostProcess` stops being `Copy` (a `String` path); in-tree uses `.clone()`.
12. **Parser.** In `ludion-assets` (no wgpu type): `TITLE`, `LUT_3D_SIZE` 17, 33 or 65, `DOMAIN_MIN`/`DOMAIN_MAX`,
    `LUT_3D_INPUT_RANGE lo hi` (Resolve's spelling, read as a domain), comments, data rows; header keywords come before
    the first data row. `L2390` for a malformed header or row (with its line number); `L2391` for `LUT_1D_SIZE` or any
    other size. ASCII and diffable, so the binary-asset objection does not apply.
13. **Position and domain.** Chain step 7a: after the grade, before the silhouette (7b, a gameplay signal the look must
    not recolour), the vignette, grain and dither. The LUT is indexed by the **sRGB-encoded** colour (encode, sample,
    decode), because that is the space a LUT authored in Resolve or Photoshop on an exported frame describes, so
    `ludion lut apply` on a PNG is the in-engine operation. A 3D `Rgba16Float` texture, trilinear with a half-texel
    inset; tetrahedral interpolation is not built (trigger: a measured banding regression on a 17³ LUT). `strength`
    mixes in the encoded domain. The LUT rides `RenderScene.lut`, renderer-owned, with no graph resource, so
    `dump-graph` never changes.
14. **Agent tooling** answers the authoring objection (`Plan/post-process-pipeline.md` "Deferred"): `ludion lut
    identity --size N --out x.cube`; `ludion lut bake --scene s.yaml --out x.cube [--size 33]` bakes the scene's analytic
    grade through a CPU twin of the WGSL grade, drift-pinned textually like `light_model.rs`; `ludion lut apply --lut
    x.cube --image in.png --out out.png`; `ludion lut stats x.cube` prints how far fixed hue bands (amber, cyan, green,
    magenta, skin, neutral grey) move in hue, saturation and lightness, as text. These are CLI pipeline verbs (batch
    producers and read-only checks of committed files), which the parity gate allows; the editor may call them with
    `--json`.

### C — metered exposure

15. **Fields.** `exposure_mode: manual | metered` (default `manual`), `exposure_compensation` EV `[-8, 8]` (default 0,
    both modes; 0 leaves `exposure` untouched on the CPU), `metering: average | center` (default `center`),
    `metering_clip: [low, high]` (default `[0.02, 0.02]`), `exposure_range: [min, max]` EV (default `[-2, 2]`).
    `L1760` when `min > max` or `low + high >= 1`.
16. **Same frame, deterministic.** Two compute passes before `composite`: `exposure_histogram` reads the HDR `scene`
    (what bloom reads) and `scene_depth` at a fixed stride and adds **integer** weights into 64 log2-luminance bins
    (`atomicAdd` on `u32`: order-independent, so identical across runs); `exposure_reduce` runs one invocation that
    walks the bins in fixed order, trims `metering_clip`, takes the mean log2 luminance, sets the correction that puts it
    on 0.18 mid-grey, clamps it to `exposure_range` and writes a 1×1 texture. `composite` reads it as an optional input
    and multiplies `exposure`. No readback, no previous frame.
17. **Weights.** Centre weighting is an integer radial falloff; sky pixels (far depth) weigh 1 against 4 (the request's
    0.25); viewmodel pixels (depth inside the viewmodel slice) weigh 0. UI is drawn after the meter, so it never counts.
18. **No temporal adaptation.** Smoothing over time would make the image a function of camera history (pipeline
    decision 5). A game eases between targets by writing `exposure_compensation` from a presentation system each tick,
    the `focus_distance` idiom; that value is world state and hashed like any field.
19. **Consistent with request 0013.** `LightExposure` answers authored light from sim state and never reads the camera;
    the meter is render-only and never feeds the sim. A game that drives zone targets from `LightExposure` gets one
    light model on both sides. The `irradiance` debug view replaces `composite`, so it stays unmetered.
20. **Instruments.** `ludion render --debug-view exposure_meter` (CLI and engine only, not on the wire: a
    `RenderDebugView` variant moves `replay_schema_hash`, `Plan/light-exposure.md` decision 18) draws the histogram and
    the chosen EV. `ludion profile` records the metered EV per frame by reading the 1×1 back off the frame path.

### D — animated grain

21. **`grain_motion: static | animated`** (default `static`). Animated folds the game tick (`u32`, from extract's
    `now`) into the `triple32` cell hash as a third coordinate; the static path is untouched. The tick travels in
    `PostData`, and every pass that owns grain (`composite`, `smaa_blend`, `resolve`, `upscale`) receives it. The external
    game confirms the motion trigger (`Plan/post-process-pipeline.md` decision on static grain) at a live sitting; the
    field ships opt-in either way.

### F — `ScreenEffect`

22. **One component**, new module `ludion-components/src/screen_effect.rs`, on a sim or presentation entity
    (presentation is unhashed). `target: fill | { region: { anchor, offset, size } } | { view: <RenderView name> }`
    (region in the UI's own layout units, `UiRect`'s anchor rules), `order` (UI order), `preset: none | crt |
    analog_video | scan`, and the stages of the request (`scanlines`, `scanline_count`, `mask`, `mask_strength`,
    `curvature`, `phosphor`, `glow`, `chroma_bleed`, `noise`, `tracking`, `roll`, `jitter`, `flicker`, `interlace`,
    `posterize`, `sweep`, `sweep_period`, `seed`). Each stage is optional: unset takes the preset's value, so canonical
    text carries only what the author overrode.
23. **Presets** take the request's proposed values: `crt` = scanlines, aperture mask, curvature, phosphor tint, glow,
    light noise; `analog_video` = chroma bleed, tracking, jitter, noise, desaturated amber, no mask; `scan` = fine
    scanlines, a slow sweep bar, slight posterize, no curvature.
24. **Placement (region and fill).** A backdrop filter in UI order: with effects present, the overlay is split at each
    effect's `order`; the region of the output is copied to a transient, the effect draws the region back over it, then
    the remaining UI draws on top. A scene with no `ScreenEffect` keeps today's single overlay pass. The 3D frame's own
    chain is never touched.
25. **Placement (view).** The effect is the last step of that `RenderView`'s own chain, so it is baked into the texture
    before any `UiRect` samples it. It waits on request 0017's view chain.
26. **Cap.** At most 4 applied per frame; the extra ones (highest `order`, then highest `EntityId`) are not applied and
    raise `L2392` once per load, never silently. `L1761` for a non-positive region size or a `view` naming no
    `RenderView`. Animated stages are functions of game time (decision 3) and `seed`. Target cost ≤ 0.15 ms at 1080p for
    a 320² region plus a fullscreen `scan`, measured on the pinned machines.

## Codes

| Code | When |
|---|---|
| `L1760` | `exposure_range` min above max, or `metering_clip` summing to 1 or more |
| `L1761` | a `ScreenEffect` target is invalid: non-positive region size, or a `view` naming no `RenderView` |
| `L2390` | a `.cube` header or data row is malformed (line number in the message) |
| `L2391` | a `.cube` is 1D, or its 3D size is not 17, 33 or 65 |
| `L2392` | more than 4 `ScreenEffect`s active in a frame (warning, once per load) |

The rest of both ranges is reserved for this plan.

## Waves

- **W1 — halation colour and anamorphic streaks** (E, A): the fields, the `lens_*` chain and its two shaders, the
  composite's new optional input, `[post_groups] lens`, `docs/post-processing.md`, gallery
  `examples/gallery/post-film-1982.yaml`. No new code.
- **W2 — `.cube` LUT** (B), split: **W2a** the parser, `lut` field, asset-cache kind, extract, the composite step 7a,
  `L2390`/`L2391`; **W2b** the tools. Together: the parser, `lut` field, the 3D input, `L2390`/`L2391`, `ludion lut
  identity|bake|apply|stats` with the grade's CPU twin, a committed `.cube` in the gallery, `docs/post-processing.md`
  and `anti-aliasing.md` (where the LUT sits with SMAA on).
- **W3 — metered exposure** (C): the fields, `L1760`, the two compute passes, `[post_groups] exposure`, the
  `exposure_meter` view, the profile field, an `image-stats` bright-to-dark pan check.
- **W4 — animated grain and region/fill effects** (D, F): the tick in `PostData`, `grain_motion`, `ScreenEffect`,
  `L1761`/`L2392`, the overlay split, `[post_groups] screen_fx`, gallery `screen-effects.yaml` with a `--frames 0,1,2`
  strip, `docs/screen-effects.md`. May split into W4a (D) and W4b (F) at planning. Needs no 0018 field (decision 3).
- **W5 — view targets** (F): `target: { view }` once request 0017's view chain closes.

Goldens for every new scene are blessed on the pinned machines; budget twins are owed per pinned OS.

## Owner decisions

Covered by the standing ruling (additive, off by default), recorded here:

- The `.cube` deferral's trigger fires (hue-selective, cross-channel grade); answered with the `ludion lut` tooling.
- Halation's colour becomes authorable; `null` keeps the engine's stock.
- Grain may animate, opt-in; the game confirms the motion trigger at its sitting.
- Metered exposure, same frame; temporal adaptation stays out (decision 18).
- The overlay may split at `ScreenEffect` orders, only when one exists.
- The pinned chain order gains the anamorphic term at step 2 and the LUT at step 7a.
- The request's proposed values: metering weights (centre, sky 1:4, 2 % trims), preset defaults, the 4-effect cap.

Differences from the request: `anamorphic_length` is the pyramid's stretch, not an exact half-length (decision 6);
`anamorphic_threshold` cannot go below `bloom_threshold` (decision 7); the LUT is indexed in the encoded domain and sits
before the silhouette (decision 13); codes renumbered.

## Out of scope / deferred

- **`lens_ghosts`.** Trigger: a look verdict that asks for mirrored flares beyond the streaks.
- **A LUT/grade exemption for world overlays** (the request's question 1). Request 0014's call.
- **A world-surface `ScreenEffect` target** (in-world terminal screens). Trigger: the first world-surface terminal; a
  request-0017 world-texture follow-up would carry it.
- **Temporal eye adaptation.** Rejected by pipeline decision 5; games write `exposure_compensation`.
- **Tetrahedral LUT interpolation.** Trigger: measured banding on a 17³ LUT.
- **An anamorphic quality switch.** Trigger: a machine whose budget cannot carry the `lens` group.
- **A custom-shader surface.** Rejected (Pillar 6); presets grow by request.

## Status — request 0019

- Planned 2026-11-20.
- W1 (`halation_color`, `anamorphic*`, the `lens` group) mechanized on the M5 2026-11-21: absent fields left five gallery
  scenes (with and without SMAA) and their `dump-graph` byte-identical on that machine; the streak's half-length at the
  default `anamorphic_length` measures about 0.30 of the frame width. Goldens and `lens` budgets owed on the pinned machines.
- W2a (`.cube` parser, `lut` field, the composite step 7a) mechanized on the M5 2026-11-21; goldens owed on the pinned machines.
