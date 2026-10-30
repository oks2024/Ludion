---
description: Per-machine render quality: the --quality setting, LUDION_QUALITY, the presets, the override file
---

# Render quality

**Scope:** Choosing how much GPU a **machine** spends drawing a scene: the `--quality` setting, the
`LUDION_QUALITY` variable, the four presets and the TOML override file. What each effect *is*, and
how a scene authors it, is `ludion docs post-processing`; the rest of a host's flags are
`ludion docs running`.

Quality is a host switch, never scene content (like the supersampling `--scale`): how much to draw is
a property of the box. It is **render-only** — the `WorldHash` sequence, a snapshot and a recorded
`.lrp` are identical at every quality. It is not a protocol command: no client can change it.

## The loop

```
ludion render --scene scenes/main.yaml --out shot.png --quality low
ludion dump-graph --scene scenes/main.yaml --quality low   # which passes survive the mask, no GPU
ludion serve --scene scenes/main.yaml --quality high

cargo run -- --scene scenes/main.yaml --quality low        # a shipped game gets the same setting
cargo run -- --scene scenes/main.yaml --quality laptop.toml
LUDION_QUALITY=low cargo run -- --scene scenes/main.yaml    # ...or set the box once and forget it

ludion explain L0016                                       # when a setting is refused
```

`dump-graph` is the check step: CPU-only, it prints the passes the frame schedules, so an effect
visibly *leaves the graph* without rendering.

## Format

A setting's value is either a preset name or **the path to a TOML override file**. There is no third
form — anything that is not a preset name is read as a path.

| preset | `ssao` | `shafts` | `dof` | `gi_dynamic` | `shadows` |
| --- | --- | --- | --- | --- | --- |
| `low` | off | off | off | off | off |
| `medium` | on | on | on | on | on |
| `high` | on | on | on | on | on |
| `full` (default) | on | on | on | on | on |

Names are case-insensitive. Only `low` changes *which* effects are drawn; `medium` and `high` keep
every effect and make each work less hard:

| preset | `ssao_samples` | `shaft_steps` | `dof_taps` | `shadow_map_size` | `local_shadow_atlas_size` | `sun_cascades` | `pcf` | `gi_rays` | `gi_probe_scale` | `render_scale` | `particle_density` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `low` | (effects off) | (effects off) | (effects off) | (shadows off) | (shadows off) | (shadows off) | (shadows off) | 16 | 2.0 | 0.75 | 0.25 |
| `medium` | 8 | 16 | 12 | 1024 | 2048 | 2 | 3 | 32 | 1.5 | 1.0 | 1.0 |
| `high` | 12 | 24 | 20 | 2048 | 4096 | 3 | 3 | 48 | 1.0 | 1.0 | 1.0 |
| `full` (default) | 16 | 32 | 32 | 2048 | 4096 | 4 | 3 | 64 | 1.0 | 1.0 | 1.0 |

A lower count is a coarser estimate of the same picture. `shadows` off keeps the sun lighting and only
stops it casting.

**`gi_probe_scale` is the one axis that reads upward.** It multiplies the scene's authored
`GiVolume.probe_spacing`, so cheaper is *larger*: `1.0` bakes the authored lattice, `2.0` twice the
spacing, an eighth of the probes. Its range is `1.0` upward.

**`render_scale`** is the fraction of the output extent the frame is drawn at before being resampled
up; cheaper is smaller, `1.0` is native. `low`'s `0.75` draws 56% of the pixels — every pass the render
extent sizes takes the discount. `medium`/`high` stay native (a resampled frame is softer, not just
coarser). More than native is supersampling — the host's `--scale` — so `1.0` is the ceiling, and
**an explicit `--scale` beats the one a quality supplies, which beats native.**

**`particle_density`** is the one axis the engine spends, not the renderer: it multiplies every
`ParticleEmitter`'s `rate` and `burst` at extract time — population only; sizes, colours, speeds,
lifetimes, `seed` and motion are untouched. Cheaper is smaller, `1.0` is the ceiling and the identity
(goldens and budgets are captured at `full`).

**The two GI axes still cost at `low`.** `gi_dynamic: false` removes the per-frame injection, not the
bake, so a scene with a `GiVolume` bakes sky visibility and bounce at every preset, and both axes land
in its pixels there.

