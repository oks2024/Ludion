# Plan: Scalability — per-machine quality settings

> Status: **Waves 1–3 LANDED (2026-08-07 → 2026-08-08; Lore revs 537–542, 552–559, 563, 567 — see
> the wave log).** All nine quality axes are real and the render scale reaches below native. Wave 4
> (the editor's `reconcileWithGraph`, the docs split, the close ritual) is planned and unstarted; the
> macOS confirmation half of waves 2–3 is owed and queued in the private macOS queue.
> The arc belongs to the graphics-engine feature stream (the `shadow-mapping.md` /
> `volumetric-medium.md` / `clustered-forward.md` shape — a standalone plan, not a numbered engine
> phase).
>
> **The driver is concrete, not speculative:** development is moving onto a low-end laptop, and the
> engine currently has exactly one rendering configuration — everything the scene authors, at native
> resolution. A box that cannot afford that today has no lever at all short of editing the scene,
> which is the one thing a per-machine setting must never require: the scene is shared, the machine
> is not.
>
> **Relationships:**
> - `Plan/global-illumination.md` §0 reserved this system before it existed: the dynamic-injection
>   layer is ruled to be "a host-level quality knob in the `--scale`/`Engine::set_render_scale`
>   category" and must arrive as **the first citizen of a coming quality/scalability system**, not as
>   a one-off boolean. GI's bake resolution is named there as a second axis. This plan owns the
>   system; the GI arc claims its fields when its features ship (`gi_dynamic` is reserved, not built
>   here).
> - `Plan/post-process-pipeline.md` owns every effect this masks, and its calibration prose is why
>   bloom is *not* maskable (decision 6 below).
> - `Plan/clustered-forward.md` shipped the light path this plan deliberately does not touch: the
>   cluster grid's dimensions are a rejected axis (decision 6).

## Why (the one-paragraph case)

Three capability steps separate "runs on this box" from "does not": **disable** an effect the machine
cannot afford, **modulate** an effect's parameters so it runs cheaper rather than not at all, and
**render below native resolution** with an upscale. The engine has none of them — `RenderScale` goes
up (supersampling) and not down, every sample count is a shader `const`, and the only way to drop an
effect is to edit the scene, which changes it for everyone. All three steps share one question — *who
decides, and where does the decision live* — so they share one answer: a `Quality` value resolved
once per process from the machine's own environment, applied render-side, invisible to the
simulation. Wave 1 builds the resolution chain and the cheapest of the three steps; waves 2 and 3
grow the other two into the same value.

## What exists today (the inventory this plan builds on)

- **`PostChain::for_scene`** (`crates/ludion-renderer/src/graph.rs`) already answers "which passes
  does this scene need" in exactly one place, with five booleans (`ui`, `sun`, `ssao`, `shafts`,
  `dof`), and a false flag leaves the graph with *none* of that flag's passes. A machine-side mask is
  therefore one `&&` per flag away from being structurally free — the reason wave 1 is small.
- **`RenderScale`** (`graph.rs`) is a validated `[1, 4]` host factor with an `L4011` for out-of-range
  values, already documented as "a quality setting, never scene content". Its precedent — a host
  switch, not a wire command — is the one this plan copies for `Quality`.
- **`LUDION_THREADS` / `--threads` / `L0014`** (`ludion_core::par`) is the settled shape for a
  per-*machine* knob: environment by default, a host flag overriding it, malformed refuses to start.
  Quality is the second member of that family; nothing about the posture is invented here.
- **The sample counts are shader `const`s** — `SSAO_SAMPLES` 16 (`ssao.wgsl`), `SHAFT_STEPS` 32
  (`shafts.wgsl`), `DOF_TAPS` 32 (`dof.wgsl`), `gi_bake.wgsl`'s 64 rays — and each one already
  records in its own comment that its sequence is *well distributed at any prefix length*, so the
  count can move without re-deriving the sequence. That is wave 2's mechanism, already half-built by
  accident of good sequence choices.
- **`ProfileReport`** carries `SimProfile.threads` for exactly the reason quality needs a descriptor:
  two measurements taken under different machine settings are not comparable, and only the report can
  say so.
- **Golden images** are rendered through `ludion_testkit`'s one golden path, so there is a single
  place to pin the harness at full quality (decision 5).

## Decisions

1. **Config surface: four built-in presets plus an optional per-knob TOML override file** (settled
   with the user 2026-08-07). Presets are the common case and the thing a human can be told over
   chat; the file is the escape hatch for a box whose bottleneck is one specific effect. Names are
   `low` / `medium` / `high` / `full`, cheapest first. A setting's value is a preset name **or a
   path** — no third form, so the parser never has to guess.
2. **Precedence: `--quality` → `LUDION_QUALITY` → full, and a malformed setting refuses to start**
   (`L0016`). The `LUDION_THREADS`/`L0014` posture, for the same reason: a silently-ignored setting
   means rendering at a quality nobody chose, which is precisely the failure the knob exists to
   prevent. An explicit flag wins *without validating the environment*, so a flag still works on a
   box whose variable is broken.
