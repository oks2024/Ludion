---
description: naga-driven WGSL reflection that generates the renderer's CPU-side shader bindings
---
# ludion-shader-bindgen

**Scope:** `ludion-shader-bindgen` — an internal, unpublished workspace crate (`publish = false`)
that reflects the engine's WGSL shaders with naga and emits the CPU-side bindings the renderer
compiles against: `crates/ludion-renderer/src/shader_gen.rs`. Covers the model/emitter split, running and drift-checking the generator, the cross-shader unification rules. It does not cover the renderer's shading model
(`ludion devdocs renderer-shading`) or the shader hot-reload dev flow
(`ludion devdocs renderer-shaders`, `designer-flow`) — those consume this crate's output but are not this crate. Read before editing WGSL that fails the drift gate, adding a shader to the reflected set, or touching `src/reflect.rs`/`emit.rs`/`model.rs`.

## Spokes
- `ludion devdocs shader-bindgen-pipeline` — how reflect and emit work end to end, the drift gate, and what is not generated.

## Layout
- `crates/ludion-shader-bindgen/src/model.rs` — the wgpu-free reflection model: `Reflection`
  (`structs`, `bindings`, `bind_groups`, `consts`, `entry_points`, `units`), `StructLayout`/
  `FieldLayout` (name, offset, size, `TypeRef`), `BindingResource` (group, binding, `BindingKind`,
  merged `visibility: Vec<ShaderStage>`), `BindGroupLayout`, `ScalarConst`, `EntryPoint`/
  `VertexInput`. No wgpu type appears here (decision 6) — only plain structs/enums.
- `crates/ludion-shader-bindgen/src/reflect.rs` — `ShaderUnit` (`new` for a raw file, `with_preamble`
  for a preamble-then-file composed unit — the compose-then-reflect seam, decision 15) and
  `reflect(&[ShaderUnit]) -> Result<Reflection, ReflectError>`: naga `wgsl-in` parse + validate per
  unit, `naga::proc::Layouter`-driven struct/array layout, per-entry-point `GlobalUse` → binding
  visibility, then cross-unit unification (see Rules).
- `crates/ludion-shader-bindgen/src/emit.rs` — `emit(&Reflection) -> String`: the deterministic,
  rustfmt-clean text of `shader_gen.rs` — header + hoisted consts + `#[repr(C)]`/`Pod`/`Zeroable`
  structs with baked `size_of`/`offset_of` asserts + binding-index consts + `create_*_bgl`
  functions + `*_VS_MAIN_INPUTS` — in sorted, byte-stable order (decision 7).
- `crates/ludion-shader-bindgen/src/error.rs` — `ReflectError`: `Parse`/`Validate`/`Layout`/
  `UnsupportedStage` (per-unit naga failures) and `StructConflict`/`BindingConflict`/
  `ConstConflict` (cross-unit disagreements), each a plain, agent-readable `Display` message
  naming both units and the offending field/detail — no `L####` codes (decision 8: this is
  engine-internal dev tooling, not a shipped diagnostic surface).
- `crates/ludion-shader-bindgen/src/lib.rs` — besides the reflect/emit API, the [`COMPOSITIONS`]
  registry (`&[(shader, &[snippet])]`) + `snippets_for(name)`: the bindgen half of the shared-snippet
  mapping (decision 16). The renderer mirrors the same mapping at compile time (the `SHADERS`
  registry rows / the `Snippet` enum in `render.rs`); the two cannot be physically merged (the renderer needs
  compile-time `include_str!` composition and must not depend on this crate; this crate must not drag
  wgpu in by depending on the renderer), so a renderer test
  (`renderer_composition_matches_bindgen_registry`) byte-compares both compositions per shader and
  fails loudly on any drift.
- The lib also owns the shader list — `RENDERER_SHADERS` (`crates/ludion-renderer/src/shaders/`),
  `RENDER_GRAPH_SHADERS` (`crates/ludion-render-graph/src/shaders/`), both sorted, and
  `COMPUTE_SHADERS` (the renderer shaders built outside `ShaderId` with auto-derived layouts) — and
  `load_units(workspace)`, which reads them and composes each one's snippets per `snippets_for`.
