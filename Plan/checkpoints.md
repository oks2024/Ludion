# In-memory checkpoints — capture and restore the world from a system

> **Status:** Waves 1 and 2 mechanized (2026-11-18); W4a shipped (Lore 2450); W4b mechanized (measured medians owed); W3 shipped in R14; W5 (the wire verbs) shipped in R14 (T10-C2a). The Status section at the end
> records what shipped as each wave closes.
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
`BTreeMap<String, Slot>` (sorted, deterministic), **cleared on `Rebuild::Load`** (a fresh session:
`RunRecording::start`), **replaced on `Rebuild::Restore`** and **kept on `Switch` and `Checkpoint`**.
*Amended in R14 (W3):* an `engine.restore`, a `ludion dev` restart, a replay seek and a RunSave resume
install the store the snapshot document carries (the `checkpoints` field); a document without the field
installs an empty store, the old clearing behaviour. So a RunSave suspend and resume carries the slots.
The row is `StoreRow { Clear, Replace, Keep }` in `commit_world`.

**5. Drain order.** `drain_checkpoints(frame)` runs right after `drain_profile`: it takes every request off
the resource first, then applies discards, then captures (call order). The last restore requested wins and
is stashed. `drain_checkpoint_restore(frame) -> bool` runs in the tail, before the scene switch: a quit
wins; a restore wins over a switch requested the same frame (the switch is dropped, `L4205`); a RunSave
resume the same frame clears the store, so the stashed restore is refused as no slot (*R14: it replaces the
store with the suspended one and the stashed restore is refused, `L4204`, "replaced the world the same frame"*); capture then restore
of one name in one frame restores what was just captured.

**6. Replays need no new record kind**: requests come from hashed state, so a replay regenerates the slots.
**Conservative Wave 1 rule:** a snapshot cannot carry slots yet, so while the store is non-empty
`keyframe_or_skip` writes no keyframe (it moves `last_keyframe` as the `L0007` skip does) and reports
`L4206` once; a seek lands on the last keyframe before the first capture. `arm_recorder` with a non-empty
store refuses with `L4207`. Wave 3 lifts both. *Lifted in R14 (W3): keyframes and `engine.snapshot` carry
the store, the skip and the refusal are gone, and `L4206`/`L4207` are retired.*

**7. Event:** `Event::SceneChanged { kind: SceneChangeKind::CheckpointRestore, path }` with the slot's scene path (amended in R14: it was `Restore`; `engine.restore` keeps that).
A dedicated kind would move the wire schema hash (accepted 2026-11-19, scheduled (R14 re-record round)).

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
held (both retired in R14, W3). W2 adds `L4208` disk write failed (error; the previous file stays, the next request retries) ·
`L4209` disk load refused (the `L1401` sibling).

## Wave 2 decisions — the disk slot

**W2-1. No config field.** The path is fixed at `save/checkpoint.json` (the `RUN_SAVE_PATH` precedent). The
request's `persist: Option<path>` is dropped; a game that never calls `persist` never writes.

**W2-2. API on `Checkpoints`**, applied at the boundary: `persist(name)` writes held slot `name` after this
frame's captures (last call wins; unknown name `L4200`, nothing written); `discard_disk()` runs before any
`persist`; `load_disk()` is a restore request sharing W1's field, now a private `RestoreRequest { Slot, Disk }`
(last `restore`/`load_disk` wins); `disk()` is the file read at boot or last written, `None` in a replay or
playback; `disk_error()` is the last `L4208`/`L4209`. No new public types; `RestoreOutcome` reports a load under
the envelope's slot name.

**W2-3. Envelope** (`format: 1`, private serde struct): `{format, build: BuildStamp, name, frame, game_frame,
scene?, scene_path?, authored_names (sorted), slot: <capture doc>}`. The slot doc is the W1 capture unchanged;
its profile and input fields are ignored on load.

**W2-4. Load order.** A host that keeps no saves: `Refused`, no backup. A recording open: `L4209`, file kept, no
backup. Strict read: a missing file is `Refused` with no backup; garbled, newer format, stamp mismatch or an
invalid name is `L4209` with a `.bak.<hash>` backup and `disk() = None`. A new name with the store full:
`L4201`, file kept. Otherwise the slot enters the store (replacing a same-named one) and W1's slot restore
runs; a restore error (drift) is `L4209` with backup and the store is put back as it was.

