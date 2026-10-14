# Audit: the replay and record pipeline (2026-10-03)

**Scope.** A report-only read of everything that produces or consumes a `.lrp`: recording
(`crates/ludion-engine/src/recording.rs` and the recorder seams in `engine.rs`: `execute`/`record`/
`write_record`, `finish_live_tick`, `free_run_keyframe`, `drain_scene_switch`, `drain_prefab_texts`,
`set_recording`/`stop_recording`/`Drop`), the container (`crates/ludion-protocol/src/replay.rs`), the
drive (`crates/ludion-engine/src/replay.rs`: `prepare_replay`, `drive_actions_with`, `replay_with`,
`replay_hashes_with`, `replay_render_frames`), `bisect.rs` (`verify_replay_keyframes`,
`bisect_divergence`, `probe_frame`), the `--profile-replay` path in `profile.rs` (tails,
`frame_total_max_ms`, `[systems_max]`, `L4005`/`L4006`), `query_drift.rs`, the game-binary replay modes
in `crates/ludion/src/lib.rs`, the CLI verbs `ludion replay`/`sim-hash`
(`crates/ludion-cli/src/{replay,sim_hash}.rs`), the docs `docs/running.md` and
`dev-docs/engine-replay.md`, and vigil's replay consumers (`games/vigil/tests/{replay,thread_identity,profile}.rs`).
Commits c89b4828 (settled-hook refactor, `replay_with`) and c2acf84f (`--profile-replay`) were read as
diffs. Git mirror rev `fe19a265`. Static reading only; no test was run.

## Overall assessment

The pipeline is in good shape where it matters most: the container decoder is bounds-checked and
bomb-proof, recording policy is one exhaustive predicate, and the c89b4828 refactor really did leave
one drive loop. Its settled-frame trace is equivalent to the old push-then-refine trace in every case
traced (initial frame, trailing commands, early stop, keyframe stop). The problems are at the edges of
that loop's contract. `drive_hashes` swallows a system panic on the last tick of every `sim.step`, in
live drives as well as replays. Every frame-windowed tool (`replay_render_frames`, `profile_replay`,
the bisector) assumes frame indices only grow, but a recorded `scene.load` or `engine.restore` breaks
that. The new profiler names per-system timings by schedule position, so its names go wrong when a
rebuild changes the schedule. No High finding. Most of the rest is error-UX and doc drift that
c2acf84f left behind.

## High

None.

## Medium

### R1 — The drive loop swallows a system panic on the last tick of every `sim.step`, live drives included · Medium · [verified]

**Evidence.** `crates/ludion-engine/src/replay.rs:150-159` (`recorded_panic`) and `:209-215`
(`run_actions`): on any `sim.step` action, an `L4007` from the tick hook on `tick + 1 == params.ticks`
is turned into `Ok(true)` and the drive continues. `drive_hashes` (`:59-67`) goes through the same
`run_actions`, so this applies to a **live** session drive too, not only to a replay. The engine
pauses on a panic (`engine.rs:1114-1125`), but the next action's `sim.step` re-arms stepping
(`ludion-sim/src/driver.rs:248-254`), so the drive keeps ticking.

**Why it matters.** The recording carries no marker that a panic happened. A `SimStep(n)` record looks
the same whether or not its last tick panicked. So the drive cannot tell a reproduced panic from a new
one, and both are accepted silently. `docs/running.md:104-109` tells agents to drive their recorded
sessions with exactly `drive_hashes`. vigil's committed-session test
(`games/vigil/tests/replay.rs:93-96`) drives `session_commands()` through it. Each leg is
`step(ticks)`, so every leg's last tick is a swallow point.

