---
description: ludion-projectiles — the pooled 3D shot/gib crate, its codec, digest and engine hosting
---

# ludion-projectiles

**Scope:** editing `crates/ludion-projectiles` and its engine hosting. Game-facing fields, responses
and drawing are `ludion docs projectiles`; the sweep's physics side is `ludion devdocs ludion-physics`;
hash rules are `ludion devdocs determinism`.

## Layout

- `crates/ludion-projectiles/src/lib.rs` — crate doc: boundary, the response semantics, determinism.
- `src/pool.rs` — `ProjectilePool` SoA columns, generational ids, `ProjectileDesc`, `Response`, `has_ext`.
- `src/step.rs` — the swept step: integrate, sweep (physics `first_hit` + `spatial3d` cast), resolve response, orientation.
- `src/codec.rs` — the byte codec for pool and events (snapshots, replays); decode validates every tag and invariant.
- `src/hash.rs` — `ProjectilePool::digest`, the XXH3 block `WorldHash` folds in.
- `tests/` — `step.rs`, `codec.rs` (legacy bytes pinned), `determinism.rs` (1-vs-N trace, gibs), `alloc.rs`, `bulk_bench.rs` (ignored).
- Engine side: `crates/ludion-engine/src/projectiles.rs` (`spawn_projectile`, `projectiles_step`),
  `projectile_draw.rs` (looks, visuals, L4028/L4033), `extract.rs` (draw items).

## Dataflow

`projectiles_step` takes the `Projectiles` resource out of the world, runs `step(pool, dt, physics,
index, cfg, events)` and puts it back; `step` fans casts out over `ludion_core::par` and applies results
serially in ascending slot order. The resource digest feeds `WorldHash`; `encode`/`decode` ride engine
snapshots (a restore resizes to the build's config). Extract reads the pool and turns each live shot
into a scene item (`ProjectileItem::Mesh` / `Posed` / …) for interpolated presentation.

## Rules

- **The ext bit.** Spin, orientation and per-shot `targets` live behind bit 1 of the parked byte (codec
  v1 and digest `v2` are unchanged). A slot with all three at default writes the legacy bytes, so old
  snapshots, replays and hashes stay valid; they are pinned in `tests/codec.rs`. Writing ext for a
  default slot moves every committed hash.
- **`Settle` is codec and digest response tag 4.** Renumbering breaks every stored pool.
- **Bit compares, not value compares.** `has_ext` and the spin test compare `to_bits`, so `-0.0` is a
  non-default value. A value compare drops it from the digest and breaks decode round-trips.
- **Orientation integrates only at non-bit-zero spin** (`step.rs`, end of the per-shot function), so a
  spin-less shot's orientation never gets renormalised and its bytes stay identical.
- **The step's only root is `ludion_core::math::sqrt`** (quaternion normalise), and it uses no
  transcendental. Any added `sin`/`cos`/`atan2` etc. must go through `ludion_core::math`, or the
  crate's clippy denylist fails.
- **No new `HitOutcome` variant.** Settle reports `Reflected` / `Stopped`; a variant moves the event
  codec and every consumer's exhaustive match.
- **A step at one thread is allocation-free** after warm-up (`tests/alloc.rs`); keep scratch in the
  pool or `StepEvents`.
- Settle parks on `n.y >= floor_cos` (+Y up), not against gravity, because gravity may be zero or
  scaled negative; a rest is final, and only a physics-world contact parks (index targets bounce).

## Gotchas

- `ProjectileItem::Posed` must be counted with `Mesh` when padding `BlendKeys` (`extract.rs`): the
  appended projectile meshes pad the mesh key list, and a miscount leaves it out of step with
  `scene.meshes`, so every mesh blends against the wrong owner.
- `Mesh` and `Voxel` looks both pivot on their asset's bounding-box centre (`projectile_draw.rs`, `bounds_centre`); a zero pivot is skipped.
- A `Mesh`/`Voxel` look is refused at build (`L4033`) unless its file is under the builder's
  `project_root`; benches and tests that draw gibs must set it.
- User-facing fields: `ludion docs projectiles`; do not copy them here.

## Verify

- `cargo nextest run -p ludion-projectiles`
- `cargo test -p ludion-projectiles --release --test bulk_bench -- --ignored --nocapture`
- `cargo test -p ludion-engine --release --test it -- projectiles_extract_bench --ignored --nocapture`
- `cargo nextest run -p ludion-testkit --test gallery projectile_gibs`
