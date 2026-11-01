---
description: Hosting a scene: the run flags, attaching a viewer or MCP, render.bounds, in-process recording
---

# Running and observing

**Scope:** Running your game as a live host and observing it — the flags, the peers that attach, the text-legible `render.bounds` answer, profiling, and recording a replay. The headless test loop is verify-loop; the raw socket is wire-protocol.

```
cargo run -- --scene scenes/main.yaml            # host it live
cargo run -- --scene scenes/main.yaml --paused   # bind, but don't tick (a deterministic attach point)
cargo run -- --scene scenes/main.yaml --record run.lrp   # record the session for replay
cargo run -- --scene scenes/main.yaml --restore snap.json   # boot from an engine.snapshot file instead of the scene; prints `restored frame=<N> report=<JSON>` (or `restore failed: …`, then loads the scene) before `listening on`
cargo run -- --scene scenes/main.yaml --addr 127.0.0.1:0 # OS-chosen port (printed on the first line)
cargo run -- --scene scenes/main.yaml --window           # also open a native window on this scene
cargo run -- --scene scenes/main.yaml --window --fullscreen  # ...borderless fullscreen (needs --window)
cargo run --release -- --profile 60                      # profile 60 frames headless, print the report, exit
cargo run --release -- --profile 60 --profile-check budgets/pinned.toml  # ...and gate on a TOML budget
cargo run --release -- --replay-frames run.lrp --frames-out clip/ --from 0 --to 1800  # render a replay to a PNG sequence
cargo run --release -- --profile-replay run.lrp --from 300 --to 900  # time a replay tick by tick: worst frame, p99, per-frame series
cargo run -- --scene scenes/main.yaml --check-scene       # validate it against *your* registry, exit
cargo run -- --scene scenes/main.yaml --render-out frame.png --frames 120  # render frame 120 of your game, exit
```

The first stdout line is machine-parseable: `listening on ws://127.0.0.1:9411`. The process hosts the
scene over the wire until Ctrl-C, until a `Shutdown` request from one of your systems (below), or,
with `--window`, until the window closes. A second Ctrl-C exits at once; a sim that does not stop
within 5 s is reported as `L4022` and the host exits non-zero. Peers attach to that address:

- **The viewer** (`editor/app`, Tauri + React) — a human's live viewport: streaming frames, an entity
  inspector, input injection. Read-only; everything it does is a protocol command.
- **`--window`** opens the game's own native OS window in-process instead: the render thread blits
  the same offscreen frame any viewer streams straight into that window's surface (no protocol
  round-trip), while the listener still binds so a peer can attach alongside it; keyboard/mouse/
  gamepad still feed back as `input.inject`. Closing the window (or Ctrl-C) stops the host. Title, size,
  extent, fullscreen and cursor: `ludion docs desktop-window`.
- **`--profile <N>`** does not host at all: it drives the scene headless for a warmup then `N`
  measured frames **with your game's own systems running**, prints one `ProfileReport` JSON (median
  per-system sim time, per-pass GPU time, and the sim frame total), and exits. This is the in-process
  twin of `ludion profile` — the CLI links no game code, so only this path can time *your* systems by
  name. Add **`--profile-check <BUDGET.toml>`** to gate on a budget (an optional `frame_total_ms`, a
  `[systems]` table, and a `[passes]` table of millisecond ceilings, plus `load_total_ms` for the
  scene-load wall time; `frame_total_max_ms` and a `[systems_max]` table gate the worst frame
  instead of the median): any over-budget line is an `L4005` and the process exits non-zero. Measure in `--release`; a debug build's numbers are not
  comparable to a release-calibrated budget.
