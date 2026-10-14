---
description: kind: sculpt and kind: map/map-kit in ludion-scene: parse split, generate, drift
---

# ludion-scene: sculpt and map documents

**Scope:** Changing `sculpt.rs` or `map.rs`. Hub and shared rules: `ludion devdocs scene-kind-documents`; formats: `ludion docs sculpt-format`, `ludion docs map-authoring`.

## Rules

### Sculpt documents (`kind: sculpt`)

`sculpt.rs` owns `sculpts/*.sculpt.yaml` — the format itself is `ludion docs sculpt-format`. Same
structural family as `mesh.rs`/`voxel.rs` (shared `yaml`/`scalar`/`envelope` layers, no registry), but
split three ways instead of two: `L1156` is document *structure*, `L1157` is *data consistency*
(finite floats, ranges, name uniqueness, the `material`-vs-`op` rule), and `L1160` is the *bone tree*
(unknown bone/parent, a second root, a cycle) — its own code because a bone-tree defect is a graph
problem, not a scalar one.

Three things about this module are not recoverable from its signatures:

- **A part's `shape` decides its key set, read before any other key.** `SculptShape` carries only its
  own shape's fields, so a key belonging to another shape (`radii` on a `box`) cannot survive to
  `validate` — it is caught in `parse_part` as `L1157` listing that shape's keys, while a key no shape
  has at all is the structure code `L1156` with a nearest match.
- **Canonical means minimal for part defaults — the sprite rule, not the voxel one.** `at` at the
  origin, `op: union`, `blend: 0`, `sharp: false`, and a box's `round: 0` are not written;
  `resolution` and `triangles` always are. `emit(parse(emit(x))) == emit(x)` holds for every document
  (byte-idempotence); `emit(parse(x)) == x` only for already-canonical `x`.
- **`apply_sculpt_overrides` is the palette override's wider twin.** Beyond the palette pointers
  `voxel::apply_palette_one` already covers, it addresses `/resolution`, `/triangles`, and every
  part's numeric fields by index — a non-numeric pointer (`op`, `material`, `bone`, a whole vector) is
  `L4008` naming the domain, never coerced.

Meshing a `SculptDocument` into geometry is **not** here — `ludion-sculpt` owns that (`ludion devdocs
ludion-sculpt`), so this crate stays free of any renderer type, the split `voxel.rs` already holds
with its engine-side mesher.

### Map documents (`kind: map` / `kind: map-kit`)

`map.rs` compiles a glyph grid plus a marker list into a canonical `Scene` of real entities, so a big
district is a small diff instead of hundreds of hand-placed component blocks
(`Plan/map-authoring.md`). The authoring format is `ludion docs map-authoring`; this section is for
changing the module. It is a structural sibling of `mesh.rs`/`voxel.rs` — shared `yaml`/`scalar`/
envelope layers, no registry — and is **engine-generic**: the kit references only builtin components,
so the game owns the kit *content* and the engine never hardcodes a floor rule.

- `parse_map`/`parse_map_kit` check *structure* only (`L1021`, with nearest-match on unknown keys).
  Grid and kit *semantics* are `generate`'s job, mirroring the mesh/voxel parse-vs-validate split.
- `generate(map, kit, fragments, registry) -> Result<Scene, Vec<Diagnostic>>` is **pure and IO-free**:
  `fragments` arrive already parsed (the `String` is each fragment's path, for diagnostics) and the
  CLI does all file IO and kit/fragment resolution. Same inputs → byte-identical `emit()` output,
  which is the property the `map-drift` gate leans on.
  `registry` names the entity-reference fields: a fragment's references are local to it and are
  rebased past the entities emitted before it (`L1017` when dangling).
- `Scene.generated_from` comes from `MapDoc.source`, which is **not a YAML key** — the CLI sets it to
  the `.map.yaml` file name. It is the drift gate's provenance marker.
- **Emission order is fixed for diff stability**: fragments verbatim (file order, entity order) → grid
  rects and props sorted row-major by anchor cell → markers in source order. Entity names are
  coordinate-derived and validated for **global uniqueness** across all three sources (`L1025`),
  because gate anchors resolve by name.
- Variant selection is `variant_pick(seed, row, col, count)` — a splitmix64 mix, **independent per
  cell**, so an edit in one corner never reshuffles variants elsewhere.
- A map is framed by `rows` or, grid-less, `bounds: [w, h]` (`MapDoc::frame`): a `bounds` map skips
  `emit_grid` — `Grid::glyph_at` past the rows would read non-void to the base underlay — and refuses
  `base`; empty `rows` with markers is `L1023` at `/rows`. `MapGrid::new` reads the same frame. A
  merged primitive (`merge: true` or the `base` glyph) must carry a cell-sized `aabb` (`L1022`), a
  fragment may not carry `physics:`/`seed:` (`L1021`), and every generated coordinate goes through
  `f32_exact` so the writer spells it as a short `f32` decimal. `generate-map --check` drift is `L1039`.
- Diagnostics: ragged rows (`L1019`) and unknown glyph (`L1020`) are reused from `Tilemap`; then
  `L1022` (invalid kit entry or marker `variant`), `L1023` (marker out of bounds), `L1024` (unknown
  marker kind), `L1025` (name collision), `L1026` (footprint misfit). **`L1027` is registered but
  never raised here** — resolving a sibling map off disk would make the generator do IO, so it is
  `ludion map-check`'s to raise.


## Verify

```
cargo test -p ludion-scene
```
