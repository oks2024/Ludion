# sokoban

A Ludion game — a fresh-agent THE-test artifact (grid-pushing puzzle: walk, push crates onto
targets, `R` resets). In-repo it lives as a workspace member; the generated `LUDION-GUIDE.md` copy
is dropped in favor of its source of truth, `docs/engine-guide.md` — **read that guide before
writing code**.

## Project layout

- `src/lib.rs` — the game: components (`GridPos`, `Player`, `Pushable`, `GameState`), systems, and
  `game() -> EngineBuilder`. Start here.
- `src/main.rs` — the ~3-line binary: `ludion::run(sokoban::game())`.
- `scenes/main.yaml` — the scene (strict-subset YAML): built-in visuals + this game's components.
- `tests/gameplay.rs` — headless sim assertions + a WorldHash-determinism test (no GPU needed).
- `tests/golden.rs` — the golden-image test against `goldens/<os>/main.png` (per-OS goldens:
  `windows`/`macos`, blessed on each pinned platform).
- `tests/replay.rs` — an in-process record/replay round trip: a fresh recording (via
  `Engine::set_recording`) replays, through a fresh engine, to the identical live WorldHash trace.
  No committed `.lrp` — unlike `games/breakout`, this game's THE-test run did not commit a session
  replay artifact.
- `tests/drift.rs` — asserts every component in the registry survives snapshot migration (see
  "Keep components drift-safe" in `docs/components.md`).
- `clippy.toml` — the sim-side determinism denylist the game code inherits.
- `ludion.toml` — the project manifest (name + default scene).

## The loop

```
cargo test -p sokoban                  # sim assertions + determinism + golden + record/replay round trip
LUDION_BLESS=1 cargo test -p sokoban   # (re)bless goldens/<os>/main.png on your pinned GPU; review, commit
cargo run -p sokoban -- --scene games/sokoban/scenes/main.yaml   # host it live; prints `listening on ws://…`
```

Verify by hash and image comparison, not by looking at the screen.
