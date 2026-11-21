# Plan: Animation layers — bone masks, layers, blend spaces, IK, root motion and a state-graph helper

> Status: **W1, W2a, W2b (sim), A1, A2, W2c, W2d, W4a and W4b mechanized (2026-11-18); part sockets and W5a (ground feet) mechanized (2026-11-19); W3 mechanized (2026-11-20); W5b bench mechanized (2026-11-21; the measurement and the budget files are owed); W6a (root motion) mechanized (2026-11-21); A3 deferred; W6b not started; the macOS twins of the W2d goldens, of `humanoid-look-yaw-limit-strip.png` and of `assembly-ground.png` are owed to the M4.** Driver: Neon Shroud request 0007
> (`engine-requests/0007-animation-layers-blendspaces-ik.md`, filed 2026-11-17, audited at engine r2369).
> It lifts the deferrals `Plan/animation-clips.md` §Non-goals recorded, each with a fired trigger. The
> design below was written from the tree on 2026-11-18, so re-grep before editing.

## Status

### W1 — bone masks, `AnimationLayers` (override and additive), L1670–L1672

| file | role |
|---|---|
| `crates/ludion-scene/src/character.rs` | `masks:` parse (`L1199`), validate and joint check (`L1200`), `emit`, `resolve_character_mask` |
| `crates/ludion-components/src/animation_layers.rs` | `AnimationLayers`, `AnimationLayer`, `LayerMode`, `layer_weight`, `fade_layer`, `play_layer`, `L1670`/`L1671` |
| `crates/ludion-engine/src/character.rs` | `CharacterData.masks` (one weight per skin joint) |
| `crates/ludion-engine/src/extract.rs` | `layer_poses`, `layered_joint_pose`, `sampled_palette_layered`; `joints_sampled` counts layers |
| `crates/ludion-engine/src/skin_bone.rs` | `L1672` |
| `crates/ludion-engine/tests/animation_layers.rs` | neutrality, mask, ramp, restore, threads, socket, bad-mask proofs |

Codes: `L1670` `ANIMATION_LAYERS_TOO_MANY`, `L1671` `ANIMATION_LAYER_INVALID`, `L1672`
`ANIMATION_LAYERS_ON_SKIN_BONE_SKIN`. A scene without `AnimationLayers` poses bit-identically: no
`WorldHash`, replay, trace, wire-schema or golden moved. Only `component_schemas.json` re-blessed.

### W2a — blend spaces, `Animation.params`, `BlendClock`, `blend_clocks`

| file | role |
|---|---|
| `crates/ludion-scene/src/blend_space.rs` | `triangulate`, `blend_weights_1d`/`_2d`, `BlendWeights` |
| `crates/ludion-scene/src/character.rs` | `blends:` parse (`L1199`), validate and point checks (`L1200`), `emit` |
| `crates/ludion-components/src/blend_clock.rs`, `lib.rs` | `BlendClock` (appended last, sim-only), `Animation.params` (omitted at `[0, 0]`) |
| `crates/ludion-engine/src/character.rs` | `CharacterData.blends` (`CharacterBlendData`, `BlendPointData`: weights, rate, point time) |
| `crates/ludion-engine/src/blend_clock.rs` | the `blend_clocks` system, `blend_crossings` |
| `crates/ludion-engine/src/animation_events.rs`, `extract.rs` | the library's blend map and the blend event slot; `PoseSide`/`PoseFade`, the N-way joint blend |
| `crates/ludion-engine/tests/blend_spaces.rs` | cadence, point pose, neutrality, threads, restore, crossfade, load errors |

Blend errors reuse `L1199`/`L1200`. A scene without a blend poses and hashes as before; `Animation` text
and `component_schemas.json` are the only wire-visible moves. The d2 amendment ships in its opt-in form
(only a blend-using entity hashes a `BlendClock`), **accepted by the owner 2026-11-19**.

### W2b (sim) — layer blends, layer `params`, `reference:`, layer events

| file | role |
|---|---|
| `crates/ludion-components/src/animation_layers.rs` | `AnimationLayer.events`/`params`/`reference`, `set_layer_params`, `L1671` on a non-finite `params` entry |
| `crates/ludion-components/src/blend_clock.rs` | `BlendClock.layers`, `LayerPhase` |
| `crates/ludion-engine/src/blend_clock.rs` | `blend_clocks` keeps the clock while a layer names a blend; `layer_fallback_phase`; `blend_crossings` takes `(prev, phase)` |
| `crates/ludion-engine/src/extract.rs` | `LayerPose.pose`/`reference` as `PoseSide`, `layer_poses` resolves a layer blend, `PoseSide::at_zero` |
| `crates/ludion-engine/src/animation_events.rs` | `ClipEvent.layer`, `walk_layers`, the "Layers" crossing rule |
| `crates/ludion-engine/tests/layer_events.rs` | events, weight 0, blend sweep and restart, scaled clock, restore, point pose, reference, neutrality |

No `WorldHash`, replay, trace, wire-schema or golden moved. A scene without a layer blend never carries a
`BlendClock`; only `component_schemas.json` re-blessed.

### A1 — part masks and `AnimationLayers` on voxel assemblies

| file | role |
|---|---|
| `crates/ludion-scene/src/rig_mask.rs` | `RigMask` (was `CharacterMask`), the shared parser, name validation, emitter and ancestor-walking `resolve_mask`; `resolve_rig_mask` (was `resolve_character_mask`) |
| `crates/ludion-scene/src/character.rs` | re-wired to the shared mask code; messages and `L1199`/`L1200` byte-identical |
| `crates/ludion-scene/src/assembly.rs` | `masks:` after `joints:`: `AssemblyDocument.masks`, parse (`L1137`), unknown part and empty name (`L1138`), emit |
| `crates/ludion-engine/src/assembly_mesh.rs` | `AssemblyData.masks` (`AssemblyMaskData`), `AssemblyData::mask`; masks stay out of the mesh content hash |
| `crates/ludion-engine/src/extract.rs` | `assembly_layer_poses`, `layered_part_pose`, `part_placements_layered`; `part_placements_with` takes the layers; `blended_part_pose` wraps an `f64` core |
| `crates/ludion-engine/src/animation_events.rs` | `walk_layers` and the baseless walk open to assemblies |
| `crates/ludion-engine/tests/assembly_layers.rs` | neutrality, mask, unkeyed channel, descendants over a non-topological order, additive and `reference`, crossfading base, restore, hash neutrality, 1/2/8 threads, events |

