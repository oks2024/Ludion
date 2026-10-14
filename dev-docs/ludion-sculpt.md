---
description: The sculpt mesher crate — field, grid, surface nets, decimation, determinism, cost model
---

# ludion-sculpt

**Scope:** `crates/ludion-sculpt`, the pure crate that turns a validated `SculptDocument`
(`ludion-scene`) into triangle geometry: `mesh(doc) -> SculptMesh`. It depends on `ludion-core` +
`ludion-scene` only and knows nothing of the renderer — `ludion-engine`'s `sculpt_mesh.rs` is the
loader wrapping `check_sculpt` + `mesh` into a `MeshData`, keyed by content hash like
`voxel_mesh.rs`. Schema: `ludion docs sculpt-format`; Rust types: `ludion devdocs
scene-kind-documents`; the `shape: voxel`/`shape: mesh` source field is its own spoke, `ludion devdocs
sculpt-sources`.

## Layout

- `src/lib.rs` — the pipeline entry: `mesh(doc) -> Result<SculptMesh, Diagnostic>`,
  `unresolved_parts` (`L1158`), `hider` (the part `L1158` names as the culprit),
  `SculptMesh`/`SculptStats`/`PartStats`/`BoneStats` (what `check-scene`'s summary and `--bounds`
  print).
- `src/field.rs` — signed distances for the six primitives and the polynomial smooth min/max
  (`smin`/`smax`) that folds them; returns `(value, weight)`, the weight feeding `material.rs` and,
  later, skinning.
- `src/grid.rs` — compiles a document's parts (mirror/repeat copies folded by exact `min` first) into
  one `SculptField` and samples its corner lattice; owns `MAX_GRID_CORNERS` (16,000,000), the cap
  `mesh` refuses past as `L1157` at `/resolution` rather than allocate.
- `src/surface_nets.rs` — naive surface nets: one vertex per crossed cell, one quad (split into two
  triangles) per crossed lattice edge, in fixed `z, y, x` discovery order.
- `src/normals.rs` — the field gradient as the smooth normal everywhere, plus the crease split: a
  triangle is sharp when every vertex's dominant material weight is a `sharp` part, and sharp
  triangles around a vertex whose face normals stay within `CREASE_ANGLE_DEGREES` (60) share a
  duplicated vertex — the only place the vertex count grows.
- `src/decimate.rs` — Garland–Heckbert quadric-error edge collapse to the document's `triangles`
  target, with a link-condition + normal-flip legality check per collapse.
- `src/material.rs` — per-part weights folded the same way the field is, and the palette-channel
  blend (`color`, `roughness`/`metallic`/`emissive`) at each vertex.
