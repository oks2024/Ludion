---
description: Per-machine quality: the presets, PostChain masking, uniform sample counts, the FULL golden pin
---
# Renderer quality

**Scope:** The mechanism behind per-machine render quality — where `Quality` and `QualityParams`
live, how masking reaches the frame graph, how a lowered count reaches a shader, and what stops a
machine's setting from moving a committed golden. The
authoring-side surface (the presets, the override file's schema, `L0016`'s wording) has one owner and
it is `ludion docs quality`. The passes a mask removes are `ludion devdocs renderer-passes`; the
resolution knob beside it is `RenderScale` in `ludion devdocs ludion-renderer`.

## Spokes
- `ludion devdocs renderer-quality-axes` — the modulated counts (uniform sample counts, `gi_rays`, `gi_probe_scale`) and what each axis can show in a dump.

## Layout
- `crates/ludion-renderer/src/quality.rs` — `Quality` (five booleans, `shadows` the newest) and
  `QualityParams` (eight axes, all modulated), each with four preset consts, plus
  `Quality::PRESET_NAMES` and the two `preset` lookups. Plain data: no serde, no I/O, no environment.
- `crates/ludion-renderer/src/graph.rs` — `PostChain::masked`, the one site that applies a quality,
  `PostChain::shadow_map` (the extent, `None` when this frame draws none) and `PostChain::gi_inject`
  (the probe lattice, scaled there).
- `crates/ludion-renderer/src/render.rs` — `Renderer::set_quality` (both halves in one call), the
  four entry points composing `for_scene(scene).masked(&self.quality, &self.quality_params)`,
  `build_sun`, and the `record_*` sites that upload a count.
- `crates/ludion-renderer/src/gi_bake.rs` — `scaled_probe_count`/`scaled_lattice` (the
  `gi_probe_scale` arithmetic), `BakeInput::rays` and `ResolvedGi::rays` (the `gi_rays` chain), and
  the cache key both join.
- `crates/ludion-renderer/src/shaders/{ssao,shafts,dof,mesh}.wgsl` — `Ssao.samples`, `Shafts.steps`,
  `Dof.taps`, `Sun.pcf`: four modulated *uniform* axes, fields rather than `const`s.
- `crates/ludion-renderer/src/shaders/{gi_bake,gi_inject}.wgsl` — `params.flags.y` and
  `params.rays.x`: the fifth, reaching two shaders from one source.
- `crates/ludion-engine/src/quality.rs` — the host half: `QualitySetting`, `QUALITY_ENV`,
  `resolve_startup_quality`, `resolve_render_scale` (the `--scale` precedence), the
  `OverrideFile`/`Effects`/`Params` TOML schema, and `L0016`.
- `crates/ludion-engine/src/engine.rs` — `Engine::set_quality`/`quality`,
  `set_render_scale`/`render_scale`/`frame_size` (the one site an extent is decided), and the
  read-only `quality` object `engine.describe` emits.
- `crates/ludion-testkit/src/golden.rs` — the golden path, which passes `QualitySetting::full()`
  explicitly.

## Dataflow

```
--quality flag ─┐
                ├─► resolve_startup_quality ─► QualitySetting { quality, params, label }
LUDION_QUALITY ─┘         (L0016 or refuse)              │
                                                         ├─► Engine::set_quality ─► Renderer::set_quality
                                                         ├─► describe's `quality` object (read-only)
                                                         ├─► the profile report's quality descriptor
--scale (a host's, optional) ─► resolve_render_scale ◄───┘
                                        └─► Engine::frame_size ─► FrameSize { output, render }

per frame:  PostChain::for_scene(scene) ──masked(&quality, &params)──► build_frame_graph
            record_ssao / record_shafts / record_dof / build_sun ──params──► the pass's own uniform
            extract_at(&params.particle_density) ─► particles::evaluate (thins rate/burst, no graph)
            scaled_lattice(scene.gi, gi_probe_scale) ─┬─► GiBake::resolve (BakeInput.rays)
                                                      │        └─► ResolvedGi.rays ─┐
                                                      └─► GiInject::prepare ◄───────┘
```

Two crossings are the whole design. **Resolution happens in the host**, once at startup, and only
plain data crosses into `ludion-renderer` — which is what keeps TOML parsing and environment reads
out of a crate whose input is meant to be exactly one `RenderScene` (the same rule that keeps asset
paths out of it). **Nothing crosses into the sim**: quality is not a component, not a resource, never
hashed into a `WorldHash`, never in a snapshot or the record stream, so an `.lrp` recorded on a
`low` box replays bit-identically on a `full` one.

The label travels alongside the toggles because several presets resolve to the same `Quality` — a
`Quality` alone cannot answer "which setting did this machine ask for", which is the question a
`describe` response and a profile report both have to answer.

**Toggles and parameters are two values, resolved together and set together.** `Quality` answers
which effects the graph schedules; `QualityParams` answers how hard each scheduled effect works, and
reaches the graph for exactly the two axes that size a *resource* — `shadow_map_size` and
`gi_probe_scale` — because a resource's extent *is* graph structure. `Renderer::set_quality` takes
both in one call, so a renderer can never hold a machine's toggles beside another setting's counts.

**`render_scale` is the one axis `masked` never sees.** It sizes the frame rather than a resource
inside it, so it travels the `FrameSize` route: `Engine::frame_size` asks `resolve_render_scale` for
the factor, and `build_frame_graph` derives every extent from the pair. Hence a machine draws below
native with no new `PostChain` field — and hence the golden harness pins `FrameSize::native` beside
its `QualitySetting::full()`: two routes, two pins.

