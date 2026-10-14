---
description: How a sim system draws random numbers and spreads per-entity work without moving a hash
---

# Deterministic streams and parallelism

**Scope:** The two things a sim system does that could couple its result to execution order — drawing
random numbers and spreading its own per-entity work across threads — and the constructions that stop
them doing so. The contract these serve is `ludion devdocs determinism`; the ECS region model the
entry points open is `ludion devdocs ecs-queries`; `Plan/deterministic-job-system.md` is the plan of
record.

## Derived streams

Rule 3's "one RNG" is about one *seed* and one master resource, not one generator object. Drawing from
the master resource makes a value depend on how many draws ran before it that tick, which couples
every drawing system to every other system's order. Two constructors break that coupling without
adding a second source of randomness:

```rust
// One entity's draws on one frame.
let mut rng = DeterministicRng::derived(world_seed, ctx.frame, entity);
// A draw that belongs to no single entity — a chunk, a wave, a spawn slot.
let mut rng = DeterministicRng::derived_salted(world_seed, ctx.frame, slot_index);
```

`world_seed` is the `WorldSeed` resource, which the driver inserts next to the `DeterministicRng` on
every world it builds (`SimDriver::new`/`restore`), so it is available wherever the RNG is. The stream
is a pure function of the arguments — same inputs, same sequence, regardless of what else drew. Both
halves of the `EntityId` participate because storage slots recycle: keying on the index alone would
hand a fresh entity its predecessor's stream.

**Derive when** the draw is per-entity or per-chunk and could plausibly run inside a parallel chunk;
or when the draw's result should not change because an unrelated system started or stopped drawing.
**The master resource is fine when** the draw is inherently serial and global — a single per-frame
decision, a one-off during scene setup — where "the Nth draw of the run" is the honest identity of the
value.

Never hand-seed from ad-hoc numbers (`DeterministicRng::new(frame * 31 + id)` and friends). The two
constructors above are the only sanctioned derivation; they fold their inputs through a splitmix64
finalizer and mask to the 63 bits PCG can actually distinguish (`new_with_stream` shifts the top bit
out when it forms the increment, so an unmasked 64-bit hash silently collides on bit 63).

Switching a call site from the master resource to a derived stream **changes the draw sequence**, so it
moves `WorldHash` and re-records the affected `.lrp` — a deliberate, batched event, never a drive-by
cleanup.

## Deterministic parallelism

Rule 5's "sequential" is about the *schedule*, not about cores. `ludion_core::par` is the one
sanctioned way a system spreads its own per-entity work across threads;
`Plan/deterministic-job-system.md` is the plan of record for why it exists.

Parallelism is deterministic iff the observable result is independent of thread interleaving. Four
properties make that true, and every `par` function preserves them:

1. **Static partitioning** — chunk boundaries come from `chunk_size` alone. Which worker claims a
   chunk is dynamic; where chunks start and end is not.
2. **Disjoint writes** — `par_chunks` hands each chunk an exclusive `&mut` sub-slice. A closure that
   writes outside its own slice is the one violation the types cannot catch, and it is a P0 bug.
3. **Ordered reductions** — float addition is not associative, so partial results must combine in
   ascending chunk order regardless of completion order. `par_map_chunks` returns per-chunk results in
   chunk order; `ordered_reduce` folds them sequentially in that order. Never accumulate into shared
   state as chunks land.
4. **Deterministic panics** — every chunk runs and is joined, then the *lowest-index* panicking
   chunk's payload is the one that propagates, so a failing frame fails the same way every time.
   Inside a system's unwind boundary its panic *site* travels with it (`ludion_core::panic_capture`).

```rust
// Disjoint mutation: chunk index + offset is the entity's position, so nothing is shared.
par::par_chunks(&mut velocities, 256, |chunk_index, chunk| { /* … */ });
// Ordered aggregate: per-chunk sums, folded in ascending chunk order.
let total = par::ordered_reduce(&forces, 256, |_, c: &[f32]| c.iter().sum::<f32>(), 0.0f32, |a, b| a + b);
```

The worker count is a process-global default (`par::set_threads`) with a thread-local scoped override
(`par::with_threads`), so concurrent caller threads can hold different counts; an effective count of 1
runs inline with no spawns, so the sequential path is the same code, not a second implementation.

**The parallelism is on by default, and that is a determinism decision, not a performance one.** The
process default resolves itself the first time it is asked: `LUDION_THREADS` if set (a positive
integer or `auto`), else available parallelism capped at `AUTO_THREAD_CAP`. Every process therefore
runs the parallel path — a game, `ludion serve`, an example, and the whole test suite alike. The
reasoning is that an invariance claim is worth what routinely runs against it: while the default was
`1`, the only thing exercising `par` was the `#[ignore]`d identity gate, so a defect in it was
invisible on every machine until someone went looking. Now an ordinary `cargo test` is a parallel run
and a `par` regression fails a committed replay the same day. None of this can move a hash — if
flipping the default ever changes a result, that is the defect the default was turned on to find.

## Over ECS queries: the region model

`ludion devdocs ecs-queries` owns the region model — how a `&mut` column is lifted out of the world and
split into disjoint chunks, why `ctx.world.get::<T>()` for a written component is an `L0026` error
inside a region (`has` panics), and when to reach for `par_for_each` rather than a plain query loop. The determinism
consequence is the part that binds here: draws inside a chunk must come from
`DeterministicRng::derived`/`derived_salted`, never the master resource, because chunks cannot share
one generator.

## Verify

```
cargo test -p ludion-ecs --test par_query          # 1/2/8-thread and chunk-size invariance vs sequential query
cargo test -p ludion-core                          # par_chunks / ordered_reduce / panic ordering
cargo nextest run -E 'binary(thread_identity)' --run-ignored ignored-only
```

The last is the 1/2/8-thread hash-identity gate over the committed replay corpus, `#[ignore]`d and run
via `node scripts/check.mjs --full` or CI. Any change to a `chunk_size` at a call site, or to a draw's
stream, is a hash-moving edit: re-record the affected `.lrp` in the same commit.