3. **Per-machine, never a committed file.** How much GPU a box will spend is a property of the box.
   An override file is a local file the machine points at; the repo carries none, and no project
   manifest key selects one. The moment quality is committable it becomes a property of the project
   and two developers' goldens diverge for a reason neither can see.
4. **Quality is host-side and render-only — not a wire method.** `Engine::set_quality` is a host
   switch, the `set_render_scale` precedent verbatim: a connected client *sees* the setting through
   `engine.describe` and can never negotiate it. It is not a component, not a resource, never hashed
   into a `WorldHash`, never snapshotted or recorded — so an `.lrp` recorded on a `low` laptop
   replays bit-identically on a `full` workstation, and the sim-side determinism story is untouched.
5. **Default `full` is today's exact bytes, and the arc's golden-churn target is zero.** Masking is
   the identity at `full`, so a renderer that was never told a quality draws what it always drew. The
   golden harness passes `Quality::FULL` **explicitly** rather than relying on the default — that
   explicit FULL *is* the pin, and it is what makes a red golden a statement about the code instead
   of a question about someone's shell.
6. **Rejected axes, with reasons, so they are not re-proposed:**
   - **Bloom (a toggle, or fewer pyramid levels)** — bloom is the imaging chain, not an optional
     effect, and `bloom_strength`'s default is calibrated against the six-level pyramid's energy.
     Masking it is a *look* change, and "the same scene at two qualities" would stop being true.
   - **Cluster grid dimensions** — the 16×9×16 grid is fixed-count precisely so it is `--scale`
     invariant; making it a quality axis reintroduces the per-machine variation that decision bought
     out, for a cost that measures in the hundredths of a millisecond.
   - **AA mode** — decision 5's zero churn: a preset moving it would move every golden's look and
     cost. Anti-aliasing is a host setting beside `--scale` (`--aa`), off by default.
   - **IBL prefilter sizes** — the prefilter is cached by content hash and paid once at load, so
     shrinking it buys frame time that was never being spent.
   - **Authoring instruments** — the model/voxel contact sheets and `render --sweep` stay at FULL
     unconditionally. They exist to be compared across machines and against committed references.
7. **One profile-schema revision for the whole arc, with the descriptor defined completely up
   front.** The report's `quality` block carries the label, the three toggles, *and* every parameter
   axis waves 2–3 will modulate, reported from today's constants. Defining the shape once costs one
   replay re-record now instead of one per wave, and it means a wave-2 measurement taken before the
   knob moved is still comparable with one taken after.
8. **Naming discipline: "quality preset" and "quality settings", never "tier".** "Tier" is taken —
   `engine-pillars.md`'s *tiered determinism* (bit-exact CPU / perceptual images / pinned-GPU
   goldens) is the vocabulary a reader already has, and a second ladder wearing the same word in the
   same renderer would make every sentence about either one ambiguous.
9. **Preset *values* come from measurement, not from taste.** Wave 2 profiles each preset on both
   pinned boxes and guards `low` with `image-stats` (banding/aliasing/clipping are what a
   too-aggressive preset actually produces), then re-tunes on the low-end laptop once it exists. The
   presets shipped before that measurement are deliberately coarse: `low` is everything off, and
   `medium`/`high` differ from `full` by name only, which is stated in the type's own docs rather
   than papered over.

## Decisions Proposed (0014, pending owner approval)

Serious Isaac request 0014 (`engine-requests/0014-desktop-packaged-quality-default.md`). Two amendments, ready
to build; neither moves a wire schema hash, a replay content hash, a `.lrp` or a `full` golden.

**P1. A desktop packaged default, chosen by GPU class (amends decision 3).** Decision 3's last clause becomes:
*a project manifest may name a **preset** per GPU class, never a file.* The `[ios] quality` precedent, on desktop.

- **Manifest.** A `[quality]` table, `QualitySection { apple_silicon: Option<String> }` under
  `deny_unknown_fields` (`crates/ludion-engine/src/manifest.rs`). An unknown key fails the manifest load with
  `L0023`; toml's message names the expected key, and `load`'s help text gains the `[quality]` key. The value is
  validated as `ios_quality` validates (`manifest.rs:510`): preset names only, `L0023` otherwise, a file path
  included. One helper serves both keys. `Manifest::packaged_quality(class) -> Result<Option<&'static str>>`.
- **Class detection.** `GpuClass { AppleSilicon, Other }` comes from the adapter's `wgpu::AdapterInfo`, at
  runtime, with no `cfg(target_os)`. The rule: backend `Metal`, and a name starting with `Apple ` (the
  `MTLDevice` name, such as `Apple M4 Pro`). Two other signals were rejected. On Metal, wgpu-hal 30 leaves
  `vendor` at 0. `device_type` is `IntegratedGpu` on Intel Iris Macs too, and an Intel Mac must stay `Other`.
  A software adapter is `Other`. The classifier is a pure function of (backend, name) in `ludion-render-graph`,
  beside `Gpu::request` (`execute.rs`).
