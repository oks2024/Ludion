---
description: The game-facing facade crate: what it adds over ludion-engine, and the host-only disk seams
---

# ludion

**Scope:** The `ludion` facade crate — the single dependency a game takes (`use ludion::prelude::*`)
and the single rustdoc root it reads. This page is for *changing the facade*: what it re-exports and
why, the four hosting-short-circuit flags, and the disk seams that exist **only** inside
`ludion::run`. Building a game with it is `ludion docs getting-started` + `ludion docs project-anatomy`;
the command core, WS host and golden harness are `ludion devdocs ludion-engine` / `ludion-server` / `ludion-testkit`.

## Layout

- `crates/ludion/src/lib.rs` — crate docs ("how to build a game") and `run(EngineBuilder) -> ExitCode`, the game-hosting entry point mirroring `ludion serve` (`--scene`/`--addr`/`--record`/`--paused`), plus the four flags that short-circuit hosting entirely: `--profile`, `--check-docs`, `--replay-frames`, `--replay-verify`. Beside it, `launch(EngineBuilder, LaunchRoots) -> ExitCode`, the argv-free host (`content`/`save` explicit) — the embedding core with no surface. The default `wire` feature gates the listener (off ⇒ no listener, no `ludion-server`/clap, `run`/`host_scene` compiled out); the default `window` feature (implies `wire`) gates the windowed shell — winit and `ludion-window`'s builders (off ⇒ `--window` refused, `L0018`).
- `crates/ludion/src/embed.rs` / `ffi.rs` — the embedding core and its C ABI (`ludion devdocs ludion-embedding`). `ffi.rs` is the only module in the workspace with `unsafe` (crate root `deny`, module `allow`).
- `crates/ludion/src/present.rs` — the core's winit client (feature `window`): the event loop on the main thread makes the window and its surface, starts an `Embedded` with them, and forwards device events → `input.inject` through the core's intake.
- `crates/ludion/src/prelude.rs` — the glob re-export a game imports. The list is the file; what matters is the two rules below (no derive macros; types only).
- `crates/ludion/src/math.rs` — the `ludion::math` module: `camera_ray`/`ray_ground_plane` for cursor and stick aim, and the three UI-anchor helpers re-exported from `ludion_engine::ui` so a mouse hit test against a `UiRect` is the *same* arithmetic the overlay pass draws with. Trig via `ludion_core::math`, matrices via `glam` — cross-platform bit-stable, mirroring `extract.rs`/`camera.ts`; the UI helpers are pure `+ - * /`. Re-exported as a **module**, not glob, so its `Projection` does not clash with the `Camera` component's (`math::Projection`).
- `crates/ludion/examples/` — one runnable example per game-facing pattern, each with a behavioural `WorldHash`-trace test beside it under `crates/ludion/tests/`: `cursor_aimed_turret.rs`, `hud_bar.rs`, `data_docs.rs`, `player_profile.rs`, `scene_transition.rs`. Keep each test asserting *behaviour plus a two-run hash identity*, not just compilation.
- `crates/ludion-engine/src/builder.rs` — `EngineBuilder` itself. It lives in the **engine**, not here, so replay and testkit can reach it without the facade. `Engine::builder()` starts one.
- `games/hello-paddle` — the worked example: one component, one system, a scene mixing built-in visuals with it, and the four test kinds (sim assertion, `WorldHash` determinism, golden, wire smoke).

## Dataflow

**Construction.** `.components(closure)` queues a registration closure, `.system(name, fn)` appends in
order, `.project_root`/`.render_size`/`.default_scene` set hosting knobs. `.build()` applies the
registrations on top of `builtin_registry()` (returning any `L0005` collision) and hands registry +
system list to `Engine::from_parts`, which installs the schedule into the *initial* world — so an
engine driven before any `scene.load` still runs its systems. `.build_at(root)` is the same with an
overridden project root (the seam replay uses); `.load_scene_file(path)` roots at the scene file's own
directory, builds, and loads — the construction path `ludion serve`/`sim-hash` share, so a scene fails
identically wherever it loads.

**`ludion::run` is the only place the engine touches disk beyond loading.** Everything else — a
replay, a test, `--profile`, `--check-docs` — leaves those seams off, which is what stops any of them
clobbering a developer's real save. Three seams:

- **Profile persistence** — `EngineBuilder::persist_profile()`, set before the boot load because it
  also gates the boot backup-rename of an unloadable save (the `set_audio_sender` mold);
  `--record` inherits it, `run_profile`/`run_check_docs`/every replay do not. `run` also prints the
  boot `profile_migration()` to stderr when it is not clean.
- **The transition's district text** — a live switch reads its target from the project root (the start
  scene's directory) and, under `--record`, embeds that text in the `.lrp` for cross-machine replay.
- **The content root** for data-docs, rooted at the scene file's own directory, so a game's docs live
  beside its scene.

**No live host seam exists after `Server::bind`** (it moves the engine onto the sim thread), so a
save-time `L1122` and a switch failure (`L3018`/`L3019`/`L3020`) are queryable on
`Engine::profile_save_error()` / `last_transition_error()`, never printed live. A new failure mode
here means a new accessor, not a print.

