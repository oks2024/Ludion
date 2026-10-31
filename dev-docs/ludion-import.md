---
description: The deterministic glTF-to-canonical-text compiler behind `ludion import` — mapping, L1202, gotchas
---
# ludion-import

**Scope:** `ludion_import::import` — a pure, deterministic compiler from a glTF `.gltf`/`.glb`
document into an in-memory [`Import`] plan (a `PrefabDocument` + `meshes/*.mesh.yaml` +
`clips/*.anim.yaml` + re-encoded `textures/*.png`) that `ludion-cli`'s `ludion import` then emits,
validates and writes. The mapping table, the determinism obligations and the footguns are here; the
canonical text formats are `ludion docs scene-format` and `ludion devdocs scene-kind-documents`,
and the CLI wrapper's flags and exit behavior are `crates/ludion-cli/src/import.rs`. Read this
before touching the glTF→component mapping, the `L1202` unsupported-feature surface, or adding a field to `Mesh`.

## Layout
- `crates/ludion-import/src/lib.rs` — the crate root: the exhaustive glTF→Ludion mapping table (in
  the module docs), `import()`, `Import`/`MeshArtifact`/`TextureArtifact`, part/mesh/texture
  building, and the emissive mapping (proportional scalar, or a tint baked into an emissive map).
- `crates/ludion-import/src/convert.rs` — pure glTF-value → `serde_json::Value` conversions:
  `transform_value`, `mesh_value` (the `Mesh` component — see Gotchas),
  `point_light_value`, `parent_value`, `mesh_document`, and the quaternion→Euler-XYZ-degrees
  conversion routed through `ludion_core::math` for cross-platform bit-exactness.
- `crates/ludion-import/src/diagnostics.rs` — the codes: `L1201` (unreadable/invalid glTF),
  `L1202` (a feature with no Ludion mapping, always naming the offender), `L1203` (output write
  failure), `L1204` (an external source refused), `L1205` (a warning: a skinned vertex with no
  weight, bound to the root joint), `L1206` (a clip over its error bound),
  `L1207` (invalid animation `extras`).
