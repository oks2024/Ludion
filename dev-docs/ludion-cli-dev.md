---
description: The ludion dev hot-reload supervisor: snapshot/restore loop, timeouts, failure states, --window
---

# ludion-cli dev supervisor

**Scope:** `crates/ludion-cli/src/dev.rs`, the `ludion dev` rebuild-and-restore loop. The subcommand pattern and the `--threads` chain are in `ludion devdocs ludion-cli`; the user-facing loop is `ludion docs dev-loop`.

## Dataflow

**The `dev` supervisor** (`Plan/phase-7-designer-flow.md`, `engine-architecture.md` §8) is a peer
protocol client (`ludion-client`) plus a child-process manager, run *from the game's own directory* —
no `-p` selection, so a `games/*` member and a standalone scaffold behave identically; a directory
with no binary target is `L0012`. It builds, launches, waits for the child's `listening on ws://…`
line, then watches `src/` (`ludion_assets::watch` — behind that crate's opt-in `watch` feature, which
the CLI enables — ~300 ms debounce) and rebuilds on every change. On
a successful build only:

```
engine.snapshot (over the wire) → write .ludion/dev-snapshot.json (input and doc_reloads stripped)
  → engine.shutdown (2 s call timeout; the child's drop waits 5 s, then kills)
  → relaunch the fresh binary on the SAME port, --restore <the kept file>
```

The relaunch is the host's restore boot (`ludion::run`'s `--restore`): it builds the engine rooted
where `--scene` roots it, does **not** load the scene, runs `engine.restore` with the file, prints
`restored frame=<N> report=<JSON>` (or `restore failed: <diagnostic JSON>`) before `listening on`,
then binds. dev parses that line (`parse_boot_restore`) and prints the report. The restore
reinstalls the snapshot's run-state, so a running game resumes running, and a component the boot
scene uses that the new build dropped restores around it (`L3015`) instead of failing the boot. A
restore that fails falls back to loading the scene, so the child still comes up on a fresh world.
The host starts its asset watch before it prints `listening on`, so a file edit made once dev has
seen the line is never missed. These details make the loop safe to leave running:

- **A watcher batch with no source change since the last build is ignored** (`src_fingerprint`:
  file count, newest mtime, total size), so one save arriving as two batches never rebuilds twice.
- **Every wire call has a timeout** (`SNAPSHOT_CALL_TIMEOUT` 10 s, `SHUTDOWN_CALL_TIMEOUT` 2 s,
  `SHADER_RELOAD_CALL_TIMEOUT` 30 s; expiry is `L3009`). `Client::connect`'s handshake is bounded
  too (`HANDSHAKE_TIMEOUT`, 10 s).
- **A snapshot that fails never keeps the old child.** `snapshot_failure` splits an exited child
  from a live one that did not deliver (a wedged sim, a refusal, the `L3003` skew); both stop the
  child (`GameChild::drop`: stdin EOF, `SHUTDOWN_GRACE`, kill) and relaunch the new build fresh. The
  exited child prints `dev reload child exited; state lost, relaunching`, the live one
  `dev reload snapshot failed (<code>: <cause>); restarting fresh, session state lost`. Status goes
  `crashed` + `message` (no new state: the editor rejects unknown ones), then `ok` + `message` with no
  `restored_frame`. `explain_version_skew` gives `L3003` its own message, also warned once after the
  first launch.
