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
GoldenTest::new("scenes/stage-1.yaml", "goldens/windows/wave-1.png")
    .with_builder(shmup::game)   // supersedes .with_registry — the builder carries the registry
    .with_frames(600);              // 600 game ticks, systems and all
```

Scripted input stays out of it: for "after the player did X", record a replay and render from it
(`--replay-frames`, in `ludion docs running`).

**Reading a golden failure.** `L2203` (SSIM below threshold, or any single tile below the 0.95 tile
floor — a local defect the global mean would hide) and `L2207` (a tile's colour error over the
ceiling: SSIM is luma-only, so a hue change passes it; the report's `worst_color_tile` names the
region) write `<stem>.diff.json` + `<stem>.actual.png` under `target/tmp/testkit/` (the scaffolded test's `out_dir`). The report lists the
*damaged tiles* — worst first, each
with pixel coordinates and a compass region (`top-left`, `center`, …) — so you know *where* the render
changed without opening an image viewer. Harness details: `ludion devdocs ludion-testkit`.

A failure via `GoldenTest::single` also prints a `provenance:` line naming the adapter it rendered on
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

## Gotchas

- **Goldens are pinned-GPU bit-exact locally, SSIM-gated elsewhere.** Goldens are blessed **per
  pinned OS** into `goldens/<os>/` (`windows`/`macos` — `ludion_testkit::platform_dir`; any other OS
  is `L2205`): a bless on one platform never touches the other's set. On the machine that blessed a
  golden the comparison is exactly 1.0; the SSIM threshold exists for cross-driver drift, not local
  noise. A local score just under 1.0 that still "passes" is a red flag, not a margin.

## Next

- `ludion docs running` — hosting a scene and observing it.
- `ludion docs dev-loop` — the live reload loop.
- `ludion docs systems` — the system contract, input, and the determinism rules.
- `ludion docs scene-format` — the strict-YAML scene document.