- **Timing.** Quality resolves in `run_short_circuit` (`crates/ludion/src/lib.rs:626`), before any mode runs
  and before any device exists. That stays. A new `probe_gpu_class()` asks for an adapter with `Gpu::request`'s
  options (the headless tiers, `HighPerformance`), reads its info, and creates **no device**. It runs only
  when nothing above it chose: no flag, no `LUDION_QUALITY`, and a manifest with a `[quality]` entry. A box
  that sets either, and every project without the table, pays nothing and behaves as today. On a
  `LUDION_FORBID_GPU` lane the probe is skipped and the class is `Other`. Headless and windowed hosts share
  the probe. A windowed host later picks its adapter with a compatible surface, but Apple silicon has one
  GPU, so only a dual-GPU Intel Mac could see a different adapter, and that machine is `Other` either way.
- **One resolution.** `ludion::run` loads `<content>/ludion.toml` once when it exists (and reuses it for
  `check_default_scene`). It computes the packaged value once and passes it to both resolution sites:
  `run_short_circuit`'s `resolve_startup_quality`, and `StartOptions.packaged_quality` for hosting
  (`embed.rs:406` already takes it). New behaviour: a malformed manifest now also refuses `run` with `L0023`,
  as it already refuses `export` and `sync`.
- **Scope.** The game host only: `ludion::run`, and `ludion dev` through it. iOS keeps `[ios] quality`
  through `launch` and `ffi`; `[quality]` is never read there. The CLI instruments (`render`, `profile`,
  `serve`) ignore it under decision 6, and goldens keep their explicit `FULL` pin under decision 5.
- **Reporting.** `QualitySetting` gains `source: QualitySource { Default, Flag, Env, Packaged }`, the existing
  private `Origin` promoted. `engine.describe`'s `quality` object gains `"source"`, plus `"gpu_class"` when the
  source is packaged. The describe body is untyped JSON, so the schema hash does not move. The `--profile`
  report's typed `QualityProfile` is in the wire schema, so it gets **no** new field: its `preset` already
  names the preset that won. The host prints one stderr line when the packaged value wins (`quality: medium
  (packaged, [quality] apple_silicon)`). A typed `source` in the report would move the schema hash once; it is
  owed only if asked for.
- **Tests.** Manifest units: a valid entry, a non-preset value (`L0023`), a file path (`L0023`), and an
  unknown key refused with `apple_silicon` named. Classifier units: `Apple M4 Pro`/Metal → `AppleSilicon`;
  Intel Iris/Metal, AMD Radeon Pro/Metal and NVIDIA/Vulkan → `Other`. A host-level test drives the packaged
  helper over a temp manifest with an injected class: the class's entry is chosen, `Other` falls through to
  `full`, and a flag and `LUDION_QUALITY` outrank it. The existing
  `a_packaged_default_ranks_below_the_flag_and_the_environment` stays. A describe test checks `source`.
- **Docs.** `ludion docs quality` §Rules: the key, its precedence slot and the class rule.
  `ludion docs export` gets a one-line cross-reference.

**P2. `ssr_scale` 0.25 at `medium` (amends the wave-2 preset table).** `QualityParams::MEDIUM.ssr_scale`
becomes `0.25`, which equals `RenderScale::MIN`, the override file's floor. `high` and `full` keep `0.5`.

- **Mechanism.** One constant (`crates/ludion-renderer/src/quality.rs:386`). The trace and pyramid extents
  already read it (`ssr_pyramid::trace_extent`, `graph.rs:1134`), so no shader changes. `LOW` inherits it
  through `..MEDIUM`, but `LOW` masks `ssr` off, so its descriptor reads `0.25` for a trace that never runs.
  That is the "repeat `medium`'s" convention `LOW`'s doc already states.
- **What moves.** Nothing at `full`: the goldens are pinned `FULL`, the `mirror-floor` budgets measure `full`,
  and quality is never hashed or recorded (decision 4). At `medium` and `low`:
  - the profile descriptor's `ssr_scale`;
  - `probe_store::world_key`, which hashes `QualityParams` through `Debug`. Store entries keyed at those
    presets miss and recapture, and none is committed. The face capture forces `post.ssr = 0`, so the
    `medium` probe-bake pin's pixels stay put.
  - vigil's iOS export (`[ios] quality = "medium"`), which traces SSR at 0.25 where a scene opts in.
- **Tests.** In `quality.rs`, `MEDIUM.ssr_scale < HIGH.ssr_scale`; the monotonic test already covers it. A
  graph test checks that the `hiz`/trace extent at `medium` is output × 0.25.
- **Docs.** `ludion docs quality`'s `ssr_scale` row becomes `0.25` at `medium`/`low` and `0.5` at
  `high`/`full`. `dev-docs/renderer-quality.md` and `renderer-ssr.md` get the same edit.
