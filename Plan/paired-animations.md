# Plan: Paired animations and a posed ragdoll — rest-relative zeros, part velocity, the way back, `kind: paired`

> Status: **W1–W4 mechanized; W5a mechanized (fit: 0008-05; run: 0008-06); W5b and W6 mechanized (0008-08, 0008-07).** Driver: Neon Shroud request 0008
> (`engine-requests/0008-paired-animations-and-posed-ragdoll.md`). A takedown aligns two voxel assemblies,
> plays one clip on both, and ends with the target ragdolled at the pose it died in; the way back
> (get-up) re-poses an assembly from a ragdoll. The design below was written from the tree on 2026-11-18,
> so re-grep before editing.

## Status

### W1 — rest-relative posed voxel switch (`JointZero`, `switch_to_ragdoll_with`), L1750

| file | role |
|---|---|
| `crates/ludion-spatial/src/joint.rs` | `Joint.zero` (R13; W1 shipped it as `JointZero { rotation }` in `joint_zero.rs`), `SemanticValidate`, `L1750` at `/zero` |
| `crates/ludion-core/src/codes.rs` | `JOINT_ZERO_INVALID` (`L1750`) and its explain entry |
| `crates/ludion-components/src/lib.rs` | `JointZero` registered last (registry 80), sim-only; removed in R13 |
| `crates/ludion-physics/src/joints.rs`, `world.rs`, `systems.rs` | `build_generic_joint` reads `joint.zero`, reconcile compares the whole `Joint` (W1 wrapped it in a `JointSource { joint, zero }`, removed R13; same JSON bytes as a bare `Joint` when `zero` is `None`) |
| `crates/ludion-engine/src/ragdoll.rs`, `lib.rs`; `crates/ludion/src/prelude.rs` | `RagdollSwitch`, `LimitsFrom`, `switch_to_ragdoll_with`; layers in the sampled pose |
| `crates/ludion-physics/tests/joint_zero.rs`, `crates/ludion-engine/tests/ragdoll_rest.rs` | the zero, rebuild, live add, blob, motor, `JointPoses`, snapshot, threads, layered proofs |
| `docs/physics-joints.md`, `docs/voxel-assemblies.md`, `crates/ludion-engine/examples/ragdoll_switch.rs` | §Rules, §Ragdolls, `--limits-from-rest` |

**R13 (API break):** `JointZero` folded into `Joint.zero`; no hash, schema, trace, golden or replay moved (a zero-less `Joint` serializes as before).

A scene, replay or game that never sets a zero and calls `switch_to_ragdoll` poses and hashes as
before: no `WorldHash`, replay, trace, physics blob, wire-schema or golden moved. Only
`component_schemas.json` re-blessed. The one behaviour change is decision 4: a layered assembly's switch
now samples its `AnimationLayers`; no game or replay called `switch_to_ragdoll` on a layered assembly, and
without layers the sampled list is empty, so the output is unchanged by construction.

### W2 — per-part clip velocity at the posed switch (`RagdollSwitch.part_velocity`)

| file | role |
|---|---|
| `crates/ludion-engine/src/extract.rs` | `animation_pose_at` and `assembly_layer_poses_at` (`now`, `then`, `back`); `animation_pose` and `assembly_layer_poses` are the `back == 0` wrappers |
| `crates/ludion-engine/src/ragdoll.rs` | `RagdollSwitch.part_velocity`; the earlier placements, the linear and angular finite difference (`rotation_rate`, f64 libm) |
| `crates/ludion-engine/tests/ragdoll_velocity.rs`, `tests/it.rs` | drawn one-tick difference for a clip, a live blend, a crossfade, clip and blend layers; time-scaled, held, rest figure, default bit-identity, 1/2/8 threads |
| `docs/voxel-assemblies.md` (+ embedded mirror), `crates/ludion-engine/examples/ragdoll_switch.rs` | §Ragdolls, `--part-velocity` |

With `part_velocity: true` each part's `Velocity` adds `(center_now − center_then)/back` and the shortest-arc
rotation rate to the root's rigid velocity, where `then` is one gameplay tick before the switch (the window
shrinks before tick 1; zero length adds nothing). The default (`false`) never calls the new functions and the
wrappers run today's arithmetic (`back` guards are `> 0.0`), so no replay, `WorldHash`, trace or golden moved;
nothing is blessed. A hold does not reach physics (`docs/game-clock.md`), so held parts keep falling; the
held switch's velocity equals the unheld one's at the same gameplay tick.

