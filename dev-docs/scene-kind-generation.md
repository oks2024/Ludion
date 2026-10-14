---
description: kind: generation, origin and mesh-edit text layers in ludion-scene
---

# ludion-scene: generation, origin and mesh-edit documents

**Scope:** Changing `generation.rs` or `mesh_edit.rs`. Hub: `ludion devdocs scene-kind-documents`; formats: `ludion docs generation`, `ludion docs mesh-editing`; clients: `ludion devdocs ludion-gen`, `ludion devdocs ludion-mesh-edit`.

## Generation recipes and provenance (`kind: generation` / `kind: origin`)

`generation.rs` owns both halves of the generation loop's text — `generation/recipes/*.gen.yaml`, and
the `Origin` provenance block an accepted asset keeps, which has **two carriers and one writer**: a
mesh document's optional `origin:` key (`emit_origin` at an indent) and a `kind: origin` sidecar
beside a generated image, which has no text place of its own (`emit_origin_document`, the same keys
under an envelope). `L1161` is structure, `L1162` data, the `mesh.rs` split exactly. The format is
`ludion docs generation`; everything past the text — the workflow file, the server, the run, the
accept — is the client's (`L1163`–`L1172`, **none raised here**), `ludion devdocs ludion-gen`.

Five things this module's signatures do not say:

- **A recipe overrides leaf inputs; it never rewires the graph.** `inputs` is node title → input name
  → value, and the only structured value it can spell is a reference into its own `images` map — so
  an edge stays the committed API-format workflow's business, which is what keeps that file the
  single source of the graph.
- **That image reference is block style**, a nested `image:` key and never `{image: subject}`: the
  strict YAML subset rejects a non-empty flow collection, so the flow spelling is not a stylistic
  choice the emitter made but the only legal one.
- **`ScalarValue` keeps `Integer` apart from `Number`** because a node input that declares an integer
  (a width, a step count) rejects the float spelling a shared `f64` would emit — the override would
  be refused by the server, not by us.
- **This module reads no file.** `workflow` and every `images` path are checked for shape only —
  empty is `L1162`, missing is nobody's business here — because resolving them against the recipe's
  own directory is the caller's job, the rule `shape: voxel` already set.
- **`parse_origin` and `validate_origin_at` take their diagnostic code from the caller**, the
  `envelope` pattern: an `origin:` inside a mesh document reports under the mesh's own
  `L1110`/`L1111`, while the traversal producing the diagnostic stays single-sourced here. An
  origin's image references name server-side uploads rather than a declared `images` map, so they are
  the one reference kind `validate_inputs` is told not to resolve.

Canonical emit is minimal in the part-default sense: `name` when absent and `count` at its default
`1` are not written, `seed` always is, and `images` and both levels of `inputs` sort by key — so an
agent or the editor splicing one token through a recipe gets a one-line diff.


## Mesh-edit documents (`kind: mesh-edit`)

`mesh_edit.rs` owns `meshes/*.mesh-edit.yaml`: an ordered, replayable list of edits to a `kind: mesh`
document's texture maps (`ludion docs mesh-editing` is the format). Same shared `yaml`/`scalar`/
`envelope` layers and no registry as `mesh.rs`; `L1173` is *structure*, `L1174` is *data* (finiteness,
ranges, the `out` containment rule). Six top-level keys: `format`, `kind`, `name`, `source`, `out`, `ops`.

- **Pure text: no path is resolved and no file is read here.** `source` (a `kind: mesh` document) and
  `out` (a derived directory, defaulting to the document's own stem via `out_dir`) are both relative to
  the document's own directory. Whether `source` exists, carries `uvs`, or whether a `size` collides
  with a map the source already has is `L1176`, raised by `ludion-mesh-edit`
  (`ludion devdocs ludion-mesh-edit`), as are `L1177` (an op
  painted nothing) and `L1178` (a `key` filter rejected every texel its region reaches) — none of
  those is raised in this crate. `L1175` (a pick resolved to no surface) comes from `ludion mesh pick`,
  `crates/ludion-cli/src/mesh.rs`.
- **`paint` is the only op**, read in file order, each reading the result of the one before. Keys:
  `op`, `map` (`base_color`, `metallic_roughness`, `normal`, `occlusion`, `emissive` — the
  `MeshTextures` field spelling, not the document's `*_texture` keys), `size`, `color`, `strength`,
  `region`. `strength` scales the region's weight, never the colour; `size` is `None` when the source's
  base-colour dimensions apply, which only the consumer can resolve.
- **A region is one shape plus optional `feather` and `key`.** Shapes — `sphere`, `capsule` (a
  polyline `points`, at least two; the old `from`/`to` spelling is `L1173` with the rewrite in its
  message), `box`, `fill` (a seeded surface flood bounded by `max_radius`) — each carry their own key
  set, so a key belonging to another shape is rejected. Weight is 1 inside, falling linearly to 0 over
  `feather` world units outside. `key` (`map`, `color`, `tolerance`, `color_feather`) zeroes texels
  whose colour is farther than `tolerance + color_feather` from `color`; the metric is Euclidean sRGB,
  the same one a `fill` uses against its seed.
- **The source mesh and its committed maps are never rewritten.** The derived tree is a pure function
  of (source bytes, edit bytes, source map bytes) and is write-only: `generated_from:` on the derived
  mesh and `scripts/mesh-edit-drift.mjs` (check.mjs, ci.yml) enforce byte-identity.
- Canonical emit is ryu floats and byte-idempotent, the `mesh.rs` property.

## Verify

```
cargo test -p ludion-scene                    # generation and mesh-edit emit fixed points, byte-idempotence
cargo run -p ludion-cli -- check-scene crates/ludion-cli/tests/fixtures/gen/idol.gen.yaml
```