- **Owed after it ships.** An M4 `profile --quality medium` on `mirror-floor` to record the saving, and a look
  check of SSR at 0.25 (decision 9: preset values come from measurement).

## Waves

### Wave 1 — the value, the chain, and the toggles (LANDED)

The `Quality` type, the host resolution chain, `PostChain::masked`, the CLI and `RunArgs` flags, the
profile descriptor, and this plan. See the wave log.

### Wave 2 — parameter modulation (planned)

The step that makes `medium` and `high` mean something: effects that run *cheaper* rather than not at
all. Each axis becomes a value the shader reads instead of a `const`, so a preset moves it without a
pipeline permutation.

| axis | today | mechanism | note |
| --- | --- | --- | --- |
| `ssao_samples` | 16 | uniform loop bound | R2 gives the *direction*; `mix(0.1, 1.0, f*f)` with `f = i/N` re-spreads the radii over the same range |
| `shaft_steps` | 32 | uniform loop bound | `dt = end / steps` already rescales the march, so the beam's *length* is unchanged |
| `dof_taps` | 32 | uniform loop bound | golden angle gives the *angle*; `sqrt((i+0.5)/N)` re-spreads the radii over the same disc |
| `shadow_map_size` | 2048 | CPU-side (the texture descriptor) | no shader change at all |
| `pcf` | 3×3 | uniform radius | the tap loop is already a nested `for` |
| shadows | always on with a sun | a toggle | needs the fallback-binding work: the scene pass binds a shadow map whether or not one was rendered |
| `gi_rays` | 64 | uniform loop bound | **verified 2026-08-08 (rev 552)** — see below; the count joins the bake cache key and `CACHE_STRIDE` becomes derived |
| `gi_probe_scale` | 1.0 | multiplies the authored spacing | the authored spacing stays the look; quality scales it (GI plan D4) |

**The N-question, answered — and the answer corrects this plan's own premise.** The table above originally
reasoned from *prefix truncation*: a low-discrepancy sequence stays well distributed at any prefix, so a
smaller count keeps the first `N` samples of the same set. That is **not** what any of these four axes
does, and the real shape is uniform across all of them: a genuinely count-free *angle or direction*
sequence (R2, the golden angle, the Fibonacci azimuth) paired with a *magnitude* term that divides by
the count and therefore **re-spreads** the reduced budget over the same domain. Re-derivation, not
truncation. For the three post axes the distinction is invisible — the domain is a radius range or a
march length either way. For `gi_rays` it is load-bearing, because the elevation term
`1 - 2(i + 0.5)/RAYS` means ray `i` of a 32-ray set is nowhere near ray `i` of the 64-ray one (measured:
up to 1.24 apart on a diameter-2 sphere).

Each count is nevertheless a complete, well-formed sphere on its own terms — pinned as properties in
`gi_bake.rs` at 16/32/48/64 rays: mean direction under 0.01 (measured 0.00008–0.0018, where a one-sided
set would be O(1)), every open hemisphere within 2 rays of half on all three axes, and the closest pair
at a near-constant **1.55× the packing bound `acos(1 - 2/N)` at every count**. Distribution *quality* is
the thing that is count-invariant, which is the property the mechanism actually needed. The test is
proven able to fail (pinning the denominator at 64 reddens the 16-ray case to a mean direction of 0.750).

**Three constraints follow, and they are the GI axis package's real content:**

1. **The bake result changes wholesale with the count**, so `gi_rays` joins the bake cache key and no two
   counts' probes are byte-comparable. Any GI test asserting exact probe values is a FULL-only test.
2. **`gi_inject.wgsl` uses `RAYS` as a denominator, not a loop bound** — it replays the bake's cached
   hits, so it must divide by the count the bake actually traced. Plumbed from one source or it silently
   replays directions the march never took.
3. **`CACHE_STRIDE` becomes a derived `RAYS / HIT_RAYS`**, not a constant 4 — which makes *"`gi_rays` is a
   whole multiple of `HIT_RAYS` (16)"* a constraint on the axis's permitted values rather than a
   coincidence of the provisional table. All four values satisfy it, at strides 1/2/3/4.

Contrast with the failure `CACHE_STRIDE` exists for, since the two are easy to conflate: a contiguous
16-prefix of a **fixed** 64-ray set spans `y ∈ [0.516, 0.984]` — the 59° polar cap — while a natively
16-ray set spans `[-0.938, 0.938]`. Re-derivation is not that failure.