### W3 — ragdoll → assembly (`switch_ragdoll_to_assembly`, `ragdoll_landing`, `PoseSnapshot`), L1751

| file | role |
|---|---|
| `crates/ludion-components/src/pose_snapshot.rs`, `lib.rs` | `PoseSnapshot { started, parts }`, `SnapshotPart`, `SemanticValidate` (`L1751`); registered last (registry 85), not sim-only |
| `crates/ludion-core/src/codes.rs` | `POSE_SNAPSHOT_INVALID` (`L1751`), its explain entry and list row |
| `crates/ludion-engine/src/extract.rs` | `AssemblySide` gains a snapshot side; `animation_pose_at` uses it as the outgoing side of a bound fade (decision 12); `animation_pose` and `animation_pose_at` borrow `&'a World` |
| `crates/ludion-engine/src/ragdoll.rs` | `mesh_bounds` crate-visible; the sampled translations hoisted before the spawns; `switch_to_ragdoll_with` removes a `PoseSnapshot` |
| `crates/ludion-engine/src/ragdoll_recover.rs`, `lib.rs`; `crates/ludion/src/prelude.rs` | `RagdollToAssembly`, `ragdoll_landing`, `switch_ragdoll_to_assembly`; re-exports |
| `crates/ludion-engine/tests/ragdoll_recover.rs`, `tests/it.rs`, `tests/registry_serialized_default.rs` | round trip at scale 1 and 2, fade end, hard cut, face up and down, landing query, retired parts, every refusal, the binding, snapshot restore, 1/2/8 threads, motor revive |
| `docs/voxel-assemblies.md`, `docs/voxel-animation.md` (+ embedded mirrors), `crates/ludion-engine/examples/ragdoll_switch.rs` | §Ragdolls "Back to the assembly", §From game code, `--recover` |

A scene, replay or game that never calls the new function poses and hashes as before: `switch_to_ragdoll`'s
extra removal is a no-op without a `PoseSnapshot`, the hoisted translations are the same values, and extract
reads the snapshot only for a bound fade. No `WorldHash`, replay, trace, wire-schema or golden moved. Only
`component_schemas.json` re-blessed.

### W4 — the `kind: paired` document, `check-scene` contacts and warp report, L1752–L1755

| file | role |
|---|---|
| `crates/ludion-scene/src/paired.rs`, `lib.rs`, `kinds.rs` | `PairedDocument` and friends, parse (`L1752`), validate (`L1753`), canonical emit; `parse_paired`/`validate_paired`/`check_paired`/`emit_paired`; the `paired` `KINDS` row |
| `crates/ludion-core/src/codes.rs` | `PAIRED_DOCUMENT_STRUCTURE`, `PAIRED_DATA_CONSISTENCY`, `PAIRED_ROLE_MISMATCH`, `PAIRED_CONTACT_GAP` (`L1752`–`L1755`), explain entries and list rows |
| `crates/ludion-engine/src/paired_doc.rs`, `lib.rs` | `check_paired_files`, `PairedReport`: role assemblies loaded, clips, event times and contact parts checked (`L1754`), gaps measured on posed parts (`L1755`) |
| `crates/ludion-cli/src/paired_report.rs`, `check_scene.rs`, `main.rs` | the `check-scene` text and `--json` report |
| `examples/gallery/voxels/choke.paired.yaml`, `crates/ludion-engine/tests/paired_doc.rs` + fixtures, `crates/ludion-cli/tests/check_scene_paired.rs` + fixtures | the gallery document and the proofs |
| `docs/paired-actions.md`, `docs/voxel-animation.md` (+ embedded mirrors) | the topic and its pointer |

Nothing plays a paired action yet. No component, `WorldHash`, replay, trace, wire-schema, schema snapshot or
golden moved; nothing was blessed.

### W5a — paired fit (`IgnoreCollisions`, `PairedLibrary`, `paired_fit`), L4370–L4373

