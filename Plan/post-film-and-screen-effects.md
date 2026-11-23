# Plan: film post and screen effects

> Status: **MECHANIZED** (Neon Shroud request 0019, `engine-requests/0019-post-anamorphic-lut-autoexposure-ui-effects.md`,
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
16. **Same frame, deterministic.** One compute pass before `composite`: `exposure_meter` (one workgroup of 256, a fixed
    128x72 sample grid, so its cost is resolution-independent) reads the HDR `scene` (what bloom reads) and `scene_depth`
    and adds **integer** weights into 64 log2-luminance bins over `[-16, 8)` (workgroup `atomicAdd` on integers:
    order-independent, so identical across runs), with a fixed-point per-bin sum of log2 luminance so the trimmed mean is
    exact to 1/512 EV. One thread then walks the bins in fixed order, trims `metering_clip`, takes the mean log2
    luminance, sets the correction that puts it on 0.18 mid-grey, clamps it to `exposure_range` and writes a 65x1
    `Rgba32Float` table (texels 0-63 the bins as `(weight, summed log2, 0, 0)`; texel 64 = `(correction EV, mean log2,
    total weight, gain)`). `composite` reads texel 64 as an optional input and multiplies `exposure`. No readback, no
    previous frame.
17. **Weights.** Integers: a base of 4 (sky, depth 1, weighs 1: the request's 0.25); viewmodel pixels (depth inside the
    viewmodel slice) weigh 0; `center` multiplies by a factor falling 4 to 1 across four radial rings. UI is drawn after the meter, so it never counts.
18. **No temporal adaptation.** Smoothing over time would make the image a function of camera history (pipeline
    decision 5). A game eases between targets by writing `exposure_compensation` from a presentation system each tick,
    the `focus_distance` idiom; that value is world state and hashed like any field.
19. **Consistent with request 0013.** `LightExposure` answers authored light from sim state and never reads the camera;
    the meter is render-only and never feeds the sim. A game that drives zone targets from `LightExposure` gets one
    light model on both sides. The `irradiance` debug view replaces `composite`, so it stays unmetered.
20. **Instruments.** `ludion render --debug-view exposure_meter` (also `render.debug {"view": "exposure_meter"}`:
    on the wire, shipped in R14, `Plan/light-exposure.md` decision 18) replaces `composite`
    with the metered frame, dimmed, and the 64-bin histogram over its bottom quarter: bars white inside the trimmed
    window and grey where `metering_clip` cut them, a green line at the trimmed mean, a yellow one at the mid-grey
    target, a translucent blue band over the means the correction does not clamp. The view needs the meter, so a manual
    scene draws the normal frame and the CLI says why. The EV is text, not drawn: the table's texel 64 is read back
    through a non-blocking three-deep ring (the cull-stats precedent, `Renderer::set_meter_readback` /
    `collect_meter_readings`) and `ludion render` prints it in its summary, ` (metered: +1.42 EV, mean log2 -4.05)` with
    `, clamped` when the correction sits on a range end; a `--frames` strip prints one per frame. **R14 item:** the
    `ProfileReport` per-frame EV field moves the protocol schema and `replay_schema_hash`; ship it in the R14 round, with
    the render summary as the shipped stand-in.

### D — animated grain

21. **`grain_motion: static | animated`** (default `static`). Animated folds the game tick (`u32`, from extract's
    `now`, truncated to `u32`) into the grain's cell hash; the static path is the exact prior expression. The tick travels
    in `PostData.grain_tick: Option<u32>` (`None` = static) and every pass that owns grain (`composite`, `smaa_blend`,
    `resolve`, `upscale`) receives it; animated adds one `hash32` round on the row seed (`hash32(row ^ tick)`). The external
    game confirms the motion trigger (`Plan/post-process-pipeline.md` decision on static grain) at a live sitting; the
    field ships opt-in either way.

### F — `ScreenEffect`

22. **One component**, new module `ludion-components/src/screen_effect.rs`, on a sim or presentation entity
    (presentation is unhashed). `target: fill | { region: { anchor, offset, size } } | { view: <RenderView name> }`
    (region in the UI's own layout units, `UiRect`'s anchor rules), `order` (UI order), `preset: none | crt |
    analog_video | scan`, and the stages of the request (`scanlines`, `scanline_count`, `mask`, `mask_strength`,
    `curvature`, `phosphor`, `glow`, `chroma_bleed`, `noise`, `tracking`, `roll`, `jitter`, `flicker`, `interlace`,
    `posterize`, `sweep`, `sweep_period`, `seed`). Each stage is optional: unset takes the preset's value, so canonical
    text carries only what the author overrode. Targets are `fill | region` for W4 (`view` arrived in W5). A phosphor of
    `[0, 0, 0]`, a `posterize` below 2 and a `mask` at strength 0 are off; an effect with every stage off is not applied
    and not counted. A region lays out like a `UiRect` (`anchor`, `offset`, `size`, canvas units) and is clamped to the
    viewport at extract.
23. **Presets** take the request's proposed values: `crt` = scanlines, aperture mask, curvature, phosphor tint, glow,
    light noise; `analog_video` = chroma bleed, tracking, jitter, noise, desaturated amber, no mask; `scan` = fine
    scanlines, a slow sweep bar, slight posterize, no curvature. Base (`none`): every stage off, `scanline_count` 240,
    `sweep_period` 2. `crt`: scanlines 0.35, aperture mask 0.2, curvature 0.06, phosphor `[0.25, 1, 0.45]`, glow 0.3,
    noise 0.04, flicker 0.02. `analog_video`: scanlines 0.1, chroma bleed 0.4, tracking 0.15, jitter 0.2, noise 0.08,
    flicker 0.03, phosphor `[1, 0.72, 0.4]`, interlace. `scan`: scanlines 0.2 at 540 lines, sweep 0.25 every 4 s,
    posterize 24, noise 0.02.
24. **Placement (region and fill).** A backdrop filter in UI order: with effects present, the overlay is split at each
    effect's `order` and writes a whole-frame transient layer instead of `output`; each effect pass blits the previous
    layer, draws the effect over its rect reading that layer, then the next UI slice draws on top, the last pass writing
    `output`. A whole-frame layer, not a region copy, because `output` is `RenderAttachment + CopySrc` only: sampling or
    copying it would change its description in every graph. Pixels outside every rect match the no-effect frame exactly.
    A scene with no `ScreenEffect` keeps today's single overlay pass and declares no layer. The 3D frame's own chain is
    never touched.
25. **Placement (view).** The effect runs in that `RenderView`'s own graph as the same `overlay -> screen_fx_k` chain
    decision 24 builds, over the whole view texture (`rect_px = [0, 0, w, h]`), so it is baked in before any `UiRect`
    samples the view. It needs no per-view post profile (request 0017 W3): the view's derived scene carries the view's own
    effect list and `PostChain::for_scene` counts it. Several effects on one view apply in `(order, EntityId)` order. A
    `view` naming no `RenderView` is `L1761` and applied nowhere; a view that exists but does not render (disabled, or
    past the view budget) drops its effects silently, uncounted. A view with no effect, and the frame, are byte-identical.
26. **Cap.** At most 4 applied per frame across every target (fill, region and view together, so the frame never runs
    more than 4 `screen_fx` passes summed over its graphs); the extra ones (highest `order`, then highest `EntityId`) are
    not applied and raise `L2392` once per load, never silently. The applied ones are the lowest `(order, EntityId)`
    among the effects that change a pixel and have a usable target. `L1761` for a non-positive region size or a `view` naming no
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
- **W3 — metered exposure** (C), split: **W3a** the fields, `L1760`, `exposure_compensation`, the `exposure_meter`
  pass, the composite's optional input, `[post_groups] exposure`; **W3b** the `exposure_meter` view, the render-summary
  EV (the `ProfileReport` field is an R14 item, decision 20), a bright-to-dark pan check as a CLI test (`image-stats`
  measures defects, not brightness, so it is not an `image-stats` metric).
- **W4 — animated grain and region/fill effects** (D, F), split three ways. Needs no 0018 field (decision 3).
  - **W4a** (D): the tick in `PostData`, `grain_motion`.
  - **W4b** (F, shipped): `ScreenEffect`, presets, `L1761`/`L2392`, extract into `RenderScene.screen_effects`.
  - **W4c** (F, shipped): the `ScreenEffect` pass, the overlay split, `[post_groups] screen_fx`, `docs/screen-effects.md`, gallery
    `screen-effects.yaml` with a `--frames 0,1,2` strip.
- **W5 — view targets** (F, shipped): `target: { view }`, the view's own `screen_fx` chain, the shared cap, gallery `screen-effect-view.yaml`.

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
  default `anamorphic_length` measures about 0.30 of the frame width. Windows goldens committed; macOS twins and the `lens` budgets owed on the pinned machines.
- W2a (`.cube` parser, `lut` field, the composite step 7a) mechanized on the M5 2026-11-21; Windows golden committed; macOS twin owed.
- W2b (`ludion lut identity|bake|apply|stats`, the `grade_model` CPU twin of `grade()`) mechanized on the M5 2026-11-21: a LUT baked from a warm/teal analytic grade renders within 5 output steps (mean 0.05) of the analytic render of the same scene. Scene `post-film-1982-lut` lives in `examples/gallery/` with its Windows golden; the macOS twin is owed.
- W3a (`exposure_mode`, `exposure_compensation`, `metering*`, `exposure_range`, `L1760`, the `exposure_meter` pass) mechanized on the M5 2026-11-21; Windows goldens (`post-metered-exposure`) committed; the macOS twins and the `exposure` budget are owed.
- W3b (`--debug-view exposure_meter`, the metered EV in the `ludion render` summary and `--frames` strip, the pan check `exposure_pan`) mechanized on the M5 2026-11-21: a metered frame and its manual twin render byte-identical with the readback armed or not, and manual scenes' graphs are unmoved. The `ProfileReport` EV field is an R14 item (it moves the protocol schema and `replay_schema_hash`). The view's look is judged by a human at the next sitting.
- W4b (`ScreenEffect` component, presets, `L1761`/`L2392`, extract into `RenderScene.screen_effects`, check-scene's cap warning) mechanized on the M5 2026-11-21; no pass reads it yet, so every frame and `dump-graph` is unchanged.
- W4c mechanized on the RTX 3090 2026-11-21: no-effect renders and `dump-graph` byte-identical (22 files: six gallery scenes plain and SMAA, `ui-hud` at both resamples); golden `screen-effects` blessed (Windows); `screen_fx` 0.072 ms at 1080p (median of three runs, fill `scan` 0.051 + 320x320 `crt` 0.021) vs the 0.15 ms target; macOS twin and the budget ceiling owed.
- W4a (`grain_motion`) mechanized on the RTX 3090 2026-11-21: absent/static left 25 renders and `dump-graph` dumps (seven gallery scenes plain and SMAA, `post-full` at both resamples) byte-identical; animated grain moves per tick in all four owning passes. The M4 check is owed.
- W5 (`target: { view }`) mechanized on the RTX 3090 2026-11-21: the effect runs in the view's own graph as the `overlay -> screen_fx_k` chain over the whole view texture, the cap is shared across fill, region and view, an unknown view is `L1761` at extract and in check-scene; with no view effect, 10 renders and `dump-graph` dumps (`render-view-radar`, `render-view-layers`, `render-view-portrait`, `screen-effects`, `ui-hud`) are byte-identical; golden `screen-effect-view` blessed (Windows). API break: `ScreenEffectTarget` is not `Copy`. macOS twin owed.
- Owed across W1-W5: the macOS twins of the goldens (`post-film-1982`, `post-film-1982-lut`, `post-metered-exposure`, `screen-effects`, `screen-effect-view`); the `lens`, `exposure` and `screen_fx` budget ceilings on both pinned OSes; the game's live sittings (the grain motion trigger, the exposure-meter view's look).
- M4 (2026-11-22, Lore 2449 verified on the pinned Apple M4): the macOS twins of `post-film-1982-lut`, `screen-effects` and `screen-effect-view` are blessed (r2451; `post-film-1982` and `post-metered-exposure` earlier). The film carry through `smaa_blend`/`resolve`/`upscale` and the composite edit keep every committed macOS golden byte-identical.
