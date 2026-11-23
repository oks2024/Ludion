# Plan: Character–assembly merge — one document kind for "a model plus its clips"

> Status: **planned (2026-11-22).** No wave shipped. The design was written from the tree at Lore 2514; re-grep
> before editing.

## Status

Nothing shipped yet. Each wave appends its file table and its verify line here.

## Trigger

`Plan/animation-clips.md` decision 10 kept two hand-authored documents: `kind: character` binds a skinned mesh
to its clips, `kind: assembly` is the voxel model with its own `clips:` map. Its merge trigger, "a character
feature that assemblies also want", fired with `Plan/animation-layers.md` (masks and blend spaces were built
twice, once per family, and IK, ground feet and sockets landed on assemblies only). The owner accepted the merge
on 2026-11-19 as its own arc (`Plan/animation-layers.md` §Owner rulings).

## What exists (Lore 2514)

| | `kind: character` | `kind: assembly` |
|---|---|---|
| scene crate | `ludion-scene/src/character.rs` (612 lines): `CharacterDocument { name, mesh, clips, masks, blends }`, `L1199`/`L1200` | `ludion-scene/src/assembly.rs`: `AssemblyDocument { name, size, palette, parts, clips, joints, masks, blends, ik, ground }`, `L1137`/`L1138` |
| clip entry | mapping `{ clip, speed?, events? }` (`CharacterClip`) | scalar `name: path` (`AssemblyClip { name, file }`, shared with `kind: prefab` through `envelope::parse_clips`) |
| masks, blends | `rig_mask.rs`, `rig_blend.rs`: shared, joint names | same modules, part names |
| IK, ground | none | `rig_ik.rs` (`ik:`, `ground:`) |
| engine load | `ludion-engine/src/character.rs` → `CharacterData` (skin palette path) | `ludion-engine/src/assembly_mesh.rs` → `AssemblyData` (part placement path) |
| shape | `Mesh.shape: { character: <path> }` (`MeshSource::Character`) | `Mesh.shape: { assembly: <path> }`, `{ assembly_part: … }` |
| tools | `check-scene` (`run_character`, contact heights), `render --model *.character.yaml` (suffix dispatch) | `check-scene` (`run_assembly`), `render --model *.assembly.yaml` |
| editor | Models view Characters library (`editor/app/src/editor/mesh/character.ts`, lists `.character.yaml`) | Voxel Artist stages `.assembly.yaml` read-only (`Plan/editor-voxel-assemblies.md`) |

The two runtimes stay different by nature: a skinned model poses a GPU bone palette, a voxel model poses one
instance per part. The merge is a **document** merge with one load-time dispatch, not a runtime merge.

## The hash constraint

Every registered component feeds `WorldHash` through its serde form (`ludion-sim/src/hash/canonical.rs` writes the
variant name and the path string). `Mesh.shape: { character: characters/mannequin.character.yaml }` is hashed in
committed scenes with traces and replays: `character-cycle` (trace, `.lrp`), `ragdoll-skin-switch` and
`ragdoll-pose-blend` (traces), `ragdoll-skinned` (trace, `.lrp`, through `prefabs/mannequin-ragdoll.prefab.yaml`'s
`$skin`) and `physics-horde` (trace, `.lrp`, 120 shapes); the other scenes naming it are golden-only. Renaming the variant **or** the file path moves those hashes; the re-record round is
spent. A document's *contents* are load-side only: the asset cache hashes the bytes for reload edges, never into
`WorldHash`, and the loader produces the same `CharacterData`, so the pictures do not move.

## Decisions

