---
description: Every op of a kind: texture recipe: slots, gray/color rules, and each field's default and range
---

# Texture ops

**Scope:** the op library behind `kind: texture` steps — what each op reads, produces, and accepts.
The recipe format, outputs, the `std:` macros and the CLI are `ludion docs texture-recipes`.

## The loop

Pick the op, write the step, run `ludion texture build <file> --size 64` and read the diagnostics: an
out-of-range field is `L1284` with a JSON Pointer to it, an unknown op or field gets a
"did you mean" (`L1283`). A field marked *req* has no default.

## Format

Every step is `<name>: { op: <op>, <fields> }`. A *step ref* field names another step. A number
field also takes `$param`; a color is `"#rrggbb"` (sRGB in, linear inside). Everything is in tile
units (the tile is 0..1 on both axes) and wraps. **Gray** = one channel, **color** = three.

### Generators

| op | slots | produces |
| --- | --- | --- |
| `constant` | — | gray from a number, color from `#rrggbb` |
| `gradient` | — | gray periodic ramp along an axis |
| `noise` | — | gray lattice noise, fbm when `octaves` > 1 |

| op.field | type | default | range |
| --- | --- | --- | --- |
| `constant.value` | number or color | req | finite |
| `gradient.axis` | `u` \| `v` | req | `u` runs left→right, `v` top→down |
| `gradient.shape` | `ramp` \| `triangle` \| `sine` | `ramp` | ramp is discontinuous at the period edge; sine is 0 there |
| `gradient.repeat` | int | 1 | 1..4096 periods per tile |
| `noise.type` | `value` \| `gradient` \| `cellular` | req | |
| `noise.scale` | int | req | 1..65536 lattice cells per tile |
| `noise.octaves` | int | 1 | 1..12 |
| `noise.lacunarity` | int | 2 | 2..65536 lattice multiplier per octave |
| `noise.gain` | number | 0.5 | amplitude multiplier per octave |
| `noise.seed` | int | 0 | 0..4294967295, mixed into the recipe seed |
| `noise.return` | `f1` \| `f2` \| `f2_minus_f1` | `f1` | cellular only; `f2_minus_f1` is bright on cell borders |
| `noise.jitter` | number | 1.0 | 0..1, cellular only: feature-point wander |
| `noise.stretch` | int | 1 | 1..256; features run this many times longer along `stretch_axis`, the cross axis gets `scale × stretch` cells |
| `noise.stretch_axis` | `u` \| `v` | `u` | the axis stretched features run along |

The finest octave (`scale × lacunarity^(octaves-1) × stretch`) may not exceed 65536 cells (`L1284`).

### Adjust

| op | slots | produces |
| --- | --- | --- |
| `blend` | `a`, `b`, optional gray `mask` | `a` moved toward `combine(a, b)` by `opacity × mask`; color if either side is |
| `levels` | `input` | per channel; the input range clamps |
| `curve` | `input` | piecewise-linear transfer, flat outside the points |
| `clamp` | `input` | per channel |
| `remap` | `input` | linear, unclamped |
| `colorize` | gray `input` | color through a stop gradient |
| `grayscale` | color `input` | gray by Rec.709 luminance |
| `posterize` | `input` | per channel, snapped down to `steps` evenly spaced levels in 0..1 |
| `hsv_shift` | color `input` | color with hue rotated and saturation and value scaled, on the stored (linear) values |

| op.field | type | default | range |
| --- | --- | --- | --- |
| `blend.mode` | `mix` `add` `subtract` `multiply` `screen` `overlay` `min` `max` `difference` | `mix` | `mix` is `b`; `overlay` multiplies where `a` < 0.5 |
| `blend.opacity` | number | 1.0 | 0..1 |
| `levels.in_low` / `in_high` | number | 0 / 1 | must differ |
| `levels.gamma` | number | 1.0 | > 0; above 1 brightens |
| `levels.out_low` / `out_high` | number | 0 / 1 | |
| `curve.points` | list of `{x, y}` | req | ≥ 2, `x` strictly increasing |
| `clamp.min` / `max` | number | 0 / 1 | `max` ≥ `min` |
| `remap.from_low` / `from_high` | number | req | must differ |
| `remap.to_low` / `to_high` | number | req | |
| `colorize.stops` | list of `{at, color}` | req | ≥ 2, `at` 0..1 non-decreasing |
| `posterize.steps` | int | req | 2..256 |
| `hsv_shift.hue` | number | 0 | degrees, any finite value, wraps |
| `hsv_shift.saturation` | number | 1.0 | ≥ 0 multiplier; the result clamps at 1 |
| `hsv_shift.value` | number | 1.0 | ≥ 0 multiplier |