The override file is a base preset plus per-effect and per-parameter overrides. Every key is
optional; an absent one keeps the base's, and an absent `preset` means `full`.

| key | type | meaning |
| --- | --- | --- |
| `preset` | one of the four names | the base these keys override (default `full`) |
| `effects.ssao` | bool | render ambient occlusion on scenes that author it |
| `effects.shafts` | bool | render volumetric light shafts on scenes that author them |
| `effects.dof` | bool | render depth of field on scenes that author it |
| `effects.gi_dynamic` | bool | inject dynamic-light GI bounce on scenes that author it (off ⇒ `GiDynamic`-marked lights contribute direct light only) |
| `effects.shadows` | bool | draw the sun's shadow map on scenes that carry a sun (off ⇒ the sun still lights, and light shafts go with it) |
| `effects.ssr` | bool | trace screen-space reflections on scenes that author `PostProcess.ssr` (off ⇒ the sky reflection) |
| `params.ssao_samples` | int, `1..=16` | hemisphere samples per ambient-occlusion pixel |
| `params.shaft_steps` | int, `1..=32` | ray-march steps per light-shaft pixel |
| `params.dof_taps` | int, `1..=32` | taps in the depth-of-field disc |
| `params.shadow_map_size` | int, `1..=2048` | edge length of the square sun shadow map, or of one cascade tile, in texels |
| `params.sun_cascades` | int, `1..=4` (`L0016`) | the most cascades a sun fits, overriding the preset's cap in either direction (`ludion docs sun-cascades`) |
| `params.local_shadow_atlas_size` | int, `16..=4096`, a multiple of `16` (`L0016`) | edge length of the square local-light shadow atlas, in texels (`ludion docs local-shadows`) |
| `params.ibl_specular_size` | power-of-two int, `16..=256` | edge length of the sky's specular reflection cube faces, in texels (`128` at `low`, `256` otherwise) |
| `params.pcf` | odd int, `1..=3` | edge length of the shadow map's filter kernel, in taps (`1` is a single centre tap) |
| `params.gi_rays` | int, `1..=64`, a multiple of `16` | rays the GI bake traces per irradiance probe |
| `params.gi_probe_scale` | float, `1.0..=4.0` | multiplier on the scene's authored `GiVolume.probe_spacing` — **larger is cheaper** |
| `params.render_scale` | float, `0.25..=1.0` | fraction of the output extent the frame is drawn at before being resampled up to it (`1.0` is native) |
| `params.particle_density` | float, `0.0..=1.0` | multiplier on every `ParticleEmitter`'s `rate` and `burst` at extract time (`1.0` is the full authored population) |
| `params.ssr_scale` | float, `0.25..=0.5` | fraction of the output extent the reflection trace runs at (`0.5` at every preset) |

A complete file — `low`, but with contact shadows at half `medium`'s samples:

```toml
preset = "low"

[effects]
ssao = true

[params]
ssao_samples = 4
```

## Rules

