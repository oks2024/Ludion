---
description: Testkit golden path: SSIM + colour gate, per-OS goldens, render seams, DiffReport, bless, suites
---

# testkit-golden

**Scope:** the comparison and bless half of `ludion-testkit` (`ludion devdocs ludion-testkit` is the hub): the gate, SSIM method, report, run vs bless, suites. GPU-less skips and the lane live in `ludion devdocs testkit-lanes`.

## Rules

- **Images gate on perceptual comparison, never bit equality** (`CLAUDE.md` tiered determinism):
  CPU-side sim state is bit-exact, but bit-exact image goldens only hold on a pinned GPU, so the
  gate is three conditions: global SSIM ≥ threshold (`DEFAULT_SSIM_THRESHOLD` 0.99), **and**
  every tile ≥ `DEFAULT_TILE_FLOOR` (0.95, clamped to the threshold so a looser manifest threshold
  loosens it too), **and** every tile's colour error ≤ `DEFAULT_TILE_COLOR_CEILING` (4.0 on the
  0–255 scale, a fixed constant — a looser SSIM threshold does not loosen colour). A tile's score is
  its *worst* sliding window, not a mean: a 16×16 element vanishing from a 1080p image barely moves
  any mean, but its window scores near zero. The colour gate exists because SSIM is luma-only — a
  grayscale or red/blue-swapped render passes it. Calibrated on the reference box: every committed
  golden compares at exactly ssim 1.0 / colour 0.0, the one non-identical legitimate pair
  (two renders in `import`/`hierarchy_render`) at colour 0.003, while the weakest oracle
  perturbation scores colour 17.75 (grayscale) / 21.48 (R↔B swap) and 0.056 worst-tile SSIM
  (flattened 16×16). `tests/compare_oracle.rs` holds that line over every committed golden of both
  platforms. `DEFAULT_SSIM_THRESHOLD` is calibrated for the local/pinned-GPU tier only — hosted
  CI on a software rasterizer would need its own, looser calibration, which does not exist yet.
  At threshold 1.0 ("byte-exact") the pair must additionally be RGBA-identical — alpha included;
  a breach fails with `L2207`, its summary giving the differing-pixel count, the first pixel and the MAE.
- **Goldens are blessed per pinned OS.** The pinned tier is a per-(OS, GPU) pair, so each pinned
  platform keeps its own committed set under a per-OS subdirectory: `goldens/windows/<stem>.png`
  (RTX 3090) and `goldens/macos/<stem>.png` (Apple M4). `platform_dir()` resolves the host's
  subdirectory from `std::env::consts::OS`; any other OS is `L2205` — game golden tests treat that
  as a documented skip (print, pass), and so do in-repo suites. A byte-exact comparison (SSIM threshold
  1.0) of a committed per-OS golden also skips, with `L2210`, on a GPU that is not the pinned
  reference adapter (`LUDION_BLESS_ANY_ADAPTER=1` opts out; `LUDION_REQUIRE_REFERENCE_ADAPTER=1`
  makes the skip panic, for boxes that carry the reference GPU); `unverifiable` is the predicate
  every such test calls. The
  `goldens/manifest.json` overrides file stays at the `goldens/` **root**: resolution/threshold/frames
  describe the scene's content, not a platform.
- **A golden test is never a second render pipeline.** `GoldenTest::run`/`bless` render through
  `ludion_engine::render_scene_with_registry` (which `render_scene` itself delegates to) — the exact
  lockstep path `ludion render --scene` uses (check → instantiate → tick(0) → extract → render) — or
  `ludion_engine::render_scene_with_builder`, the exact path a game binary's `--render-out` uses, or,
  for a `kind: voxel`/`kind: mesh` source, through `ludion_engine::render_model_sheet`, the exact
  turntable-contact-sheet path `ludion render --model` uses. Either way render failures propagate as
  their own diagnostics, never masked as comparison failures.
