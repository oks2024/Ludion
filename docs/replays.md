---
description: Player run recordings: record, suspend, game identity, play back in the window, list, verify, size
---

# Player replays

**Scope:** Recording a player's run to a `.lrp` from inside the game, playing it back in the game's own window, listing and deleting recordings, and verifying one. The profile and the suspend file are `ludion docs player-profiles`; hosting flags and `--record` sessions are `ludion docs running`; the key and menu side is `ludion docs presentation`; the test loop is `ludion docs verify-loop`.

A run recording is an ordinary `.lrp` replay of one run, written by the game at runtime instead of by `--record`. It carries the scene text, the seed, the profile at the start and every input, so it replays to the live trace on any machine with the same build.

## The loop

```rust
Engine::builder()
    .project_root(root)
    .run_recordings(RunRecordingConfig::default())      // dir "save/replays", keyframe_every 3600
    .replay_playback(ReplayPlaybackConfig::default())   // the same dir
    .playback_controls(|input, controls| {              // keys -> pause / speed / stop, live input only
        if input.was_pressed(Key::Escape) { controls.stop(); }
    })
    .game_version("1")                                   // bump on balance changes
    .persist_profile()
```

A system starts and stops a run; a menu plays one:

```rust
world.resource_mut::<RunRecording>().unwrap().start("run-1");   // "new run"
world.resource_mut::<RunRecording>().unwrap().stop();           // run over
world.resource_mut::<ReplayPlayback>().unwrap().play("run-1", "menu.yaml");
```

Then check a saved file with `cargo run -- --replay-verify save/replays/run-1.lrp`. The runnable example, with a 500-enemy variant of the same game behind an ignored test, is `cargo run -p ludion --example run_replays`.

## Recording a run

- **Persisting hosts only.** A test engine or a replay never writes a file: the request is only logged (`Engine::run_recording_log`).
- **Requests apply at the next free-running frame boundary.** The last `start` in a frame wins; a `start` while one is recording finishes that one first.
- **`start` restarts the run** from the current scene's text at frame 0 and records it. Carried across: the live `Profile`, unsaved writes included (this is the profile-at-start in the header), and the run-save state. Nothing else survives, so a title screen that calls `start` for "new run" begins the run clean.
- **`stop` publishes** `<save root>/save/replays/<name>.lrp`, crash-durably: a file is complete or absent.
- **A name** is 1 to 128 of `[A-Za-z0-9._-]` and does not start with `.`.
- **Failures** are `L1402` (a name, a write or the finalize failed, or `start` during a `--record` session) and `L1403` (a zero `keyframe_every` or a `dir` outside the save root fails the build). `ludion explain L1402`.
- **Checkpoint slots** (`ludion docs checkpoints`) ride each keyframe, so a seek inside a held window works; the replay reproduces every capture and restore.
- **`keyframe_every`** trades file size for verify grain: a keyframe is a full snapshot, so a shorter interval is a larger file and a finer `--replay-verify` (see Size).

## Suspend and resume

A successful `RunSave::suspend` finalizes the open recording: the file replays up to the suspend, and later ticks never extend it, also when a scene switch lands in the same tick. A suspend during a lockstep `sim.step` ends the file at the end of the step command that carried it. `RunSave::resume()` while a run is recording is refused with `L1401`, the pending save left as it was and no backup made: `stop()`, then `resume()`. A quit with a run recording open publishes the partial run. `RunSave::pending()` is disk input: a replay, and an in-place playback, sees `None`, so read it **before** `start()`. The suspend file itself is `ludion docs player-profiles`.

## Game identity

Run recordings, and only they, carry `GameIdentity { schema, version }` in their header. `schema` is a description-stripped hash of the game's component and data-doc schemas, engine components included; `version` is the string from `EngineBuilder::game_version`, which a game bumps for changes the schema cannot see, such as a balance pass. A replay whose identity differs from the running game's is refused with `L1404` after the engine check (`L3010`) passes, and the message names both builds. A file with no identity passes. A menu shows `ReplayEntry::refusal` as "recorded by an older version" without playing the file; `Engine::game_identity()` against the file's `header().game` is the same comparison by hand.

## Playing a replay