No component, `WorldHash`, replay, trace, wire-schema or golden moved; the component schema did not move.

### A2 — blend spaces on voxel assemblies (base and layer)

| file | role |
|---|---|
| `crates/ludion-scene/src/rig_blend.rs` | `BlendSpace` (was `CharacterBlend`), `BlendPoint`, `BlendAt`; the shared parser, validation, point check (bound, looping, sync over the caller's effective events) and emitter, taking the family's structure and consistency codes |
| `crates/ludion-scene/src/character.rs` | re-wired to the shared blend code; messages and `L1199`/`L1200` byte-identical |
| `crates/ludion-scene/src/assembly.rs` | `blends:` after `masks:`: `AssemblyDocument.blends`, parse (`L1137`), validate (`L1138`), emit, `check_assembly_blends` |
| `crates/ludion-engine/src/blend_space.rs` | `BlendSpaceData` (was `CharacterBlendData`) and `resolve_blends`, rig-neutral over each bound clip's name, duration, speed and events |
| `crates/ludion-engine/src/assembly_mesh.rs` | `AssemblyData.blends` and `AssemblyData::blend`; the blend check runs where the clips are read |
| `crates/ludion-engine/src/animation_events.rs`, `blend_clock.rs` | `AnimationLibrary.blends` keyed by model; the base and layer blend branches and `blend_clocks` accept assemblies |
| `crates/ludion-engine/src/extract.rs` | `AssemblySide`, `part_placements_sides` (the wrappers keep their signatures), `animation_pose` and `assembly_layer_poses` resolve blends, `layered_part_pose` samples `layer.side` |
| `crates/ludion-engine/tests/assembly_blends.rs` | cadence, point-equals-clip, 2D, hash neutrality, 1/2/8 threads, restore, crossfade, layer blend on a mask, no clock on a plain assembly, ragdoll switch, `L1138` refusals |

No component, `WorldHash`, replay, trace, wire-schema or golden moved; the component schema did not move.

### W2c — `check-scene` masks and blends, `render --model --blend/--layer`

| file | role |
|---|---|
| `crates/ludion-cli/src/check_scene.rs` | `run_assembly` lists `mask` and `blend` lines (2D triangles beneath) and emits `masks` and `blends` arrays in `--json` |
| `crates/ludion-engine/src/model_sheet.rs` | `parse_blend`, `parse_layer`, `BlendSweepSpec`, `Sweep`, `LayerSpec`; `resolve_blend`, `check_layers`; `PhaseReport.params`/`phase`, `ClipOutcome.blend`/`layers`; each phase entity carries `AnimationLayers`; the report poses through `part_placements_sides` |
| `crates/ludion-engine/src/extract.rs` | `assembly_layer_pose` (the per-layer body `assembly_layer_poses` maps over), `AssemblySide::blend` crate-visible; `part_placements_faded` retired |
| `crates/ludion-cli/src/main.rs`, `render.rs` | `--blend`, repeatable `--layer`, `<name>-<blend>-blend.png`, the `-layered` suffix, the report lines |
| `crates/ludion-engine/tests/model_sheet.rs`, `crates/ludion-cli/tests/check_scene_voxel_animation.rs`, `render_model.rs` | the sweep, the layered pose, the refusals, the listings |

No component, `WorldHash`, replay, trace, wire-schema or golden moved. `check-scene` does not list a
`kind: character`'s blends.

### W2d — the gallery humanoid, `assembly-layers` and two goldens

| file | role |
|---|---|
| `examples/gallery/voxels/humanoid.assembly.yaml`, `voxels/anims/humanoid-{idle,walk,run,carry,lean}.anim.yaml` | the ragdoll's ten parts with looping `idle`/`walk`/`run` clips carrying `foot_l`/`foot_r`, a `carry` override clip, a `lean` additive clip, an `upper_body` mask and a 1D `locomotion` blend synced on `foot_l` |
| `examples/gallery/assembly-layers.yaml`, `goldens/manifest.json` | five figures at frame 60: the blend at `[1, 0]` and `[3.5, 0]`, a `carry` layer mid-fade, an additive `lean` at 0.7, a layer blend; golden `assembly-layers.png` |
| `crates/ludion-testkit/src/golden.rs` | `GoldenTest.blend` and `layers`, parsed by `parse_blend`/`parse_layer` as `crossfade` is |
| `crates/ludion-testkit/tests/gallery.rs` | `humanoid_locomotion_blend_strip_test`: `locomotion:0..5` over six phases at 0.3 s; golden `humanoid-locomotion-blend-strip.png` |
| `crates/ludion-engine/tests/gallery_assembly_layers.rs` | the humanoid and its clips are canonical, the scene validates, the blend figures fire `foot_l` and `foot_r` alternately |

Both goldens are blessed on the RTX 3090 only; `goldens/macos/assembly-layers.png` and
`humanoid-locomotion-blend-strip.png` are owed to the M4. Every strip phase reports 1 component. No
engine behaviour, component, `WorldHash`, replay, wire-schema or existing golden moved.

### W4a — the assembly `ik:` key, `IkTargets`, look-at and two-part IK

| file | role |
|---|---|
| `crates/ludion-scene/src/rig_ik.rs`, `assembly.rs` | the `ik:` key: parse, canonical emit, `L1137` structure and `L1138` cross checks; `AssemblyPartEntry::point_in_mesh_frame` |
| `crates/ludion-engine/src/assembly_mesh.rs` | `AssemblyData.ik`: a chain's part indices, mesh-frame effector, hinge axis, bend and limits; a look-at's mask weights |
| `crates/ludion-components/src/ik_targets.rs`, `codes.rs` | the hashed `IkTargets`, `fade_ik_target`, `ik_target_weight(_at)`, `L1673` |
| `crates/ludion-engine/src/assembly_ik.rs`, `extract.rs` | the `f64` solver and `assembly_ik_goals`, applied after `part_placements_layered` |
| `crates/ludion-engine/tests/assembly_ik.rs` | neutrality, a point target, a rewritten point, a direction and its yaw limit |

No `WorldHash`, replay, wire-schema or golden moved; the component schema snapshot gained `IkTargets`.
Tooling (`check-scene`, `render --look/--ik`, a golden) is W4b.

### W4b — `check-scene` lists `ik:`, `render --look/--ik`, the yaw-limit golden

| file | role |
|---|---|
| `crates/ludion-cli/src/check_scene.rs` | the `ik:` listing, text and JSON (`"ik"` only when non-empty), from the document's names and effector and the resolved hinge |
| `crates/ludion-engine/src/model_sheet.rs`, `lib.rs` | `parse_look`/`parse_ik`, `ModelSheetRequest.look`/`ik`, `PhaseReport.look`, `ClipOutcome.ik`, the `IkPlan` the report poses through `apply_assembly_ik` and the scene's `IkTargets` |
| `crates/ludion-cli/src/main.rs`, `render.rs` | `--look [NAME=]YAW,PITCH` and a repeatable `--ik NAME=X,Y,Z`; `-ik` in the output name; `look [y, p]` per phase and one `ik:` echo |
| `crates/ludion-testkit/src/golden.rs`, `tests/gallery.rs` | `GoldenTest.look`/`ik`; `humanoid_look_yaw_limit_strip_test`, golden `humanoid-look-yaw-limit-strip.png` |
| `examples/gallery/voxels/humanoid.assembly.yaml` | a `head_look` mask, a `look` look-at (yaw 70, pitch 40) and a `hand_r` chain |
| `crates/ludion-cli/tests/fixtures/assembly_ik/` | the CLI fixture: an explicit-hinge chain, a defaulted `lower-arm-*` chain and a look-at |

The strip sweeps the yaw 0 to 100 over six phases, so the last two cells clamp at 70°; every phase reports
1 component. The golden is blessed on the RTX 3090; `goldens/macos/humanoid-look-yaw-limit-strip.png` is
owed to the M4. Masks and `ik:` are pose data outside the mesh content hash, so `assembly-layers.png` and
`humanoid-locomotion-blend-strip.png` passed unblessed. No component, schema, `WorldHash`, replay or existing
golden moved.

### Part sockets — `ParentJoint` naming an assembly part (request 0007 item 0)

| file | role |
|---|---|
| `crates/ludion-engine/src/assembly_mesh.rs` | `part_socket_rests`: each part's `offset + pivot` from the documents, no meshing |
| `crates/ludion-engine/src/parent_joint.rs` | `SkinRests` learns `MeshSource::Assembly` (the sim rest, a `Trs` per part name); `posed_joint_world` answers for an assembly; `L1318`/`L1319` cover assemblies |
| `crates/ludion-engine/src/extract.rs` | `assembly_part_models`, the one placement sequence (clip or blend, crossfade, `hold`, layers, IK) for the mesh loop, the sockets and `posed_joint_world`; `pose_each` poses assembly sockets |
| `crates/ludion-core/src/codes.rs`, `crates/ludion-components/src/lib.rs` | `L1318`/`L1319` text and the `ParentJoint` doc comment |
| `crates/ludion-engine/tests/assembly_socket.rs` | the sim rest across clip, layers and IK, the draw riding the part, drawn == sim at rest, hash neutrality with a collider, thread invariance, diagnostics |
| `examples/gallery/assembly-socket.yaml`, `goldens/` | a flashlight on a walking arm and a cube on a resting head; golden blessed on the RTX 3090 (macOS twin owed to the M4) |
| `docs/voxel-assemblies.md`, `character-animation.md`, `components.md` | §Sockets on parts and the pointers |

No `WorldHash`, replay, wire-schema or existing golden moved; the component schema snapshot changed only in the
`ParentJoint` doc comment. A `ParentJoint` inserted at runtime under an assembly used to compose as a plain
`Parent` and now gains the rest level (committed content could not carry one: `L1318`).

### W5a — ground feet

| file | role |
|---|---|
| `crates/ludion-scene/src/rig_ik.rs`, `assembly.rs`, `lib.rs` | `IkGround`, `IkKind::Chain.ground`, the top-level `ground:` key (parse `L1137`, cross checks `L1138`, canonical emit after `ik`) |
| `crates/ludion-engine/src/assembly_mesh.rs`, `lib.rs` | `AssemblyIkKind::Chain.ground`, `AssemblyData.ground` / `AssemblyGroundData` (outside the mesh content hash) |
| `crates/ludion-engine/src/assembly_ik.rs`, `extract.rs` | `apply_assembly_ground` (the cast, dead band, `root_drop`), called from `assembly_part_models` after the IK block |
| `crates/ludion-engine/tests/assembly_ground.rs` | flat and no-collider bit-identity, a step, a step too tall, `root_drop`, a goal winning, a socket on a grounded part, repeat, thread and hash proofs, the gallery files |
| `examples/gallery/voxels/humanoid-grounded.assembly.yaml`, `assembly-ground.yaml`, `goldens/` | a grounded humanoid; one figure with a foot on a step, one on a 15° ramp; golden `assembly-ground.png` blessed on the RTX 3090 |
| `crates/ludion-cli/src/check_scene.rs`, `tests/check_scene_voxel_animation.rs` | `, ground` on a chain row, a `ground:` line, JSON `"ground"` |
| `docs/voxel-animation.md`, `docs/voxel-assemblies.md`, embedded mirror | §Ground feet and the key table row |

No `WorldHash`, replay, wire-schema, component-schema or existing golden moved; new golden
`assembly-ground.png` blessed on the RTX 3090, macOS twin owed.

### W3 — `AnimGraph` and `AnimGraphState`

| file | role |
|---|---|
| `crates/ludion-engine/src/anim_graph.rs` | `AnimGraph`, `AnimState`, `AnimEdge`, `AnimGate`: `step`, `current`, `describe`, with unit tests |
| `crates/ludion-components/src/anim_graph_state.rs`, `lib.rs` | the hashed `AnimGraphState`, registered after `PoseSnapshot` |
| `crates/ludion-core/src/codes.rs` | `L4290`, `L4291` |
| `crates/ludion-testkit/tests/anim_graph_cycle.rs` | the six-state port of `CYCLE`: equal `Animation` every tick, the switch log, a snapshot restore |
| `crates/ludion-engine/tests/anim_graph.rs`, `examples/anim_graph.rs` | an assembly driven through carry, wave and idle; layer enter/leave, one-shot end, `Interrupts`, thread invariance, `L4290`; the runnable example |
| `docs/animation-layers.md` §State graph, `character-animation.md`, `voxel-animation.md`, `components-fx.md`, embedded mirrors | the API, the step rules and the initial-state idiom |

No `WorldHash`, trace, replay, protocol schema or golden moved (no committed scene carries
`AnimGraphState`); the component schema snapshot gained `AnimGraphState` only. `CYCLE`, `cycle_builder`,
`character-cycle.trace.txt` and `character-cycle.lrp` are untouched (decision 53).

### W5b — the assembly animation bench

| file | role |
|---|---|
| `crates/ludion-engine/tests/assembly_anim_bench.rs`, `it.rs` | the five arms, the in-file 0.5 ms gate on `full`, the optional pinned budget check, the non-ignored `assembly_anim_bench_scene_poses` companion (hash at 1 and 8 threads, layers/IK/ground change figure 0, at least half the figures have a foot moved by the ground) |
| `crates/ludion-engine/tests/bench_rows.rs` | `sequential(report, name)`, the median of a `sim.sequential` entry |
| `docs/voxel-animation.md` §Ground feet, embedded mirror | the cost line and the bench command |

No measurement was taken and no budget file written. Only tests and docs changed.

### W6a — root motion on assemblies

| file | role |
|---|---|
| `crates/ludion-scene/src/animation.rs`, `assembly.rs`, `lib.rs` | `ROOT_TRACK`, the `$root` `L1142` rules, the reserved part name `L1138` |
| `crates/ludion-engine/src/animation_events.rs` | `RootClip`, the `roots` cache, `root_clips`, the pre-fill in `timings` |
| `crates/ludion-engine/src/root_motion.rs`, `lib.rs` | `RootMotion`, `root_motion_delta`, the window accumulation |
| `crates/ludion-engine/tests/root_motion.rs`, `it.rs`, `examples/root_motion.rs` | sums, fades, refusals, clock, threads, inertness |
| `docs/voxel-animation.md` §Root motion, `docs/animation-layers.md`, embedded mirrors | the user-facing contract |

No hash, schema or golden moved.

## Trigger

The game's characters do locomotion plus something else on one skin: carry a body, aim, peek around a
corner, look at a stimulus, walk by stick deflection, take down a guard. `Plan/animation-clips.md`
§Non-goals deferred layers, IK, root motion and a state graph, each with a trigger. Request 0007 fires
all four: three characters (player, militia guard, android worker), built as voxel assemblies (decision 25),
share one driver shape.

## What exists (2026-11-18)

- **One clip plus one crossfade per skin.** `Animation { clip, started, speed, anchor, from*, fade, hold }`;
  `extract::sampled_palette` blends one outgoing and one incoming sample per joint. The pose is
  presentation-only (decision 2 of `Plan/animation-clips.md`); the playback inputs are hashed.
- **One pose source per skin.** `L1197` refuses `Animation` on a `SkinBone`-driven skin.
- **Events** come from the incoming clip of a crossfade only (`animation_events`).
- **Sockets.** `ParentJoint` and `posed_joint_world` go through `skin_palette`, so they read whatever pose
  extract composes.
- **Crowd posing** is parallel and thread-invariant (`par_map_chunks`, fixed chunks); every addition keeps that.

## Decisions

1. **Masks are `joints:` + `descendants:` + optional `weights:`.** One spelling. The request's
   `from: X` is `joints: [X], descendants: true`. `weights` (one per listed joint, `[0, 1]`) defaults to 1.
   With `descendants`, an unlisted joint inherits its nearest listed ancestor's weight; every other joint
   is 0. An unknown joint is `L1200` (help names the nearest); a bad shape or weight is `L1199`.
   **Why:** two ways to say "this joint and below" is the second way the pillars forbid.
2. **Layers are presentation-only, hashed inputs.** `AnimationLayers` is ordinary sim state (it snapshots,
   rides a scene switch); no system reads it. Layers evaluate in list order over the base pose; at most 4
   (`L1670`). A scene without the component is byte-identical.
3. **Override.** A joint the layer clip has no track for is untouched. The effective weight is
   `layer weight × mask weight`; exactly 1 takes the layer's channels verbatim (a missing channel reads as
   rest); below 1 slerps rotation and lerps translation and scale from the pose beneath.
