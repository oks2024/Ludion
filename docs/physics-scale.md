---
description: How many physics bodies the solver is gated for, the horde shape, motor batches, and L4034
---

# Physics scale

**Scope:** how big a scene the physics tier is measured for, what is only gated for determinism, and the warning past the line. Defers to `ludion docs physics` (bodies), `ludion docs motor` (character moves) and `ludion devdocs physics-pin` (why the numbers are pinned).

## The two gated shapes

| scene | shape | gated for |
| --- | --- | --- |
| `examples/gallery/physics-stress.yaml` | 512 rigid bodies | a pinned `physics_step` budget on the RTX 3090 (`budgets/pinned-rtx3090-physics-stress.toml`), trace, replay, restore |
| `examples/gallery/physics-horde.yaml` | 500 `CharacterMotor`s, 120 eleven-part `SkinBone` ragdolls (24 pinned to the walls by runtime joints), 5,000 pool projectiles | determinism only: committed trace, replay, snapshot restore, 1/2/8-thread identity. **no pinned budget** |

512 non-static bodies is the **gated scale**; the single-threaded solver's measured wall is about 2,000 rigid bodies. Past 512, the step cost is unmeasured on the pinned boxes: build a stress scene at your game's shape and measure it with `ludion profile` against a budget on each box.

## Indicative figures

Unpinned M5, 2026-11-09, medians of three runs — **indicative, not authoritative** (`Plan/physics.md` "Horde bench" owns them). Horde scene, 1,865 bodies, 1,820 awake:

- `physics_step` 1.78 ms mean (solver about half, narrow phase about a quarter).
- 500 serial `move_character` calls: 3.76 ms, about twice the whole step.
- The same 500 moves through `move_characters`: 3.72 ms at 1 thread, 1.22 ms at 8.
- `projectiles_step` for 5,000 shots: 0.68 ms at 1 thread, 0.26 ms at 8.

Reproduce: `cargo test -p ludion-engine --release --features physics-profiler --test it -- horde_bench --ignored --nocapture`. It checks `budgets/pinned-<box>-physics-horde.toml` when one exists for the platform.

## Crowds of motors

Motor casts dominate a character crowd, not contacts. Move many motors per tick with `move_characters` (`ludion docs motor`, "Batch moves"): the casts run in parallel and the result does not depend on the thread count.

**Pair prefilter.** Kinematic–kinematic and kinematic–static pairs never reach the contact graph, so a motor's capsule adds no contact pairs and no narrow-phase work (it still costs broad-phase work and its own casts). `PhysicsWorld::contact_pair_census` counts tracked and touching pairs per body-kind pairing: read it to see where pairs come from before blaming the solver.

## L4034

`L4034` is a **warning**: the scene has more than 512 non-static solver bodies. It counts rigid and kinematic bodies, character motors included, never static ones; a 2D `circle`/`aabb` collider is not a solver body.

- `ludion check-scene` (and the wire `scene.check`, a game's `--check-scene`) counts the authored bodies and reports at `/physics`.
- At runtime it latches once per session on the live count, printed to stderr and sent as a `Log` event, so `ludion profile`, `--profile` and a live capture show it. A horde that spawns ragdolls at runtime trips it live while its authored scene stays clean.
- Nothing is dropped and the exit code does not change.

## Next

- `ludion docs motor` — `move_characters` and the carry rule.
- `ludion docs physics-joints` — the ragdoll joints the horde runs.
- `ludion docs projectiles` — the pooled shots swept against physics.
