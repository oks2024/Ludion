# Plan: Skeletal animation — skinned glTF meshes whose bones follow rigid bodies

> Status: **DESIGN SETTLED (2026-09-26, grilling session; every ruling is the user's, except the
> component shape of decision 5, which this plan derives and records with its reasons).**
> **W1 mechanized (2026-09-27):** the mesh document's `joints`/`weights`/`bone_indices` keys
> (`L1190` structure, `L1191` data), `ludion import`'s skin mapping (`crates/ludion-import/src/skin.rs`;
> zero-weight vertices `L1205`; beyond the plan, a mesh shared by skinned nodes and a node, mesh,
> camera or light on a joint are `L1202`), `MeshSkin::rest_model_matrices` (W3's spawn pose reuses
> it), and `check-scene <mesh> --bounds [--json]`'s joint tree. The tube and mannequin fixtures come
> from the Blender pipeline; the mannequin imports clean. **W1 exit met (2026-09-29):** the external game's goblin v2, exported by
> `generation/blender/goblin/scripts/v2/skin_export_v2.py` (the external game's repo) with its own blended weights
> (up to 4 influences), imports clean — 11 joints named after the external game's parts, `check-scene --bounds` OK;
> the glb is derived, not committed.
> **W2 mechanized (2026-09-27):** the GPU bone palette — `MeshVertex` gains `weights`/`bone_indices`
> (locations 18–19, `max_vertex_attributes` 18→20), `MeshData` carries them plus a per-asset
> `rest_palette: Vec<Mat4>` (`mesh_asset::decode`, one matrix per joint, `rest_model × inverse_bind`),
> a `@group(0) @binding(1) bone_palette` storage buffer is read by shadow, prepass and scene shaders
> (`skin.wgsl`), and a per-instance `bone_offset` rides the spare `fade.z` lane. `extract_at` appends
> each skinned instance's own copy of its asset's rest palette and offsets it past the renderer's
> prepended identity (entry 0); an unskinned instance stays at offset 0, which takes the unskinned
> arithmetic textually. Proven by a CPU test that the mannequin's rest palette is identity within
> `1e-5` and a GPU render of the mannequin skinned at rest against its skin-stripped copy (SSIM 1.0,
> threshold 0.999) with a negative control (a nudged inverse bind fails); no golden moved. W4 not
> started. The driver is the external game's goblin (the external launcher's request `0002-ragdoll-skinning`,
> priority "painful"). The meshes come from `Plan/asset-pipeline-blender.md`, which runs in parallel.
> **W3 mechanized (2026-09-27):** `SkinBone { skin, bone }` links a rigid part to one joint of a
> skinned mesh entity (registered last in the pinned registry order); at scene load a part with no
> authored `Transform` spawns at the skin's world transform times the bone's rest model transform, and
> `L1194`–`L1197` refuse a bad link (an unknown skin is the generic `L1017`; a skin with no skinned mesh, an unknown bone, two
> parts naming one bone, `Animation` on a skin that parts name), with `check-scene --bounds` printing
> each joint's head in its parent's frame so an anchor is a copy job. `extract.rs::posed_palette`
> builds each skinned instance's palette from its parts' current world poses (a followed joint's entry
> is `skin_model⁻¹ × part_world × inverse_bind`; an unfollowed joint composes its rest local transform
> onto its posed parent), proven hash-neutral against a never-extracted run of the same session
> (`tests/skin_bone.rs`). The hand-authored gallery scene `examples/gallery/ragdoll-skinned.yaml` (the
> W1 mannequin, 11 `SkinBone` parts, 10 `Joint`s, over a small staircase) carries a trace, a `.lrp`,
> the thread-identity and CLI sim-hash twins, and a mesh-asset import-drift gate; every
> scene-instantiating render path (`Engine` loads, district switches, `ludion render`, the discovery
> goldens, `dump-graph`, `render --sweep`) applies the same `SkinBone` spawn pose through one shared
> helper, so the scene needs one golden, `ragdoll-skinned` at frame 150 (a manifest `frames`
> override) through the bare path — mid-tumble, physics-driven, not the authored A-pose. Decision
> 8 records automatic anchor derivation as not built. No wire, golden (other than the one new one), or
> schema-hash change.
>
> **Sockets (Clockout 0028, 2026-11-01):** `ParentJoint { joint }` attaches an entity to a joint of its
> skinned parent. Sim reads compose the joint's rest pose (no hash depends on clip data); `extract` draws
> the posed joint, and `posed_joint_world` returns it to presentation code (`L0028` from sim). The
> `character-socket` gallery golden proves three run phases. Deferred, each with its trigger: a `Mat4`
> variant of `posed_joint_world` for sheared poses (a sheared socket); a cycle (a part under
> a skin's own socket that drives that skin) settles after a capped rerun, deterministically;
> `posed_joint_world` on a skin that is an animated prefab part composes the part's sim transform,
> not its clip-animated local, while the draw does follow it (a consumer); `reparent(Keep::World)` on a `ParentJoint` child keeps the rest world, not the
> drawn pose (a detach that must keep the drawn pose; the idiom is `posed_joint_world` plus a fresh
> presentation spawn).
>
> **Serious Isaac 0016 (2026-11-09):** `switch_skin_to_ragdoll(world, skin, parts)` turns a live skinned
> mesh into its `SkinBone` ragdoll at the **rest** pose (`$skin` last part; velocity inherited), proven by the
> `ragdoll-skin-switch` gallery scene (trace, restore, 1/2/8 threads, driven golden). The posed switch is
> proposed below (§Decisions), not approved.
>
> Every claim of current-code fact below was read from the tree on 2026-09-26; file:line pointers are
> of that date, so re-grep before editing.

## Trigger

The external game's goblin is eleven rigid bodies (the external game's GDD says "about ten": head, torso, pelvis, and left and right upper arm, lower arm,
upper leg and lower leg) held by `Joint`s. Every motion comes from physics and joint motors; there
are no animation clips. Its look is **one skinned mesh whose bones follow those eleven bodies**. Until
this plan ships, the parts render as eleven separate meshes. The external game schedules the skinned look at its F7.
The user moved the goblin mesh to the front of the Blender experiment, so the engine side starts now.

