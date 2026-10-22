---
description: Reflection probes in the renderer: capture, ranked shading, the probe store, bakes and dumps
---
# Renderer probes

**Scope:** How a `ReflectionProbe` becomes pixels: the six-face capture, the ranked cluster-mask shading in `reflections`, the content-addressed store and its hosts, and the dump and observability surfaces. The authoring side (the field table, placing, the bake commands) is `ludion docs reflection-probes`; the blend site is `ludion devdocs renderer-reflections`; the trace blended over probes is `ludion devdocs renderer-ssr`; the design of record is `Plan/reflections.md`.

## Layout
- `crates/ludion-renderer/src/probe_capture.rs`, `shaders/probe_face.wgsl` — the probe capture (below).
- `crates/ludion-renderer/src/probe_shade.rs`, `shaders/snippets/{probes,cluster_grid}.wgsl` — the ranked probe list and cell masks, the binding-free probe maths (`probe.rs` holds the CPU twins), and the cluster cell lookup shared with `lights.wgsl`.

## Dataflow
### Probe capture
`probe_capture.rs` captures each `RenderScene.probes` entry once, at the top of `prepare_frame`: six faces of `QualityParams::ibl_specular_size` (no new quality field), prefiltered by `Ibl::prefilter_probe` into slot `i` of a renderer-owned cube array (`ProbeCubes`: texture, cube-array view, probe → slot table, face size, `max_lod`). The in-memory cache key is the face size plus each probe's entity and position, so the world is captured once; a scene without probes frees the array and does no GPU work.
- **Derived scene**: each face is an ordinary frame (`prepare_frame` + `build_probe_capture_graph` + `Gpu::execute_into`) of a clone of the scene with the sprites, particles, text, ground markers, UI, debug lines, probes and `debug_view` emptied, `ssao`/`ssr`/`shafts`/`dof` zeroed, and the sun disc off (the IBL excludes it, so a probe does). Textures, mesh assets, lights, sun, GI and voxel stay, so every cache is a hit. Passes: shadow, gi_inject, scene, reflections, transparents, then `probe_face`.
- **Faces and the flip**: wgpu layer order +X, −X, +Y, −Y, +Z, −Z; up = +Y, +Y, −Z, +Z, +Y, +Y; world-axis-aligned (a probe's rotation belongs to its box proxy). The cube convention is left-handed against `look_to_rh`, so a face renders unmirrored and `probe_face.wgsl` flips x at store. Never flip the projection: it reverses the winding and fights `is_mirrored`. `capture_faces_match_the_cube_convention` pins it.
- **Alpha is sky visibility**: `probe_face` writes `hdr_safe(scene.rgb)` with alpha 1 where `scene_depth >= 1.0` and 0 on geometry (`scene`'s own alpha is 1 for both). The rgb keeps the sky, so shading needs no relight.

### Probe shading
`build_probe_list` ranks the extract indices by `probe_rank`; GPU probe `k` is rank `k`, and bit `k` of a cluster cell's `u32` mask (`probe_masks`, bounding spheres through the lights' margins) says the probe may reach it. The pass walks a cell's set bits lowest first, takes at most four with `weight > 0`, each `weight × (1 − coverage)`, and `fill = Σ probe · (F0·ab.x + ab.y) · spec.a + sky_fill · (1 − coverage)`; SSR then mixes over `fill`. Occlusion: the probe term takes specular occlusion only, sky visibility stays on the sky fill. Box lookups intersect the oriented box, spheres the sphere (`probe_lookup`).
- **Sky-fraction intensity**: `rgb · (1 − a + a·k)` with `a` the prefiltered alpha and `k` = `ibl_intensity` (1 with no `Environment`), so captured sky matches the sky fill and captured geometry stays physical. The interim form of the W4 relight.
- **Transparents**: `probes.wgsl` is binding-free so a transparent mesh composes it; none exists. Its fragment would bind 13 mesh textures + sky cube + BRDF LUT + probe array = 16, the floor, and one storage buffer (the mask).
- A frame with no probe uploads count 0, binds a 1×1 fallback array and is byte-identical to a probe-less renderer.

### Probe store
`Renderer::set_probe_store(Some(dir))` (default `None`: capture fresh) makes resolve load `<dir>/<key:016x>.ktx2` per probe, or capture, read the slot back and write the entry. Hit, miss and store-less slots are byte-identical (`probe_store.rs`).
- **Key** = `probe_key(world_key(face_scene(scene, ZERO, 0), …), position)`. It is built from the *face scene*, so a sprite, the sun disc, `ssr` or the camera never moves it. `world_key` destructures `RenderScene` with no `..`: a new field is a compile error until it is hashed or its exclusion argued. Small `Copy` structs hash through `Debug` (every field, automatically); meshes by `MeshData::hash` plus each map's `identity()`, wrap and scales; the voxel grid by `hash`. Machine terms: `QualityParams`, face size, `PROBE_BAKE_VERSION` (bump when capture or prefilter bytes change for the same input). The store is per machine; across machines the tier is perceptual.
- **Position only.** The capture is world-axis-aligned, so shape, blend, priority and rotation shade but never rebake.
- **Entry**: KTX2, `R16G16B16A16_SFLOAT`, six faces, mips smallest first, each mip's faces back to back (`ludion_assets::cooked::{write,read}_probe_cube`). A missing or invalid entry is a miss and is rewritten. Writes go to `<name>.<pid>.tmp` then `rename`; a failed write is dropped.
- Stale entries are never evicted.
- **Hosts.** `Engine::set_probes(ProbeSource)` is the render-only axis (never hashed or recorded). `run`, embed and `serve` host `Store` (the dir is `<project>/.ludion/cooked/probes`, the project being the `ludion.toml` ancestor, else the project root) unless `--probes rebake`, which `dev` forwards to every child. `render`, `test`, goldens, `--profile` and `ludion profile` stay `Fresh`. A `Store` miss is an `L1279` warning. `Engine::bake_probes(force)` is the bake core: it forces cooked textures and the store, because a cooked map's identity is its cook key, so a `--textures source` host misses a bake's entries.
- **Dump.** `ludion render --scene S --dump-probes DIR` (`Renderer::read_probes`, `probe_dump.rs`) writes `probe-<rank>-captured.png` (the six raw faces) and `probe-<rank>-prefiltered.png` (a row per mip) as unexposed sRGB bytes, plus `probes.json` (with a per-probe `capture_ms`).
- **Observability.** `FramePhase::ProbeBake` brackets each captured probe (loaded ones are not), feeding `LoadProfile.probe_bake_ms`; `ProfileReport.probes` is hits against captures; the `probes` debug-draw category (`extract::probe_debug_lines`) and `render.debug` `view: "probe_index"` are the live views.
- It captures fresh and never touches the store; `stored` only says a `Store` host at this quality would find the entry. Keys come from the same `store_keys` as `resolve`, taken from a cooked-texture host's extract (`key_source: "store-host"`; `"dump"` under `--set`), so they match a bake's.

### Debug view
- **`probe_index`**: `probe_debug` (`probe_debug.wgsl`) replaces `composite` the same way via `PostChain.probe_view`, set only when the scene has probes (else the normal frame is drawn and the summary says so). Same mask walk as `reflections`; the winner is the largest `weight × (1 − coverage)`.
  Colour = `PROBE_VIEW_PALETTE[rank % 8]` (sRGB) × that contribution; gray 64 where no probe weights the surface, black where there is no surface.

## Rules
- Probe shape, blend, priority and rotation shade but never rebake: the capture is world-axis-aligned and keyed on position (`Probe store`).

## Verify
```
cargo test -p ludion-renderer probe_capture::
cargo test -p ludion-renderer probe_store::
cargo test -p ludion-renderer probe::
cargo test -p ludion-renderer cluster::
cargo test -p ludion-renderer probe_shade::
cargo test -p ludion-assets cooked
cargo test -p ludion-renderer graph::tests::the_probe_capture
```
