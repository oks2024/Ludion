---
description: Physics's pinned rapier3d features, their gate, the body-count ceiling, the pin-upgrade ritual
---

# Physics pin, feature gate, body-count ceiling and upgrade ritual

**Scope:** Why `ludion-physics`'s `rapier3d` feature set is exactly what it is, and the mechanical
gate that verifies the resolve graph never drifts from it. Defers to `ludion devdocs ludion-physics`
for everything else about the crate.

`rapier3d` is pinned with `=` in `[workspace.dependencies]` (`=0.35.3` today, vendored and patched under `vendor/rapier3d/`) — a pin move is a
hash-moving event the pin-upgrade ritual (`Plan/physics.md` W4) gates, never a routine bump.
`default-features = false`, features `dim3 f32 std enhanced-determinism parallel serde-serialize`:
`dim3`/`f32` are the only shape/scalar type rapier builds for us; `std` pulls `alloc`, without which
rapier does not build the collections its solver needs; `enhanced-determinism` selects `IndexMap`/
`IndexSet` with a fixed hasher inside the solver; `parallel` runs rapier's internal scheduler on a
pool `ludion-physics` owns, sized `par::threads()` and synced in `new`, `from_bytes` and every step,
never on rayon's global pool (the two features compile together in 0.35.3; the one `compile_error!`
is `simd8` + `enhanced-determinism`); `serde-serialize` is what the snapshot codec (`bincode`,
pinned `"1"`) needs. Never a `simd-*` feature.

`scripts/physics-features.mjs` (wired into `check.mjs`'s default lane and `ci.yml`) asserts over
`cargo metadata --format-version 1 --locked`: exactly one `rapier3d` and one `parry3d` in the
resolve graph; `rapier3d` resolves to exactly `alloc dim3 enhanced-determinism f32 parallel
serde-serialize std`; `parry3d` to exactly its recorded set, `enhanced-determinism` and `parallel`
included; `rayon`/`rayon-core` reachable from this crate's normal dependency closure only through the
named carve-out, the `rapier3d -> rayon` and `parry3d -> rayon` edges (both must exist), with the
reaching path printed when anything else reaches them. The carve-out is documented in
`Plan/deterministic-job-system.md`. It exists because clippy's denylist bans a type path in *our*
code and has never seen inside a dependency, and cargo unifies features workspace-wide — a `simd-*`
feature or a dropped `parallel` anywhere would silently change every build.

**`hashbrown` is deliberately not gated.** `std` pulls `alloc`, which turns on `parry3d/alloc`,
which is what makes `hashbrown` resolve — unavoidable here, and not physics-specific: the root
`Cargo.toml`'s `serde_json` `preserve_order` feature already pulls `indexmap` (and so `hashbrown`)
into every crate. Parry3d's own use is an explicitly fixed-seed hasher, but a hasher *choice* is a
source fact `cargo metadata` cannot see. What stands behind it is the per-scene cross-arch trace
gate (`Plan/physics.md` gate 1): a non-deterministic container reachable from the solver would show
up as a diverging hash on the second pinned machine.

## The vendored patches

rapier3d lives in `vendor/rapier3d/`: the 0.35.3 release, byte-identical except four files: `joint_constraint_helper.rs` (the angular-limit and motor jacobians use the exact rate of the angle they constrain), `collider.rs` and `collider_components.rs` (`set_friction`/`set_restitution` flag `ColliderChanges::MATERIAL` so a material edit on a resting pair is re-read), and `Cargo.toml` (the `parry3d` path, and an empty `[workspace]` so the crate is its own root for the importer fingerprint's `cargo metadata --no-deps`). `vendor/rapier3d/LUDION-PATCH.md` says what and why; `LUDION-PATCH.diff` is the change. Three things keep it in the build: the `path = "vendor/rapier3d"` on the workspace `rapier3d` entry (a path dependency, not a root `[patch.crates-io]`, which cargo ignores for a game that depends on Ludion by path), `exclude = ["vendor/rapier3d"]` under `[workspace]` (so clippy, fmt and nextest never run over it), and the features gate, which fails if rapier3d is not a path package under `vendor/`, in the workspace and in a scratch path consumer of `ludion-physics`.
parry3d lives in `vendor/parry3d/` the same way: the 0.30.2 release, byte-identical except one function, the dim3 heightfield ray's start cell (`vendor/parry3d/LUDION-PATCH.md`, `LUDION-PATCH.diff`). It is kept in the build by `path = "../parry3d"` on the `parry3d` dependency in `vendor/rapier3d/Cargo.toml` (part of rapier's patch), `exclude = ["vendor/parry3d"]` and the features gate's path-package assertion; a path dependency cannot silently fall back to the registry crate.

Path dependencies are a temporary mechanism: Ludion is unpublished, and a crate published to crates.io cannot carry a path dependency on an unpublished one. Revisit before any crates.io publish.

A pin move means porting both `LUDION-PATCH.diff`s onto the new releases first; the ritual refuses `--to` until then.

## The body-count ceiling

A scene's rigid-body count has a ceiling. The solver runs on `par::threads()` workers (the pool
`ludion-physics` owns). The wall below was measured single-threaded, before `parallel`, and has not
been re-measured. The **gated scale** is 512 rigid bodies (`examples/gallery/physics-stress.yaml`):
`physics_step` measures ≈1.13 ms median on the RTX 3090, with the ceilings in
`budgets/pinned-rtx3090-physics-stress.toml`. The **measured wall** is ≈2 000 bodies (4.6 ms mean,
9.6 ms p99 of a 16.7 ms tick — W0, one box, one scene). Three things hold it: the TeamCity profile
gate on that budget; `the_stress_pile_holds_the_budgeted_rigid_body_count`, which pins the scale;
and the stress replay's size ceiling (`REPLAY_MAX_BYTES`, `tests/physics_stress.rs`), which watches a
keyframe's physics blob. When content needs more bodies: a new stress scene and a budget *measured*
on each box — never a raised number.

