---
description: The custom ECS: World, generational entities, component registry, queries, schedule
---

# ludion-ecs

**Scope:** The custom, milestone-minimal ECS: `World`, generational entity allocation, per-component sorted dense-column storage, typed queries (sequential and chunk-parallel), the component `Registry` (name + JSON Schema + erased serde), sequential `Schedule`, typed resources, and the `Parent` hierarchy component. **The parallel-query region model is owned by `ludion devdocs ecs-queries`**, which `ludion devdocs determinism` and `ludion devdocs sim-parallelism` both point at. Scoped by the freeze list in engine-architecture.md §4. Depends only on ludion-core (see ludion-core for `EntityId` and diagnostics); consumed by ludion-scene for scene IO and by `ludion-sim` for ticking.

## Layout

- `crates/ludion-ecs/src/lib.rs` — crate root; `Component` trait (blanket impl), re-exports, determinism contracts in the crate docs.
- `crates/ludion-ecs/src/entity.rs` — `EntityAllocator` (crate-private): generational slots, LIFO free-list.
- `crates/ludion-ecs/src/world.rs` — `World`: spawn/despawn, insert/remove/get/get_unrecorded/get_mut/has, `read_column()` (a per-frame read-only column handle), `entities()`, `query()`/`query_ids()`, `par_for_each()`/`par_join_fold()`, `entity_indices_with(TypeId)` (an erased ascending presence cursor, for whole-world walks like the hash), resource methods, `ensure_alive`, and the opt-in access recorder.
- `crates/ludion-ecs/src/storage.rs` — `Storage<T>`: two parallel dense `Vec`s, `keys: Vec<u32>` (strictly ascending) aligned with `values: Vec<T>`, plus a per-id-range (sim, presentation) slot table so a presentation id never allocates the sim range's gap; `as_slices`/`as_slices_mut` are what queries and parallel chunks read. Type-erased `AnyStorage` (`#[doc(hidden)]`).
- `crates/ludion-ecs/src/par_query.rs` — `par_for_each`/`par_join_fold`, the `ParCtx` chunk context, and the `ParQuery`/`ParReadQuery`/`ParParam` machinery (`#[doc(hidden)]`). The region model lives here.
- `crates/ludion-ecs/src/query.rs` — `Query`/`QueryParam` and the join iterators (`Join1`–`Join4`), all `#[doc(hidden)]`; plus the public `QueryElemShape` and `Query::shape()`, the lifetime-free extraction seam for a *declared* query.
- `crates/ludion-ecs/src/registry.rs` — `Registry` and `ComponentInfo`, plus `serialize_erased` (a zero-copy `&dyn erased_serde::Serialize` borrow for callers driving their own serializer, e.g. the world hash) and `serialize_failed` (the one owner of the `L0007` message).
- `crates/ludion-ecs/src/schedule.rs` — `Schedule` (ordered `fn(&mut World, &SystemCtx)`) and `SystemCtx { frame, time, dt, game_frame, game_time, game_dt }` (the last three read through a world's `GameClock`, folded once per tick before systems run — `ludion devdocs ludion-sim`); the two observation entry points `run_observed` and `run_probed`.
- `crates/ludion-ecs/src/hierarchy.rs` — `Parent(pub EntityId)` with a hand-written `JsonSchema` impl, and `ChildMap` (the derived child map the cascade walks).
- `crates/ludion-ecs/src/journal.rs` — the opt-in write journal (crate-private): `SlotBits` per id range, per-marker watched/dirty/gone state, one union bit for the hot path.
- `crates/ludion-ecs/tests/conformance.rs` — generational safety, iteration order, registry order.
- `crates/ludion-ecs/tests/par_panic.rs` — a panic inside a `par` region reports one `SystemPanic` (lowest chunk's message and site) at 1/2/8 threads with nothing reaching the previous hook.
- `crates/ludion-ecs/tests/par_query.rs` — the parallel-join contract: 1/2/8-thread and chunk-size invariance against the sequential `query`, region semantics, panic propagation, recording parity.
- `crates/ludion-ecs/tests/write_journal.rs` — the journal's exactness, membership, despawn/stale, ancestor expansion, restore, presentation range, `par_for_each` invariance, and a randomized model oracle.
- `crates/ludion-ecs/tests/presentation.rs` — the presentation id range, every row of the phase guard table through a real `Schedule`, query windowing (incl. `par_for_each` at 1/2/8 threads), the `Host` phase after panics and probes, `restore_presentation`.

## Rules

- **Freeze list is law** (architecture §4): no parallel scheduler, no archetypes, no general change ticks (the opt-in write journal is the one exception), no relations, no world splitting. Chunk-level data parallelism *within* one system is the one sanctioned exception. Budget 3250 code lines — non-blank, non-comment, outside the test module — under `src/`, gated by `scripts/ecs-budget.mjs`; raising it is the user's decision (raised from ~2–3k by `Plan/deterministic-job-system.md` J1 for the par region machinery; to 3250 by Serious Isaac request 0039 for the write journal). Every addition must be exercised by the current milestone.
- **`ludion-ecs` never knows YAML exists.** The registry deals in `serde_json::Value` only; `ludion-scene` does the YAML↔Value translation.
- **Determinism contracts (conformance-tested):** the free-list is LIFO (most recently despawned slot reused first, fresh generation); storage and query iteration is ascending entity index, independent of spawn order; `Registry::iter` is registration order (the canonical key order for scene files *and* the world hash); resources are keyed by `TypeId` and never iterated, so `TypeId` ordering is never observable.
- **`despawn_many(&ids)` ≡ `despawn` on each id in slice order** (cascade included; same `L0002`s — an id an earlier cascade killed reports one — free-list order, storages) at O(N+k) instead of O(k·N); prefer it when the ids are already collected.
- **Write journal (opt-in, 0039).** `World::watch_with::<M>()` arms it; `drain_watched::<M>()` returns, sorted and deduplicated, every entity carrying `M` that was handed out mutably (`get_mut`/`component_mut` on a present component, `insert`, a `remove` that returns a value, a `&mut` query yield, a `par_for_each` row) or whose `M` was inserted/removed, plus despawned ones (cascade included). **Drain contract:** each id is alive (re-read it; it may have lost `M`, so evict) or dead (evict); consumers never need a reset signal, restores included (the old ids come back). A drained ancestor expands to its watched `Parent` descendants. Never hashed, snapshotted or replayed; unwatched entities cost one bit test per mutable access and an unarmed world costs nothing per query row. `par_for_each` journals from its sequential join (thread-count-invariant; a caught panic still journals every joined row). Hooks sit after `ensure_access`, so a stale id or `L0028` journals nothing. No `unwatch`.
- **Spawn has no batch twin.** LIFO reuse lands mid-column, so a component insert into a recycled slot is O(rows) in that column. No world has measured it; `remove_many` is the shape to mirror when a profile names it.
- **Spawn rank (0043).** `EntityAllocator::alloc` stamps each occupancy with a monotonic `u64` (`next_rank`), so relative rank is spawn-call order whatever slots the LIFO free list hands out; a reused slot gets a fresh, later rank. `World::spawn_rank(id)`, `spawn_order()` (live sim slots in rank order, `None` when that is slot order) and `restore_spawn_order(&[u32])` (a permutation of the live slots, `L0025` otherwise; ranks rebase to `0..n`). `restore_entities`/`restore_allocator` default to slot order. Ranks are never hashed and are excluded from `Slot`/`EntityAllocator` equality, so `allocator_state()` stays `None` for a reducible allocator and clean snapshots stay byte-identical. Sim range only. Consumers that must not depend on slot values (physics) order by rank.
- **Stale handles are errors, not hits.** Every method taking an `EntityId` calls `ensure_alive` first and returns an `L0002` diagnostic for a dead handle. Generations bump on *free*, so a despawned handle can never match again even before slot reuse.
- **System names are unique.** Recording, query drift, profiling and `L4007` key by name. `Schedule::add_system*` panics with `L4018` on a duplicate; `EngineBuilder::build*` returns `Err(L4018)` naming both registration sites, and also refuses the names the physics systems install under (`physics_write`/`physics_step`/`physics_mirror`).
- **Restore constructors validate.** `World::restore_entities` and `restore_allocator` return `L0025` for a duplicate or out-of-range free-list index, a duplicate live slot index, or a slot count past `MAX_ENTITY_SLOTS` (2^24) — they neither panic nor merge — and validate before clearing (and, for the limit, before allocating), so a refused call leaves the world intact. `engine.restore` additionally checks the allocator's live slots against the snapshot's `entities` list (`L3041`).
- **Entity limit: `MAX_ENTITY_SLOTS` = 2^24.** Slots are reused, so the count is the peak of *concurrent* live entities (the largest measured world holds ~3k), not the total spawned. `World::spawn` panics when every slot is live, and every restore refuses a snapshot past it (`L0025`) — an engine-written snapshot is therefore never refused.
- `#![forbid(unsafe_code)]` — the whole ECS is sorted dense columns; no archetype tables, no pointer tricks. Even the parallel path is safe: disjoint `&mut` chunks come from progressive `split_at_mut`.
- **Unregistered component types are invisible** to the world hash, snapshots and saves. `World::component_types` lists every non-empty storage; the engine reports one with no registry entry as `L0027` (warning, once per type per session), it does not refuse it.
- **The presentation boundary is the world's, not a lint.** Phase checks sit after the access recorder and before `ensure_alive` (a crossing is `L0028`, never `L0002`); every refused accessor also queues the `L0028` in `World::crossings` (deduped by `system/op/Component`, never hashed or snapshotted) for `take_boundary_crossings`; only the `Schedule` sets the phase, and `run_guarded` restores `Host` before it writes `SystemPanic`. `World::entities()` stays sim-only so the hash, snapshots and bisection never see presentation state.
- Registration is append-only and rejects duplicates (`L0005`, same name or same Rust type) because registry order is load-bearing.

## Dataflow

**Components.** `Component` is a marker trait with a blanket impl: any
`'static + Send + Sync + Serialize + DeserializeOwned + JsonSchema` type qualifies. You never write
`impl Component` — derive the three traits.

**World.** `World { entities: EntityAllocator, storages: BTreeMap<TypeId, Box<dyn AnyStorage>>,
resources: BTreeMap<TypeId, Box<dyn Any + Send + Sync>> }`. **One sim thread solely owns it** — never
an `Arc<Mutex<World>>` — and every read and write flows through that owner. It is nevertheless
`Send + Sync` (statically asserted in `world.rs`), and the `Sync` exists for exactly one purpose: so a
`par_for_each` chunk can hold `&World` for shared reads inside one system call. The only interior
mutability is the access recorder's `Mutex`, which carries no sim state. `entities()` iterates live
entities in ascending slot-index order — the canonical enumeration order for whole-world serialization
and `WorldHash`. `has<T>` is a total predicate (false for dead handles), so it works as a cheap guard.

## Spokes

- `ludion devdocs ecs-queries` — joins (`query`, `query_ids`, `Query::shape()`), **the parallel-query region model** (`par_for_each`, `par_join_fold`, `L0026`), the opt-in access recorder.
- `ludion devdocs ecs-schedule` — `Registry`, `Schedule` and its observation entry points, the `catch_unwind` boundary (`SystemPanic`/`L4007`), `Parent` hierarchy and cascade, presentation entities and the phase guard (`L0028`).
- `ludion devdocs ecs-numeric-ranges` — legal `minimum`/`maximum` vs the inert `x-ui-range` slider hint.

## Verify

```
cargo test -p ludion-ecs                      # conformance suite + unit tests + doctests
cargo test -p ludion-ecs --test par_query     # the 1/2/8-thread parallel-join contract
cargo clippy -p ludion-ecs -- -D warnings     # strict sim-side denylist
```

Conformance coverage lives in `crates/ludion-ecs/tests/conformance.rs`: generational safety (a stale
id is an error, not a recycled hit), LIFO free-list order, ascending query/storage iteration order,
registry order, batch-despawn equivalence; `tests/hierarchy.rs` pins the cascade (order, free-list
reuse, `despawn_many` ≡ loop on random forests, stale/cycle termination, the presentation rules).

## Gotchas

- `Parent` hand-writes its `JsonSchema` impl because `EntityId` lives in ludion-core, which has no schemars dependency. Any component wrapping a core type needs the same treatment; keep the hand-written schema matching the serde shape exactly. `AudioEmitter.category` is the second case — a `#[schemars(schema_with = …)]` field attribute rather than a whole impl, since `AudioCategory` is a field type, not the component.
- `world.query` needs `&mut self` even for `(&T,)` — a read-only query in a context holding `&World` will not compile; restructure to take `&mut World`, or use `par_join_fold`, the one join that takes `&self`.
- Inside a `par_for_each` region, `ctx.world.get::<T>()` for a `T` the query took `&mut` is an `L0026` error (and `has`, `par_join_fold` and `entity_indices_with` over it panic), not the live value. It is the region model working as designed; a system that needs cross-entity reads of a written component is not a `par_for_each` candidate. A caller that swallows the `Err` (`.ok().flatten()`) reads it as absent again.
- `read_column` is for engine read walks over many ids (extract); it skips the query recorder like `get_unrecorded`, so a sim system must not use it — the drift gate would not see the access.
- `insert` on an entity that already has `T` silently replaces and returns the old value — it is not an error.
- Resource methods require only `T: 'static` — resources are not `Component`s and never serialize; anything that must survive a snapshot belongs in a component or is handled explicitly (the RNG resource is snapshot-serialized by the sim layer, not by `World`).
- `EntityAllocator` panics if the u32 index space is exhausted; generation uses `wrapping_add` on free.
- The unwind boundary needs unwinding: under `panic = "abort"` nothing is caught and a panicking system kills the process as before. No workspace profile sets it; if one ever does, `L4007` silently stops firing.
- Query trait machinery (`Query`, `Join*`, `Column`) is `#[doc(hidden)]` but public because it appears in `World::query`'s signature — do not treat it as stable API.
