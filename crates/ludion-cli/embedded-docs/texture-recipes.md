---
description: Procedural tileable PBR textures: the kind: texture recipe, the std: library and ludion texture
---

# Texture recipes

**Scope:** writing a `<name>.texture.yaml` recipe and building it into the PBR map set a mesh
consumes, with `ludion texture`. Every op and its fields is `ludion docs texture-ops`; attaching the
maps to a mesh is `ludion docs materials`.

## The loop

1. Write `<name>.texture.yaml` (worked example: `examples/textures/tiles.texture.yaml`).
2. `ludion texture build <file> --size 256` — writes the set to `<name>/` beside the recipe and
   prints the validator lines. `--json` prints one report (`steps` timings, per-map `maps` min/mean/max,
   `validators`, `written`). `--out <dir>` redirects; `--rules <toml>` sets validator bounds.
3. Read the diagnostics and validators, correct the recipe, rebuild.
4. Commit the recipe **and** the `<name>/` set. `ludion texture check <file>` rebuilds at the size in
   `generated.json` and byte-compares the committed set (`L1286` per differing, missing or stray file);
   it writes nothing, so it is the drift gate.
5. To customize an embedded macro: `ludion texture vendor std:cobbles [dest]` copies it out (never
   overwrites); reference the copy by path.

## Format

| key | meaning |
| --- | --- |
| `format: 1`, `kind: texture` | the envelope; a missing or wrong `kind` is a document error |
| `seed` | integer ≥ 0, default 0; every generator's seed is mixed with it |
| `params` | name → number or `"#rrggbb"`; a step field written `$name` takes the value |
| `steps` | name → `{op, fields}`; a DAG, any order, referenced by name |
| `outputs` | map key → step name |

`$param` substitutes literally: there is no arithmetic, so a derived value is a second param. A
sub-recipe's own params are overridden by the referencing step's `params:`.

All images are in **tile units** (0..1) and tileable by construction; a recipe has no pixel size.
Pick the size at build time (`--size`, 8..8192, default 1024).

### Outputs

| key | slot | required | when omitted |
| --- | --- | --- | --- |
| `base_color` | gray or color | yes | |
| `roughness` | gray | yes | |
| `height` | gray | yes | |
| `metallic` | gray | no | 0 |
| `normal` | color | no | `normal_from_height` of `height`, depth 0.05 |
| `occlusion` | gray | no | 1 |
| `emissive` | gray or color | no | no `emissive.png` is written |

### Sub-recipes

`op: recipe` with `path` (relative to the referencing file, or `std:<name>`), `output` (the step to
take), optional `params:` (number, color or `$param` of this recipe), `inputs:` (`<input step of
the sub-recipe>: <step of this recipe>`) and `seed:`. The sub-recipe declares what it receives with
`op: input`. A `std:` recipe may reference only other `std:` recipes and loads no image.

### Outputs on disk

`<name>/` holds PNGs in the order below plus `generated.json` (`generated_by`, `recipe`, `size`,
`maps`): **write-only — never hand-edit the set or the sidecar**, rebuild.

| file | encoding |
| --- | --- |
| `base_color.png` | 8-bit sRGB |
| `metallic_roughness.png` | linear; G = roughness, B = metallic (glTF packing) |
| `normal.png` | linear, tangent space, +Y image-up |
| `occlusion.png` | linear gray |
| `emissive.png` | 8-bit sRGB, only when authored |
| `height.png` | 16-bit linear gray; not a mesh map |

A `.mesh.yaml` references the five PNGs through its `*_texture` fields (`ludion docs materials`).

### The std library

Referenced as `path: std:<name>`. Params list their defaults.

| name | input | params | outputs |
| --- | --- | --- | --- |
| `cobbles` | — | `scale` 6, `gap` 0.04, `bevel` 0.3, `jitter` 0.85 | `height`, `mask` (1 on a stone), `stone_id` (random gray per stone) |
| `planks` | — | `columns` 2, `rows` 6 (even if `stagger` > 0), `gap` 0.08, `stagger` 0.5, `grain_scale` 14 | `boards`, `board_id`, `grain`, `height` |
| `edge_wear` | `height` gray (default 0.5) | `amount` 0.9, `radius` 0.01, `breakup_scale` 8, `breakup` 0.7 | `edges`, `mask` (the one to use) |
| `cavity_grime` | `height` gray (default 0.5) | `amount` 0.9, `radius` 0.03, `breakup_scale` 6 | `cavities`, `occlusion`, `mask` |
| `rust_spots` | — | `coverage` 0.25, `scale` 4, `breakup` 0.06 | `mask`, `pits` |

The macros are style-agnostic masks and layouts; colors, roughness and the look are the consumer's
`colorize`/`levels`/`blend` steps.

### Validators

`build` and `check` measure the evaluated maps (before PNG quantization) and report each. A value
outside its bounds is `L1287` at level `warn` or `L1288` at level `error`; maps are written either
way, and an `error` makes the exit code non-zero.

| validator | measures | default bound | level |
| --- | --- | --- | --- |
| `albedo_out_of_range` | share of base color texels with sRGB luminance outside 30..240 | max 0.02 | warn |
| `metallic_ambiguous` | share of metallic texels strictly between 0.1 and 0.9 | max 0.10 | warn |
| `roughness_mirror` | share of roughness texels below 0.05 | max 0.05 | warn |
| `normal_invalid` | share of normals not unit length (±0.05) or pointing into the surface | max 0 | error |
| `height_range` | height max − min | min 0.05 | warn |
| `seam_base_color` | wrap-edge difference ÷ interior neighbour difference of base color | max 1.5 | warn |
| `seam_height` | same, of height | max 1.5 | warn |

A rules TOML (`--rules`) overrides `min`, `max` and `level` field by field: top-level tables for the
project, `[textures."<recipe file>"]` for one recipe.

```toml
albedo_out_of_range = { max = 0.10 }

[textures."tiles.texture.yaml".seam_height]
max = 3.0
level = "error"
```

An unknown validator name or unreadable file is `L1289`.

## Rules

- **Deterministic.** A recipe, its referenced files and a size give the same bytes on any thread
  count and on both pinned OSes. Each size is separately exact: changing `--size` rewrites every map
  and `check` then reports drift — rebuild and commit.
- `L1283` structure: a missing or wrong-typed key, an unknown op or field — the help names the
  nearest valid one. `L1284` data: a value out of range, a gray/color mismatch, a bad size.
  `L1285` graph: a step reference that does not exist, a cycle. `L1286` drift. `ludion explain L1283`
  and the others give the full text.
- `ludion check-scene` on a recipe reports `L0031` (the kind is validated by another verb); use `ludion texture build` to
  validate it.

## Gotchas

- A recipe's step order is irrelevant but its **seed** is not: reusing a sub-recipe twice with the
  same `seed` repeats the same pattern; pass a different `seed:` on each `recipe` step.
- A flat `height` fails `height_range` and gives a flat normal; `levels` a noise up before using it
  as relief.
- Feed `edge_wear`/`cavity_grime` a real height through `inputs:`; unbound they see a flat 0.5 and
  return an empty mask.

## Next

- `ludion docs texture-ops` — every op, field, default and range.
- `ludion docs materials` — attaching the maps to a mesh.
- `ludion docs mesh-editing` — painting a map afterwards.
