# Audit: snapshot/restore and the `ludion dev` hot-reload loop (2026-10-03)

**Scope.** `engine.snapshot` / `engine.restore` on the engine side
(`crates/ludion-engine/src/snapshot.rs`, `commands/engine_cmds.rs`, the `install_restored` /
`commit_world` rebuild table in `engine.rs`, `docs.rs`'s reload re-application, `field_diff.rs`,
`ludion-sim`'s `SimDriver::restore`), the `ludion dev` supervisor (`crates/ludion-cli/src/dev.rs`,
its one integration test `crates/ludion-cli/tests/dev.rs`), the game host boot path it relaunches
(`crates/ludion/src/lib.rs`, `host.rs`), the restore-equivalence tests in
`crates/ludion-engine/tests/`, and `games/vigil`'s resources and schedule. Docs read:
`docs/dev-loop.md`, `dev-docs/ludion-cli.md`, `dev-docs/ludion-engine.md`,
`dev-docs/engine-replay.md`, `docs/scene-transitions.md`. Rev `629339c0` (git mirror). Static
reading only; no test was run. Not re-reported: physics P1 (`fresh_colliders`), replay R2/R10, wire
W1/W2 (frame-size limit, stuck-sim shutdown), and the July audits' items (stale-handle aliasing,
dead-child recovery, doc-reload log, fingerprint purge), all of which are fixed or tracked.

## Overall assessment

The engine half is in good shape. The snapshot is a single additive document. Validation runs
before the engine is touched, so a failed restore leaves the old world whole. The allocator,
presentation range, doc-reload log, residue, profile, game clock and held input all round-trip, and
per-component drift becomes a structured report instead of a crash. The weak half is the `dev`
supervisor's failure branches. Only its happy path has an integration test, and two of the
remaining branches lose the designer's state or report success when no reload happened. The restore
contract itself has two gaps: the doc-reload log can pin stale doc text over a newer file, and the
documented "resources are derived caches" assumption does not hold for vigil's `NavField`. No test
checks that a vigil run resumed from a snapshot continues the same as an uninterrupted one.

## High

### D1 — When the rebuilt child fails to boot, `dev` throws away the snapshot it already took and exits · High · [verified]

**Evidence.** `reload` (`crates/ludion-cli/src/dev.rs:973-1031`) takes the snapshot, shuts the
old child down (`drop(old)`, `:1023-1024`), then launches the new binary. If `launch` fails it
prints the diagnostic and returns `ReloadOutcome::Fatal` (`:1026-1031`). The in-memory `snapshot`
is dropped without being written to disk. Only the restore-failed branch calls `write_snapshot`
(`:1050-1056`). `Fatal` ends `supervise_loop` (`:820`), so `dev` exits with a failure code. No
status is written on that path either, so `dev-status.json` stays at `building` (`:941`) for an
editor tailing it.