4. **Additive reference is the layer clip's first frame (`t = 0`).** With layer sample `c` and reference `r`:
   rotation `base ⊗ slerp(I, r⁻¹ ⊗ c, k)`, translation `base + k (c − r)`, scale `base × (1 + k (c / r − 1))`.
   Quaternion product and conjugate are plain arithmetic. A named `reference: <clip>` is decision 21.
5. **The weight ramps from `from_weight` at `changed` over `fade` seconds**, derived from tick counts like
   `crossfade`, on the base `Animation.hold` grid. `fade_layer` writes only on change, so a per-tick caller
   does not churn the hash; `play_layer` restamps only on a new clip. Games pass `ctx.game_time`.
6. **W1 layers ignore `SkinBone`-driven skins (`L1672`) and assemblies and prefab parts.** One pose source
   per skin stays true. Masks name skin joints, so assembly parts would need part-name masks (see Owner rulings).
7. **Layer events are not emitted in W1.** Replaced by decision 22 (W2b): an opt-in `events: true`.
8. **Blend spaces** (W2): `blends:` in the character document, 1D by a scalar and 2D by a vector, driven by
   `Animation.params`, with phase sync through a hashed `BlendClock`. A blend emits the events of its
   highest-weight clip (ties go to the lower index) so footstep noise stays on the feet.