## What exists (2026-09-26)

- **No skinning anywhere.**
  - `MeshVertex` (`crates/ludion-renderer/src/mesh.rs:18`) has position, normal, color, material, uv
    and tangent (locations 0–5).
  - `MeshInstanceRaw` (`render.rs:76`) has 12 vec4 lanes (locations 6–17). Two components of `fade`
    are reserved.
  - Meshes are uploaded as static indexed buffers (`upload_mesh_asset`, `render.rs:3226`) and drawn
    instanced.
- **Three passes read mesh vertices:** `record_shadow` (`shadow.wgsl`, positions and model rows
    only), `record_prepass` and `record_scene` (`mesh.wgsl`). Attribute layouts are at
    `render.rs:4131-4163`.
- **The design is already written:** `Plan/mesh-modeling.md` decision 12 and Wave 4 step 1.
  - 4 weights and 4 bone indices per vertex.
  - A bone palette in a storage buffer on `@group(1)`: groups 0–3 are taken, and WebGPU guarantees
    four.
  - A per-instance `bone_offset` in a spare lane.
  - Unrigged instances point at an identity entry, so every existing golden renders byte-identically.
  - Poses are built render-side in `extract_at` and are hash-neutral.
  - That design targets sculpt documents and clip-driven poses. This plan builds its renderer half,
    fed by glTF and by rigid bodies.
- **Import:** skins are refused as `L1202` (`crates/ludion-import/src/lib.rs:369`), and so are
  animations (`:229`) and morph targets (`:748`).
- **The mesh document:** `MESH_KEYS` (`crates/ludion-scene/src/mesh.rs:48`) is a fixed list of 18 keys
  at `MESH_FORMAT_VERSION = 1`. An absent optional key emits identical bytes, so new keys are
  additive. No mesh-document byte enters the `WorldHash` or the wire schema; `MeshData.hash` is only
  the renderer's cache key.
- **Ragdolls today:** `switch_to_ragdoll` (`L3036`) turns a posed voxel `kind: assembly` into one body
  per part, with joints from the assembly's `joints:` map, and each part renders rigidly
  (`Mesh.shape: assembly_part`). It has no skeleton or palette path.
- **Physics authoring:**
  - `Joint { a, b, kind, anchor, break_impulse, collide }` (`crates/ludion-spatial/src/joint.rs:36`).
    `anchor` is in `a`'s local frame.
  - A `Collider` shape can be a `compound` of placed parts (`collider.rs:163`), so a capsule offset
    along a bone needs no new field.
  - `Parent` is a link only: nothing propagates transforms through it (`crates/ludion-ecs/src/hierarchy.rs`).

