# Plan: Paired animations and a posed ragdoll — rest-relative zeros, part velocity, the way back, `kind: paired`

> Status: **W1 mechanized (2026-11-18); W2–W6 not started.** Driver: Neon Shroud request 0008
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

## Waves

- **W1 (shipped).** `JointZero`, `LimitsFrom`, `RagdollSwitch`, `switch_to_ragdoll_with`, layers in the
  switch pose, `L1750`, docs and the example flag (decisions 1–8).
- **W2.** `part_velocity`: a two-sample clip finite difference. A live `BlendClock` side samples `prev`, not
  `phase`, for the earlier sample (`extract.rs`, where `fraction` is relative to `floor(now)`).
- **W3.** Ragdoll → assembly: `switch_ragdoll_to_assembly`, `PoseSnapshot`, `Animation.from = "$pose"`, and
  the placement and face-up rule.
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
  preferred rule; the engine documents its own in W3.
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