9. **A Rust `AnimGraph` helper** (W3), not an asset: a table of `(clip, from, leave, next, fade)` plus an
   `AnimGraphState` component, replacing `CYCLE` in `crates/ludion-testkit/tests/common/mod.rs` with an
   identical switch log and `.lrp`. There is no animation asset language. *Amended (2026-11-20): the shipped table
   is states + edges (the request's §6 shape) rather than `(clip, from, leave, next, fade)` rows, because states
   that share a clip (carry vs locomotion) need their own identity; the `CYCLE` swap moves to R14 (decisions
   50–53).*
10. **IK and look-at** (W4): `IkRig` and `IkTargets` components, a look-at chain and a two-bone solver, run
    after layers; the final pose goes through `posed_joint_world` so sockets follow. (Decision 36 replaces
    `IkRig` with the assembly's `ik:` key.)
11. **Foot grounding** (W5): read-only physics casts in extract, the 32-character budget of ≤0.5 ms, and
    `ProfileReport` counters (a schema move, re-blessed there). *Amended (2026-11-21):* the bench gates
    `ProfileReport`'s existing extract sequential timing (`Budget.extract_ms`); the four counters move the
    protocol schema, so they ride R14 (decision 54).
12. **Root motion** (W6): *amended by W6a:* root motion is the reserved `$root` track of `kind: animation` (request
    section 5), read by `root_motion_delta` on assemblies, a deterministic sim read of clip data through the libm
    sampler. The glTF `ludion_root_motion` extras flag becomes a later import-side writer of `$root`. **Trigger:** a
    skinned `kind: character` game needs root motion. Why: the owner's 2026-11-17 ruling makes the game's
    characters assemblies, and one runtime serves both once the importer writes the track.
13. **Blend points** are `clip` plus `at`: a number is 1D (`params[0]`), a block list `[x, z]` is 2D
    (`params[0..2]`). The dimension is inferred and every point must agree; there are no param names.
14. **Weights.** 1D is linear between the sorted neighbours and clamps at the ends. 2D is barycentric over
    a deterministic triangulation: every index triple `a<b<c`, non-degenerate, with no point strictly inside
    its circumcircle, accepted in lexicographic order and skipped if it overlaps an accepted one. A query
    takes the triangle whose closest point to it is nearest, ties to the first, and clamps outside the hull.
    At most 16 points.
15. **Phase** φ is unwrapped cycles. A point clip `i` plays at `frac(φ + sᵢ) · durᵢ`, `sᵢ = t_syncᵢ / durᵢ`
    (0 without `sync`). The rate is `Animation.speed · Σ wᵢ · binding_speedᵢ / durᵢ` per game second,
    integrated by `blend_clocks` with `game_dt`. A restart (new `(clip, started)`) sets φ to the stateless
    value `elapsed · rate` (so a fresh clock agrees with decision 18) and parks `prev` at `-1`.
16. **Blend events.** A blend emits the dominant point's effective events (max weight, ties to the lower
    index), each at phase position `frac(e/durᵢ − sᵢ)`, fired on crossings in `(prev, φ]`; `prev = -1` on a
    restart tick opens the window at phase 0 inclusive. Events report the **blend** name as `ClipEvent.clip`.
17. **`hold` does not step a blend's phase** (W2a limit; trigger: a held-cadence character on a blend).
18. **With no `BlendClock`** (system not registered) extract falls back to `φ = elapsed · rate(params now)`:
    stateless, exact while the params hold still, used by tools; no events fire.
19. **A blend name may not equal a clip name** (`Animation.clip` resolves both).
20. **A layer `clip` resolves as a clip, else as a blend of the character** (decision 19 keeps the names
    disjoint). A layer blend is driven by `AnimationLayer.params`, never `Animation.params`. Its phase is a
    `BlendClock.layers` entry `{ name, blend, started, phase, prev }`, integrated by `blend_clocks` exactly as
    decision 15 does, at `layer.speed`, and restarted on a new `(clip, started)`. This happens only while the
    entity's base `Animation` sits on a character `Mesh`. Without a clock, extract falls back to
    `clock_elapsed(started, speed) · rate(params)` (decision 18).
21. **`reference: <clip>`** (additive only) takes the reference pose from that clip's first frame. Without it,
    the reference is the layer's own first frame: for a blend, each point at `t = 0` under the current
    weights. An unknown reference skips the layer silently, as an unknown mask does.
22. **Layer events are opt-in with `events: true`.** They fire only while the layer's weight at `now` is above
    0. A plain clip layer is walked **statelessly** over `(T(i − step), T(i)]` on the exact gameplay instant
    `i`, where `T` is the layer's clock times the binding speed and `step` is this tick's gameplay step. The
    window is `[0, T(i)]` when `started` lies after `floor(i − step)`. Windows tile exactly, so nothing
    repeats. A layer started by a system registered *after* `animation_events` loses only an event at exactly
    `t = 0`; register `play_layer` callers before it, as for `Animation`. A raw `speed` write mid-clip may
    skip or repeat one crossing that tick. A blend layer fires its dominant point's events over its
    `BlendClock.layers` window (decision 16). Events carry `ClipEvent.clip` = the layer's clip or blend name
    and `ClipEvent.layer` = the layer name. Base events have `layer: ""`. Order: an entity's base events,
    then its layers' in list order.
23. **`set_layer_params(&mut layers, name, params) -> bool`** writes only on change. The request's `param: N`
    indices into `Animation.params` are not built (decision 13): a layer owns its two params.
24. **`hold` does not step layer blend phases** (decision 17, same trigger).

25. **Assemblies are the game's rig** (owner ruling 2026-11-17, request 0007 §Summary). The shipped skinned
    path stays for skinned games; each later wave lands on assemblies first.
26. **Assembly `masks:` use decision 1's spelling, with `joints:` naming parts**, under `L1137`/`L1138`. The
    request's `except:` is spelled by listing the excluded subtree root with weight 0 under
    `descendants: true`. Resolution walks ancestors, because authored part order is not topological.
27. **Assembly override** blends each channel the layer clip keys toward the layer by `k = weight × mask`,
    verbatim at `k ≥ 1`. A channel it does not key keeps the base, and a part with no track is untouched.
    This is the request's preference; it differs from skins, where a tracked joint's missing channel reads rest.
28. **Assembly additive** measures from the part's **rest**, because a rigid part's sample already is its delta
    from rest (request §2). A `reference: <clip>` replaces rest with that clip's first frame:
    `R = R_base · slerp(I, r⁻¹ c, k)`, `T = T_base + k (c − r)`, `S = S_base · (1 + k (c/r − 1))`, with
    `r = (I, 0, 1)` by default.
29. **Layers on assemblies are presentation only**, read on the base `hold` grid (decision 5). The ragdoll
    switch keeps sampling the base pose alone; trigger: a ragdoll that must start from a layered pose. Layer
    events work on assemblies exactly as decision 22.
30. **`excluded: local | model`** (legs staying planted under a torso lean) is deferred, an owner question.
    Trigger: the owner confirms it, or a layered torso visibly swings the legs.
31. **Assembly `blends:` use decisions 13–19** through the shared parser (`L1137`/`L1138`). Point clips are
    bound in `clips:` and loop. `sync` is among the clip's own events (an assembly has no overlay). A
    point's binding speed is 1. A blend name may not equal a clip binding.
32. **The assembly N-way part blend.** Walk the entries in order: rotation is a running slerp `w / accumulated`,
    a missing rotation read as identity; translation and scale are weighted sums, missing read as 0 and 1; a
    channel **no** entry keys stays `None`, so an untouched part keeps its exact rest. One weight-1 entry is that
    clip's `part_pose` verbatim.
33. **The ragdoll switch samples the base blend** at the switch instant, from the `BlendClock` phase advanced by
    the tick fraction, else the stateless fallback. Layers stay excluded (decision 29).
34. **Layer blends on assemblies follow decision 20** (`BlendClock.layers`, `AnimationLayer.params`). An
    additive blend layer measures from rest unless `reference:` is set (decision 28).

35. **`check-scene` lists an assembly's masks and blends** (points with `at`, `sync`, 2D triangles).
    `render --model` sweeps a blend with `--blend <name>:<p0>[:<p1>]`, where each `p` is a number or `lo..hi`
    swept linearly across the strip (default 5 phases), and stacks `--layer`s
    `<clip>:<weight>[:<mask>[:override|additive]]` (at most 4, `L1670`) over `--clip`, `--crossfade` or
    `--blend`. A sheet's blend phase is the stateless `elapsed · rate` (decision 18), and every assembly phase
    keeps the gap analysis. Refusals are `L1117`, the model sheet's own code.

36. **The `ik:` key replaces decision 10's `IkRig` component**: the rig is authored with the assembly, so a
    chain or look-at names parts, a `masks:` entry and an effector beside them. `L1137`/`L1138`.
37. **Solver and order.** In extract, after layers, on the composed placements (the model sheet and the ragdoll
    switch are untouched): every look-at in name order, then every chain. A chain is a ball at the upper pivot
    plus a hinge at the lower: the closed-form flex ψ* = bend · acos((d² − |u|² − |v|² + 2u∥v∥) / (2|u⊥||v⊥|))
    clamped to the limits, then the shortest-arc swing of the upper part; no pole vector, an out-of-reach
    target leaves the limb straight or folded. The hinge is `hinge:`, else the second part's `joints:` hinge,
    else the humanoid convention (+X; knee −1, elbow +1). A look-at clamps the target's yaw and pitch to
    `limits_deg` and splits the turn over the aim parts by mask weight, root-first. All maths is `f64` through
    `ludion_core::math::f64` and `ludion_scene::slerp`.
38. **`IkTargets`** is a hashed map of entry name → one of `point` (world) or `direction` (`Look`'s
    convention), with the layer weight ramp (`weight`, `from_weight`, `changed`, `fade`); `L1673`.
    Entry-name keys also mean a dead target is a missing key, never a stale id.
39. **Deferrals.** `rate_deg`: a look target's jump visibly snaps (needs presentation state; the request's open
    question). Socket-part targets: request item 0, part sockets. `ground` is W5a (decisions 45–49); a third
    chain part stays deferred.
    IK in the ragdoll switch: a ragdoll that must start from an IK'd pose. **Entity targets**: a target that
    must follow an entity without per-tick writes — it needs registry support for entity references inside a
    map (`L1392`); until then game code writes `point` from the entity's world position each tick it moves.
    Registry support now exists: `EntityRefCollection` (`Plan/ai-behaviour.md` decision 16).

