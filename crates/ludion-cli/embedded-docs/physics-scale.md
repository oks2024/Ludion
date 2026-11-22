---
description: How many physics bodies the solver is gated for, the horde shape, motor batches, and L4034
---

# Physics scale

**Scope:** how big a scene the physics tier is measured for, what is only gated for determinism, and the warning past the line. Defers to `ludion docs physics` (bodies), `ludion docs motor` (character moves) and `ludion devdocs physics-pin` (why the numbers are pinned).

## The two gated shapes

| scene | shape | gated for |
| --- | --- | --- |
| `examples/gallery/physics-stress.yaml` | 512 rigid bodies | a pinned `physics_step` budget on each box (`budgets/pinned-rtx3090-physics-stress.toml`, `budgets/pinned-apple-m4-physics-stress.toml`), trace, replay, restore |
| `examples/gallery/physics-horde.yaml` | 500 `CharacterMotor`s, 120 eleven-part `SkinBone` ragdolls (24 pinned to the walls by runtime joints), 5,000 pool projectiles | determinism only: committed trace, replay, snapshot restore, 1/2/8-thread identity. **no pinned budget** |

512 non-static bodies is the **gated scale**; the step runs on the machine's `par::threads()` workers (`LUDION_THREADS` or `--threads`), and its measured wall, taken single-threaded and not re-measured, is about 2,000 rigid bodies. Past 512, the step cost is unmeasured on the pinned boxes: build a stress scene at your game's shape and measure it with `ludion profile` against a budget on each box.

## Indicative figures

Unpinned M5, 2026-11-09, medians of three runs — **indicative, not authoritative** (`Plan/physics.md` "Horde bench" owns them). Horde scene, 1,865 bodies, 1,820 awake:

- `physics_step` 1.78 ms mean (solver about half, narrow phase about a quarter). A spike of the threaded step cut the 8-thread step from 1.83 to 1.06 ms.
- 500 serial `move_character` calls: 3.76 ms, about twice the whole step.
- The same 500 moves through `move_characters`: 3.72 ms at 1 thread, 1.22 ms at 8.
- `projectiles_step` for 5,000 shots: 0.68 ms at 1 thread, 0.26 ms at 8.

Reproduce: a game enables the timers with the `ludion` crate's `physics-profiler` feature. `cargo test -p ludion-engine --release --features physics-profiler --test it -- horde_bench --ignored --nocapture`. It checks `budgets/pinned-<box>-physics-horde.toml` when one exists for the platform. Its table (d) compares the dispersed walk with a converged arm, where every motor walks to one point and the crowd piles up.

## Tick 1 after a load

A scene load or switch builds the physics world and runs its first collision detection at commit, so the one-time settle cost is paid in the load, not in the frame the scene appears in. Tick 1 reuses that work unless a Sim system changed an input physics reads (a body's `Velocity`, `Collider`, pose, a joint) before `physics_write`; then it builds cold as before, with the identical result. `PhysicsWorld::last_step_primed()` tells which happened. A restore never primes.

The settle bench (`cargo test -p ludion-engine --release --features physics-profiler --test it -- settle_bench --ignored --nocapture`) measures a 2,000-body jointed town primed vs cold, and `budgets/pinned-rtx3090-physics-settle.toml` with its twin `budgets/pinned-apple-m4-physics-settle.toml` gates the primed tick 1.

## Crowds of motors

Motor casts dominate a character crowd, not contacts. Move many motors per tick with `move_characters` (`ludion docs motor`, "Batch moves"): the casts run in parallel and the result does not depend on the thread count.

**Pair prefilter.** Kinematic–kinematic and kinematic–static pairs never reach the contact graph, so a motor's capsule adds no contact pairs and no narrow-phase work (it still costs broad-phase work and its own casts). `PhysicsWorld::contact_pair_census` counts tracked and touching pairs per body-kind pairing: read it to see where pairs come from before blaming the solver.

**Converged crowds.** A motor's move walks its neighbourhood once and answers its casts and contact queries from that short list, so the cost no longer multiplies by the number of queries; it is still the colliders near the motor, including ones its layer ignores (each costs a bitmask test, not an exact test), so a crowd piled into one spot costs more per motor than a dispersed one and the cost follows local density. The bench's converged arm measures it.

## L4034

`L4034` is a **warning**: the scene has more than 512 non-static solver bodies. It counts rigid and kinematic bodies, character motors included, never static ones; a 2D `circle`/`aabb` collider is not a solver body.

- `ludion check-scene` (and the wire `scene.check`, a game's `--check-scene`) counts the authored bodies and reports at `/physics`.
- At runtime it latches once per session on the live count, printed to stderr and sent as a `Log` event, so `ludion profile`, `--profile` and a live capture show it. A horde that spawns ragdolls at runtime trips it live while its authored scene stays clean.
- Nothing is dropped and the exit code does not change.

## Next

- `ludion docs motor` — `move_characters` and the carry rule.
- `ludion docs physics-joints` — the ragdoll joints the horde runs.
- `ludion docs projectiles` — the pooled shots swept against physics.
