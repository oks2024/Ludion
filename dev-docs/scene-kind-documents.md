---
description: The kind:-dispatched document families in ludion-scene: hub, mesh, data-docs, spokes
---

# ludion-scene: the kind: document families

**Scope:** The sibling document families `peek_kind` dispatches to. Each mirrors the scene pipeline
(structure pass → semantic pass → canonical emit) and takes **no** registry, because these documents
are self-describing rather than component-bearing. This hub owns `kind: mesh` and the generic
data-doc/profile machinery; the rest are spokes. The scene pipeline itself, and the rules every
family shares, are `ludion devdocs ludion-scene`.

## Spokes

- `ludion devdocs scene-kind-voxel` — `kind: voxel` and its write side (`voxel_edit.rs`, `voxel_gen.rs`).
- `ludion devdocs scene-kind-sculpt-map` — `kind: sculpt`, `kind: map`/`map-kit`.
- `ludion devdocs scene-kind-sprite-font` — `kind: font`/`font-source`, `kind: sprite`/`palette`/`atlas-source`/`atlas`.
- `ludion devdocs scene-kind-generation` — `kind: generation`/`origin` and `kind: mesh-edit`.
- `ludion devdocs assembly-animation` — `assembly`/`part`/`animation` and their evaluator.

Formats: `ludion docs voxel-format`, `ludion docs voxel-ops`, `ludion docs sculpt-format`,
`ludion docs map-authoring`, `ludion docs generation`, `ludion docs mesh-editing`,
`ludion docs data-docs`, `ludion docs player-profiles`.

## Mesh documents (`kind: mesh`)

`mesh.rs` owns `meshes/*.mesh.yaml`: flat `positions`, optional `normals` (omitted computes faceted
normals at load, a pure function of `positions` + `indices`), optional `uvs`, optional `tangents` (4
floats/vertex: `xyz` + handedness `w` ∈ {−1, 1}), optional per-vertex `emissive`/`metallic`/
`roughness`, up to five optional texture references in canonical order —
`base_color_texture`/`metallic_roughness_texture`/`normal_texture`/`occlusion_texture`/
`emissive_texture`, each `{ path, wrap?, scale?/strength? }` with `path` relative to the document's
own directory — then optional `alpha_mode` (`opaque`/`mask`), `alpha_cutoff` and `double_sided`, each
omitted at its default (`opaque`, `0.5`, `false`) — optional `rim` (`{ color, strength, power }`, shared with voxel and sculpt via `ludion-scene`'s `rim` module; `L1111` on a bad value) — optional skin keys `joints` (each `name`, `parent?`, `rest: { t, r, s }`,
`inverse_bind` — 16 floats, column-major; parent-first order, at most 256), `weights` and
`bone_indices` (4 per vertex each), all three or none — optional surface tags `surfaces` (local names, block list, at most 256) and `triangle_surfaces` (run-length `[count, local index]` pairs in triangle order; both or neither, counts summing to the triangle count, `L1696`), which are the `MeshDocument.surfaces` field (`MeshSurfaces`; `check_mesh`, `parse_mesh` and `emit_mesh` carry them, so a rewrite cannot drop the tags) — and a required `indices` triangle list. It mirrors the scene pipeline (`parse` →
`validate` → `check` → canonical `emit`) and takes **no** registry, because a mesh document is
self-describing rather than component-bearing.

