# {{name}}

A Ludion game. **Read `LUDION-GUIDE.md` before writing code** — it is the index of the engine guide
(a copy of `ludion docs engine-guide`; `ludion new --refresh-guide` updates it).

The guide names deeper **topics** in passing — run `ludion docs` (in an engine-checkout scaffold,
`node scripts/ludion.mjs docs` when `ludion` is not on your PATH) to list the ones available here, and `ludion docs <topic>` to fetch one
(e.g. `ludion docs scene-format`). Pull a topic only when you're working in that area. A name the
listing doesn't show is engine-internals documentation that ships with the engine's own source rather
than with this project — `L0009` says so.

## Project layout

- `src/lib.rs` — the **wiring root**: `register` (the component list) and `game() -> EngineBuilder`
  (the ordered `.system()` list). Start here. Definitions live in per-concern modules glob re-exported
  here; add modules as the game grows (the engine repo's `games/shmup-stress/src/` shows the split).
- `src/components.rs` — the game's components (and, at this scale, their tunables).
- `src/systems.rs` — the game's systems (`fn(&mut World, &SystemCtx)`).
- `src/main.rs` — the ~3-line binary: `ludion::run({{name_ident}}::game())`.
- `scenes/main.yaml` — the scene (strict-subset YAML): built-in visuals + this game's components.
- `scenes/model-stage.yaml`, `scenes/model-stage-night.yaml`, `scenes/effects-stage.yaml` — the
  Ludion editor's stage scenes (Voxel Artist / Models, Effects views), never this game's own scenes;
  safe to delete, the editor re-adds them with **Add stage scenes**.
- `tests/gameplay.rs` — headless sim assertions + a WorldHash-determinism test (no GPU needed).
- `tests/golden.rs` — a golden-image test (bless-or-skip until you commit `goldens/<os>/main.png`
  — goldens are per pinned OS: `windows`/`macos`).
- `tests/drift.rs` — asserts every component in the registry survives snapshot migration (see
  "Keep components drift-safe" — `ludion docs components --section "Keep components drift-safe"`).
- `clippy.toml` — the sim-side determinism denylist your game code inherits.
- `ludion.toml` — the project manifest (name + default scene).

## The loop

```
cargo test                              # sim assertions + WorldHash determinism + golden
LUDION_BLESS=1 LUDION_BLESS_ANY_ADAPTER=1 cargo test   # (re)bless goldens/<os>/main.png on your pinned GPU; review, commit
cargo run -- --scene scenes/main.yaml   # host it live; prints `listening on ws://…`
```

Verify by hash and image comparison, not by looking at the screen. `LUDION-GUIDE.md` has the details.
