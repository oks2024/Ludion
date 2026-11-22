# Plan: Occluded silhouette

> Status: **W1–W2 mechanized** (Neon Shroud request 0015, r2384, W2 r2440). Usage is `ludion docs occluded-silhouette`; the gallery
> scene is `examples/gallery/occluded-silhouette.yaml`.

## Why

Neon Shroud's art direction (§6–§7) wants "a white occlusion silhouette only when hidden behind geometry": a
figure behind a wall or under a truck stays readable, and a figure in the open looks exactly as before. The engine
had no way to draw an entity where something is in front of it. Neon Shroud request 0015 asks for it as a
render-only, per-entity component. A whole-frame stylized outline (`Plan/post-process-pipeline.md`) is a different
decision and is not settled by this.

## Decisions

1. **No stencil, no MRT, no separate fullscreen pass.** The mask is one `R8Unorm` colour target plus one sampled
   depth target. The hidden test and the outline are a pure per-pixel function of bound inputs, so they live in
   `composite` (`Plan/post-process-pipeline.md` decision 2), which already reads `scene_depth`.
2. **Hidden test.** A pixel is hidden when its style is non-zero, the mask depth is below 1, and
   `view_z(scene_depth) < view_z(mask_depth) − bias`, with `view_z` measured along the view forward vector so it
   holds for perspective and ortho alike. Self-occlusion is free: where the body is the front surface the two
   depths are equal.
3. **Style table.** At most 8 distinct styles per frame (`MAX_SILHOUETTE_STYLES`), carried in the composite
   uniform. Style `k` (1-based) rides the mask as `k/255`. A ninth distinct style is not drawn; `L2352` warns once.
4. **Outline.** An outer dilation: a pixel outside every mask is outlined when a filled pixel with an outline lies
   within its radius (8 directions, 1..=r steps) and takes the nearest hit's style. `px` is in 1080p pixels and
   scales with the render height, like `grain_size`. A visible body pixel is never outlined.
5. **Display-referred colour.** Blended over the graded colour, after tonemap, so white is white at any exposure.
6. **Exclusions.** A `Viewmodel` mesh is never in the mask (`L2351` warns once). `Visibility`-hidden meshes are
   never extracted. `Fade` on the silhouetted entity is ignored by the mask; the scene's own fade still shows the
   real body.
7. **Validation.** One code, `L2350`, covers every field (colour channels in `[0,1]`, `outline.px` in `(0,8]`,
   `outline.alpha` in `[0,1]`, `bias` finite and `≥ 0`): one code per component, as `GroundMarkerClip` does. The
   request asked to reuse `L1263`/`L1014`.

8. **Pattern fill (W2a).** Anchored to the screen, in render pixels. `period` is in 1080p pixels within `[2, 64]`;
   the render period is `pr = max(2, round(period*h/1080))`. `duty` is the lit fraction, within `(0, 1)`.
   `scanlines` is lit where `fract(y/pr) < duty`; `hatch` where `fract((x+y)/pr) < duty` (45° lines). An unlit
   pixel has fill alpha 0; outlines stay solid. Screen anchoring is the classic x-ray read and is deterministic; a
   pattern that follows the body needs UVs in the mask, which means MRT, and decision 1 rules MRT out.
9. **Inner outline (W2a).** A filled pixel of style `k` whose inner radius `r` is above 0 is drawn
   `mix(color, tint.rgb, inner_alpha)` when some in-bounds pixel within 8 directions × 1..=r steps is not filled with
   `k`; it also beats the pattern. Pixels outside the frame never count, so the frame border draws no line. The band
   also traces the seam where the body leaves cover.
10. **Film opt-out (W2b).** With `film: false`, the vignette becomes `mix(c, vignette(c), f)` and the
    grain amplitude becomes `grain*f`, where `f = 1 − a`. Under SMAA or a resample, the LDR alpha carries `f` to the
    pass that owns the grain.
11. **Frozen path (W2a).** Every new WGSL branch is gated, so a W1 style runs W1's arithmetic verbatim: a miss
    returns `color` unchanged and a fill does `mix(color, tint.rgb, tint.a)`.

## Waves

- **W1** (this one): the component, the conditional `silhouette_mask` pass, the composite step between the grade and
  the vignette, inheritance down `Parent`, docs, one gallery scene.
- **W2a:** scanline or hatch pattern fills, an inner outline, and the game's slice golden
  (`occluded-silhouette-styles`).
- **W2b:** the per-style `film` option to exclude the silhouette from vignette and grain (decision 10).
- **Carried bodies:** no wave needed. A `ParentJoint` child always carries a `Parent` (`L1318` refuses one without),
  so nearest-ancestor inheritance already reaches it.

## Out of scope / deferred

- **Body-anchored patterns.** Trigger: a game asks for one; needs UVs in the mask (MRT).
- **Silhouettes for sprites, particles, text and ground markers.** Trigger: a game asks for one.
- **Whole-frame stylized outlines.** Trigger: an art-direction decision (`Plan/post-process-pipeline.md`).
- **A stencil or depth-format change.** Trigger: more than 8 styles in one frame, or a second consumer for stencil.

## Owner rulings (2026-11-19)

- Accepted. The interplay with `Fade` on an occluder (interleaved).
- Accepted. Grain and vignette apply to the silhouette (the game's preferred position).
- Accepted. The note in `Plan/post-process-pipeline.md` that this does not settle stylized outlines.
- Accepted. 8 styles per frame.

## Proposed — owner ruling owed (W2)

- Decision 8's screen anchoring of the pattern (not body-anchored).
- Decision 9's seam: the inner band also traces where the body leaves cover.
- Decision 10's fade semantics for `film: false`.

## Status — request 0015

- **W1 mechanized (r2384, 2026-11-18):** the `OccludedSilhouette` component, the `silhouette_mask` pass and the
  composite; scenes without the component keep the same frame graph and pixels. The `occluded-silhouette`
  gallery golden is blessed on the RTX 3090. No replay content hash, `WorldHash`, existing golden or protocol
  schema moved.
- **W2a mechanized (r2440, 2026-11-21):** `pattern` (scanlines or hatch, screen-anchored) and `inner_outline` on
  `OccludedSilhouette`; `L2350` covers the new paths; composite style lanes grew to four vec4 per style (lane 3 `.y`
  is the `film` slot, held at 1 for W2b). The `occluded-silhouette-styles` gallery golden is blessed on the RTX 3090
  (macOS twin owed to the M4). No existing golden, dump-graph, replay, `WorldHash` or protocol schema moved.
- **W2b mechanized (r2440, 2026-11-21):** `film` on `OccludedSilhouette` (default true, not written when true). The composite fades
  the vignette and scales the grain by `f = 1 - a`; under SMAA or a render-scale resample it carries `f` in the LDR alpha
  (`film_carry`) and `smaa_blend`, `resolve` and `upscale` read it from the nearest source texel. The
  `occluded-silhouette-film` gallery golden is blessed on the RTX 3090 (macOS twin and M4 proof of the four edited
  passes owed). No existing golden, replay, `WorldHash` or protocol schema moved.
- M4 (2026-11-22, Lore 2449 verified on the pinned Apple M4): the macOS twins of `occluded-silhouette-styles` and `occluded-silhouette-film` are blessed (r2452). The M4 proof of the four edited passes is done: a full gallery re-render reproduced all 108 committed macOS goldens byte for byte.