**The escape hatch, decided in advance:** a uniform loop bound can move `full`'s bytes on a pinned
GPU even when the arithmetic is identical, because the generated code changes (the volumetric wave-3
precedent, restated by `clustered-forward.md`'s risk section). If that happens, the axis becomes a
**shader permutation** through `compose_parts` — the full-quality variant keeps a literal `const` and
its bytes — rather than accepting a golden re-bless. The zero-churn target (decision 5) outranks the
elegance of one pipeline.

**Provisional preset values — pending measurement, not settled.** These are the starting points wave
2 measures *against*, recorded so the sweep has a hypothesis and not so it has an answer:

| axis | low | medium | high | full |
| --- | --- | --- | --- | --- |
| `ssao_samples` | (off) | 8 | 12 | 16 |
| `shaft_steps` | (off) | 16 | 24 | 32 |
| `dof_taps` | (off) | 12 | 20 | 32 |
| `shadow_map_size` | 1024 | 1024 | 2048 | 2048 |
| `pcf` | 1×1 | 3×3 | 3×3 | 3×3 |
| `gi_rays` | 16 | 32 | 48 | 64 |
| `gi_probe_scale` | 2.0 | 1.5 | 1.0 | 1.0 |

Exit for the wave: a `ludion profile` run per preset on both pinned boxes, `image-stats` on `low`'s
output showing no new banding or aliasing defect class, and every `full` golden byte-identical.

### Wave 3 — sub-native rendering (LANDED 2026-08-08, Lore revs 563 and 567)

- **`RenderScale` extends to `[0.25, 4.0]`**, keeping `L4011` and its message shape — one range, one
  code, no second concept for "below native".
- **A new `upscale.wgsl`**, bilinear. The existing `resolve` pass is a box filter over a supersampled
  buffer; a box filter *degenerates* under magnification (it becomes nearest-neighbour), so
  magnification needs its own shader rather than a reused one.
- **The sizing rule is unchanged** and that is the point: only the scene pass's colour/depth targets,
  the AO prepass and `ldr` take the render extent; everything else takes the output extent. That rule
  is exactly what keeps "a scale change is a quality change, never a look change" true *below* native
  as well as above it — bloom radius and beam width stay a fixed fraction of the frame.
- **UI stays native-crisp structurally** — the overlay pass already runs at the output extent — plus
  a test that proves it, because "structurally true" is what silently stops being true.
- **Precedence is explicit:** an explicit `--scale` beats a quality-supplied one, which beats native.
  `low` carries 0.75.

### Wave 4 — the editor stops lying, and the close (planned)

- The editor's `reconcileWithGraph` compares the effect chain it believes in against the graph the
  host actually built. With masking live, a host at `low` makes it report a mismatch that is not one;
  it must read `describe`'s `quality` object and mask its own expectation the same way.
- **No editor quality controls.** Quality is per-machine and host-side; the editor is an
  out-of-process peer that may not even be on the same box as the host it attached to.
- Close ritual: Status flip, notebook, CLAUDE.md's Current state, and the docs topics re-checked
  against what waves 2–3 actually shipped.

## Open questions

- **Above-`full` values.** Nothing currently expresses "more than the scene asked for" — more AO
  samples, a larger shadow map, more GI rays than the bake authored. A `ludion render` used for
  marketing stills or a golden re-bless might want it. Deferred: it would make `full` no longer the
  ceiling, and every "masked equals unauthored" sentence would need a second half.
- ~~**Whether `high` earns its keep.**~~ **RESOLVED 2026-08-08 (wave 2's sweep, RTX 3090): keep it.**
  On post-heavy content `high` costs 87% of `full`'s GPU pass total (0.564 vs 0.645 ms on
  `post-full`) — a real 13%, well outside noise. On scenes authoring none of ambient occlusion, light
  shafts or depth of field it is *identical* to `full` (`sun-shadows` and `gi-interior` both 100%),
  and that is structural rather than a defect: `HIGH` differs from `FULL` only in the three post
  counts plus `gi_rays`. The deletion test was "within noise of `full`", and on the content that has
  the effects it is not. The M4 has not confirmed this half.
- **Per-effect `half_post` scaling at sub-native.** Wave 3 scales the whole render extent; a
  finer-grained "bloom and DoF at half the render extent, the scene pass at full" is a different
  trade and interacts with the sizing rule that wave 3 deliberately leaves alone. Not opened until
  something measures.
- **The laptop re-tune.** Every preset value in this plan is derived from the RTX 3090 and the M4.
  The machine the arc exists for has not been measured, so the values are provisional twice over —
  once for wave 2's sweep, once for the hardware.
- **`gi_rays` has no instrument, and this is the sweep's one genuinely unwelcome finding.**
  `ludion profile` never reports the bake as a timed pass — it is cached and runs outside the
  measured window (confirmed at `--frames 1`) — so the axis's entire benefit, bake time and probe
  memory, is unmeasured and ungated. One of the eight axes is invisible to the arc's own instrument.
  This is not merely untidy: the laptop re-tune above *cannot evaluate this axis at all* as things
  stand, so the choice is to grow the profile report a bake-cost line or to accept that `gi_rays`'s
  preset values stay a reasoned guess. Decide before the re-tune, not during it.
- ~~**`low` roughly triples edge aliasing.**~~ **RESOLVED 2026-08-08 — and the prediction in the
  second half of this entry was wrong in direction.** Wave 2 measured `post-full` at 1080p: banding
  flat (0.0816 → 0.0857), `edge_aliasing` 0.1088 → 0.3905, caused structurally by depth of field
  being masked off, exactly as "masked equals unauthored" promises. This entry then predicted wave 3
  would make that *worse*. It does the opposite. Shipped `low` (0.75 + the bilinear upscale) measures
  banding **0.1141** and `edge_aliasing` **0.0003** — below `full`'s own 0.1088 — while `low` forced
  to `--scale 1.0` reproduces wave 2's numbers exactly (0.0857 / 0.3905), so the render scale is the
  only variable. The magnifier is a low-pass filter and it erases the aliasing DoF-masking created.

  Three things follow. **`image-stats` cannot distinguish "well-resolved" from "blurred"**, so a
  near-zero `edge_aliasing` on a sub-native capture is not a quality claim and will be read backwards
  by anyone not told. The committed defect gate is inherently *full-quality* anyway (it is checked
  against the goldens, which the golden pin fixes at `full`). And **the cost moved to banding**, +33%
  over `full` — the largest in the arc — most likely because the output dither is applied in
  `composite` at the *render* extent, so below native it is magnified and stops breaking up 8-bit
  quantization at output-pixel granularity.

- **Should the output dither move into the `upscale` pass?** Raised by the banding above. It would
  cost `full` nothing — that pass does not exist at native, so the zero-churn target is untouched —
  and it is the natural home for a dither that must land at output-pixel granularity. Not taken in
  wave 3, which deliberately left the sizing rule and the effect chain alone. Decide with the laptop
  re-tune, where the trade is actually felt.
- **`gi_dynamic`.** Reserved by `Plan/global-illumination.md` §0 as this system's named first citizen;
  the field is claimed by the GI arc when the dynamic layer ships, not by this plan.

## Risks

- **A uniform loop bound moves `full`'s bytes.** Likely enough to plan for; the answer is decided in
  advance (wave 2's escape hatch) so it does not become a re-bless negotiation mid-wave.
- **The descriptor is defined before the knobs exist**, so it could describe an axis wave 2 then
  implements differently. Mitigated by the axes being *today's constants* reported verbatim — the
  descriptor is a measurement of the build, not a promise about a design.
- **The shadows-off toggle is the one masking case with real work behind it.** Every other effect's
  passes simply leave the graph; a sunless-but-shadow-mapped scene needs the fallback binding the
  scene pass currently gets for free from `PostChain::sun`. Scoped into wave 2 with that named.
- **`medium` and `high` shipping as aliases of `full` invites a vacuous test.** A test asserting two
  presets render differently passes for two of the four pairs by coincidence. Recorded in
  `dev-docs/renderer-quality.md`'s Gotchas rather than left to be rediscovered.

## Wave log

### Wave 3 — landed 2026-08-08 (sub-native rendering)

Every bullet above, built. `RenderScale` is `[0.25, 4.0]` with a public `MIN`, one `L4011` whose
explain text now closes both ends for stated reasons; `upscale.wgsl` is a bilinear magnifier written
in `resolve.wgsl`'s idiom (`textureLoad`, exact texels, the half-texel offset written down once
rather than trusted to a sampler); `render_scale` is the ninth quality axis at `0.75` for `low`; and
`resolve_render_scale` is the precedence chain.

**Three things worth not re-deriving:**

- **`FrameSize::resample()` returns the pass label**, because the label is the entire difference
  between the graph tail's two branches — `supersampled()` narrows to mean strictly *larger* and is
  defined in terms of it. The mixed extent pair is **asserted rather than typed away**: no
  constructor produces one (`scaled` applies one factor to both axes, and rounding can only take an
  axis back *onto* the output extent), and sealing the two `pub` extents behind accessors would tax
  every reader to exclude a case that does not occur.
- **The precedence resolves on read, not on write.** The `Engine` holds the explicit scale as an
  override beside the `QualitySetting`, so neither setter writes the other's field and
  `set_quality`/`set_render_scale` commute. `resolve_render_scale` is a free function rather than a
  method because two of its three callers build a `FrameSize` with no `Engine` at all.
- **The CLI's `--scale` had to become absent-able.** Defaulting to `1.0` made "unset" and "the user
  asked for native" the same value, which would have stomped every quality-supplied scale on every
  invocation.

**Exit, Windows half.** Zero golden churn (the gallery suite is 20/20 at SSIM 1.0000 with
`LUDION_QUALITY=low` in the environment — the FULL pin holds by name, not by luck); `post-full`
@1080p GPU total 0.649 ms at `full` → **0.175** at `low`, against 0.196 for Wave 2's native `low`;
`dump-graph --quality low` shows `scene`/`ldr` at 1440×810 with an `upscale` tail, and an explicit
`--scale 2.0` beats it back to `resolve`. Authoring instruments verified byte-identical at `low` and
`full` rather than assumed. **The macOS half is owed**, as Wave 2's is.

**This plan's own prediction about aliasing was wrong, and the measurement is clean.** See the
resolved Open question below.

### Wave 2 — parts 1 and 2 landed 2026-08-08 (the post axes; the shadow axes)

**Part 1, the params spine + `ssao_samples`/`shaft_steps`/`dof_taps` (Lore rev 555).** `QualityParams::CURRENT`
is deleted in favour of per-preset `FULL`/`HIGH`/`MEDIUM`/`LOW` consts plus a `preset()` lookup held
against `Quality::PRESET_NAMES` by a test, carried on `QualitySetting`, threaded by
`Renderer::set_quality(quality, params)` — one call for both halves, so a renderer can never hold one
setting's toggles beside another's counts.

**The escape hatch was never spent.** The wave's stated risk — a uniform loop bound moving a pinned
GPU's bytes — did not materialise, in either part, and the reason is worth recording as technique
rather than luck: **each new `u32` absorbed a pad lane its uniform struct already carried**, so every
struct's size and every prior field offset stayed unchanged (`Ssao` 160, `Shafts` 192, `Dof` 16;
`Sun` did grow 96 → 112, still with every prior offset fixed). Every Windows golden stayed
byte-identical across both parts. One pipeline stands; `compose_parts` was not needed.