- **A game golden test validates against the game's registry via the seam.** `GoldenTest::new`
  defaults its private `registry` field to `ludion_engine::builtin_registry`;
  `.with_registry(fn() -> Registry)` swaps in a game's (built-ins + its components), so a scene naming
  a game component renders instead of failing `L1013`. That render is still a static tick-0 frame —
  only built-in visual components are drawn; a game component is validated and instantiated but
  invisible — so no game *schedule* is involved, only its registry. **`.with_builder(fn() ->
  EngineBuilder)` is the seam that does involve one**: the scene renders through the game's whole
  engine, so `frames` ticks run the game's systems and a golden can be an early wave or a boss phase
  rather than the scene at rest. It supersedes `with_registry` when both are set (the builder's
  engine already validates against the game's registry). Scripted input stays out of scope — a
  committed replay is the tool for "after the player did X". Built-in directory suites
  (`discover`/`run_suite`) never touch either seam. (`fn` pointers derive `Clone`/`Debug`, so
  `GoldenTest`'s derives are unchanged.)
- **The comparison itself is bit-deterministic everywhere**: pure integer/`f64` CPU math over
  decoded RGBA8 — no GPU, no `f32` platform drift. Identical images score exactly `1.0`, no
  tolerance; unit tests assert this.
- **Artifacts are written on failure only**, into a build-output-style `out_dir` (the CLI uses
  `<dir>/target/testkit/`), never inside `goldens/`. A passing test leaves no trace beyond its
  return value.
- **Blessing records "what the renderer currently produces," not "this is correct."** Review the
  written PNG before committing it. Creating a missing golden counts as blessing it.

## Dataflow

**SSIM method (read `ssim.rs` before changing).** The textbook 11×11 sliding window over Rec. 709
luma, weighted by the separable binomial kernel `[1,10,45,120,210,252,210,120,45,10,1]/1024` (exact
rationals — no `exp`, so the score is bit-identical everywhere; identical images score exactly
1.0). Only fully-inside windows count; the global score is their mean. The fixed **8×8 tile grid**
only *localizes*: a window belongs to the tile holding its centre pixel and a tile scores its
minimum window (a tile with no window centre — an image under 11 px — falls back to one flat
window). Per tile, `color_error` is the largest of the R, G, B and A mean absolute channel differences
(exact integer sums, alpha included). Luma is Rec. 709 over the sRGB-encoded bytes directly (no
linearization — the comparison cares about the pixels written to disk); alpha is excluded from
SSIM. Constants are the textbook `k1 = 0.01`, `k2 = 0.03`, `L = 255`. `mean_absolute_error`
(all four channels, 0–255 scale) stays the secondary signal. A small colour-only change below tile
scale (a few pixels of one hue) is not gated: the colour error is a per-tile mean.

**The report.** Every completed comparison produces a `DiffReport` (serialized to disk only on
failure): golden/actual paths and dimensions, `threshold`, `tile_floor`, `color_ceiling`, `pass`, `ssim`, `mae`, `tile_grid`,
`damaged_tiles` (every tile below threshold or over the colour ceiling, lowest SSIM first), `worst_tile`
and `worst_color_tile` (even on a pass — margin visibility), and a one-line `summary`. Each
`TileDiff` carries row/col, an exact pixel `rect`, its `ssim` and `color_error`,
and one of nine compass labels (`top-left` … `center` … `bottom-right`) — an agent with no image
viewer gets "where, roughly" plus exact coordinates. A dimension mismatch is reported differently:
`pass: false` with `ssim`/`mae`/tiles absent, never a low score. `DiffReport::diagnostic()`
renders the rustc-style terminal summary (`L2202` mismatch, `L2203` below threshold or floor, `L2207` colour-only); the JSON
stays the primary artifact.

**Run vs bless.** `GoldenTest::run(out_dir)` renders, loads the golden (`L2201` missing, `L2204`
undecodable), and returns `GoldenOutcome::Passed { report }` or `Failed { report, actual_path,
report_path }` — on failure writing `<stem>.actual.png` and `<stem>.diff.json` into `out_dir`.
`GoldenTest::bless()` renders and (re)writes the golden unconditionally, returning `Blessed`
(`run` never blesses; `bless` never compares). Game projects gate the two on
`bless_requested()` — true when `LUDION_BLESS` is set non-empty/non-`0` (it delegates to `ludion_core::bless`, the one parse every bless switch shares; `lists-in-step` rejects a direct `env::var` read of one) — so `LUDION_BLESS=1
cargo test` is the bless flow for scenes naming game components (which `ludion test --bless`
cannot render: directory suites are built-in-registry only). An asked-for bless that cannot
render — no GPU included — must be a hard failure, never a skip. `bless()` also refuses (`L2206`)
a golden whose parent dir is the host's `goldens/<os>/` when the adapter does not contain the
platform's reference constant (`RTX 3090` / `Apple M4`); a scratch golden outside an OS dir is never
refused, and `LUDION_BLESS_ANY_ADAPTER=1` opts out (a game project whose pinned GPU is its own).

**Wire-side comparison.** `compare_png_bytes(golden_png, actual_png, threshold)` runs the
identical decode + comparison on two in-memory PNGs — the entry point for a `render.screenshot`
fetched over the protocol, where pixels never touch disk. File-based and bytes-based paths share
one private `report_for` function, so there is exactly one comparison-decision path.

**Suites.** `discover(dir, defaults, filter)`: every `*.yaml`/`*.yml` directly inside `dir` (no
recursion) is one test, paired with `<dir>/goldens/<os>/<stem>.png` (`<os>` from `platform_dir()`;
discovery fails with `L2205` off the pinned tier), sorted by filename for deterministic order.
`Path::file_stem` only strips the *last* extension, so a model document's `.voxel`/`.mesh` suffix
survives into the stem (`lantern.voxel.yaml` → stem `lantern.voxel` →
`goldens/<os>/lantern.voxel.png`) — the `GoldenTest`'s own `kind:` dispatch (above) is what then
renders it as a contact sheet instead of a scene; `discover` itself does not need to know the
difference. An optional `<dir>/goldens/manifest.json` (at the `goldens/` root, shared by every
per-OS set) maps scene stems to per-test `width`/`height`/`ssim_threshold`/`frames` overrides; every
field falls back to `SuiteDefaults`. The manifest is strict (`L2208`): unknown fields, an
`ssim_threshold` outside `(0, 1]`, a zero extent and a key matching no scene are refused.
`run_suite` runs (or blesses) all discovered tests and never stops early — one hard failure does
not hide the rest.

**Sim assertions.** `sim_assert` extracts the two hand-written patterns from `ludion-sim`'s own
determinism suite — "tick until a predicate holds" (`run_until`/`assert_reaches`) and "compare
per-tick `WorldHash` sequences" (`hash_sequence`/`assert_hash_sequences_match`, which names the
first diverging frame) — as plain functions, not a DSL. Callers still build their own `World`,
`Schedule`, and `SimDriver`.

## Gotchas

- `ssim::compare` (and `mean_absolute_error`) **panic** on differently-sized images — callers
  must check dimensions first, because a mismatch is a distinct report shape (no SSIM at all,
  not a low one). Go through `GoldenTest::run` / `compare_png_bytes`, which handle this.
- A same-machine SSIM just below 1.0 that still passes is a red flag, not a margin: on the
  blessing machine the comparison is expected to be exactly 1.0 (byte-identical render). The
  threshold exists for cross-machine/driver drift, not local noise.
- The score uses only exact arithmetic: binomial weights, no `exp`, `f64`, a fixed summation order —
  keep it that way (a transcendental would let two machines score the same pair differently), and keep
  every expression symmetric in its two inputs so identical images stay at exactly 1.0.
- `discover` does not recurse: a scene inside a subdirectory is invisible to the suite. That is
  the convention (assets/ and goldens/ live in subdirectories), not a bug.
