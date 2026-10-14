---
description: ludion-physics time scale and freeze, plus the bincode snapshot format
---
# physics-time-snapshot

**Scope:** `scale_time`/`freeze` and the `to_bytes`/`from_bytes` snapshot of `PhysicsWorld`. Hub: `ludion devdocs ludion-physics`.

## Rules

### Time scale

`time_scale.rs` holds `scale_time`, `freeze` and `last_step_dt`; the state is three fields on
`PhysicsWorld` (`time_scale`, `frozen`, `last_step_dt`); only `last_step_dt` is in the snapshot. `step` takes
both (resetting them to `1.0`/`false`, so a call lasts one step slot), then either returns —
frozen: `last_step_dt = 0.0`, contact and joint events already cleared, `fresh_colliders` and `touching`
untouched so the next real step diffs against the last real one — or runs rapier with
`integration_parameters.dt = base × scale` and restores `base` straight after. The default path is a
multiply by `1.0` (the joint helper below is skipped), so no committed hash moves. The write and mirror systems are untouched: on a frozen
frame the write system's impulses and motors still change velocities, and `blast_hits` stay because
they describe impulses that applied. Neither the scale nor the freeze enters the blob or `WorldHash`
(`last_step_dt` rides the blob, so a restore after a frozen tick reports `0.0` like the
uninterrupted run), and determinism rests on the game re-deriving the scale each frame from hashed
state. `to_bytes` refuses (`Encode`) while a scale below
`1.0` or a freeze is pending — only reachable from a bare call between ticks — like the impulse queue.
`last_step_dt` starts at the base timestep in `new`.

A scaled step (`scale < 1.0`) also retunes each attached joint: rapier's lock-row bias is `err × erp_inv_dt`, about `err / substep` and unclamped, so `step` solves each joint's softness frequency to hold `erp_inv_dt` at its full-step value for the scaled substep (`scale_joint_softness`), then restores the saved coefficients by handle straight after the rapier step. Contacts and motors are bounded as `dt → 0` and are untouched; the `scale == 1.0` path skips the helper.

## Dataflow

### Snapshot

`to_bytes`/`from_bytes` encode the solver state with **bincode**: rapier's nine persistent
structures (gravity, integration parameters, islands, broad phase, narrow phase, bodies, colliders,
impulse joints, multibody joints) plus the creation-order handle table, the joint table, and the
plane, plus the last step's touching set, contact events and windows, blast hits, fresh colliders
and `last_step_dt`. The step pipeline and CCD workspace carry nothing across a step and are rebuilt. Bytes rather
than a structured value because the narrow phase's 128-bit solver masks do not fit in a JSON number;
base64-wrapping is the caller's job. A bincode major bump is a snapshot-format event. The timestep
comes from the `TickRate` argument, not the bytes, so a session restores at the rate it runs at.

## Verify

```
cargo test -p ludion-physics
```