The new child boots its scene before it can be restored. `ludion::run` loads `--scene` (or the
builder default) through `load_scene_file` and exits non-zero on any diagnostic, before it binds
or prints `listening` (`crates/ludion/src/lib.rs:698-703`). Any source edit that makes that file
fail to load in the new build therefore kills the session: renaming a registered component the
scene uses (`L1013`), tightening a doc kind so a content doc no longer validates (`L1119`/`L1120`),
a builder-time validator error, or a panic in builder setup. `docs/dev-loop.md` presents the
renamed-component case as one the restore report handles ("a component you renamed/removed dropped
with an L3015/L3016 code"). In practice the boot load fails before restore runs. The world being
restored may not even come from that scene: a `scene.load` or district switch earlier in the
session moved it elsewhere, and the relaunch still boots the original `--scene`.

**Why it matters.** A failed build is documented as the safe case ("never kills the running
child"). A build that compiles but cannot boot is the next most common mistake in the loop, and
here it destroys both the session and the supervisor. `BIND_RETRY_WINDOW` also respawns the failing
child for 5 s first (`:1417-1428`). The `L0013` help then suggests a same-port bind race
(`:1920-1923`), which points the reader at the wrong cause.

**Failure scenario.** A designer has played vigil to district 4, frame 30 000. The agent renames
`Lantern` to `CarriedLight` in `src/` and saves. The build succeeds, the old child shuts down, the
new child exits with `L1013` on `scenes/main.yaml`, and `dev` prints `L0013` and exits. The run is
gone, and no `dev-snapshot.json` exists to recover it from.

**Fix.** Write the snapshot to `.ludion/dev-snapshot.json` (atomically) before shutting down the
old child, on every reload. On a launch failure, write a `failed` status, keep `dev` alive, and
retry the launch plus restore of the kept snapshot on the next save. Longer term, add a host
boot mode for reloads (`--restore <file>`, or `--paused` with no scene) so the relaunch never
depends on the `--scene` file loading under the new code.

## Medium

### D2 — A snapshot failure on a live but wedged child is reported as a successful build, and the reload never happens · Medium · [verified]

**Evidence.** On any `snapshot_and_shutdown` error, `reload` probes `try_wait`. If the process is
still alive, it writes `status_ok` and returns `Continued` (`dev.rs:982-995`). Nothing goes to
stdout: no status line covers this branch (module docs `:71-97`). `dev` sets no call timeout
anywhere (`Client::connect` at `:1622`, `:1652`, `:1675`; `with_call_timeout` is never used). Two
common triggers keep the child alive while the snapshot fails every time:

- **A system that loops forever** ("the engine's typical failure", wire audit W2). The sim thread
  never drains the intake, and the server answers `L3023` after `SIM_REPLY_TIMEOUT` = 120 s
  (`crates/ludion-server/src/connection.rs:600`). The agent fixes the loop and saves; `dev` builds,
  waits 120 s, writes `ok`, and leaves the wedged child serving. Every later save repeats this. The
  hard-kill fallback (`SHUTDOWN_GRACE`) is never reached, because it sits behind a successful
  snapshot.
- **A `ludion` CLI built from a different wire schema than the game** (a stale `ludion` on PATH, or
  a standalone game pinned to another engine rev). `Client::connect` fails with `L3003`
  (`crates/ludion-client/src/lib.rs:183-190`) on every reload, and the initial launch never
  connects, so nothing warns at startup.

**Why it matters.** This is a false green. The editor and an agent reading `dev-status.json` see
`ok` with no `restored_frame`, while the running binary predates the edit. The fixed code never
runs, and the only recovery is restarting `dev`.

**Fix.** Use `with_call_timeout` for snapshot, restore and resume (a few seconds for snapshot). On
a snapshot failure with a live child, kill it after a bounded wait, relaunch fresh from the new exe
(as `RelaunchFresh` already does), and write a distinct `crashed`-style status and stdout line.
`L3003` deserves its own message naming the version skew. Also, the `RelaunchFresh` branch
(`:996-1010`) never writes `ok` after a successful relaunch, so the status stays `crashed` while a
fresh child serves.

### D3 — The restore re-applies the session's doc-reload log over the disk walk, so a doc edited for a code change fails or silently reverts the reload · Medium · [verified]

**Evidence.** A live reload is logged only when the old binary accepts it
(`crates/ludion-engine/src/engine.rs:2087-2096`). A rejected edit (`L1120`, including "a field the
kind does not declare", `crates/ludion-core/src/codes.rs:460-465`) leaves the previous accepted
text in `session.doc_reloads`. `capture` snapshots that log (`snapshot.rs:372-380`). On restore,
`prepare_docs` walks the disk with the new binary's schemas, then re-applies every logged text on
top (`crates/ludion-engine/src/docs.rs:194-197`) and fails the whole restore on the first logged
text that no longer validates.

**Failure scenario.** Mid-session, the designer tunes `docs/enemies/rusher.yaml` live (accepted
and logged). The agent then adds a required `armor` field to the doc type and adds `armor: 5` to
the file. The file save comes first. The old child rejects it as an unknown field, so the log keeps
the old text. The code save follows. The new child boots fine from disk, but the restore re-applies
the logged text, which lacks `armor`, gets `L1120`, and fails. `dev` prints "restore failed;
running a fresh world". The kept `dev-snapshot.json` carries the same stale `doc_reloads` entry, so
retrying it over the wire fails the same way. If the new field is `#[serde(default)]`, nothing
fails: the stale text silently replaces the file's `armor: 5` with the default, and stays until the
file is touched again. Docs edited during the restart window (old child down, new child's watcher
not yet up) are masked the same way.

**Why it matters.** The log exists to make replay seeks and switches reproduce cross-machine (the
July audit's 2.1). That reasoning does not hold for a dev reload: the binary has changed, and disk
is the newer source of truth.

**Fix.** In `dev`, drop `doc_reloads` from the snapshot before restoring, the way
`release_held_input` drops `input` (`dev.rs:1643-1647`). The new child's boot walk already read
current disk. Alternatively, have restore skip a logged entry whose key's disk text differs from
the log and report it, rather than failing the whole restore.

### D4 — vigil's `NavField` carries state across frames but is a dropped resource, so the first tick after a restore takes a different branch · Medium · [verified]

**Evidence.** On restore, `commit_world` installs no game resources
(`crates/ludion-engine/src/engine.rs:4319-4326`), and vigil's `CLAUDE.md` calls
`NavField`/`SpatialIndex` "derived caches". `NavField`, however, is rebuilt by `update_flow_field`
at schedule position 774 (`games/vigil/src/lib.rs:774`) and read earlier in the same tick:

- by `spawn_enemies` (`:603`) through `spawn_candidates`, which returns empty without a field
  (`games/vigil/src/nav.rs:212-214`), and then `field_pending`, which is `true` with no field
  (`nav.rs:277-285`). An elite or ranged release then defers a frame;
- by an escort follower, which steers straight at its target instead of following the flow
  field when `field_pending` is true.

In an uninterrupted run, the previous frame's field is present with a matching static signature,
so these systems take the other branch. `SpatialIndex` is fine because `spatial_rebuild` runs first.

**Why it matters.** `snapshot.rs:9-10` promises a restore that is "exact for the observable world",
and the snapshot "doubles as the replay keyframe format". For vigil, a session resumed from a
snapshot can draw RNG and spawn on a different frame than the uninterrupted run, and nothing
detects it. No test restores a vigil world and compares hashes. The engine's resume-equivalence
tests (`allocator_restore`, `held_input`, the physics `*_reproduces_the_uninterrupted_trace` tests)
only cover engine state. This is the same class as physics P1. The code paths are verified; a
hash-level reproduction was not run.

**Fix.** Make the field history-free at its read sites: build it on demand when absent in
`field_pending`/`spawn_candidates`, or move `update_flow_field` ahead of its readers. Add a vigil
test that snapshots mid-run, restores into a fresh engine, steps N ticks, and compares the
`WorldHash` trace with the uninterrupted run, with an escort and a due elite in the window. More
generally, state in `ludion docs systems` (or the `carry_resource` docs) that a resource read before
its writer in the tick is cross-frame state and does not survive a restore.

## Low

### D5 — The migration report diffs only top-level keys, so nested-field drift restores as "clean" · Low · [verified]

`migrate_component` reports drift through `diff_fields` (`snapshot.rs:789-799`), which compares
only top-level object keys (`crates/ludion-engine/src/field_diff.rs:14-31`). A renamed or removed
field inside a nested struct with `#[serde(default)]` resets silently, and the report stays empty,
which `docs/dev-loop.md` defines as "a clean restore". Fix: recurse into nested objects and report
dotted paths.

### D6 — A snapshot from a newer build restores into an older one with fields silently dropped · Low · [verified]

`SnapshotDoc` does not deny unknown fields (`snapshot.rs:77-78`), and every addition since format 1
has been additive with `SNAPSHOT_FORMAT` left at 1. `engine_version` is written (`:347`) but never
read on restore. A newer snapshot (with `allocator`, `residue`, `doc_reloads` or `presentation`)
restored by an older engine, for example after a game pins an older ludion git rev mid-`dev`,
drops those fields, and the report is clean. Fix: warn (report entry or diagnostic) on a different
`engine_version` or on unknown top-level fields.

### D7 — `dev` prints one migration line per entity, so a change to a common component floods stdout · Low · [verified]

`print_report` emits one line per `(entity, component)` entry (`dev.rs:1684-1716`). Adding one
defaulted field to `Transform` in a horde scene prints thousands of identical lines and buries the
`restored` status line. Fix: group by `(component, filled, removed)` with an entity count, and keep
the per-entity list in the status file.

### D8 — Stale docs about what a snapshot holds and how the reload blink works · Low · [verified]

- `dev-docs/ludion-engine.md:117-119` says restore does not capture "dead-slot free-list
  generations". It has captured them since the `allocator` field (`snapshot.rs:15-21`, `:160-161`).
- `dev-docs/ludion-cli.md:133` and `dev.rs:707-709` say "the old child serves until the new one is
  listening". It is shut down before the new one is launched (`dev.rs:1023-1026`), as a same-port
  rebind requires. The blink is shutdown + boot + restore, not just snapshot → relaunch.
- `docs/dev-loop.md` lists what survives a reload but not that game resources (session latches,
  caches) and held input are dropped, or that the boot scene must still load under the new code
  (D1).

### D9 — `dev`'s failure branches have no integration coverage · Low · [verified]

`crates/ludion-cli/tests/dev.rs` has one end-to-end test (`dev_reloads_and_preserves_state_across_a_real_edit`).
The restore-failed, crashed-child, launch-failure and snapshot-failure branches are covered only
through pure helpers (`snapshot_failure_recovery`, `release_held_input`), which is how D1 and D2
went unnoticed. Fix: one scripted test per branch against the same scratch game, asserting the
status file and stdout lines.

## Checked and healthy

- **Transactionality.** Every failure point in `restore_into` (document shape, format, speed,
  `scene_physics` validation, allocator/live-set agreement `L3041`, physics blob `L3033`, doc
  preparation) runs on a detached world before `commit_world`, which cannot fail. A failed restore
  leaves the old engine intact.
- **Entity identity.** Live `(index, generation)`, dead-slot generations and free-list order
  round-trip for both the sim and presentation ranges. Entity-valued components (`Parent`) and the
  physics handle map stay valid without remapping. `allocator_restore.rs` checks that post-restore
  spawns match the uninterrupted run.
- **Session state.** RNG, frame, run-state, speed, seed, game clock (including mid-freeze), held
  input, the profile (from the embedded text, not disk), residue with `last_visit`, scene-physics
  settings and the solver blob are all captured. The prefab library is rebuilt fresh, with the
  "placement unknown after a restore" fallback documented. Engine-owned resources
  (`AnimationEvents`, path/collider/font libraries, debug draw, audio queue) are rewritten or
  re-derived before they are read.
- **Schema drift.** Unregistered components → `L3015`, undeserializable → `L3016` (dropped, never
  default-filled), top-level added/removed fields reported. Data docs are revalidated against the
  new schemas.
- **`dev` process hygiene.** Children run a session-scoped exe copy (no Windows link lock). Stdin
  EOF supervision means a child never outlives `dev`. Dead session directories are swept under a
  lock and grace period. The bind-race retry window, held-input release, the post-build mtime
  recheck for saves during a build, and the failed-build keep-serving path are all sound.
- No `#[serde(skip)]` fields on any engine or vigil component, so nothing registered is silently
  left out of a snapshot.

## Suggested order of work

1. **D1:** write the snapshot to disk before shutdown on every reload, and survive a boot failure.
   This is the one path that loses work outright.
2. **D2:** add call timeouts and kill-and-relaunch a wedged child. Fix the false `ok` and the
   stuck `crashed` status at the same time.
3. **D3:** strip `doc_reloads` in `dev` (one line, next to `release_held_input`).
4. **D4:** make `NavField` history-free at its readers, and add the vigil snapshot→restore→N-ticks
   hash test, which will also guard the next cache of this kind.
5. **D9** alongside 1–3, so each fix lands with a test of its branch. Then D5–D8.
