---
description: Catalogue of budgets/ files: calibration scene, gated keys and measured values per file
---

# budget-corpus

**Scope:** the file-by-file catalogue behind `ludion devdocs budget-files` (the hub: calibration rules, CI wiring, gotchas, verify).

## Layout

The `*-particles-stress`, `*-physics-stress`, `*-shmup-stress` and `*-sprite-stress` families and
vigil's E6 pair carry `threads = 8`; the rest state no worker count.

- `budgets/pinned-rtx3090.toml` / `budgets/pinned-apple-m4.toml` — the engine's own tripwire,
  calibrated on `examples/gallery/lights.yaml` (no `PostProcess`, so it renders the shipped
  defaults), checked with `ludion profile --check`.
- `budgets/pinned-rtx3090-post-full.toml` / `budgets/pinned-apple-m4-post-full.toml` — the
  post-process arc's twin, calibrated on `examples/gallery/post-full.yaml` (the one scene authoring
  the whole `PostProcess` roster). A separate file per calibration scene, never merged — see Rules.
- `budgets/pinned-rtx3090-lights-stress.toml` / `budgets/pinned-apple-m4-lights-stress.toml` — the
  clustered-forward arc's, calibrated on `examples/gallery/lights-stress.yaml` (500 point lights, no
  post/sun), gating two lines: the `scene` pass at a light population that exercises the cluster
  path, and `cluster_build_ms` — the CPU binning cost, the one ceiling in the corpus that needs no
  adapter to measure, and the one that is *skipped* rather than failed when a report carries no
  `cluster` section (every `--attach` capture does). Both twins exist — the M4 file was measured
  2026-08-09.
