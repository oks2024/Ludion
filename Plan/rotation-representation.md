# Plan: Rotation and camera representations

> Status: **planned, not started — a decision plan.** Source: the 2026-09-25 code audit, finding J7.
> Current-code facts were read from the tree on 2026-09-26; file:line pointers are of that date, so
> re-grep before editing.

## Trigger

`Transform` stores Euler XYZ degrees, and the physics solver stores quaternions. The mismatch already
caused one High bug: an Euler round trip broke exact-bits edit detection, so every body was re-posed and
woken each tick. It was fixed by a read-back baseline.

The conversions are duplicated across four Rust sites and two TypeScript sites. The camera convention
is written out in five places, and nothing tests them against each other.

Build the consolidation (W1–W2) whenever convenient; it is cheap and changes no output.

Take the storage decision (W3) when **any one** of these happens:

- a second bug of the round-trip class (a value that never settles, a twitch at a pole);
- animation needs to interpolate rotations (slerp on stored Eulers is wrong);
- an import round trip (glTF quaternion → Euler → quaternion) is found to lose a pose that matters;
- gimbal lock bites an editor rotate gesture.

## What exists (2026-09-26)

**`Transform.rotation: [f32; 3]`** holds Euler XYZ in degrees (`crates/ludion-engine/src/components.rs:48-58`).
`PhysicsPose` converts through `ludion_physics`'s libm pair (`:81-104`).

**C1's fix.**

- `mirror` writes the pose, reads it back through the component's own accessors, and rebases the
  baseline on the read-back (`crates/ludion-physics/src/systems.rs:346-378`, `world.rs:1375`).
- The trait contract requires a stable round trip, not a lossless one (`pose.rs:28-33`).
- Regression: `a_tumbled_transform_body_falls_asleep` (`crates/ludion-engine/tests/physics_scene.rs`).
- Comparisons already happen in the component's own space, which is the audit's option (b).

**Euler/quaternion conversions:**

1. `extract.rs:1913` `euler_quat`: glam `Quat::from_euler` with std trig. It serves render, particles
   and `render_view.rs`.
2. `ludion-physics/src/pose.rs:59,94`: libm, both directions. It serves the physics mirror and the
   ragdoll (`ragdoll.rs:311`).
3. `ludion-import/src/convert.rs:23,40`: libm quaternion → Euler. This is the same algorithm as
   `pose.rs:94`, as that file says.
4. `ludion/src/math.rs:342`: libm Euler → quaternion, private. It is a third copy.
5. `model_sheet.rs:2796`: glam `to_euler`.
6. `extract.rs:2440` `part_placements_with` takes the conversion as a function parameter, so render and
   ragdoll can pass different ones.
7. The editor: `editor/app/src/editor/gizmo/camera.ts:99,112` (whose own comment says its branches need
   not match glam) and `orbit.ts:75-110`'s look-at decomposition.

**`ludion_core::math`** wraps libm scalar functions only; it has no rotation conversions. glam is built
without its `libm` feature (root `Cargo.toml:73`). Switching glam to libm is a separate, hash-moving
fix-stage item, and this plan assumes it lands first.

**The camera convention is written five times:**

- `extract.rs:2019` `build_camera`;
- `render_view.rs:113-125`, a duplicate of it;
- `model_sheet.rs:1212`;
- `ludion/src/math.rs:277,303`, a hand-built libm projection;
- `camera.ts:169-203`.

The parity test `math.rs:557` checks `math.rs` only against itself, although `:300` claims it mirrors
`build_camera`.

**Content:** 68 tracked YAML files carry `rotation:` (vigil's scenes, the gallery, import fixtures, the
fuzz corpus).

## Proposed design

### W1 — one home for the conversions