- **`--replay-frames <FILE>`** does not host either: it drives a committed `.lrp` frame-by-frame
  through *your* engine builder and renders each stepped sim frame to a zero-padded PNG
  (`frame-00000.png`, …) under **`--frames-out <DIR>`**, so `ffmpeg` can assemble a gameplay clip
  (`ffmpeg -framerate 60 -i frame-%05d.png clip.mp4`). Offline and headless — no window, no audio, no
  server — and it never persists a profile. The scene comes from the replay header, so no `--scene` is needed. The frame
  window is **`--from`/`--to`** (inclusive, absolute sim-frame indices; frame 0 is the initial
  post-load world, and the sim always drives from 0 since replay state is cumulative); omit them to
  render the whole session. Resolution is **`--width`/`--height`** (default 1920×1080). A window past
  the replay's final frame is an `L3021`, and so is a replay with an `engine.restore` or a
  `scene.load` after its first tick (it resets the frame numbering); no GPU adapter is an `L2003`.
  This is the CLI seam behind the engine-level `ludion_engine::replay_render_frames`.
- **`--profile-replay <FILE>`** times a committed `.lrp` tick by tick through *your* engine builder and
  prints one `ProfileReport` JSON — the answer to "which system hitches, and at which frame?", which a
  median hides. Every system, sequential cost and the frame total carries a `tail` (`max_ms`,
  `max_frame` — the frame the worst sample came from — and nearest-rank `p99_ms`) beside its median, and
  `samples` is the per-frame series (`frames`, `system_ms`, `frame_total_ms` columns). **`--from`/`--to`**
  pick the measured window (inclusive sim-frame indices, as for `--replay-frames`); earlier ticks run
  unmeasured and count as `warmup_frames`. Sim only: the report has no GPU or cluster section. Headless,
  the scene comes from the replay header. Add `--profile-check` with `frame_total_max_ms` /
  `[systems_max]` to gate the worst frame; a window outside the recording is an `L3021`, as is a
  window that measures no frame (`--from 0 --to 0`: no tick produces frame 0) and a replay with an
  `engine.restore` or a `scene.load` after its first tick. Engine-level:
  `ludion_engine::profile_replay`. Measure in `--release`.
- **`--check-scene`** and **`--render-out <PNG>`** host nothing either: they are `ludion check-scene`
  and `ludion render --scene` run against **your** registry instead of the built-in one, so a scene
  naming your components answers instead of failing `L1013` (the CLI links no game code — that is the
  whole difference). `--check-scene` parses and validates `--scene`, printing each diagnostic
  rustc-style and then `<path>: OK (N entities, M components)` or `<path>: N errors found` (errors
  only: a warning prints and never fails), and exits 0 or 1 — no GPU, no window, nothing
  instantiated. `--render-out` writes one frame of the same scene to that PNG at
  **`--width`/`--height`** (default: the builder's render size), after **`--frames <N>`** ticks of
  **your game's own schedule** — `N` game ticks, systems and all, so the frame can be
  an early wave or a boss phase rather than the scene at rest. `--quality` applies, and no adapter
  means `L2003`. A one-line load summary (milliseconds per phase, then what was decoded) goes to
  stderr on every run. The CLI's `ludion render --frames` stays the built-in-only twin (its ticks run only the
  engine-installed systems — path followers and the scene's `physics:` block — never your game's).
  Both require `--scene` and conflict with hosting and every other mode here — the two cheapest
  gates an agent has: *is my scene valid*, and *what does it look like*.
- **ludion-mcp — your eyes.** The MCP adapter turns the wire protocol into agent tools. A screenshot
  *is* how you see the game. Register it and point it at your running engine:

  ```
  claude mcp add ludion -- ludion-mcp --url ws://127.0.0.1:9411
  ```

  Tools mirror protocol methods: `sim_step`, `sim_hash`, `render_screenshot` (returns the frame as an
  image), `render_bounds`, `input_inject`, plus `ludion_status`. Start `cargo run -- --scene …`
  first, then drive it. Details: `ludion devdocs ludion-mcp`.

**Scripted input over the wire** is `input.inject` — the same command the sim-assertion test uses,
carrying `{"kind": "key_down", "key": "Right"}`. Inject at a frame, step, screenshot, assert.
A `frame` the sim has already run is refused (`L3005`); omit it to apply at the next tick.
Analog values (sent as bit patterns in `value_bits`/`position_bits`/`delta_bits`) must be finite,
sticks in `[-1, 1]` and triggers in `[0, 1]`, or the inject is refused (`L3005`).

