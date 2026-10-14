---
description: Slowing or freezing the physics step: scale_time, freeze, hit-stop, impact impulses
---

# Physics time scale

**Scope:** `PhysicsWorld::scale_time` and `freeze`: the call contract, the frozen frame, determinism, and what slow motion does to impacts, sleep, kinematic bodies and joints. Contacts themselves are `ludion docs physics`.

Slow or stop the physics step alone (a hit-stop, slow-motion blast) with three per-frame
calls on the `PhysicsWorld` resource:

- `scale_time(scale)` — the step runs at `scale` of the fixed timestep, finite in
  `[MIN_TIME_SCALE, 1]` (`1e-3`, else `QueryInput`; `freeze()` stops). Calls in a frame take the minimum.
- `freeze()` — skips the step outright; it wins over any scale.
- `last_step_dt()` — the last step slot's timestep: `base × scale`, `0.0` when frozen.

The scale resets after the step, so call it every frame the effect should last, before the write
system (under `physics:` every game system does). The sim clock and game systems keep their rate;
velocities are unchanged, so a body covers less distance per tick. Frozen frame:

| Part | On a frozen frame |
|---|---|
| write system, mirror | run — queued impulses, blasts and motors change velocities, not poses |
| step | skipped |
| `contact_events`, joint events | empty; a contact starting now is reported by the next step |
| `blast_hits` | kept — the impulses applied |
| queries | read the last completed step, so a body spawned, rebuilt (`Collider` edited) or teleported now is unseen, and blast-immune until the next real step |

**Determinism.** A scale or freeze is in neither the snapshot nor `WorldHash` (the last step's `last_step_dt` is in the snapshot): derive it every frame from
your own hashed timer and a replay or restore reproduces it. A snapshot refuses to encode (`Encode`)
while one is pending.

**Consequences.** Sleep counts simulated seconds: `sleep_after_ticks` takes `1 / scale` times the ticks. Resting-contact impulses and joint `break_impulse` are compared per step, so they shrink with `dt`,
and a joint that breaks under sustained load at full speed may hold in slow motion. Kinematic bodies follow the
ECS pose, not the scale: a full per-tick target in a scaled step pushes rigid bodies at `1 / scale` times
the speed, so scale your kinematic motion by the same factor.

**Impacts.** An impact's impulse is the `Started` impulse at any scale (pinned by
`an_impact_impulse_survives_time_scale`). A pair starts the first step it touches (penetrating or
exchanging impulse; a pair inside the prediction margin with the gap open has not started), and its
`Started` carries the impulse summed over one base tick of simulated time from that step: the same
step at full speed, `ceil(1 / scale)` steps under `scale_time`. The event arrives up to `1 / scale`
ticks after the touch (the same simulated moment as at full speed), with the touch's `point` and
`normal`; a pair that separates inside the window reports its `Started`, then its `Stopped` on the next
step, and a flushed `Started` may name an entity despawned that tick. Resting-contact impulses are per step and shrink with `dt`, so threshold on `Started`, never on
resting contact.

Joints keep their full-speed gain: a scaled step corrects a joint's drift at the full-speed rate per simulated second, not amplified by `1 / scale`, so a ragdoll landing into a hit-stop settles as unscaled and an impact's joint stretch closes at the slowed rate.

**A hold does not reach physics:** `GameClock::hold` (`ludion docs game-clock`) and `Speed` do not slow
bodies; call `freeze()` on the same frames.

A hit-stop then slow motion is two frame counters in a hashed component: `freeze()` while `hit_stop > 0`,
else `scale_time(0.25)` while `slow_mo > 0` (the doctest on `PhysicsWorld::scale_time`).

## Next

- `ludion docs physics` — rigid bodies, contact events, queries.
- `ludion docs game-clock` — the gameplay clock whose hold does not reach physics.
