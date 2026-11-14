---
description: ludion-ecs queries: joins, the parallel region model, opt-in access recording
---

# ludion-ecs: queries

**Scope:** Changing `query.rs`, `par_query.rs` or the access recorder in `world.rs`. Hub: `ludion devdocs ludion-ecs`; the thread-count contract is `ludion devdocs sim-parallelism`.

## Rules

### Queries

**Queries.** `world.query::<(&Transform, &mut Velocity)>()` yields one tuple per entity holding all
components, in ascending entity index, arity 1–4. It takes `&mut World` even for read-only tuples —
disjoint mutable storage borrows are how mutability is sound without `unsafe` — and naming the same
component type twice in one tuple panics, since that would alias one storage. The join gallops
(exponential, then binary search) past non-matching keys, so it costs about its smallest column's
length times a log factor, whatever the tuple order. `query_ids` is the id-yielding variant: same
join, same rows, same order, each row `(EntityId, Q::Item)`. Reach for it when the row needs identity
(deferred despawn, a spatial key, a cross-entity reference). It is the **same shape** as `query` and
records identically, so the drift verifier and `engine.describe` do not distinguish the two; the live
generation is read from the allocator borrowed disjointly from the storages, with no `unsafe` and no
index→id snapshot.

`QueryParam` also carries the lifetime-free facts a *declaration* needs — `type_id()`, `type_name()`
and `const MUTABLE` (`&T` = read, `&mut T` = write) — which `Query::shape()` aggregates without
building an iterator or borrowing a world. That is what lets `ludion-engine`'s
`EngineBuilder::query::<(&A, &mut B)>()` record a system's declared queries at build time.

## Dataflow

### Parallel queries: the region model

`world.par_for_each::<(&mut Position, &Velocity), _>(chunk_rows, |ctx, entity, (pos, vel)| { … })`
runs the same join across `ludion_core::par` chunks and returns one `R` per row **in row order**;
`world.par_join_fold::<(&A, &B), _, _>(chunk_rows, map, init, fold)` is its read-only, `&self`
counterpart, mapping each chunk to one value and folding those in ascending chunk order. Arity 1–4
for `par_for_each`, 1–4 reads only for `par_join_fold`.

The mechanism is a **region**. The join runs sequentially first, producing a fixed row list identical
to `query`'s — the same rows in the same ascending entity index. Each `&mut` column's storage is then
lifted **out of the world** and its dense value column split into disjoint `&mut` sub-slices by
progressive `split_at_mut`: columns are ascending, so a chunk's rows occupy one contiguous position
range per column, and no `unsafe` is needed. Each `&` column stays in the world. Every closure gets a
`ParCtx` carrying `&World` (hence `World: Sync`) plus `ctx.in_join(entity)`, a binary search over the
joined key list for "does that entity match this same query?". The sim thread still solely owns the
world, no `&World` outlives the call, and `Arc<Mutex<World>>` remains banned.

**The rule to internalize: inside the region, the query's `&mut` columns are absent from
`ctx.world`.** A `ctx.world.get::<Position>(other)` inside a `(&mut Position, …)` region returns an
`L0026` error, and `has` panics with the same text — a lifted column is never answered as "absent".
That is the price of disjoint `&mut` slices in safe code, not a bug.
Everything else — the query's own `&` columns, unnamed components, resources — reads normally, access
recording included (the recorder's lock is what lets a worker chunk file its reads under the running
system). A chunk that panics gets its columns put back before the unwind resumes, so the `Schedule`'s
system-boundary catch still hands the host a structurally whole world.

`&mut` yields — sequential queries and `par_for_each` rows (journaled from the sequential join) — feed the opt-in write journal (`ludion devdocs ludion-ecs`, "Write journal").

Determinism: results are `to_bits`-identical at 1, 2 and 8 threads, gated in `tests/par_query.rs`
against the sequential `query`. `par_for_each` performs no reduction, so it is `chunk_rows`-invariant
as well; `par_join_fold` deliberately is not (a different grouping is a different float sum), matching
`ordered_reduce`. **Chunk size is part of the determinism contract** — changing a call site's
`chunk_rows` is a hash-moving edit.

**Reach for `par_for_each` when** the system is genuinely element-wise (each row writes only its own
components), the row count is large, and the per-row cost is worth the chunk dispatch — the S1 profile
in `Plan/deterministic-job-system.md` names the candidates, not a guess. **Stay with a plain `query`
loop when** the loop is small, mutates shared state (a resource, a spatial index, an accumulator),
needs `&mut World` mid-loop (spawn/despawn — collect the ids with `query_ids` and mutate after the loop), or needs to read a component the
query writes on *another* entity: that read cannot see through the region, and restructuring it is a
design change, not a translation. Draws inside a chunk must come from
`DeterministicRng::derived`/`derived_salted`, never the master resource
(`ludion devdocs sim-parallelism`).

### Access recording (opt-in)

`World` carries an off-by-default recorder: `begin_query_recording()` arms it,
`take_query_recording() -> Option<QueryObservations>` drains it. While armed it observes a system's
**full** component read/write set, not just its typed queries: every `query`/`query_ids` records its
`Q::shape()`, and every per-entity accessor records the one-component shape it names — `get` a read,
`get_mut`/`insert`/`remove` a write. That closes the gap the `&mut self` borrow model opens: systems
snapshot a query into a `Vec` and then reach for components per entity mid-loop, and those accesses
used to be invisible. `has` records nothing (it reads presence, not a value), nor do `spawn`/`despawn`
or the resource accessors. **A missed access still records** — a `get` on a dead handle or on an
entity without the component records the same shape as a hit, because the observation is about the
system's access *intent*, not whether this run's data satisfied it.

A system whose only touch of `T` is `has::<T>` is therefore outside `L1126`. This is deliberate: recording it would make every guard a declared shape, including guards inside engine helpers (`lane`, `camera_offset`, `hierarchy`) that would be charged to the calling game system.

Attribution comes from the `Schedule`: it tags each system with `set_recording_system` before running
it and calls `clear_recording_system` once the tick's systems are done, so host-side component traffic
(scene loading, the world hash, extract, command application) lands in no system's set. A
non-recording sim pays only one `Option` check per recorded access — the discriminant is the whole
cost on `get`, the sim's hottest path. Recording never branches sim logic and never enters the
`WorldHash`, a snapshot, or a replay: it purely observes. It is the one piece of interior mutability
in `World` — a `Mutex` around the recording state alone, which is what lets the `&self` accessors
record themselves, from a `par_for_each` worker chunk as readily as from the owning thread; concurrent
chunk reads file into a *set*, so arrival order is not observable.

## Verify

```
cargo test -p ludion-ecs
```