**W2-5. The frame never moves back.** A Continue runs at a frame below the slot's, so the checkpoint driver
frame is `max(engine.frame(), slot.frame)`; `game_frame` then equals the slot's. When the frames differ the
`Rebuild::Checkpoint` input row clones the held `InputState` and drops pending injected input (addressed to
skipped frames); equal frames still `carry_input_from`. In-memory slots are never ahead, so W1 is unchanged.

**W2-6. Persist under recording is allowed:** it is output, and a replay regenerates it on a host that only
updates info.

**W2-7. Determinism.** `disk()` is disk input like `RunSave::pending()`: read it on the title screen, before a
recording starts. A continued run cannot be recorded from the loaded state (`RunRecording::start` reloads
scene text, and held slots refuse arming, `L4207`).

## Waves

- **W1 — in-memory slots** (shipped, r2372):
  `Checkpoints` resource (`capture`/`restore`/`discard`, `slots`, `slot`, `last_restore`),
  `CheckpointConfig { max_slots }`, `Rebuild::Checkpoint`, the clock rebase, the keyframe skip and arm
  refusal, tests, `docs/checkpoints.md`, the `checkpoints` example.
- **W2 — disk slot** (shipped, r2373; decisions W2-1..7). `persist`/`load_disk`/`discard_disk`/`disk()`/`disk_error()`; a
  `save/checkpoint.json` envelope with a `BuildStamp`, kept on load; strict refusal with a `.bak.<hash>`
  backup (`L4209`); `load_disk` refused while a recording is open; the frame jumps forward to a slot ahead of
  it; `L4208` write failures.
- **W3 — the store rides the snapshot** (shipped in R14; decisions W3-1, W3-2). A `checkpoints` snapshot
  field lifts the keyframe skip and the arm refusal (re-records the replay corpus); the slot digest joins
  `WorldHash`; `SceneChangeKind::CheckpointRestore` (shipped with W5).
- **W4a — structured slots** (shipped, Lore 2450; decisions W4-1..7). In-memory slots as erased column clones plus a
  typed `SnapshotDoc`; disk slots keep the JSON envelope.
- **W4b — budgets** (0012-04). The capture ≤ 4 ms / restore ≤ 50 ms budgets, measured by a bench or test that
  reads no new wire field; a `ProfileReport` `checkpoints` section and the `checkpoint_capture` /
  `checkpoint_restore` profiler keys would be R14 items.
- **W5 — wire commands** (shipped in R14, T10-C2a; decisions W5-1..4). `checkpoint.*`, designed in W4-6.

## Wave 3 decisions — the store rides the snapshot

- **W3-1** The snapshot field is `checkpoints: Option<CheckpointsDoc>`, omitted with no slot and no
  `last_restore`, so a checkpoint-free snapshot is byte-identical. `CheckpointsDoc { slots, last_restore }`;
  a `SlotDoc` is the disk envelope's slot fields less `format`/`build` (`name`, `frame`, `game_frame`,
  `scene`, `scene_path`, `authored_names`, `slot`: the slot's own snapshot document, never nesting a
  store). `last_restore` rides because a game reads `Checkpoints::last_restore()` to reseed after a
  restore; without it a seek would diverge. `disk` and `disk_error` are host facts and do not ride. Only
  `snapshot::capture` fills the field; the slot capture does not, so slot capture cost is unchanged. A
  structured slot's rendered document is cached on first use, so a recording renders each slot once.
- **W3-2** The `WorldHash` term is a rule-6 resource digest over the slot **metadata**: per slot in name
  order, the name, `frame`, `game_frame`, scene name, `scene_path`, `authored_names` and the slot's clock
  fields; `None` (no bytes) for an empty store. Not the body: a body is a pure function of hashed state at
  the capture frame, whose component content the per-frame hash already covers; it must hash equal whether
  `Structured` or a `Document` (after a keyframe restore); and hashing a whole world per slot would break
  the 4 ms capture budget. A body divergence surfaces as component divergence at the next restore. The
  digest reader became `fn(&T) -> Option<u64>` so a resource in its identity state writes nothing; the
  `GameClock` term (`Plan/game-time-scale.md` decision 8) uses the same seam.
