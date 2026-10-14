---
description: The voxel author → check → render loop, and the check-scene text views that answer a model
---

# Voxel authoring

**Scope:** the loop you work a voxel model through, and the text checks that answer most of it with
no GPU. The contact sheet the loop's render step writes is `ludion docs voxel-sheets`; the document
schema is `ludion docs voxel-format`; judging the result is `ludion docs voxel-craft`; writing models
too big to type is `ludion docs voxel-ops`.

## The loop

1. **Author** `voxels/model.voxel.yaml` — by hand up to about 32³; past that see the
   ladder (`ludion docs voxel-ops`), which is the write side of this same loop.
2. **Check the model alone** — fast, no GPU, and it points at the exact bad row:
   ```
   ludion check-scene voxels/model.voxel.yaml
   ```
   The `OK (…)` line always carries the **greedy-meshed** `quads / triangles / vertices` — the
   renderer's real cost, and what to decide `budget_exempt:` on (`L1115` counts cells; a flat wall of
   them is one quad). Four optional views (voxel-only; each prints before the `OK (…)` and never
   changes the validation outcome) let you reason about a model as text without holding every layer
   in your head:
   - `--mirror-x` reports left/right symmetry — `symmetric`, or the differing cells clustered into
     connected regions, largest first, with cell count, bounding box and glyph set (informational: an
     asymmetric model, e.g. a censer in one hand, is legal). Two tight regions read as deliberate;
     forty scattered ones read as a bug. Coordinates are low-half (`x < width/2`).
   - `--slice <y>` prints one layer as its authored rows with a column ruler, `z` indices, and
     `z=0 (front, world -Z)` / `z=<max> (back)` markers; `--slice all` prints every layer bottom-up.
   - `--column <x,z>` prints the vertical profile at one column bottom-up (`y=N: 'G'` / `y=N: .`) plus
     a run-length summary like `filled y: 2-7,9` — what stacks above and below a cell.
   - `--exposure` reports what of the model can actually be *seen*: the filled / surface /
     fully-enclosed totals, one line per glyph in use (cells, enclosed, up to five enclosed sample
     coordinates), and each emissive glyph's exposed faces by direction in the fixed order
     `+X -X +Y -Y +Z -Z`:
     ```
     exposure: 283 filled, 170 surface, 113 fully enclosed
       glyph D: 11 cells, 0 enclosed
       glyph F: 261 cells, 111 enclosed — samples (3,2,6), (5,2,6), ... and 106 more
     emissive faces:
       glyph E (emissive 5.5): 4 exposed — +X 0, -X 0, +Y 2, -Y 0, +Z 0, -Z 2
     ```
     `--from game` adds a line per emissive glyph — `from game view (58 deg): 640 face the camera,
     430 occluded by own geometry` — ray-testing the camera-facing faces against the model's own
     cells at the gameplay pitch. **Exposed is not visible:** a panel recessed too deep scores
     perfectly above and still arrives as two slivers under its own lintel.

     Interiors are conventionally solid, so a large enclosed count on the *structural* glyph is
     normal and says nothing. Two things are signal: a **detail** glyph with enclosed cells (work
     that was authored and can never be seen), and an emissive glyph whose faces all point one way —
     the "lightbar" bug, where a glow meant to read from the front escapes only through the top.
     Both used to cost a GPU render to notice.

   `--json` (alone — not with the views above) prints one summary object instead of the text, on
   success and on a coded failure: `{path, kind, ok, …, diagnostics}`. A voxel adds `name`, `size`,
   `filled`, `components`, `meshed`, `scale`, `generated_from`, `budget_exempt`; an assembly adds
   `name`, `size`, `components` (over the composed grid), `parts[]` (name, file, place, parent,
   mirror_x, meshed) and `clips[]`. `diagnostics` lists every warning and error in order, an error
   omitting `severity`; a failed check has no kind fields. The editor's model readout reads this
   object.
3. **Render the model alone and look** — no scene, camera, or lights to author, and it costs one
   look at one image regardless of how many angles you check:
   ```
   ludion render --model voxels/model.voxel.yaml
   ```
   writes `model.sheet.png` to the current directory: a deterministic turntable contact sheet of the
   model from six canonical angles. This is the fast inner loop for a shape or palette change, before
   the model is ever wired into a scene — `ludion docs voxel-sheets` is the full flag reference, the
   game-rig and lineup modes, and the palette sweep.
4. **Reference it** from a scene and check that too (`ludion check-scene scene.yaml` catches a wrong
   path or a malformed `Mesh`).
5. **Render the scene and look:**
   ```
   ludion render --scene scene.yaml --out shot.png
   ```
   or, in a game project with a golden test, bless once you like the picture:
   ```
   LUDION_BLESS=1 LUDION_BLESS_ANY_ADAPTER=1 cargo test    # writes goldens/<os>/main.png; review, then commit
   cargo test                   # compares; a regression is L2203 with a localized diff report
   ```
   (`ludion check-scene` and the CLI's `render`/`test` see only built-in components; a scene that
   names your game's components is validated through your game's own tests —
   `ludion docs verify-loop`.)

### Live editing

Under `ludion dev` (or any attached viewer), the asset watcher re-reads a `.voxel.yaml` on save: edit
a row, save, and the paused viewport re-meshes and updates with no restart. The text file is the
editor — the tight loop is edit → save → look.

## Gotchas

- **A voxel diagnostic raised while checking a *scene* is re-based onto the referencing entity** —
  e.g. `/entities/2/components/Mesh/shape/voxel` — so you learn both which entity and which cell.

## Next

- `ludion docs voxel-sheets` — the `render --model` harness the loop's step 3 calls.
- `ludion docs voxel-format` — the document schema, orientation, and palette.
- `ludion docs voxel-craft` — what to look for once the checks above pass.
- `ludion docs voxel-ops` — the write side above ~32³.
- `ludion docs voxel-assemblies` — the same loop over a multi-part model, and its composed grid.
