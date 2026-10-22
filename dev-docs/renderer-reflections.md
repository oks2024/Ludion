---
description: Indirect specular: the reflections blend site, surface targets, lobe function, occlusion split
---
# Renderer reflections

**Scope:** How a lit opaque surface gets its indirect specular: the one `reflections` blend site, the targets it reads, the sky resolution, the roughness → lobe function, and the occlusion split. The sky prefilter itself is `ludion devdocs renderer-ibl`; the pass walk is `ludion devdocs renderer-passes`; the design of record is `Plan/reflections.md`.

## Layout
- `crates/ludion-renderer/src/shaders/reflections.wgsl` — the fullscreen additive pass.
- `crates/ludion-renderer/src/lobe.rs` — the CPU lobe function (`cone_half_angle`, `cube_lod`, `mip_roughness`, `chain_max_lod`); `shaders/snippets/lobe.wgsl` is its WGSL twin.
- `crates/ludion-renderer/src/shaders/snippets/{specular_ibl,gi_probe}.wgsl` — binding-free sky specular and the sky-visibility read.

## Dataflow
`scene` (opaque meshes, a multi-target pass) → `reflections` → `transparents` (sky, ground markers, sprites, particles, text). All three write `scene`, and `reflections` and `transparents` also attach or read `scene_depth`; the graph orders them by declaration order (`ludion devdocs ludion-render-graph`).

- `scene` writes the HDR colour plus two surface targets: `scene_normal` (world normal in `xyz`, clamped roughness in `w`; `Rgba16Float`) and `scene_specular` (F0 in `rgb`, specular occlusion in `a`; `Rgba8UnormSrgb`). Specular occlusion is SSAO × the occlusion map × the hit-flash complement.
- `reflections` is the **one blend site**: it adds `sky_specular × ibl_intensity × occlusion` into `scene`. `mesh.wgsl` adds no indirect specular and never samples the specular cube. A scene without an `Environment` adds exactly zero, before any sample.
- The resolution chain is SSR, then the sky cube (see Trace). Every source joins this pass, so the sources blend in one place.

### Pyramids
`PostChain::ssr` is `Some(scale)` when `PostData.ssr > 0` and `Quality::ssr` (`for_scene` fills `QualityParams::FULL.ssr_scale`; `masked` swaps in the machine's, or drops it at LOW). Then two mipped transients are built between `scene` and `reflections`, at output × `scale` (`QualityParams::ssr_scale`, 0.5 at every preset; the output extent, never the render extent), as seed + one reduce per mip (`ssr_pyramid.rs`, `hiz_*.wgsl`, `ssr_color.wgsl`). A scene that does not author `ssr` has none of them, so its graph, dump and goldens are unchanged.
- `hiz`: `R32Float` min-Z of `scene_depth`. Depth is standard-Z (far = 1.0) and a trace may skip a cell only when the ray is in front of the *closest* surface in it, so the cell holds the minimum. fp16 depth has 2^-11 spacing near 1, hence the 32-bit format.
- `ssr_color`: `Rgba16Float` of the **opaque** `scene` (the seed runs before `reflections` adds into it), `hdr_safe` on each seed fetch. The far-depth clear stays out of the hit texel (a hit needs depth < 1), and a roughness-0.3 floor under open sky was rendered with the coarse levels checked: no dark rim at silhouettes.
- **Footprint rule** (`snippets/footprint.wgsl`): child texel `i` covers source `[i·r, (i+1)·r)`, `r = source / target`, touching `floor(lo) ..= ceil(hi) - 1`, clamped. Conservative on odd sizes, where a floor-halving drops the last row or column. One rule serves the seeds and every Hi-Z reduce.
- **Tent and blur**: mip 0 is the area box (the sharp mirror level); each further mip is a separable `[1,3,3,1]/8` tent (variance 0.75 parent-texel²), so stacked σ_m ≈ 0.5·2^m base pixels, and ±σ spans 2^m pixels — the cone diameter `lobe::pyramid_level` maps to level m. A CPU test pins it.

### Trace
With `ssr` on, `ssr_trace` (`ssr_trace.wgsl`, `ssr_trace.rs`) marches one mirror ray per trace texel through `hiz` to a fixed iteration cap, no jitter and no history (the look is `SsrLook::from_post`: `ssr_fade`, `ssr_max_distance`, `ssr_thickness`, `ssr`), and reads `ssr_color` at the lobe's footprint level. It writes radiance in `rgb` and a confidence in `a`, a product of linear ramps (`snippets/ssr.wgsl::ssr_confidence`, twin of `ssr_trace.rs::confidence`): screen edge, ray length, hit thickness, back-facing hit, roughness fade. `reflections` reads it as its fourth input (bound as `refl_ssr`; a 1×1 fallback when off).
- **Upsample**: bilinear over the four nearest trace texels, each weighted by depth (`1 - |Δdist| / 0.1·dist`, squared) and normal (`dot^8`) agreement between the pixel it traced from and the shaded pixel. It is premultiplied by confidence (`Σ rgb·a·w / Σ a·w`, confidence `Σ a·w / Σ w`), so a miss tap beside a hit does not darken its radiance. No agreeing tap gives confidence 0.
- **Blend**: `s = mix(sky_fill, ssr.rgb · (f0·ab.x + ab.y), confidence · intensity)` behind `if (confidence > 0)`, with the same split-sum `ab` as the sky. Intensity scales confidence, so intensity 0 is exactly the sky and confidence 0 is exactly the SSR-off term. SSR takes neither `scene_specular.a` nor sky visibility (decision 7: the trace found its own occluder). An SSR-only frame (no `Environment`) has `sky_fill = 0`.

### Debug views
`ludion render --debug-view ssr_confidence|ssr_footprint` (`DebugView`, `ssr_debug.rs`) replaces `composite` with `ssr_debug` (`ssr_debug.wgsl`), which reads `ssr_trace` and writes the same target, so resample and overlay follow unchanged. It is host state like `--debug-draw`: `RenderScene.debug_view` → `PostChain.ssr_view`, never in a scene file or a `WorldHash`. Live hosts reach it through `render.debug`'s `view` (`Engine::set_debug_view`, extracted into `RenderScene.debug_view`). **A view never turns SSR on**: `for_scene` and `masked` drop it whenever `ssr` is `None`, the frame is the normal one, and `RenderOutcome.debug_view` (the view actually drawn) and the CLI summary say so.
- **Confidence**: gray = the trace's `a`. **Footprint**: the trace writes `level_f` in `r` when `proj_levels.z` is 1 (set from the chain's view); gray = `level_f / (levels − 1)`, magenta `(255, 0, 255)` where `a == 0` (no confident hit: a miss, a back-face or past the fade band), so it is not level 0.
- The pass reads the trace texel **nearest** the render pixel (no upsample), and stores `srgb_to_linear(gray)` to the sRGB target so the byte is `round(value·255)` and `image-stats` reads the data directly. The transfer pair lives in `snippets/srgb.wgsl`, shared with `composite`.
- Bloom, shafts and dof still schedule on a view frame with unread outputs; accepted for a debug frame.

