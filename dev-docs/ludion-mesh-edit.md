---
description: The mesh-edit crate: regions, UV raster, linear-light paint, the seeded fill, the ray cast, derive
---

# ludion-mesh-edit

**Scope:** the crate that turns a `kind: mesh-edit` document into a derived mesh document plus one
content-hash-named PNG per edited map. The document's own structure/data checking lives in
`ludion-scene` (`ludion devdocs scene-kind-documents`); the camera a pick reuses lives in
`ludion-engine`'s model sheet; the CLI wrapper is `ludion devdocs ludion-cli`. The authoring format
and the blind-editing loop are `ludion docs mesh-editing`.

## Layout

- `crates/ludion-mesh-edit/src/region.rs` — the signed distance of `sphere`/`capsule`/`box` (negative
  inside), the `feather` fall-off turned into a `[0, 1]` weight, and the `Aabb` a zero-texel op
  reports against the mesh's own bounds.
- `crates/ludion-mesh-edit/src/raster.rs` — `rasterize`: which UV triangle (if any) reaches each
  texel centre of a `width × height` grid, and the surface point there. `MAX_EXTENT` (4096) is the
  derived-map ceiling.
- `crates/ludion-mesh-edit/src/paint.rs` — `WorkingMap` (`f64`, straight-alpha, linear RGB), `paint`
  (blend one op into a map through a region's weight), `dilate` (bleed past a UV island's border).
- `crates/ludion-mesh-edit/src/fill.rs` — the seeded flood fill: `surface` builds the welded adjacency
  (UV islands and seams), `flood` walks it from the seed. The header states the adjacency rule; the
  Rules below state what breaks it. `paint.rs` also holds `key_filter`, the colour-match weight mask.
- `crates/ludion-mesh-edit/src/ray.rs` — `cast`: Möller–Trumbore, double-sided, brute force over the
  triangle list. Pure geometry — the caller supplies `origin`/`direction`.
- `crates/ludion-mesh-edit/src/derive.rs` — `derive()` builds the whole tree in memory; `Derived::write`
  commits it, `Derived::check` byte-compares it. Also the lexical path folding (`components`,
  `normalize`, `relative`) that keeps a derived document's paths OS-independent.
- `crates/ludion-cli/src/mesh.rs` — the `mesh pick`/`mesh apply` verbs. Supplies the one thing this
  crate deliberately does not own: the camera `render --model` draws a cell with
  (`ludion::math::model_camera`/`camera_ray`).

## Dataflow