## Decisions

1. **Scope: skin import plus bones driven by bodies. No clips.** glTF skins are imported (joints,
   inverse bind matrices, 4 weights per vertex). The GPU palette skins every mesh pass. Bones follow
   rigid bodies. glTF animations and morph targets stay `L1202` until a game asks for clips.
   **Why:** The external game, the driver, needs a skeleton driven by physics. Clip playback is a whole sim-side
   system (a hashed pose, blending, replays and saves) with no current consumer.
2. **The renderer half of sculpt W4 is reused as written.** The vertex attributes, the palette
   storage buffer, the `bone_offset` lane, the identity entry and render-side pose evaluation all
   carry over. When sculpt resumes, its W4 shrinks to deriving weights from the field, plus clips.
3. **The Blender rig is the only source of the skeleton.** The rest poses come from the imported skin.
   Nothing restates them in text. **Why:** re-proportioning the goblin in Blender must move its joints
   too. Two hand-kept copies of one skeleton drift.
4. **Physics tunables stay in scene text.** Colliders, masses, joint kinds, limits, motors and break
   impulses are authored per part entity with the existing `Collider` and `Joint` components. They
   are gameplay tunables iterated dozens of times, so they must hot-reload as text and never need a
   Blender re-run. This generalizes the voxel-only ragdoll path to skinned meshes, so vigil can use
   it too.
5. **One link per part: `SkinBone { skin, bone }`.** It is a component on each rigid part entity,
   naming the entity that draws the skinned mesh (resolved like a `Joint`'s `a`/`b`) and a bone of
   that mesh's skin. It does two things:
   - **Spawn pose.** A part entity with `SkinBone` and **no authored `Transform`** is placed at the
     skin entity's world transform × the bone's rest world transform when the scene loads. An
     authored `Transform` wins, so a scene saved mid-flight reloads where it was.
   - **Pose source.** In `extract_at`, the bone's palette entry is the part's world pose × the
     bone's inverse bind matrix. A bone that no part names keeps its rest pose relative to its parent,
     and the skin entity's own `Transform` is the root.

   **Why this shape instead of request 0002's `BoneFollow { bones: { bone: entity } }` map on the
   skin entity:** the spawn-pose rule already needs a per-part link from part to bone. The map would
   restate every link in reverse, which makes two ways to say one thing. One link per part also keeps
   everything about a part (collider, joint, bone) on that part's entity. The request's semantics are
   kept unchanged: render-only palette, unlisted bones at rest, one pose source per skin. A skin
   entity carrying `Animation` while parts name it is refused.
6. **The spawn pose makes rest data sim input.** A scene that uses `SkinBone` without authored
   `Transform`s reads the mesh document's joint rest transforms at load, so re-rigging the goblin
   moves its `WorldHash`, and its replays re-record. That is correct: the bodies really move. Scenes
   that don't use `SkinBone` are untouched, and the palette itself stays hash-neutral. The rest
   transforms are parsed from canonical text (ryu floats) on the CPU, so they are bit-stable across
   the two pinned platforms.
7. **No wire movement.** `SkinBone` is a registered component like `Joint`. No new method is added.
   The wire schema snapshot is expected not to move (it names no component type), and W3 verifies it.
8. **Automatic anchor derivation: not built.** `ludion check-scene <mesh> --bounds --json` already reads
   back every joint's head-in-parent — authoring `examples/gallery/ragdoll-skinned.yaml`'s 10 `Joint`
   anchors was a straight copy from that output (rounding a few `1e-8`-scale export noise components to
   `0`), a few minutes' work. The real cost was the two elbow hinge axes: the mannequin's arms sit at
   45° in the A-pose, so the plan's "bone X is lateral" shortcut (which held for the vertical leg
   bones, reused from `ragdoll.yaml` unchanged) doesn't apply — the axis had to be worked out by hand
   from the raw mesh document's `upper_arm_*` joint `inverse_bind` matrix (its rotation submatrix maps
   a model-space direction into the joint's local frame), applied to the world axis the A-pose's own
   geometry picks out as the natural bend plane's normal. That took reading raw mesh-document floats
   and doing the linear algebra myself; `check-scene --bounds` has no instrument for it. **Verdict:**
   not built — head-in-parent anchors are already a copy job via `--bounds --json`; a derived-axis
   helper (e.g. `--bounds` printing each joint's local X/Y/Z restated in model space, so a hinge axis
   choice is a lookup instead of a matrix multiply) is the concrete future win, triggered if a second
   rig needs off-axis hinges.

