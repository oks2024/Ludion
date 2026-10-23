---
description: Screen-space reflections: the Hi-Z and colour pyramids, the cone trace, its blend and debug views
---
# Renderer SSR

**Scope:** The opt-in screen-space reflection trace (`PostProcess.ssr`): the pyramids it marches and reads, the trace and its confidence, how it blends over the probe and sky fill, and the SSR debug views. The `reflections` blend site and the lobe function are `ludion devdocs renderer-reflections`; the probes it blends over are `ludion devdocs renderer-probes`; the design of record is `Plan/reflections.md`.

## Layout
- `crates/ludion-renderer/src/ssr_pyramid.rs`, `shaders/{hiz_*,ssr_color}.wgsl` — the pyramids.
- `crates/ludion-renderer/src/ssr_trace.rs`, `shaders/ssr_trace.wgsl`, `shaders/snippets/{ssr,footprint,srgb}.wgsl` — the trace, its confidence and the shared footprint rule.
- `crates/ludion-renderer/src/debug_view.rs`, `shaders/ssr_debug.wgsl` — the debug views.

## Dataflow
### Pyramids
`PostChain::ssr` is `Some(scale)` when `PostData.ssr > 0` and `Quality::ssr` (`for_scene` fills `QualityParams::FULL.ssr_scale`; `masked` swaps in the machine's, or drops it at LOW). Then two mipped transients are built between `reflections` and `transparents`, at output × `scale` (`QualityParams::ssr_scale`, 0.5 at every preset; the output extent, never the render extent), as seed + one reduce per mip (`ssr_pyramid.rs`, `hiz_*.wgsl`, `ssr_color.wgsl`). A scene that does not author `ssr` has none of them, so its graph, dump and goldens are unchanged.
- `hiz`: `R32Float` min-Z of `scene_depth`. Depth is standard-Z (far = 1.0) and a trace may skip a cell only when the ray is in front of the *closest* surface in it, so the cell holds the minimum. fp16 depth has 2^-11 spacing near 1, hence the 32-bit format.
- `ssr_color`: `Rgba16Float` of the lit `scene` **with** the probe and sky fill (the seed runs after `reflections` has added it, so a hit shows the look an SSR-off camera sees), `hdr_safe` on each seed fetch. The far-depth clear stays out of the hit texel (a hit needs depth < 1), and a roughness-0.3 floor under open sky was rendered with the coarse levels checked: no dark rim at silhouettes.
- **Footprint rule** (`snippets/footprint.wgsl`): child texel `i` covers source `[i·r, (i+1)·r)`, `r = source / target`, touching `floor(lo) ..= ceil(hi) - 1`, clamped. Conservative on odd sizes, where a floor-halving drops the last row or column. One rule serves the seeds and every Hi-Z reduce.
- **Tent and blur**: mip 0 is the area box (the sharp mirror level); each further mip is a separable `[1,3,3,1]/8` tent (variance 0.75 parent-texel²), so stacked σ_m ≈ 0.5·2^m base pixels, and ±σ spans 2^m pixels — the cone diameter `lobe::pyramid_level` maps to level m. A CPU test pins it.

### Trace
With `ssr` on, `ssr_trace` (`ssr_trace.wgsl`, `ssr_trace.rs`) marches one mirror ray per trace texel through `hiz` to a fixed iteration cap, no jitter and no history; a level-0 candidate is resolved against the render pixels the ray crosses, the first one it is behind decides (a hit within the thickness, an occluded miss beyond it) and the hit's normal, colour and thickness come from that pixel (the look is `SsrLook::from_post`: `ssr_fade`, `ssr_max_distance`, `ssr_thickness`, `ssr`), and reads `ssr_color` at the lobe's footprint level. It writes radiance in `rgb` and a confidence in `a`, a product of linear ramps (`snippets/ssr.wgsl::ssr_confidence`, twin of `ssr_trace.rs::confidence`): screen edge, ray length, hit thickness, back-facing hit, roughness fade. `ssr_blend` reads it (`reflections` no longer does); with SSR on `reflections` also writes the fill to a second target, `refl_fill`, which the blend subtracts.
- **Upsample**: bilinear over the four nearest trace texels, each weighted by depth (`1 - |Δdist| / 0.1·dist`, squared) and normal (`dot^8`) agreement between the pixel it traced from and the shaded pixel. It is premultiplied by confidence (`Σ rgb·a·w / Σ a·w`, confidence `Σ a·w / Σ w`), so a miss tap beside a hit does not darken its radiance. No agreeing tap gives confidence 0.
- **Blend** (`ssr_blend.wgsl`, additive into `scene` after the trace): it adds `confidence · intensity · (hdr_safe(ssr.rgb · (f0·ab.x + ab.y)) - fill)`, with `fill` read from `refl_fill`, so the pixel ends at `mix(fill, hit, confidence)`; the add may be negative. It returns exactly 0 where the depth is far or the confidence is 0, so intensity 0 and confidence 0 are exactly the SSR-off frame. SSR takes neither `scene_specular.a` nor sky visibility (decision 7: the trace found its own occluder). An SSR-only frame (no `Environment`) has `fill = 0`.

### Why SSR and a probe still differ
Where the trace is fully confident, SSR-only and probe-only frames agree on a chrome prop's brightness (region luma ratio 1.02) but not per pixel (mean relative luma step 0.38). That residual is placement, not the fill: a probe captures from one point and a sphere volume reprojects it (parallax and cube resolution), while SSR reads the screen, so near content lands a few pixels apart. Dielectric pillars reflected in the same floor show a larger step (0.52, ratio 0.74), so the parity test bounds the chrome props by the dielectric reference instead of an absolute budget (`reflection_parity.rs`, `POP_K`). Before the colour pyramid carried the fill the step was 1.0 and the ratio 0 (the props traced black).

### Debug views
`ludion render --debug-view ssr_confidence|ssr_footprint|probe_index` (`DebugView`, `debug_view.rs`) replaces `composite` with `ssr_debug` (`ssr_debug.wgsl`), which reads `ssr_trace` and writes the same target, so resample and overlay follow unchanged. It is host state like `--debug-draw`: `RenderScene.debug_view` → `PostChain.ssr_view`, never in a scene file or a `WorldHash`. Live hosts reach it through `render.debug`'s `view` (`Engine::set_debug_view`, extracted into `RenderScene.debug_view`). **A view never turns SSR on**: `for_scene` and `masked` drop it whenever `ssr` is `None`, the frame is the normal one, and `RenderOutcome.debug_view` (the view actually drawn) and the CLI summary say so.
- **Confidence**: gray = the trace's `a`. **Footprint**: the trace writes `level_f` in `r` when `proj_levels.z` is 1 (set from the chain's view); gray = `level_f / (levels − 1)`, magenta `(255, 0, 255)` where `a == 0` (no confident hit: a miss, a back-face or past the fade band), so it is not level 0.
- The pass reads the trace texel **nearest** the render pixel (no upsample), and stores `srgb_to_linear(gray)` to the sRGB target so the byte is `round(value·255)` and `image-stats` reads the data directly. The transfer pair lives in `snippets/srgb.wgsl`, shared with `composite`.

- Bloom, shafts and dof still schedule on a view frame with unread outputs; accepted for a debug frame.

## Gotchas
- **`ssr_source_pixel` is shared** by the trace and the upsample (`snippets/ssr.wgsl`): the render pixel a trace texel traced from. Change one use, change both.
- The culling HZB (`gpu-driven-culling.md` rung 3b) needs the *farthest* depth, built from its own occluder pre-pass. It reuses this machinery (the `R32Float` format, `footprint.wgsl`, the reduce shader with `max`), not this frame's `hiz`; no max channel exists.
- `R32Float` is unfilterable: `hiz_src` is listed in `UNFILTERABLE_TEXTURES` (`ludion-shader-bindgen`), and the pyramid shaders read with `textureLoad`.

## Verify
```
cargo test -p ludion-renderer ssr_pyramid::
cargo test -p ludion-renderer needs_gpu_ssr
cargo test -p ludion-renderer needs_gpu_a_debug_view
cargo test -p ludion-cli --test render debug_view
```
