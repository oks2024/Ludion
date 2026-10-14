# Plan: Mesh modeling — the `kind: sculpt` document and the implicit-surface mesher

> Status: **DESIGN SETTLED (2026-09-06, grilling session — every ruling the user's). Wave 1
> mechanized-complete 2026-09-15; Wave 2 mechanized-complete 2026-09-15; checkpoint 1 CLOSED
> 2026-09-16 (verdicts in decisions 7, 9, 21 — defaults 48 / 5 000, silhouette trial extended to
> checkpoint 2, crisp hard-surface planned as Wave 6); the Models view's (formerly the Sculpt view)
> live smoke owed; Wave 3
> mechanized-complete 2026-09-18 — checkpoint 2, the creature sitting, is owed.**
>
> **Stays paused (2026-10-02):** resuming no longer blocks anything in vigil — the environment moved to Blender kits.
>
> **PAUSED 2026-09-26 (the user's ruling)** for the Blender experiment (`Plan/asset-pipeline-blender.md`
> decision 1). Everything shipped stays built and gated. Checkpoint 2, Wave 4 and Wave 6 are frozen.
> **Resume trigger:** the user's word after that plan's phase-1 verdict. Wave 4's renderer half
> (vertex weights, palette, passes) is now built by `Plan/skeletal-animation.md`, fed by glTF skins.
> On resume, Wave 4 keeps only the weights derived from the field, and clips.
>
> **Wave 3 outcome.** Bones carry `head`/`tail`; parts carry `deform` (`bend`/`twist`/`taper`), `noise`
> and `shape: voxel` (`edt.rs`, `sources.rs`, `mesh_with`; `ludion_engine::read_sculpt_sources` the one
> file reader); `--set`/`--sweep` and the Models view's sliders cover the new numeric fields. The
> creature (`examples/gallery/sculpts/creature.sculpt.yaml`, a quadruped at rest: 20 parts, 9 bones,
> 48/unit, 8 000 triangles, ~630 ms) was authored blind in 16 renders and ~155 k tokens; the log
> (`Plan/artifacts/mesh-modeling-log.md`) ranks thirteen tool findings. Rulings made during the
> build, superseding step 1's wording: **every deformer acts about the part's own centre** (a
> capsule's `from`/`to` are model coordinates, so an origin pivot swung it across the model and
> mis-placed every tapered capsule), and **there is no per-part Lipschitz divisor** — a deformed
> part's distance is sign-exact, the conservative bound lives only in the grown box, because the
> divisor let a part win the smooth-min share far outside its geometry and steal a neighbour's
> surface and material; `L1158` names the hiding part (`PartStats::hidden_by`). A centred primitive
> keeps its arithmetic bit for bit, so the idol conformance hash and both idol sheets did not move;
> the creature was re-meshed (one retune: `belly` lowered 0.06 out of the torso) and its Windows
> sheet re-blessed (the M4 twin re-blessed 2026-09-26, rev 916). **Owed:** checkpoint 2 — the sitting on the creature sheet and stage
> (the `--silhouette` flag deleted 2026-09-26, decision 9). Open findings from
> the log, not built: a cone apex sheds a four-vertex `L1159` island; the sheet's bloom hides
> emissive defects a `--part` close-up shows.
>
> **Wave 1 outcome.** Codes `L1156`–`L1160`, `ludion-scene/src/sculpt.rs`, the `ludion-sculpt` crate
> (conformance hash `crates/ludion-sculpt/tests/idol.rs`), `Mesh.shape: sculpt`, `check-scene
> --silhouette/--bounds`, `ludion sculpt new`, `render --model --part/--isolate`, the docs
> `sculpt-format` / `sculpt-authoring` / `sculpt-craft` / `dev-docs/ludion-sculpt`. Rulings made
> during the build: `material` is required on `union` parts and refused on `subtract`/`intersect`;
> `parts: []` is valid (the scaffold); a cone's `at` is its base centre; `--part` pads a tiny part's
> box to 1/8 of the model's largest extent and keeps the camera outside the model; step 7 is a
> no-op (the editor's `protocol.d.ts` comes from the wire snapshot, which names no component type).
> The gallery idol (`examples/gallery/sculpts/idol.sculpt.yaml`, 14 parts, 48/unit, ~179 ms) was
> authored blind by an Opus subagent in 10 sheet renders and ~191 k tokens
> (`Plan/artifacts/mesh-modeling-log.md`); `idol-sheet` and `sculpt-idol` are first-blessed on the
> 3090, and on the M4 2026-09-15 (conformance hash green there; sheet SSIM 0.9999, scene 0.9995 against
> the 3090 set). Checkpoint 1 sat 2026-09-16 on the sheet, the stage and both sweeps (`--views iso`, front for the plinth).
> **Evidence for the sitting** (the author's reads, not rulings): decision 7 — 32/unit is fine for
> plain props, 48 visibly better for faces and carving at ~3× mesh time, 4 000 triangles clean
> (2 000 facets); decision 6/21 — the plinth reads crisp at 48/unit with a faint corner bevel,
> rounded at 32 and below, and the 60° crease split barely fires because surface nets bevels box
> edges below the crease angle, so the hard-surface trigger is half-fired at the default
> resolution; decision 9 — `--bounds` was load-bearing, `--silhouette` marginal (used twice,
> never decisive: a mirrored model's front/back silhouettes match and a wide base fills the top
> views), `--part head` load-bearing. Measured: the test idol meshes in ~55 ms at 32/unit, above the
> "milliseconds" estimate; small emissive parts bleed along long decimated triangles.
> **Wave 2 outcome.** The editor's Models view (named Sculpt at the time): the host watcher (`stage.rs`, `watches_models`) watches
> `<root>/voxels` and `<root>/sculpts` on one `notify` instance; `AssetBrowser` takes `ext` and `root` as
> props (`displayName` in `voxel/tree.ts` is the one suffix strip); `views/ModelsView.tsx` starts the
> same `model` slot on the unchanged `model-stage.yaml` and swaps the subject by `component.set` of
> `Mesh.shape.sculpt`, writing no `Transform` (a sculpt has no declared scale); `editor/sculpt/`
> holds `stage.ts`, `doc.ts` (block-style only — a flow sequence is silently unparsed, never
> misread), `edit.ts` (one splice, integer keys without a decimal point, `x.0` kept when the file
> had it), `ranges.ts` (`SLIDER_KEYS` checked exhaustive at compile time), `rows.ts` (grouping,
> labels, `quantize` to the step so a drag never writes a float residue), `useSculptEdit.ts` (the
> `fileSync` reducer + `save.ts`'s throttle at the watcher's own 150 ms debounce, the whole source
> re-parsed after every splice because a length change moves every later offset) and
> `SculptInspector.tsx`. Rulings: `useVoxelEdit` is not reused (typed on `VoxelDoc` end to end);
> the stage lifecycle is duplicated as `EffectsView` already does, not extracted; the readout is
> static, not a number box. Added beyond the step list: `editor/personas/models.md` (the terminal
> column spawns the persona by view id) and the project-dir card naming both libraries. **Owed:**
> step 5, the next editor sitting's live smoke — open the idol, drag a `blend`, watch the reload.
> Every claim of current-code fact below was read from
> the tree on 2026-09-06 (file:line pointers are of that date; re-grep before editing). Independent
> of every in-flight arc: no wire, schema-hash, `WorldHash` or replay movement until Wave 5 places a
> sculpt into a vigil district.
>
> **Relationships.** The organic sibling of the voxel arc (`Plan/roadmap-voxel-mesh-pbr.md`,
> `Plan/voxel-authoring-tools.md`): the same author → check → sheet → look loop over a second model
> document, producing meshes for the same mesh path. It does **not** replace the voxel format (low-poly
> faceted stays voxel) and does **not** touch `Plan/textured-pbr.md` (its trigger is recorded in
> decision 11). The editor half is the voxel-artist stage view (`Plan/editor-voxel-artist.md`)
> generalized to a second document kind. Driven by `games/vigil` props; the arc is an **experiment
> with three recorded checkpoints**, any of which may end it (decision 18).
>
> **How to read this plan.** Decisions 1–19 are the rulings; *The document* and *The mesher* are the
> specification; each Wave is a numbered step list an instance executes top to bottom, with its tests,
> docs, verify commands and exit criterion. A wave closes by appending its outcome to this Status block
> (checkpoint verdicts are recorded as decisions, never as prose), refreshing the root `CLAUDE.md`
> Current-state paragraph, and pruning the notebook.

## Why

The user has seen LLMs produce good meshes by driving Blender through an MCP server. Read closely, that
workflow is: the model writes Blender operations (primitives, transforms, modifiers, booleans), a
vision tool photographs every attempt, corrections are persisted as hand-written markdown playbooks,
and a mesh takes up to fifteen attempts and on the order of two million tokens of coaching before it is
good. The valuable part is the loop, not Blender's operator breadth — and this repo already runs that
loop for voxels with three things Blender cannot give: a text source of truth that diffs, bit-exact
deterministic meshing gated by tests, and a check step that answers most questions without a render.
Blender's Python surface is enormous and version-drifting, the `.blend` is opaque binary, and a
screenshot is the only feedback. A Ludion sculpt document is a closed, small vocabulary designed around
what a model gets wrong; every op is validated with a coded diagnostic; the result is meshed by the
deterministic CPU tier the voxel mesher lives in. The user also needs no Blender install and no MCP.

The central tension the design answers: voxels are LLM-friendly because there is no continuous
coordinate to hallucinate, and meshes are continuous by nature. So the tool is **never a vertex
editor**. It stays at the level the model is reliable at — named parts with dimensions, ordered
operations, symmetry declared not eyeballed — and the mesher does the rest. That is also what an LLM is
actually good at in a ZBrush-like tool: the ZSpheres idea (a tree of named spheres joined into a smooth
skin), which is discrete, nameable, and literally a skeleton. An implicit-field document built from
capsules, ellipsoids and smooth blends is the same thing, gives booleans and carving for free with none
of polygon CSG's robustness fight, and makes the user's skinning goal cheap later because the sphere
tree already is the rig.

## What exists today (verified)

- **The mesh path takes any triangle list.** `MeshVertex` (`crates/ludion-renderer/src/mesh.rs:17`):
  position, normal, RGBA color, `(roughness, metallic, emissive)`. `MeshData`
  (`crates/ludion-renderer/src/scene.rs:540`): flat `positions` / `normals` / `colors` / `materials` /
  `indices`, `authored_material: bool` (selects per-vertex material over the `Mesh` component's),
  `hash: u64` = content hash of the source bytes, the GPU mesh-cache key. Normals are whatever the
  loader supplies — a voxel mesh's are faceted, a `kind: mesh` document may author smooth ones.
- **Every `Mesh.shape` source is one enum + one loader + one cache entry.** `MeshSource::{Asset,
  Voxel, Assembly}(String)` (`crates/ludion-engine/src/components.rs:217-225`, round-trip test at
  `:2986-3018`); `PlacedMeshSource` mirrors it in `extract.rs:1337-1339`, resolved at `:723-746`
  through the `SceneAssets` trait (`extract.rs:59-71`: `mesh` / `voxel` / `voxel_grid` / `assembly`)
  implemented by `DiskAssets` (`:101`, one-shot: `crate::voxel_mesh::load`) and `CachedAssets`
  (`:150`, through `asset_cache.rs`'s `AssetCache`, `BTreeMap`-keyed, with a per-kind reload branch at
  `asset_cache.rs:407-478`). `bounds.rs:206-216` gives voxel/assembly the bottom-centered unit box;
  `voxelizer.rs:304-308` voxelizes a `Voxel` source from its grid and treats `Asset`/`Assembly` by
  their meshes. `voxel_mesh.rs` is the template: `load(root, rel) -> MeshData` (`:54`), `mesh(doc,
  hash)` (`:244`), `voxel_mesh_stats(doc)` (`:198`, what `check-scene`'s `OK (…)` line prints).
- **Document kinds are routed by `peek_kind`** (`crates/ludion-scene/src/lib.rs:351`). A kind module
  is `parse_x` / `validate_x` / `check_x` / `emit_x` plus out-of-band `Option<Diagnostic>` warnings
  (`voxel_budget_warning`, `voxel_connectivity_warning`, `lib.rs:441-519`); `voxel.rs` owns
  `PaletteEntry { glyph, color: [f64; 4], emissive, metallic, roughness: Option<f64> }` (`:66`) and
  `apply_palette_overrides` (`:2304`, what `--set`/`--sweep` write into). All floats are `f64` in
  this crate; the strict-YAML layer is `yaml.rs::parse_document` (one parser, `pub(crate)`). The
  engine's data-doc walk skips engine kinds through `ENGINE_KINDS` (`crates/ludion-engine/src/docs.rs`;
  an unlisted kind under a content root is `L1119`).
- **The model harness dispatches on `kind:`.** `render_model_sheet` (`model_sheet.rs:570`) loads a
  document at `:1226-1337` into a `LoadedModel { kind: ModelKind::{Voxel, Mesh, Assembly}, bounds,
  parsed }`, the error texts at `:1316-1337` enumerate the accepted kinds (`L1117`); palette overrides
  are applied at `:1866`. `sweep.rs::render_sweep_sheet` (`:142`) and `render_frame_strip` (`:292`)
  address a document by JSON pointer, `L4008` refusing an unstageable pointer. `check_scene.rs` routes
  kinds at `:74-89` into `run_voxel` (`:217`, prints warnings, the `VoxelViews` text views, then the
  `OK (…)` line at `:264`), `run_mesh` (`:197`), `run_assembly` (`:516`); `VoxelViews` (`:18`) is the
  flag struct. `ludion voxel new --palette-from` is `voxel.rs::run_new` (`:29`), dispatched from
  `main.rs:1262-1284`.
- **Goldens.** `ludion_testkit::discover` (`suite.rs:82`) scans **top-level** `examples/gallery/*.yaml`
  and dispatches on `kind` (`golden.rs` module docs); model documents living in subfolders get their
  sheet goldens by hand in `crates/ludion-testkit/tests/gallery.rs` (`lantern_sheet_test` `:54`,
  `figure_sheet_test` `:74`, `figure_wave_strip_test` `:97` → `goldens/<os>/<name>-sheet.png`). A
  golden's first bless is the pinned GPUs' (the particles precedent: the scene ships, the suite reports
  `L2201` until blessed); the laptop never blesses.
- **Diagnostic codes** live in `crates/ludion-core/src/codes.rs` as documented `pub const`s ; `ludion explain` reads the same table.
  (2026-09-14: `L1147` and `L1155` were taken after this plan was written, so the arc's codes are
  `L1156`–`L1160`, one higher than the grilling session reserved.)
- **A new pure crate follows `ludion-sprite`**: `crates/ludion-sprite/Cargo.toml` (deps `ludion-core`
  + `ludion-scene` + `ludion-assets`, workspace metadata), its `clippy.toml` denylist (the sim-side
  rules), a row in `dev-docs/crate-graph.md:19`, `ludion-engine/Cargo.toml:17` depending on it. The
  workspace is `members = ["crates/*", "games/*"]` — nothing else to register; `mobile-check` only
  asserts the facade tree names no desktop crate.
- **Rigid animation exists, skinning does not.** `kind: assembly` (`parts:` with `pivot`, `mirror: x`,
  `clips:`) + `kind: animation` (block keyframes per named track, `linear`/`smooth`/`step`) + the
  `Animation { clip, started: FrameIndex, speed }` component (`components.rs:2273`), evaluated by
  `ludion_scene::clip_eval::{part_pose, clip_phase, sample_channel, track}` and
  `ludion_engine::mesh_assembly` in `extract_at` with sim time as a parameter — proven hash-neutral
  (`dev-docs/assembly-animation.md`). `mesh.wgsl` uses `@group(0)` camera, `@group(1)` the
  clustered light storage buffers, `@group(2)` sun/shadow, `@group(3)` IBL/GI — **all four of the
  WebGPU-guaranteed bind groups**; the mesh instance buffer is 11 `vec4` attributes
  (`render.rs:3670`).
- **The stage view.** `StageSlot::{Model, Effects}` (`editor/app/src-tauri/src/stage.rs:71`); the
  Model slot's watcher covers `<content-root>/voxels/**` only (`watches_voxels` `:85`, `start_watcher`
  `:628`) and is the **only** `asset.reload` caller in the system; `games/vigil/scenes/model-stage.yaml`
  is the neutral stage whose `subject` entity the view swaps by `component.set` of `Mesh.shape.voxel`.
  `views/VoxelArtistView.tsx` composes `editor/voxel/{AssetBrowser (MODEL_EXT '.voxel.yaml', root
  scenes/voxels), StageViewport, orbit, stage, doc (a hand-parsed read for editing), edit (surgical
  in-place scalar rewrite), save}`; `EffectsView.tsx` + `editor/effects/EmitterPanel.tsx` render
  sliders from `x-ui-range` schema annotations (`components.rs:253` is the pattern). The frontend
  writes files through `write_editor_file`. Adding a `MeshSource` variant changes the component JSON
  schema, which `editor/protocol`'s `npm run generate` (the `schema-drift` step) regenerates into
  `protocol.d.ts`; it does **not** touch `ludion_protocol::schema_hash` (the wire schema names no
  component type), so no replay re-records.
- **The verify loop** is `node scripts/check.mjs` (`--rust-only` / `--fix` / `--full`); on the
  laptop every GPU test lane runs serially and the known reds are listed in the machine memory. Docs
  are written with the `ludion-docs` skill; editing `docs/` needs `--fix` to re-mirror
  `embedded-docs/` before `ludion-cli` builds.
- **The craft-doc pattern.** `docs/voxel-craft.md` and `Plan/voxel-artist-playbook.md` are the
  in-repo form of the "persisted corrections" the Blender workflow hand-builds.

## Decisions

1. **Build it; the edge is the loop.** Verified text source of truth, deterministic CPU meshing,
   text checks, the sheet as the photograph, a craft playbook grown from real authoring. Operator
   breadth is explicitly not the target: a dozen operations an agent gets right first time beat a
   thousand it gets wrong.
2. **Driver: vigil props.** The first brief is an idol on a plinth with one emissive element
   (blends, subtraction, mirror, emissive and a hard-edged base in one model). Then a creature at
   rest, then the same creature skinned and walking. The external shmup does not consume this arc.
3. **Representation: one implicit-field document, `kind: sculpt`.** Named primitives with real
   dimensions, combined in file order by `union` (default) / `subtract` / `intersect` with a per-part
   `blend` radius (smooth min; `0` = hard), `mirror` duplication, `repeat` (an arithmetic march), the
   deformers `bend` / `twist` / `taper`, and a seeded surface `noise` op for skin detail (Wave 3).
   Every part is parented to a named bone in a `bones:` tree. **Rejected:** direct mesh ops addressed
   by vertex or face index (the hallucination trap); a polygon construction format (primitives +
   extrude/lathe/loft/subdivision cages) as a second format — subdivision is at most an op, never a
   format; text-driven brush strokes.
4. **The bone tree is in the format from Wave 1** (the user's ruling, weighed against the "no dead
   schema fields" rule): every part carries `bone:` (default `root`), `bones:` lists `{name, parent}`.
   Wave 1 *consumes* it — `L1160` validates unknown bones and cycles, `check-scene` reports per-bone
   bounds — so it is never a dead field; bone positions (`head`/`tail`) arrive additively in Wave 3
   when the overlay and skinning read them. The cost is one key per part; an unrigged prop has one root
   bone; the payoff is that no model is re-authored when skinning lands.
5. **Meshing: deterministic CPU isosurface extraction, at load, no committed derived file.** Naive
   surface nets on a grid at the document's `resolution`, gradient normals, quadric-error decimation
   to the document's `triangles` target, output a `MeshData` keyed by the document's content hash,
   exactly like a voxel model (specification below). **Rejected:** GPU meshing (breaks the bit-exact
   tier); a committed `.mesh.yaml` behind a drift gate — recorded as the fallback if measured load cost
   on a real scene says otherwise (Risks); quad-dominant remeshing / clean edge loops (a recorded later
   trigger, not a wave).
6. **Sharp edges in Wave 1 are a crease-angle normal split on parts marked `sharp: true`**, which
   recovers the shading of an edge, not its exact geometry. A feature-preserving extractor (dual
   contouring, hermite data + a small per-cell QEF solve) is the recorded upgrade the idol's plinth may
   trigger at checkpoint 1.
7. **Tunables of record (settled at checkpoint 1, 2026-09-16, the user's sweep read):** `resolution`
   default 48 cells/unit (48 read visibly better than 32 on the idol's sweep; the ~3× mesh time is
   accepted); `triangles` default 5 000 for a standard prop, 10 000 for a hero (5 000 read clean,
   10 000 slightly better; ~5 000 vertices, under `MESH_VERTEX_BUDGET`); `L1112` still fires above
   10 000 vertices unless `budget_exempt: true`. Both live in `SCULPT_DEFAULT_RESOLUTION` /
   `SCULPT_DEFAULT_TRIANGLES`; the gallery idol keeps its authored 48 / 4 000 (moving it re-blesses
   two goldens on both boxes for no gain).
8. **Material: per-part, from a named `palette:` of the voxel shape, blended by the union weight.**
   A palette entry is the voxel `PaletteEntry` (`color` + optional `emissive`/`metallic`/`roughness`),
   keyed by the same one-glyph rule so `--palette-from` works across voxel and sculpt documents and
   vigil's props share one color language. At a smooth union the surface belongs partly to two parts;
   the material is blended by the same weight the smooth min used, so a glowing vein fades into flesh.
   **Rejected:** hard nearest-part assignment. The same per-part weights are the skinning weights of
   Wave 4 — one definition, two consumers.
9. **Verification ships with the format, Wave 1:** `check-scene` on `kind: sculpt` prints the
   grid extent, cell count, triangles before and after decimation, per-part and per-bone world-unit
   bounds; warns on a disconnected result (`L1159`, the `L1129` shape with a nearest pair) and on a
   part thinner than two cells or contributing no surface vertex (`L1158`, the mesh analogue of the
   wrong-pivot catch: authored and never seen). **On trial** under the "losers deleted" rule: six-view
   ASCII silhouette projections (`--silhouette [cols]`, default 32) — no GPU, exact enough to verify
   "the arm ends at column 12", diffable between edits, cheaper in tokens than one image; it hides
   interior form, so the sheet stays the judge of shading and value. If Wave 1's authoring transcript
   shows it unused, it is deleted. **Trial extended at checkpoint 1 (user, 2026-09-16):** Wave 1's
   transcript used it twice, never decisively — not enough to rule; it stays through the next two
   authorings (Waves 3 and 5) and is judged at checkpoint 2 on those transcripts. **Deleted at
   checkpoint 2 (user, 2026-09-26)**, both authors having voted to delete it: `--bounds` and the sheet
   are the verification surface.
10. **The human surface is the file first**, then the editor stage generalized to sculpt documents
    as soon as the format renders (Wave 2a), an inspector with sliders over every numeric part field
    (Wave 2b, the Effects-view pattern, writing the file so the watcher reloads), the bone overlay
    when skinning arrives (Wave 4). **No new wire methods in this arc. Rejected:** a sculpting gizmo
    that writes back to the document — it would be the first editor-private capability.
11. **Textures are out of this arc.** The user intends a local AI image generator to supply textures
    later. That is a texture-*authoring* surface, which `Plan/textured-pbr.md` decision 1 rejects
    (import-only). **Recorded trigger:** the first generated texture worth applying to a sculpt
    reopens that ruling — the mesher then grows a tri-planar or box-projected `uvs` lane (the mesh
    document already allows it; authoring it now would be a dead field). Nothing in this arc samples a
    texture.
12. **Skinning is a planned wave, not a deferred one** (Wave 4): `MeshVertex` widens once with
    `weights: [f32; 4]` + `bones: [u32; 4]` default-filled (`(1,0,0,0)`, `(0,0,0,0)`) so every
    unrigged mesh renders byte-identically (the textured-PBR widen-once-default-fill pattern); the bone
    palette is a storage buffer on the existing `@group(1)` (all four guaranteed bind groups are taken);
    `kind: animation` clips target bone names through the existing `Animation` component; poses are
    evaluated in `extract_at` and are hash-neutral by the same proof as voxel animation. Weights derive
    deterministically from decision 8's per-part field weights. The rigid fallback if skinning
    disappoints: an assembly of separate sculpt parts posed by the existing evaluator (blends across
    joints lost).
13. **Script rung on trigger only.** A sculpt document is compact per unit of form and `repeat`
    covers the arithmetic march; a `scripts/sculpt/` library + `from-json` verb is added only when an
    agent stops being able to edit a model's document by hand (the voxel decision-11 trigger, applied).
14. **Voxel models are admissible as a source part** (`shape: voxel`, path to a `.voxel.yaml`, the
    field an exact Euclidean distance transform of its occupancy) so existing content gains bevel,
    smoothing and carving. Wave 3, with the deformers. The reverse (voxelizing a sculpt for GI) already
    exists on the mesh path.
15. **glTF import stays for external assets; no export.** A round-trip to Blender breaks the single
    source of truth the moment a human edits the exported copy.
16. **Frontier model first.** The craft playbook is written for ourselves; a small local model
    following it is the idea noted in `Plan/voxel-artist-playbook.md`, not this arc's.
17. **The convergence metric is logged from Wave 1.** Every model authored in the arc gets one line
    in `Plan/artifacts/mesh-modeling-log.md`: model, wave, attempts (sheet renders before acceptance),
    tokens (from the transcript), sitting verdict, plus the provenance columns Kiln records per example
    (decision 20): authoring model and harness, source access (what examples/docs the author saw),
    human intervention, review fidelity. The Blender baseline to beat is fifteen attempts and ~2 M
    tokens per mesh.
18. **Three checkpoints, each a recorded decision, each able to end the arc:** the idol judged on the
    sheet and the stage; the creature at rest; the skinned creature walking under a clip. The
    comparison on quality and tokens (the same brief authored blind through sculpt, voxel and Kiln —
    decision 20 — with Blender MCP if a box with Blender is at hand) runs after whichever checkpoint
    the user declares satisfying. If skinned characters cannot be
    generated, that conclusion is written into this Status with its evidence: "Blender is the way".
19. **Naming:** `kind: sculpt`, `*.sculpt.yaml` under `sculpts/` (a content-root sibling of
    `voxels/`), `Mesh.shape: { sculpt: … }`, the `ludion sculpt` verb family, the `ludion-sculpt`
    crate. **Rejected:** `model` (already the harness's word for any authored document), `sdf`/`form`
    (date the format to its implementation).
20. **Kiln (2026-09-14, the user's ruling): not integrated, not adopted as a side tool; it is the
    comparison baseline of decision 18.** Kiln (MIT, TypeScript/Bun over three.js + Manifold + glTF
    export) has the agent write a JavaScript `build()` of named parts, runs structural QA, renders a
    six-view CPU sheet and edits saved revisions through an MCP surface. Rejected as a component or
    format: a JavaScript program is a second Turing-complete layer with no canonical serialization,
    JSON-pointer addressing or schema; it is a second engine (its own renderer, CSG and MCP) beside
    the wire; its output is a derived GLB whose source lives outside this repo's loop. Rejected as a
    side tool for vigil: its gallery is hard-surface, which the voxel path already covers, it has no
    skinning, and its own experiments keep implicit fields experimental. Nothing in it falsifies the
    bet on a closed implicit vocabulary for organic form; its one organic success (a swept tube of
    hand-typed stations with a seeded sine hash for skin) supports capsule chains and Wave 3's
    `noise`. Borrowed: part-framed close-ups (Wave 1 step 5), the log's provenance columns
    (decision 17), and connectivity exemption by declared role, never by inferred shape (the `L1159`
    rule when `shape: voxel` and card-like parts arrive in Wave 3). Checkout: the user's local
    clone; the survey is this decision, not a doc.
21. **Hard-surface is in scope; crisp hard-surface rides a recorded trigger (2026-09-14, the user's
    ruling).** Games that reject voxels for their visual identity need agent-authored hard-surface
    models from the same tool. Wave 1 already covers the stylized form: exact dimensions, carving by
    `subtract`/`intersect`, `repeat`/`mirror`, and flat faces decimating to a few triangles.
    What it lacks is crisp edges — surface nets round every edge to about one cell and decision 6's
    crease split fixes the shading, not the silhouette — and profile primitives, the workhorse of
    Kiln's hard-surface gallery. **Trigger:** the first game brief that needs crisp hard-surface
    schedules one wave of its own: the feature-preserving extractor (dual contouring, hermite data +
    per-cell QEF, decision 6's recorded upgrade) and `extrude` / `revolve` of a 2D polyline as
    implicit primitives (exact 2D polygon distance; a profile *primitive* inside the field, not the
    polygon *format* decision 3 rejects). The idol's plinth at checkpoint 1 is the early signal: if
    its edges read rounded on the sheet, the trigger has half-fired. Hard chamfers are not planned;
    blends give fillets and stylized games rarely miss the difference. **Checkpoint-1 ruling (user,
    2026-09-16): the plinth is correct and acceptable at 48/unit, and could be better — the
    improvement is planned, not triggered by a brief.** Wave 6 below holds it; it is scheduled on the
    user's word, after checkpoint 2, never ahead of the creature.

## The document

Canonical key order (what `emit_sculpt` writes, block style like every other kind — the flow-style
sketch below is abbreviated for reading): `format`, `kind`, `name`, `resolution`, `triangles`,
`budget_exempt`, `palette`, `bones`, `parts`.

```yaml
format: 1
kind: sculpt
name: idol
resolution: 32          # cells per world unit; integer 4..=128
triangles: 4000         # decimation target; integer >= 12
palette:                # the voxel palette shape, one-glyph keys, sorted on emit
  S: { color: [0.35, 0.33, 0.3, 1.0], roughness: 0.85 }
  E: { color: [0.2, 1.0, 0.6, 1.0], emissive: 4.0 }
bones:                  # authored order; the first entry is the root and has no parent
  - name: root
  - { name: head, parent: root }
parts:                  # evaluated in file order
  - { name: plinth, shape: box, size: [1.2, 0.3, 1.2], at: [0, 0.15, 0], round: 0.02,
      material: S, sharp: true }
  - { name: body, shape: capsule, radius: 0.25, from: [0, 0.3, 0], to: [0, 1.1, 0],
      material: S, blend: 0.1 }
  - { name: head, shape: ellipsoid, radii: [0.22, 0.18, 0.2], at: [0, 1.25, 0],
      material: S, blend: 0.08, bone: head }
  - { name: eye, shape: sphere, radius: 0.05, at: [-0.09, 1.28, 0.16], material: E,
      blend: 0.02, mirror: x, bone: head }
  - { name: mouth, shape: ellipsoid, radii: [0.1, 0.03, 0.08], at: [0, 1.18, 0.18],
      op: subtract, blend: 0.02, bone: head }
  - { name: spine, shape: cone, radius: 0.04, height: 0.12, at: [0, 0.5, -0.24],
      repeat: { count: 5, step: [0, 0.12, 0] }, blend: 0.03 }
```

**Part keys (Wave 1).** `name` (required, unique, `L1157` on a duplicate), `shape` (required: `sphere`
`radius`; `capsule` `radius` + `from` + `to`; `ellipsoid` `radii`; `box` `size` + optional `round`;
`torus` `radius` + `tube`; `cone` `radius` + `height`, apex up), `at` (center, default `[0,0,0]`;
`capsule` uses `from`/`to` instead), `rotation` (Euler XYZ degrees, default none), `op` (`union` |
`subtract` | `intersect`, default `union`), `blend` (>= 0, default `0`), `mirror` (`x` | `y` | `z`;
duplicates the part across the model's origin plane *before* its op runs, so a subtract mirrors too),
`repeat` (`count` >= 2, `step` `[x,y,z]`; copies the part `count` times translating by `step`, all
copies one op), `material` (a palette glyph, required unless the part is a `subtract`), `bone` (a
`bones:` name, default the root), `sharp` (bool, default false). A key that does not belong to the
shape (a `radii` on a `box`) is `L1157` with the shape's key list. Wave 3 adds `deform`, `noise`,
`shape: voxel` and bone `head`/`tail`, each consumed on landing.

**Semantics.** Model space is world units at `Transform.scale: 1`, origin bottom-center like a voxel
model: the meshed surface is translated so its bounding box's minimum `y` is `0` and its `x`/`z`
extents are centered — *after* meshing, so a part authored below `y = 0` is not an error (the plinth's
`at` in the sketch is simply convenient). Parts fold left to right: the running field `F` starts at
`+inf`; `union` is `smin(F, p, blend)`, `subtract` is `smax(F, -p, blend)`, `intersect` is
`smax(F, p, blend)`, with `blend = 0` the exact `min`/`max`. Materials: each `union` part contributes
its palette entry with the smooth-min weight `h` (below); a `subtract`/`intersect` contributes no
material but re-weights the survivors. Every numeric field is a `--set`/`--sweep` pointer
(`/parts/<i>/<key>`, `/resolution`, `/triangles`, `/palette/<glyph>/<field>`) and a slider in Wave 2b.

## The mesher (`ludion-sculpt`)

All arithmetic is `f32` after parsing (the document is `f64` text like every kind), transcendentals
only through `ludion_core::math`, no hashed-iteration-order container anywhere, every loop in a fixed
`z, y, x` order, every tie broken on an explicit key. The output is a pure function of the document
bytes and must hash identically on every machine — the conformance test in Wave 1 step 2 is the proof.

1. **Field.** Exact signed distances for the six primitives (`box` with `round` = the rounded box;
   `capsule` from segment distance; `ellipsoid` by the standard bound-and-refine approximation, its
   sign exact; `cone` and `torus` exact). Transforms apply the inverse rotation to the sample point.
   Smooth min is **polynomial** (no `exp`): `h = clamp(0.5 + 0.5 (b - a) / k, 0, 1)`,
   `smin = mix(b, a, h) - k h (1 - h)`; `smax(a, b, k) = -smin(-a, -b, k)`. `h` is the running
   field's share, so `1 - h` is the material / skinning weight of the incoming part; with `k = 0` the
   weight is `1` or `0`. A cone's `at` is the centre of its base disc (apex at `at + (0, height, 0)`).
2. **Grid.** Extent = the union of every part's conservative bounding box (mirror and repeat copies
   included) padded by `max(blend) + 2 cells`; spacing `1 / resolution`; the corner lattice is
   `(nx+1)(ny+1)(nz+1)` samples, evaluated once each into a `Vec<f32>` (a `Vec<[f32; N]>` of part
   weights alongside, `N` = part count, per corner, for material blending — the memory is the
   product; a hero at 64/unit over 2 u is 2.1 M corners × parts, so weights are stored per *surface
   cell* only, computed on the second pass).
3. **Surface nets.** For every cell whose eight corners are not all one sign, the vertex is the mean
   of the sign-change points along its twelve edges (linear interpolation); vertex index = order of
   discovery in `z, y, x`. For every lattice edge with a sign change, the quad joins the four cells
   sharing that edge, wound so the normal follows the field gradient; each quad splits along its
   shorter diagonal, ties to the `(0, 2)` diagonal. Result: a closed triangle mesh whenever the field
   is finite; smooth fields are also manifold (asserted in tests: every edge shared by exactly two
   triangles), but a hard (`blend: 0`) crease can leave an ambiguous cell whose edge four triangles
   share (measured in Wave 1).
4. **Normals.** The field gradient by central differences at the vertex, normalized, is the smooth
   normal. For triangles whose three vertices all carry a `sharp` part as their dominant weight, the
   normal is split by crease angle (60°, one named constant) into per-triangle faceted normals —
   vertices are duplicated at the split, the only place the vertex count grows.
5. **Decimation.** Garland–Heckbert quadric error metrics on edges, a binary heap keyed on
   `(error.to_bits() as ordered, edge index)`, collapsing to the optimal position (falling back to
   the midpoint when the quadric is singular, tested by determinant threshold), refusing a collapse
   that flips a face normal, until the triangle count is `<= triangles`. Materials and weights at the
   surviving vertex are the collapse's position-weighted blend. Normals are recomputed from the
   field gradient after decimation (so they stay smooth), then the crease split re-runs.
6. **Output.** `SculptMesh { positions, normals, colors, materials, indices, weights, stats }` →
   `MeshData` with `authored_material: true`. `SculptStats { extent, cells, surface_cells,
   triangles_before, triangles_after, vertices, components, parts: Vec<PartStats { name, bounds,
   surface_vertices }>, bones: Vec<BoneStats { name, bounds }> }` is what `check-scene` prints.
7. **Silhouettes.** Six orthographic projections (`+X -X +Y -Y +Z -Z`, the exposure report's order)
   of the decimated mesh rasterized into a `cols × rows` character grid (rows keep the model's aspect,
   cell = `#` covered / `.` empty, a ruler line every 8 columns), the same conservative triangle
   rasterizer for every view. Printed by `check-scene --silhouette [cols]`, default 32.

Cost model to measure in Wave 1 step 9: the field pass is `corners × parts` distance evaluations; the
idol at 32/unit over ~1.5 u is ~110 k corners × ~8 parts — milliseconds. Decimation is
`O(E log E)`. Both numbers are printed by `check-scene` (`meshed in N ms`) so the load-cost trigger of
decision 5 is read from every run.

## Waves

### Wave 1 — the format, the mesher, the checks, the sheet, the idol (checkpoint 1)

**Coordination.** `components.rs`, `extract.rs`, `bounds.rs`, `clip_eval.rs` and `atlas.rs` are
shared with the projectile-patterns and sprite-engine instances; claim by message before editing
(the machine memory's rule) and expect transient breakage from their half-landed edits.

1. **Codes.** `crates/ludion-core/src/codes.rs`: `SCULPT_DOCUMENT_STRUCTURE` `L1156`,
   `SCULPT_DOCUMENT_DATA` `L1157`, `SCULPT_PART_UNRESOLVED` `L1158`, `SCULPT_DISCONNECTED` `L1159`,
   `SCULPT_BONE_TREE` `L1160`, each with the documented what/where/why/fix paragraph the table's
   entries carry; `ludion explain L1156`…`L1160` must print (the `explain` smoke step).
2. **`ludion-scene/src/sculpt.rs`** (the `voxel.rs` sibling; `mesh.rs` for the smaller shape).
   `SculptDocument { name: Option<String>, resolution: u32, triangles: u32, budget_exempt:
   Option<bool>, palette: Vec<PaletteEntry>, bones: Vec<SculptBone { name, parent: Option<String> }>,
   parts: Vec<SculptPart> }`, `SculptPart` with `shape: SculptShape` (an enum carrying each shape's
   own fields), `op: SculptOp`, and the optional keys above. `parse_sculpt` (structure → `L1156`),
   `validate_sculpt` (data → `L1157`: finite floats, `resolution` 4..=128, `triangles` >= 12, unique
   part names, palette keys one glyph, `material` naming a palette glyph, `count` >= 2, `blend` >= 0;
   bones → `L1160`: unique names, every `parent` and every part `bone` naming a bone, exactly one root,
   no cycle), `check_sculpt` = parse + validate, `emit_sculpt` (canonical order above, ryu floats,
   byte-idempotent: `emit(parse(emit(doc))) == emit(doc)` tested). Re-export from `lib.rs` beside
   the voxel exports; `PaletteEntry` is reused as-is; `apply_palette_overrides` gains a sculpt twin
   (`apply_sculpt_overrides` covering palette *and* part pointers, so `--set /parts/2/blend=0.2`
   works — this is where `L4008` is raised for a pointer that is not a numeric field). Tests in the
   module: every diagnostic code with its JSON-pointer path, the round trip, the `L1157` unknown-key
   nearest-match message.
3. **`crates/ludion-sculpt`** (new; copy `ludion-sprite`'s `Cargo.toml` metadata and `clippy.toml`;
   deps `ludion-core`, `ludion-scene`; `#![forbid(unsafe_code)]`, `#![warn(missing_docs)]`). Modules
   `field.rs`, `grid.rs`, `surface_nets.rs`, `normals.rs`, `decimate.rs`, `material.rs`,
   `silhouette.rs`, `lib.rs` (`pub fn mesh(doc: &SculptDocument) -> Result<SculptMesh, Diagnostic>`,
   `pub fn silhouettes(mesh: &SculptMesh, cols: u32) -> [String; 6]`, `pub fn connectivity(mesh) ->
   (u32, Option<Diagnostic>)` with the `L1159` nearest-pair report computed on vertex positions). Tests:
   each primitive's distance at authored points; `smin` symmetric and equal to `min` at `k = 0`; a
   unit sphere at 32/unit meshes closed (every edge shared twice), with surface area within 2 % of
   `4π` before decimation and every vertex within one cell of the analytic surface; a `subtract` removes
   the carved volume; `mirror` produces a mesh symmetric to the cell; decimation reaches the target
   and keeps the mesh closed; **the conformance hash**: `tests/fixtures/idol.sculpt.yaml` meshes to a
   committed XXH3 of its `MeshData` bytes (`ludion_assets::content_hash` over the concatenated flat
   lists). The hash constant is landed from the authoring machine and **verified on both pinned boxes
   before the first bless** — a disagreement is a determinism bug to fix in the mesher, never a per-OS
   constant.
4. **Engine.** `components.rs`: `MeshSource::Sculpt(String)` with the doc comment and the round-trip
   test (`:2986` block). `extract.rs`: `PlacedMeshSource::Sculpt`, `SceneAssets::sculpt`, the
   `DiskAssets` and `CachedAssets` arms, the `:723-746` resolution arm. New `sculpt_mesh.rs`:
   `load(root, rel) -> Result<MeshData, Vec<Diagnostic>>` (bytes → `check_sculpt` → `ludion_sculpt::
   mesh` → `MeshData` with `hash = content_hash(bytes)`), `sculpt_stats(doc)`; the `L1112` vertex
   budget re-checked on the meshed count unless `budget_exempt`. `asset_cache.rs`: a sculpt entry and
   the `.sculpt.yaml` reload branch (`:407-478` shape). `bounds.rs:206`: `Sculpt` takes the voxel box.
   `voxelizer.rs:304`: `Sculpt` joins the `Asset`/`Assembly` arm. `docs.rs::ENGINE_KINDS`: add
   `sculpt` (or the data-doc walk rejects the gallery folder with `L1119`). `model_sheet.rs`:
   `ModelKind::Sculpt` in the `:1260` dispatch, bounds from the meshed data, the `:1316-1337` error
   texts list the fourth kind, palette/part overrides through `apply_sculpt_overrides` at `:1866`;
   `sweep.rs` needs no change if the override seam is the document. `check-scene` on a **scene**
   already validates a `Mesh.shape` path through the loader; confirm the `sculpt` arm reports its
   diagnostics re-based onto `/entities/<i>/components/Mesh/shape/sculpt`.
5. **CLI.** `check_scene.rs`: route `sculpt` at `:74-89` to `run_sculpt(path, source, views)`;
   `SculptViews { silhouette: Option<u32>, bounds: bool }` (new flags `--silhouette [N]` and
   `--bounds`, refused on other kinds like the voxel flags at `:102`); print `L1158`/`L1159` warnings,
   then the views, then
   `<path>: OK (sculpt, P parts, B bones, grid AxBxC @ R/unit, T triangles after decimation from T0,
   V vertices, meshed in N ms)`. `main.rs`: `Command::Sculpt { command: SculptCommand::New { out,
   --name, --palette-from } }` → `sculpt.rs::run_new` (scaffold: one root bone, an empty `parts:`,
   the header comment; never overwrites); `voxel::run_new`'s `--palette-from` accepts a sculpt
   document (dispatch on `peek_kind`). `render --model` gains `--part <name>` (and `--isolate`):
   the sheet's six views framed on that part's meshed bounds, the rest of the model kept (or hidden
   with `--isolate`) — one image answers "does the eye sit in the socket" (decision 20; `L4008`
   for an unknown part name). `render.rs` module docs name the fourth model kind.
6. **Goldens and content.** `examples/gallery/sculpts/idol.sculpt.yaml` — **authored through the
   loop by the wave's instance as the experiment itself**: write, `check-scene`, `render --model`,
   look, repeat; count every sheet render; the log line is written at acceptance. `examples/gallery/
   sculpt-idol.yaml` (a scene like `voxel-model.yaml`: ground plane, key + fill, the idol at
   `Transform.scale: 1`) → `goldens/<os>/sculpt-idol.png`; `idol_sheet_test()` in
   `crates/ludion-testkit/tests/gallery.rs` → `goldens/<os>/idol-sheet.png`. Both first-blessed on
   the pinned GPUs (owed, not done here); until then they report as the particles precedent does.
7. **Schema drift.** `cd editor/protocol && npm run generate` to refresh `protocol.d.ts` for the
   new `MeshSource` variant (the `schema-drift` step); the inspector's `Mesh` shape select gains
   `sculpt` with no frontend code.
8. **Docs** (the `ludion-docs` skill; `node scripts/check.mjs --fix` re-mirrors). New:
   `docs/sculpt-format.md` (the document, every key, the semantics section above),
   `docs/sculpt-authoring.md` (the loop: new → check → sheet → look; the text views; gotchas),
   `docs/sculpt-craft.md` (**written from the idol transcript**: what went wrong, what fixed it —
   the persisted-corrections doc, the arc's answer to the Blender playbook),
   `dev-docs/ludion-sculpt.md` (layout, rules, the determinism obligations, the cost model). Rows:
   `docs/components.md` (`Mesh.shape: sculpt`), `docs/visuals.md`, `docs/voxel-models.md` (a
   pointer: "smooth organic forms are `sculpt-format`"), `docs/scene-format.md` (the kind list),
   `dev-docs/scene-kind-documents.md`, `dev-docs/engine-content-kinds.md`, `dev-docs/crate-graph.md`,
   `dev-docs/ludion-cli.md`, `dev-docs/ludion-engine.md` (the loader). Root `CLAUDE.md`: the CLI
   list gains `sculpt`; the Current-state paragraph at wave close.
9. **Verify.** `node scripts/check.mjs --rust-only` then the full suite; on the laptop the GPU lanes
   serially (`LUDION_QUALITY` unset); `cargo test -p ludion-sculpt`; the idol's `check-scene` timing
   line recorded in the log; the conformance hash on both pinned boxes.
10. **Exit (checkpoint 1).** The idol's sitting: the sheet and the stage judged; the resolution/
    triangles sweep read and decision 7's defaults confirmed or moved; the plinth ruling on decision
    6; the silhouette trial ruled on from the transcript (kept or deleted in the same wave). Verdict
    appended to Status as a decision; log line complete; notebook pruned.

### Wave 2 — the Models view (no wire movement)

1. **2a, the stage.** `stage.rs`: `watches_voxels` → `watches_models`, `start_watcher` watching
   `<root>/voxels` **and** `<root>/sculpts` (each optional); tests at `:1147` follow. The Model slot's
   `model-stage.yaml` is unchanged — the view swaps the subject by `component.set` of
   `Mesh.shape.sculpt` exactly as the Voxel Artist sets `Mesh.shape.voxel`. `views/ModelsView.tsx`
   composes `StageViewport`, `orbit`, `stage` and an `AssetBrowser` parametrized by `MODEL_EXT`
   (`.sculpt.yaml`, root `scenes/sculpts`) — the browser takes the extension and root as props so
   the two views share one component. No slice painter, no palette editor. Route it in the app's view
   switch beside the Voxel Artist.
2. **2b, the inspector.** `editor/sculpt/doc.ts` (a hand-parsed read of the strict block-style
   document into `{ pointer, value, line, column }` numeric fields — the `voxel/doc.ts` precedent),
   `editor/sculpt/edit.ts` (rewrite one scalar in place at its source position, ryu-shortest text —
   the `voxel/edit.ts` precedent; the file is otherwise byte-preserved), `editor/sculpt/ranges.ts`
   (the slider range per key: coordinates ±2× the model's extent, radii/size 0..2× their value,
   `blend` 0..0.5, `round` 0..0.2, `resolution` 4..128 integer, `triangles` 500..20000 integer, palette
   color 0..1, `emissive` 0..8, `roughness`/`metallic` 0..1), `SculptInspector.tsx` (one slider per
   field, grouped by part in file order, the `EmitterPanel` look). A slider change → `edit.ts` →
   `write_editor_file` → the watcher's `asset.reload` → the stage re-meshes. Vitest: `doc.ts` finds
   every numeric field of the idol with its position; `edit.ts` round-trips a value and leaves every
   other byte intact; `ranges.ts` is total over the key set.
3. **Docs.** `Plan/editor-views.md` gains the view's row; `docs/dev-loop.md` or the editor topic
   names the Models view beside the Voxel Artist.
4. **Verify.** `node scripts/check.mjs` (the `tauri-*`, `oxlint`, `tsc`, `vitest` lanes).
5. **Exit.** The next editor sitting's live smoke: open the idol, drag a `blend`, watch the reload.

### Wave 3 — the creature at rest (checkpoint 2)

1. **Format additions**, each consumed on landing: bones gain optional `head`/`tail` `[x, y, z]`
   (validated finite, `L1160` when only one of the pair is authored; consumed by the `--bounds`
   report and, in Wave 4, the overlay and the pose); parts gain `deform: { bend: { axis, degrees },
   twist: { axis, degrees }, taper: { axis, from, to } }` (applied to the sample point before the
   primitive's distance, the standard domain deformations, each bounding-box-conservative by the
   Lipschitz bound noted in the field module), `noise: { amplitude, scale, seed }` (a seeded
   value-noise displacement of the part's distance, deterministic through `ludion_core::math` and a
   splitmix64 hash of the lattice point — the sprite crate's per-texel seeding precedent), and
   `shape: voxel` with `path` + `size` (units per voxel; the field is an exact Euclidean distance
   transform — Meijster's integer algorithm, deterministic — of the model's occupancy, signed by
   inside/outside, sampled trilinearly). `mirror` on a `shape: voxel` part is legal.
2. **The armature convention** in `docs/sculpt-craft.md`: one capsule chain per limb, one bone per
   capsule, joints at the blend, `head`/`tail` on the capsule's `from`/`to`; the noise op for skin.
3. **Content.** `examples/gallery/sculpts/creature.sculpt.yaml` (a vigil creature body at rest,
   authored through the loop, logged), `creature_sheet_test()` → `goldens/<os>/creature-sheet.png`.
   The silhouette verdict of checkpoint 1 is applied (flag kept or gone).
4. **Docs.** `sculpt-format` (the new keys), `sculpt-craft` (the convention), `dev-docs/
   ludion-sculpt` (the distance transform and the deformer bounds).
5. **Exit (checkpoint 2).** The sitting; verdict as a decision; log line. This is the checkpoint at
   which "the representation cannot carry a creature" would end the arc before any renderer work.

### Wave 4 — skinning (checkpoint 3)

1. **Renderer.** `MeshVertex` gains `weights: [f32; 4]` + `bones: [u32; 4]` (default `[1,0,0,0]` /
   `[0,0,0,0]`), `MeshData` gains `weights: Vec<f32>` (4 per vertex) + `bone_indices: Vec<u32>` (always
   populated, the `colors` rule); the mesh pipeline's vertex layout grows two attributes; `mesh.wgsl`
   skins `position` and `normal` by a bone palette read from a new storage-buffer binding on
   `@group(1)` (WebGPU guarantees four bind groups and groups 0–3 are taken; verify `Gpu`'s requested
   limits before choosing otherwise), indexed by a per-instance `bone_offset` carried in a spare
   instance lane (`render.rs:3670` has 11 vec4 attributes — audit for a free component before adding
   a twelfth). An unrigged instance has offset `0` into an identity palette entry, so **every existing
   golden renders byte-identically** — asserted by the gallery suite before anything else lands.
2. **Engine.** `ludion-sculpt`: per-vertex bone weights = the per-part weights of decision 8 summed
   per bone, top-4 by `(weight, bone index)`, normalized; `SculptDocument` gains `clips:` (the
   assembly `clips:` map: short name → `kind: animation` path, tracks keyed by **bone** name);
   `sculpt_mesh.rs` loads the clips like `assembly_mesh::load_documents`; `extract_at` builds the
   bone palette per instance from the `Animation` component (`clip`, `started`, `speed` — reused
   unchanged) through `clip_eval::{clip_phase, sample_channel, track}` down the bone tree
   (`T(head) · R · T(-head)` per bone, parent-composed, the assembly evaluator's shape), rest pose
   when no clip. Hash-neutrality proof: the voxel-animation test shape (an animated run's `WorldHash`
   sequence and `.lrp` bytes identical to a never-extracted run's). `render --model --clip/--time/
   --strip` accept a sculpt (the `:1405` error text drops "only an assembly").
3. **Editor.** The bone overlay in `StageViewport` for the Models view: `head`→`tail` lines drawn from
   the document (the frontend already has the doc in `doc.ts`), toggled by a button; no wire.
4. **Content.** `examples/gallery/sculpts/anims/creature-walk.animation.yaml`, the creature's
   `clips: { walk }`, `creature_walk_strip_test()` → `goldens/<os>/creature-walk-strip.png`.
5. **Docs.** `docs/sculpt-animation.md` (clips on bones, the weight derivation, what deforms badly
   and why), `dev-docs/renderer-shading.md` or `renderer-passes.md` row for the skinning term,
   `dev-docs/assembly-animation.md` cross-pointer, `docs/components.md` (`Animation` on a sculpt).
6. **Exit (checkpoint 3).** The sitting: the creature walking. Verdict as a decision. If it fails,
   the rigid fallback of decision 12 is offered and "Blender is the way" is written into Status with
   the evidence.

### Wave 5 — the comparison and the placement

1. **The brief**, written once and sealed before any authoring: one prop or creature, its size, its
   palette, its emissive, the three views it must read from.
2. **Three blind authorings**: sculpt (a fresh instance with only the docs), voxel (a fresh instance),
   Blender MCP (the user's setup — **user-gated**: Blender and an MCP server installed by the user; a
   fresh instance drives it). Each logs attempts and tokens; the user judges the three sheets and
   scenes blind. Verdict as a decision in Status, with the numbers in the log.
3. **Placement.** The winning content enters vigil: `games/vigil/scenes/sculpts/<name>.sculpt.yaml`,
   placed by the district's map/kit or scene; the model path is hashed `Mesh` bytes, so `run.lrp`
   re-records by the machine memory's protocol (`vigil --replay-verify` on a copy under `scenes/`,
   `cargo test -p ludion-protocol` after), the district golden re-blesses with its diff report at
   `Plan/artifacts/mesh-modeling-wave5-district-<name>.diff.json`, and `stress.lrp` plus the segment
   replays are confirmed byte-identical or re-recorded with `L1126` clean.
4. **Close.** Status collapses the wave log; the notebook shrinks to its close form; root `CLAUDE.md`
   Current state; decisions that outlive the feature (the format, the tunables) move to the docs.

### Wave 6 — crisp hard-surface (planned at checkpoint 1, scheduled on the user's word after checkpoint 2)

Decision 21's content, promoted from a brief-triggered wave to a planned one by the checkpoint-1
plinth ruling. Scope, unchanged from the decision: (1) a feature-preserving extractor — dual
contouring over the same grid, hermite data at edge crossings, a per-cell QEF solve with the singular
fallback decimation already uses, selected per document by `extract: dual` (surface nets stays the
default; the conformance hash is per extractor, so the idol's never moves); (2) `extrude` and
`revolve` of a 2D polyline as implicit primitives (exact 2D polygon distance, a profile *primitive*
inside the field — not the polygon format decision 3 rejects); (3) decision 6's crease split kept as
the shading half. Exit: the idol's plinth re-rendered under `extract: dual` reads crisp at the default
resolution with no visible bevel, one hard-surface prop authored blind through the docs, and the
craft doc's edge paragraph rewritten from that transcript. Nothing here moves a golden until a
document opts in.

## Engine/game boundary

Everything above is engine and tooling. Vigil consumes a sculpt exactly as it consumes a voxel model
(a `Mesh` on an entity); no game system changes until Wave 5's placement.

## Deferred / non-goals

Textures and UVs (decision 11's trigger); feature-preserving extraction (decision 6's trigger);
compile-to-mesh + drift gate (decision 5's fallback, on measured load cost); the script rung
(decision 13); clean topology / quad remeshing; the local-model path; the sculpting gizmo, polygon
construction, direct vertex ops, GPU meshing, export (all rejected). Runtime re-meshing per frame
(a pose is never re-meshed; skinning deforms the rest mesh). `ludion sculpt apply` (an ops file over a
sculpt document): a sculpt document *is* its op list, so the verb would be a second way to say the
same thing — revisit only if the Wave 2 inspector proves insufficient for batch edits.

## Risks / open questions

- **Load cost.** A hero at 64/unit over 2 u is 2.1 M lattice corners × every part; if a real scene
  measures seconds of meshing, decision 5's fallback fires. The `meshed in N ms` line on every
  `check-scene` run is the instrument; the idol's number goes in the log.
- **Cross-OS bit identity of the decimation.** `f32` through libm and an explicitly keyed heap
  should hold; the conformance test is the proof and runs on both pinned boxes before the first
  bless.
- **Bind-group ceiling for skinning.** Groups 0–3 are taken and WebGPU guarantees four; Wave 4's
  palette goes on `@group(1)` unless the requested limits already exceed the floor.
- **Thin features versus resolution.** `L1158` catches what the grid cannot resolve; the craft doc
  must teach authoring at the grid's scale.
- **Blended material versus blended weights.** One definition serves both (decisions 8/12); a joint
  that looks right may still deform badly — checkpoint 3 is where that is judged.

## Reserved diagnostic codes

`L1156` sculpt structure, `L1157` sculpt data consistency, `L1158` unresolvable part (warning),
`L1159` disconnected result (warning), `L1160` bone tree (unknown bone, cycle, half a head/tail pair).
## Artifacts

`Plan/artifacts/mesh-modeling-log.md` — the per-model attempts / tokens / verdict log (decision 17),
created with the idol's line in Wave 1: a table with columns `model | wave | attempts | tokens |
meshed ms | verdict`, one row per model, never rewritten.

## Docs owed (by wave)

W1: `sculpt-format`, `sculpt-authoring`, `sculpt-craft`, `dev-docs/ludion-sculpt` + the nine rows in
Wave 1 step 8. W2: the editor rows. W3: the format and craft additions. W4: `sculpt-animation` + the
renderer rows. W5: the close.