| file | role |
|---|---|
| `crates/ludion-physics/src/motor/ignore.rs`, `mod.rs`, `lib.rs`, `mover.rs`, `resize.rs`, `suspend.rs`, `posture.rs`, `turn.rs`, `queries.rs`, `sweep.rs` | `IgnoreCollisions`, `QueryFilter::ignore_partner`, the `partner` on `SweepFilter` and `motor_filter`, `character_fit_ignoring` |
| `crates/ludion-physics/tests/motor_ignore.rs` | fit, move, bit-identity and blob proofs |
| `crates/ludion-engine/src/paired.rs`, `engine.rs`, `lib.rs`, `motor.rs` | `PairedLibrary`, `aligned_targets`, `paired_fit`, `Warp`, `PairedFit`; the `character_fit_ignoring` wrapper |
| `crates/ludion-core/src/codes.rs` | `L4370`–`L4373` |
| `crates/ludion-components/src/lib.rs`, `ludion-testkit/src/drift.rs`, `registry_serialized_default.rs`, the facade prelude | `IgnoreCollisions` registered (sim-only), seeds, exports |
| `crates/ludion-engine/tests/paired_fit.rs` + `fixtures/paired/three-roles.paired.yaml` | the fit proofs |
| `docs/paired-actions.md`, `docs/motor.md` (+ mirrors) | §Can it start?, the `IgnoreCollisions` paragraph |

No `WorldHash`, replay, protocol schema, `PHYSICS_BLOB_VERSION` or golden moved. Only `component_schemas.json` re-blessed. The M4 cross-arch twin of the aligned-pose test is owed.

### W5a run — `PairedAction`, `start_paired`, `paired_motion`, `paired_update`, `abort_paired`, L4374–L4375

| file | role |
|---|---|
| `crates/ludion-components/src/paired_action.rs`, `lib.rs` | `PairedAction` (hashed, registered last, not sim-only) |
| `crates/ludion-engine/src/paired.rs`, `animation_events.rs`, `lib.rs` | `resolve`, `start_paired`, `paired_motion`, `paired_update`, `abort_paired`, `PairedMotion`; `AnimationEvents::push` |
| `crates/ludion-scene/src/paired.rs` | `end` reserved (`L1753`) |
| `crates/ludion-core/src/codes.rs` | `L4374` (warning), `L4375` |
| `crates/ludion-engine/tests/paired_run.rs`, `examples/paired_action.rs` | the run proofs; the headless example |
| `docs/paired-actions.md` (+ mirror) | §Playing it |
| `ludion-testkit/src/drift.rs`, `registry_serialized_default.rs`, the prelude | seed, fixture, exports |

`paired_update` is game-registered (decision 32). No `WorldHash` of a scene that never starts an action, replay, protocol schema, `PHYSICS_BLOB_VERSION` or golden moved. Only `component_schemas.json` re-blessed. The M4 cross-arch twin of the warp test is owed.

## Trigger

The game's takedowns, chokes and neck-breaks end in a collapse, and its collapse already calls
`switch_to_ragdoll`. Limits, `JointMotor` targets and `JointPoses` then count from wherever the body was
when it died, so a knee switched at −40° has −40° as its zero. Request 0008 asks for rest-relative zeros.

## Decisions