- **Precedence is `--quality` → `LUDION_QUALITY` → a packaged default → `full`.** The packaged
  default is what an exported app ships with (`[ios] quality`, `ludion docs export`; `"low"` is how
  iOS turns SSR off, and the report's descriptor carries `ssr`, `ssr_scale` and `ibl_specular_size`). An explicit
  setting wins outright and lower sources are not read for validity, so a flag works on a box whose
  `LUDION_QUALITY` is malformed. Absent, empty or whitespace counts as not chosen.
- **An unresolvable setting refuses to start** — one `L0016` naming both interpretations (the preset
  list *and* the unreadable file), never a silent fall back to `full`. Unknown preset, unreadable
  path, malformed TOML, wrong-typed value and unknown key are all this code.
- **Unknown keys and sections in an override file are refused**, so a file written for a newer build
  fails loudly rather than describe a quality that is not rendered. `[params]` accepts exactly the
  nine keys above.
- **`full` is the ceiling.** Every `[params]` count is at most its `full` value and at least `1`;
  otherwise `L0016` naming the range. A quality only ever spends less than the scene authors.
  `render_scale` is bounded the same way (`1.0` is native; above it is the host's `--scale`).
  **`gi_probe_scale` reads the other way**: `1.0` is its *floor*, `4.0` its ceiling.
- **Four axes carry a shape rule beyond range, each its own `L0016` message.** `ibl_specular_size` must be a power of two of at least `16`: the
  cube's mip chain halves down to 8×8. `pcf`'s kernel is
  centred on the shaded texel, so an even edge is refused. `gi_rays` must be a multiple of `16`: the
  bake's hit cache holds sixteen slots and takes every `gi_rays / 16`'th ray, and a fractional stride
  crowds directions into one pole. `gi_probe_scale` must be finite and at least `1.0`, `render_scale`
  finite and at most `1.0`.
- **The setting is per-machine, never in a committed file.** It lives in the environment or command
  line, like `LUDION_THREADS`. An override file is *local*: committing one makes one machine's budget
  everyone's.
- **A masked effect is byte-identical to the scene not authoring it.** Its passes are not scheduled
  (not weakened), so `low` lands on the frame the engine produces for those strengths at zero. A
  *lowered* effect still runs, as a coarser estimate of the same image.
- **UI and bloom are content, not quality** — a vanished HUD is a different frame; bloom is part of
  the imaging chain. No toggle. **Neither has the sun:** `shadows` off removes the shadow map and
  what reads it; the sun keeps lighting.
- **Golden images are pinned at `full`.** The harness renders every golden at full explicitly, so a
  machine's setting can never move a committed image. `ludion docs verify-loop` is that gate.
- **A budget check requires `full`.** `profile --check` refuses a non-full capture: ceilings are
  calibrated against everything the scene authors.

## Textures: source or cooked

`--textures <source|cooked>` picks where imported mesh maps come from: `source` decodes the PNGs,
`cooked` reads the BC7/ASTC copies `ludion sync` wrote under `.ludion/cooked/`. A map with no cooked
copy loads from its PNG, with one `L1271` warning per load. Pixels only — never the sim or its hash.

| Host | Textures |
|---|---|
| game window, `--addr`, `ludion dev`, `ludion serve` | `cooked` |
| exported and `launch` builds | `cooked`, always |
| `--profile` | `source`, unless `--textures cooked` |
| `--render-out`, `--replay-*`, `--check-*`, `ludion render`, `ludion test`, goldens | `source` |

The verification modes refuse the flag, so a golden never depends on the store.

## Gotchas

- **A misspelled preset reports as a missing file.** `--quality medum` is read as a path; the `L0016`
  names both readings — check the preset list in the message first.
- **`engine.describe` reports quality read-only**: its `quality` object carries the label (`low`,
  `custom`, …) plus the five toggles. To change it, restart or attach to another host.
- **`dump-graph` cannot see a lowered parameter** — `medium` schedules exactly what `full` does — with
  five exceptions that size a *resource*: `shadow_map_size` (the `shadow_map` transient),
  `sun_cascades` (how many tiles the `shadow_map` extent holds), `local_shadow_atlas_size` (the local atlas, where a light is granted; `shadows: false` masks it and
  the profile panel's `atlas_edge` reads 0), `gi_probe_scale` (the `gi_inject` volume, where injection is scheduled) and `render_scale` (at `low`
  the scene targets are 0.75 of the output and an `upscale` pass carries them back). Use it to check
  a *mask* or an extent; render an image to check a count.
- **`gi_rays` and `gi_probe_scale` change what is baked.** The bake is cached on its inputs, so
  changing either at runtime pays one bake (like moving an unmarked light); setting them at startup,
  the only way a host exposes them, costs nothing extra.
- **Shadows off also turns light shafts off.** The march samples the shadow map, so `shadows = false`
  with `shafts = true` renders neither, and is not refused.
- **A profile report records the quality it was captured at** (label, all five toggles, every axis
  used), like the thread count; measurements at different settings are not comparable. Read a toggle
  before its axis: `shadows: false` beside `shadow_map_size` 1024 means no map was drawn; the GI pair
  describes work done at every preset. **Read `render_scale` before any GPU number**: every one is a
  per-pixel cost, so a `low` capture is 56% of the frame.
- **`custom` labels every override file**, whatever it contains; tell two machines' files apart by
  path.

## Next

- `ludion docs post-processing` — what `ssao`, `shafts` and `dof` are, and how a scene authors them.
- `ludion docs running` — hosting a scene, and the rest of the host flags.
- `ludion docs verify-loop` — the golden-image gate, and why it is pinned at full.
- `ludion docs atmosphere` — the medium the light shafts march through.