- **W3-3** A restore makes the `GameClock` non-identity (it rebases past the frames it gave up) and a held
  slot is a store term, so the hash after a restore is not the hash at the capture. "The restore is the
  capture" is a claim about world content, which the tests assert with a digest-free content hash.

## Wave 4 decisions — structured slots

- **W4-1** Registration requires `Clone` [BREAK, under the 2026-11-19 blanket ruling]: `Registry::register`,
  `register_validated`, `register_reserved` take `T: Clone`. The `Component` trait is unchanged.
- **W4-2** The structured slot: a typed `SnapshotDoc` (resources, id-only entity lists) plus one column copy
  per registered component, in registry order.
- **W4-3** Disk slots stay documents. `persist` renders the document from the structured slot, byte-identical
  to `snapshot::capture` at that frame. Both restores go through one decoded install path.
- **W4-4** Non-finite values are reported at persist (`L4208`), not at capture: a capture inspects no value.
- **W4-5** `serde(skip)` runtime fields (`Text::resolved`, `UiText::resolved`) ride the clone; they are pure
  functions of hashed state.
- **W4-6** Wire design (R14): `checkpoint.list` (read-only: slots, disk, last_restore);
  `checkpoint.capture` / `checkpoint.restore` / `checkpoint.discard {name}`, mutating behind the existing write
  guards and applied at the next boundary through the same `Checkpoints` requests; the reply carries the
  outcome after that boundary. R14, because new `Command` variants move the protocol schema and
  `replay_content_hash`.
- **W4-7** Budgets in W4b, on the fixture "5 000 entities with physics".

## Wave 5 decisions — the wire verbs

- **W5-1** Shapes. `checkpoint.list` (no params) and `checkpoint.capture` / `.restore` / `.discard {name}` all
  reply `ResponseBody::Checkpoints { slots: [{name, frame, game_frame, scene}], disk?, last_restore? }`;
  `last_restore` is `{outcome: restored, name, frame}` or `{outcome: refused, name, reason}`.
- **W5-2** Each mutating verb applies at the boundary it is dispatched at (dispatch runs between ticks),
  through the code the `Checkpoints` request drain runs (`capture_slot`, `restore_named`), and the reply is the
  store after it applied. A refused capture or restore is an `Err` carrying the diagnostic the drain reports
  (`L4200`-`L4204`); discarding an unknown name is not an error, as in the game API.
- **W5-3** `L4210` `CHECKPOINTS_NOT_ENABLED` for every verb, `list` included, on an engine built without
  `.checkpoints()`.
- **W5-4** Recording: `list` is a read and leaves no record; capture, restore and discard are recorded as
  commands, so a replay re-dispatches them, and a live command during playback is refused through
  `record_kind`. `persist` and `load_disk` have no wire verb.

## Wave 4b decisions — budgets

- **W4b-1** Measured by an `#[ignore]` release bench (`crates/ludion-engine/tests/checkpoint_bench.rs`), by the
  frame-difference method: a request made in tick N is drained in the same `advance_live()`, so the bench
  times whole calls and subtracts the frame before. No engine API, wire field or profiler key.
- **W4b-2** Budget files `budgets/checkpoints-<os>.toml` with their own schema (`threads`, `capture_ms`,
  `restore_ms`), the image-stats / reflection-parity precedent; the ceilings are the request's fixed targets
  (capture 4 ms, restore 50 ms), not 2x tripwires. Measured medians are owed to the pinned boxes.
- **W4b-3** The `checkpoint_capture` / `checkpoint_restore` profiler keys and a `ProfileReport` `checkpoints`
  section stay R14.
- **W4b-4** "Memory per slot <= 2x live component bytes" is not gated: the column heap size is not observable
  without per-type size hooks. Trigger: a game reporting slot memory pressure.

## Owner rulings (2026-11-19)

