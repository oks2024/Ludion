# Assembly `--set`/`--sweep` staging

**Status: SHIPPED (2026-08-20, one commit).** Waves A+B+C landed as designed; `L4008` for an assembly now fires only on `--sweep` + `--strip`. Laptop verification: ludion-scene / ludion-engine model_sheet / ludion-cli render_model + docs green under the ICD pin; the gallery byte-exact goldens fail here at the known Intel/Dx12 SSIM ~0.9997 class (pre-existing, reference box owes the full lane).

Lifts the `L4008` deferral recorded in
`Plan/voxel-animation.md` ("the override path stages one re-emitted file; an assembly needs
itself plus every part re-staged"). Trigger fired by the wave-4a player flame value pass —
the largest remaining assembly-authoring cost.

## Goal

`ludion render --model x.assembly.yaml --set /palette/<glyph>/<field>=<v>` and `--sweep`
behave exactly as they do for a `kind: voxel` model: the assembly's one `palette:` is the
override target, the rendered sheet is the posed/rest sheet of the mutated copy, and the
source tree is never written.

## Design (settled by the existing machinery, no new primitives)

1. **Palette applier generalizes, it does not fork.** `AssemblyDocument::palette` is
   `Vec<PaletteEntry>` — the same type as `VoxelDocument::palette` by design. Lift
   `apply_palette_overrides` / `apply_palette_one` (`crates/ludion-scene/src/voxel.rs`) to
   operate on `&mut Vec<PaletteEntry>`; keep the `VoxelDocument` fn as a thin wrapper and add
   `apply_assembly_palette_overrides(&mut AssemblyDocument, &[SceneOverride])`. Same pointers
   (`/palette/<glyph>/{color/0-3,emissive,metallic,roughness}`), same `L4008` shapes,
   same `AppliedOverride` echo. No new L-code.
2. **Staging = the lineup precedent + the voxel set/sweep precedent composed.** In
   `crates/ludion-engine/src/model_sheet.rs::stage`, the `LoadedExtra::Assembly` refusal
   branch becomes: clone doc → apply overrides → `ludion_scene::emit_assembly` into
   `<temp>/set/<file_name>` (or `<temp>/cell-<i>/<file_name>` per swept value) → copy the
   dependency tree beside it relative to the assembly's own directory. `copy_assembly_parts`
   grows clip files (`doc.clips[*].file`) — today it copies parts only because the lineup
   path renders rest pose; a posed `--set` cell needs its clip. Rename to
   `copy_assembly_dependencies` (one name, one job). `TempStage` cleanup unchanged.
3. **Composition rules.** `--set` composes with everything (`--clip`, `--time`, `--strip`,
   `--views`, lineups). `--sweep` + `--clip [--time]` is allowed (one posed cell per value).
   `--sweep` + `--strip` is `L4008` ("both multiply cells; pick one") — the same class as
   the existing sweep-across-two-views refusal. `--sweep` across a lineup stays refused as
   today.
4. **Byte-identity invariants (pinned by test):** an assembly with `--set` of a value equal
   to its authored value renders byte-identical to the un-overridden sheet; the voxel path
   is untouched (existing set/sweep tests unmoved).
5. `check-scene --help` wording: the voxel-view flags apply to `kind: assembly` too; fix
   the help text in the same change (tiny, rides wave C).

## Waves

- **A+B (engine):** ludion-scene generalization + model_sheet staging + engine tests
  (`tests/model_sheet.rs`: set on assembly stages parts+clips and renders; set+clip poses the
  mutated copy; sweep cells; sweep+strip refusal; equal-value byte identity).
- **C (CLI + docs):** flip `set_on_an_assembly_model_is_a_deferred_l4008` into the happy
  path + an assembly sweep test + a sweep+strip refusal test in
  `crates/ludion-cli/tests/render_model.rs`; `check-scene --help` wording; docs —
  `docs/voxel-assemblies.md` gotcha → a one-line "works like voxel" note,
  `dev-docs/assembly-animation.md` lines 96-99, `Plan/voxel-animation.md` deferral closed;
  `node scripts/check.mjs --fix` to re-mirror embedded docs.

## Out of scope

Overriding a *part* file's content (parts have no palette); a wire/editor surface; any
change to `--set` on scenes.
