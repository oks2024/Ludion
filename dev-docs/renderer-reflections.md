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
- The resolution chain is the sky cube only. Every source added later joins this pass, so the sources blend in one place.

## Rules
- **The lobe function is one function for every path.** `cone_half_angle(r) = 2·atan(r²)`: 0 for a mirror, exactly π/2 at r = 1. The cube lod solves the lobe's solid angle against K texels, K = 32π, so r = 1 fills the 8×8 face and the chain stops there. A one-texel match sent rough metal to 1×1 faces and made a cube-face artifact on the gallery night-sky tile. `mip_roughness` is the exact inverse of `cube_lod` above mip 0, so the prefilter and the lookup agree by construction; the `needs_gpu_` test in `lobe.rs` holds `lobe.wgsl` to the Rust. Change one twin, change both.
- **Both ends clamp.** `cube_lod` clamps to `[0, max_lod]`, so a mirror reads mip 0 (below r ≈ 0.149 the lobe is under 1.2° on 0.45° texels) and r = 1 never leaves the 8×8 face; `pyramid_level` clamps the half-angle (`MAX_HALF_ANGLE`) to keep `tan` finite.
- **Occlusion is split by source** (`Plan/reflections.md` decision 7). GI sky visibility darkens only the sky fill, through `sky_visibility_at` (`gi_probe.wgsl`, the one function that reads the probe volume); SSAO and the occlusion map apply to every source via `scene_specular.a`. Applying sky visibility to a source that already contains its own occlusion double-darkens it.
- **The snippets are binding-free.** Textures and samplers arrive as parameters so any pipeline composes them. A snippet that declares a `@group` binding collides with the host's.
- **The ceiling is enforced at the read.** The opaque pass and `reflections` each store up to `HDR_CEILING`, and their One+One sum may pass half-float's max. Every reader of `scene` (`threshold`, `dof_coc`, `composite`) runs `hdr_safe` on the fetch, which reads an infinity or NaN as the ceiling or 0 — the old clamp-the-sum-once result.
- **Far depth is masked.** A depth ≥ 1.0 texel is the sky, which `transparents` draws; without the mask it picks up `prefiltered × ab.y` even at zero F0.

## Gotchas
- `transparents` has no pass-timing budget of its own yet: sprite and particle stress budgets' `scene` lines cover the opaque meshes only.
- No transparent meshes exist (`L1202` refuses the glTF feature), so only `reflections` calls `sky_specular`.

## Verify
```
cargo test -p ludion-renderer lobe::
cargo test -p ludion-renderer needs_gpu_reflections
cargo test -p ludion-renderer needs_gpu_metal_reflects
cargo test -p ludion-renderer needs_gpu_a_reflection_over_a_bright
cargo run -p ludion-cli -- dump-graph --scene examples/gallery/night-sky.yaml   # scene → reflections → transparents
```