### Filters

| op | slots | produces |
| --- | --- | --- |
| `warp` | `input`, `by` | `input` displaced by `by`; a gray `by` moves along `angle` by `(by−0.5)×amount`, a color `by` uses its R and G; neutral gray is identity |
| `blur` | `input` | wrap-around gaussian-like blur; with `angle`, a line blur along that direction |
| `normal_from_height` | gray `input` | color tangent-space normal (Sobel), +Y image-up (glTF) |

| op.field | type | default | range |
| --- | --- | --- | --- |
| `warp.amount` | number | req | tile units |
| `warp.angle` | number | 0 | degrees, gray `by` only |
| `blur.radius` | number | req | 0..1 tile units |
| `blur.angle` | number | none | degrees as `warp.angle`; absent = blur in all directions |
| `blur.spread` | `both` \| `forward` | `both` | `forward` smears content toward `angle` in a one-sided trail; needs `angle` |
| `normal_from_height.depth` | number | 0.05 | > 0; world height of value 1 in tile units |

### Hard generators

| op | slots | produces |
| --- | --- | --- |
| `tile_sampler` | optional gray `pattern` | gray; shapes (or the pattern) stamped on a `columns × rows` grid with wrap-around |
| `flood_fill` | gray `input` | gray per connected region of `input ≥ threshold` (across seams); color for `uv` |
| `distance` | gray `input` | gray exact distance to the nearest texel at or above `threshold` |
| `band` | gray height `input` | gray; 1 on a line of constant tile-unit width along the `level` contour |
| `ambient_occlusion` | gray height `input` | gray; 1 open, darker in recesses (horizon search, 8 directions) |
| `curvature` | gray height `input` | gray; 0.5 flat, bright on crests, dark in cavities |

| op.field | type | default | range |
| --- | --- | --- | --- |
| `tile_sampler.columns` / `rows` | int | req | 1..1024; `rows` even when `offset` > 0 |
| `tile_sampler.shape` | `box` `disc` `bell` `pyramid` | `box` | ignored with a `pattern` |
| `tile_sampler.scale` | number | 1.0 | 0..4, instance size relative to its cell |
| `tile_sampler.scale_jitter` / `position_jitter` / `rotation_jitter` / `value_jitter` | number | 0 | 0..1 each |
| `tile_sampler.rotation` | number | 0 | degrees |
| `tile_sampler.offset` | number | 0 | 0..1 cell widths, shifts odd rows |
| `tile_sampler.offset_jitter` | number | 0 | 0..1 cell widths, random extra shift of each row |
| `tile_sampler.gap` | number | 0 | 0..0.5 tile units shaved off every instance edge-to-edge, uniform on both axes |
| `tile_sampler.pattern_scale_jitter` | number | 0 | 0..1, per-instance scale of the `pattern` lookup in `[1 - j, 1 + j]` |
| `tile_sampler.pattern_offset_jitter` | number | 0 | 0..1, per-instance offset of the `pattern` lookup in `[0, j)` on each axis |
| `tile_sampler.density` | number | 1.0 | 0..1 chance an instance is kept |
| `tile_sampler.combine` | `max` \| `add` | `max` | how overlaps combine |
| `flood_fill.threshold` | number | 0.5 | 0..1 |
| `flood_fill.output` | `random` \| `gradient` \| `uv` | `random` | `uv` = position in the bounding box (R, G) and a random B |
| `flood_fill.angle` / `angle_jitter` | number | 0 / 0 | degrees / 0..1 turn; `gradient` only |
| `distance.threshold` | number | 0.5 | 0..1 |
| `distance.max` | number | 0.1 | > 0; distance that maps to 1 |
| `band.level` | number | 0.5 | 0..1; texels at or above it are inside |
| `band.width` | number | req | (0, 0.5] tile units; `both` lights half each side |
| `band.side` | `both` \| `inside` \| `outside` | `both` | which side of the contour is lit |
| `band.softness` | number | 0.5 | 0..1 share of the width that falls off to 0 |
| `ambient_occlusion.radius` | number | 0.05 | (0, 0.5] tile units |
| `ambient_occlusion.depth` | number | 0.05 | > 0 |
| `ambient_occlusion.strength` | number | 1.0 | 0..1 |
| `curvature.radius` | number | 0.01 | (0, 0.25] blur radius |
| `curvature.strength` | number | 4.0 | > 0 gain of height minus blur |
| `flood_fill.seed`, `tile_sampler.seed` | int | 0 | 0..4294967295 |

