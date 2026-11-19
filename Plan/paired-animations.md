# Plan: Paired animations and a posed ragdoll — rest-relative zeros, part velocity, the way back, `kind: paired`

> Status: **W1–W3 mechanized (2026-11-18); W4–W6 not started.** Driver: Neon Shroud request 0008
> (`engine-requests/0008-paired-animations-and-posed-ragdoll.md`). A takedown aligns two voxel assemblies,
> plays one clip on both, and ends with the target ragdolled at the pose it died in; the way back
> (get-up) re-poses an assembly from a ragdoll. The design below was written from the tree on 2026-11-18,
> so re-grep before editing.

## Status

### W1 — rest-relative posed voxel switch (`JointZero`, `switch_to_ragdoll_with`), L1750

| file | role |
|---|---|
| `crates/ludion-spatial/src/joint_zero.rs`, `lib.rs` | `JointZero { rotation }` (default identity), `SemanticValidate`, `L1750` |
| `crates/ludion-core/src/codes.rs` | `JOINT_ZERO_INVALID` (`L1750`) and its explain entry |
| `crates/ludion-components/src/lib.rs` | `JointZero` registered last (registry 80), sim-only |
| `crates/ludion-physics/src/joints.rs`, `world.rs`, `systems.rs` | `JointSource { joint, zero }` (same JSON bytes as a bare `Joint` when `zero` is `None`), `build_generic_joint(.., zero)`, the gather reads the carrier's `JointZero`, reconcile compares the whole source |
| `crates/ludion-engine/src/ragdoll.rs`, `lib.rs`; `crates/ludion/src/prelude.rs` | `RagdollSwitch`, `LimitsFrom`, `switch_to_ragdoll_with`; layers in the sampled pose |
| `crates/ludion-physics/tests/joint_zero.rs`, `crates/ludion-engine/tests/ragdoll_rest.rs` | the zero, rebuild, live add, blob, motor, `JointPoses`, snapshot, threads, layered proofs |
| `docs/physics-joints.md`, `docs/voxel-assemblies.md`, `crates/ludion-engine/examples/ragdoll_switch.rs` | §Rules, §Ragdolls, `--limits-from-rest` |

A scene, replay or game that never inserts a `JointZero` and calls `switch_to_ragdoll` poses and hashes as
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

## Trigger

The game's takedowns, chokes and neck-breaks end in a collapse, and its collapse already calls
`switch_to_ragdoll`. Limits, `JointMotor` targets and `JointPoses` then count from wherever the body was
when it died, so a knee switched at −40° has −40° as its zero. Request 0008 asks for rest-relative zeros.

## Decisions

1. **`JointZero` is a carrier component, not a `Joint.zero` field (the request's shape).** `Joint` is a plain
   pub struct and two external games build it with struct literals; a new field breaks both. The conservative
   form is a component beside `Joint` on the carrier (the child part). Folding it into `Joint` is Proposed.
2. **`LimitsFrom::{SwitchPose (default), Rest}`.** `SwitchPose` is `switch_to_ragdoll` exactly. `Rest`
   inserts `JointZero` at the identity on each joint carrier (an assembly's parts have no rest rotation, so
   the rest relative rotation is the identity for every joint). `RagdollSwitch` is `#[non_exhaustive]` and
   built from its default, so W2's `part_velocity` field is additive.
3. **A switch pose outside a rest limit is clamped by the solver on the first step**, a bounded, documented
   snap. It is not refused (the game's preferred answer).
4. **Layers are in the sampled pose**, in both paths. Look-at, IK and ground placement are excluded: not
   built on assemblies, and presentation-only. This supersedes `Plan/animation-layers.md` decisions 29 and 33
   for layers (trigger fired: a ragdoll that must start from a layered pose).
5. **`hold` and the scale channel are ignored** (`Plan/animation-clips.md` d19).
6. **Shapes differ from the request:** `game_time` instead of `ctx`, the `Result<_, Diagnostic>` return of
   `switch_to_ragdoll`, and `JointZero` instead of `Joint.zero`.
7. **The stored-zero math.** `JointZero.rotation` is `q_a⁻¹·q_b`, xyzw. The anchors keep coinciding at the
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

## Waves

- **W1 (shipped).** `JointZero`, `LimitsFrom`, `RagdollSwitch`, `switch_to_ragdoll_with`, layers in the
  switch pose, `L1750`, docs and the example flag (decisions 1–8).
- **W2 (shipped).** `part_velocity`: a two-sample finite difference of the drawn pose, the earlier sample a
  closed form (decision 9).
- **W3 (shipped).** Ragdoll → assembly: `switch_ragdoll_to_assembly`, `ragdoll_landing`, `PoseSnapshot`,
  the placement and face-up rule (decisions 10–14).
- **W4.** The `kind: paired` document: the `KINDS` row, parse and validate, L-codes (0008's range
  L4370–89 / L1750–59), `check-scene` contacts and warp report.
- **W5.** Paired runtime: `paired_fit`, `start_paired`, `paired_motion`, `abort_paired`, hashed
  `PairedAction`, `paired:<action>` events, the lockstep guard, N roles, per-role `posture:`.
- **W6.** `render --paired`, the `paired-takedown` gallery scene and goldens.

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

## Proposed — owner ruling owed

- Folding `JointZero` into `Joint` (`Joint.zero`, the request's shape) at a breaking window, when the two
  external games' struct literals can move.
- `LimitsFrom::Rest` as the default for new content.
- The request's three plan amendments: `Plan/physics.md` W3 joints and the pose-blend rule (`Joint` gains an
  optional stored zero, here as a component); `Plan/animation-clips.md` decision 14 (the outgoing crossfade
  side may be a `PoseSnapshot`); `Plan/document-kinds.md` (`kind: paired` as one more `KINDS` row).

## Non-goals

- A skinned rest-relative switch: `switch_skin_to_ragdoll` already switches at the rig rest pose.
- Look-at, IK and ground placement in the switch pose.
- A `Joint` field change, and flipping the default.

## Risks

- A quaternion zero beyond a limit snaps on the first step; a game that wants no snap clamps its switch
  pose first.
- The M4 cross-arch twin of the new tests is owed.
