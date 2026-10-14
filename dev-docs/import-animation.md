---
description: ludion-import animation mapping — clip pipeline, key reduction, bound check, L1206-L1208
---
# import-animation

**Scope:** glTF animations to `clips/*.anim.yaml` in `ludion_import` (`animation.rs`). Hub: `ludion devdocs ludion-import`; skins: `ludion devdocs import-skin`.

## Dataflow

### Animation
Runs after `build_entities` (which returns the skin bindings): each glTF animation becomes one
`clips/<name>.anim.yaml` (`Import.clips`, name from the animation, sanitized and deduped case-insensitively, else
`animation-<i>`). `generated_from` is set by the CLI (`run_gltf`, relative to `clips/`), never the
importer. Design: `Plan/animation-clips.md` decisions 8, 9, 11–13.
`ludion check-scene <clip>` re-imports a clip's `generated_from` and prints its `ClipReport` (`crates/ludion-cli/src/clip_report.rs`).
- **Mapping.** Translation/rotation/scale channels on a joint of a bound skin → the track named after
  the joint (parent-first order). `L1202`, channel dropped: morph weights, a non-joint node, a
  channel on a *root* joint whose rest is re-expressed in the mesh frame or on a joint whose rest
  folds a helper node above it (`SkinBinding.reframed`), a joint of a second skin, sparse accessors. An animation with
  no surviving channel, or zero length, writes no clip (`L1202`).
- **Pipeline.** Source frames = union of sampler input times (each CUBICSPLINE interval 4, 8, 16, 32 or 64 equal steps — the fewest at which every step's chord
  midpoint is within the channel's reduce tolerance of the spline, else `L1206`, so between frames the clip stays within that tolerance of what was measured; times snapped
  to the shortest `f32` decimal) → source sampled per glTF spec → candidate key per frame (rotation
  as Euler XYZ degrees, unwrapped against the previous key over the alternate solution) → greedy
  reduction (the two-key `clip_eval` sampler must reproduce every frame between within tolerance;
  a rotation segment of 180° or more on an axis is never merged, since the validator refuses it; a
  segment left at exactly 180° between two frames gets a key at the source's midpoint, which keeps the
  source's direction, and the keys after it are re-expressed)
  → `measure`: joints composed to model space, source vs reduced clip, at every frame. Rotation is
  compared as `2·acos|dot|`, translation Euclidean, scale max-abs. The reduced clip's rotation uses
  `ludion_scene::sample_rotation` (quaternion slerp, the runtime's rule).
- **Measurement details.** A joint's model rotation is Gram-Schmidt orthonormalized (a non-uniform
  parent scale shears the matrix). A channel whose every source frame is within the reduce tolerance
  of the joint's rest is dropped, and a joint left with no channel has no track; `measure` samples
  the gap as rest.
- **Extras (animation `extras`, no CLI flags).** `loop` (bool, default false) and the tolerances
  `reduce_rotation_deg`/`reduce_translation`/`reduce_scale` (defaults 0.1°/0.5 mm/0.001) and
  `bound_rotation_deg`/`bound_translation`/`bound_scale` (0.5°/2 mm/0.005), finite and positive.
  Anything else is `L1207`. Blender: Action custom properties, `export_extras=True`.
- **`L1208`** (every channel within the key-reduction tolerance of rest, so the clip would have no tracks) is refused in both modes, `--allow-lossy` included; it runs after the bound check so `L1206` wins on real motion.
- **`L1206`** (worst error/bound offence per clip: animation, joint, frame, channel) is never
  downgraded — it fails the import under `--allow-lossy` too.
- **`ClipReport`** (per clip): source frames and times, key counts before/after per track channel,
  every joint's worst model-space error with its frame, and `seam` for `loop: true` clips.
- **Determinism.** All `f64`; trig only via `ludion_core::math::f64`; glam `D*` for matrix
  arithmetic, never its `from_euler`/`slerp`/`normalize`; `ludion_core::math::f64`'s Euler pair is
  tested against glam's `EulerRot::XYZ`.

## Verify

```
cargo test -p ludion-import --test animation
```