**Failure scenario.** A gameplay change makes a system panic on the frame a walk leg ends.
`drive_hashes` returns `Ok` with the live trace. The fresh recording replays to the identical trace,
because replay swallows the same panic. The round-trip test is green, and the panic shows up only on
stderr/the `log` channel. `replay_with` observers, `--profile-replay` and `verify_replay_keyframes`
(when the panicking system wrote nothing hashed) are green the same way. In a lockstep recording made
of `SimStep(1)` records (`sim-hash` style per-tick stepping, or any client stepping one tick at a
time), *every* tick is a batch's last tick, so no panic anywhere in the replay is ever reported.

**Suggested fix.** Make the reproduction explicit. Record the panic when it happens (for example a
`panicked: true` field on the flushed `SimStep` record, or a dedicated record), and accept `L4007` in
replay only where that marker is present. Keep `drive_hashes` on a live engine strict: propagate every
`L4007`.

### R2 — Frame-windowed replay tools assume monotonic frames; a recorded `scene.load`/`engine.restore` resets the frame · Medium · [verified]

**Evidence.** A scene load resets the frame to 0 (`engine.rs:2504-2563`, a fresh `SimDriver`), and a
restore jumps to the snapshot's frame. Both replay as ordinary actions (`replay.rs:231-237`, the
`EngineRestore` command). But:
- `total_frames` (`replay.rs:548-556`) sums every `sim.step`'s ticks across the reset, so `--to`
  validation (`resolve_window`, `:515-544`) accepts frames the engine never reaches.
