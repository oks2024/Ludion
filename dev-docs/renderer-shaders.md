---
description: Authoring WGSL in the renderer: generated bindings, shared snippets, and dev-flag hot reload
---
# Shader authoring

**Scope:** What an agent editing a `.wgsl` file in `crates/ludion-renderer/src/shaders/` needs at the point of editing. The generator's own model/emitter split and unification rules are the `ludion-shader-bindgen` topic.

The CPU-side bindings for every shader in `shaders/` — uniform/storage structs, binding-index
consts, `create_*_bgl` functions, `*_VS_MAIN_INPUTS` — are **generated**, not hand-written.

- **Adding a shader is three edits, all in `render.rs`:** a `ShaderId` variant, a `SHADERS` registry
  row (file name, compiled-in body, composed snippets, the layouts its pipeline binds), and a
  `build_pipeline` arm for the per-shader construction. The registry is the single place the GPU
  side and the drift gate agree about what exists.
- **Edit the WGSL, regenerate, never edit `shader_gen.rs`.** After any change to a file in
  `shaders/` (a struct field, a binding, a hoisted `const`), run `node scripts/check.mjs --fix` (or
  directly, `cargo run -p ludion-shader-bindgen`) before building — the committed
  `crates/ludion-renderer/src/shader_gen.rs` only reflects what's on disk after that step.
