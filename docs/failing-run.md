---
description: From a headless test that failed at frame N to a clip of the last frames and a paused live host
---

# The failing run: from a red test to its frames

**Scope:** A headless test failed at frame N and you want to see it: a recording that survives the panic, a clip of the last frames with debug lines, and a paused live host at frame N. The test loop itself is `ludion docs verify-loop`; hosting a scene is `ludion docs running`; the lines a clip draws are `ludion docs debug-draw`.

## The loop

1. **Record every run.** Arm `set_recording` right after loading, to a fixed scratch path that each run overwrites, and put `engine.frame()` in the failure message. A panicking test still leaves the whole file: dropping the engine during the unwind flushes the batched ticks.

```rust
engine.set_recording(fs::File::create(&lrp)?)?;
// ... drive the test ...
panic!("the ball left the arena at frame {}", engine.frame().as_u64());
```

2. **A clip of the last frames.** In a test, `replay_with` calls its observer at every settled frame; draw and render the window `N-120..=N`:

```rust
replay_with(game(), &bytes, scenes_dir, |engine, frame| {
    if frame.as_u64() >= N - 120 {
        engine.set_debug_draw(DebugCategories::parse("colliders"));
        let png = engine.render_png(320, 180)?;
        // write frame-{frame}.png
    }
    Ok(if frame.as_u64() >= N { ControlFlow::Break(()) } else { ControlFlow::Continue(()) })
})
```

From a shell the same window is a flag set on the game's binary:

```
cargo run --release -- --replay-frames run.lrp --frames-out clip/ --from <N-120> --to <N> --debug-draw motor,lane,colliders
```

Its summary line echoes the categories as `--render-out` does.

3. **A paused live look.** Break at `N`, write the `engine.snapshot` document as JSON, then boot a host from it:

```rust
let snapshot = GameplayDriver::from_engine(engine).snapshot();
fs::write("snap.json", serde_json::to_vec(&snapshot)?)?;
```

```
cargo run -- --scene scenes/main.yaml --restore snap.json --paused
```

The host prints `restored frame=<N> …` before `listening on`. Then use `render.debug`, `sim.step`, the viewer or MCP screenshots.

## Rules

- The test reaches the world only through commands and input. A `world_mut()` edit is not recorded, so the replay diverges silently from that tick on; `--replay-verify` names it as `L3024` (`ludion docs verify-loop`).
- The observer passed to `replay_with` must not mutate sim state. `set_debug_draw` is host state and changes no hash.
- Frame 0 is the world right after the scene loads.
- A window past the replay's last frame is `L3021`.

## Gotchas

- Rendering needs a GPU (`L2003`); skip on it as the golden tests do.
- A `panic = "abort"` profile skips the unwind and loses the recording's tail.

Worked example: `games/breakout/tests/failing_run.rs`.

## Next

- `ludion docs verify-loop` — the test loop and replay verification.
- `ludion docs running` — hosting, `--restore`, `--paused`.
- `ludion docs debug-draw` — the categories a clip can draw.