**Proposed (Serious Isaac 0016, owner approval owed).** The posed switch, ready to build:

- (i) Decision 5 gains a second spawn-pose source: the switch's posed pose. Scene load is unchanged.
- (ii) The pose is sampled once, in sim, at the switch tick, through the libm sampler
  (`ludion_scene::sample_rotation`/`slerp`, hand-normalized quats as in `ragdoll.rs:153-186`) over clip +
  crossfade + speed + the `set_clip_speed` anchor; `hold` and the clip `scale` are ignored.
  `posed_joint_world` stays presentation-only (`L0028`).
- (iii) Joint limits still count from rest: the joint frame carries the rest-relative offset.
- (iv) An opt-in clip bone velocity (a finite difference of two samples), default off.
- (v) A `ragdoll:` key on `kind: character` names the parts prefab.

## The mesh document (additive, format stays 1)

Three optional keys, present only on a skinned mesh:

```yaml
joints:            # glTF order; parent index or null; rest = local TRS at bind; inverse_bind = 16 floats, column-major
  - { name: pelvis, parent: null, rest: { t: [...], r: [...], s: [...] }, inverse_bind: [...] }
  - { name: torso,  parent: 0,    rest: { ... },                          inverse_bind: [...] }
weights:           # 4 per vertex, normalized, sorted by (weight desc, joint index)
bone_indices:      # 4 per vertex, into joints
```

- The importer folds a skin's joint nodes into `joints:`. **They are not emitted as scene entities.**
  The skinned mesh's node becomes one entity as today.
- **Import rules:**
  - Only `JOINTS_0`/`WEIGHTS_0` are read. A second set is `L1202`.
  - A mesh skinned by more than one skin is `L1202`.
  - Weights are renormalized, and a zero-weight vertex binds to its skin's root joint (a warning).
  - A joint count cap (256) is enforced.
- `check_mesh` validates: an index in range, a weight sum of 1 ± ε, a parent that exists and is
  earlier, unique names. It uses new structure and consistency codes (the next free `L1xxx` slots when
  W1 lands).
- `check-scene --bounds` on a skinned mesh prints the joint tree with each joint's rest head in model
  space. That list is the author's read of the rig.

## Waves

### W1 — skins through import

1. `ludion-scene` mesh keys, reader and canonical writer for `joints`, `weights`, `bone_indices`.
   Mesh documents without a skin re-emit byte-identically, which the existing import conformance
   test already proves.
2. `ludion-import`: skin mapping per the rules above. `L1202`'s help text drops "a skin".
3. **Test fixtures made by the Blender pipeline.** A two-bone tube and an eleven-bone mannequin whose
   bone names match the external game's parts. Each is a committed `.glb`, since Blender is not on CI, beside the
   bpy script that built it (`Plan/asset-pipeline-blender.md` *The asset contract*).
   Tests: a byte-identical re-import, the joint tree, the weight invariants, each refusal.
4. Docs: the mesh-format topic (the new keys) and `dev-docs/ludion-import.md` (the mapping table).
5. **Exit:** the mannequin and the external game's goblin glb (once its brief has run) import clean with no
   `--allow-lossy`.

### W2 — the palette in the renderer

1. Sculpt W4 step 1 as written: `MeshVertex` gains `weights` and `bones` (defaults `[1,0,0,0]` and
   `[0;4]`), and `MeshData` gains both arrays. The palette storage buffer goes on `@group(1)` after
   checking `Gpu`'s requested limits. `bone_offset` goes in the free `fade` components. The shadow,
   prepass and scene shaders skin position and normal (tangent too where the pass reads it).
2. **The golden gate comes first.** Every existing gallery and vigil golden renders byte-identically
   before any skinned content lands.