1. **A stored zero (amended R13: `Joint.zero`, the request's shape).** W1 shipped it as a `JointZero` carrier
   component beside `Joint`, because `Joint` is a plain pub struct that external games build with struct literals.
   The owner ruled 2026-11-19 that a better engine API beats an external game building unchanged, so R13 folded it
   into `Joint` as `zero: Option<[f32; 4]>` (absent from the text and the physics blob when `None`) and removed `JointZero`.
2. **`LimitsFrom::{SwitchPose (default), Rest}`.** `SwitchPose` is `switch_to_ragdoll` exactly. `Rest`
   sets `Joint.zero` to the identity on each joint (an assembly's parts have no rest rotation, so
   the rest relative rotation is the identity for every joint). `RagdollSwitch` is `#[non_exhaustive]` and
   built from its default, so W2's `part_velocity` field is additive.
3. **A switch pose outside a rest limit is clamped by the solver on the first step**, a bounded, documented
   snap. It is not refused (the game's preferred answer).
4. **Layers are in the sampled pose**, in both paths. Look-at, IK and ground placement are excluded: not
   built on assemblies, and presentation-only. This supersedes `Plan/animation-layers.md` decisions 29 and 33
   for layers (trigger fired: a ragdoll that must start from a layered pose).
5. **`hold` and the scale channel are ignored** (`Plan/animation-clips.md` d19).
6. **Shapes differ from the request:** `game_time` instead of `ctx`, the `Result<_, Diagnostic>` return of
   `switch_to_ragdoll` (and, until R13, `JointZero` instead of `Joint.zero`).
7. **The stored-zero math.** `Joint.zero` (W1: `JointZero.rotation`) is `q_a⁻¹·q_b`, xyzw. The anchors keep coinciding at the
   bodies' current poses; only the rotation of the second frame comes from the zero:
   `frame2 = (pb⁻¹·pa·frame1).translation` with rotation `r0⁻¹·frame1.rotation`. A non-finite or
   near-zero quaternion is treated as absent (`L1750` on the validated paths).
8. **The physics source is `JointSource { joint, zero }`**, compared as a whole every tick, so adding,
   editing or removing a zero rebuilds the joint. With no zero its JSON bytes are exactly the bare `Joint`'s,
   so the physics blob does not change and `replay_content_hash` does not move.
9. **The earlier velocity sample is a closed form, not `BlendClock.prev` (approved by the Tech Lead).** A
   live phase sampled at `now − 1 tick` reads the same `fraction` from `floor(now)`, so it would equal the
   current phase and a blend's velocity would read zero. The earlier sample therefore winds the live phase
   back by `rate · speed · back` on the current params, with `back` the window in gameplay seconds; clip
   sides, crossfade weights and stateless blends are evaluated at `then`. `prev` is rejected: it is one
   *sim* tick back (not one gameplay tick under a time scale or a hold), the outgoing side of a crossfade
   has none, and it is `-1` on a restart tick. Exact for constant params, the common case.
10. **The placement rule (W3).** Let `q_r` be the first parentless part's world rotation and `c` its
    pivot in world space (`p_r + q_r·pivot·s`). `face_up` is `(q_r·−Z).y > 0`; `root_yaw` is
    `atan2(−f.x, −f.z)` for `f` the head (`q_r·+Y`) projected on the ground (the chest's when the head
    points straight up or down), so the entity's `−Z`, the motor's facing, lies along the head; `feet` is
    `[c.x, lowest, c.z]`. The root `Transform` is written exactly as `resume_character(feet, root_yaw)`
    writes it (a Y quaternion through `quat_to_euler_xyz_degrees`), so a revive does not move it. All of it
    is f64 scalar on libm, never glam's `normalize` or `inverse`, so it matches across x86-64 and arm64.
11. **`feet` is the lowest part corner, not a raycast.** The lowest world `y` over every part's eight box
    corners needs no physics world (a retired part has no collider and a despawned body is still in the
    world until the next step), the body cannot hit itself, and the result is a function of the parts alone.
    A game that wants the floor raycasts from `feet` itself; the landing is the engine's own rule.
12. **The `PoseSnapshot` binding (replaces the request's `from: "$pose"`).** The snapshot is the outgoing
    side of the entity's crossfade exactly while `animation.from` is empty, `animation.fade_started ==
    snapshot.started`, and the fade is active; otherwise it is inert (the `ClipAnchor` idiom). The switch
    writes `started` and `fade_started` as one frame. No sentinel (pillar 6: one way to name a fade source),
    and no removal system, which would need a schedule edit and, until it ran, would let a game's
    `crossfade()` fade from a stale pose. The next `crossfade()` (a non-empty `from`, a new `fade_started`)
    or a hard cut retires it by construction. The next `switch_ragdoll_to_assembly` overwrites it and
    `switch_to_ragdoll_with` removes it (a no-op when absent, so the default stays byte-identical); an
    inert snapshot between those two points is documented hashed state. Edge: a `crossfade()` on the same
    tick as the switch, from an empty clip, matches the snapshot; call it a tick later. The snapshot's
    side has no outgoing blend clock (`blend_clock` skips an empty `from`).
13. **`ragdoll_landing` is a pure query,** returning the `RagdollToAssembly` the switch returns, because
    `face_up` must be known before the get-up clip is chosen. `root_yaw` is in radians so `feet` and
    `root_yaw` feed `resume_character(world, e, feet, yaw, ..)` directly.
14. **Refusals and what stays game code.** `L3036` through `ragdoll::switch_invalid`, nothing mutated: the
    entity dead, without `Transform`, parented, still holding a `Mesh`, or non-uniformly scaled; a part
    list of the wrong length, a dead or `Transform`-less part, a part whose `Mesh` is not an
    `AssemblyPart` or names another part or assembly, a part mesh with no vertices; an unknown clip (with
    the nearest name). The switch does not resume the motor, re-insert a `Collider` or `Velocity`, or touch
    `PoseBlend`: the game does, after the get-up. The despawned parts leave the physics world on the next
    physics step, so `resume_character` (or a `Collider` insert) is called from the next tick on; on the
    same tick their bodies still block the capsule. The rustdoc and `docs/voxel-assemblies.md` say so.

15. **Document shape.** Top level `format: 1`, `kind: paired`, `name` (required: W5's events fire as
    `paired:<name>`), `roles`, `align`, `events`, `contacts`. `roles` is a mapping (at least 2 entries,
    sorted on emit) of `{ assembly, clip, align_only, posture, scale }`. `align` is `{ reference, moves,
    ticks, offsets }`, all four required, where `offsets` maps every non-reference role to `{ translation:
    [x, y, z] m, yaw_deg }` in the reference entity's frame (−Z forward). `moves` is a role name or `split`.
    Differences from the request: `align.offsets.<role>` replaces `align.target` (N roles, and the request's
    example role is itself named `target`); `moves` names a role instead of the fixed `instigator | target`;
    `scale` is new (decision 19); events are `{ name, t }` entries and contacts `{ a, b, at, max_gap }`.
16. **Clip roles only.** `clip` names a key of the assembly's `clips:`; a 0007 blend needs parameters the
    document has no place for. Trigger: a paired action that needs a parametric clip.
17. **No prefab roles yet;** `align_only: true` (no `clip`) covers the search spot. Trigger: the locker
    stash (a prefab door with node clips).
18. **Two passes.** `ludion-scene` checks the file alone (`L1752` shape, `L1753` data); `ludion-engine`'s
    `check_paired_files` loads each role's assembly and checks against it (`L1754`) and measures the
    contacts (`L1755` warning). The `KINDS` row is `doc_store: false`: nothing loads a paired document at
    boot yet; W5 decides how `start_paired` resolves one.
19. **`scale`** (optional, above 0, default 1) is the role entity's uniform `Transform` scale: assembly
    cells are 1 unit, offsets are metres, so a contact gap is meaningless without it.
20. **Event time** is gameplay seconds since the shared `started` at speed 1; events strictly ascend with
    unique names, and `t` is at most the `duration` of every clip role's clip.
21. **Gap and warp metrics.** A role entity sits at `T(offset) · R_y(yaw) · S(scale)` (the reference at
    identity); each part cell's centre goes through its part's `posed · rest⁻¹` at the event's `t` (rest for
    an `align_only` role). `gap = max(0, min centre distance − (s_a + s_b)/2)`, reported in metres and in
    role `a`'s cells (`/ s_a`); 0 means touching or overlapping; it is cell-quantized like the model sheet's
    posed gaps. The warp report is each role's aligned offset, yaw, planar distance from the reference, the
    mover and `ticks`; its start pose is runtime, so the path from it is W5's `paired_fit`.
22. **`IgnoreCollisions { with }`.** A hashed `ludion-physics` component: the motor's casts and fit tests
    ignore `with`'s collider. It is read at query time and is not in the physics blob. Absent means
    bit-identical. The solver pair filter for dynamic bodies rides `Plan/character-controller.md` decision 28
    (carry). It holds a list of at most four entities (`EntityRefCollection`, decision 37).
23. **Document resolution (answers decision 18).** `paired_fit` and `start_paired` take the document's
    content-root path. A non-hashed `PairedLibrary` loads it lazily through `check_paired_files` (the
    `AnimationLibrary` rule: rebuilt at every world build; `asset.reload` does not reach it). There is no
    builder opt-in, and the `KINDS` row stays `doc_store: false`.
24. **The aligned pose.** `moves: <role>` moves that role onto the other; `split` meets at the pair's
    midpoint, preserving the mean feet position and the mean yaw. Planar only: feet height is kept.
25. **`paired_fit`.** The cast must be exactly the document's roles; two to five roles; more is `L4373` (decision 41).
    Entity checks are `L4371`, the scale check `L4372`. Warp: each moving role's planar distance and absolute
    yaw change against `max_warp`. Every motor role is fit-tested at its target pose and role `posture`,
    ignoring its partner; a role without a motor is not fit-tested. Nothing is mutated.
26. **Differences from the request.** `Warp { distance, yaw_deg }`; `PairedFit` adds `over_warp` and `roles`.

27. **`PairedAction { document, action, role, reference, started, fired }`.** One ref per participant; the
    partners are the entities with the same `(document, reference, started)`. `fired` counts the document
    events this participant has fired.
28. **`start_paired(world, paired, cast, fade, game_time) -> Result<(), Vec<Diagnostic>>`.** It takes
    `game_time` rather than `ctx` (decision 6) and does not re-test the fit: call `paired_fit` first. Order:
    validate everything; set postures, rolling back if one is blocked; end any earlier role of a cast entity
    (chaining); insert `IgnoreCollisions` both ways and `PairedAction`; crossfade each clip role to its clip
    at speed 1. An `align_only` role's `Animation` is left alone.
29. **`paired_motion(world, entity, ctx) -> Option<PairedMotion { displacement, yaw, warping }>`.** A
    closed-loop smoothstep warp toward the aligned pose recomputed each tick, held after the warp (`f = 1`
    once complete; `f = 0` only on a held tick). `None` off the action; a zero step for a non-mover or a gone partner.
30. **`paired_update`.** Events fire once per participant as `ClipEvent { clip: "paired:<action>" }` in
    ascending entity order after the clip events, by the `fired` index: never lost, never twice. A `t: 0`
    event fires on the start tick when `start_paired` runs before the system, else one tick later. The end is
    the longest clip role's duration (`ticks` for `align_only`-only documents): the remaining events and an
    implicit `end` fire and every `PairedAction` and `IgnoreCollisions` go on that tick. `end` is reserved
    (`L1753`). A participant whose `Animation` or assembly `Mesh` is gone ends its own role only. A changed
    `clip`, `started` or `speed` aborts the whole action and warns `L4374` once per entity; the guard does
    not name the writer (W5b).
31. **`abort_paired(world, entity) -> Result<Vec<(String, EntityId)>, Diagnostic>`.** Ends every
    participant now and returns the cast so the game crossfades; no `fade` or `clip_for` (W5b). `L4375` for
    an entity not in an action.
32. **`paired_update` is game-registered, not engine-scheduled.** It must run after `animation_events`,
    which rewrites `AnimationEvents`, and before any reader. A default system would change every game's
    schedule and traces, including games that never pair (the `animation_events` and `play_patterns` precedent).

33. **`render --paired <doc>`.** The roles stand at their aligned pose (decision 21), one cell per phase.
    A phase is an instant: the document events by default (`[0]` with none), `--strip N` (2 to 16) evenly over
    `[0, end]`, or `--time T`. A clip role is posed statically through `Animation { started: 0, speed: 1 }`
    extracted at `quantize(t)`. The refusals are `L1756`, and the default view is `iso` (one `--views` value).
34. **Framing.** The union of each role's assembly AABB under its `T·R·S`, padded 15% for posed limbs, normalized
    through `Subject::from_box`, so every cell shares one camera and the roles keep their relative scale.
35. **The gallery takedown.** `paired-takedown.yaml` fits, warps and starts `takedown.paired.yaml` (events `grab`
    0.25, `quiet` 0.6, `release` 0.9) on two motor figures, the instigator 0.4 m and 20° off. On `quiet` the
    guard ragdolls with `LimitsFrom::Rest` and gets up 90 ticks later through `switch_ragdoll_to_assembly`
    (`idle`, 0.5 s). The game half is `takedown_builder` in `ludion-testkit/tests/common/mod.rs`.
36. **No `render --paired` golden.** Trigger: the first regression the driven goldens miss.

37. **`IgnoreCollisions.with` is a list of at most 4 entities** (`MAX_PARTNERS`; inline, no allocation, no lifetime on
    `SweepFilter`, which stays `Copy`). Each participant ignores every other. `SemanticValidate` refuses more than four
    with `L1759`; at runtime the first four count. `end_role` removes the leaver from every member's list and drops
    an emptied component; `paired_update` prunes dead entries, writing only when one is dead.
38. **The N-role frame rule.** `moves: X`, X not the reference: the frame is the reference's current pose, X goes
    to its offset and the rest hold. `moves: <reference>`: the frame is the mean implied reference pose of every
    other role (yaw by wrapped mean around the first voter's yaw), and only the reference moves. `split`: the mean
    of every role's implied pose, every role moves. One voter is exact, so two-role results are unchanged. A role
    that left drops out of the mean; with no voter left every present role holds.
39. **Root motion.** A role whose clip carries `$root` is driven by it whenever it is not warping: a non-mover from the
    first tick, a mover from the tick after its warp (travel inside the warp is dropped); no `$root` keeps the hold.
    `paired_motion` reads unrecorded through `root_motion_of`; a paired entity takes motion only from it.
40. **`abort_paired(world, entity, to, fade, game_time)`.** `to` is `(role, clip)` pairs validated before any mutation
    (unknown or repeated role: `L4371`); those roles crossfade in the same call. An empty `to` is W5a's behaviour.
41. **`L4373` stays**, re-worded: more than five roles (a motor looks through at most four partners).
42. **Deferred, with triggers.** The lockstep guard naming the writer system (needs an ECS write-attribution hook;
    trigger: a lockstep abort a game cannot trace from the `L4374` entity alone). `moves` as a list of roles (trigger:
    a document where two non-reference roles travel while the reference holds). Prop roles (16) and blend roles (17)
    are unchanged: a locker is a `kind: assembly` clip role today.

## Waves

- **W1 (shipped).** `JointZero`, `LimitsFrom`, `RagdollSwitch`, `switch_to_ragdoll_with`, layers in the
  switch pose, `L1750`, docs and the example flag (decisions 1–8).
- **W2 (shipped).** `part_velocity`: a two-sample finite difference of the drawn pose, the earlier sample a
  closed form (decision 9).
- **W3 (shipped).** Ragdoll → assembly: `switch_ragdoll_to_assembly`, `ragdoll_landing`, `PoseSnapshot`,
  the placement and face-up rule (decisions 10–14).
- **W4 (shipped).** The `kind: paired` document: the `KINDS` row, parse and validate, `L1752`–`L1755`,
  `check-scene` contacts and warp report (decisions 15–21). 0008's runtime range L4370–89 stays for W5.
- **W5a.** Fit (0008-05): `IgnoreCollisions`, `PairedLibrary`, `paired_fit`. Run (0008-06): `PairedAction`,
  `start_paired`, `paired_motion`, `abort_paired`, `paired_update` with events, end and the lockstep guard.
- **W5b (mechanized).** N roles (2 to 5), `$root` motion after the warp, `abort_paired` crossfading to clips
  (decisions 37–42); prefab and blend roles and the writer-naming guard stay deferred.
- **W6 (mechanized).** `render --paired`, the `paired-takedown` gallery scene and goldens (decisions 33–36).

## Open questions, with the game's answers

- **Who moves during the warp?** `instigator` for takedowns (the guard must not slide toward the player),
  `split` for scripted beats.
- **Max warp.** 0.6 m and 45° for takedowns; beyond that the prompt is not offered.
- **Posture per role.** Roles may declare a `posture:` (0006) that `paired_fit` tests and `start_paired` sets.
- **More than two roles.** N roles with one `reference` (the locker stash needs three).
- **Event clip name.** `paired:<action>`, so both sides test the same string.
- **A switch pose outside a rest limit.** Clamp on the first step (decision 3).
- **Look-at in the switch pose.** Excluded with the IK (decision 4).
- **Placement and face-up rule** for the way back: the root part's +Y heading and −Z chest is the game's
  preferred rule; decision 10 is the engine's own, as documented.
- **`LimitsFrom::Rest` as the default.** Not flipped here: it would move existing voxel ragdoll hashes.

## Owner rulings (2026-11-19)

- Shipped R13 (accepted 2026-11-19; the owner fixes the external games by hand, vigil needed no change). `JointZero` is
  folded into `Joint` (`Joint.zero`, the request's shape); `Joint` literals gain `zero: None`.
- Accepted, read conservatively: `LimitsFrom::Rest` is the documented choice for new content, and the serde default is not flipped, so existing voxel ragdoll hashes stay put.
- Accepted. The request's three plan amendments: `Plan/physics.md` W3 joints and the pose-blend rule (`Joint` gains an
  optional stored zero, here as a component); `Plan/animation-clips.md` decision 14 (the outgoing crossfade
  side may be a `PoseSnapshot`); `Plan/document-kinds.md` (`kind: paired` as one more `KINDS` row).

## Non-goals

- A skinned rest-relative switch: `switch_skin_to_ragdoll` already switches at the rig rest pose.
- Look-at, IK and ground placement in the switch pose.
- Flipping the `LimitsFrom` default. (The `Joint` field change, folding `JointZero` into `Joint.zero`, shipped in R13.)

## Risks

- A quaternion zero beyond a limit snaps on the first step; a game that wants no snap clamps its switch
  pose first.
- The M4 cross-arch twin of the new tests is owed.

### W5b — N roles, `$root` motion, `abort_paired` to clips, L1759

| file | role |
|---|---|
| `crates/ludion-physics/src/motor/ignore.rs` and `mover.rs`, `resize.rs`, `suspend.rs`, `posture.rs`, `turn.rs`, `queries.rs`, `sweep.rs` | `IgnoreCollisions.with: Vec<EntityId>`, `MAX_PARTNERS`, inline `Partners` on `SweepFilter`, `character_fit_ignoring(.., Option<&[EntityId]>)`, `L1759` |
| `crates/ludion-engine/src/paired.rs`, `root_motion.rs` | N-role `aligned_targets`, `resolve` (five roles), `start_paired`, `end_role`, dead-entry pruning in `play_group`, `paired_motion` root travel, `abort_paired(.., to, ..)` |
| `crates/ludion-core/src/codes.rs` | `L1759`, `L4373` re-worded |
| `crates/ludion-engine/tests/paired_run.rs` | three roles, a leaver and a despawn, abort to clips, root travel of a non-mover and a mover, restore and thread counts |
| `docs/paired-actions.md`, `motor.md`, `voxel-animation.md` (+ mirrors) | N roles, the frame rule, root motion, abort `to`, the codes |

No `WorldHash` of existing content, replay, protocol schema, `PHYSICS_BLOB_VERSION` or golden moved. Only `component_schemas.json` re-blessed. The M4 twin of the N-role warp test is owed.

### W6 — `render --paired`, the `paired-takedown` gallery scene and goldens, L1756

| file | role |
|---|---|
| `crates/ludion-engine/src/paired_sheet.rs`, `model_sheet.rs`, `lib.rs` | `render_paired_sheet`, `PairedSheetRequest`/`Outcome`/`Phase`; the model-sheet pieces widened to `pub(crate)` |
| `crates/ludion-core/src/codes.rs` | `PAIRED_RENDER_INPUT` (`L1756`), explain entry and list row |
| `crates/ludion-cli/src/main.rs`, `render.rs`, `paired_report.rs` | `--paired`, `run_paired`, `print_contacts` shared with `check-scene` |
| `crates/ludion-cli/tests/render_paired.rs` | phases, strip, time, contact line, `L1756`, the clap conflict |
| `examples/gallery/paired-takedown.yaml`, `voxels/takedown.paired.yaml`, `voxels/guard.assembly.yaml` | the scene, its document and the khaki guard |
| `crates/ludion-testkit/tests/common/mod.rs`, `paired_takedown.rs`, `gallery.rs` | `takedown_builder`, the CPU proof, three driven goldens |
| `examples/gallery/paired-takedown.trace.txt`, `goldens/windows/paired-takedown{,-grab,-ragdoll,-getup}.png` | the committed trace and four new goldens, blessed on the 3090 |
| `docs/paired-actions.md` (+ mirror) | §Looking at it, the `L1756` row |

No sim, component, wire-schema, replay or existing-golden change. Owed: the macOS twins of the four goldens
and the M4 run of the trace; the 0018 porosity change moves these goldens along with the existing assembly ones.
