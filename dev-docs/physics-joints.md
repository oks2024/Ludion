---
description: The Joint-to-rapier mapping, frame capture, motor drive, break detection and inert/missing bodies
---

# Physics joints

**Scope:** `ludion-physics`'s `joints.rs` and the joint half of `world.rs`: the `Joint`-to-
`GenericJoint` mapping, how a joint's frame is captured, the `JointMotor` drive, break detection and
a missing body. Defers to `ludion devdocs ludion-physics` for the crate's boundary rule, the world
resource and the write/step/mirror triple this rides inside, and `ludion devdocs
physics-events-queries` for how a broken or inert joint is reported (`JointBroken`, `L3035`, the
`pending_missing` queue).

## Layout

`joints.rs` — `JointBroken` (public), `JointEntry` (the table row), `build_generic_joint`, the
pure `Joint` → `GenericJoint` mapping, and the motor helpers `desired_motor`, `current_motor`,
`apply_motor` and `motor_off_target`. `world.rs` — `PhysicsWorld`'s `joints: Vec<JointEntry>` (the
`table`/`BodyEntry` precedent: creation order, the only iteration order, rides the snapshot) and
`joint_rows: StableMap<EntityId, usize>` (a lookup accelerator, iterated by nothing), plus
`reconcile_joints`, `drive_joints`, `attach_joint`, `joint_bodies` and `collect_broken_joints`.

## Dataflow

`reconcile_joints` runs at the end of `systems.rs::write`, after the body loop, from a
`(&Joint,)` query collected ascending by carrier — so `self.rows` already reflects this tick's body
creations, rebuilds and retirements when it reads them. It first purges every `broken` entry
unconditionally (along with every carrier no longer in `live`) — the flag only has to survive until
the mirror removes its `Joint` component, which happens the same tick the break was detected, so by
the *next* reconcile the row is stale one way or another: gone from `live` if nothing re-added a
`Joint` to the carrier, still present (and now taking the `None` arm below, forming fresh) if
something did. Per carrier still present after that purge: a missing body drops the rapier joint (if
attached); a changed `Joint` drops and rebuilds; a `handle` that is `None` or no longer in
`rapier.impulse_joints` is reattached.

| authored `kind` | locked axes | free/limited axis | motor rows |
|---|---|---|---|
| `hinge` | `LOCKED_REVOLUTE_AXES` (all linear, `ANG_Y`/`ANG_Z`) | `ANG_X` — `axis`, limits in degrees | `AngX` |
| `ball` | `LOCKED_SPHERICAL_AXES` (all linear) | `ANG_X`/`ANG_Y`/`ANG_Z` free, each independently limited | `AngX/Y/Z` |
| `slider` | `LOCKED_PRISMATIC_AXES` (`LIN_Y`/`LIN_Z`, all angular) | `LIN_X` — `axis`, limits in world units | `LinX` |
| `fixed` | `LOCKED_FIXED_AXES` (everything) | none | none |

`GenericJoint::new(locked)` selects the mask; `set_local_axis1` rotates the joint's local X onto the
authored `axis` (irrelevant for `ball`/`fixed`, which have none), so the kind's one free (or
per-axis-limited) degree of freedom is always local X (or, for `ball`, all three angular axes).
Angles are authored in degrees and converted to radians at the boundary.

`drive_joints` runs after `reconcile_joints` in `systems.rs::write`, from the `(&JointMotor,)` query
collected ascending by carrier. It diffs `desired_motor` (the carrier's `JointMotor` and `Joint` kind,
angles in radians) against `current_motor`, read off the live `GenericJoint`, and calls
`get_mut(h, true)` + `apply_motor` only on a difference, so a retarget never rebuilds the joint or
re-captures its zero. `apply_motor` sets `MotorModel::ForceBased` per driven axis and `current_motor`
treats any other model as a difference. The motor state rides the rapier blob, so the joint table and
the snapshot format are unchanged; a reattached joint is a fresh `GenericJoint`, re-motored the same
tick. A driven joint with a sleeping dynamic body is tested by `motor_off_target`, which measures the
error as rapier's motor row does but in the half-angle-sine domain (`ludion_core::math` has no
`asin`): the relative rotation's vector part against `sin(target / 2)`, and the slide along frame 1's
x; past `MOTOR_REST_EPSILON` (1e-3) it wakes the sleepers so a steady drive never stalls short of its
target. Motored joints leave rapier's SIMD row group for the scalar path. The user-facing facts
(units, tuning, errors) are in `ludion docs physics-joints`.