**`--replay-verify <FILE>`** re-drives a committed `.lrp` through the game's own builder and checks
live state against every keyframe the recording embedded
(`ludion_engine::verify_replay_keyframes`). Clean exits 0; a divergence prints `L3024` — the frame,
the entities, the components that drifted — and exits non-zero. **Run it before blessing a
re-recorded `.lrp`:** after an intentional gameplay change a committed replay's keyframes describe
code that no longer exists, and re-recording is a rubber stamp unless something names what moved. It
takes its scene from the replay header, so it runs before `--scene` is resolved and needs no GPU,
window, or server; its project root is the scene's, as when hosting. Conflicts with
`--profile`/`--check-docs`/`--window`/`--record`/`--replay-frames`.

**Input capture is out-of-sim.** The sim only ever sees *recorded* input (an `InputScript` or
`input.inject`); device capture is `ludion-window`'s pure mapping layer — winit mouse through the
letterbox transform into render-target pixels, one `gilrs` pad, no `ludion-sim` dependency — which
`present.rs` and that crate's own viewer both reuse. Headless — a plain `ludion serve`, every CI
run — captures nothing, so `InputState` stays default. The game-side API is `ludion docs input`.

## Rules

- **A game is a bin depending on `ludion`.** It brings components (plain serde/schemars structs) and
  systems (`fn(&mut World, &SystemCtx)`); everything else comes from the facade. Games take
  `ludion-testkit` as a **dev-dependency** for tests, and `ludion-engine` as a dev-dep for
  `replay_hashes_with`/`drive_hashes`.
- **The facade may depend on both `ludion-engine` and `ludion-server`; the engine still never depends
  on the server.** `ludion` is the one crate that unites them behind a game's `main`; the server
  sits behind the default `wire` feature, winit/`ludion-window` behind the default `window` one.
  `default-features = false` is the shipped mobile shape (`Plan/ios-target.md`): no listener, no
  window, and no `notify` — the watcher is `ludion-assets`' opt-in `watch` feature, which only `wire`
  and `ludion-cli` ask for. `scripts/mobile-check.mjs` gates that shape, and vigil's lib
  for `aarch64-apple-ios` where that target is installed.
- **The embedding core is the one host lifecycle** — `Embedded::start`, the surface-before-`Server::bind` order, the safe rect, argv modes on the boot thread, touch layouts (`L0021`), the C ABI (`L0020`) and the iOS staticlib build: `ludion devdocs ludion-embedding`.
- **The serde/schemars derive macros are NOT re-exported.** A game's components derive `Serialize`,
  `Deserialize` and `JsonSchema`, so a game adds `serde` and `schemars` to its own `Cargo.toml` — they
  are the game's dependencies, not the facade's to inject. The prelude re-exports *types*, never
  derive macros.
- **Game components register on top of the built-ins.** `EngineBuilder::components(closure)` runs
  against a registry that already holds `Transform`, `Sprite`, …, so a scene mixes built-in visuals
  with game components freely. A name/type collision is a structured `L0005` from `build()`, never a
  silent overwrite.
- **A configured schedule survives `scene.load` and `engine.restore`.** The `Engine` stores the system
  list (plain `fn` pointers) and rebuilds a fresh `Schedule` from it at every world reset
  (`Engine::build_schedule()`); systems are engine *configuration*, not world state, so they outlive
  the world. (The old hard-coded `Schedule::new()` silently dropped a game's schedule on first load.)
- **`WorldHash`, recording, replay and snapshot need no extra wiring for custom components** — they
  are entirely registry-driven. A game component is serialized, hashed, keyframed and replayed by the
  same registry-order machinery as a built-in.
- **`run` mirrors `ludion serve`** (same `listening on ws://…` line, same Ctrl-C flush of an armed
  recording).

## Verify

```
cargo test -p ludion                              # facade doctest ("how to build a game") compiles
cargo test -p hello-paddle                        # sim assertion, WorldHash determinism, golden, smoke
cargo run -p hello-paddle -- --scene games/hello-paddle/scenes/paddle.yaml --paused   # prints `listening on ws://…`
cargo clippy -p ludion -p hello-paddle -- -D warnings    # both carry the strict sim-side denylist
cargo run -p breakout -- --replay-verify games/breakout/run.lrp   # the committed replay still describes this build
node scripts/mobile-check.mjs                     # wire/window off: clippy, tree; + aarch64-apple-ios if installed
```

In the private Lore tree, `cargo run -p vigil -- --replay-verify run.lrp` from `games/vigil/` is the same check on the larger corpus.

## Gotchas

- **`ludion::run` blocks until Ctrl-C.** A test cannot call it in-process; drive the *binary* as a
  child and read the `listening on` line off stdout, as `games/hello-paddle/tests/smoke.rs` does.
- **The examples and the argv-shell tests need `wire`** — `cargo test -p ludion --no-default-features`
  cannot build them. The mobile gate lints the lib alone (`--no-deps`), the surface a device links.
- **Building consumes the `EngineBuilder`** (the registration closures are `FnOnce`). Expose the game
  config as a `fn() -> EngineBuilder` so `main`, tests and replay each get a fresh one.
- **`default_scene` is a hosting hint** only `run` (cwd-relative; prefer `--scene` in tests and CI) and
  the embedding core (content-root-relative) read; the engine core ignores it.
- **A game recording will not replay through `ludion replay`** (built-in registry only, `L1013`) —
  use `ludion_engine::replay_hashes_with(builder, bytes, root)` with the game's own builder.
- **A game's golden test needs `with_registry`** or `check` rejects its components with `L1013`; the
  render is a static tick-0 frame, so no game schedule runs. Harness: `ludion devdocs ludion-testkit`.
