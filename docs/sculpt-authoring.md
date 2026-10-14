---
description: The sculpt new -> check-scene -> render --model -> look loop, and the text-only views
---

# Sculpt authoring

**Scope:** the loop you work a sculpt model through, and the text checks that answer most of it with
no GPU. The document schema is `ludion docs sculpt-format`; judging the result is `ludion docs
sculpt-craft`; the mesher internals are `ludion devdocs ludion-sculpt`.

## The loop

1. **Scaffold** a model:
   ```
   ludion sculpt new sculpts/model.sculpt.yaml [--name] [--palette-from <model>]
   ```
   Writes one root bone, an empty `parts:`, and a header comment pointing back at this loop; never
   overwrites an existing file. `--palette-from` copies an existing voxel or sculpt document's
   palette instead of a one-entry stub, so a new model joins an existing set's color language.
2. **Add a part by hand**, editing `parts:` directly — there is no `sculpt apply` yet
   (`Plan/mesh-modeling.md`); every field is documented in `ludion docs sculpt-format`.
3. **Check the model alone** — fast, no GPU, and it points at the exact bad field:
   ```
   ludion check-scene sculpts/model.sculpt.yaml
   ```
   Prints the `L1112` vertex-budget warning if the meshed result crosses it, then the mesher's own
   `L1158`/`L1159` warnings, then the closing `OK (…)` summary — part/bone counts, the lattice
   extent, triangles before and after decimation, vertex count, and how long meshing took. One
   optional view (sculpt-only; prints before the `OK (…)` line and never changes the validation
   outcome):
   - `--bounds` — one line per part (`min..max` in world units, its surface vertex count, or "no
     surface"), then one line per bone (the union of its parts' bounds). `--bounds` also works on a
     `kind: mesh` document — a single block (no parts, no bones): the model-space box, its centre,
     size, vertex and triangle count, the numbers a `kind: mesh-edit` region's `at`/`radius`/`size`
     are placed from (`ludion docs mesh-editing`).
4. **Render the model alone and look:**
   ```
   ludion render --model sculpts/model.sculpt.yaml
   ```
   writes `model.sheet.png`: a deterministic six-view turntable, exactly like a voxel model's sheet
   (`ludion docs voxel-sheets` covers the shared flags — `--rig`, `--views`, `--true-size`). Two
   sculpt-only additions:
   - `--part <name> [--isolate]` frames every view on one named part's meshed bounds while the rest
     of the model stays in the picture (or is hidden with `--isolate`) — one image answers "does the
     eye sit in the socket". An unknown part, or one that meshes to no surface, is `L4008`;
     `check-scene <model> --bounds` lists the parts.
   - `--set`/`--sweep` reach every numeric field, not just the palette (the override-pointer table in
     `ludion docs sculpt-format`) — e.g. `--sweep /parts/0/blend=0,0.1,0.2` renders one cell per
     blend radius.
5. **Reference it** from a scene and check that too (`ludion check-scene scene.yaml` catches a wrong
   path or a malformed `Mesh`).
6. **Render the scene and look**, exactly as a voxel model's scene render
   (`ludion docs voxel-authoring`'s loop step 5).

### Live editing

Under `ludion dev` the asset watcher re-reads a `.sculpt.yaml` on save and re-meshes — the same edit
→ save → look loop a voxel model gets. The editor's **Sculpt** view is the human's seat at that loop: it
puts a model from `scenes/sculpts/` on the model stage beside the Voxel Artist, and its inspector is
one slider per numeric field, each drag rewriting that one scalar in place (every other byte kept) so
the host's watcher reloads the stage. The text file stays the source of truth; sliders and the
terminal persona edit the same bytes.

## Gotchas

- **Facing.** The sheet's `front` camera looks from `−Z`, so author a model's face toward `−Z` — the
  voxel convention, unchanged.
- **A thin part is invisible before it is wrong.** `L1158` catches a part narrower than two grid
  cells; the warning's `help` names the exact `resolution` that would resolve it, so raise that or
  thicken the part deliberately — not both blindly.
- **`MSYS_NO_PATHCONV=1`** is needed before `--set`/`--sweep` in Git Bash, which otherwise rewrites
  the pointer's leading `/` as a Windows path.
- **`L1112`'s triangle budget is on the *meshed* count.** Lowering `triangles` is the direct fix; a
  document over budget on purpose is `budget_exempt: true`, the same call a voxel model's `L1115`
  asks for.
- **A `sharp` box still reads slightly rounded** — `sharp: true` fixes shading at a crease, not the
  exact silhouette (`ludion docs sculpt-format` Gotchas); don't chase a crisper edge by raising
  `resolution` alone.
- **A sculpt diagnostic raised while checking a *scene* is re-based onto the referencing entity** —
  e.g. `/entities/2/components/Mesh/shape/sculpt` — the same rule a voxel model's diagnostics follow.

## Next

- `ludion docs sculpt-format` — the document schema, semantics, and override pointers.
- `ludion docs sculpt-craft` — what makes a checked, valid model actually read.
- `ludion docs voxel-sheets` — the `render --model` flags shared with voxel and assembly models.
- `ludion docs voxel-format` — the palette shape.
- `ludion devdocs ludion-sculpt` — the mesher: pipeline, determinism, cost model.