- `crates/ludion-shader-bindgen/src/main.rs` — the thin bin: `load_units`, reflect, emit, and either
  write `crates/ludion-renderer/src/shader_gen.rs` (default; a no-op if bytes already match) or
  compare only and exit nonzero on drift (`--check`).
- `crates/ludion-shader-bindgen/tests/real_shaders.rs` — reflects the actual repo shaders via
  `load_units` (not fixtures), asserts on real names/sizes, and checks the lists against the shader
  directories, so a new `.wgsl` cannot be left out; the renderer's `shader_ids_cover_the_bindgen_list`
  does the same against `ShaderId::all()`.
- `crates/ludion-renderer/src/shader_gen.rs` — the generated output (checked-in, not this crate's
  source tree, but the thing the crate exists to produce). Owned by the drift gate, never
  hand-edited (see `ludion devdocs renderer-shaders`).

## Rules
- **The reflection model is wgpu-free** (decision 6): `model.rs` names no wgpu type, only
  `emit.rs` does. This is the ash-migration seam the architecture doc keeps open — a backend
  switch rewrites the emitter, not the reflector.
- **Reflection input is compose-then-reflect, not raw files** (`ShaderUnit`): the reflector must
  see exactly the string `create_shader_module` sees. The decision-16 snippet dedup uses this seam —
  the shared `Camera` and clustered-light declarations (`LightHeader` + the three storage arrays) and the fullscreen VS live in
  `crates/ludion-renderer/src/shaders/snippets/*.wgsl`, and each shader that uses them is composed
  through `with_preamble` (snippet text, then the body, joined by `\n`). The mapping is [`COMPOSITIONS`] (Layout).
- **Same-named struct in two shaders must have an identical layout — hard error** (decision 4).
  `reflect` unifies structs by name across every unit and refuses to emit on disagreement
  (`ReflectError::StructConflict`, naming both files and the differing field/offset). This keeps
  `Camera`/`Light`/`Lights` byte-identical between `mesh.wgsl` and `sprite.wgsl`.
- **Bind-group unification key is the WGSL global resource variable name, not `(group, binding)`**
  (decision 13). WGSL bind groups are anonymous, so two units declaring a resource
  of the same name (`camera`, `lights`, `atlas_tex`, …) must agree on group index, binding index,
  and kind — a hard `ReflectError::BindingConflict` on disagreement — while **visibility merges**
  across units (mesh's fragment stage reads `camera.cam_pos`, so the shared `camera` BGL ends up
  `Vertex | Fragment`). A *different*-named resource legitimately reusing the same `(group,
  binding)` slot in another pipeline (`lights` and `atlas_tex` both at `@group(1) @binding(0)`) is
  **not** a conflict — the key is the name.
- **BGL identity is the set of member resource-variable names sharing a `@group`**. Structurally-identical texture+sampler pairs with different names stay separate BGLs
  (sprite/atlas/rect each get their own); same-name-different-shape is the same hard error as for
  structs. Generated function names join the member names in binding order
  (`create_src_tex_src_sampler_bgl`).
- **Only buffer-backed structs unify.** Vertex I/O structs (`VsIn`/`VsOut`) differ per shader by
  design and are excluded from cross-unit unification.
