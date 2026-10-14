---
description: Quality axes: uniform sample counts, gi_rays, gi_probe_scale, and what dump-graph can observe
---
# Renderer quality axes

**Scope:** The `QualityParams` axes that change *how hard* a scheduled effect works. Masking, the host half and the golden pin are `ludion devdocs renderer-quality`.

## Rules
- **A modulated count is a uniform the shader reads at every site that uses it**, never a `const` and
  never a literal beside the uniform. Each estimator divides by its own count as well as looping to
  it — `ssao.wgsl` ramps the radius and normalizes, `shafts.wgsl` derives `dt = end/N`, `dof.wgsl`
  sets the spiral radius and coverage, `mesh.wgsl` walks ±`pcf/2` and divides by `pcf²` (three reads,
  not two) — and a site left holding a literal would make a lowered preset take fewer samples than it
  divides by: not a cheaper estimate but a wrong one, invisible to every golden because `full` would
  still be exact. Pinned by `each_modulated_axis_is_a_uniform_its_shader_reads_at_every_site`, which
  counts the WGSL reads with comment lines stripped, and by
  `needs_gpu_a_lowered_parameter_makes_each_effect_cheaper_and_reaches_nothing_else` (GPU), which renders each
  effect alone at two presets with the sun's axes pinned at the ceiling.
- **A uniform loop bound is allowed only while `full`'s bytes hold.** Making a count a uniform
  changes the generated code, which can move a pinned GPU's output even when the arithmetic is
  identical. Measured on the RTX 3090 for all five — the three post axes, the PCF kernel in the
  *scene* pass, and `gi_rays` in both GI compute shaders: every golden stayed byte-identical. If a
  future axis moves one, the answer is a `compose_parts` shader permutation whose full-quality
  variant keeps a literal `const` — never a re-bless (`Plan/scalability.md`).
- **`gi_rays` reaches both GI shaders from the *bake's* value, never the machine's read twice.**
  `BakeInput::rays` carries the setting in, the bake hands it back on `ResolvedGi::rays`, and
  `InjectInput::rays` takes it from there. The ray set is **re-derived** at every count (the
  elevation divides by it) rather than truncated, and the injection divides by the count to recover a
  cached slot's direction — so any other count reconstructs directions the march never took,
  silently, with no binding to fail. The same re-derivation is why the count joins the bake's cache
  key (`needs_gpu_a_changed_ray_count_re_bakes_and_travels_with_the_resolved_frame`): a held-over volume is
  stale, not coarse. `HIT_RAYS` stays a constant because it sizes the cache *buffer*; what moves is
  the stride `gi_rays / HIT_RAYS`, hence the multiple-of-16 rule on the axis's values.
- **`gi_probe_scale` is applied once, in `masked`, and the spacing cancels.** Re-laying `count`
  probes at `scale ×` the spacing needs `ceil((count - 1) / scale) + 1` — the spacing divides out,
  which is what lets a scene-free `masked` size the injected volume while `render.rs` builds the
  matching `GiData` through `scaled_lattice`. At `1.0` it is the exact identity, field for field
  (`scaling_the_probe_spacing_is_the_identity_at_full_and_only_ever_coarsens`), and it only ever
  *removes* probes, so it cannot breach the `L2103`/`L2105` budgets the engine already enforced.
- **"Has a sun" and "draws a shadow map" are two facts, and only the second is maskable.** The sun's
  light rides the `Sun` uniform every frame uploads; `PostChain` carries only the map. `build_sun`
  zeroes `light_space` exactly when there is no map, sending `sun_shadow` down its fully-lit
  early-out — without that the scene pass comparison-samples the 1×1 fallback depth and shadows the
  world solid, which is what `needs_gpu_masking_shadows_removes_the_pass_and_leaves_the_sun_lit`'s
  brighter-than-shadowed assertion catches. **Shafts leave with the map** (`masked` ANDs
  `shadow_map.is_some()` into `shafts`): the march samples it, and keeping the map alive for the
  beams alone would make "shadows off" still schedule the `shadow` pass — weakening, not absence.
- **A field added to `Quality` must land in four homes in one change**: the struct plus every preset
  const and the `masked` site (`ludion-renderer`), the `Effects` TOML struct (`ludion-engine`,
  `deny_unknown_fields` — miss it and a file naming the new knob is an `L0016`), `describe`'s
  `quality` object, and the profile report's descriptor (miss it and a measurement no longer says
  what it was taken at). Only the first is a compile error; the rest fail as behaviour. A new
  **modulated** `QualityParams` axis reaches those plus two more: its shader's uniform field and the
  `record_*` site that uploads it. `render_scale`'s are the two exceptions worth knowing — the
  `Params` TOML struct and the descriptor as usual, but `resolve_render_scale` in place of `masked`,
  and no shader at all. `particle_density` is a third exception, further still: the `Params` TOML
  struct as usual, but no `masked` site, no shader, and no `record_*` upload at all —
  `extract_at` reads it directly and `particles::evaluate` thins `rate`/`burst` there, and the
  particles plan's no-schema-rev ruling keeps it out of the profile report's descriptor too.
- **An explicit scale beats a quality-supplied one, which beats native**, resolved in one function
  (`resolve_render_scale`). The two sources are held apart and combined on *read* — `Engine` keeps
  `render_scale_override: Option<RenderScale>` beside its `QualitySetting` — which is what makes
  `set_quality` and `set_render_scale` commute; fold them on write and a host applying its flags in
  the other order silently renders a different frame. The flag inherits the rule: `--scale` defaults
  to *absent*, never to `1.0`, because `1.0` is indistinguishable from a host demanding native and
  would stomp a machine's own factor on every invocation.
- **An axis a preset cannot observe is still reported, at one real value.** `LOW` repeats `MEDIUM`'s
  five effect counts rather than nulling them: a sentinel would make every reader of the descriptor
  carry a per-axis special case for a distinction the pixels do not make. **The GI pair is not in
  that category** — see the masking asymmetry in Gotchas — and carries real values at `LOW`.
- **`gi_probe_scale` is the one axis whose bound reads upward.** It multiplies an authored *length*
  rather than naming a count, so cheaper is larger, `full`'s `1.0` is its floor rather than its
  ceiling, and `ludion-engine`'s `L0016` range is `1.0..=GI_PROBE_SCALE_MAX` instead of `1..=full`.
  Any "every axis is bounded above by `full`" statement has to exclude it
  (`gi_probe_scale_is_bounded_from_one_upward`).

## Gotchas
- **`dump-graph` cannot see a lowered count — except the three that size something.** `medium`
  schedules exactly the passes `full` does and the difference is inside a shader, so the dump is the
  check for a *mask* only. The three axes it does show are declared extents: `post-full` dumps
  `shadow_map` at 2048 and at 1024 under `--quality medium`, `gi-interior` dumps its `gi_inject`
  volume at 49×11×49 and at 25×6×25 under a `gi_probe_scale` of 2.0, and **`render_scale` shows up as
  a whole pass** — `--quality low` dumps a 1440×810 `scene` inside a 1920×1080 `final` plus the
  `upscale` that joins them, which makes the dump the GPU-free check for the resolution too.
- **The GI axes stay observable where the sun's do not.** `shadows: false` at `LOW` makes
  `shadow_map_size` and `pcf` unobservable — no map is drawn. `gi_dynamic: false` masks the per-frame
  *injection* only: the bake runs at every preset, so both GI axes land in a `GiVolume` scene's pixels
  at `LOW` too. A test asserting "this preset cannot distinguish the GI axes" asserts the opposite of
  the design.