- `ludion_core::math` gains `euler_xyz_deg_to_quat` and `quat_to_euler_xyz_deg`, libm-backed.
- Copies 2–4 are deleted and `extract.rs`'s `euler_quat` calls the core pair.
- `part_placements_with` loses its function parameter, because render and sim now agree.
- The faceted-normal helper the audit names moves with them.
- This is **hash- and golden-moving** only where render used std trig: sub-ulp matrix differences can
  shift a pixel. Pinned goldens are the judge. Expect SSIM-level passes, and re-bless on the reference
  box only if a tile falls below the floor (the M4 half is owed). The sim-side paths
  already use libm, so `WorldHash` should not move. Verify with the 1/2/8-thread gate and the physics
  traces.

### W2 — one camera convention, tested against the renderer

- `build_camera` becomes the single source.
- `render_view.rs` calls it.
- `ludion::math` builds its matrix through the same function, or a libm twin that a test compares
  against `build_camera` element-wise, over a sweep of camera poses, both projections and
  representative extents.
- The editor's `camera.ts` gets a generated fixture (poses → expected matrices, written by a Rust test)
  that a frontend test reads, so the TypeScript and Rust conventions are pinned by one data file.

### W3 — the storage decision

This wave only runs when a trigger fires. There are two options.

**(a) Store a quaternion.**

- `Transform.rotation` becomes `[f32; 4]`, or a tagged form that accepts `euler:` on input.
- It needs a component migration (`crates/ludion-engine/src/snapshot.rs`'s `migrate_component`
  pattern), a rewrite of the 68 content files through the canonical writer, and editor gizmo changes.
- The wire schema hash moves and every `.lrp` is re-recorded last.
- Designers lose hand-editable angles in YAML unless input keeps accepting Euler.

**(b) Keep Euler (today's state, made explicit).**

- Every comparison happens in component space, through the read-back rule `PhysicsPose` already
  follows.
- Document the rule in `dev-docs/ludion-physics.md` and `docs/`'s transform topic.
- Hand-editability stays, and so does gimbal lock in the gizmo.

**Recommendation: (b), until a trigger fires.** C1 is closed under (b), and (a)'s cost is a
schema-moving migration across all content. If a trigger fires, choose (a) with Euler still accepted on
input and quaternion as the canonical output. That form keeps a minimal diff for the authored scenes
only if the writer is taught to preserve the input form, and that must be settled before choosing it.

## Risks

- **W1 moves pixels on both pinned platforms.** Batch it with the next golden re-bless if one is
  queued.
- **Editor drift.** The TypeScript conversions stay hand-written. W2's fixture pins the matrix, not the
  Euler branch choices. Gizmo round trips that pick a different Euler branch than Rust still produce a
  correct pose but a noisy diff. Test `quatToEulerXyzDeg` against the Rust function through the same
  fixture.

## Exit gate

- `node scripts/check.mjs` green per wave; `--full` after W1.
- `rg 'fn .*euler' crates/` finds only `ludion_core::math`.
- W2's parity test fails when a sign is flipped in either `build_camera` or `ludion::math`. Prove it
  once by mutation.
- The W3 decision is recorded under §Rulings with the trigger that forced it.

## Status

W1 shipped 2026-10-01 as `Plan/transform-hierarchy-and-prefabs.md` W0: the libm Euler XYZ↔quat pair,
`quat_mul`/`quat_rotate` and the `Trs`/`Affine3` composition live in `ludion_core::math`
(`math/rotation.rs`, `math/trs.rs`; the f64 pair in `math/rotation_f64.rs`), pinned by
`tests/trs_bits.rs`; the physics, extract, ragdoll, import, clip-eval and facade copies are gone
(`ludion::math` keeps its names). Render moved from glam's closed-form `from_euler` to the
`qx·qy·qz` product: four Windows gallery tiles re-blessed at ulp level, the M4 half owed. No
`WorldHash`, replay, physics trace or budget moved. Left on purpose: `model_sheet.rs`'s look-at
`to_euler` (moving it shifts the exact idol gate), sculpt's Euler→matrix (byte-gated meshes), the
faceted-normal helpers (not a rotation). W2 (camera convention) and W3 (storage) not started.
Shipped earlier, in the 2026-09 fix stage: C1's read-back baseline (option (b) for physics).
