---
description: Determinism enforcement: clippy denylist tiers, denylist canary, hash harness, thread-identity gate
---

# determinism-enforcement

**Scope:** how the rules in `ludion devdocs determinism` (the hub: contract, layout, gotchas) are enforced and verified.

## Dataflow

**Enforcement is per-crate `clippy.toml`, not convention.** clippy resolves the `clippy.toml` nearest
the crate being checked, so each crate carries its own list — generated, not hand-kept:
`scripts/clippy-denylist.mjs` holds the rule groups and three tiers, and each file is its header
comment plus the generated body. A strict/sim file declares `disallowed-types`
(`std::collections::HashMap`/`HashSet` → `StableMap`/`StableSet`) and `disallowed-methods`
(`Instant::now`/`SystemTime::now` and their `elapsed` → `SimTime`/`FrameIndex`; the `f32`/`f64`
transcendental methods → `ludion_core::math`, each reason naming the wrapper; `powi` → multiply
out; the backend-dependent glam `Quat`/`Vec4` methods, `Vec3A::normalize` and `Mat4::{inverse, determinant}` → component-wise fixed-order code (`ludion_core::math::mat4_*`); and, in
the `sim` tier only, `std::thread::spawn`/`scope` and `thread::Builder::spawn`/`spawn_scoped` →
`ludion_core::par`). Tiers: **`sim`** = all of it, **`strict`** = all but the thread ban,
**`permissive`** = no lists. Edit the script and run `node scripts/clippy-denylist.mjs
--write` (`check.mjs --fix` does too); `lists-in-step.mjs` fails on drift and on an unclassified crate. CI runs `cargo clippy --workspace --all-targets --all-features -- -D warnings` on
both OSes, so a violation is a build failure — and the **denylist-canary** job proves the mechanism
itself works: `ci/scripts/denylist-canary.mjs` lints two scratch crates (four probes) under `target/denylist-canary/` — each carrying a copy of a real crate's
`clippy.toml` — with a `HashMap` use and an `x.sin()` (the `strict` tier, `ludion-core`) and an `std::thread::spawn` (the `sim` tier, `ludion-ecs`) and every banned glam method, `Vec3A::normalize` and `Mat4::{inverse, determinant}` included (a second `sim` probe, with glam as its one dependency), and asserts clippy *rejects* each, naming the banned path **and** the
denylist's reason, so a silent denylist regression fails CI loudly. It never writes a tracked file.

The mechanical denylist covers rules 1, 2 and 4 and rule 5's thread ban. The thread entries are on the
**pure-sim** crates only (the `sim` tier) — `ludion-ecs`, `ludion-sim`, `ludion-spatial`, `ludion-scene`,
`ludion-physics`, `ludion-mesh-edit`, `ludion-sculpt`, `ludion-sprite`, `ludion-font`, and every game crate — with two deliberate carve-outs: **`ludion-core` owns `par`**, so it cannot ban the primitive
it implements, and the **host-side** crates (`ludion-engine`, `ludion`, `ludion-cli`, `ludion-assets`,
`ludion-server`, `ludion-window`, …) have legitimate thread seams — the live sim thread, the dev
supervisor, frame-stream pumps. A test that drives two whole engines side by side is orchestration,
not sim work: vigil's `tests/replay.rs` (private, Lore-only) opts out per use with
`#[allow(clippy::disallowed_methods)]` and a one-line reason, which is the sanctioned escape.

Rule 3 holds structurally rather than mechanically (no `rand`/`rayon` in any manifest) — check
manifests before adding such a dependency, and extend the lists when the surface appears. Rule 4's
list covers the inherent methods on `f32`/`f64` only; glam's own trig is covered by its `libm`
feature, and the renderer's trig (permissive tier) stays on `std` because it never reaches a hash.

### Derived streams and deterministic parallelism

Two subjects with one shape — how a system draws random numbers and spreads its own per-entity work
without coupling either to execution order — live in `ludion devdocs sim-parallelism`: the two
sanctioned `DeterministicRng::derived`/`derived_salted` constructors and when to reach for each, the
four properties every `par` function preserves, and why the parallelism is on by default. The ECS
region model those entry points open is `ludion devdocs ludion-ecs`.

**Verification is the three-layer hash harness**, built on `WorldHash` (canonical world serialization
→ XXH3, see `ludion devdocs ludion-sim`):

1. **In-process double run** — same seed + script twice, compare per-frame hash sequences
   (`crates/ludion-sim/tests/determinism.rs`); catches per-instance `RandomState` leaks.
2. **Cross-process** — two invocations of `ludion sim-hash --scene s --script i --frames 600` compare
   final hashes (`crates/ludion-cli/tests/sim_hash.rs`); catches ASLR/env-dependent nondeterminism.
3. **Replay round-trip** — record a session (`ludion sim-hash --record`, or any
   `Engine::set_recording` run), re-drive it with `ludion replay`, compare hash sequences
   (`crates/ludion-cli/tests/replay.rs`).

The hash tells NaN, `+inf`, `-inf` and `None` apart and the first one to meet a non-finite float
warns with `L4024`, so a NaN in sim state is a visible mismatch and a named component.

On the image side, PNG output is byte-deterministic per adapter (the `png` encoder writes no
timestamps; timing instrumentation runs as a separate render so it cannot perturb image bytes), but
the *gate* is SSIM.

## Verify

```
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p ludion-sim
cargo nextest run -p ludion-cli --test it -E 'test(/^sim_hash::/)'
cargo run -p ludion-cli -- sim-hash --scene crates/ludion-cli/tests/fixtures/good.yaml --script crates/ludion-cli/tests/fixtures/script.json --frames 600
cargo nextest run -E 'binary(thread_identity)' --run-ignored ignored-only
```

Run the `sim-hash` command twice and diff the output — identical 16-hex-digit hashes, or determinism
is broken. To prove the denylist bites locally, add
`let _m = std::collections::HashMap::<u8, u8>::new();` to any strict crate and watch clippy fail.

The last command is the 1/2/8-thread hash-identity gate. Every test it selects is `#[ignore]`d (a full
`run.lrp` replay driven three times, hashed every frame, costs minutes), so it rides
`node scripts/check.mjs --full` / CI rather than the default lane. It has live subjects: vigil's (private, Lore-only)
`enemy_ai` runs one `World::par_for_each` region, and its chunk size is a tuned, hash-bearing constant
(`ENEMY_AI_CHUNK_ROWS`). `move_projectiles` is the counterexample — it holds a `par_for_each` call
shape with `MOVE_PROJECTILES_CHUNK_ROWS = usize::MAX`, i.e. one chunk, i.e. sequential at any worker
count, because the bolt hit-test lost to the sequential path at every chunk size and thread count
measured on both pinned platforms.