### Plumbing

| op | slots | produces |
| --- | --- | --- |
| `image` | — | one PNG resampled to the build size; gray stays gray, anything else color |
| `input` | — | an image the parent step binds; `default` when the recipe is built alone |
| `recipe` | `inputs:` bindings | one step of another recipe |
| `layer` | the maps in `base:`, a mask per layer | one step per map, `<step>.<map>`; see Layers in `ludion docs texture-recipes` |

| op.field | type | default | range |
| --- | --- | --- | --- |
| `image.path` | string | req | relative to the referencing recipe; forbidden in `std:` recipes |
| `image.color` | `srgb` \| `linear` | `srgb` | how a color file is decoded |
| `input.type` | `gray` \| `color` | req | |
| `input.default` | number or color | 0 / black | must match `type` |
| `recipe.path` | string | req | relative to the referencing file, or `std:<name>` |
| `recipe.output` | step name | req | the sub-recipe step to use |
| `recipe.params` | map | none | number, `#rrggbb`, or `$param` of this recipe |
| `recipe.inputs` | map | none | `<input step of the sub-recipe>: <step of this recipe>`; channels must match (`L1284`) |
| `recipe.seed` | int | 0 | offset for the sub-recipe |
| `layer.base` | map of `<map>: <value>` | req | maps `base_color` `roughness` `metallic` `height` `emissive`, ≥ 1; a value is a step, a number, `#rrggbb` or `$param` |
| `layer.layers` | list | req | ≥ 1, bottom to top |
| `layers[].mask` | step | req | gray |
| `layers[].opacity` | number | 1.0 | 0..1 |
| `layers[].<map>` | step, number, `#rrggbb` or `$param` | none | ≥ 1 per layer; the map must be in `base`; gray maps take gray values (`L1284`) |

## Rules

- **Slots are typed gray or color.** A mismatch is `L1284` naming the step and field; the fix is
  `colorize` (gray → color) or `grayscale` (color → gray). `blend` of gray and color broadcasts the
  gray; a `mask` must be gray.
- **Sub-recipe inputs are real edges.** An `input` step the parent does not bind uses its `default`;
  a bound step's channel count must equal the input's.
- A missing step, a cycle or an unknown output name is `L1285`.

## Gotchas

- `normal_from_height` and `ambient_occlusion` read the height's *scale*: halve `depth` and relief
  halves. Feed both the same height or the shading disagrees with the normal.
- `distance` and `flood_fill` threshold the input; a soft gradient gives a thin or empty region —
  `levels` it first.
- `noise` stretches only along `u` or `v` (the lattice must stay periodic); for diagonal fibres `warp` a stretched noise.
- `tile_sampler` with `offset` > 0 and an odd `rows` would not tile; it is refused; `offset_jitter` needs no even `rows`.

## Next

- `ludion docs texture-recipes` — the recipe format, outputs, `std:` macros, validators.
- `ludion docs materials` — how the built maps attach to a mesh.