1. **One document kind: `kind: assembly`.** It holds either a voxel body (`size`, `palette`, `parts`, as today)
   or a skinned body (`mesh:`, the skinned mesh document's path). Clips, masks and blends are shared. **Why:** the
   owner's ruling names `assembly` as the survivor, and the code agrees: the assembly is the richer document
   (ten keys against five) and owns every newer feature (IK, ground, sockets, `$root`), while a character's five
   keys map onto assembly keys one-to-one.
2. **The body is one optional field, not an enum.** `AssemblyDocument` gains `mesh: Option<String>`; `Some` is a
   skinned assembly. `mesh` and any of `size`/`palette`/`parts` together are `L1137`; a skinned assembly also
   refuses the voxel-only keys `joints:` (the ragdoll switch's part joints; a skinned model ragdolls through a
   prefab's `$skin`), `ik:` and `ground:` (`L1137`, each naming why). Without `mesh`, `size`/`palette`/`parts`
   stay required. **Why:** an enum body would rewrite every consumer of `size`/`parts` (the assembly mesher, the
   model sheet, `check-scene`, `rig_ik`) for no behavior; the engine reads the body once, at load.
3. **Field mapping.** `kind: character` → `kind: assembly`; `name`, `mesh`, `clips`, `masks`, `blends` carry over
   unchanged, in the same canonical order (`format`, `kind`, `name`, `mesh`, `clips`, `masks`, `blends`). A
   character migrates by its `kind:` line alone (plus the clip-entry canonical form, decision 4).
4. **One clip entry, two spellings, one canonical.** An entry is a scalar path (`walk: anims/walk.anim.yaml`) or
   a mapping `{ clip, speed?, events? }`. The writer emits the scalar when `speed` is 1 and `events` is empty, the
   mapping otherwise. `AssemblyClip` gains `speed: f64` (default 1.0) and `events: Vec<AnimationEvent>`. **Why:**
   every committed voxel assembly (vigil's and the external games') re-emits byte-identically, and the mapping is
   the character overlay (`animation-clips` decision 11) unchanged. `kind: prefab`'s `clips:` stays scalar-only.
5. **The overlay (`speed`, `events`) is skinned-only.** On a voxel assembly it is `L1138`, whose help says to put
   events in the clip document and to scale playback with `Animation.speed`. **Why:** the overlay exists because an
   imported clip is derived (decision 11); voxel clips are hand-authored and already carry their own events. Trigger
   to lift: a voxel game that needs a per-model clip rate.
6. **The cross-file rules move with the document.** `character::check_clips` (overlay against the clip's own
   events, overlay event times against the duration, track and mask names against the mesh's joints, blend points)
   becomes `ludion_scene::check_assembly_mesh_clips(doc, clips, joints)` with `L1138`/`L1198`. Masks on a skinned
   assembly name joints; on a voxel one, parts (as `rig_mask` already does).
7. **The old document is `L1246` `CHARACTER_KIND_MERGED`.** Reading `kind: character` anywhere (`check-scene`, the
   loader, the fuzz round-trip) reports: "`kind: character` was merged into `kind: assembly`", help "change the
   line to `kind: assembly`; every other key carries over". `L1199` and `L1200` retire into `codes::RETIRED` when
   the character module is deleted (W2), their facts now `L1137`/`L1138`. No silent alias: an old document is an
   error that names the one-line fix.
8. **`Mesh.shape: { character: <path> }` keeps its spelling until the next re-record round** (Proposed — owner
   ruling owed). It names a skinned assembly; `{ assembly: <path> }` names a voxel one and refuses a skinned
   assembly (`L1138`, help: draw it with `{ character: … }`). The committed `*.character.yaml` files keep their
   names. **Why:** decision-level hash rule above; this keeps one spelling per body at every moment (no alias),
   and the external game's 1,000-odd shape lines keep working. The final step (decision 12) is scheduled, not
   dropped.
9. **The runtime types keep their names.** `CharacterData`, `load_character`, `contact_heights` stay: they name
   the skinned runtime, which still exists. Renaming them is churn for an external game's Rust with no behavior.
10. **What each side gains.** Skinned models gain nothing they lacked in document terms (masks and blends were
    already shared); voxel models gain the mapping clip spelling. Skinned IK (`ik:`/`ground:` by joint name) is not
    built here — trigger: a skinned game asks for IK or foot grounding (the external voxel game's request scoped it
    out). `$root` root motion on skins stays `Plan/animation-layers.md` W6b's trigger. Sockets and `AnimGraph`
    already serve both.
11. **No hash, image or schema moves in W1–W3.** `MeshSource` is untouched, so neither the component schema
    snapshot nor the wire schema moves; migrated mannequins load to the same `CharacterData` and render
    byte-identically; no replay or trace is re-recorded.
12. **The final rename rides the next re-record round** (Proposed — owner ruling owed): `{ assembly: … }` routes
    both bodies, `MeshSource::Character` is removed (its old spelling then an `L1246`-style error), the
    `*.character.yaml` files become `*.assembly.yaml`, the editor's Characters library folds into the assembly
    lister, `render --model`'s suffix dispatch reads `kind:`, and every listed trace and `.lrp` re-records last.
    Trigger: the next scheduled re-record round, or the owner authorizing one for this.

## Inventory (Lore 2514)

**`kind: character` documents and fixtures (migrate in W2):**
- `examples/gallery/characters/mannequin.character.yaml`, `examples/gallery/characters/mannequin-lod.character.yaml`
- `fuzz/corpus/kind_roundtrip/character.yaml`
- inline fixtures: `crates/ludion-scene/tests/character_roundtrip.rs` (24), `crates/ludion-scene/tests/path_roundtrip.rs`,
  `crates/ludion-cli/tests/check_scene_character.rs` (3), `crates/ludion-engine/tests/{animation_events,
  animation_layers,blend_spaces,character_animation,layer_events}.rs`, `crates/ludion-engine/examples/character_clips.rs`
- code naming the kind: `crates/ludion-scene/src/{character.rs,lib.rs,kinds.rs}`, `crates/ludion-cli/src/check_scene.rs`,
  `crates/ludion-engine/src/{character.rs,asset_cache.rs,blend_clock.rs,blend_space.rs,mesh_check.rs}`,
  `crates/ludion-components/src/animation_layers.rs` (doc text), `crates/ludion-core/src/codes.rs`

**Docs (W3):** `docs/{character-animation,voxel-animation,animation-layers,components,physics-joints}.md` and
their `crates/ludion-cli/embedded-docs/` mirrors; `dev-docs/{assembly-animation,engine-layout}.md`;
`editor/app/README.md`, `editor/app/src/editor/mesh/character.ts` (+ test), `editor/app/src/views/ModelsView.tsx`.

**`character:` shapes (kept, decision 8):** `examples/gallery/{character-cycle (1), character-socket (3),
crowd-animation (500), decals (1), local-shadows (1), lod-hall (500), physics-horde (120), ragdoll-pose-blend (1),
ragdoll-skin-switch (1), sequence-beat (1), sun-cascades (1)}.yaml`, `examples/gallery/staged/static-kit.yaml` (8),
`examples/gallery/prefabs/mannequin-ragdoll.prefab.yaml` (1); Rust `MeshSource::Character(…)` in engine tests and
`crates/ludion/examples/ragdoll_budget.rs`. vigil has none. `ludion new` writes neither kind.

## Waves

- **W1 — the merged document (`ludion-scene`, `ludion-core`, a guard in `ludion-engine`).** `mesh:`, the clip
  mapping and overlay, body rules, `check_assembly_mesh_clips`, `L1246` from the assembly parser, the
  `assembly_mesh` loader refusing a skinned body. `kind: character` still loads through the old module. Verify:
  `check.mjs --scope` on the three crates, `--lib` for each, `assembly_roundtrip`.
- **W2 — the switch and migration (`ludion-engine`, `ludion-scene`, `ludion-cli`, `ludion-core`, fixtures).** The
  character loader reads `check_assembly` and requires `mesh`; `check-scene` routes a skinned assembly to the
  character report and `kind: character` to `L1246`; the `kinds.rs` row; the two gallery files, the fuzz seed and
  every inline fixture migrate; `ludion-scene/src/character.rs` and its public API are deleted; `L1199`/`L1200`
  retire. Verify: scoped check, `--lib` per crate, the character/animation `--test it` filters, the
  `character-cycle`, `character-socket` and crowd goldens and traces unchanged.
- **W3 — docs and editor text.** The topics above (mirror after each edit), `ludion explain L1246`, the editor's
  Characters library reading the merged kind (it lists by suffix, so text only), the plan Statuses
  (`animation-clips` d10, `animation-layers` §Owner rulings). Verify: scoped check incl. `docs` and the editor
  lane.
- **Next re-record round — decision 12.** Not scheduled here.

## Out of scope (with triggers)

- Skinned IK and ground (`ik:`/`ground:` by joint name). Trigger: a skinned game asks.
- The overlay on voxel assemblies. Trigger: decision 5's.
- An enum body. Trigger: a third body kind (e.g. a sculpt with clips).
- Prefab clip overlay. Trigger: an imported prefab clip that needs events without a Blender re-run.
- Decision 12's rename. Trigger: the next re-record round.

## Risks

- **Validation on an empty voxel body.** The hierarchy rule ("exactly one root") fails on zero parts; every
  voxel-only rule must be gated on `mesh.is_none()` (W1 tests a minimal skinned assembly).
- **Two engine paths reading one kind.** A skinned assembly reaching `assembly_mesh::load` must be refused, not
  meshed to nothing (W1's guard, tested).
- **External game breaks.** Each `kind: character` file fails `L1246` after W2 until its `kind:` line changes; the
  list is in the arc's DONE report.