The load-bearing implementation rule, which cost nothing to state and would have been expensive to
discover: **every use site reads the uniform, not just the loop bound.** Each estimator divides by its
count somewhere — SSAO's radius ramp and its final divisor, the shaft `dt`, DoF's spiral radius and
its coverage term, PCF's squared divisor. A divisor left holding a literal is not a cheaper estimate
but a *wrong* one, and no golden catches it because `full` stays exact. A test counts the read sites
per shader and separately asserts the old consts are gone.

**Part 2, `shadow_map_size` + `pcf` + a fifth toggle (Lore rev 557).** The package's real content was
a refactor, not the axes: `PostChain::sun: bool` became `shadow_map: Option<u32>` (the `gi_inject`
precedent — a resource's extent *is* graph structure) and `graph::SHADOW_MAP_SIZE` was deleted rather
than left beside it. The old boolean conflated *the scene carries a sun* with *this frame draws a
shadow map*, and only the second is a machine's business. The sun's light needs no graph structure at
all, so masking shadows yields a **sunlit shadowless** frame, never a sunless one.

Two decisions worth not re-litigating:
- **Masking shadows also masks shafts.** The march reads the shadow map; keeping the map alive for the
  shafts alone would mean "shadows off" still schedules the `shadow` pass, i.e. masking would mean
  *weakening* instead of absence — the one thing the masking invariant forbids.
- **`build_sun` zeroes `light_space` exactly when the frame has no map**, reaching `mesh.wgsl`'s
  existing fully-lit early-out. Without that zeroing the scene pass comparison-samples the 1×1
  fallback depth and shadows the world *solid* — the failure the guard test is aimed at.

**Further additive revision (Clockout request 0009):** the descriptor gains `quality.anti_aliasing`
(the mode the host resolved; absent reads `off`), moving the wire hash to `3d3a318c5d8a9d00`. A
budget check refuses a report whose mode is not `off`, as it refuses a non-`full` preset (`L4020`).

**Deviation from decision 7, taken deliberately: a second profile-schema revision**
(`e49517e93ed32dd8` → `3b840452912903dc`, purely additive). Decision 7 budgeted one revision for the
whole arc and was written before a fifth `Quality` toggle existed. `shadows` is not derivable from
anything else the descriptor carries — `shafts: false` says the same thing for a scene that never
authored shafts, and an override file reports `custom` for `preset` — so without the field a capture
could report a 2048-texel shadow map for a frame that drew none. That is precisely the class of lie
part 1 spent its effort removing from the descriptor, and it outranks the rev budget. The three
replays were re-recorded and **verified at the decode level rather than byte-wise** (the
`ReplayHeader` is zstd'd JSON, so a changed hash re-compresses the whole header frame): every header
field identical but `schema_hash`, and 13041/13041 + 32/32 + 456/456 records identical.

Two tests were caught going vacuous as the axes accumulated, and the pattern is worth expecting again:
a test comparing `FULL` against `MEDIUM` on a fixture carrying a sun would have passed no matter what
the axis under test did, once `MEDIUM` also shrank the shadow map. The masking walk is now 48 chains
rather than 64 and asserts the count, because shafts without a shadow map is not a chain any scene
derives.

**Part 3, `gi_rays` + `gi_probe_scale` (Lore rev 559).** The gate's three consequences, built. The
ray count travels **writer→reader rather than being read twice** — `QualityParams` → `BakeInput` → a
reclaimed pad lane of `GiBakeParams` → `ResolvedGi::rays` → the injection — so `gi_inject.wgsl`
divides by the count the march behind *its own hit cache* traced, structurally, and not by whatever
the machine happens to be set to now. `CACHE_STRIDE` is deleted from both shaders and derived.
`LOW` stops being an alias of `MEDIUM` here, on an asymmetry worth naming: `gi_dynamic: false` masks
the *injection*, but the bake still runs, so both GI axes are observable at `low` — unlike
`shadow_map_size` and `pcf`, whose pass leaves the graph entirely.

The scene-free lattice scaling is the part worth reading twice. `masked` cannot re-derive a lattice
from the grid box, so it scales the *count*: re-laying `count` probes at `scale×` the spacing needs
`ceil((count-1)/scale) + 1`, and **the spacing cancels out of that arithmetic entirely**. That
cancellation is what lets a scene-free function answer the question, and it is why
`PostChain::for_scene`'s signature did not have to change. At `1.0` it is the exact identity, which is
where this axis's zero-churn rests.

**Exit criteria, Windows half (2026-08-08).** Sweep run in release at 120-frame medians, GPU pass
totals: `post-full` 0.645 / 0.564 / 0.438 / 0.196 ms across full/high/medium/low (100 / 87 / 68 /
30 %), `sun-shadows` 100 / 100 / 97 / 76 %, `gi-interior` 100 / 100 / 99 / 87 %. Every `full` golden
byte-identical, verified after each of the three packages. `image-stats` on `low` and the `high`
question are written up under Open questions, both resolved there. Vigil's own scenes cannot be swept
this way — `profile --scene` links no game code, so `district-plaza.yaml` is `L1013` on `Player` — so
a real-workload sweep needs `serve` + `--attach` or vigil's budget harness. **The macOS half is
owed**: the M4 has re-verified neither FULL's bytes for the five touched shaders nor any preset's
cost.

### Wave 1 — landed 2026-08-07 (the value, the chain, the toggles)

- **`ludion_renderer::Quality` is three booleans and nothing else** — `ssao`, `shafts`, `dof` — with
  `FULL`/`HIGH`/`MEDIUM`/`LOW` consts, `PRESET_NAMES` (the list a host prints when it rejects one),
  and a case-insensitive `preset()` lookup that a test holds against `PRESET_NAMES` so the advertised
  list and the lookup cannot drift. `Default` is `FULL`. The module takes no serde derive and reads no
  file: resolving a quality belongs to the host that owns the sources, which keeps the crate free of
  I/O exactly as `RenderScene` keeps it free of asset paths.
- **`PostChain::masked` is the whole application site**, and it ANDs only the three effect flags —
  `ui` and `sun` pass through untouched. Proven at both altitudes: `masking_is_the_identity_at_full_
  and_reaches_only_the_effects_at_low` walks all 32 chains a scene can derive, and the GPU test
  `a_masked_effect_renders_exactly_as_an_unauthored_one` renders a fixture authoring AO, shafts and
  DoF at `LOW` and byte-compares it against the same scene with those three strengths at zero at
  `FULL` — with a per-effect loop proving each one moves the frame, so the equality is three effects
  being *skipped* rather than three fields never being read.
- **`ludion_engine::quality` is the host half**: `resolve_startup_quality` (the only reader of
  `LUDION_QUALITY` anywhere in `crates/`), the `deny_unknown_fields` `OverrideFile`/`Effects` schema,
  and `L0016` for every way a setting can fail to mean a quality — unknown preset, unreadable path,
  malformed TOML, wrong value type, unknown key or section. One code, one message naming *both*
  interpretations, because a typo'd preset and a missing file are genuinely indistinguishable. The
  pure half (`resolve_setting`) takes the environment value as an argument, so the precedence and the
  parsing are testable without mutating the process environment.
- **`QualitySetting` keeps the label**, and that is not decoration: three of the four presets resolve
  to identical toggles, so a `Quality` alone cannot answer "which setting did this machine ask for" —
  the question `describe` and the profile report both have to answer. An override file labels itself
  `custom`.
- **`engine.describe` carries a read-only `quality` object** (label + the three toggles). A client
  sees, never negotiates — the `--scale` posture: quality is a host switch, not a protocol command.
- **The golden pin is explicit, not incidental.** `ludion-testkit`'s golden path passes
  `Quality::FULL` by name with the reason written beside it. Every golden stayed byte-identical
  through the wave.
- **The CLI/`RunArgs`/descriptor package landed beside it**: a global `--quality` on the `ludion`
  binary (meaningful to `render`, `serve`, `profile`, `dump-graph`), the same flag on a shipped game's
  `RunArgs` so `ludion::run` gets it for free, a `quality` descriptor in the profile report (label,
  toggles, and the parameter axes reported from today's constants — `ssao_samples`, `shaft_steps`,
  `dof_taps`, `shadow_map_size`, `pcf`, `gi_rays`, `gi_probe_scale`), and `profile --check` refusing a
  non-full capture, because the budgets are calibrated against everything the scene authors. That is
  the arc's one profile-schema revision and its one replay re-record (decision 7).
- **Docs:** `ludion docs quality` (the setting, the presets, the file schema, `L0016`) and
  `ludion devdocs renderer-quality` (the mechanism, the masked==unauthored invariant with its test
  names, the FULL golden pin, and the three homes a new field must reach in one change).
