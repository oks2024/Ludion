---
description: ludion dev: what a reload does, its status lines, and what survives it (and what does not)
---

# Iterating with `ludion dev`

**Scope:** The live edit loop — rebuild, snapshot, relaunch, restore — and which content reloads without any of it. The drift-safety rule it depends on is components; the headless loop is verify-loop.

`cargo test` is the tight inner loop; `ludion dev` is the *live* loop — the one a designer watches
over your shoulder, and a fast way for you to verify a change against a running world without
re-scripting the session each time. Run it from the game's directory:

```
ludion dev                                  # watch src/, host scenes/main.yaml (the default scene)
ludion dev --scene scenes/level2.yaml       # a specific scene
ludion dev --window                          # also open one persistent native window
```

**What it does, and why it's parity-safe.** `dev` is an ordinary protocol client plus a process
manager. On every save under `src/` it: rebuilds your game → `engine.snapshot` over the wire →
writes the snapshot to `.ludion/dev-snapshot.json` → `engine.shutdown` the child → relaunches it
`--restore .ludion/dev-snapshot.json` on the same port: the new game runs `engine.restore` with that
file before it binds, never loading the scene; once the restore succeeds the file is deleted, so it
exists only while a snapshot is waiting to be restored. The snapshot and the shutdown are wire
commands you could send yourself; the restore is the host's `--restore` boot flag
(`ludion docs running`). `dev` invents no endpoint.
The upshot for you: **your code swaps onto the live world.** The restored snapshot carries the world's
component state (positions, scores, RNG, the current frame) while the *new* binary's code drives it —
so you can change a system, save, and watch the fix apply mid-session without rebuilding the state you
were testing against. Under `dev` the game's stdin is the supervision pipe (EOF ends the child), so game
code must not read stdin.

**Status lines** (stdout, one per step — parseable, the same discipline as `listening on ws://…`):

```
dev watching <src-dir>
listening on ws://127.0.0.1:9411          # relayed verbatim from the child; attach a viewer/MCP here
dev reload building
dev reload restored frame=9724 in 3175 ms  # a reload completed; the world is back at frame 9724
dev resumed frame=9724 from the kept snapshot <path> in 812 ms   # a fresh start resumed a kept snapshot
dev reload build failed                     # cargo errors printed above this; see below
dev reload launch failed; snapshot kept at <path>, retrying on the next save   # see below
dev reload snapshot failed (L3009: …); restarting fresh, session state lost    # see below
dev reload child exited; state lost, relaunching                               # see below
```

**A failed build never kills the running child.** `dev` only snapshots and restarts *after* a
successful build, so when your edit doesn't compile you keep playing the last good binary while
cargo's errors stream through — fix them and the next save reloads. This is why the loop is safe to
leave running while you iterate.

**A build that boots badly keeps `dev` alive.** The snapshot is written to
`.ludion/dev-snapshot.json` *before* the old child is stopped. If the rebuilt game then cannot boot
(a scene it no longer loads, a panic, a hang), `dev` prints the `L0013` with the game's output, then
`dev reload launch failed; snapshot kept at <path>, retrying on the next save`, writes `dev-status.json`
state `failed` (with the `L0013` and `snapshot`), and keeps watching. Fix the cause and save: the next
reload launches and restores the kept snapshot. The boot scene is not loaded when the snapshot
restores, so a component it uses that you renamed or removed does not fail the boot: the restore drops
it and reports it (`L3015`, below). With
`--window` the window stays open, flagged "— reconnecting…", until the next good save relaunches
the child; it closes when `dev` stops. The snapshot stays on disk.

**A child that cannot hand over its world is replaced, never kept.** Every call `dev` makes has a
timeout. A child that has exited, or that does not answer `engine.snapshot` within 10 s (a system
that never returns) or refuses it, gets stopped, and `dev` starts the new build on a fresh world:
it prints one of the two lines above, writes `dev-status.json` state `crashed` with a `message`
naming the cause, then `ok` with a `message` (and no `restored_frame`) once the fresh child serves,
so a status read later still says the world was not restored. Keeping the old child would mean your
edit never runs. A `ludion` CLI built from another engine revision than the game is `L3003`: `dev`
warns right after the first launch and every reload restarts a fresh world until the CLI matches.

