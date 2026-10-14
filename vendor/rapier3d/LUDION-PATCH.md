# rapier3d 0.35.3, modified by Ludion

This is rapier3d 0.35.3 (Apache-2.0, Dimforge) with four files changed, as Apache-2.0 §4(b) requires
stating. Everything else is byte-identical to the crates.io release; the license is `LICENSE-APACHE`.

**Changed:**
- `src/dynamics/solver/joint_constraint/joint_constraint_helper.rs`, the dim3 branches of
  `JointConstraintHelper::limit_angular` and `JointConstraintHelper::motor_angular`.
- `src/geometry/collider.rs` (`set_friction`, `set_restitution`) and
  `src/geometry/collider_components.rs` (`ColliderChanges::MATERIAL`).
- `Cargo.toml`, the `parry3d` dependency gains `path = "../parry3d"`: a consumer never sees a root
  `[patch]`, so this is how the parry3d patch reaches it (and keeps one parry3d in the build).

The exact change is `LUDION-PATCH.diff`.

**Why (joints):** both rows measure an angle from the relative rotation `q = (w, q_i, q_j, q_k)` of the two
joint frames but use the bare frame-1 axis as the jacobian, so on a ball joint swung about several
axes the row's velocity target disagrees with the angle it corrects, and its bias is fed back as
energy. A resting limited ragdoll gained energy from it and never slept.

**Why (material):** contact recycling skips the full pair update while the relative pose barely moves, and
the friction/restitution combine lives in that update, so a material edit on a resting pair was never
read. `set_friction`/`set_restitution` now flag `MATERIAL` (not recycle-safe) when the value actually
changes; an equal write flags nothing.

**Formulas** (`ω` the relative angular velocity in frame-1 coordinates, `(i, j, k)` cyclic):
- limit angle `θ = 2·atan2(q_i, w)`: `θ̇ = ω_i + [(w·q_k + q_i·q_j)·ω_j + (q_i·q_k − w·q_j)·ω_k] / (w² + q_i²)`
- motor angle `2·asin(q_i)`: `θ̇ = (w·ω_i + q_k·ω_j − q_j·ω_k) / √(1 − q_i²)`

The denominators are floored at `1e-4`. dim2 and every other row are untouched. Only correctly
rounded SIMD arithmetic is used, so results stay cross-architecture exact.

**Re-apply** onto another release: `patch -p1 < LUDION-PATCH.diff` (or `git apply`) from the
release's root; if it does not apply, port the two joint formulas and the
three small material edits by hand, then regenerate the diff with `diff -u --strip-trailing-cr`.

Upstream: not reported.
