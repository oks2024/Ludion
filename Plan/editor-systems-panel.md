# Plan: Editor Systems Panel

> Status: **evaluated, not scheduled.** The introspection seam (`describe.systems`) already ships; the panel that renders it does not. This plan records the design so the decision to build it is made from a written evaluation, not on a whim, and so the seam is not over-built ahead of a consumer.

## Trigger criterion

Build the panel when **either**:

1. per-system timings become streamable to the editor (the profiler already measures per-system cost for `ludion profile`; exposing them as a live wire signal is the natural pairing that makes a Systems panel more than a static list), **or**
2. the first real schedule-introspection demand lands from a designer using the editor — "what runs, in what order, and where is it wired?" asked against a live game, not from the CLI.

Until then `describe.systems` is **agent/CLI surface**: an agent reads the ordered schedule and each system's `registered_at` straight off `engine.describe`. Not before — a panel with no timings and no designer asking for it is engine-vanity (vertical-slice rule).

## What it is

A read-only editor panel that renders the game's schedule from `describe.systems`:

- **The ordered list** — each system's `name` and `order` (`order` is the registration index, which is the execution order; systems run strictly in the order added). The designer sees exactly what runs each tick, top to bottom.
- **Jump to definition** — click a system → open its `registered_at.file:line` (the `.system(…)` wiring call site) in the editor. The seam already normalizes `file` to forward slashes so a path recorded on Windows resolves anywhere.
- **Later, per-system timings** — once trigger (1) lands, annotate each row with its live cost, turning the list into a lightweight in-editor profiler. This is the feature that earns the panel over the plain CLI dump.
- **Later, jump to the *definition* (not just the wiring)** — `fn_path` (the `type_name` of the system fn, e.g. `vigil::movement::move_player`) is already captured on `SystemSpec` but **deliberately unexposed** by `describe.systems` — kept on the spec only until a consumer needs it. When the panel wants "open where this system is *defined*" (distinct from where it was *wired*), expose `fn_path` then, not before.

## The designer-flow story (why it fits Pillar 6)

A designer opens the panel, sees the tick's schedule in execution order, and spots the system they want changed — say the enemy AI. They point the agent at it ("the enemy approach in `enemy_ai` should slow near walls"), and the agent edits the Rust and reloads through the existing `ludion dev` recompile + `engine.snapshot`/`engine.restore` loop — no scripting language, no live-editable behavior graph. The panel is a **map and a pointer**, not an authoring surface: it makes the schedule legible and gives the designer a precise handle to hand the agent. Behavior stays Rust (Pillar: "Rust is the only Turing-complete layer"); the designer describes, the agent writes.

## What already exists (the seam, zero new wire methods)

- `EngineBuilder::system` is `#[track_caller]` and generic over the fn type: it captures the wiring call site (`std::panic::Location::caller()`) and the definition path (`std::any::type_name`) into a `SystemSpec` at build time. One indirect call per tick, unchanged — the metadata is free at runtime.
- `engine.describe` carries a `systems` array of `{ name, order, registered_at: { file, line } }`, in registration order. The wire surface stays at **26 methods** — this is a payload on an existing method, not a new one.
- The `L1124` schedule-concentration boot warning (`SCHEDULE_MODULE_CONCENTRATION`) already groups systems by `fn_path`'s definition module and nudges when one module concentrates too many — the same metadata the panel would render, proving the seam works end to end.

The panel is therefore **pure client work** when it comes: no protocol change, no new engine capability, no parity gate to clear (the command already exists). That is exactly why it can wait — building it early would freeze a UI against a seam whose one remaining consumer-driven field (`fn_path`) and its timing pairing are still unproven.