- `RenderHook::settled` (`replay.rs:568-583`) stops at the first `frame > to` ("Frames are
  monotonic"), and its `last_rendered` dedupe keys on a bare frame index.
- `ReplayProfile::tick` (`profile.rs:510-551`) windows and labels by `engine.frame()`.
- `first_divergent_frame` (`bisect.rs:696-704`) returns a bare frame index, and `ProbeDrive::tick`
  (`bisect.rs:884-916`) probes the **first** tick that produces it.

**Why it matters.** These are the debugging instruments. On such a replay they return `Ok` with
wrong output instead of a diagnostic.

**Failure scenario.** A session records 600 ticks, then a `scene.load` (an editor reload, or the
vigil stress shape after a warm-up), then 600 more. `--replay-frames --to 900` passes validation
(total 1200). Rendering covers frames 0..=600 of the first world, then 0..=600 of the second
**overwriting the same `frame-NNNNN.png` files**, and it never stops early. `--profile-replay --from
100 --to 200` measures both worlds' frames 100..200 under the same indices, so `max_frame` is
ambiguous. A thread-count divergence at post-load frame 50 is probed at **pre-load** frame 50, and the
bisect report blames a system in a world that never diverged. The committed corpus is not affected
today: `stress.lrp` loads its scene at frame 0 (`games/vigil/tests/replay.rs:240-250`), so the reset
is 0 to 0.

**Suggested fix.** Key windows and divergences by drive position (a monotonic tick counter the drive
owns) and not by `engine.frame()`, or refuse a frame window on a replay whose actions contain a
world-resetting action, with a readable `L3021`.

### R3 — `--profile-replay` attributes per-system timings by schedule position, which a rebuild can shift · Medium · [verified]

**Evidence.** `ReplayProfile::tick` captures `names` once, from the first measured frame
(`profile.rs:541-549`). Every later frame's `observer.frame` is then pushed into `per_system[i]` by
index. `reduce_sim` (`:311-381`) labels column `i` with `names[i]`. The schedule is rebuilt on every
load, switch and restore (`engine.rs:2533-2534`, `transition.rs:462`, `snapshot.rs:672`). In
`build_schedule` (`engine.rs:715-734`), `path_follow` is **prepended** only when the new world holds a
`PathFollower`, and the physics systems are inserted between the sim and presentation systems only
when the scene has a `physics:` block.

**Why it matters.** The mode exists to answer "which system hitches, at which frame". Once the
schedule shape changes mid-replay, every following sample is credited to a neighbour. `[systems_max]`
then gates the wrong system and names it in an `L4005`.

**Failure scenario.** A replay crosses from a district without a `physics:` block into one with it.
The physics step's spikes are reported under the first presentation system's name, and a budget on
the real physics system never sees them. vigil has no such district today, so this is latent there,
but the shmup and physics games are exactly the users of this mode.

**Suggested fix.** Accumulate by system name (a `BTreeMap<&'static str, Vec<Duration>>`, ordered by
first appearance), or reset the columns when the observed name sequence changes.

## Low

### R4 — `check_query_drift_replay` computes and throws away a full per-frame `WorldHash` trace · Low · [verified]

`crates/ludion-engine/src/query_drift.rs:150-152` calls `crate::replay::drive_actions`, which hashes
every settled frame (`HashTrace`, `replay.rs:163-183`). The caller only wants the query observations.
`Plan/code-audit-2026-07-25.md` C10 measured that hash as the dominant per-frame cost of long replays,
and vigil's `segments.rs:1216` pays it over a whole segment corpus. Fix:
`drive_actions_with(&mut engine, actions, &mut PlainDrive)`. `probe_frame` already made this exact
change for the same reason (`bisect.rs:847-849`).

### R5 — A `--profile-replay` window that measures nothing passes `--profile-check` · Low · [verified]

`resolve_window` accepts `--from 0 --to 0` (and any replay whose total is 0). `ReplayProfile::tick`
measures produced frames `>= 1` only (`profile.rs:512-519`), so zero frames are measured. `median_ms`
of an empty set is `0.0` (`profile.rs:713-716`), so a `frame_total_ms` ceiling passes, and
`frame_total_max_ms` is skipped because there is no tail (`:991`). The gate is green with no
measurement. Each `[systems]`/`[systems_max]` key also becomes a misleading `L4006` "unknown system"
(`:1002-1031`), because the measured list is empty. Fix: treat `measured_frames == 0` as an `L3021`
("the window contains no tick").

### R6 — The bisector's frame-0 and end-of-frame captures leave out the actions applied at the divergent frame; its "never reaches" error borrows `L3008` · Low · [verified]

A trace entry for frame N is the world *after* every action recorded at N (`replay.rs:114-118`).
`initial_doc` (`bisect.rs:799-806`) diffs the world straight out of `prepare_replay`, before any
frame-0 action. vigil's session opens with a frame-0 `component.set` (`pin_ci_duration`). In
`CaptureEnd`, `probe_frame` captures right after the probed tick (`bisect.rs:863-865`), also before
frame N's actions. If the divergence entered through one of those actions, the report says "the
divergence entered outside the schedule (a command applied at this frame…)" and then prints "worlds
are identical". Separately, "this replay never reaches frame N" is raised as
`PROTOCOL_SNAPSHOT_INVALID` (`bisect.rs:853-861`). `L3008`'s registered meaning is a malformed
`engine.restore` document, so `ludion explain L3008` misleads. Fix: apply the frame's remaining
non-step actions before capturing (or say plainly that the capture predates them), and raise `L3021`
for an unreachable frame.

### R7 — Keyframe verification is vacuous for short recordings and for every recording's tail · Low · [verified]

Keyframes are written at arm time and then every `KEYFRAME_INTERVAL` (300) frames
(`engine.rs:3456-3477`, `:3596-3604`, `:1153-1165`). `stop_recording` (`:3489-3503`) writes no final
one. So `verify_replay_keyframes` checks only the arm-time state of any recording shorter than 300
frames, and nothing after the last interval boundary of a longer one.
`committed_replays_reproduce_their_recorded_keyframes` (`games/vigil/tests/thread_identity.rs:157-178`)
verifies `stress.lrp`: 30 ticks armed on `EMPTY_STAGE`, so its only keyframe is an **empty world**.
The stress half of that gate cannot fail on gameplay drift. Fix: write a closing keyframe in
`stop_recording`. This is additive under the existing format and requires re-recording the corpus.

### R8 — With zero `SceneSwitch` records, a replay resolves a regenerated switch from disk · Low · [verified]

`prepare_replay` installs the off-disk scene map only `if !scene_text.is_empty()`
(`replay.rs:681-683`). Otherwise `drain_scene_switch` takes the live disk path
(`engine.rs:1509-1519`). The live recorder writes no record for a rejected switch
(`engine.rs:1527-1533`). So a session whose only switch request was rejected live (target missing on
the recording machine) replays on a checkout where the target exists, performs the switch, and
diverges. Prefab texts already took the stricter posture ("Always installed, even empty",
`replay.rs:684-686`). Fix: always install the map for a replay. Pre-E7 replays never requested a
switch, so their behaviour does not change.

### R9 — Hostile tick counts overflow `total_frames` · Low · [verified]

`total_frames` (`replay.rs:548-556`) is a plain `u64` `.sum()`. Two `SimStep` records with
`ticks` near `u64::MAX` (a corrupt or fuzzed `.lrp`) panic with "attempt to add with overflow" in
debug builds (`replay_render_frames`, `profile_replay`) instead of returning an `L3010`. The reader's
fuzz target covers framing only, not `prepare_replay`. Fix: `checked_add`, mapped to `L3010`.

### R10 — `set_recording` arms silently on a session it cannot reproduce · Low · [verified]

`replay_header` (`engine.rs:3506-3522`) records the scene *text* and the seed, and replay always
starts from a fresh load of that text at frame 0. Arming after ticks or edits (frame > 0, mutated
world, advanced RNG) produces a header that does not describe the session. The deltas are stamped
from frame F, so the replay diverges from its first tick. Arming with no scene loaded writes an empty
inline document, and the replay then fails with an unrelated scene-parse diagnostic. The only guard is
prose ("Arm it after loading the scene", `dev-docs/engine-replay.md`). Every shipped host arms right
after the load, so this hits only API users. Fix: refuse to arm at `frame() != 0` or with no scene,
with an `L####` naming the fix. Alternatively, write the header from the arm-time snapshot.

### R11 — Stale docs and comments around the refactor · Low · [verified]

- `L3021`'s const doc and `ludion explain` text (`crates/ludion-core/src/codes.rs:1440-1443`,
  `:6934-6950`) describe only the replay-to-frames render. c2acf84f also raises it from
  `--profile-replay` (`profile.rs:452`).