- **The report is grouped on stdout, whole in the status.** `report_lines` prints one line per
  `(kind, component, fields)` group with an entity count and the first entity (an `L3016` group keys on
  the component alone and shows the first entry's error line as the example); the per-entity list
  is `report` on the reload's `ok`.

- **A failed build never kills the running child.** Cargo's errors pass through to dev's stderr and
  the last good binary keeps playing. `L0013` fires only when a child that *built* then dies or hangs
  before listening (the message carries its captured output; the bind window is retried only when the
  output shows a bind failure — `could not bind`, or silence — for same-port `TIME_WAIT` races).
- **A child that cannot boot never ends dev.** The snapshot is on disk before the old child is stopped
  (an unwritable path aborts the reload with the old child serving), so dev prints the `L0013`,
  `dev reload launch failed; snapshot kept at <path>, retrying on the next save`, writes status `failed`
  (diagnostics + `snapshot`), and the next save launches and restores the kept snapshot. The kept
  flag lives in `supervise_loop`; deleting the file drops it. `ReloadOutcome` has no fatal arm.
- **Children run a throwaway *copy* of the built binary**, so a rebuild never fails on Windows's lock
  of a running `.exe`. That decoupling is what lets a failed build leave the serving child untouched.
  The copies live in `<temp>/ludion-dev-session-<pid>/`, whose `lock` file dev holds locked for its
  life; the next `ludion dev` sweeps every session directory whose lock is free (a dead session).
- **A child never outlives dev.** dev launches it `--supervised` with a piped stdin it keeps open; the
  child (headless hosts only, conflicts with `--window`) exits gracefully on stdin EOF, and the OS
  closes the pipe however dev dies — one platform-neutral seam, no job object or process group.
- **A child's stderr reaches dev's stderr `[game]`-prefixed** for the whole session; before its
  `listening` line it is held in a 200-line ring for a failed launch's `L0013`.
- **The file exists iff a snapshot is kept, and a fresh start resumes it.** `relaunch_and_restore`
  deletes `dev-snapshot.json` after every successful restore (a delete error other than not-found is
  an `L0033` warning); `supervise_loop` starts `kept` from `is_file()`. `run` checks the file before
  the first launch and, when it is there, relaunches through `relaunch_and_restore` with
  `RestoreOrigin::Resume`: `dev resumed frame=<N> from the kept snapshot <path> in <ms> ms` and an `ok`
  status with `restored_frame` and a `message`; a failed restore prints `dev resume failed; …` and
  `restore_failed` with the file kept; a launch error ends dev, the file kept for the rerun.
- **A failed restore is loud, not silent.** The child fell back to the scene and runs unpaused, the
  snapshot stays at `<project>/.ludion/dev-snapshot.json`, and dev prints
  `dev reload restore failed; running a fresh world, snapshot kept at <path> (…)` and writes
  `dev-status.json` state `restore_failed` (with `message` and `snapshot`). The next save shuts the
  fresh world down *without* snapshotting it and retries the kept file; deleting it drops the snapshot.
- **The restored document is exactly the file:** `input` (the old child's key-ups cannot reach it) and
  `doc_reloads` (the session's live data-doc log, older than the disk the new child walked) are
  stripped before it is written.
- **`scenes/` is watched independently and never rebuilds or restores** — the running world is the
  designer's state. dev cannot classify a changed path (a game's doc kinds live in the game binary,
  not the CLI), so it prints one hint stating the rule for the whole directory; the precise per-file
  signal stays the `asset.reloaded` wire event.

Status lines on stdout are the exit-gate test's parse surface: `dev watching <src-dir>`, the child's
`listening on ws://…` relayed verbatim, `dev reload building`, `dev reload build failed`,
`dev reload restored frame=<N> in <ms> ms` (or `dev resumed frame=<N> from the kept snapshot …`, or
the restore-failed, launch-failed, snapshot-failed or child-exited line above). Wall clock is legitimate here — dev is the supervisor, not sim-side,
so its `clock` module carries the one documented denylist `allow`.

**`--window`** hosts one persistent native window *in the dev process itself* across every restart:
the supervision loop moves to a worker thread so winit keeps the main thread, and the window connects
to the first child's concrete `ws://` URL. Every reload rebinds that URL, so the window rides the
blink via `ludion-window`'s endless reconnect. The child is always headless — launching it with
`--window` would make the window die and reopen every reload. The blink is shutdown + boot + restore, so
a slow rebuild never lengthens it: the old child serves until the build has succeeded and its snapshot
is on disk, which is why dev's build time and the window's reconnect never fight. A child that
cannot boot leaves the window open, flagged "— reconnecting…", until the next good save relaunches
it; the window closes when dev stops.
