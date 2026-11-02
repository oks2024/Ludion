---
description: ludion-ecs registry, schedule, unwind boundary, hierarchy and presentation phases
---

# ludion-ecs: registry, schedule, hierarchy, presentation

**Scope:** Changing `registry.rs`, `schedule.rs`, `hierarchy.rs` or the presentation phase guard. Hub: `ludion devdocs ludion-ecs`.

## Rules

### Registry, schedule

**`Registry`** records name, `TypeId`, `schemars` JSON Schema, and erased fns operating on a `World` by
`EntityId`; every diagnostic is tagged with the component name (`L0007` on serde failure, `L0006` on
shape mismatch). Lookup misses are `L0004`, built by `Registry::unknown_component` — the single owner
of that diagnostic, so the dispatcher's query filters raise the identical message; its help comes from
`Registry::name_suggestion`, which scene validation's `L1013` shares. This one table powers scene IO,
snapshots, the inspector, and protocol validation.

**`Schedule::run`** executes strictly in registration order — no parallelism, no dependency inference.
`SystemCtx` is the only way time enters the sim: `frame`, `time`, and a fixed `dt` (`1/tick_rate`,
never varying). Two observation variants sit beside `run`, and the split is deliberate.
`run_observed(…, &mut dyn ScheduleObserver)` brackets each system with callbacks receiving a **name
only** — the profile driver's clock-free seam, kept state-free so an observer cannot become a back
door into the world. `run_probed(…, &mut dyn FnMut(&'static str, &World))` hands the probe the world
**as each system left it** — what localizing a divergence needs and what an observer refuses to give.
Both are pure observation, so the world they leave (and its `WorldHash`) is exactly what `run` leaves.
Attribution is identical in all three, except that a probe runs with the recording system **cleared**,
because a probe's reads are host-side work and must not be attributed to the system it just followed.

**The unwind boundary.** Every system body runs inside `catch_unwind`. A panicking system used to take
the sim thread down, after which the host answered `L3009` forever with nothing to say about the
cause — the one authoring mistake in the engine that produced no diagnostic, in the layer an agent
writes most. Now the panic is caught, the *rest of that tick's systems are skipped*, and a
`SystemPanic { system, message, location, backtrace }` resource is left in the world. That resource is
the only channel back to the host — a `Schedule` is owned by the `SimDriver` that runs it, so the
world is the one thing both sides hold — and `ludion-engine` removes it at the frame boundary, renders
it as `L4007`, and auto-pauses. A process-global panic hook, installed once, suppresses the default
stderr print for a thread currently inside a system body (a thread-local flag), delegating to the hook
it replaced everywhere else, and records `file:line:column` plus the backtrace when `RUST_BACKTRACE`
asked for one — strictly more informative than the stderr line it replaced. The hook and its flag live
in `ludion_core::panic_capture`; the flag is inherited by the workers of a `par` region inside the
system, and `par` carries the lowest-index chunk's site back to the calling thread, so a parallel panic
reports that chunk's message *and* location, identical at any thread count (`tests/par_panic.rs`). On the happy path the
boundary is behaviourally inert: same systems, same order, byte-identical hashes and replays.

**Hierarchy** is exactly one plain component, `Parent(pub EntityId)`, registered like any other. No
stored child lists and no transform in this crate: `ChildMap::build(&world)` derives parent →
children in one pass over the column (full-`EntityId` match, ascending slot index, cycle-safe
`descendants`), and `despawn` / `despawn_many` cascade through it (the named entity, then
descendants ascending; a sim entity takes its presentation descendants even in `Sim` phase, a
presentation entity never takes a sim child; zero cost while the `Parent` column is empty — ruling
E1, `Plan/transform-hierarchy-and-prefabs.md`; `tests/hierarchy.rs`). A scene rejects a `Parent`
cycle (`L1035`, from the registry's `hierarchy` mark on `Parent`'s `x-entity-ref` schema, so
`ludion-scene` never names the type) and `scene.save` refuses to write one (`L1018`); the runtime walk,
in `ludion-engine`'s `hierarchy.rs`, degrades a loop, a dead parent or a chain past 64 to a root and
reports `L1218`–`L1220` once.

## Dataflow

### Presentation entities and phases

`spawn_presentation` takes ids from a second `EntityAllocator` offset by `PRESENTATION_BASE` (2^24), so sim ids never move and every column's presentation rows form one contiguous tail (keys ascend). Every system runs in a `Phase` (`Sim` for `add_system`, `Presentation` for `add_presentation_system`; `Host` outside systems, unrestricted). The guard, in prose:

- **Sim:** presentation ids are invisible — `has` false, `presentation_entities()` empty, queries and `entity_indices_with` stop before the base; `get`/`get_mut`/`insert`/`remove`/`despawn` return `L0028`; `spawn_presentation` panics with it.
- **Presentation:** sim entities are read-only — their writes return `L0028` (also queued and reported once, `World::take_boundary_crossings`), `spawn` panics; resource writes panic unless the type is in `allow_presentation_writes::<T>()` (outbox resources only).
- **Windowing:** each query builds one key window from the phase (`World::query_window`) and clamps every column with one `partition_point` in `window_slices`, before the join and the `par_for_each` chunking. A presentation query naming any `&mut` joins presentation rows only.

The why is in `Plan/presentation-state.md`.

## Verify

```
cargo test -p ludion-ecs
```