The split: `L1110` is *structure* (missing or wrong-typed keys, an `alpha_mode` other than `opaque`/`mask`, an unrecognized `kind`, a malformed
list, an unknown key with a nearest match), `L1111` is *data consistency* (every float list finite;
`positions` a multiple of 3; `normals` and each authored material list matching the vertex count
one-for-one; `uvs` a multiple of 2 matching the vertex count; `tangents` exactly 4 floats per vertex
with every 4th (`w`) exactly `-1` or `1`; `alpha_cutoff` finite, in `[0, 1]` and only with `alpha_mode: mask`; `roughness`/`metallic` in `[0, 1]`; `emissive >= 0`;
`indices` a multiple of 3 with every index in range; a texture reference needs `uvs`, `normal_texture`
additionally needs `tangents`, and its `path` is non-empty). A `None` material or texture list means
"use the default" and is never checked. Under the skin keys the split moves: `L1190` is structure
(a partial set of the three keys, a malformed joint or wrong list length) and `L1191` is skin data
(1–256 unique non-empty joint names, each `parent` earlier, finite rest/bind values with a unit `r`,
4 non-negative weights summing to 1 and 4 in-range bone indices per vertex; weight order is not
checked). `L1112` is the non-fatal vertex-count budget above
`MESH_VERTEX_BUDGET`. The texture format, what each map multiplies, and the render-side handling are
`ludion docs materials` and `ludion devdocs renderer-shading`; the glTF mapping and PNG dedup are
`ludion devdocs ludion-import`.

`kind: mesh-edit` is this family's sibling for texture edits — `ludion devdocs scene-kind-generation`.

## Game data-docs (`check_doc` / `emit_doc`)

`doc.rs` is the **generic, type-agnostic** doc machinery a game reaches through `ludion-engine`'s
`doc_kind::<T>`. Where `mesh.rs`/`voxel.rs` hand-roll one fixed type each, these two functions
validate and emit *any* type by taking its schema as a `Value` argument — so the crate stays true to
"never names a concrete component." `parse_doc::<T>(source, kind)` adds a typed entry over them (generic
`T`, still no concrete type named): `schema_for!(T)` → `check_doc` → `deserialize_doc`. The engine's
install and reload call the same `deserialize_doc`, so a test parse and the boot parse give the same
value and the same `L1120` text.

- `check_doc(source, kind, schema)` — strict-YAML → `Value`, then **envelope** validation (`format ==
  1`, `kind` matches the argument, no stray top-level envelope key → `L1119`, with a nearest match
  over `format`/`kind`), then strips the envelope and runs the shared `Checker` on the remaining field
  map (`L1120`, JSON Pointer + `suggest::nearest`). Returns the validated field map, which
  `deserialize_doc::<T>` turns into `T`.
- `emit_doc(kind, value, schema)` — writes the envelope, then the field map in schema-declaration
  order via the same generic value emitter scenes use. Byte-idempotent, property-tested in
  `tests/doc.rs` over a hand-built schema.

Reusing the `Checker`/emit core widened their crate-internal visibility with **no** behavior change to
scene validate/emit — every scene golden stayed byte-identical.

`open_profile` is the **lenient** sibling, for a player save. It parses, requires `format` present and
`kind == "profile"`, strips them, and returns the raw fields — and **deliberately does not run the
`Checker`**: no unknown-field or type rejection. That is the whole point of the split. An authored
data-doc is content an agent must fix, so `check_doc` hard-rejects drift; a profile is a *player's*
save, so drift must migrate, not fail. The typed lenient load (fill/drop/backup) lives engine-side in
`player_profile.rs`, which owns `P`; `open_profile` only validates and strips the envelope, and bad,
empty or non-mapping input `Err`s into the engine's backup-and-default path. The canonical write
reuses `emit_doc("profile", …)` verbatim.

## Verify

```
cargo test -p ludion-scene                                        # includes mesh_roundtrip / voxel_roundtrip / sculpt round-trip / sprite_roundtrip / atlas_roundtrip / generation emit fixed point / doc byte-idempotence
cargo run -p ludion-cli -- check-scene examples/gallery/voxels/lantern.voxel.yaml
cargo run -p ludion-cli -- check-scene crates/ludion-sculpt/tests/fixtures/idol.sculpt.yaml --bounds
# voxel verb family and voxel drift: ludion devdocs scene-kind-voxel
cargo run -p ludion-cli -- map-check crates/ludion-cli/tests/fixtures/district.map.yaml
node scripts/map-drift.mjs                                        # committed generated scenes still match their maps
```