40. **`render --model --look/--ik` pose through the one solver** (`apply_assembly_ik` on the sheet's
    placements, then the gap analysis). `--look [NAME=]YAW,PITCH` takes a number or `lo..hi` per side (`Look`'s
    convention); a range makes the strip's phases look phases (`--strip`, default 5), every one at the same
    `--time`, and a bare `--look` needs exactly one look-at. `--ik NAME=X,Y,Z` is repeatable, names a chain, and
    its point is in **assembly-grid voxels** (the `place:` frame), converted to the solver's bottom-centre frame;
    the scene's `IkTargets` carry the same point in world space (`translation + scale · p`). Both need `--clip`,
    `--crossfade` or `--blend`, assemblies only, and a look range refuses a blend range; every refusal is
    `L1117`, a bad name listing the entries. Without them every report and image is unchanged. `check-scene`
    lists each `ik:` entry (a chain's resolved hinge, a look-at's forward and limits; JSON `"ik"` only when
    non-empty).
41. **A socket's frame is the part's pivot, in voxel units.** The part's placement is in its mesh frame
    (origin at `offset`, rotation about `pivot`), so the socket level is `placement × T(pivot)`; at rest that is
    `T(offset + pivot)` bit for bit. The child's `Transform` is local to it and scaled by the assembly
    entity's `Transform` like any child (the skin-socket contract).
