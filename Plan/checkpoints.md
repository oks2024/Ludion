# In-memory checkpoints — capture and restore the world from a system

> **Status:** Wave 1 PLANNED (2026-11-18). Nothing shipped yet; this section records what shipped as
> each wave closes.
>
> Neon Shroud request 0012 (`engine-requests/0012-in-memory-checkpoints.md`, filed 2026-11-17, audited
> at engine r2369). The request's §2 table is the semantics of record (below); its §1–§4 and §7 are what
> Wave 1 ships. Siblings: `Plan/sector-state-persistence.md` decisions 8–9 (residue regenerates in
> replays), `Plan/phase-16-profile.md` decision 7 (a restore installs the snapshot's profile), the
> RunSave suspend (`crates/ludion-engine/src/run_save.rs`).

## Why

A game whose player dies often reloads "the world as it was at the last threshold". Today nothing
a system can call does that. `snapshot::capture` and `restore_into` are engine-private, and the snapshot
document carries what a death must not rewind (frame, held input, profile). `RunSave` is a one-shot disk
suspend, consumed on resume and refused under a `RunRecording`. `SceneSwitch::restart` rebuilds from scene
text and loses every change. A checkpoint is a **sim event inside the session**: the world rolls back, the
frame, the input, the profile and the recording keep going, and a run with deaths stays one linear,
replayable recording.

## What exists today

- `snapshot::capture(&Engine) -> Result<Value, Diagnostic>` and `restore_into(&mut Engine, &Value,
  DriftPolicy)` (`crates/ludion-engine/src/snapshot.rs`): the whole world, allocator, RNG, physics blob,
  projectile pool, residue, `GameClock`, scene ref.
- `drain_frame_boundary` (`engine.rs`) is the shared drain order for every tick primitive. `drain_run_save`
  and `resume_run` are the pattern: request-flag resource, engine-side state that outlives rebuilds, a
  mirror resource reinstalled after each rebuild, host pacing kept, a `SceneChanged` event.
- `commit_world` with `Rebuild::{Load, Restore, Switch}` is the one place each rebuild decides what it
  resets and what it carries.

## Decisions

**1. A slot is the existing snapshot document plus metadata.** `snapshot::capture`'s `serde_json::Value`
is the slot body; a checkpoint restore reuses the restore machinery. The document's frame, input, pending
input, run state, speed, render extent, profile and doc-reload log are **ignored** on a checkpoint
restore: the live values stay. JSON is deliberately the slow, correct path; structured slots and budgets
are Wave 4. `SnapshotDoc` does not change (its schema feeds `replay_content_hash`).

**2. A new `Rebuild::Checkpoint`**, with explicit rows in `commit_world`:
- diagnostics carried (like Switch); `resource_carriers`: run, so `carry_resource` resources survive (like
  a restart);
- input: `carry_input_from` (like Switch), so held input stays and no edge re-fires;
- `frame_timeline` / `discard_stream_ring` not bumped, because the frame does not move back;
- `authored` reset; session `rewind(residue, authored_names)` keeps every log and the doc-reload log and
  replaces residue and `authored_names` with the slot's;
- `game_clock` rebased (decision 3); `arrived_by_restart` the slot's;
- profile reinstalled from the text taken before the swap (the Switch precedent);
- prefab library fresh and the replay prefab epoch bumped, as `Restore` does;
- no physics prime: the slot carries its own `PhysicsWorld`.

**3. GameClock rebase.** `held_new = slot.held + (now − slot.frame)`, `holding` from the slot, so
`game_frame(now) == slot_clock.game_frame(slot.frame)` and every `started` stamp in the slot stays valid.
`GameClock::rebased(frames)` in `ludion-core` (saturating add). Frames are `engine.frame()` values.

**4. The store lives on the engine** (`Option<CheckpointState>`, beside `run_save`), is a
`BTreeMap<String, Slot>` (sorted, deterministic), **cleared on `Rebuild::Load` and `Rebuild::Restore`**
(a fresh session: `RunRecording::start`, `engine.restore`, `ludion dev` restarts, RunSave resume) and
**kept on `Switch` and `Checkpoint`**.

**5. Drain order.** `drain_checkpoints(frame)` runs right after `drain_profile`: it takes every request off
the resource first, then applies discards, then captures (call order). The last restore requested wins and
is stashed. `drain_checkpoint_restore(frame) -> bool` runs in the tail, before the scene switch: a quit
wins; a restore wins over a switch requested the same frame (the switch is dropped, `L4205`); a RunSave
resume the same frame clears the store, so the stashed restore is refused as no slot; capture then restore
of one name in one frame restores what was just captured.

**6. Replays need no new record kind**: requests come from hashed state, so a replay regenerates the slots.
**Conservative Wave 1 rule:** a snapshot cannot carry slots yet, so while the store is non-empty
`keyframe_or_skip` writes no keyframe (it moves `last_keyframe` as the `L0007` skip does) and reports
`L4206` once; a seek lands on the last keyframe before the first capture. `arm_recorder` with a non-empty
store refuses with `L4207`. Wave 3 lifts both.

**7. Event:** `Event::SceneChanged { kind: SceneChangeKind::Restore, path }` with the slot's scene path.
A dedicated kind would move the wire schema hash (Proposed, Wave 3).

**8. RNG rewinds to the slot.** No `restore(name, keep)`. Other game resources reset to fresh, as on every
rebuild.

### Semantics of record (request §2)

| state | rewinds to the slot | why |
|---|---|---|
| sim entities, components, allocator, spawn order | yes | "world state is restored as it was" |
| presentation entities | yes | a ghost marker or overlay belongs to the moment |
| RNG stream | yes | an exact replay of the checkpoint moment; a game reseeds after `last_restore()` |
| physics solver, projectile pool, spatial indexes, animation walk, follower motions | yes | exact continuation |
| `Residue` | yes | world state of districts the player may return to |
| scene ref | yes; a slot from another district performs the switch | a checkpoint before a door is valid after it |
| sim `frame` | **no**, keeps counting | replay stays linear; input stays frame-addressed |
| `GameClock` | **rebased** | every `started` stamp stays valid |
| held input, pending injected input | no; edges do not re-fire | the hand is still on the pad |
| run state, `Speed`, render extent, doc-reload log | no | host and session facts |
| `Profile<P>` | **no**, the live profile stays | meta must survive a death |
| `carry_resource` resources | kept | the "session, not world" rule a restart uses |
| other game resources | reset to fresh | resources are not world state |
| the checkpoint store | no | slots survive their own restore |
| `RunSave` pending, `RunRecording` | no; recording continues | nothing outside the sim was read |

Presentation: the rebuild's audio stop and the `present_epoch` cut apply; camera rig state is restored.
No new camera API (request 0010 and the mixer, 0023, own theirs).

## Diagnostics (Neon Shroud 0012 range L4200–L4219 / L1560–L1569)

`L1560` config invalid (`max_slots` 0 or > 64) · `L4200` no such slot (outcome `Refused`) · `L4201` slots
full (never evicts) · `L4202` invalid name (empty, over 64 chars, outside `[A-Za-z0-9_.-]`) · `L4203`
capture failed (old slot kept) · `L4204` restore failed (world untouched) · `L4205` switch dropped by a
restore · `L4206` keyframe skipped while slots are held · `L4207` recording arm refused while slots are
held.

## Waves

- **W1 — in-memory slots**:
  `Checkpoints` resource (`capture`/`restore`/`discard`, `slots`, `slot`, `last_restore`),
  `CheckpointConfig { max_slots }`, `Rebuild::Checkpoint`, the clock rebase, the keyframe skip and arm
  refusal, tests, `docs/checkpoints.md`, the `checkpoints` example.
- **W2 — disk slot.** `persist`/`load_disk`/`discard_disk`/`disk()`; a `save/checkpoint.json` envelope with
  a `BuildStamp`, kept on load; strict refusal with a `.bak.<hash>` backup; `load_disk` refused while a
  recording is open.
- **W3 — the store rides the snapshot.** A `checkpoints` snapshot field lifts the keyframe skip and the arm
  refusal (re-records the replay corpus); the slot digest joins `WorldHash`;
  `SceneChangeKind::CheckpointRestore`.
- **W4 — structured slots.** In-memory slots as erased column clones, the capture ≤ 4 ms / restore ≤ 50 ms
  budgets, the `checkpoint_capture` / `checkpoint_restore` profiler keys, `checkpoint.*` wire commands.

## Proposed — owner ruling owed

- **(a)** W3's replay corpus re-record (the snapshot gains a field).
- **(b)** The slot digest joining `WorldHash` (additive term only when the store is non-empty).
- **(c)** `SceneChangeKind::CheckpointRestore` (moves the wire schema hash).
- **(d)** `Plan/phase-16-profile.md` decision 7 exception: a checkpoint keeps the live profile. Wave 1
  ships this behaviour; the decision text is amended only by a note pointing here.
- **(e)** `carry_resource` resources crossing a checkpoint restore, and the matching
  `docs/scene-transitions.md` line. Wave 1 ships this.
- **(f)** The RNG rewinds on restore.

## Out of scope

- Disk persistence, the snapshot field, the digest, structured slots and wire commands: Waves 2–4.
- `restore(name, keep: &[EntityId])` — trigger: a game that cannot carry meta state in the profile or a
  carried resource.
- Multiple named disk slots — trigger: a game with a save-slot screen.
- Time scale (request 0011), camera zones (0010) and mixer state (0023): the rebase stays correct under
  them; nothing is built for them here.

## Status

Wave 1: not started.
