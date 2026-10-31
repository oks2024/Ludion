---
description: The cargo test loop: sim assertions, WorldHash determinism, and blessing golden images
---

# The verification loop — the heart of it

**Scope:** The everyday check: drive the engine headless, assert on state, prove the hash sequence repeats, and gate the picture on a golden. Hosting a live session instead is running; the live reload loop is dev-loop.

```
cargo test          # sim assertions + WorldHash determinism + the golden
```

**Sim assertions.** Drive the real engine and assert on component state. This is the everyday check —
no GPU needed. (Full example: `tests/gameplay.rs`.) A test engine roots at `.` unless you
`project_root(...)` it — root it where the host does, or a `*.yaml` walk (data-docs) descends
whatever sits under the default root, `target/` included.

```rust
let mut engine = <game>::game().build().unwrap();
engine.load_inline_scene(SCENE).unwrap();
engine.execute(Command::InputInject(command::InputInjectParams {
    frame: Some(FrameIndex::new(0)),
    event: serde_json::json!({"kind": "key_down", "key": "Right"}),
})).unwrap();
engine.execute(Command::SimStep(command::SimStepParams { ticks: 30, hash: false })).unwrap();
// ... query the world and assert.
```

**WorldHash determinism.** A `WorldHash` is a 16-hex-digit fingerprint of all live component state.
Two runs of the same commands must produce identical per-frame hash sequences — that is what proves
your sim is deterministic. `ludion_engine::drive_hashes(&mut engine, commands)` returns the sequence;
assert two runs are equal (see `tests/gameplay.rs`). Cross-process, the same idea is
`ludion sim-hash --scene … --frames N` (run it twice: identical output). Concept: `ludion devdocs ludion-sim`.

**Golden images.** A golden test renders the scene and compares it to a committed PNG via tiled SSIM.
The generated `tests/golden.rs` is green out of the box: until the first golden is committed it prints
how to bless one and passes; after that a missing golden is `L2201`, and every present one compares
(skipping on `L2003` when there is no GPU, on `L2205` on an OS outside the pinned golden tier, or on `L2210` for a byte-exact comparison off the pinned GPU).

Bless a golden on your pinned GPU, then commit it:

```
LUDION_BLESS=1 LUDION_BLESS_ANY_ADAPTER=1 cargo test   # (re)writes goldens/<os>/main.png; review before committing
cargo test                      # compares; a regression is L2203 with a localized diff report
```

(PowerShell: set both with `$env:`, then `Remove-Item` them.) `LUDION_BLESS_ANY_ADAPTER=1` declares this
GPU is your pinned one: without it a bless into `goldens/<os>/` on a GPU that is not Ludion's own
reference card is refused (`L2206`). Blessing goes
through your own golden test because only your game knows its component registry — the CLI's
`ludion test --bless` directory suite validates against the built-ins only, so it cannot render a
scene that names your components.

**A golden of your game in motion.** By default a golden is the scene at rest, and `.with_frames(N)`
ticks only the engine's empty schedule — so frame N is frame 0 with the flipbooks advanced.
`.with_builder(<game>::game)` renders through *your* engine instead, running *your* systems for those
N ticks, which is how an early wave or a boss phase gets a committed golden:

```rust
GoldenTest::in_set("scenes/stage-1.yaml", "goldens", "wave-1")
    .with_builder(shmup::game)   // supersedes .with_registry — the builder carries the registry
    .with_frames(600)            // 600 game ticks, systems and all
    .check(out_dir);
```

"After the player did X" has two forms, each the same skip/bless/compare recipe: an engine your test
built and drove, or a frame of a recorded replay:

```rust
check_engine_golden(&mut engine, "goldens", "menu-open", out_dir);   // renders it as it stands
GoldenTest::from_replay(bytes, 240, env!("CARGO_MANIFEST_DIR"), "goldens", "shop")
    .with_builder(shmup::game)
    .check(out_dir);                                                  // frame 0 = post-load world
```