42. **Sim reads the rest socket, from documents only.** `SkinRests` composes `offset + pivot` from the
    committed assembly and part documents (`load_documents`: parse and validate, no mesher, no clip), never a
    clip, mask, layer, `IkTargets` or blend, so a socket child's `world_transform`, a collider on it and every
    `WorldHash` are independent of animation state. The cache is separate from the posed-data one, so a broken
    clip file cannot change a sim read. Posed-sim sockets would need the pose in the sim (amending
    `animation-clips` decision 2).
43. **The draw is the part's final pose through one helper.** `extract::assembly_part_models` is the
    sequence the mesh loop draws parts with (`held_for`, `animation_pose`, `assembly_layer_poses`,
    `part_placements_layered`, then IK); `pose_each` and `posed_joint_world` call it, so a socket cannot
    disagree with the part it rides. `posed_joint_world` accepts an assembly and a part name (an unknown part
    is `Ok(None)`) and meshes the parts once per path on the first host read.
44. **Diagnostics widen, no new codes.** `L1318` is "not a skinned mesh or an assembly" and `L1319` an unknown
    part, its help naming the nearest part; `assembly_part`, `voxel` and primitive parents stay `L1318`.
    Non-goals: IK targets naming a socket part (decision 39 stays deferred), and sockets on `assembly_part`
    entities and prefab parts (trigger: a consumer asks).
45. **Ground feet: authoring.** `ground: true` on a chain; a top-level `ground:` with `max_adjust` (voxels,
    finite, > 0, required) and `root_drop` (bool, default false, omitted when false). `L1137`: wrong types,
    an unknown key in `ground:`, `ground` on a look-at. `L1138`: a `ground: true` chain without a `ground:`
    block (help: add `ground:\n  max_adjust: 3.0`), or a `ground:` block with no grounded chain.