**`derive()`** (`derive.rs`): parse the edit document (`ludion_scene::check_mesh_edit`) → resolve
`source` and `out` **lexically** relative to the edit document's own directory → read the source mesh
and require `uvs` (`L1176` otherwise) → run `ops` in file order. Each `paint` op: open the map it
targets (the source's own image, decoded into a `WorkingMap`; or a fresh `WorkingMap::opaque_black`
sized by `size` or the base-colour map's dimensions) → rasterise that map's extent once and reuse the
`Coverage` for every op at the same size → `paint::paint` blends `color` through `region::weight` in
**linear light**, quantising the authored colour to its sRGB byte first so full weight reproduces it
exactly → zero texels painted is `L1177`. After every op, each touched map is `paint::dilate`d
`DILATE_PAD` (2) texels outward, then encoded to RGBA8 exactly once
(`WorkingMap::to_rgba8`) and named by the content hash of its PNG bytes. The derived mesh document is
the source's text with `generated_from:` added and every texture slot repointed — touched slots at
the new hash-named file, untouched slots at the pristine file's path, recomputed lexically rather
than resolved on disk (Rules). `derive()` self-checks the emitted document's parse→emit round trip
before returning, so a derive can never commit a document the drift gate would reject forever.

**`Derived::write`/`Derived::check`** (`derive.rs`) both start from the same in-memory `Derived` —
one bake shared by writing and checking, `generate-atlas`'s own shape (`ludion devdocs
sprite-pipeline`). `write` refuses (`L1176`) if the derived directory holds a file `derive()` did not
produce (`is_derive_owned`: exactly `mesh.mesh.yaml`, `textures/<16 hex>.png`, or a crash-leftover
hidden `.tmp` from `ludion_core::atomic` at either level, which the stale pass sweeps), or if `mesh.mesh.yaml` there carries another edit's `generated_from:`
(a line scan, so a drifted copy of this edit's own document still regenerates) — before writing a
single byte. Then the order: the PNGs (content-hash names, so a reader of the old tree is
undisturbed), the document by temp-then-rename, stale maps last — a failed write leaves the previous
tree whole. `derive()` refuses a `source` inside its own `out` (`L1174`). `check`
byte-compares every produced file and reports a foreign file too, as drift rather than a refusal.

**`ludion mesh pick`** (`ludion-cli/src/mesh.rs`): rebuild the exact `render --model` camera for
`(view, bounds, cell_w, cell_h)` via `ludion::math::model_camera`, turn the requested pixel's
**centre** into a ray via `ludion::math::camera_ray`, transform the mesh's positions forward into
that camera's normalized space (`p * scale + translation`, `ModelCamera`'s own fields), and cast
against them with `ray::cast`. The hit's barycentrics are then re-interpolated over the **un**normalized
positions, so the point a region is written against is read in the same coordinates as the region
algebra, never the sheet's normalized space.

## Rules

- **Strict determinism denylist, the `ludion-sprite` shape** (`ludion devdocs determinism`): no std
  `HashMap`/`HashSet`, no wall clock, no threads; every irrational operation goes through
  `ludion_core::math` (`region.rs`'s `sqrt`). A derived PNG is a **committed, byte-compared** artifact,
  so nothing here may vary by platform, allocator order or thread count.
- **Texel math is `f64`, rounded to bytes exactly once**, after the last op and the dilate pass
  (`WorkingMap::to_rgba8`). Ops therefore compose without a per-op requantisation: painting once at
  weight 0.75 and painting twice at weight 0.5 land on the same bytes (`paint.rs` test
  `ops_compose_without_a_round_trip_through_bytes`).
- **A texel covered by more than one UV triangle resolves to the lower triangle index** — the rule
  `ludion-sculpt`'s triangle soup and this crate's own ray cast both already follow, so a texel centre
  sitting exactly on a shared edge, and a ray tie between two coplanar triangles, each have one answer
  rather than two.
- **Every path a derived document carries is computed lexically, never through the filesystem**
  (`derive.rs`'s `components`/`normalize`/`relative`). Resolving a real path folds `..` differently
  across platforms; textual folding does not, so the same edit document derives byte-identical text
  on Windows and macOS.
- **Both conformance hashes must be reproduced on both pinned platforms before a bless counts**
  (`crates/ludion-mesh-edit/tests/derive.rs`: `PLATE_EMISSIVE_HASH` for the rasteriser, region algebra
  and blend; `STRIP_FILL_HASH` for the fill's adjacency and seam bridges over a split-seam strip). A
  mismatch between the two platforms is a determinism bug, never a per-OS constant — the M4 owes both.
- **A fill's membership is reachability, so it must not depend on visit order** (`fill.rs`). The FIFO
  queue and fixed neighbour order are belt and braces; the guarantee is that a texel is in iff a path
  of in-tolerance, in-radius covered texels reaches it. Anything that makes membership depend on which
  texel was dequeued first (a running colour, a first-come claim) breaks the conformance hash on one
  platform only. A `color_feather` widens "in-tolerance" to the fade band; a member's colour weight is
  still its own distance to the seed colour, never its neighbour's.
- **`paint::colour_weight` is the one colour-edge formula**, shared by the key and the fill: the
  inclusive `d² <= tolerance²` test comes first and returns exactly `1`, so an absent or `0`
  `color_feather` is bit-identical to the hard edge and never moves a committed hash.
- **Fill adjacency is exact-position weld plus seam bridges, never a distance tolerance** (`fill.rs`
  header). A near-position weld would merge distinct vertices of a fine mesh; the exact weld is safe
  because a generated mesh duplicates a seam vertex bit-for-bit. Islands that merely touch in UV, and
  texels no triangle covers, are never connected except through a seam bridge (`BRIDGE_REACH` texels,
  same-island only). The seed must lie within `SEED_EPSILON_FRACTION` (1e-3) of the mesh diagonal of the
  surface (`L1178`).
- **`pick` composes the model sheet's camera; it never re-derives one.** `model_camera` and
  `camera_ray` are the same functions `render --model` calls, made `pub` and reused rather than
  reimplemented, because two camera definitions would agree today and diverge silently — every pick
  after that would be a few pixels off with the author blaming their region. Two tests guard this:
  `ludion-engine`'s `model_sheet.rs::the_public_camera_is_the_sheet_camera` checks `model_camera`
  against the cell scene actually synthesized, over every view at two cell sizes; `ludion-cli`'s
  `tests/mesh_pick_camera.rs` renders a real cell and picks against the pixels that came back, so a
  drawn silhouette and a picked miss can never disagree.

## Gotchas

- **A UV outside `[0, 1]` is refused (`L1176`), not wrapped.** A map addresses that one square; a UV
  past it means the mesh tiles, so one texel would serve several surface points and "paint here" has
  no single answer.
- **The `mesh-edit-drift` gate is a fourth member of the drift-gate family, kept in step the same
  way** (`ludion devdocs sprite-pipeline`'s `atlas-drift` precedent): `scripts/mesh-edit-drift.mjs` is
  the gate, wired as a step in `scripts/check.mjs`, `.github/workflows/ci.yml` and the private
  provisioner — all three named the same and changed together. It walks the edit
  documents (`*.mesh-edit.yaml`), spawns `ludion mesh apply --check --json` per edit rather than
  re-implementing the byte-compare, and requires each tree at that edit's `out_dir`: a derived mesh
  naming an edit from anywhere else is a moved or orphaned copy.
- **The derive owns every byte under `out`, so a hand-added file there is refused, not ignored** —
  `Derived::write` checks the directory's full contents against `is_derive_owned` before writing
  anything, and `Derived::check` reports the same file as drift rather than silently skipping it.
  One edit per `out`: the document's `generated_from:` is the owner record `write` compares.

## Verify

```
cargo test -p ludion-mesh-edit                      # region/raster/paint/ray unit tests + doctests
cargo test -p ludion-mesh-edit --test derive         # the conformance hash, drift, L1176/L1177 paths
cargo nextest run -p ludion-cli --test it -E 'test(/^mesh::/)'                 # `ludion mesh pick`/`apply` against the built binary
cargo nextest run -p ludion-cli --test it -E 'test(/^mesh_pick_camera::/)'     # render a cell, pick against its own pixels (gpu group)
node scripts/mesh-edit-drift.mjs                     # committed derived trees still match their edits
```
