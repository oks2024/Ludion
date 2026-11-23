---
description: Checkpoints — capture the world into a named slot, restore after a death, persist a Continue
---

# Checkpoints

**Scope:** Capturing the whole world into a named in-memory slot and restoring it from a system — `Checkpoints`, what a restore rewinds and keeps, how it records and replays, and the one disk slot that survives a quit. Suspending the exact run to disk is `ludion docs player-profiles`; rebuilding the scene from text is `ludion docs scene-transitions`; the gameplay clock a restore rebases is `ludion docs game-clock`.

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
- A `RunSave::resume` the same frame replaces the store with the suspended one and the restore is refused (`L4204`).
- A scene load clears every slot; a scene switch keeps them. An `engine.restore`, a `ludion dev` restart, a replay seek and a `RunSave` resume **install the slots the snapshot carries** (a snapshot taken with none installs none).

## Recording and replay

A restore needs no record kind: replaying regenerates each capture and restore from the same hashed state. `RunRecording` and a session recording continue across a restore.

A snapshot document carries the slots (and `last_restore`, which a game reads to reseed after a restore), so a keyframe taken while slots are held is a full keyframe: a seek lands inside a held window and the restores the game requests after it still reproduce the live run. A snapshot taken with no slot and no restore has no `checkpoints` field.

The `WorldHash` folds in the slot **metadata** (name, frames, scene, clock) while any slot is held, and the `GameClock` while it has excluded time. A restore rebases the clock, so the hash after a restore is not the hash at the capture; compare world content, not the hash, when a test claims "the restore is the capture".

## Refusals

Every refusal is a warning on both sinks and leaves the world untouched.

| code | case |
|---|---|
| `L1560` | `max_slots` is 0 or above 64 (a build error) |
| `L4200` | restore of a name with no slot |
| `L4201` | capture of a new name past `max_slots`; nothing is evicted, recapturing a held name is fine |
| `L4202` | a name that is empty, over 64 characters or outside `[A-Za-z0-9_.-]` |
| `L4203` | the capture failed (the solver or projectile pool could not be encoded); the old slot is kept |
| `L4204` | the slot could not be restored |
| `L4208` | the disk slot could not be written or removed (an error; the previous file stays) |
| `L4209` | a disk load was refused (see Disk slot) |

## Disk slot

One held slot can be written to `save/checkpoint.json`, so a Continue survives a quit. The path is fixed; a game that never calls `persist` never writes.

```rust
use ludion::prelude::*;

fn continue_logic(world: &mut World, _ctx: &SystemCtx) {
    let on_title = false; // read from hashed state
    let checkpoints = world.resource_mut::<Checkpoints>().unwrap();
    // After a capture: keep it across a quit.
    checkpoints.persist("b_to_c");
    // Title screen: offer Continue when the file exists, then:
    if on_title && checkpoints.disk().is_some() {
        checkpoints.load_disk();
    }
    // New game:
    checkpoints.discard_disk();
}
```

- `persist(name)` writes held slot `name` after this frame's captures (the last call wins; an unknown name is `L4200` and writes nothing). `discard_disk()` deletes the file and runs before any `persist`. `load_disk()` is a restore request, so the last `restore`/`load_disk` in a frame wins.
- `disk()` is the file read at boot or last written, as the slot's `CheckpointInfo`; `disk_error()` is the last `L4208`/`L4209`.
- A load is **not consumed**: the file stays, the slot also lands in the store, and `restore(name)` after it works, so dying after a Continue returns to the threshold. The live profile is kept and the clock is rebased as on any restore.
- **The frame never moves back.** A Continue usually runs at a title-screen frame below the slot's: the frame jumps forward to the slot's, so `game_frame` equals the slot's. Input injected for frames in between is dropped; held input stays.
- Disk only on a persisting host (`persist_profile`). Elsewhere `persist` only updates `disk()` and `load_disk()` is `Refused` ("this host keeps no saves"); tests and replays never touch disk.
- **Strict refusal.** A garbled or newer-format file, one written by another build (engine version, component or data schemas, projectile pool), an invalid name, or a slot that no longer restores is `L4209`: the file is renamed to `checkpoint.json.bak.<hash>`, `disk()` becomes `None`, and the world is untouched. While a recording is open the load is refused (`L4209`) with the file **kept** and no backup: stop the recording first.
- `disk()` is **disk input**, like `RunSave::pending()`: a replay and a playback see `None`. Read it on the title screen, before a recording starts. A continued run cannot be recorded from the loaded state (a recording starts from scene text).
- A write is crash-durable, so a quit mid-write keeps the previous file. A failed write or delete is `L4208`; the next request retries.

**Continue policy:** offer `RunSave::pending()` first (the exact quit moment); fall back to `disk()` (the last threshold). Quit versus suspend: a quit that wants the exact moment suspends; a game that only wants a threshold persists at the capture and needs nothing at quit.

## Suspend, checkpoint or restart

| you want | use | the file | frame, profile, recording |
|---|---|---|---|
| "Continue" after a quit, the exact moment | `RunSave::suspend` | `save/suspend.json`, consumed on resume | resume is refused while recording |
| back to the last threshold after a death | `Checkpoints::restore` | memory only | all continue |
| "Continue" after a quit, the last threshold | `Checkpoints::persist` / `load_disk` | `save/checkpoint.json`, kept on load | load is refused while recording |
| a clean level, every change lost | `SceneSwitch::restart` | none | frame and RNG continue; only `Residue` and carried entities survive |

A slot lives and dies with the process unless it was persisted.

## How a slot is held

A held slot is structured, not JSON: a capture clones each registered component's column plus a typed copy of the resources and entity ids, and a restore installs those columns in bulk. Every registered component must therefore derive `Clone` (`Registry::register` requires it). Only the disk slot is JSON: `persist` renders the slot as the snapshot document, byte-identical to `engine.snapshot` at the capture frame, and `load_disk` restores a document.

Because a capture reads no component value, a NaN or infinite float does not fail it. It fails the `persist`: `L4208` carrying the `L0007` message, nothing written, the slot kept and restorable in memory. A restore still costs a world rebuild, so budget it for a fade, not for a frame.

## Cost

The targets, on a 5,000-entity scene with physics, are a capture of at most 4 ms and a restore of at most 50 ms. An `#[ignore]` release bench gates them on the pinned boxes against `budgets/checkpoints-*.toml`. Hide a restore under your fade: the capture fits a frame, the restore does not.