3. `extract_at` builds a rest-pose palette for a skinned mesh: world(rest) × inverse bind, which is
   identity only up to the export's precision (f32-exported inverse binds), and the skinned shader
   branch runs different arithmetic (a blended matrix, then the model) even at bind.
   - Test: every mannequin rest-palette entry is identity within `1e-5` per element.
   - GPU test: the mannequin rendered skinned at rest matches its skin-stripped copy at SSIM `0.999`
     (`ludion_testkit::compare_png_bytes`), with a negative control (a nudged inverse bind) that does
     not pass — proving the palette reaches the GPU on this path.
   - Unit test: an injected palette deforms as expected.
4. **Bounds.** No mesh frustum culling exists (`render.bounds` uses a unit box per asset mesh), so a
   skinned instance can't be culled either. When mesh culling lands, a skinned instance's bounds are
   the union of its palette's joint positions, expanded by the rest mesh's largest per-joint vertex
   radius.
5. Docs: the `dev-docs` renderer passes row for the skinning term.

### W3 — `SkinBone`: bodies drive bones

1. The component and its registration.
   - At scene load, the spawn pose is resolved from the skin entity's `Transform` and the rest
     transforms.
   - Diagnostics, with the next free codes:
     - an unknown skin entity;
     - a skin entity whose mesh has no skin;
     - an unknown bone, with the nearest-match suggestion;
     - two parts naming one bone;
     - `Animation` on a skin that parts name.
2. `extract_at` fills followed bones from their parts' world poses, and unfollowed bones by rest
   composition down the tree.
3. **Proofs.**
   - The palette is hash-neutral: an extracted run's `WorldHash` sequence and `.lrp` bytes match a
     never-extracted run (the voxel-animation test shape).
   - The spawn pose is thread-count-invariant and cross-arch stable: it joins the M4 queue.
4. **The collider along the bone** is a one-part `compound` placed along the bone's +Y (Blender's
   head → tail axis). Joint anchors are authored in the parent part's frame. `check-scene` prints each
   child bone's head in its parent's frame, so an anchor is copied, not computed by hand.
   Deriving anchors automatically is evaluated here and built only if the gallery scene shows the
   authoring is error-prone — not built, decision 8.
5. **Content:** `examples/gallery/ragdoll-skinned.yaml`. The mannequin as a skinned ragdoll dropped
   onto steps, with a trace and a golden (`ragdoll-skinned`), blessed on the 3090. The M4 re-bless
   is owed.
6. Docs: `docs/physics-joints.md` (§ skinned ragdolls, replacing "Next") and `docs/components.md`
   (`SkinBone`). The rest-pose rule is stated where the joint-zero gotcha is.

### W4 — the goblin (exit)

1. In the external game's repo: F7 swaps the eleven placeholder part meshes for the skinned goblin, one `SkinBone`
   per part. Request 0002 moves to `done/` with a Shipped section.
2. **Exit (human gate, owed):** the goblin tumbling in the external game. Does it read as one continuous
   body through the shoulders, hips and neck at launch speed? The verdict is recorded as a decision.
   Close: the root `CLAUDE.md` line, and this plan's Status collapsed.

## Non-goals

- Animation clips and blending: designed in `Plan/animation-clips.md`.
- Morph targets.
- IK.
- Root motion.
- Skinning sculpt meshes: that is sculpt W4's weight half, when sculpt resumes.
- CPU skinning.
- Dual-quaternion skinning.
- More than 4 influences.
- Townsfolk ragdolls: the external game defers them.

## Risks

- **Bind-group ceiling.** Groups 0–3 are taken, and the palette goes on `@group(1)` unless the
  requested limits allow more (sculpt W4's recorded risk).
- **Stretching at the joints under physics.** Rigid bodies don't hold perfect joint positions at
  high impulse, so a skinned shoulder can tear visually under a hard hit. The W4 sitting judges it.
  Stiffer joint solver settings are the tuning lever, not a mesh fix.
- **Rest-pose mismatch.** A part whose collider or joint anchor was tuned for an older rig silently
  misfits after a re-rig. The `check-scene` head list is the author's read. A diagnostic for a
  collider far outside its bone's weighted vertices is a W3 candidate.
- **The instance lane budget.** `bone_offset` takes the reserved `fade` components. The next per-instance
  datum needs a 13th lane.
- **No skinned-instance culling bounds yet.** Trigger: mesh frustum culling lands, or `render.bounds`
  loads real asset extents instead of a unit box. Either one needs a skinned instance's bounds to be
  the union of its palette's joint positions, expanded by the rest mesh's largest per-joint vertex
  radius (W2 step 4).