- `replay_project_root`'s doc (`crates/ludion/src/lib.rs:731`) lists `--replay-verify` and
  `--replay-frames` but not `--profile-replay`, which also uses it (`:873`).
- `prepare_replay`'s comment "keyframes are seek targets, skipped here" (`replay.rs:618`) contradicts
  the arm 45 lines later that pushes them as actions (`:663-666`). The module doc (`:12-13`) says the
  same.
- `replay_hashes`' doc says it replays "through [`drive_hashes`]" (`replay.rs:255`). It goes through
  `drive_actions`.
- `ReplayProfile::tick`'s comment "a recorded last-tick panic is the replay's reproduction, anywhere
  else a divergence" (`profile.rs:525-526`) sits on code that always returns the `Err`. The decision
  is made in `run_actions`.
- `dev-docs/engine-replay.md:39` says `execute` "writes only on success — a failed command did not
  mutate". The code records the partially applied form of a failed `sim.step` and a doc reload that
  applied before its command failed (`engine.rs:2404-2425`). The same doc's "`drive_actions` is it with
  `PlainDrive` and hashing on" predates `HashTrace`.
- `Recorder::pending_ticks`' doc (`recording.rs:148-150`) says the batch is flushed before a
  "scene-switch" delta. `write_scene_switch` (`recording.rs:282-292`) does not flush, so a free-run
  `SceneSwitch` record precedes the `SimStep` that reaches its frame. This is harmless today because
  replay only pre-scans switch records, but a frame-ordered stream is what a future seek consumer will
  assume.