`collect_broken_joints` runs in `PhysicsWorld::step`, after `collect_contacts`: for every entry with
a `break_impulse` and a live `handle`, it reads `ImpulseJoint::impulses` (`[Real; 6]`,
`LinX/Y/Z/AngX/Y/Z`) and sums the linear part's (indices `0..3`) squares; a `Slider` also adds its
free axis's limit impulse (`ImpulseJoint::data.limits[JointAxis::LinX].impulse` — `LinX` is the
slider's *unlocked* axis, so it never shows up in `impulses` above, only here once the ride hits an
end stop). The magnitude is scaled by `num_solver_iterations` (rapier writes each locked/limited
DOF's impulse back once per solver substep, and the pinned build's `num_solver_iterations` doubles as
the substep count — `staged_island_solver/init.rs` — so the raw read is one substep's share, not the
step's) before comparing against the threshold. A break calls `remove_impulse_joint`, sets
`handle = None` and `broken = true`, and pushes a `JointBroken` carrying the same scaled magnitude.
The mirror system removes the carrier's `Joint` component the same tick.

## Rules

- **The second frame is never authored.** `local_frame2 = pb.inverse() * pa * local_frame1`, so the
  bodies' *current* relative pose at build time is the joint's zero — a mid-flight grab, a
  runtime-spawned joint, or a rebuild (a changed `Joint`, or a body rebuilt by a `Collider` edit)
  re-captures it at the bodies' poses as they are that tick, never the authored numbers alone.
- **A stale `handle` is not always `None`.** `remove_body` (used by both retirement and a `Collider`
  rebuild) cascades to `impulse_joints`, so a body rebuild silently drops an attached rapier joint —
  the entry's `handle` stays `Some` but is no longer in `rapier.impulse_joints`. `reconcile_joints`
  checks `impulse_joints.contains(handle)`, not just `handle.is_some()`, or a rebuilt body's joint
  would never reattach.
- **A `broken` entry is retired by the flag itself, on the next reconcile.** The flag stops
  `reconcile_joints` from reattaching a joint the mirror has not yet caught up to removing within the
  tick the break happened; on the *next* `write` the entry is purged unconditionally, whether or not
  its carrier is still in `live` — so a `Joint` re-added to the same carrier (a game system reacting
  to `joint_events()`, or a wire edit at the frame boundary) always takes the fresh-attach path
  instead of staying inert forever.
- A missing body (`a`/`b` dead, or claimed by neither tier — `ludion devdocs ludion-physics`'s
  `claims` rule) removes the rapier joint, if any, and leaves the entry with `handle: None` — inert,
  never deleted, retried every `write` until both bodies exist.

## Gotchas

`collect_broken_joints` already corrects for the substep bias (see above), so `break_impulse` is
authored and reported as a per-*step* quantity — but rapier's own per-substep write means a
`solver_iterations` change still moves *which* substep last wrote `impulses`/`limits[i].impulse`
before the scaling multiplies it back up, so calibrate a `break_impulse` against a run at the scene's
actual `solver_iterations`, not by hand from `mass * gravity * dt`.

`current_motor` never compares `motors[i].impulse`: it is warm-start state the solver rewrites every
step, so comparing it would rewrite the motor every tick and keep waking the bodies.

Only the four `JointKind`s above are mapped, over `ImpulseJointSet` — no springs, velocity-target
motors or multibody joints.

## Verify

```
cargo clippy -p ludion-physics --all-targets --all-features -- -D warnings
cargo nextest run -p ludion-physics --test joints --test joint_motor
cargo nextest run -p ludion-engine --test it -E 'test(/^(physics_joints|joint_motor)::/)'
```
