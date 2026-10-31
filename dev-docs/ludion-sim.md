---
description: Fixed-timestep deterministic simulation, input scripts, and the world hash
---

# ludion-sim

**Scope:** The `ludion-sim` crate: the fixed-timestep tick loop (`SimDriver`), frame-addressed
input scripts (`InputScript`/`InputState`), run/pause/step state (`RunState`/`Speed`), and the
canonical `WorldHash`. For the rules this crate enforces (and how) see `determinism`; for the
crate's position in the workspace see `crate-graph`; for who instantiates worlds and calls the
driver see `ludion-engine` and `ludion-cli`.

## Layout

- `crates/ludion-sim/src/lib.rs` — crate doc with a full runnable scripted-run example; re-exports everything.
- `crates/ludion-sim/src/driver.rs` — `SimConfig`, `SimDriver`: owns `World`, `advance()`/`run_ticks()`, the normative tick order; plus the two watched ticks `advance_observed` (per-system names, the profiler's seam) and `advance_probed` (per-system *state*, the divergence-bisect seam), which thread a private `Watch` down to `do_tick` and change nothing but the watching.
- `crates/ludion-sim/src/input.rs` — `Key`, `MouseButton`/`GamepadButton`/`GamepadAxis`, `InputEvent`, `ScriptedInput`, `InputScript` (sorted-by-frame invariant), `InputState` resource (+ `MouseState`/`GamepadState`), the `f32_bits`/`f32x2_bits` serde helpers.
- `crates/ludion-sim/src/hash.rs` — `WorldHash::compute(&World, &Registry)` and `compute_with_report` (also names the first non-finite float), the normative byte-level hashing rules.
- `crates/ludion-sim/src/run_state.rs` — `RunState` (Running/Paused/Stepping), `Speed` (validated pacing factor, `L4001`).
- `crates/ludion-sim/tests/determinism.rs` — the in-process determinism suite (double-run hash-sequence comparison etc.).
- `crates/ludion-sim/clippy.toml` — strict determinism denylist (see `determinism`).

## Rules

- **`SimDriver` owns `World` outright.** No `Arc`, no `Mutex`, ever. All access goes through
  `driver.world()` / `driver.world_mut()` between ticks. This is the structural form of the
  "sim thread solely owns World" rule.
- **No wall clock in this crate.** Time enters systems only as `SystemCtx { frame, time, dt }`
  derived from the tick count. `Instant::now`/`SystemTime::now` are clippy build errors.
  Wall-clock *pacing* of a live run is the embedder's job; `Speed` is stored state the crate
  never reads.
- **`dt` never varies.** It is `1 / tick_rate` (60 Hz default via `TickRate`). `Speed` and
  `sim.set_speed` scale scheduling only, never tick contents.
- **Normative tick order** (frame *N*, in `do_tick`, four steps — `driver.rs` module doc): (1)
  `InputState::begin_tick()` clears edge sets, then every scripted event addressed to frame ≤ *N*
  applies in script order; (2) the world's `GameClock` folds, if present — a frame the game marked
  frozen becomes one more held frame, and the fold reports whether gameplay time advanced into this
  tick; a world with no clock behaves as an unheld one; (3) systems run via `Schedule::run`, in
  registration order, against `SystemCtx::new(N, rate).with_clock(clock, advanced)`; (4) frame
  counter becomes *N*+1. `ludion-engine`'s `extract_at` re-reads the clock again on its own,
  after the tick completes, for its own gameplay-time evaluation (`ludion devdocs ludion-engine`).
- **`InputScript` is always sorted by frame, stably.** Every construction path (`push`,
  `From<Vec<ScriptedInput>>`, serde deserialize) canonicalizes; same-frame events keep push order.
- **Analog input crosses serde as an IEEE-754 bit pattern, never a JSON number.** Every f32 in a
  mouse/gamepad event and its state (`MouseMove.position`, `MouseWheel.delta`, `GamepadAxis.value`)
  serializes through the `f32_bits`/`f32x2_bits` helpers under a `*_bits` field (a `u32`), so a
  replayed analog value is bit-identical by construction. `serde_json` parses a JSON number via
  `f64`-then-cast, and that decimal→f64→f32 double rounding is not guaranteed to reproduce the
  original bits — the P0 determinism pillar forbids leaning on the parser's luck. Keys and button
  names stay fully symbolic and hand-writable; only the analog channel is bit-encoded.
- **`WorldHash` byte rules are versioned.** Domain tag `ludion.worldhash.v2`; changing any
  encoding rule bumps the suffix and re-blesses recorded hashes.
- **Seeding is the driver's job.** `SimDriver::new` inserts `DeterministicRng::new(config.seed)`
  and a fresh `InputState`, *replacing* any the caller pre-inserted — `SimConfig` is the single
  source of truth for a run's seed. The engine overrides that fresh state per rebuild
  (`SimDriver::carry_input_from` on a switch, the snapshot's `input` on a restore).

## How it works

`SimConfig { tick_rate, seed, input }` plus an initial `World` and a `Schedule` fully determine
every tick — that triple is what a replay header will capture. `SimDriver::advance()` performs at
most one tick according to `RunState`: `Running` ticks, `Paused` does nothing, `Stepping {
remaining }` ticks and decrements to `Paused` at zero (`step(n)` is the future `sim.step` wire
command; `step(0)` pauses immediately). `run_ticks(n)` is the headless convenience: `step(n)` then
drain `advance()` as fast as possible — no wall clock involved, bit-identical everywhere.

A system panic (caught by the schedule, which skips the rest of that tick) is the driver's to
report: `do_tick` removes the `SystemPanic` resource, holds the rendered `L4007` `Diagnostic`, and
forces `Paused`, so `run_ticks(n)` stops at the failing tick. `take_system_panic()` hands it out once;
`Engine` forwards it to its log and `sim.step` reply, the testkit helpers fail on it, and the bare
`render`/`sweep` hosts return it. The driver holds the panic and stays `Paused` until `take_system_panic` takes
it, so a caller stepping a bare driver must take it to see the failure.

Input is data, never OS events: `InputEvent::KeyDown/KeyUp { key: Key }` serializes as
`{"kind": "key_down", "key": "W"}`; a script is a plain JSON array of `{frame, event}` records.
`InputState` (a `World` resource, backed by `ludion_core::collections::StableSet`) exposes
`is_held` (level), `was_pressed` / `was_released` (edges, cleared each tick). A `KeyDown` on an
already-held key is not a new press; a `KeyUp` on a non-held key is ignored — application is
idempotent per level, so auto-repeat quirks cannot change a script's meaning.

`InputState` also carries a `mouse: MouseState` and a `gamepad: GamepadState` (accessed via
`input.mouse()` / `input.gamepad()`), each mirroring the key vocabulary. **Level** values persist
across ticks: `mouse.position()` (render-target pixels, origin top-left, +Y down), the gamepad
sticks (`[-1, 1]` per axis, +Y up) and triggers (`[0, 1]`). **Edges** clear each `begin_tick`:
mouse and gamepad buttons (`is_held`/`was_pressed`/`was_released`, same idempotent rules as keys),
plus `mouse.wheel()` — an accumulator that sums this frame's `MouseWheel` deltas and resets to zero
next tick. The matching events are `MouseMove`/`MouseDown`/`MouseUp`/`MouseWheel`,
`GamepadAxis { axis, value }` (one generic event names one of six axes), and
`GamepadButtonDown`/`GamepadButtonUp`. Each trigger axis also drives its `GamepadButton` at
`TRIGGER_THRESHOLD` (held ⇔ value ≥ 0.5), and a trigger button event sets the axis. Capture is out-of-sim, in whichever window is hosting (the
facade's in-process `--window` or the `ludion-window` viewer — both emit only `input.inject`);
headless leaves `InputState` at its default (mouse `[0,0]`, no buttons, zero axes), the correct
quiescent degrade.

`WorldHash` is XXH3-64 (seed 0) over a canonical byte stream: preamble, then every *live* entity
in ascending slot-index order (`E` + index/generation as u32 LE), then each entity's *registered*
components in registry registration order (`C` + length-prefixed name + canonical JSON-value
encoding with explicitly sorted object keys and ryu shortest-round-trip number text). Resources —
including RNG state — and the free-list shape are deliberately excluded: the hash covers
observable world content; divergence elsewhere must eventually surface as diverging component
data, which the per-frame hash sequence catches. Display/serde form is exactly 16 lowercase hex
digits (`WorldHash::parse` accepts only that).

## Verify

```
cargo test -p ludion-sim
cargo run -p ludion-cli -- sim-hash --scene crates/ludion-cli/tests/fixtures/good.yaml --script crates/ludion-cli/tests/fixtures/script.json --frames 600
```

Run the `sim-hash` invocation twice: the two hashes must be identical (the cross-process
determinism check; CI layer 2). `cargo test -p ludion-sim` includes
`tests/determinism.rs::double_run_produces_identical_per_frame_hash_sequences` (CI layer 1).

## Gotchas

- `driver.frame()` is the *next* frame to be simulated, i.e. the count of completed ticks — after
  `run_ticks(3)` it is 3, and the systems of that run saw `ctx.frame` = 0, 1, 2.
- Unregistered component types are invisible to `WorldHash` (as they are to scenes and
  snapshots; the engine warns `L0027` when a system creates one). Hashing a world against a registry missing a component silently ignores that data —
  always hash with the same registry the world was built from (the CLI uses
  `ludion_engine::builtin_registry()` for both).
- With no systems registered, ticking never changes world content, so `--frames 0` and
  `--frames 600` hash identically at CLI level (pinned by `crates/ludion-cli/tests/sim_hash.rs`).
  A hash that doesn't change with frame count is not automatically a bug.
- `run_ticks`/`step` *replace* the current run-state (steps do not accumulate), and `run_ticks`
  leaves the driver `Paused` — call `resume()` before reusing a driver in free-running mode.
- A non-finite float in a component does **not** fail the hash: NaN, `+inf`, `-inf` and `None`
  each hash distinctly (a NaN's sign and payload are ignored, as x86 and ARM disagree on them), and
  the first `Engine::world_hash` that meets one warns with `L4024`, once per component type per
  session (`compute_with_report` carries the site). `L0007` from
  `WorldHash::compute` means a component's own `Serialize` implementation failed, which no derived
  component does.
- `Speed` validates on construction *and* deserialization (`#[serde(try_from = "f64")]`); zero,
  negative, NaN, infinite, and out-of-`[1/64, 64]` factors are `L4001`, never a stalled scheduler
  (`Duration::from_secs_f64` in the live loop overflows on a tiny factor).