## Checked and healthy

- **Container decoding** (`ludion-protocol/src/replay.rs`). Every slice is `checked_add`-bounded. An
  unknown kind (`> KIND_MAX`) is refused. Each frame streams through `take(ceiling + 1)`, so a bomb
  never allocates past its ceiling. The writer refuses exactly what the reader would
  (`check_writable` uses the same `frame_ceiling`). Truncation, bad JSON and bad zstd map to
  `L3010`/`L3011`/`L3017` with help text.
- **Compatibility gate.** `check_compatible` rejects a protocol/schema mismatch as `L3010` before any
  engine is built, and the container version is checked in `ReplayReader::new`.
- **Render-extent header.** It is installed before the scene loads through `apply_recorded`
  (validated, scale-check skipped), and absent means the default extent. Recorded `render.size`
  commands replay the same way. `render_png` at a different `--width/--height` does not move the sim
  extent.
- **The c89b4828 refactor.** The settled-hook trace reproduces the old push-then-refine trace in
  every case: empty action list, trailing non-step actions, `tick` returning false, keyframe stop.
  `replay_with`'s `Break` ends the drive with no extra final call. `RenderHook`'s `last_rendered`
  prevents a double render of the final frame. No second copy of the drive loop remains:
  `query_drift`, `bisect` and `profile` all go through `prepare_replay` + `drive_actions_with`.
- **Free-run batching.** `flush_pending_ticks` stamps `[batch_end - pending, batch_end)` correctly
  for inputs, commands, doc reloads, prefab texts and keyframes. No path traced gives a stamp below
  the batch start, so the subtraction cannot underflow. The panicking tick is batched without a
  keyframe, and pure reads do not split a batch. `Drop` flushes the tail.
- **Recording policy.** `record_kind` has no wildcard arm. The input apply frame
  `max(params.frame, drain)` matches the driver clamp, and `sim-hash`'s future-frame inputs replay
  correctly ahead of their `SimStep`.
- **Determinism hygiene.** The replay path uses `BTreeMap` (scene text, `PrefabTextSource`) and no
  std hash collections. Wall-clock reads are confined to sanctioned profile sites. Reports carry
  `sim.threads`. Prefab epochs count the same rebuilds live and in replay (restore/load/switch;
  rejected switches count on neither side).
- **`--profile-replay` hash neutrality.** `step_ticks_observed` runs the same `drain_frame_boundary`
  as `step_ticks`, and `measure_sequential`'s zero-tick `sim.step` restores the run state. Both are
  pinned by `replay_profile_is_hash_neutral`. The nearest-rank p99 and first-max tie rule match their
  docs. The `max` and `L4006` budget rules are documented consistently in `codes.rs`,
  `dev-docs/budget-files.md` and `docs/running.md`.
- **Docs mirror.** `docs/running.md` is byte-identical to `crates/ludion-cli/embedded-docs/running.md`.
  Its `replay_with`/`drive_hashes` examples match the signatures.
- **Keyframe comparison.** Values are normalized through the registered type (the 1-ULP
  `serde_json` issue). RNG and free-list drift are reported separately. Run-state, speed and profile
  text are deliberately excluded, as documented.

## Suggested order of work

1. **R1**: make panic reproduction explicit and keep live drives strict. It is the only finding that
   turns committed-session tests silently green.
2. **R3**, then **R2**: key profile columns by system name, and refuse or position-key frame windows
   across world resets, before the shmup and physics games lean on `--profile-replay` and the
   bisector.
3. **R7**: a closing keyframe in `stop_recording`, done in the same corpus re-record as the next
   hash-moving change.
4. **R4**, **R5**, **R6**, **R9**: small, local fixes, each a few lines.
5. **R8**, **R10**: posture fixes, one line and one guard respectively.
6. **R11**: doc and comment sweep. `L3021`'s explain text first, since it is user-facing.