`ReplayPlayback::play(name, return_to)` turns the live engine into the replay of `<name>.lrp`; `return_to` is a scene **file**, relative to the project root.

- **Persisting hosts only**, applied at a frame boundary. Everything is checked first (name, file, engine, game identity, the scene to return to), so a refusal leaves the world untouched: `last_outcome()` is `Refused` with `L1404` (another build of the game), `L3010` (a file this engine cannot read) or `L1405` (a bad name, a missing file, a recording or playback already running, an unreadable `return_to`).
- **Outcomes** are `Finished`, `Stopped`, `Refused` and `Failed`. At the end or on stop the `return_to` scene reloads with the profile as it was before the play.
- **While playing**, a live command that would change the sim, `asset.reload`, and a command that writes the replayed world to disk (`scene.save`, `prefab.create`, `prefab.apply`) are refused (`L1405`) and reads pass; the replayed world sees no pending suspend, no open or finished recording, no listing and `ReplayPlayback::playing()` of `None`, as under `--replay-verify`; a `render.size` is kept and applied on the return; a quit the replayed game requests is dropped.
- **Controls.** The `playback_controls` hook sees live input and a `PlaybackControls` (pause, resume, `PlaybackSpeed` X1/X2/X4, stop). It runs at every boundary, paused included, and has no world access: the game maps the keys, the engine binds none. Speed changes the pacing only; the replayed trace is the same at every speed.
- **Live input** goes to the `PlaybackInput` resource. Read it in a presentation system only: a sim system that reads it diverges the playback.
- While paused the same frame is presented again and `frame()` and the hash hold. There is no seek.

## Listing and deleting

`refresh()` re-reads the directory; `entries()` then lists each file as `ReplayEntry { name, bytes, refusal }`, `refusal` being why this build cannot play it. `delete(name)` removes one. Keeping the list short (a count or a byte cap) is the game's policy, applied with `delete`.

## Verifying a saved run

`--replay-verify <file>` on the game binary replays a file and compares the world at every keyframe, so a divergence is located to within `keyframe_every` ticks. In a test, `ludion_engine::replay_hashes_with(game(), &bytes, root)` returns the per-tick `(frame, WorldHash)` trace, which equals the trace of an in-window playback, and `ludion_engine::verify_replay_keyframes(game(), &bytes, root)` is the keyframe check. Pass a fresh builder configured as the recording game was.

## Size

A 30-minute run (108 000 ticks at 60 Hz) of the `run_replays` example scaled up (500 rigid-body enemies, a projectile pool firing six shots a second, a scripted input bot) at the default `keyframe_every: 3600` records **7.20 MiB** (7 550 119 bytes): 32 keyframes of about 177 KiB each (compressed) and about 1.8 MB of inputs and commands between them. The record pass took 23.7 s in release. Measured on an M5 laptop (not a pinned machine) at revision 2268; byte counts depend little on the machine. The figure comes from `cargo test --release -p ludion --test run_replays -- --ignored --nocapture thirty_minute_500_enemy_recording_size`.

To scale it: the file is about `1.8 MB + keyframes x 177 KiB`, so a `keyframe_every` of 1800 doubles the keyframe share from about 5.5 MiB to 11 MiB (12.7 MiB in all) and 7200 halves it. A keyframe's size follows the world's size (this one is 500 bodies plus the pool), and the input share follows how much the player does.

## Gotchas

- **A director keyed on a frame number must guard on `RunRecording::active()` and `last_finished()`**: `start` restarts the scene at frame 0, so an unguarded `frame == 30` starts again every time the frame comes round.
- **Never write hashed state from `active()`.** In a replay the resource is non-persisting, so `active()` differs from the live run.
- **A sim system reading `PlaybackInput` diverges the playback**; it is for presentation systems, which detect a playback by the resource's presence (`ReplayPlayback::playing()` is `None` in the replayed world, as in `--replay-verify`; `Engine::playing()` is the host's check).
- **Read `RunSave::pending()` before `start()`**: a replay and a playback see `None`.

## Next

- `ludion docs player-profiles` — the profile, and suspending a run.
- `ludion docs running` — hosting flags, `--record` sessions.
- `ludion docs presentation` — the menu and key side.
- `ludion docs verify-loop` — the `cargo test` verification loop.