**A failed restore keeps the world running, not frozen.** If the snapshot cannot restore (an
unreadable file, a format this engine refuses), the game falls back to loading the boot scene, so it
still comes up; `dev` prints `dev reload restore failed; running a fresh world, snapshot kept
at <path> …` and keeps the snapshot at `.ludion/dev-snapshot.json`. The fallback scene must load under
the new code, or the launch fails like any other boot. The next
save retries that snapshot and **discards the fresh world** to do it; delete the file to drop the
snapshot instead (the next reload then snapshots whatever is running).

**A kept snapshot resumes on the next `ludion dev` start.** If `dev` ends while
`.ludion/dev-snapshot.json` exists (a boot that failed, a restore that failed, the window closing),
the next `ludion dev` restores it before serving: it prints `dev resumed frame=<N> from the kept
snapshot <path> in <ms> ms` and writes `dev-status.json` state `ok` with `restored_frame` and a
`message` saying so. A successful restore deletes the file, so only a snapshot still waiting resumes —
**delete `.ludion/dev-snapshot.json` before starting `dev` to start fresh.** A resume that cannot
restore behaves like a failed reload restore (`dev resume failed; running a fresh world, snapshot kept
at <path> …`, state `restore_failed`, the next save retries it); a resume whose child cannot boot ends
`dev` with the `L0013`, and the rerun resumes again.

**The migration report rides the restore.** Because a restore deserializes the previous build's
document against this build's component shapes, `dev` prints one report line per group of changed
components (a deserialize failure shows the first entity's error as the example), with how many
entities it hit and the first one: a field you added filled from its
`#[serde(default)]`, a component you renamed/removed dropped with an `L3015`/`L3016` code; a nested
field is reported as a dotted path (`inner.b`). The full per-entity list is `report` in
`.ludion/dev-status.json`, on the reload's `ok`. A snapshot from another engine version or with fields this build does not know adds an
`L3048`/`L3049` line, and the restore still goes ahead. An empty report means a clean restore. The
restore reapplies the world only: data-docs come from the files on disk, not from the session's
live-reload log, and keys held at snapshot time are released. This is
the drift-safety rule (`ludion docs components --section "Keep components drift-safe"`) made visible —
a loud report beats silent data loss. If a drop surprises you, `ludion explain L3016`.

**The world is your state; the scene file is not auto-reloaded.** `dev` watches `src/` (code) — it does
**not** replace the live world when you edit a scene file, because the running world *is* the
designer's state and clobbering it on every `scenes/*.yaml` save would throw away the session. A scene
edit prints a hint (`dev scenes changed …`) but leaves the world running; to pick up scene changes
deliberately, send `scene.load` (or restart `dev`).

**Assets and data-docs are different — they *do* reload live.** The scene file is the only inert thing
in that directory. A mesh, voxel, texture, or `doc_kind::<T>` data-doc under the content root reloads
into the running game at a frame boundary, with no rebuild and no restart, because the game host
watches its own content root and applies each change as an `asset.reload`. That is the tuning loop:
edit a number in a `.yaml`, watch it land in the running game. It works under `ludion dev` and under a
plain `cargo run` alike — `dev` is not what makes it happen.

What survives a reload is the **world state** — component values, the RNG, the frame counter, the
run-state — carried across by `engine.snapshot`/`engine.restore`. Game resources (session latches,
caches — everything but `GameClock`) and held input do not: they restart from their defaults (`ludion
docs systems`, `ludion docs scene-transitions`). Recording is *not* part of the dev
loop: `ludion dev` takes no `--record`, and each reload starts a fresh child, so an `.lrp` does not
accumulate across restarts. To record, host a stable session with `ludion serve --record run.lrp`, or
record in-process from a test with `engine.set_recording` (`ludion docs running`).

Keep one `dev` host running and drive it as a peer (viewer, `ludion-mcp`, or raw wire) — a screenshot,
a `render.bounds`, a `sim.hash` — exactly as you would a `ludion serve`. Full designer-side walkthrough
(for the human describing changes to you): `ludion docs designer-flow`.

## Next

- `ludion docs components` — declaring components, and the built-in visual set.
- `ludion docs verify-loop` — the `cargo test` verification loop.
- `ludion docs running` — hosting a scene and observing it.
- `ludion docs data-docs` — game data as validated text.