46. **The cast.** Per grounded chain without an active goal, in name order, on the placements after targeted
    IK: `e` = the effector in world (`model · placed[lower] · effector`), `floor` = world y of the assembly
    frame's origin (`model · (0,0,0,1)`), `reach = max_adjust · |model.y_axis|`. One `ray_cast([e.x,
    floor + reach, e.z], [0,-1,0], 2·reach, QueryFilter::new().only_fixed().exclude(id))`; take `hits[0]`; no
    hit or `distance == 0` (the origin is inside something taller than `reach`) leaves that foot alone; else
    `delta = (floor + reach − distance) − floor`, clamped to `±reach`. Measuring from the floor (not the foot)
    keeps an animated foot lift: the target is `e + (0, delta, 0)`.
47. **Dead band.** `|delta| < 1e-3 · voxel` (voxel = `|model.y_axis|`) is no move, so flat ground at the floor
    poses **bit for bit** as without ground; so does a scene with no physics.
48. **`root_drop`.** `drop = min(0, min delta)` over feet that hit; if below the dead band, pre-multiply
    `T(inverse(model) · (0, drop, 0, 0))` onto every part, then solve each grounded foot whose residual
    `delta − drop` is outside the dead band toward its (pre-drop) world target with `chain(…, k = 1)`. Without
    `root_drop`, the drop is 0. A foot is never raised by lowering, and the root is never raised.
49. **Scope.** Visual only, extract only (`assembly_part_models`), static (`only_fixed`) ground as of the last
    completed physics step. No foot pitch to the normal (a third part; trigger: an assembly with a separate
    foot part), no moving-platform ground (trigger: a character must stand on a kinematic body), not in the
    model sheet, the ragdoll switch or skinned characters.
50. **`AnimGraph<S, I>` API and `step` semantics.** `AnimState { id, name, plays, layers: (layer, clip, weight),
    one_shot }`, `AnimGate { None, Interrupts, OnEvent(name), MinTicks(n) }`, `AnimEdge { from: Option<S>, to, when:
    fn(&I) -> bool, fade, gate }`, `AnimGraph { states, edges }` (plain `pub` structs, so a game writes a `static`)
    with `step`, `current` and `describe`. The request's `State`/`Edge` carry an `Anim` prefix because the prelude
    has other `State`-like names. `step` walks `edges` in declaration order; an edge is a candidate when `from` is
    `None` or the current state and `to` is not the current state (no self-transitions, so a `from: None` edge never
    restarts its own target), and is taken when `when` and its gate pass (`MinTicks` reads `game_frame − entered`,
    `OnEvent` reads `AnimationEvents` for the current state's clip). While the current state is `one_shot` and its
    `end` has not fired, only `Interrupts` edges qualify. The first taken edge wins. Entering writes `crossfade` (even
    for an unchanged clip: a state restarts its clip), `AnimGraphState`, and the layers: each of the new state's layers
    is played and faded to its weight over `fade`, each layer only the old state listed fades to `0`. Nothing firing
    writes nothing, so the hash stays quiet between transitions.
51. **`AnimGraphState { state, entered, previous }`** records a stable state *name* (not `Debug` output, not an
    index), `previous` omitted when empty. There is no implicit entry: a game spawns it beside an `Animation` already
    playing that state's clip. A name the graph lacks is a runtime `step` error, not a load error, because the graph
    is Rust and invisible at load. No `SemanticValidate`. A graph is game-side Rust called from the game's own system
    after `animation_events`: no system registration, wire command or scene key.
52. **Errors write nothing.** `L4290` (`ANIM_GRAPH_STATE_INVALID`): no `AnimGraphState`, an unknown state name (help:
    the nearest declared name and the declared list), no `Animation`, or a layer the entity lacks, checked before the
    first write. `L4291` (`ANIM_GRAPH_INVALID`): duplicate state names, duplicate state ids, or an edge naming an undeclared state.
53. **The `CYCLE` swap is an R14 re-record item.** `common::cycle_builder` moving onto a graph puts a hashed
    `AnimGraphState` on the mannequin, which moves every `character-cycle.trace.txt` line and the `.lrp`
    (`ludion-sim/src/hash.rs` rule 4). W3 proves the port instead: `anim_graph_cycle.rs` runs `CYCLE` and a six-state
    graph port side by side and asserts an identical `Animation` every tick. Vigil's `animate_player` on the graph
    (the request's acceptance) waits for the same round. **Trigger:** R14.
54. **The bench is a runnable `#[ignore]` target over five arms** (`empty`, `clip`, `layers`, `ik`, `full`; the
    same 32 placements, each adding one feature), printing the extract median, p99 and delta over `empty` at 1
    and 8 threads. The `full` arm at one thread is gated in-file at 0.5 ms (decision 11, the owner's 2026-11-19
    acceptance) and by an optional `budgets/pinned-<box>-assembly-anim.toml` (`threads = 1`, `extract_ms` at
    about 2x the pinned median, never above 0.5); its `WorldHash` at 1 and 8 threads is asserted equal. The
    fixture figure is the gallery's 10-part humanoid with three IK chains (two feet, a hand) and a look-at,
    where the request's figure is 11 parts with four chains; no assembly was authored for the bench.
    **Owed:** the quiet release measurement and the budget files on the 3090 and the M4.

55. **`$root` rules.** `ROOT_TRACK = "$root"` (`ludion-scene`). `animation::validate` refuses a `scale` channel, a
    translation key with `v[1] != 0` and a rotation key with `v[0]` or `v[2] != 0` (`L1142`, pointer at the key);
    a part named `$root` is `L1138`. No new code. The canonical writer quotes the key; no committed file had one.
56. **Window and clock.** `root_motion_delta(&World, entity, &SystemCtx)` integrates the track over
    `(prev instant, now]` of the exact gameplay clock (`clock_elapsed`, anchor and speed included, not the held pose
    clock). A one-shot clamps, a loop wraps (latest 64 cycles), a held frame or a clip started this tick opens at
    0, a reversed clock reads nothing; windows tile under a scaled clock. Travel accumulates per key segment in the
    frame faced at the segment start; yaw is the unwrapped degrees lerp (validation keeps a turn under 180 per
    segment), no `atan2`.
57. **Crossfade.** The incoming side counts at `fade_weight`, the outgoing `from` at its complement. A blend name,
    unknown name, clip without `$root` or `PoseSnapshot` side contributes nothing; `RootMotion.weight` is the summed
    weight so a game scales its own move by `1 - weight`. `None` when no side contributes.