- `src/connectivity.rs` (`pub`) — connected-component count over raw `positions`/`indices`, treating
  bit-identical positions (a crease's duplicated vertex) as connected; raises `L1159` in the `L1129`
  shape. Document-agnostic.
- `src/edt.rs`, `src/sources.rs`, `src/soup.rs` — the field a `shape: voxel`/`shape: mesh` part reads
  from another document (BVH, winding number, exact EDT, cost): `ludion devdocs sculpt-sources`.

## Dataflow

`mesh_with(doc, sources)`: `SculptField::new(doc, sources)` compiles every part — its `Modifiers`
when it carries `deform`/`noise`, its `VoxelField`/`MeshField` from `sources` when it is `shape:
voxel`/`shape: mesh` (§field, §Rules below) → `Grid::sample` builds the corner lattice, `None` when no cell changes sign (an empty
document meshes to an empty mesh) → `surface_nets::extract` gives raw positions/indices → `material::weights_at` samples per-part weights at each raw
vertex (before decimation, so weights track position exactly) → `decimate` collapses to
`doc.triangles`, carrying the weight blob as a flat per-vertex attribute → `normals::
gradient_normals` recomputes smooth normals from the field at the *decimated* positions (so they stay
correct after collapse), then `crease_split` duplicates vertices across `sharp` seams → a
bottom-center translation is computed and applied to positions and the stats' `extent` → `material::
blend` turns each vertex's weight vector into `colors`/`materials` → `connectivity::connectivity`
counts components for `SculptStats` (and, separately, `check-scene` calls it again for the `L1159`
warning) → `SculptMesh` is returned with `part_of` (nearest-part attribution, ties to the lower part
index) and `SculptStats`.

`ludion-engine`'s `sculpt_mesh.rs::load` is the only consumer turning a `SculptMesh` into a render
`MeshData` (`authored_material: true`, `hash = content_hash(source bytes)`) — this crate never
imports a renderer type, the boundary rule `ludion-scene` holds for YAML.

## Rules

- **Determinism, the sim-side denylist** (`crates/ludion-sculpt/clippy.toml`, the `ludion-core`/
  `ludion-scene` list): no `std::HashMap`/`HashSet`, no wall clock, no `std::thread::spawn`/`scope`.
  Arithmetic is `f32` except `decimate.rs`'s quadric accumulation, collapse-position solve and fold
  tests, in `f64` before truncating back (documented there, not a crate-wide exception). Every
  transcendental goes through `ludion_core::math`.
- **Every loop order and tie-break is fixed and explicit**, because the output must hash identically
  on Windows x86-64 and macOS arm64: `surface_nets` walks `z, y, x`; a crossed cell's quad splits on
  its shorter diagonal, ties to `(0, 2)`; `decimate`'s heap orders on `(error.to_bits(), edge index,
  version)` — error bit patterns sort like the values since errors are non-negative, and edge indices
  never tie; `nearest_part` attribution ties to the lower part index.
- **The idol conformance test is the proof, not the intent.** `crates/ludion-sculpt/tests/idol.rs`
  hashes `positions/normals/colors/materials/indices/weights` (XXH3 via `ludion_assets::
  content_hash`) against a constant landed from the authoring machine. A mismatch on the second
  pinned box is a mesher determinism bug to fix, **never** a per-OS constant — the rule every
  cross-arch `WorldHash` gate holds.
- **`MAX_GRID_CORNERS` is a hard refusal, not a silent clamp.** A document whose `resolution` would
  sample more than 16,000,000 lattice corners gets `L1157` at `/resolution` from `mesh`, before any
  allocation.
- **Every deformer acts about the part's own centre, the midpoint of the primitive's undeformed box**
  (`field.rs::Modifiers`). A `Capsule`'s `from`/`to` are document coordinates, so its local frame
  *is* the model frame: measured from the local origin a horn at `y ≈ 1.3` with `degrees: 140`
  rotated ~180° and landed on the floor. A primitive already centred on its local origin (sphere,
  box, ellipsoid, torus) stores `centre: None` and keeps exactly the arithmetic it had before the
  frame existed, bit for bit; a cone and a voxel part are off-centre in `y` and shift with the rest.
- **A deformed part's distance is sign-exact, not a true distance, and nothing rescales it.**
  `primitive(D(p))` is off by up to `|∂D/∂p|`, which is all surface nets reads: the sign per corner,
  one interpolation along a crossed edge. A per-part Lipschitz divisor used to shrink it — and so
  shrank the part inside the smooth-min fold, where it won the blend share far outside its geometry
  and stole a neighbour's surface and material (a `taper` to `0.25` blanked one into an `L1158`).
  The conservative bound lives in the box instead.
- **The sample point passes through the deformers in a fixed order — `bend`, then `twist`, then
  `taper` — independent of the order the document writes them in**, and a part's local box grows to
  cover the deformed, displaced solid before folding into the model's overall extent
  (`PlacedPrimitive::bounds_of`), travelling back through the inverses in the opposite order: `noise`
  pads the primitive's own box by `amplitude` *first*, then a taper grows it by its scale range, then
  a rotation grows it to the in-plane radius of the plane it turns.
- **The source field (`edt.rs`/`sources.rs`/`soup.rs`) has its own Rules and Gotchas** —
  `ludion devdocs sculpt-sources`.

## Gotchas

- **Surface nets bevels a box edge into faces under the crease angle** before decimation reduces a
  slab to its flat faces — `sharp_parts_split_creases_and_nothing_else_grows` (`lib.rs` tests)
  measures it: a `sharp` slab decimated to 12 triangles has the *same* triangle count as a smooth
  one, because no creases are left to split at that count. Only a triangle budget generous enough to
  keep the flat faces separate lets `sharp` do anything.
- **A hard (`blend: 0`) crease can leave a non-manifold cell** whose lattice edge four triangles
  share rather than the usual two — a measured limit of naive surface nets at an exact `min`/`max`
  boundary, not (yet) asserted against everywhere.
- **`unresolved_parts` attributes on the *decimated* surface.** A part driven to zero surface vertices
  by a low `triangles` target reports `L1158` exactly like a genuinely hidden or too-thin part —
  raising `triangles` before reading the warning as "this part is truly invisible" is the first
  thing to try. The culprit `L1158` names is the part with the largest total
  fold weight over the lattice corners where the victim's own distance is negative, ties to the lower
  index. `unresolved_parts(doc, sources, mesh)` computes it (`hider`, over a sample-free `Grid::frame`),
  one extra lattice pass per surfaceless part, so it is a warning's cost; the mesher never runs it, so
  an engine load or reload does not pay.
- **The idol meshes in roughly 55–70 ms** (13,436 → 4,000 triangles) on the reference RTX 3090 box —
  `check-scene`'s `meshed in N ms` is the one legitimate wall-clock read here, carrying
  `ludion-cli`'s single documented `#[allow(clippy::disallowed_methods)]` (`check_scene.rs`'s
  `timed`): a CLI report timing the mesher from outside, not a value the mesh depends on. The field
  pass costs `corners × parts` distance evaluations and a deformed part grows the lattice in every
  axis at once; decimation is `O(E log E)`.
- **The source field's own conformance hashes and measured cost (EDT, winding number)** are
  `ludion devdocs sculpt-sources`.

## Verify

```
cargo test -p ludion-sculpt          # unit tests + the idol conformance hash
cargo run -q -p ludion-cli -- check-scene crates/ludion-sculpt/tests/fixtures/idol.sculpt.yaml --bounds
cargo run -q -p ludion-cli -- explain L1158
```
