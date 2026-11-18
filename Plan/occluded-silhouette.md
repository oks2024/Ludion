# Plan: Occluded silhouette

> Status: **W1 mechanized** (Neon Shroud request 0015, r2384). Usage is `ludion docs occluded-silhouette`; the gallery
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

## Waves

- **W1** (this one): the component, the conditional `silhouette_mask` pass, the composite step between the grade and
  the vignette, inheritance down `Parent`, docs, one gallery scene.
- **W2:** scanline or hatch pattern fills, a per-style option to exclude the silhouette from vignette and grain, an
  inner outline, and the game's slice golden.
- **Later:** carried bodies via `ParentJoint`, if `Parent` inheritance does not already follow them.

## Out of scope / deferred

- **Pattern fills, inner outline, a per-style post opt-out.** Trigger: W2, when the game's slice needs them.
- **Silhouettes for sprites, particles, text and ground markers.** Trigger: a game asks for one.
- **Whole-frame stylized outlines.** Trigger: an art-direction decision (`Plan/post-process-pipeline.md`).
- **A stencil or depth-format change.** Trigger: more than 8 styles in one frame, or a second consumer for stencil.

## Proposed — owner ruling owed

- The interplay with `Fade` on an occluder (interleaved).
- Grain and vignette apply to the silhouette (the game's preferred position).
- The note in `Plan/post-process-pipeline.md` that this does not settle stylized outlines.
- 8 styles per frame.

## Status — request 0015

- **W1 mechanized (r2384, 2026-11-18):** the `OccludedSilhouette` component, the `silhouette_mask` pass and the
  composite; scenes without the component keep the same frame graph and pixels. The `occluded-silhouette`
  gallery golden is blessed on the RTX 3090. No replay content hash, `WorldHash`, existing golden or protocol
  schema moved.