58. **Frames and reads.** Keys are in the entity's frame at the clip start in assembly units, forward `-Z`; the result
    is scaled by `Transform.scale` x/z and rotated by `Transform.rotation`, the frame `move_character` moves a root
    motor in; yaw is applied as `turn_character(current + yaw)`. The read takes `&World`: `AnimationLibrary` keeps a
    Mutex'd `roots` map (`PairedLibrary` pattern), pre-filled by `timings`' assembly branch, never locked across a
    second model's load, never hashed.
59. **Base only.** Layers and blends contribute no root motion (the game's preferred answer).
60. **Never drawn, never applied.** Extract samples by part name so `$root` is invisible to the pose; no system
    applies the delta. An unconsumed `$root` clip leaves matrices and `WorldHash` bit-identical (test).

## Waves

- **W1 (shipped).** Masks, `AnimationLayers`, composition in extract's `Skin` phase, `L1670`–`L1672`, docs.
- **W2a (shipped).** 1D/2D blend spaces, `Animation.params`, `BlendClock`, `blend_clocks`, blend events
  (decisions 13–19).
- **W2b (sim, shipped).** A layer's `clip` naming a blend, layer `params`, `reference:`, layer `events`
  (decisions 20–24).
- **A1 (shipped).** Assembly `masks:`, override and additive layers in the assembly path, layer events
  (decisions 25–30).
- **A2 (shipped).** Assembly `blends:` and layer blends, sharing the blend parser through `ludion-scene`
  (decisions 31–34).
- **A3 (deferred).** Prefab-part layers and blends, in `prefab_part_poses`. **Trigger:** a prefab-part
  character or prop needs layers or blends. Request 0007 targets assemblies, and a prefab whose root carries
  an assembly `Mesh` already plays through the assembly path.
- **W2c (shipped).** `check-scene` masks and blends, `render --model … --blend/--layer` strips
  (`model_sheet.rs` + CLI; decision 35).
- **W2d (shipped).** The gallery humanoid, `assembly-layers.yaml` and two goldens (macOS twins owed).
- **W3 (shipped 2026-11-20).** `AnimGraph` and `AnimGraphState` (decisions 9 and 50–53). Moving `CYCLE` and
  vigil's `animate_player` onto the graph is an R14 re-record item (decision 53).
- **W4a (shipped).** The `ik:` key, `IkTargets`, look-at and two-part IK in extract (decisions 36–39).
- **W4b (shipped).** `check-scene` lists `ik:`, `render --model --look/--ik`, `GoldenTest.look`/`ik`, and the
  look-at-at-its-yaw-limit golden (decision 40; macOS twin owed).
- **Part sockets (shipped 2026-11-19).** `ParentJoint` naming an assembly part, a child composing
  on that part's final drawn pose (after layers and IK), and `posed_joint_world` for assemblies (decisions
  41–44; the macOS twin of the `assembly-socket` golden is owed to the M4). Lifts decision 39's socket-part
  deferral for assembly parts.
- **W5a (shipped 2026-11-19).** Ground feet on voxel assemblies (decisions 45–49; the macOS twin of the
  `assembly-ground` golden is owed to the M4).
- **W5b (bench shipped 2026-11-21).** The 32-character bench (decision 54); the measurement and the budget
  files are owed. The `ProfileReport` counters (decision 11) ride R14.
- **W6a (shipped 2026-11-21).** The `$root` track and `root_motion_delta` on assemblies (decisions 12 and 55-60).
- **W6b.** Blend-space root motion (points weighted on the `BlendClock` phase; trigger: a locomotion blend whose
  clips carry `$root`), a `check-scene` root row and a `render --model --clip --root-motion` travel strip, characters
  and prefabs (trigger: a skinned or prefab game asks) and the glTF flag (decision 12).

## Owner rulings (2026-11-19)

- Accepted. Lifting `animation-clips` decision 5 ("masked layers, root motion and IK are non-goals"): W1 ships the
  conservative opt-in variant — absent component, identical bytes.
- Accepted. Amending `animation-clips` decision 2 ("the pose is visual-only"): W2's `BlendClock` and W6's root motion
  are sim reads of clip data (the trade decision 15 made for events).
- Accepted. The `AnimGraph` helper against `animation-clips` decision 4 and `voxel-animation` decision 6 (no state
  machine asset): it is Rust, not data.
- Accepted. Lifting `voxel-animation`'s deferral "Assembly parts and prefab parts as socket targets" for assembly parts
  (prefab parts stay deferred).
- Accepted. A socketed prop's collider and every sim read follow the **rest** socket, not the animated part; posed-sim
  sockets would need the pose in the sim (amending `animation-clips` decision 2).
- Accepted. The W5 budget (≤0.5 ms for 32 characters).
- Accepted. Ground `max_adjust` in the assembly's voxel units (the request's proposal).
- Accepted. Foot placement ships visual-only, amending `Plan/voxel-animation.md` decision 2 (trigger fired: ramps and
  stairs).
- Accepted. An active `IkTargets` goal on a grounded chain wins over ground.
- Accepted, scheduled (its own arc after the Neon Shroud queue empties). `animation-clips` decision 10's merge trigger fired (a character feature assemblies also want); merging
  `kind: character` and `kind: assembly` is not done here.

## Non-goals

- Layers and blends on prefab parts, and `render --blend/--layer` on a prefab. **Trigger:** A3 (a prefab-part
  character or prop needs layers or blends).
- Per-layer anchors (`set_clip_speed` for a layer). **Trigger:** a layer clip that must change rate mid-clip.
- Layer-to-layer blending (a crossfade inside one layer). **Trigger:** a layer whose clip switch visibly pops.
- A masked crossfade of the base. **Trigger:** a base switch that must not touch the upper body.

## Risks

- **Per-frame cost.** A layer is another sampler pass per skinned entity per frame. Cap 4, and `joints_sampled`
  counts `joints × (1 + fading + layers)` so the profile shows it; the W5 bench holds the line.
- **A layer clip names joints the mesh lacks.** The character's clips are checked at load, so this cannot
  happen for a bound clip; an unknown clip or mask name is a silent degrade, as for `Animation`.
- **Weight ramp on the hold grid.** A held base steps the layer weight with it; that is intended (stop-motion
  reads as one cadence).