## Rules
- **`Quality` is plain data in `ludion-renderer` and stays that way.** No `Deserialize`, no
  `std::fs`, no `std::env` — hosts own their own sources. Give the type a serde derive and the crate
  acquires a config-file format, which is the boundary rule `RenderScene` already states.
- **Masked equals unauthored, byte for byte** — the claim the whole feature rests on. `masked` ANDs
  the machine's booleans over the scene's derived chain and the masked passes are then *absent from
  the graph*, so the frame is the one that scene already renders with those strengths at zero.
  A masked effect is never a weakened one: weakening is what a *parameter* does, inside a pass that
  still runs. Pinned by
  `needs_gpu_a_masked_effect_renders_exactly_as_an_unauthored_one` (`render.rs`, GPU) and
  `masking_is_the_identity_at_full_and_reaches_only_the_effects_at_low` (`graph.rs`, all 48 derivable
  chains). **`shadows` is the exception and states the weaker claim**: no scene can author a sun that
  lights without casting, so what it promises is that the passes leave and the light does not.
- **`ui` passes through `masked` untouched, and bloom has no toggle at all.** Both are content or
  imaging: a HUD that vanished is a *different* frame, and bloom's strength is calibrated against the
  composite it feeds. Masking either makes quality a look change, at which point "the same scene at
  two qualities" stops being true.
- **No ambient input on a render path.** `resolve_startup_quality` is the only reader of
  `LUDION_QUALITY` anywhere in `crates/`; every other path takes a setting as an argument, and the
  golden harness passes `QualitySetting::full()` explicitly. **That FULL is the pin.** Let a render path read the
  environment on its own and a developer's `LUDION_QUALITY=low` silently re-blesses — or reddens —
  every committed golden on their box, and a red image becomes a question about someone's shell
  rather than about the code.
- **`Quality` carries five booleans (`ssao`, `shafts`, `dof`, `gi_dynamic`, `shadows`) and nothing
  else**, with the counts in `QualityParams` beside it and the resolution in `RenderScale` beside
  both. The axes considered and rejected are recorded in `Plan/scalability.md` — bloom (calibrated
  imaging, masking it is a look change), the cluster grid dimensions, and the IBL prefilter sizes.

## Gotchas
- **A flag beats the environment without validating it.** `resolve_setting` returns on the explicit
  value before it ever looks at `LUDION_QUALITY`, deliberately: a box whose variable is malformed
  must still be drivable by passing a flag. A test that sets both and expects the environment's error
  is asserting the opposite of the contract (`an_explicit_value_beats_the_environment`).
- **Anything that is not a preset name is a path**, so a typo'd preset surfaces as a file that could
  not be read. The two readings are genuinely indistinguishable at that point, which is why there is
  one code and one message naming both — see
  `an_unresolvable_value_is_l0016_naming_both_interpretations`.
- **`Quality::MEDIUM` and `Quality::HIGH` are `Quality::FULL`.** What separates those three presets
  is `QualityParams`, so a test asserting that two presets' *toggles* differ passes vacuously for two
  of the four pairs. Assert against `LOW` when the point is that masking reached the frame, and
  against `QualityParams::MEDIUM` when the point is that a count did.
- **Authoring instruments render at `FULL` regardless of the machine.** The model and voxel contact
  sheets and `render --sweep` exist to be compared across boxes and against committed references; a
  per-machine setting leaking into them would make two agents' sheets disagree for a reason neither
  can see in the file.

## Verify
```
cargo test -p ludion-renderer quality          # presets, the preset-name/lookup drift test
cargo test -p ludion-renderer masking          # masking over all 48 derivable chains, shafts included
cargo test -p ludion-renderer a_masked_effect  # the byte-exact masked==unauthored proof (needs a GPU)
cargo test -p ludion-renderer a_lowered        # each count moves its own effect and nothing else (GPU)
cargo test -p ludion-renderer shadow           # the sun's map: sized, filtered, and maskable (GPU)
cargo test -p ludion-renderer gi_              # the GI pair: the cache key, the scaling identity, both axes on a frame (GPU)
cargo test -p ludion-engine quality            # precedence, the override file, every L0016 shape
cargo run -p ludion-cli -- dump-graph --scene examples/gallery/post-full.yaml --quality low
cargo run -p ludion-cli -- dump-graph --scene examples/gallery/post-full.yaml   # the same, unmasked
```
The two `dump-graph` runs are the cheapest end-to-end check that a mask reaches the graph: the masked
one must be missing exactly the `ssao` trio, the `shafts` pair, the `dof` pair, the `shadow` pass —
and, on a scene that authors GI with marked lights, the injection pass — and nothing else. It is also
where the resolution chain is read off: the `low` dump ends in `upscale` with `scene` at 0.75 of
`final`, and adding `--scale 2.0` to that same command replaces it with `resolve` at 2×, which is the
precedence in one pair of runs.
`cargo run -p ludion-cli -- test --dir examples/gallery` must stay green with `LUDION_QUALITY` set to
anything at all — if it does not, a render path is reading the environment.

A golden's bytes at `full` are the gate a parameter change has to clear, and SSIM 1.0000 is not that
gate. Render at the golden's own 960×540 and compare the files:
```
cargo run -p ludion-cli -- render --scene examples/gallery/post-full.yaml --out /tmp/x.png \
  --width 960 --height 540
cmp /tmp/x.png examples/gallery/goldens/windows/post-full.png
```