The second gated shape is `examples/gallery/physics-horde.yaml`: 500 `CharacterMotor`s, 120
eleven-part ragdolls (1,320 bodies, 1,200 impulse joints, 24 pinned to the walls) and 5,000 pool
shots, gated at 1/2/8 threads for trace, replay and snapshot restore (`tests/physics_horde.rs`).
Its budgets `budgets/pinned-{apple-m4,rtx3090}-physics-horde.toml` are unmeasured; `horde_bench`
checks them when present. `ludion_physics::GATED_BODIES` (512) drives `L4034`, which warns in
`check-scene` and once live past it; the number rises only when a horde budget is measured on both boxes.

## The pin-upgrade ritual

`node scripts/physics-pin-ritual.mjs [--to <x.y.z>] [--bless-traces]` runs the ritual on demand (not in
`check.mjs` or CI; `--help` prints its header): (1) with `--to`, move the `=` pin, run `cargo update -p
rapier3d --precise`, and print the `Cargo.lock` movement and every rapier-mentioning text file still naming the old
version — fix those by hand, and re-audit the clippy determinism denylist against the
new source; (2) the feature gate, whose failure stops the run; (3–4) the CPU physics gates under nextest
— every `ludion-physics` test, the `ludion-engine` `physics_*` and `ragdoll*` binaries, the
`ludion-cli` `physics` binary, and the ignored `thread_identity` scenes; (5) the re-bless order is
printed, never run: schema snapshot, goldens, budgets, the `.lrp` corpus last.

Verdicts: **PASS** (exit 0), **FAIL** (1: an in-process gate broke or the build did — the pin does not
move; record it, exit trigger (b) of `Plan/physics.md`), **MOVED** (3: the only failures are tests in the
script's `COMMITTED_COMPARISONS` list, which compare against a committed trace or replay recorded by the
previous pin), usage error (2). A test that reads a new committed trace or replay joins that list.

**MOVED protocol.** On the first machine run with `--bless-traces`: it runs the gates unblessed, and only
on MOVED re-records the five committed traces and re-runs the gates (PASS: nothing to bless; FAIL: nothing
blessed). The replay tests then pass — they re-derive from scene, seed and inputs and compare hashes to the
trace, never bytes to the `.lrp` — but `examples/gallery/physics-stress.lrp` and
`examples/gallery/ragdoll.lrp` still hold the old pin's physics blob in their keyframes and are stale until
step 5 re-records them (the run lists them). A blob names its pin (`rapier3d::VERSION`), so a stale keyframe blob is refused at decode (`L3033`); the pin stays out of the replay content hash so the gates can run across the move.
Carry the candidate pin and traces to the second machine and run the script there with no flags — green is
the cross-arch check. Only then step 5.

When a `--to` run ends FAIL the script prints the revert (`--to <previous>`) and never reverts for you;
do not commit `Cargo.toml` or `Cargo.lock` at a candidate pin in the shared checkout.

Moving the pin rewrites `Cargo.toml` and `Cargo.lock` and rebuilds rapier for the whole checkout: clear
every other cargo lane first, and never commit a rehearsal pin. Measured on the RTX 3090 box
(0.35.3 → 0.35.2 → 0.35.3, no hash moved): 15 s warm at the committed pin, about 2 min after a pin move,
dominated by rebuilding rapier and everything above it.

## Verify

```
node scripts/physics-features.mjs
node scripts/physics-pin-ritual.mjs
cargo nextest run -p ludion-engine --test it -E 'test(/^physics_stress::/)'
```
