---
description: Budget files hub: calibration rules, CI wiring, gotchas, verify; the catalogue is budget-corpus
---

# Budget Files

**Scope:** The `budgets/` and `games/vigil/budgets/` file corpus (vigil's files are private — Lore-only, absent from a public clone) — what distinguishes each file,
the convention that sets a ceiling, and what a contributor touching a pass/system/scene must do to
the files that gate it. Defers the `Budget`/`ThresholdFile` schema, the `L4004`–`L4006` and
`L4009`–`L4010` codes, and the CLI mechanics entirely to `ludion devdocs ludion-cli` (`profile
--check` / `image-stats --check`) and `ludion devdocs profiling` (the report shape, the
`--scale`/`--threads` comparability rules) — this topic covers the files, not the machinery that
reads them.

## Layout

The per-file catalogue (one entry per calibration scene and per-OS twin, with measured values) is `ludion devdocs budget-corpus`. Families: engine tripwire (`pinned-*.toml`), `-post-full`, `-lights-stress`, `-particles-stress`, `-shmup-stress`, `-sprite-stress`, `-physics-stress`, `-gi-inject-stress`, `image-stats-*`, vigil's E6 files, and the frozen `stress-*.json` recordings.

## Dataflow

- Both engine-level files and vigil's E6 files flow through the identical core: measure with
  `ludion_engine::profile_scene`/`profile_engine`, parse the TOML into `Budget`
  (`crates/ludion-engine/src/profile.rs`), compare with `check_budget`. `ludion devdocs ludion-cli`
  owns that pipeline's detail; what matters here is that **one report shape feeds both file
  families**, so the same calibration reasoning applies whichever binary produced the numbers.
- **A budget only means something on the scene/config it was calibrated on.** `post-full.yaml`
  costs roughly 2x `lights.yaml` on the `scene` pass alone (0.103 ms vs 0.061 ms on the RTX 3090) and
  `lights-stress.yaml` costs 11x it (0.69 ms), so checking one file against another's scene would
  either always fail or silently pass at several times true headroom. This is why there is one file
  per calibration scene rather than one merged file, and why `ci/teamcity/README.md`'s
  `budgets`/`budgets-macos` configs run **every** `profile --check` invocation their platform has a
  file for: a scene a CI step forgets to profile is a tripwire disarmed by omission, which no
  diagnostic catches — only running the scene does.
- The engine-level `budgets`/`budgets-macos` TeamCity configurations are pinned-GPU-gated (they run
  on the hardware the ceilings were calibrated on, in `--release`) — see that README's job 8 for the
  exact steps. The sprite-stress file rides that same Windows configuration, as one extra step
  invoking the game binary's own `--profile-check` — a game-checked budget wired into CI, which is
  the shape a future vigil step would take. **Vigil's E6 files themselves are still checked by hand,
  by no CI configuration**: grepping `.github/workflows/ci.yml`, `ci/teamcity/`, and
  `scripts/check.mjs` for `budget-horde` finds nothing. CLAUDE.md's Current-state records the E6 gate
  as "re-measured at every milestone close" — a manual ritual, not an automated tripwire. Do not
  assume a vigil budget breach would be caught by any green CI run.

## Rules

- **The engine-level calibration rule:** every ceiling is set to roughly twice the median measured
  over 120 frames in `--release`, so a failure means a real, reproducible regression rather than
  machine noise (stated in every `budgets/pinned-*.toml` header, e.g. `pinned-apple-m4.toml` lines
  1–4, and restated in `ci/teamcity/README.md`'s `budgets` job: "Budgets carry ~2x headroom over the
  observed median"). `image-stats-*.toml` uses a **different** rule, because a defect score is a pure
  function of committed bytes rather than a noisy wall-clock measurement: ceiling = measured value +
  0.02, clamped at the metric's own 1.0 ceiling (`budgets/image-stats-windows.toml` lines 9–12, 44–47).
- **Vigil's E6 gate splits the 2x convention further.** Systems doing real work get ~2x their median
  (`update_flow_field` 0.70 = 2x of the measured 0.32–0.34); any system under ~0.01 ms gets a flat
  0.05 ms floor instead, because 2x-of-near-zero still trips on scheduler jitter
  (`games/vigil/budgets/pinned-apple-m4.toml` lines 21–25). And E6's `frame_total_ms` (16.6) is **not**
  2x anything — it is the fixed 60 fps deadline, a hard requirement rather than a regression tripwire
  (same file, line 43: "A fixed target, not 2x a median"). Contrast the *engine's* own
  `frame_total_ms` (0.5 ms in `pinned-apple-m4.toml`), which genuinely is a 2x-median tripwire on
  built-in loop overhead, since the CLI driver links no game systems. Same key name, two different
  calibration philosophies, in two different files — read the file's own header before assuming which
  one applies.
- **Worst-frame keys: `frame_total_max_ms` and `[systems_max]`.** They gate a report's `tail.max_ms`
  (the single worst frame — `ludion::run`'s `--profile-replay` and the offline driver fill it)
  instead of the median; an `L4005` names the frame (`system `x` (worst at frame N): max …`). A
  report with no tail (a live `--attach` capture, JSON predating the field) is skipped, never failed,
  and an unknown `[systems_max]` key is `L4006` like `[systems]`. A max ceiling is a spike tripwire:
  calibrate it from a replay's observed `max_ms` with headroom well above 2x — a worst frame is far
  noisier than a median. No committed budget uses them yet.
- **A ceiling only tightens deliberately, never automatically.** When a machine-wide swing moves
  every group by a similar amount (a driver update, thermal state), the file records the swing but
  leaves every ceiling where it was — re-cutting on a global swing is how a tripwire becomes flaky
  (`budgets/pinned-rtx3090-post-full.toml`'s header: all four `[post_groups]` moved ~10% "including
  `bloom`, which had no reason to move at all," and no ceiling moved). A *per-group* swing with a
  known cause, by contrast, is re-measured and left at ~2x the new value with the reason recorded
  inline (`pinned-apple-m4-post-full.toml`'s `shafts` group: +9.0% from the volumetric medium's new
  per-step arithmetic, cross-checked against the RTX 3090 twin's +7.3% on the identical change).
- **A partial re-cut across a per-OS pair is deferred whole, not split.** When an engine-wide win
  verifies on one platform first, the other platform's ceilings are left loose too, with the pending
  cut recorded as a note rather than actioned half — cutting only the twin measured first would leave
  the two files disagreeing about what the gate means. (`games/vigil/budgets/pinned-apple-m4.toml`'s
  2026-08-01 J1 note: `move_projectiles`/`spatial_rebuild` "want re-cutting in BOTH per-OS files in
  one deliberate pass".)
- **Adding, renaming, or regrouping a pass is a `ludion-engine`/`ludion-render-graph` change first.**
  The producible-name tables that decide whether a stale budget key is `L4006` (a likely
  rename/typo) or simply not-measured-this-run live in `crates/ludion-engine/src/profile.rs`
  (`ludion devdocs ludion-cli` names it; rule 3 bars re-listing its contents here). The budget-file
  obligation on top of that code change: re-measure with `ludion profile --json` (or the game's
  `--profile`) on the pinned hardware in `--release`, replace the ceiling per whichever convention
  above the file uses, and leave the old measured value in a comment — every file in the corpus does
  this, and a ceiling with no measured-value comment leaves the next contributor unable to tell
  whether a future breach is real or was always this loose.
- **SSR cost is the `ssr` post group** (the Hi-Z and colour pyramids plus the trace; `reflections` runs
  every frame for the sky, so it is its own `[passes]` line). A pyramid pass is one per level, named
  `hiz_reduce_<k>` / `ssr_color_reduce_<k>`: a `[passes]` key may name any level, and the group sums them all.
- **A budget with GPU lines needs a GPU measurement.** `[passes]`, `[post_groups]` and
  `post_total_ms` against a run with no adapter, or no timestamp queries, is `L4019` (exit non-zero),
  not a quiet pass; a pass merely absent from a run that did measure stays quiet. A CPU-only budget
  carries none of those lines. `committed_budget_files_parse_and_name_producible_keys` loads every
  committed budget CPU-only, so a renamed pass fails per push.
- **Load ceiling: `load_total_ms`.** It gates `ProfileReport.load.total_ms`, **one sample, not a
  median**: calibrate from the median of three warm runs (cold run discarded) and set about 2x. The
  total includes renderer creation (about 0.85 s on the RTX 3090 box, a fixed floor) and runs on a warm
  OS file cache. A report with no `load` section (a live `--attach` capture, `--profile-replay`, an
  unobserved host) is `L4025`, never a quiet pass. A no-adapter run's `load` lacks the render phases
  and is not gated for it. Vigil's district files are checked by hand, like E6, and measure the shipped `cooked` axis
  (`--textures cooked`, after a `ludion sync`); a report with texture `decoded` > 0 measured source.

## Gotchas

- **"Budget" is overloaded.** `L1112`/`L1115` (mesh-vertex-count / voxel-cell-count "style budgets")
  are an unrelated `ludion-scene` validation-time warning — nothing to do with the `budgets/`
  directory, the `Budget` TOML shape, or `L4004`–`L4006` (`ludion devdocs diagnostics` covers those
  two). Grepping "budget" in the codebase surfaces both; do not conflate a content-style budget with a
  profile budget.
- The `stress-spatial-*.json`/`stress-baseline-*.json`/`stress-j1-*.json` files sit in the same
  directory as the enforced `pinned-*.toml` files and share the `ProfileReport` shape, but carry no
  ceilings and are read by no gate. The `.json` vs `.toml` extension is the only reliable signal — a
  script that globbed `games/vigil/budgets/*` for enforcement targets would try to parse one of these
  as a `Budget` and fail.
- `deny_unknown_fields` is set on both the `Budget` and the `image-stats` `ThresholdFile` parsers, so
  a typo'd key is a hard parse failure (`L4004`/`L4009`) rather than a silently-ignored no-op. The
  failure mode that actually bites is the opposite: a **stale** key (a pass that still exists under a
  different name) parses fine and simply never breaches, unless it happens to collide with nothing
  producible — which is exactly the gap `L4006`/an unknown-metric check exists to catch, and why a
  rename must touch the budget file in the same commit rather than trusting the parser to notice.

## Verify

```
ludion profile --scene examples/gallery/lights.yaml --frames 120 --check budgets/pinned-apple-m4.toml
ludion profile --scene examples/gallery/post-full.yaml --frames 120 --check budgets/pinned-apple-m4-post-full.toml
ludion profile --scene examples/gallery/lights-stress.yaml --frames 120 --check budgets/pinned-rtx3090-lights-stress.toml
ludion profile --scene examples/gallery/physics-stress.yaml --frames 300 --check budgets/pinned-rtx3090-physics-stress.toml
ludion image-stats examples/gallery/goldens/windows/*.png --check budgets/image-stats-windows.toml
cargo run --release -p vigil -- --scene scenes/budget-horde.yaml --profile 120 --profile-check budgets/pinned-apple-m4.toml   # from games/vigil/
cargo run --release -p shmup-stress -- --scene games/shmup-stress/scenes/steady-drawn.yaml --profile 200 --profile-check budgets/pinned-rtx3090-sprite-stress.toml
cargo run --release -p vigil -- --scene scenes/district-breach.yaml --profile 30 --textures cooked --profile-check budgets/pinned-rtx3090-district-breach.toml   # from games/vigil/
ludion explain L4004   # L4005, L4006, L4009, L4010 read the same way
```

Swap `apple-m4`/`rtx3090` for the platform actually running — a budget checked on the wrong platform
is checked against numbers it was never calibrated against.