- **A runtime-sized array is bound bare.** Declare `var<storage, read> x: array<T>` (or
  `read_write`; `read_only` follows the WGSL) — wrapping it in a struct with a runtime tail is a hard
  error (`ReflectError::RuntimeSizedMember`: no `#[repr(C)]` POD can mirror one). Storage textures
  reflect as `StorageTexture { dim, format, access }`, arrayed 2D/cube dims as `D2Array`/`CubeArray`.
  A struct gap becomes an explicit `_padN: [u8; n]` field (so the POD stays `Pod`-safe). **A bind
  group whose every member is compute-only gets binding consts, PODs and `*_WORKGROUP_SIZE` consts
  but no `create_*_bgl`**: compute pipelines keep `layout: None`, whose per-entry-point subsets and
  use-derived filterability one unified BGL cannot express. Two properties come from naga rather than from this crate. **Layout is address-space-independent** — WGSL has one
  layout algorithm and the uniform space only adds validation constraints — so the std140/std430
  split does not exist here and a struct shared between a uniform and a storage binding unifies to
  one emitted POD. And **naga sizes a runtime array as exactly one element**, so the emitted
  `min_binding_size` is one element's stride out of the existing `min_size` plumbing, by naga's own
  contract and not by a special case.
- **Canonical, deterministic output** (decision 7): file order = sorted shader paths, declaration
  order within a file, rustfmt-clean as emitted, a `// GENERATED by ludion-shader-bindgen — edit
  the WGSL, then regenerate` header naming the sorted source list. Byte-stable output is what
  makes the drift gate an exact comparison, not a fuzzy one.
- **Consumers of `shader_gen.rs` carry no dependency on this crate** — only the drift gate (`cargo
  run -p ludion-shader-bindgen [-- --check]`) invokes it. `ludion-renderer` depends on `bytemuck`
  for the generated `Pod`/`Zeroable` structs, not on `ludion-shader-bindgen` itself.
- `publish = false`, excluded from the packaging gate; not a `ludion` CLI subcommand — games have no custom-shader surface (decision 1). **Promotion trigger:** if games author their own shaders, this becomes a subcommand and its errors grow `L####` codes (decision 8).

## Verify
```
cargo test -p ludion-shader-bindgen              # unit tests (layout corners, unification errors)
cargo run -p ludion-shader-bindgen               # regenerate shader_gen.rs (no-op if already fresh)
cargo run -p ludion-shader-bindgen -- --check    # compare-only; nonzero exit on drift (CI's form)
node scripts/check.mjs --rust-only               # includes the shader-drift step in the Rust lane
node scripts/check.mjs --fix                     # regenerates before the Rust lane runs
cargo test -p ludion-renderer                    # exercises the generated structs/BGLs/asserts
```

## Gotchas
- **Never hand-edit `crates/ludion-renderer/src/shader_gen.rs`.** It is generated committed text (a shader edit gives a reviewable diff) — a hand edit silently reverts at the next regenerate, or disagrees with the WGSL and fails `--check`. Edit the `.wgsl`,
  then `node scripts/check.mjs --fix` or `cargo run -p ludion-shader-bindgen`.
- **A `StructConflict`/`BindingConflict`/`ConstConflict` names both units and the exact
  disagreement** (e.g. "struct `Lights` is defined in `mesh.wgsl` and `sprite.wgsl` with different
  layouts: field `ambient` at offset 16 vs 32") — the fix is almost always to make the two WGSL
  declarations agree, not to change the generator.
- **Reflecting a shader that composes snippets (decision 16) requires `ShaderUnit::with_preamble`,
  not `ShaderUnit::new`.** Reflecting just the body half of a composed shader reports a missing
  const/type the real pipeline never trips on (e.g. `mesh.wgsl`'s body alone has no `Camera` or
  `Lights` — those come from `snippets/camera.wgsl`/`snippets/lights.wgsl`). The bin and the
  `load_units` (bin and `tests/real_shaders.rs`) both compose via `snippets_for`; do the same in any new reflection
  entry point.
- **naga moves in lockstep with wgpu** (same repo, same release train) — a wgpu major bump is a
  naga major bump too, and the upgrade PR regenerates + re-runs the drift gate in the same commit;
  a silent mismatch is exactly what `--check` exists to catch loudly instead.
- **Sampler filterability and dynamic offsets are not derivable from WGSL text.** The emitter
  defaults to filterable-float textures / filtering samplers (matches every binding in the repo
  today); a real non-filterable case needs a per-binding override added to the generator's input,
  not a preemptive one.
