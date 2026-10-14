---
description: ludion-sculpt's source fields — edt.rs, sources.rs, soup.rs behind shape: voxel/mesh
---

# ludion-sculpt: source fields

**Scope:** `crates/ludion-sculpt/src/{edt,sources,soup}.rs` — the field a `shape: voxel` or
`shape: mesh` sculpt part reads from another document. The rest of the mesher is `ludion devdocs
ludion-sculpt`; the schema these two shapes carry is `ludion docs sculpt-format-sources`.

## Layout

- `src/edt.rs` (`pub(crate)`) — `signed_distance`/`SignedDistanceGrid`, an exact signed Euclidean
  distance transform of a voxel occupancy grid (§Rules, §Gotchas). Called from `sources.rs` only.
- `src/sources.rs` (`pub` for `SculptSources`) — one field per distinct source path, `Arc`-shared by
  every part naming it: `insert_voxel`/`contains_voxel` the `edt::signed_distance` transform for a
  `shape: voxel` path, `insert_mesh`/`contains_mesh` a `soup.rs` bake (below) for a `shape: mesh` one,
  each paid for once however many parts name it. `PAD` (`1`) adds one empty voxel of margin around a
  voxel model's occupancy first, so a model filled to its own grid boundary still meshes with its
  faces exactly on the box, not half a voxel proud. The crate still reads no files: `mesh_with(doc,
  &sources)` is `mesh` with the caller's sources already resolved, and `mesh(doc)` is
  `mesh_with(doc, &SculptSources::new())` — an empty set, so a `shape: voxel`/`shape: mesh` part
  refuses with `L1157` at `/parts/<i>/path` unless the caller filled it first.
  `ludion_engine::read_sculpt_sources` is the one file reader, shared by the sculpt loader,
  `check-scene`, and the model sheet. The `Arc` is why `field::Primitive` derives `Clone`, not
  `Copy`, once its `Voxel`/`Mesh` variants hold one.
- `src/soup.rs` (`pub(crate)` `TriangleSoup`) — a mesh source's field: one BVH (median split, stable
  order) gives closest-triangle distance for magnitude (ties to the lower index) and hierarchical
  generalised winding number for sign (far nodes answer from an area-weighted dipole, near ones sum
  exact solid angles; `w > 0.5` is inside), so a non-watertight generated mesh keeps a usable sign.
  `sources.rs` bakes it once per path onto a grid of the mesh's own box (64 cells on the longest axis
  plus padding, `par` parallel, thread-count-invariant) into the same `SignedDistanceGrid` the voxel
  EDT uses — detail finer than that grid never reaches the field.

## Rules

- **`edt::signed_distance` is the exact transform, not the chamfer approximation** — every distance
  is an integer squared value (Meijster–Roerdink–Hesselink, three separable 1D passes) until the one
  final `sqrt`, so it needs no cross-arch tolerance a floating accumulation would. The zero level
  sits on the voxel faces: both the inside and outside branches subtract a half-voxel shift before
  negating the inside one. `SignedDistanceGrid::sample` beyond the grid's box returns a valid lower
  bound — the clamped interior sample plus the distance to the box — not an unbounded read, so a
  caller stepping toward the surface from outside the grid never overshoots it.

## Gotchas

- **`edt::signed_distance`'s conformance proof is a pinned hash, the idol test's own pattern one
  level down.** `a_pseudo_random_occupancy_hashes_to_a_pinned_value` runs the transform over a fixed
  pseudo-random occupancy and asserts the XXH3 hash of its output bytes against a landed constant —
  the idol test's cross-arch discipline, proven before `sources.rs` folds a transform into a field.
- **`soup.rs`'s conformance proof is `tests/mesh_part.rs`'s pinned hash** —
  `the_pebble_meshes_to_the_conformance_hash` hashes a pebble sculpt's meshed bytes against a landed
  constant; both pinned platforms must agree, the idol test's rule again.
- **The winding number is the mesh field's cost.** A pebble's 4,986-triangle mesh bakes in ≈126 ms at
  8 workers (≈856 ms at one) on the reference RTX 3090 box; its hierarchical error against the direct
  solid-angle sum is ≈3.4e-3 worst case over 400 points (the test gates at 1e-2; classification always
  agrees).

## Verify

```
cargo test -p ludion-sculpt edt::             # the distance transform: exact-vs-brute-force and its pinned hash
cargo test -p ludion-sculpt --test mesh_part  # the mesh-part field: conformance hash, thread-count identity
```
