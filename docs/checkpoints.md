---
description: In-memory checkpoints — capture the world into a named slot and restore it after a death
---

# Checkpoints

**Scope:** Capturing the whole world into a named in-memory slot and restoring it from a system — `Checkpoints`, what a restore rewinds and keeps, and how it records and replays. Suspending a run to disk is `ludion docs player-profiles`; rebuilding the scene from text is `ludion docs scene-transitions`; the gameplay clock a restore rebases is `ludion docs game-clock`.

A checkpoint is a **sim event inside the session**. A system captures at a threshold, and after a death restores the slot: the world is as it was, while the frame, the held input, the live profile and an open run recording carry on. A run with deaths stays one linear, replayable recording.

## The loop

```rust
use ludion::prelude::*;

// Builder: `Engine::builder().checkpoints(CheckpointConfig { max_slots: 4 })`.
fn checkpoint_logic(world: &mut World, _ctx: &SystemCtx) {
    let crossed_threshold = false; // read from hashed state: a sensor, a flag component
    let player_died = false;
    let checkpoints = world.resource_mut::<Checkpoints>().unwrap();
    if crossed_threshold {
        checkpoints.capture("b_to_c");
    }
    if player_died {
        checkpoints.restore("b_to_c");
    }
}
```

Requests are drained at the frame boundary, never mid-tick, so no system sees a half-restored world. Issue them from **hashed state** (a component, the frame): the same input then reproduces the same requests on the same frames, which is how a replay regenerates the slots. A request made from unhashed state (a resource, wall time) diverges on replay.

`discard(name)` drops a slot. `slots()` lists `CheckpointInfo { name, frame, game_frame, scene }` sorted by name; `last_restore()` is `Restored { name, frame }` (the slot's capture frame) or `Refused { name, reason }`. A restore keeps its slot, so the same checkpoint restores ten deaths in a row.

## What a restore rewinds and keeps

| state | on restore | why |
|---|---|---|
| entities, components, allocator, spawn order, presentation entities | rewinds | the world as it was |
| RNG stream | rewinds | an exact replay of the moment; reseed after `last_restore()` for fresh rolls |
| physics solver, projectile pool, spatial indexes, animation walk, follower motions | rewinds | exact continuation |
| scene and its residue | rewinds | a slot from another district performs that switch |
| sim `frame` | **keeps counting** | replay stays linear; input stays frame-addressed |
| `GameClock` | **rebased** | `game_frame(now)` equals the slot's, so every `started` stamp stays valid |
| held input, injected input | kept; no edge re-fires | the hand is still on the pad |
| run state, speed, render extent | kept | host facts |
| `Profile<P>` | **the live profile stays** | deaths, settings and unlocks outlive the rollback |
| `carry_resource` resources | kept | the same "session, not world" rule a restart uses |
| other resources | reset to fresh | resources are not world state |
| the slots, `RunSave` pending, an open `RunRecording` | kept | a restore is repeatable and recorded |

The rebuild's audio stop applies: music and loops fade out, so restart music from `last_restore()`. Every entity is cut for interpolation on the restore tick. A `SceneChanged` event with kind `restore` and the slot's scene path is pushed for tools.

## Drain order

At one boundary: every `discard`, then every `capture` in call order, then the **last** `restore` called. Quit wins over a restore.

- A capture and a restore of one name in one frame restore what was just captured.
- A restore wins over a scene switch requested the same frame: the switch is dropped and reported (`L4205`).
- A `RunSave::resume` the same frame clears the store, so the restore is refused as no slot (`L4200`).
- A scene load, an `engine.restore` and a `ludion dev` restart clear every slot; a scene switch keeps them.

## Recording and replay

A restore needs no record kind: replaying regenerates each capture and restore from the same hashed state. `RunRecording` and a session recording continue across a restore.

A snapshot document cannot carry slots, so while any slot is held:

- a recording writes **no keyframe** (`L4206`, once per session): a seek lands on the last keyframe before the first capture, and the replay itself is unaffected.

A recording can only start at frame 0 of a freshly loaded scene, which holds no slots; `L4207` is the engine's guard if one is ever armed with slots held.

## Refusals

Every refusal is a warning on both sinks and leaves the world untouched.

| code | case |
|---|---|
| `L1560` | `max_slots` is 0 or above 64 (a build error) |
| `L4200` | restore of a name with no slot |
| `L4201` | capture of a new name past `max_slots`; nothing is evicted, recapturing a held name is fine |
| `L4202` | a name that is empty, over 64 characters or outside `[A-Za-z0-9_.-]` |
| `L4203` | the capture failed (a NaN component, `L0007`); the old slot is kept |
| `L4204` | the slot could not be restored |

## Suspend, checkpoint or restart

| you want | use | the file | frame, profile, recording |
|---|---|---|---|
| "Continue" after a quit, the exact moment | `RunSave::suspend` | `save/suspend.json`, consumed on resume | resume is refused while recording |
| back to the last threshold after a death | `Checkpoints::restore` | memory only | all continue |
| a clean level, every change lost | `SceneSwitch::restart` | none | frame and RNG continue; only `Residue` and carried entities survive |

A slot lives and dies with the process. Cost: a capture and a restore each go through the JSON snapshot document, so budget them for a fade, not for a frame.