- Accepted, shipped in R14 (T10-A1); the corpus re-record is the final re-record wave. **(a)** W3's replay corpus re-record (the snapshot gains a field).
- Accepted, shipped in R14 (T10-A1). **(b)** The slot digest joining `WorldHash` (additive term only when the store is non-empty).
- Accepted, shipped in R14 (T10-C2a). **(c)** `SceneChangeKind::CheckpointRestore` (moves the wire schema hash).
- Accepted. **(d)** `Plan/phase-16-profile.md` decision 7 exception: a checkpoint keeps the live profile. Wave 1
  ships this behaviour; the decision text is amended only by a note pointing here.
- Accepted. **(e)** `carry_resource` resources crossing a checkpoint restore, and the matching
  `docs/scene-transitions.md` line. Wave 1 ships this.
- Accepted. **(f)** The RNG rewinds on restore.

## Out of scope

- Disk persistence, the snapshot field, the digest, structured slots and wire commands: Waves 2–4.
- `restore(name, keep: &[EntityId])` — trigger: a game that cannot carry meta state in the profile or a
  carried resource.
- Multiple named disk slots — trigger: a game with a save-slot screen.
- Recording a continued run — trigger: a game needing a replay that starts from a loaded checkpoint
  (`RunRecording::start` reloads scene text).
- Time scale (request 0011), camera zones (0010) and mixer state (0023): the rebase stays correct under
  them; nothing is built for them here.

## Status

- **W1 mechanized (r2372, 2026-11-18):** in-memory slots: the `Checkpoints` resource (`capture`, `restore`,
  `discard`, `slots`, `slot`, `last_restore`), `CheckpointConfig`, `Rebuild::Checkpoint`, the clock rebase, the
  keyframe skip and arm refusal, tests, `docs/checkpoints.md` and the `checkpoints` example.
- **W2 mechanized (r2373, 2026-11-18):** the disk slot: `persist`, `load_disk`, `discard_disk`, `disk` and
  `disk_error` in `crates/ludion-engine/src/checkpoints.rs`, the `save/checkpoint.json` envelope with a
  `BuildStamp`, strict refusal with a `.bak.<hash>` backup (`L4209`), `L4208` write failures; documented in
  `docs/checkpoints.md`.
- **W4a shipped (Lore 2450):** structured in-memory slots (`ColumnCopy`, `World::copy_column`/`install_column`,
  `SlotBody`, `capture_structured`, `restore_decoded`); `Registry::register*` require `Clone`; decisions W4-1..7.
- **W4b mechanized:** `checkpoint_bench.rs` (smoke test plus the ignored 5,001-entity bench),
  `budgets/checkpoints-{windows,macos}.toml` with the fixed targets, the corpus-test exclusion; the pinned
  medians are owed to the RTX 3090 box and the M4; decisions W4b-1..4.
- **W3 shipped in R14 (T10-A1), except (c):** the `checkpoints` snapshot field (slots and `last_restore`), the
  store installed on `engine.restore`, a replay seek and a RunSave resume (`StoreRow`), the `L4206` keyframe
  skip and `L4207` arm refusal lifted and both codes retired, the slot-metadata and `GameClock` `WorldHash`
  terms (`register_resource_digest` now takes `Option`), decisions W3-1..3. `SceneChangeKind::CheckpointRestore`
  (ruling (c)) touches the protocol and the editor's `sceneChange.ts` and rides the wire wave. The
  `replay_content_hash`, the committed replays and the WorldHash traces of sessions that held a slot or the
  clock move with the final re-record wave.
  Bench after W3 (RTX 3090 box, quiet, plain frame 0.50 ms, 3 runs): capture median 3.909 / 3.919 / 3.935 ms
  against the 4 ms target, restore 6.224 / 6.259 / 6.365 ms against 50 ms.
- **W5 shipped in R14 (T10-C2a):** the four `checkpoint.*` commands, `L4210`, ruling (c)
  (`CheckpointRestore`, also for a game's `restore` and `load_disk`), `ai.describe` beside them; decisions
  W5-1..4. The protocol schema snapshot, `replay_schema_hash`, `replay_content_hash`, the replay corpus and the
  editor's generated types move with the final re-record wave.
- M4 (2026-11-22): the W4b medians are measured on the pinned Apple M4, recorded in `budgets/checkpoints-macos.toml`'s MEASURED line: capture 1.31 ms and restore 2.08 ms, against the 4 ms and 50 ms targets. The RTX 3090 box's medians are still owed.
