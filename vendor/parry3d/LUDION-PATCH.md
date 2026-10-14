# parry3d 0.30.2, modified by Ludion

This is parry3d 0.30.2 (Apache-2.0, Dimforge) with one file changed, as Apache-2.0 §4(b) requires
stating. Everything else is byte-identical to the crates.io release; the license is `LICENSE-APACHE`.

**Changed:** `src/query/ray/ray_heightfield.rs`, the dim3 `RayCast for HeightField`'s
`cast_local_ray_and_get_normal`: the start cell is re-seated right after `cell_at_point`. The exact
change is `LUDION-PATCH.diff`.

It reaches every build through rapier3d's path dependency on it (`vendor/rapier3d/Cargo.toml`), not a
root `[patch]`.

**Why:** `cell_at_point` divides by the cell size, but `triangles_at` and the cell walk put boundaries
at `x_at`/`z_at`, so on a grid line the two disagree by an ulp and the ray starts in a cell whose
triangles stop short of it. A ray with no x or z motion never walks across that axis, so it never
reaches the right cell: a vertical ray through a vertex or along an edge missed (about half the
vertices of a 151x41, 2 m grid). The patch moves the start cell along `x_at`/`z_at` with exact
comparisons, clamped to the real cells.

**Cross-arch:** only comparisons and the arithmetic `x_at`/`z_at` already use; no new rounding.

**Residue:** an oblique ray lying in a grid plane can still miss on a shared edge or vertex, from f32
rounding in the triangle test, which has no tolerance (282 of 46488 for in-plane rays travelling along
x in `ludion-physics/tests/queries.rs`).

**Re-apply** onto another release: `patch -p1 < LUDION-PATCH.diff` (or `git apply`) from the
release's root; if it does not apply, port the one hunk by hand, then regenerate the diff with
`diff -u --strip-trailing-cr`.

Upstream: not reported.
