---
description: Testkit crate hub: layout, verify, gotchas; routes to golden and lane spokes
---

# ludion-testkit

**Scope:** The verification harness for the perceptual tier of tiered determinism: `GoldenTest`
(a scene, or a `.voxel.yaml`/`.mesh.yaml` model document, vs a committed golden PNG), the windowed SSIM
comparison, the `DiffReport` JSON artifact, the bless flow, directory suite discovery, and the
`sim_assert` gameplay-test helpers. The determinism rules themselves (and why images are the
perceptual tier at all) are `determinism.md`; the render pipeline being verified is
`ludion-renderer.md`; the `ludion test` CLI surface driving this crate is `ludion-cli.md`; the
`L22xx` code registrations are `diagnostics.md`; testkit's position and dependency edges are
`crate-graph.md`.

## Layout

- `crates/ludion-testkit/src/lib.rs` — crate docs + re-exports: `GoldenTest`, `GoldenOutcome`, `compare_png_bytes`, `DiffReport`, `DecodedImage`, the `ssim` types, the `suite` functions, the `sim_assert` functions, `GameplayDriver`/`loaded_engine`.
- `crates/ludion-testkit/src/golden.rs` — `GoldenTest` (`{scene, golden, width, height, ssim_threshold, frames}` + private `registry`/`builder` seams), `new`/`in_set`/`from_replay`/`with_registry`/`with_builder` (any `Fn() -> EngineBuilder`)/`with_frames`/`run`/`bless`/`check` (the whole game-project golden-test recipe — skip/bless/compare), `single` (`in_set(..).with_registry(..).check(..)`), `check_engine_golden` (the same recipe for an engine the test drove), `missing_golden` (the one missing-golden decision: `L2201` panic once any per-OS subdir of the goldens root holds a `.png`, a loud skip on a fresh project; every game recipe calls it), `DEFAULT_TILE_FLOOR`, `bless_requested` (the `LUDION_BLESS=1` game-side bless convention), `platform_dir` (the per-OS goldens subdirectory: `windows`/`macos`, `L2205` elsewhere), `unverifiable` (the skip predicate: `L2205`, `L2210` — a byte-exact compare off the reference adapter — or `L2003`), `compare_png_bytes`, defaults (960×540, SSIM threshold 0.99, `frames` 0 — extract at rest). Its private `render()` dispatches on the source's `kind:` (`ludion_scene::peek_kind`): a `kind: voxel`/`kind: mesh` model document renders through `ludion_engine::render_model_sheet` (all six views, `width`×`height`), anything else through `render_scene_with_registry` as a scene, ticked `frames` times before extracting — how a mid-clip `Sprite` flipbook or `ParticleEmitter` burst lands on a golden — or, when `with_builder` set one, through `ludion_engine::render_scene_with_builder`, whose `frames` ticks run the *game's* schedule (that arm gets PNG bytes back and decodes them with the crate's own decoder).
- `crates/ludion-testkit/src/ssim.rs` — `compare` (windowed SSIM + per-tile colour error + MAE), `Comparison`, `TileDiff`/`TileRect`/`TileGrid`, `TILE_GRID = 8`, compass-style `region_name`.
- `crates/ludion-testkit/src/report.rs` — `DiffReport`: the JSON artifact, plus `diagnostic()` (L2202/L2203/L2207) and `to_json()`.
- `crates/ludion-testkit/src/suite.rs` — `discover`/`run_suite`/`SuiteDefaults`: the "one folder of scenes = one golden suite" convention (goldens resolved per-OS via `platform_dir`) and the optional `goldens/manifest.json` overrides.
- `crates/ludion-testkit/src/sim_assert.rs` — `run_until`, `assert_reaches`, `hash_sequence`, `assert_hash_sequences_match` over a `SimDriver`.
- `crates/ludion-testkit/src/driver.rs` — `loaded_engine` (build a game's `EngineBuilder`, load a scene inline — the opening step of every headless gameplay/replay/audio test) and `GameplayDriver` (wraps it with a frame cursor and `step`/`inject`/`inject_value`/`hold`/`release`/`tap`, for tests that script key presses instead of hand-building `Command`s).
- `crates/ludion-testkit/src/audio.rs` — `audio_stream`: an engine's audio log as `(frame, sound)` pairs, the text-first assertion surface for audio (no goldens); `audio_messages` is its unflattened twin, for telling a loop's sync from its stop; `SoundTap` captures every message and asserts each sound/track decodes.
- `crates/ludion-testkit/src/stats.rs` — `image_stats`/`ImageStats`: four normalized, no-reference defect metrics (`banding_score`, `edge_aliasing`, `clipped_low`, `clipped_high`) over a single decoded image. Where `ssim` asks "did this frame change?" against a committed golden, `stats` asks "does this frame carry a defect?" with no reference image at all — the instrument `ludion image-stats` (`ludion-cli.md`) drives.
- `crates/ludion-testkit/src/imports.rs` — `missing_derived_imports`/`require_derived_imports`: a project's `ludion.toml` `[[import]] to` trees (derived, git-ignored) must be synced before a suite runs; the panic names `ludion sync`.
- `crates/ludion-testkit/src/image.rs` — private: `DecodedImage` + the crate's own small PNG decoder (see Gotchas).
- `crates/ludion-testkit/tests/gallery.rs` — end-to-end suite over `examples/gallery/` + the Phase 2 exit criterion (broken scene → localizing diff report).
- `crates/ludion-testkit/clippy.toml` — deliberately permissive (host-side crate); the header explains why.


## Spokes

- `ludion devdocs testkit-golden` — the perceptual gate, per-OS goldens, render seams, SSIM method, `DiffReport`, run vs bless, suites.
- `ludion devdocs testkit-lanes` — GPU-less skips and `needs_gpu_*`, the spelling gate, the nextest lane, the saturated-box crash.

## Rules

- **A game whose `ludion.toml` declares `[[import]]` calls `require_derived_imports` in its test roots**: an unsynced tree silently drops meshes and colliders, so two runs agree on the wrong world.
- **Proptests use a fixed seed and no failure persistence** (`ProptestConfig { rng_seed: RngSeed::Fixed(..), failure_persistence: None, .. }`; `crates/ludion-scene/tests/support/deterministic.rs` is the shared form): a red run must reproduce on every machine. A case a property finds becomes a named regression test, never a `.proptest-regressions` file.
- Testkit is host-side: its `clippy.toml` carries no denylist (see `determinism.md`); SSIM purity
  is enforced by design and tests, not lints.

## Verify

```
cargo test -p ludion-testkit                                  # unit + gallery (GPU tests skip without an adapter, printing why)
cargo run -p ludion-cli -- test --dir examples/gallery        # the same suite as the user-facing gate (loud L2003 without a GPU)
cargo run -p ludion-cli -- test --dir examples/gallery --bless    # re-bless after an intentional render change; review before committing
```

On the machine that blessed the goldens, the gallery suite must pass at **exactly** SSIM 1.0 —
`tests/gallery.rs` asserts it, proving the render → encode → decode → compare chain is
byte-deterministic on one machine, not merely perceptually close. The suite resolves
`goldens/<os>/` automatically, so a bless on Windows writes the `windows/` set and a bless on
macOS the `macos/` set; blessing never touches the other platform's goldens.

## Gotchas

- Testkit has its **own local PNG decoder** (`src/image.rs`), deliberately not `ludion-assets`
  — the two decoders are independent (see `ludion-assets.md`). It accepts only what our encoder
  writes (8-bit, after EXPAND/STRIP_16 transformations); goldens are always produced by this
  crate's own bless flow.
- `sim_assert` depends on `ludion-ecs`/`ludion-sim` directly rather than through
  `ludion-engine`, a deliberate deviation from the architecture doc's diagram — see
  `crate-graph.md`; don't move testkit down the graph or add re-exports to `ludion-engine` to
  "fix" it.
