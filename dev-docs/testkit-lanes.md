---
description: Testkit lanes: GPU-less skips, needs_gpu_* naming, spelling gate, nextest lane, crash sighting
---

# testkit-lanes

**Scope:** how testkit-backed tests run and gate in CI (`ludion devdocs ludion-testkit` is the hub; the comparison itself is `ludion devdocs testkit-golden`).

## Rules

- **GPU-less environments: skip in `cargo test`, loud failure in `ludion test`.** In-repo
  `cargo test` suites (e.g. `tests/gallery.rs`) skip gracefully — printing why — when the render
  fails with `L2003` (no GPU adapter), and *only* on `L2003`; any other diagnostic is a real
  failure. Every such skip routes its detection through `ludion_core::require_gpu`: with
  `LUDION_REQUIRE_GPU=1` (set by `check.mjs`'s test lane and the pinned CI agents; unset under
  `--profile no-gpu`) a missing adapter panics instead, so a lane that gates images cannot pass by
  skipping. `scripts/lists-in-step.mjs` rejects an unrouted skip.
  **A test that puts a wgpu device up — directly or through a subprocess that does — is named
  `needs_gpu_*`:** that name is the nextest `gpu` group's filter and the `no-gpu` profile's exclusion
  (hosted CI runs `--profile no-gpu`), and `lists-in-step` flags a device-touching test without it.
  `LUDION_FORBID_GPU=1` is the runtime oracle: device creation (`Gpu::request`, `Presenter::new`)
  panics, so run `LUDION_FORBID_GPU=1 cargo nextest run --workspace --no-fail-fast` to list the tests
  that need the prefix — hosted CI sets it on the no-gpu lane. `ludion test` is the user-facing
  gate and never gets the skip: a missing adapter is a non-zero-exit `L2003` failure like any other
  render error.

### The spelling gate

`assert_saves_respell_nothing(project, roots, builder)` (`spelling.rs`) proves a committed
hand-authored scene or prefab already carries the spelling the writer would give it (audit
scene-text S1): a hand edit saved by `scene.save` must not drag float or quoting churn with it. It
loads each scene through `scene.load` and `Engine::scene_text`, re-emits each prefab through
`check_prefab` + `emit_prefab`, and compares every value scalar's source spelling per JSON Pointer,
read with `saphyr_parser` directly so the oracle shares no code with the writer. Comments and
layout are not compared. A same-value respelling (`1` → `1.0`, a widened f32, quoting) is
blessable: `LUDION_BLESS=1` rewrites only the scalar's characters. A changed or dropped value, or a
file that fails to load, is a load/save defect and is never blessed. Skipped: any file with a
top-level `generated_from:` (own drift gate), any `kind:` other than prefab, and the dirs `.ludion`,
`target`, `goldens`, `node_modules`. `project` is the content root the game loads from (vigil's — private, Lore-only — and
shmup-stress's is `scenes/`); zero files checked panics. Each game's `tests/drift.rs` (or
`stress.rs`) and `tests/committed_scene_spelling.rs` wire it.

### The workspace suite lane

**How it runs.** The test lane — in `scripts/check.mjs`, CI, and TeamCity alike —
is `cargo nextest run --workspace` followed by `cargo test --workspace --doc`: nextest runs every
test in its own process, scheduled across all cores instead of one test binary at a time, and it
cannot run doctests — the second command is the other half of the same gate, not an optional
extra. `.config/nextest.toml` bounds the few tests that must not be co-scheduled: a `gpu` group
(every `needs_gpu_*` test, i.e. anything that creates a wgpu device, capped so N simultaneous devices never contend), a
`wall-clock` group (assertions measured against real time; the ≥30 fps delivery floors — every codec
twin, matched by name prefix — claim the whole run pool and run first, so they measure an unloaded
machine), and a `cargo-build` group (tests that spawn
`cargo` and contend on its build lock). Tests slower than 60 s are reported, never killed
(`slow-timeout` with no `terminate-after`), and check.mjs surfaces that slow list in its summary.
Without nextest on PATH the lane falls back to plain `cargo test --workspace` — serial and slower,
but gating exactly the same set.

### Wall time

The default `check.mjs` run is about 756 s on the reference 32-core box (it was 914 s before the sync
fingerprint, 2026-11-01). The critical path is roughly the total: import-sync → test → doctest →
vigil-clippy → vigil-test → mobile → fps-gate. The root test step (~390 s) is the largest.
`importer_fingerprint` hashes the ludion binary, so every ludion-cli rebuild re-imports all vigil kits
(~60 s of import-sync); the source-derived fix is owed (`Plan/owed.md`, Engineering debt). Time a run
only on a tree with no foreign dirty crates: the `ludion-cli` `dev` and `new` tests build a scratch
game against the live workspace, and another instance's uncommitted engine edits make them recompile
or time out (root test then reads 500–550 s). `node scripts/check.mjs --history` holds the per-step
numbers.

## Gotchas

- `scripts/check.mjs` exits 1 when `LUDION_BLESS` or `BLESS_SCHEMA` is set: a session-wide bless makes
  every golden test rewrite its file (the schema test its snapshot) and pass, so the lane would report
  green over rewritten baselines.
- The skip-in-`cargo test` pattern keys on `L2003` *specifically* — do not widen it; a broken
  scene or missing texture must still fail the suite on a GPU-less machine.
- **A saturated Windows box can kill a process holding a wgpu device with `0xc0000005`**
  (STATUS_ACCESS_VIOLATION) — no Rust panic, no wgpu validation message; captured stdout is
  `running 1 test` then `(test aborted)` with no libtest result line, so the fault is inside the
  test body, not teardown. Seen twice — both times mid-suite on a loaded box (a `check.mjs` lane
  alongside a concurrent cargo build and two replays), once in `ludion-renderer --lib` and once in
  `ludion-server::phase10` — and never reproduced across ~170 targeted executions, including the
  `gpu` group raised to 8 and 32. Any wave that adds a shader or pipeline shifts the test schedule
  and will look guilty: do not accept a small stash/restore A/B as proof of a regression, and do
  not lower the `gpu` group's `max-threads = 4` in `.config/nextest.toml` in response. A third
  sighting needs a crash dump: set `HKLM\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps`
  with `DumpType=2` (requires admin) — one dump names the faulting module and settles whether this
  is the DX12 driver, wgpu, or engine code.
