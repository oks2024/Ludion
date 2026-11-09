---
description: Recording, replay, snapshots and keyframes: the reproduce-a-session machinery in ludion-engine
---

# Engine: recording, replay and snapshots

**Scope:** How a session is captured and reproduced — the recording predicate, the one stepping
driver, the snapshot document, keyframes, and divergence bisection. The `Engine` and dispatch are
`ludion devdocs ludion-engine`; the on-wire container and `REPLAY_FORMAT_VERSION` are
`ludion devdocs ludion-protocol`; why the traces must match at all is
`ludion devdocs determinism`. Sim-visible content reloads that a replay *cannot* regenerate are
`ludion devdocs engine-content-kinds`.

## Dataflow

**Arming.** `set_recording(sink)` writes the `ReplayHeader` — scene ref, seed, versions, the description-stripped
schema hash, the replay content hash, the profile-at-start, and for run recordings the game identity (see
[Game identity](#game-identity)) — plus an initial keyframe. Arm it right *after* loading the scene, before the
first tick: it is refused with `L3047` at a frame other than 0 or with no scene loaded, since the header
would not describe the state the recording starts from. The scene ref is the scene **text, inline**
(`scene.load` stores `SceneRef::Inline`, never the path it read), which is what makes a recording
self-contained: it reproduces the identical `WorldHash` sequence from any directory or machine.
A write that fails mid-session stops the recording and reports `L3042` through `Diagnostics`;
`stop_recording` returns it once.

**What gets written.** `record_kind` (`recording.rs`) is one exhaustive predicate with no wildcard
arm, so a new `Command` variant forces an explicit recording decision exactly as it forces a dispatch
handler:

- `input.inject` → an `Input` record at its *apply* frame, `max(params.frame, drain_frame)`, matching
  the driver's own clamp.
- world/sim/run-state mutations → `Command` records, an applied `render.size` included (the extent is
  sim input).
- `scene.load` → a `SceneLoad` record carrying the scene text it resolved, never re-read on replay.
- reads and transport/export (`component.get`, `query.entities`, `sim.hash`, `scene.check`/`save`,
  `render.*` other than an applied `render.size`, `engine.describe`/`snapshot`, `stream.*`) → never
  written. A pure read leaves no trace.
- `asset.reload` → the one conditional case; see `ludion devdocs engine-content-kinds`.

`Engine::execute()` is what records; `dispatch()` is the same routing **without** the recorder. It
captures the drain frame up front and records what the command applied: nothing for one that failed
before mutating, the applied form for one that failed midway (a `sim.step` stopped by a system panic
records the ticks it ran, `note_applied_before_error`), and a doc reload that swapped the store before
its command failed.

**Keyframes** — full snapshot documents — land at arming, every `KEYFRAME_INTERVAL` (300) frames, and
once at stop (unless nothing was recorded since the last keyframe or the session ended on a system
panic), as seek targets for `ReplayReader::seek`. The closing keyframe is why `verify_replay_keyframes`
checks a session's end, not only its last interval boundary.

**Replay.** `replay_hashes(bytes, project_root)` rejects a mismatched protocol version, an absent or stale replay content hash and a stale
description-stripped schema hash (all `L3010`; `compatibility` in `replay_compat.rs` is the gate), loads the header's scene into a fresh engine, reseeds,
reconstructs the command list from the delta records (keyframes kept as inert actions — a linear replay
replays, it does not seek), and feeds it to `drive_actions`, after installing the scene-switch text map (always, even empty, so a replay never reads a switch target off disk), which decomposes each `sim.step` into single ticks so
the returned `(FrameIndex, WorldHash)` trace samples every tick boundary. Live session and replay run
the *same* `execute` path, which is why the traces are identical rather than merely similar.

A system panic is a recorded fact, not something the loop forgives. The recorder writes a
`system_panic` record (kind 7) right after the `SimStep` whose last tick panicked — a free-run batch is
flushed to end at that tick, a lockstep step is already cut there. `prepare_replay` folds the pair into
one `PanickedStep` action (a marker that follows no `sim.step` ending on its frame is `L3010`), and the
step loop accepts `L4007` only on that action's last tick: a panic anywhere else propagates as `L4007`,
a marked tick that ticks cleanly is `L3050`. `drive_hashes` carries no markers, so it propagates every
`L4007`; an old `.lrp` without markers replays strictly.

`drive_actions_with` is that driver with its boundaries opened up — a `DriveHook` decides what happens
at each recorded keyframe and how each tick runs — and `drive_actions` is it with the `HashTrace`
hook; `PlainDrive` is the hook-free drive (`query_drift`, profile and bisect use it). There is deliberately no second copy of the stepping-and-hashing logic, so an instrumented
drive cannot step differently from a plain one.

**Snapshots** (`snapshot.rs`, format version 1) are self-describing JSON: format and engine version,
frame, run-state, speed, seed, scene ref, the RNG state verbatim, the embedded profile, and every live
entity with its `(index, generation)` and components in registration order. Restore is exact for the
*observable* world — same live-id set, same components, same continuing RNG — and rebuilds the driver
while leaving registry, project root, render settings, subscriptions and recording sink untouched
(`L3008` on a malformed or incompatible document). The same document doubles as the replay keyframe
format.

A snapshot of a session running rigid-body dynamics also carries one additive `physics` field:
base64 of the solver state's binary codec, absent when the session has no `PhysicsWorld` resource.
It is opaque text rather than structured JSON because the solver's state is closed by its own type —
building a JSON value out of it fails on the narrow phase's 128-bit masks — and base64 rather than a
byte array because `serde_json` writes a `Vec<u8>` as an array of numbers. Like `allocator`,
`doc_reloads` and `residue` it is optional and `skip_serializing_if`, so a physics-free snapshot is
byte-identical to what it was and `SNAPSHOT_FORMAT` stays `1`; unlike them, the bytes are the pinned
solver's own serialization, so a snapshot outlives neither a solver-pin move nor a codec major bump
(`L3033`, and the reason a pin move is a snapshot-format event). Restore inserts the decoded world
as a resource into the freshly rebuilt world before the driver takes it; a document without the
field leaves the restored session with no solver rather than stale bodies. The blob is invisible to
`WorldHash` — the solver is a resource, and what reaches the hash is the pose and `Velocity` the
mirror writes back into components, which is why a solver divergence surfaces in the trace one tick
after it happens. Because a replay keyframe *is* an engine snapshot, a `.lrp` recorded from a
physics scene carries the blob in every keyframe.

**Divergence bisection** (`bisect.rs`) turns "the hashes differ at frame N" — which localizes
nothing — into a named entity, component and system. Two tools over one world-diff core, both
single-threaded. The core is public: `WorldCapture` (an owned world reduction), `world_diff(&a, &b)`,
and the read-only `TickObserver` (`Engine::set_tick_observer`, hooked in `advance_live` only) that the
testkit's `assert_deterministic` is built on:

- `verify_replay_keyframes(builder, bytes, root)` re-drives a committed `.lrp` with today's code and
  compares live state against every embedded keyframe, reporting the **first** that fails
  (`KeyframeDriftReport` → `L3024`): entity-set differences, per-component JSON value diffs bounded at
  `ENTITY_DETAIL_CAP` entities, plus separate flags for RNG state and the allocator free list — both
  excluded from `WorldHash`, so they are divergence that has not surfaced in component data *yet*. It
  deliberately ignores run-state, speed, scene name and the embedded profile: a free-run recording
  keyframes while `Running` where a replay steps, so those differ on a perfectly good replay.
- `bisect_divergence(left, right, bytes, root)` takes two `RunConfig`s — a builder factory plus an
  ambient scope closure (`DriveDone` makes forgetting to run the body a compile error), so the two
  sides may differ by thread count, a feature flag, or the builder alone. It drives both, finds the
  first divergent frame, then re-drives *that one frame* under each with `Schedule::run_probed` and
  names the first system after which the two worlds part company, plus the component diff there.
  `localize_divergence` is the same when the caller already holds two traces.

A replay whose frame numbers are not monotonic — an `engine.restore`, or a `scene.load` after a tick —
is refused up front with `L3021` (`require_monotonic_frames`, shared with replay-to-frames and
`--profile-replay`), since "frame N" would name two states. The frame-0 and end-of-frame captures
include the actions recorded at that frame, so they show the world the trace's hash covered.

Both drives run in one process. The probed drives skip per-frame hashing (they exist only to reach the
frame), and localization re-drives four times rather than holding a snapshot per system boundary, so
memory stays one world regardless of schedule length.

## Game identity

A run recording's header carries `game: Some(GameIdentity { schema, version })`: the description-stripped hash of
the registered component and data-doc schemas (`run_save::game_schema_hash`, built-in engine components included,
so an engine component change also reads as a mismatch) and the `EngineBuilder::game_version` string, for changes
the schema cannot see, such as balance. Session recordings (`set_recording`, `--record`) write none, so their
headers keep their bytes: a session file is verified against an expected `WorldHash` trace, and an identity would
refuse every committed corpus file on each game component edit, whereas a player file has no expected trace and
would diverge silently. The gate (`replay_compat::check_game`, `L1404`) runs in `prepare_replay` right after the
game engine is built, so `L3010` (engine) comes first and `L1404` (game) second, covering every replay entry. An
absent field passes. `Engine::game_identity()` lets a replay menu compare a file's `header().game` without
replaying it.

## Rules

- **Recording and replay share one driver.** Any new stepping path goes through `drive_hashes` or
  `drive_actions_with`, never a fresh loop.
- **Recording policy is one exhaustive predicate.** Adding a `Command` variant without deciding its
  `record_kind` is a compile error, by design.
- **A recording is self-contained.** Anything a replay cannot regenerate from the sim must travel
  *inline* in the `.lrp` — the scene text (a mid-session `scene.load` is a `SceneLoad` record holding
  the text it read; older `.lrp`s hold it as a plain `Command` that replays from disk), a reloaded
  doc's text, a switched district's text. A path reference would make the recording reproduce only
  on the machine that made it.
- **Additive records only.** New record kinds carry `#[serde(default)]` and leave
  `REPLAY_FORMAT_VERSION`/`SNAPSHOT_FORMAT` alone, so committed `.lrp` files keep decoding.
- **A recording carries its render extent, but golden sessions are still recorded at the default.**
  The extent is sim input (a game unprojects the cursor through `RenderExtent`), so the header holds
  the extent at arm time (`render_extent`) and an applied `render.size` is a `Command` record; a
  replay installs the header extent through `render.size` before it loads the scene, so any `.lrp`
  replays at its own extent. The comparability half remains: `InputEvent::MouseMove` carries
  *render-target pixel* positions, so the same play session at 960×540 and at 1280×720 records two
  different input traces with two different hash sequences. Each is internally sound; they are not
  comparable, and a `games/*/run.lrp` recorded from a shrunken editor viewport silently commits a
  trace no one can reproduce by playing the same way at the default extent. A header without
  `render_extent` is a pre-field `.lrp`, recorded at the default by convention.
- **Keyframes carry presentation state; verify and bisect compare sim only.** A seek restores the
  `presentation` entities with the sim world, but `verify_replay_keyframes` and `bisect_divergence`
  never look at them, so a build without the presentation system still verifies clean
  (`Plan/presentation-state.md`).

## Gotchas

The traps this machinery has actually sprung — keyframe alignment by record order, the 1-ULP JSON
comparison that faked a divergence, arming order, and what a restore does not capture — are in
`ludion devdocs ludion-engine`'s `## Gotchas`, which owns them for the whole crate.

## Verify

```
cargo run -p ludion-cli -- sim-hash --scene examples/gallery/meshes.yaml --frames 60 --record target/session.lrp
cargo run -p ludion-cli -- replay target/session.lrp --hashes
cargo nextest run -p ludion-engine --test it -E 'test(/^dispatch::/)'        # snapshot/restore determinism, replay round-trip; no GPU
cargo run -p breakout -- --replay-verify games/breakout/run.lrp
```

The `replay` run must print the same final hash the recording run printed — the round-trip check in
miniature.