- **The drift gate catches a forgotten regenerate**, not a silent mismatch:
  `cargo run -p ludion-shader-bindgen -- --check` (the `shader-drift` step in `scripts/check.mjs`'s
  Rust lane, which calls `ci/scripts/shader-drift.mjs`) fails loudly and prints the fix ("Regenerate
  it with `cargo run -p ludion-shader-bindgen` and commit the result") whenever committed bytes
  disagree with a fresh reflection.
- **Shared snippets live in `shaders/snippets/` and are *composed* (prepended), not `#include`d**
  (WGSL has no preprocessor). A shader's compiled source = its snippets' text, in the declared order,
  then its own body, joined by `\n` — the same string the pipeline's `create_shader_module`, the
  bindgen reflection, and the hot-reload rebuild all see. A shader body must **not** redeclare
  anything its snippets provide. `camera.wgsl` and `lights.wgsl` exist so the `Camera`/`Light`/
  `Lights` declarations come from one place; `bayer.wgsl` is one `BAYER4` tile for the whole engine
  rather than one per consumer; `hdr.wgsl` holds `HDR_CEILING` (60 000, under half-float's 65 504)
  and `hdr_safe` — the mesh output and the bloom threshold, downsample and upsample clamp through it,
  and it reads NaN as 0 by a bit test (WGSL has no `isnan`); `medium.wgsl` and `sky_gradient.wgsl` are shared *because* two
  shaders each must not disagree about the medium's shape or the sky's colour
  (`ludion devdocs renderer-medium`); `glyph.wgsl` holds the atlas bindings and the coverage fragment
  that the world-space and UI text passes share. Editing a snippet is a real change: regenerate and re-check
  like any WGSL edit, and a snippet edit that alters a layout makes the drift gate fail.
- **The snippet mapping lives in two mirrored places** — the `SHADERS` registry rows in `render.rs`
  (what the GPU compiles) and `ludion_shader_bindgen::COMPOSITIONS` (what the drift gate reflects).
  A renderer test (`renderer_composition_matches_bindgen_registry`) byte-compares the two, so a
  snippet added, dropped or reordered on only one side fails loudly.
- **Two shaders carrying the same value on purpose stay name-distinct**, so bindgen never tries to
  unify them. `mesh.wgsl`'s group-2 `Sun` uniform and `shadow.wgsl`'s group-0 `ShadowView` both carry
  the scene's `light_space` matrix (uploaded from one `SunData.light_space`), but `Sun` also carries
  `direction`/`color` while `ShadowView` is matrix-only. Reusing one struct name across them would
  hard-error the moment their shapes drifted at all; naming them apart makes that a non-issue by
  construction. `shadow.wgsl` composes only `skin.wgsl` — it needs neither the camera nor the
  lights binding, only its own light-space matrix. Its sibling `shadow_cutout.wgsl` composes
  `skin.wgsl` + `mesh_material.wgsl` so its fragment stage binds the asset's material and discards on
  the same coverage test as the scene pass.
- **What stays hand-written on purpose:** the vertex-buffer *structs* (`MeshInstanceRaw`,
  `SpriteInstanceRaw`, `TextVertexRaw`, `UiRectInstanceRaw`, `UiTextVertexRaw` in `render.rs`), the
  per-vertex/per-instance buffer split, and vertex step modes — WGSL doesn't express any of that,
  only the `@location`/format list, which the generated `*_VS_MAIN_INPUTS` arrays pin against a
  hand-written renderer test.

## Hot reload (dev flag only)

Outside a dev-flagged host, shaders are `include_str!`-compiled in, exactly like every other
build — goldens, `ludion render`, and every test never touch disk for shaders, so there is no
divergence risk between the shipping path and the files a shader author edits. Under a dev flag
(`ludion serve --dev-shaders`, a game run with `--dev-shaders`, or `ludion dev`, which sets it
automatically when run from inside the engine workspace), a `.wgsl` path routes through the
existing `asset.reload` command instead of an engine rebuild:

- **Not dev-flagged:** `asset.reload` on a shader path is a structured `L2008` naming the three
  ways to enable the flag.
- **Dev-flagged, valid edit:** the renderer re-validates the new source with naga (the same
  front-end/validator `create_shader_module` runs) and, on success, recreates the shader module and
  every pipeline built from it. A **shader body** edit rebuilds its one pipeline; a **shared snippet**
  edit rebuilds every dependent pipeline at once (editing `snippets/fullscreen_vs.wgsl` rebuilds
  every post pass), composed against the current source of the other parts.
  Measured edit→pixels latency over the wire is single-digit-to-low-tens of ms — see
  `crates/ludion-server/tests/shader_reload.rs`.
- **Dev-flagged, bad edit (parse/validation failure, or a recreated pipeline incompatible with the
  existing bind group layouts — a binding-shape change):** a structured `L2009` naming the reason,
  and **the last good pipeline stays live** — a bad edit never kills the session, it just doesn't
  take. A snippet reload is **transactional**: if any dependent fails, *all* dependents keep their
  last-good pipeline, so there is no half-applied snippet edit.
- Reloaded shader state is render-side only: no sim state, no `WorldHash` interaction, nothing for
  snapshot/restore to carry.

`ludion dev` watches `crates/ludion-renderer/src/shaders/` **recursively** (so a `snippets/*.wgsl`
edit is picked up too) only when running from inside the engine workspace, and on a `.wgsl` change
sends `asset.reload` over the live connection instead of the usual rebuild→snapshot→restart cycle —
sub-second edit→pixels against the ~3.1 s release rebuild path. The designer-facing version of that
loop is `ludion docs designer-flow`.

## Gotchas
- A hand edit to `shader_gen.rs` is not a shortcut — it reverts, or diverges and fails `--check`,
  the moment anyone regenerates. Edit the `.wgsl` in `shaders/` and regenerate.
- **A cross-shader unification error** — `struct X is defined in a.wgsl and b.wgsl with different
  layouts: ...`, or the equivalent for a resource binding or const — means two shaders declared the
  same-named struct/resource/const with disagreeing shape. With the shared snippets above it fires
  mainly when a *body* redeclares a snippet type, or two bodies share a new struct; the fix is
  almost always to make the WGSL declarations agree (or move the shared declaration into a snippet),
  not to work around the generator.

The frame graph and this crate's layout are `ludion devdocs ludion-renderer`; the per-pass walk is `ludion devdocs renderer-passes`.