**Verify what you drew — without a screenshot.** `render.bounds` (MCP tool `render_bounds`) is the
text-legible answer to "did my sprite land where I meant". It returns, for every renderable entity
under the current camera, its world-space AABB and its `screen_rect` in pixels (`{x, y, w, h}` at the
render size), plus `on_screen` — and a `null` rect when the thing is behind the camera. Pure CPU, so
it answers even with no GPU. A voxel model or assembly is boxed by its grid, a mesh, sculpt or
character by its vertex extent at rest (not a posed or animated one); a document that does not load
gets a unit box instead, so the query still answers for a broken scene. A first-render geometry mistake becomes a diff you can read:

```
# entity 2's screen_rect is {x: -80, y: 96, w: 64, h: 64}, on_screen: false
# → it drew off the left edge; its Transform.x is too small. No image needed to see that.
```

**Recording in-process (the agent-friendly path).** `--record` suits a hosted session, but a test
shouldn't have to spawn and Ctrl-C a child process to produce an `.lrp`. The same API is public:

```rust
engine.set_recording(std::fs::File::create("run.lrp")?)?;   // arm, exactly like --record does
let live = ludion_engine::drive_hashes(&mut engine, commands)?; // play the session
engine.stop_recording()?;                                    // flush
let replayed = ludion_engine::replay_hashes_with(game(), &std::fs::read("run.lrp")?, root)?;
assert_eq!(live, replayed);                                  // the recording is the session

// Or read game state off the driven engine (called at every settled frame; Break stops early):
let mut end = ludion_engine::replay_with(
    game(), &std::fs::read("run.lrp")?, root,
    |_, _| Ok(std::ops::ControlFlow::Continue(())))?;
let score = end.world_mut().query::<(&Score,)>().next().map(|(s,)| s.0);
```

`drive_hashes` fails on any system panic (`L4007`). A recording marks the tick a system panicked on, so
its replay expects that panic and only that one (`L3050` if it no longer happens there).

Recordings embed the scene text, so an `.lrp` is self-contained and replays identically anywhere
(given the same game builder). Worked example: `games/breakout/tests/replay.rs`; from a failing test to its frames: `ludion docs failing-run`.

## Quitting from a system

A system is handed `&mut World`, so it asks the host to stop the same way it asks for a district
change — by setting a resource the engine honors at the frame boundary. `Shutdown` is that resource,
and `request()` is its whole surface: the title screen's back binding, a quit menu item, an
end-of-demo timer.

```rust,ignore
use ludion::prelude::*;

/// The title screen's back binding: `Escape` ends the process.
fn quit_on_back(world: &mut World, _ctx: &SystemCtx) {
    let back = world.resource::<InputState>().unwrap().was_pressed(Key::Escape);
    if back {
        world.resource_mut::<Shutdown>().unwrap().request();
    }
}
```

The request is honored exactly as the wire `engine.shutdown` is: the host leaves its loop after the
current frame and runs the same graceful teardown Ctrl-C triggers — recordings flushed, connections
closed, exit 0 — and a `--window` session closes its window. Calling it every frame requests one quit, not many. Driving an `Engine` yourself with no host
(a test, `--replay-frames`) sets `Engine::shutdown_requested()` and nothing else, so a test can
assert on the quit. A quit in the same frame as a `SceneSwitch` wins, and since a quit mutates no
world state it writes no replay record: the recording ends at that frame, and so does the replay.

## Gotchas

- **`ludion replay` on the CLI is built-in-only** too; replay a recording of *your* game through
  `ludion_engine::replay_hashes_with(game(), bytes, root)`, handing it the same builder
  (`replay_with` for the driven engine; never mutate sim state in its observer).

## Next

- `ludion docs desktop-window` — the game window and the `launch` player host.
- `ludion docs verify-loop` — the `cargo test` verification loop.
- `ludion docs dev-loop` — the live reload loop.
- `ludion docs wire-protocol` — the raw JSON protocol.
- `ludion docs post-processing` — the imaging chain.
- `ludion docs scene-transitions` — the other request a system leaves for the frame boundary.