## Rules
- **The lobe function is one function for every path.** `cone_half_angle(r) = 2·atan(r²)`: 0 for a mirror, exactly π/2 at r = 1. The cube lod solves the lobe's solid angle against K texels, K = 32π, so r = 1 fills the 8×8 face and the chain stops there. A one-texel match sent rough metal to 1×1 faces and made a cube-face artifact on the gallery night-sky tile. `mip_roughness` is the exact inverse of `cube_lod` above mip 0, so the prefilter and the lookup agree by construction; the `needs_gpu_` test in `lobe.rs` holds `lobe.wgsl` to the Rust. Change one twin, change both.
- **Both ends clamp.** `cube_lod` clamps to `[0, max_lod]`, so a mirror reads mip 0 (below r ≈ 0.149 the lobe is under 1.2° on 0.45° texels) and r = 1 never leaves the 8×8 face; `pyramid_level` clamps the half-angle (`MAX_HALF_ANGLE`) to keep `tan` finite.
- **Occlusion is split by source** (`Plan/reflections.md` decision 7). GI sky visibility darkens only the sky fill, through `sky_visibility_at` (`gi_probe.wgsl`, the one function that reads the probe volume); SSAO and the occlusion map apply to every source via `scene_specular.a`. Applying sky visibility to a source that already contains its own occlusion double-darkens it. With SSR on, the hit-flash complement no longer suppresses the SSR term, because `spec.a` packs SSAO × occlusion × flash and SSR skips all of it; the flash is a few frames of white emissive override that swamp it.
- **The snippets are binding-free.** Textures and samplers arrive as parameters so any pipeline composes them. A snippet that declares a `@group` binding collides with the host's.
- **The ceiling is enforced at the read.** The opaque pass and `reflections` each store up to `HDR_CEILING`, and their One+One sum may pass half-float's max. Every reader of `scene` (`threshold`, `dof_coc`, `composite`) runs `hdr_safe` on the fetch, which reads an infinity or NaN as the ceiling or 0 — the old clamp-the-sum-once result.
- **Far depth is masked.** A depth ≥ 1.0 texel is the sky, which `transparents` draws; without the mask it picks up `prefiltered × ab.y` even at zero F0.

## Gotchas
- `transparents` has no pass-timing budget of its own: sprite and particle stress budgets' `scene` lines cover the opaque meshes only.
- **`ssr_source_pixel` is shared** by the trace and the upsample (`snippets/ssr.wgsl`): the render pixel a trace texel traced from. Change one use, change both.
- **fp16 residual stands**: the blend adds into `scene` after the opaque pass, so opaque + reflection can pass the half-float max (`hdr_safe` at the readers hides it). The exact fix is a `scene_opaque` target with `reflections` writing `hdr_safe(opaque + s)` unblended; it costs a full-res target and can move bytes, so it lands as a follow-up with the next re-bless.
- No transparent meshes exist (`L1202` refuses the glTF feature), so only `reflections` calls `sky_specular`.

- The culling HZB (`gpu-driven-culling.md` rung 3b) needs the *farthest* depth, built from its own occluder pre-pass. It reuses this machinery (the `R32Float` format, `footprint.wgsl`, the reduce shader with `max`), not this frame's `hiz`; no max channel exists.
- `R32Float` is unfilterable: `hiz_src` is listed in `UNFILTERABLE_TEXTURES` (`ludion-shader-bindgen`), and the pyramid shaders read with `textureLoad`.

## Verify
```
cargo test -p ludion-renderer lobe::
cargo test -p ludion-renderer needs_gpu_reflections
cargo test -p ludion-renderer ssr_pyramid::
cargo test -p ludion-renderer needs_gpu_ssr
cargo test -p ludion-renderer needs_gpu_a_debug_view
cargo test -p ludion-cli --test render debug_view
cargo test -p ludion-renderer needs_gpu_metal_reflects
cargo test -p ludion-renderer needs_gpu_a_reflection_over_a_bright
cargo run -p ludion-cli -- dump-graph --scene examples/gallery/night-sky.yaml   # scene → reflections → transparents
```