- `budgets/pinned-rtx3090-particles-stress.toml` / `budgets/pinned-apple-m4-particles-stress.toml`
  — the particles arc's, calibrated on `examples/gallery/particles-stress.yaml` (~24,000 live
  particles over a score of endless emitters, alpha and additive mixed, no post/sun), gating
  `[passes]` (`transparents` — the particle pass since the opaque/transparent split — `scene` and
  `reflections`) and `extract_ms` — the first ceiling in the corpus on that metric, checked
  wherever a report carries an `extract` sequential entry (the offline `ludion profile --scene`
  driver and a live `--attach` capture both do). The RTX 3090 file exists (measured
  2026-09-07: extract 2.446 ms median, `scene` 0.9595 — the reading on which the particles plan's
  decision-9 `par` trigger fired); the M4 file was measured 2026-09-15 (extract 1.213 median —
  half the 3090 box's, a faster sim thread; `scene` 2.952, the Metal per-pass smear).
- `budgets/pinned-rtx3090-shmup-stress.toml` / `budgets/pinned-apple-m4-shmup-stress.toml` — the projectile-pattern arc's, calibrated on
  `games/shmup-stress/scenes/steady.yaml` (the 2,975-bullet field as closed-form volleys; the entity
  twin below is its A/B), checked in process by the game binary; measured on the RTX 3090 2026-09-12:
  extract 0.4229 ms median, `scene` 0.0594, `[systems]` `play_patterns`/`advance_volleys`/`player_hit_test`.
- `budgets/pinned-rtx3090-sprite-stress.toml` / `budgets/pinned-apple-m4-sprite-stress.toml` — the sprite arc's, calibrated on
  `games/shmup-stress/scenes/steady-drawn.yaml` (2,975 atlas-sprite bullets over 40 regions of one
  atlas, 357 additive, on a 480-cell scrolling `Tilemap` — 3,456 quads, unlit, no post/sun), gating
  `frame_total_ms` (the 60 fps deadline, vigil's E6 posture), `extract_ms` and `[passes]` (sprites
  draw in `transparents`; `scene` and `reflections` take flat floors). It
  lives in `budgets/` with the engine's own files but is checked **in-process by the game binary**
  like vigil's, since the CLI driver links none of the systems that build the field. Measured
  2026-09-12 on the RTX 3090: extract 1.694 ms against the same field drawn as flat quads at 0.852,
  `scene` 0.057344. The M4 file was measured 2026-09-15 (four 200-frame runs on 8 sim threads) — measured on each box, never derived.
- `budgets/pinned-rtx3090-physics-stress.toml` / `budgets/pinned-apple-m4-physics-stress.toml` — the physics arc's, calibrated on
  `examples/gallery/physics-stress.yaml` (512 mixed box/sphere/capsule rigid props falling into a
  static bin, `Plan/physics.md` W2), checked with `ludion profile --frames 300` (not the corpus's
  usual 120 — the pile's cost is time-varying), gating `[systems] physics_write`/`physics_step`/
  `physics_mirror` and `frame_total_ms`; no `[passes]`/`extract_ms`/`cluster_build_ms`. Measured on
  the RTX 3090 2026-09-23: `physics_step` median 1.2455 ms; the M4 file was measured 2026-09-26 (Lore rev 916, four 300-frame runs).
- `budgets/pinned-rtx3090-gi-inject-stress.toml` / `budgets/pinned-apple-m4-gi-inject-stress.toml`
  — the GI dynamic-injection arc's, calibrated on `examples/gallery/gi-inject-stress.yaml` (300
  `GiDynamic` point lights at the `MAX_PROJECTILES` cap over one `GiVolume`, no other light), gating
  `[passes] gi_inject` and `scene` plus `cluster_build_ms`. Its own file because `gi_inject` costs
  nothing on a scene that never schedules it. Both twins exist; each file's header records its
  four-run medians. Run it alongside the other `profile --check` steps (`ci/teamcity/README.md`).
- `budgets/pinned-rtx3090-mirror-floor.toml` — the reflections arc's, calibrated on
  `examples/gallery/mirror-floor.yaml` (a near-mirror floor at the full preset, `Plan/reflections.md`
  Wave 2), gating `[post_groups] ssr` (both pyramids, the trace and the blend), `[passes] scene` and
  `reflections`. Measured on the RTX 3090 2026-10-27: `ssr` 0.4332 ms, `reflections` 0.0512. The M4
  twin is owed.
- `budgets/image-stats-windows.toml` / `budgets/image-stats-macos.toml` — a different instrument:
  not timing ceilings but per-golden defect ceilings (`banding_score`, `edge_aliasing`,
  `clipped_low`, `clipped_high`), checked with `ludion image-stats --check`. Per-OS because the two
  GPUs render different bytes: the sets agree to within 0.003 on `edge_aliasing` but diverge up to
  29x on `banding_score` for flat-background content (`text.png`: 0.0090 on the RTX 3090, 0.2632 on
  the M4) — both files' headers record the sub-LSB-dither mechanism.
- `budgets/reflection-parity-<os>.toml` — the reflection parity gate's floors
  (`ludion-engine`'s `reflection_parity` test, `Plan/reflections.md` decision 27): per roughness band,
  the SSIM between the SSR-only and probe-only floor of a fitted box room, over the pixels where SSR is
  fully confident. The rule is `image-stats`' (the value is a pure function of one machine's render, so
  each floor sits a flat `margin` = 0.02 below the measurement); per-OS because GPU float differs.
  The sky-to-probe seam is recorded beside the bands, never checked. Re-calibrate with
  `LUDION_BLESS=1 cargo nextest run -p ludion-engine --test it -E 'test(/^reflection_parity::/)'`; Windows was calibrated
  on the RTX 3090 (2026-10-27), macOS on an unpinned M5 (the M4 re-calibration is owed), and a pinned
  OS with no file fails the test until it is calibrated.
- `games/vigil/budgets/pinned-rtx3090.toml` / `pinned-apple-m4.toml` — vigil's own E6 gate (vigil is the private reference game: its files exist only in the Lore tree, absent from a public clone) over
  `scenes/budget-horde.yaml` (the generated 3x-scale, 500-enemy/300-projectile/200-pickup horde),
  checked **in-process**, from `games/vigil/`: `cargo run --release -p vigil -- --scene scenes/budget-horde.yaml
  --profile 120 --profile-check <file>`. No `--attach` needed — a game binary links its own systems,
  which is the whole reason `ludion::run`'s `--profile`/`--profile-check` pair exists alongside
  `ludion profile` (`ludion devdocs ludion-cli`).
- `games/vigil/budgets/pinned-rtx3090-district-breach.toml` — vigil's scene-load tripwire (private, Lore-only):
  one `load_total_ms` line over `scenes/district-breach.yaml`, checked by hand from `games/vigil/` with
  `--profile 30 --textures cooked --profile-check <file>` after a `ludion sync`; measured at `cooked`,
  median 1764 ms (the plaza twin, 2427 ms, ceiling 5000). The `pinned-apple-m4-district-{breach,plaza}.toml`
  twins read 458 / 1886 ms (ceilings 1000 / 4000); most of the gap is the 3090's ~945 ms `renderer_init`.
- `games/vigil/budgets/stress-spatial-{rtx3090,apple-m4}-{1,2,8}t.json` (and the sibling
  `stress-baseline-*`/`stress-j1-*` files) — **not budget files.** No ceilings; each is a committed
  raw `ProfileReport` JSON dump (one per thread count) evidencing the deterministic-job-system's
  chunk-size/thread-scaling campaign (`Plan/deterministic-job-system.md`). No gate reads these — they
  are recorded measurements, distinguished from the enforced `.toml` files by extension alone.
  **Two of the three families are frozen.** `stress-baseline-*` measures pre-J1 code and `stress-j1-*`
  pre-spatial-split code — neither exists any more, so those four files can never be re-recorded and a
  system renamed inside one is a falsified measurement (it happened once, 2026-09-13, and was reverted
  2026-09-16). Only `stress-spatial-*` measures an architecture the engine still has, so only it is
  re-recordable — on both boxes at one rev, since a pair at different revs compares nothing.
