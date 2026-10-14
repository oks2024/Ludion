# breakout

A Ludion game — and Milestone 1's proof artifact: a fresh agent built it from the one-line prompt
"make a Breakout clone with dynamic lighting", from `ludion new` to this passing suite, with zero
human intervention (see `../../milestone-1-proof.md`). In-repo it lives as a workspace member; the
generated `LUDION-GUIDE.md` copy is dropped in favor of its source of truth, `docs/engine-guide.md`
— **read that guide before writing code**.

## Project layout

- `src/lib.rs` — the game: components (`Paddle`, `Ball`, `Brick`, `BallGlow`, `PauseMenu`,
  `BrickBurst`), systems, and `game() -> EngineBuilder`. Start here. The `Escape` pause menu is a
  `UiList` on the `PauseMenu` entity, and being paused is *derived* from it — the list holds items
  while open, none while closed (`is_paused`); every gameplay system returns early while it is.
  Breaking a brick also spawns a one-shot `ParticleEmitter` spark burst tagged `BrickBurst`, reaped by
  `reap_bursts` once its particles age out.
- `src/main.rs` — the ~3-line binary: `ludion::run(breakout::game())`.
- `scenes/main.yaml` — the scene (strict-subset YAML): built-in visuals + this game's components.
- `tests/gameplay.rs` — headless sim assertions + a WorldHash-determinism test (no GPU needed).
- `tests/menu.rs` — the pause menu under scripted keyboard, gamepad and mouse input: open/close, the
  total freeze (visible state *and* a still WorldHash), cursor wrap, and `RESTART`'s scene switch.
- `scenes/fonts.yaml`, `scenes/fonts/` — the font table (`body`) and breakout's own copy of Inter
  (TTF, licence, baked atlas) the pause menu draws with; `scenes/` is the runtime content root.
- `tests/golden.rs` — the golden-image tests against `goldens/<os>/main.png` and `menu.png` (the open
  pause menu) (per-OS goldens: `windows`/`macos`, blessed on each pinned platform).
- `tests/replay.rs` — the committed session replay (`run.lrp`): a fresh recording of the same
  session must replay to the live WorldHash trace, and the committed artifact must still match it.
- `tests/drift.rs` — asserts every component in the registry survives snapshot migration (see
  "Keep components drift-safe" in `docs/components.md`).
- `clippy.toml` — the sim-side determinism denylist the game code inherits.
- `ludion.toml` — the project manifest (name + default scene).

## The loop

```
cargo test -p breakout                  # sim assertions + determinism + golden + session replay
LUDION_BLESS=1 cargo test -p breakout   # (re)bless goldens/<os>/main.png AND re-record run.lrp; review, commit
cargo run -p breakout -- --scene games/breakout/scenes/main.yaml   # host it live; prints `listening on ws://…`
```

When only the hash trajectory moved (a game/scene change with no visual consequence), re-record the
session alone with `LUDION_BLESS=1 cargo test -p breakout --test replay` — the golden PNG's bless
belongs to the pinned reference GPU and must not be run anywhere else.

Verify by hash and image comparison, not by looking at the screen.
