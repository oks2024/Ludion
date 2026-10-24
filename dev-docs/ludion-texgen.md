---
description: ludion-texgen: the kind: texture evaluator (recipe to f32 images to PNG maps) and its rules
---

# ludion-texgen

**Scope:** editing `crates/ludion-texgen/`, the CPU recipe evaluator behind `ludion texture`. The
user-facing format, ops and validators are `ludion docs texture-recipes` and `ludion docs texture-ops`
(one owner each; do not restate them here). Determinism rules shared with sim crates are
`ludion devdocs determinism`; the CLI side is `ludion devdocs ludion-cli-verbs`.

## Layout

- `src/lib.rs` — module map, `build()` (load → evaluate → encode), `SIZE_RANGE` (8..=8192, `L1284`).
- `src/recipe.rs` — parses one document (`parse`), `OutputKey`, binds params into an `Instance`.
- `src/ops.rs` — `Op`, `OP_NAMES`, the field `Reader` (names, defaults, ranges) and every build arm.
- `src/resolve.rs` — `load` through a `Resolver` (`FsResolver` or the embedded `stdlib`); `infer`
  types every step gray or color; the DAG order and sub-recipe nesting live here.
- `src/eval.rs` — `evaluate` (only steps the outputs reach; images drop after their last consumer;
  an observer sees each top-level step) and `compute`, the op → image dispatch.
- `src/noise.rs`, `adjust.rs`, `filter.rs`, `scatter.rs`, `fill.rs` — the op kernels: noise;
  blend/levels/curve/colorize/posterize/hsv_shift; blur (and the line blur)/warp/normal/AO/curvature;
  `tile_sampler`; `flood_fill`/`distance`/`band`.
- `src/image.rs`, `picture.rs`, `hash.rs` — the `Image` (interleaved `f32`, wrap-around sampling),
  PNG decode for the `image` op, the integer hash the lattices use.
- `src/encode.rs` — quantize, sRGB-encode base color and emissive, write PNGs (`MapSet`); `to_rgba8`
  is the quantizer the CLI's sheets reuse. `Image::downsample` (`image.rs`) makes the step thumbnails.
- `src/validate.rs` — `measure` over the f32 maps and the default bounds (the 9 `NAMES`, `default_rule`).
- `src/stdlib.rs` + `std/*.texture.yaml` — the embedded `std:` recipes; each header comment lists
  its inputs, params and outputs.
- Outside the crate: `ludion-cli`'s `texture_feedback.rs` (sheets, thumbnails, tiling views) and
  `texture_preview.rs` (the lit preview: a generated scene rendered in place).
- `tests/` — `determinism.rs` (pinned hashes), `tileable.rs` (seam measures), `errors.rs`
  (one case per error class), `stdlib.rs` (every macro builds standalone).

## Dataflow

Recipe text → `parse` → `load` (sub-recipes through the `Resolver`; the crate opens no file itself —
`Resolver::read_bytes` feeds the `image` op) → `evaluate` → `Maps` (f32, linear) → `encode` → PNG
bytes. `validate::measure` reads the f32 `Maps`, before quantization. Only `ludion-cli` links the
crate; the engine never evaluates a recipe at runtime.

## Rules

- **Bit-exact across Windows x86-64 and macOS arm64, and across thread counts.** Transcendentals go
  through `ludion_core::math` (platform `sinf` differs in the last bit and moves a PNG byte);
  parallelism only through `ludion_core::par` over fixed row chunks (chunking that follows the thread
  count changes float summation order); no hash-order iteration (use ordered maps). The crate's
  `clippy.toml` is the sim tier; edit its lists in `scripts/clippy-denylist.mjs`.
- **Pinned hashes in `tests/determinism.rs` move only deliberately.** Any kernel change moves them:
  re-pin in the same change and say why in the commit. A hash that differs only on one OS is a
  determinism bug, not a re-pin.
- **A `std:` recipe loads no image and references only `std:` recipes**, so the embedded library is a
  pure function of the binary.
- **Adding an op** touches: the `Op` variant, `OP_NAMES`, `Op::inputs()` and the build arm in
  `ops.rs`; `infer` in `resolve.rs` (gray/color); `eval::compute`; and a row in
  `ludion docs texture-ops` (the only copy of the field tables).
- **Adding a macro**: `std/<name>.texture.yaml`, a `FILES` row in `stdlib.rs`, and a row in
  `ludion docs texture-recipes`.

## Gotchas

- Replacing central differences with a Sobel filter in `normal_from_height` moved every pinned
  output hash; the normal, occlusion and height hashes were re-pinned.
- The pinned hashes were minted on arm64; an x86-64 run is the cross-arch confirmation.
- Params substitute literally (no arithmetic), so a derived value needs a second param or a new op.

## Verify

```
cargo nextest run -p ludion-texgen
cargo test --doc -p ludion-texgen
node scripts/clippy-denylist.mjs
cargo nextest run -p ludion-cli -E 'binary(texture)'
```

The last is the CLI drift test over `examples/textures/`.
