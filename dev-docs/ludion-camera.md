---
description: ludion-camera: the pure camera-rig pipeline crate and how the engine's camera_rig system drives it
---

# ludion-camera

**Scope:** editing the rig maths or its engine system. Authoring and fields are `ludion docs camera`.
The engine side (`camera_rig`, faults `L1304`–`L1309`, a faulted rig holding its last pose) is
`crates/ludion-engine/src/camera_rig.rs`; its module doc is the fault list.

## Layout

- `crates/ludion-camera/src/lib.rs` — crate doc: the stage order (snap or switch, advance, blend, shake). Read it, don't transcribe it.
- `crates/ludion-camera/src/rig.rs` — the authored `CameraRig` and the engine-written state.
- `crates/ludion-camera/src/step.rs` — `step_rig`, the per-tick pipeline.
- `crates/ludion-camera/src/spring.rs` — critically damped springs and the angle helpers the stages share.
- `crates/ludion-camera/src/track.rs` — the `track` pose: target projected onto a path, eye at an offset in its frame.
- `crates/ludion-camera/src/shake.rs` — seeded trauma noise.
- `crates/ludion-camera/src/validate.rs` — range rules (`L1304`), shared by the document validator and the runtime check.
- `crates/ludion-camera/src/relative.rs` — camera-relative stick mapping.

## Dataflow

The crate is pure. Each tick the engine resolves the target pose and the path, calls `step_rig` on the
gameplay clock, and writes the result to the camera entity's `Transform` and perspective `Camera`. The
system runs after the physics triple and before the presentation systems, so it reads this tick's
settled target.

## Rules

- **Transcendentals go through `ludion_core::math`.** `CameraRig.state` is hashed and replays bit-exact
  across platforms; a platform libm call moves the `WorldHash`.
- **Shake touches the output only, never the state.** Feeding it back makes the springs chase the
  noise, so a shake displaces the rig's pose instead of jittering around it.
- **An empty `state.last_applied` is the implicit snap, and a restore carries it.** A scene load starts
  without it and snaps; `engine.restore` carries the state, so it resumes instead of snapping. Clearing
  it elsewhere turns a restore into a camera cut.

## Verify

```
node scripts/check.mjs --scope crates/ludion-camera
```