- `crates/ludion-import/src/skin.rs` — the skin mapping: `bind_skin` (joints, parent-first order,
  rest, inverse binds) and `map_weights` (a primitive's `JOINTS_0`/`WEIGHTS_0`).
- `crates/ludion-import/src/animation.rs` — the animation mapping (`ludion devdocs import-animation`): channel reading,
  key reduction, `measure`, the bound check, `ClipReport`.
- `crates/ludion-import/src/sources.rs` — buffer/image resolution (`.glb` blob, `data:` URIs,
  external files at or below the `.gltf`'s directory).
- `crates/ludion-import/src/tangents.rs` — the deterministic `f64` tangent generator (and the
  faceted normals a generated frame needs), with a pinned conformance hash and a Blender-exported
  oracle (`tests/fixtures/tangent-quad.glb`, built by `tangent-quad.py` beside it: three UV layouts,
  `export_tangents=True`).
- `crates/ludion-import/tests/animation.rs` — synthetic-glTF clip tests (spin, breathing, wrap-around, bound refusal, STEP/CUBICSPLINE, loop, `L1202`/`L1207`, determinism).
- `crates/ludion-import/tests/import.rs` — plan-level tests: the mapping, the exhaustive `L1202` surface, compiler determinism — no component registry.
- `crates/ludion-cli/src/import.rs` — the CLI wrapper: reads bytes, calls `import`, sets the prefab's
  `generated_from`, emits `model.prefab.yaml` through the canonical writer with the built-in
  registry, re-validates (`check-scene` + `check_mesh`), then writes the tree. Owns the registry dependency `ludion-import` deliberately
  does not have.
- `crates/ludion-cli/tests/import.rs` — the full pipeline: default-mode `L1202` failure writes
  nothing, `--allow-lossy` reproduces a committed fixture tree byte-for-byte, the imported tree
  passes `check-scene`, and the **round-trip gate** (the committed `expected/scene.yaml` is a hand-authored wrapper: an instance entry of `model.prefab.yaml` plus the camera a prefab cannot carry; `ludion docs prefabs`): import → check → golden render → re-export →
  re-import, asserting byte identity end to end.
- `examples/import/scene.gltf` — the lossy-path fixture (a `BLEND` material, a base-color texture, several meshes) both test suites import; `examples/import-cutout/scene.gltf` is the strict `MASK`/`doubleSided` one (Blender 5.2 script, imported strict, tree byte-gated); `examples/import-textured/scene.gltf` is the clean-path one (five maps on two cubes, the normal map tilted in both axes, one cube with an exporter-convention authored `TANGENT`).

## Rules
- **Same input, byte-identical plan — always.** Every walk is glTF **document order** (nodes,
  meshes, primitives, images); name dedup uses stable index suffixes (`dedup`, `BTreeSet`/
  `BTreeMap` throughout, never a hashed-iteration-order map; mesh and clip file stems dedup
  case-insensitively, `dedup_file_stem`, since `Wall` and `wall` are one file on Windows/macOS); textures go through
  `ludion_assets::encode_png`, a pure function of decoded pixels. The output must diff cleanly and re-import identically.
- **Unsupported features are structured `L1202` errors, never silent drops.** Every glTF construct
  with no Ludion mapping — a texture on a second UV set or with per-axis wrap modes that disagree (a
  reference holds one mode for both), a textured material on a primitive without `TEXCOORD_0`, a
  `BLEND` `alphaMode` (`MASK` and `doubleSided` map), a camera (a prefab carries none), a directional light, an
  animation channel of morph weights or on a node with no part (or one mixing skin joints and parts), a morph target, a sparse or
  non-triangle primitive, a *referenced* JPEG image, an unrecognized required extension — is named (node/mesh/primitive/material index and, if present,
  its glTF name) in an `L1202`. By default any `L1202` **aborts the import: nothing is written.** The
  five metallic-roughness maps themselves are supported (`Plan/textured-pbr.md`) and no longer
  appear here; an image no supported slot references is not a diagnostic, it is simply not written.
- **`--allow-lossy` downgrades every `L1202` to a warning** and compiles the supported subset; the
  dropped features are still named, just non-fatal (`Import::warnings`, which also carries the
  `L1205` notes in both modes).
- **The f32 round-trip rule (`convert.rs` module docs).** Every float landing in a component or
  mesh value is stored as `f64::from(some_f32)` — the exact widening, never a re-rounded decimal.
  The writer prints an `f32`-schema field as its shortest f32 decimal, so a component float in an
  import is short *and* a `scene.load` → `scene.save` fixed point. Mesh and clip documents are
  `f64`-typed and keep the widened digits.
- **Quaternion→Euler conversion is `ludion_core::math`'s** (f32 for node transforms, `math::f64` for clips), never glam's `to_euler`/`normalize` — glam's backends round the last bit differently per platform.
- **`origin:` is never written here.** `MeshDocument.origin` stays `None` through this crate;
  `ludion-gen`'s `accept` step is the sole writer, splicing `Origin` into the plan before
  `Import::write` runs — calling `ludion_import::import` in-process, the crate's other caller beside
  the `ludion import` CLI reading a `.glb` from disk (`ludion devdocs ludion-gen`).

## Dataflow
`import(bytes, external, allow_lossy)` parses the glTF (`L1201` on failure), resolves buffers
(`.glb` blob / `data:` / external files per `external`), then walks the document once:
document-level features (unsupported required extensions, and a `L1239` warning for each optional
`extensionsUsed` entry with no mapping) are recorded first, then
`build_entities` walks nodes in document order, emitting one part per node (`Transform` always;
`Mesh`/`PointLight`/`Parent` as present; a `/` in a node name becomes `_`) at positions `0..N`, followed by one extra child
entity per primitive of any multi-primitive mesh at positions `N..` (parented to its node — a
multi-primitive mesh has no single-entity Ludion representation, so it fans out). A skin's joint
nodes are folded (below), so the positions count emitted nodes only.

`external` is an `ExternalFiles`: `Beside(dir)` reads external URIs only at or below `dir` (the
input file's directory), `Refused` reads none. A URI must pass `ludion_core::paths::relative_refusal`
as written and percent-decoded, then name a regular file of at most 512 MiB; else `L1204`. A read
that merely fails stays `L1201`.

Accessors and mesh data are checked before any reader (`accessors.rs`, `ludion_scene::validate_mesh`)
and every float handed to a component builder is checked finite; malformed data is `L1201` naming the element.

Material, texture and geometry mapping (`mesh_reference`, the emissive mapping and tint bake, content-hashed textures, tangents): `ludion devdocs import-materials`.


`Import::write` then lays out `model.prefab.yaml` (caller-supplied canonical text — this crate never emits
it directly, since that needs the component registry, which lives above it), `meshes/*.mesh.yaml`,
and `textures/*.png` under `--out`. The CLI (`crates/ludion-cli/src/import.rs`) is the one that
calls `ludion_scene::emit_prefab`/`check_prefab`/`check_mesh` before writing anything, so a plan that wouldn't
pass `check-scene` is caught as a bug here rather than silently written to disk.

Skins fold joint nodes into the mesh document's `joints:` (`skin.rs`): `ludion devdocs import-skin`. Each glTF animation becomes one `clips/<name>.anim.yaml` (`animation.rs`): `ludion devdocs import-animation`; codes `L1205`–`L1208` live there.


## Verify
```
cargo test -p ludion-import                    # plan-level: mapping, L1202 surface, clips, determinism
cargo test -p ludion-cli --test import         # full pipeline incl. the round-trip gate
cargo run -p ludion-cli -- import examples/import/scene.gltf --out target/imported --allow-lossy
cargo run -p ludion-cli -- explain L1202      # and L1201, L1203, L1206
```

## Gotchas
- **Absent factors take the glTF spec default, not Ludion's default.** glTF's
  `pbrMetallicRoughness` defaults `metallicFactor` to **1.0** and `roughnessFactor` to **1.0** when
  the material omits them (`gltf::Material::pbr_metallic_roughness()` returns those spec defaults,
  not `0.0`). A factor-less material — common in simple glTF exports — therefore
  imports as **fully metallic**, which is rarely what the source content intended. Check exported factors before assuming a "plain" import is dielectric.
- **`mesh_value` (`convert.rs`) is hand-built JSON, not derived from `Mesh`.** It constructs the
  `Mesh` component's wire shape (`shape`/`color`/`roughness`/`metallic`/`emissive`) field by field
  with `serde_json::json!`. **A new `Mesh` component field must be added here too**, or the
  imported scene silently omits it — and because the CLI's writer always serializes every field the
  engine's `Mesh` struct has, the imported `model.prefab.yaml` will fail to be a canonical fixed point
  (`scene.load` → `scene.save` reintroduces the missing field) — which the round-trip gate's
  byte-identity check is there to catch. If it fails after a `Mesh` field is added, `mesh_value` is
  the first place to look.
- **`--allow-lossy` is per-feature, not per-material or per-primitive.** A textured material still
  imports its flat color/roughness/metallic; only the texture reference itself is dropped. A
  unmappable tinted `emissiveFactor` (a zero base channel, no `TEXCOORD_0`) drops only the emissive, not the mesh. Read an `L1202`'s
  message to know exactly what was kept vs. dropped.
- **With a skin, entity positions are not node indices.** Joint nodes are folded out of the entity
  list, so every `Parent` goes through `entity_of` (node index → entity position); don't index the
  prefab's parts by glTF node index in a skinned document.
- **A multi-primitive mesh silently changes entity count.** One glTF node with an N-primitive mesh
  becomes 1 (the node's own entity, no `Mesh`) + N child entities — "one node in, one entity out" breaks counts on `import.prefab.entities.len()`.
- **`check-scene` does not read a mesh document's textures.** It validates the document itself
  (structure, `L1111`'s texture rules: `uvs` present, `tangents` present for a normal map, a
  non-empty `path`) but never opens the referenced PNGs — that happens only at load, in
  `ludion-engine`'s `mesh_asset::load` (`L1101`/`L1102`). A path that validates but points nowhere
  passes `check-scene` and fails only when the scene is actually rendered.