**Reading a golden failure.** `L2203` (SSIM below threshold, or any single tile below the 0.95 tile
floor — a local defect the global mean would hide) and `L2207` (a tile's colour error over the
ceiling: SSIM is luma-only, so a hue change passes it; the report's `worst_color_tile` names the
region) write `<stem>.diff.json` + `<stem>.actual.png` under `target/tmp/testkit/` (the scaffolded test's `out_dir`). The report lists the
*damaged tiles* — worst first, each
with pixel coordinates and a compass region (`top-left`, `center`, …) — so you know *where* the render
changed without opening an image viewer. Harness details: `ludion devdocs ludion-testkit`.

A failure of any form also prints a `provenance:` line naming the adapter it rendered on
against the pinned reference for your platform (`ludion_testkit::provenance_label`): off the pinned
adapter, a red is environment until it is re-run on the reference box; on it, a red is a regression.

**Verify a committed replay before you re-record it.** `cargo run -p <game> -- --replay-verify <FILE>`
re-drives a committed `.lrp` through your own builder and checks live state against every keyframe the
recording embedded, including the one written when the recording stopped. Clean exits 0; a divergence prints `L3024` — the frame, the entities, the
components that drifted — and exits non-zero. **Run it before blessing a re-recorded `.lrp`:** after an
intentional gameplay change a committed replay's keyframes describe code that no longer exists, and
re-recording is a rubber stamp unless something names what moved. It takes its scene from the replay
header, so it needs no GPU, window, or server, and it roots content where *hosting* roots it —
`--scene`'s directory when you pass one, else your builder's default scene's directory, never the
replay file's — so a `.lrp` committed away from your scenes still loads the data-docs it recorded
with. Conflicts with `--profile`/`--check-docs`/`--window`/`--record`/`--replay-frames`/`--profile-replay`.
Its timing twin is `--profile-replay <FILE>`: the same replay, timed per tick, reporting each system's
worst frame and p99 (`ludion docs running`).

**Explain any code.** `ludion explain L2203` prints the full what/why/fix for any `L####` diagnostic.

## Numeric trace

A per-tick JSON Lines record of a scene driven by an input script, for asserting on motion without a
picture. Three entry points print the same lines: `ludion trace --scene <FILE> [--script <FILE>]
--frames <N> [--channels motor,sensors]` (built-ins only, no game systems), `cargo run -p <game> --
--trace <N> [--scene …] [--script …] [--channels …]` (the game's systems run, so its motors move), and
`ludion_engine::trace_with(builder, scene, &script, frames, &channels)` in a `cargo test`, with
`write_jsonl` to print. The script is `ludion sim-hash --script`'s format (`ludion docs systems`).

```
{"channel":"motor","tick":41,"entity":"hero","feet":[2.05,0.0,0.0],"moved":[0.05,0.0,0.0],"grounded":true,"landed":false,"ground":{"entity":"floor","point":[2.05,0.0,0.0],"normal":[0.0,1.0,0.0],"walkable":true,"distance":0.0},"ceiling":null,"walls":[],"stepped":null,"platform":{"entity":null,"translation":[0.0,0.0,0.0],"yaw":0.0,"surface":[0.0,0.0,0.0]},"stuck":false,"height":0.9}
{"channel":"sensors","tick":15,"sensor":"zone","other":"hero","phase":"entered"}
```

- `motor` (fields: `ludion docs motor`): one line per `CharacterMotor` entity per tick, ascending by entity. The report fields
  appear only when game code called `move_character` on it during that tick; otherwise the line is
  `channel`, `tick`, `entity`, `feet`. A call held to a lane (`ludion docs lane`) adds a last `lane` object: `s`, `ds`, `weight`, `lateral_error`, `pull_blocked`. `sensors`: one line per enter or exit (`phase` `exited` adds
  `cause`: `separated`, `despawned`, `removed`). Within a tick, motor lines precede sensor lines.
- `tick` is the frame the tick ran as; an input addressed to frame `N` applies at the start of tick
  `N`. A sensor line is tick `N`'s own physics step (`ludion docs physics-sensors` for the one-tick
  latency a game system sees). Entities print as their `name`, else `e<index>v<generation>`.
- Floats are shortest round-trip, so two runs of one build are byte-identical. Compare two runs;
  never commit a trace as a golden.

## Gotchas

- **Goldens are pinned-GPU bit-exact locally, SSIM-gated elsewhere.** Goldens are blessed **per
  pinned OS** into `goldens/<os>/` (`windows`/`macos` — `ludion_testkit::platform_dir`; any other OS
  is `L2205`): a bless on one platform never touches the other's set. On the machine that blessed a
  golden the comparison is exactly 1.0; the SSIM threshold exists for cross-driver drift, not local
  noise. A local score just under 1.0 that still "passes" is a red flag, not a margin.

## Next

- `ludion docs running` — hosting a scene and observing it.
- `ludion docs dev-loop` — the live reload loop.
- `ludion docs systems` — the system contract and the determinism rules.
- `ludion docs scene-format` — the strict-YAML scene document.
