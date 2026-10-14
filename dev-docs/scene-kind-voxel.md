---
description: kind: voxel in ludion-scene: the document module, the ops/gen write side, drift gate
---

# ludion-scene: voxel documents

**Scope:** Changing `voxel.rs`, `voxel_edit.rs` or `voxel_gen.rs`. Hub and shared rules: `ludion devdocs scene-kind-documents`; formats: `ludion docs voxel-format`, `ludion docs voxel-ops`.

## Rules

### Voxel documents (`kind: voxel`)

`voxel.rs` owns `voxels/*.voxel.yaml` — the format itself is `ludion docs voxel-format`. Structurally
it is `mesh.rs`'s sibling: same shared `yaml`/`scalar` layers, no registry, `L1113` for structure and
`L1114` for data consistency, and a fixed key set that is also the canonical emit order.

Three things about this module are not recoverable from its signatures:

- **`scale`, `generated_from` and `budget_exempt` are metadata the mesher never reads.** All three are
  parsed, validated, emitted, and *never consumed* — which is why adding them moved no golden and no
  replay. The doc's `scale` is authoritative as *intent*, a scene's `Transform` as *fact*, so
  `Transform.scale` keeps one meaning everywhere.
- **Two warnings are returned out-of-band as `Option<Diagnostic>`, not as errors.** The `Diagnostic`
  model has no warning severity, so a warning that must not fail is modeled as an `Option`.
  `voxel_budget_warning` is `L1115` above `VOXEL_CELL_BUDGET` (`budget_exempt: true` returns `None` —
  the author's explicit opt-out, never inferred and never written by a tool);
  `voxel_connectivity_warning` is `L1129`, more than one 6-connected component, which is legal (a
  deliberately detached accent) and is what catches a limb an ops edit severed.
- **`L1129`'s message costs O(cells), not O(main × stray).** It names the main body's size, then one
  line per stray piece smallest-first with its cell count, a sample cell, and the **nearest pair** of
  cells between it and the main body plus the gap in steps. The nearest pair comes from ONE
  multi-source BFS seeded with every main-body cell at once, traversing empty cells too; each stray's
  answer is the minimum over its own cells on the explicit key `(distance, cell)`, never on traversal
  order.

Typed accessors are `palette_index(x, y, z)` — `None` for empty, out-of-range, or a non-palette glyph,
and **out-of-range-reads-as-empty is the deliberate seam** a later chunked-voxel-worlds phase reads
through — plus `cell_count()`, `filled_count()`, and `exposure()`.
**A sweep over every cell goes through `for_each_cell`, never a per-cell `palette_index` loop** —
`palette_index` is `chars().nth(x)` plus a palette scan, so a whole-grid loop over it is
O(cells × width), ~175M character steps on the 128×192×112 `house-c`, which is what collapsed the
editor's Voxel Artist view on selection. `for_each_cell` decodes each row once through a
`[Option<u8>; 128]` ASCII glyph lookup (an array, not a map: the determinism denylist bans std
hashing), shared by `Occupancy` and `validate`'s slice scan. A caller wanting both the component
count and `L1129` calls `connectivity()` — one occupancy, one flood fill; `component_count()` and
`voxel_connectivity_warning` are the single-answer wrappers over it. `ExposureReport` is one 6-neighbour
sweep: filled/surface/fully-enclosed totals, `GlyphExposure` per glyph in use (up to five enclosed
samples in `(y, z, x)` scan order plus an overflow count), and `EmissiveFaces` per emissive glyph,
whose `faces: [u64; 6]` histogram is in the **fixed** order `EmissiveFaces::FACE_LABELS` =
`+X -X +Y -Y +Z -Z`, never sorted by count. The meshing that turns a `VoxelDocument` into geometry is
**not** here — it lives engine-side, so this crate stays free of any renderer type.

## Dataflow

### Voxel authoring (the write side: `voxel_edit.rs`, `voxel_gen.rs`)

These two modules are how a model gets authored above hand-editing size
(`Plan/voxel-authoring-tools.md`; the `ludion voxel` verb family drives them,
`ludion docs voxel-ops` is the authoring loop and `ludion docs voxel-generation` the script rung). Both are **pure and IO-free** — they take source text
and return result text, and the CLI does every read, temp-write and rename.

**Two output paths, and the split is the module's whole design.** A cell-level edit patches the source
text per character (`surgical: true`), so every comment, blank line, key order and float spelling is
byte-identical outside the changed cells; a reshape (`grow`/`crop` — anything that changes `size`)
re-emits canonically through `emit_voxel_with_comments`, remapping layer comments to their new indices
and reporting any it cannot re-anchor. **The surgical patch is accepted only if the patched text
re-parses to exactly the op-produced document**, else it falls back to the canonical re-emit — that
check is what makes per-character patching safe rather than clever.

- `parse_voxel_ops` — the strict-YAML ops document, structure only (`L1131`, JSON Pointer +
  nearest-match over the op names and each op's own field set). Coordinates are comma-joined *strings*
  (`at: "3,10,4"`) because the strict subset rejects inline sequences.
- `apply_voxel_ops` — the executor. Volume ops clip; a `set` outside the grid, a glyph with no palette
  entry, or an inverted `crop` is `L1132`. `warnings` carries `L1134`: one entry per (mirror op,
  earlier writer op) pair where the mirror's copy overwrote cells that writer deliberately set — a
  warning, not a rejection, since the mirror already won and the model is well-formed; it exists to
  flag the likely-accidental op order.
- `upscale_voxel` — nearest-neighbor `factor`³ block expansion, `factor` in `2..=8` (`L1132` outside,
  and on a `size` that would overflow `u32`). `scale` divides by `factor` so the world footprint is
  invariant, `budget_exempt` carries, and **`generated_from` is dropped** and reported: the output is
  not that script's product, and keeping the key would make the `voxel-drift` gate reject the file.
- `extract_voxel_comments` — the comment model both lean on: a header block, a per-layer-index map,
  and the `stray` comments belonging to no layer.
- `voxel_from_json` (`voxel_gen.rs`) — the `kind: voxel-gen` JSON intermediate a committed
  `*.voxel.gen.mjs` prints (`L1133`; `ludion explain L1133` prints the schema). The `kind` is
  deliberately *not* `voxel`, so a `.voxel.yaml` converted to JSON cannot be mistaken for one.

**The invariant that justifies all of it: canonical serialization exists in exactly one place.**
Generator scripts print JSON and the engine writes the YAML — a script that emits YAML is a second
source of truth for key order, float formatting and comment conventions, and drifts the first time
the format grows a key. Enforced socially by the docs and mechanically by the `voxel-drift` gate
(`scripts/voxel-drift.mjs`, check.mjs + ci.yml): every committed model bearing `generated_from:` must
regenerate byte-identical through `ludion voxel from-json --check`. The key is both the contract and
the gate's discovery mechanism — there is no manifest.


## Verify

```
cargo test -p ludion-cli --test voxel_cmd   # the `ludion voxel` verb family
node scripts/voxel-drift.mjs               # committed generated models still match their scripts
node examples/voxel-gen/brazier.voxel.gen.mjs | cargo run -q -p ludion-cli -- voxel from-json --check examples/voxel-gen/brazier.voxel.yaml # POSIX shells only — a PowerShell pipe re-encodes the bytes (L1135); use `emit({ out })` + `--in` there
```
