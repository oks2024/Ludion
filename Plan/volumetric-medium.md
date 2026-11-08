# Plan: The Coherent Volumetric Medium (fog + light shafts as one)

> **CLOSED — shipped 2026-07-30; see the wave log's "Wave 4 delivered 2026-07-30 — mechanized half.
> The exit sitting is the one thing still open." Owed: that volumetric-medium exit sitting — the
> game-side camera/geometry that frames the air, deferred by user direction.**
> Status: **ACCEPTED 2026-07-29** (user direction; proposed 2026-07-26). Direction chosen by the user
> on proposal: merge fog and light shafts into one physically-coherent participating medium, built as a
> **full single-scattering PBR medium** (explicit single-scatter albedo + sky/IBL-sourced ambient
> in-scatter, not the minimal shared-σ_t set), with the existing `fog_*` / `shafts` field names kept
> (no `medium_*` rename). Successor plan to `Plan/post-process-pipeline.md` (the `environment-ibl.md`
> / `shadow-mapping.md` shape — a standalone plan under the graphics-engine feature stream, not a
> numbered engine phase). It runs **after** the post-process arc closes (wave 12), because it
> re-authors `post-full.yaml`'s fog/shaft block and re-blesses that golden, and the arc's own closing
> sitting should judge the shipped look first.
>
> **This plan supersedes two recorded post-process decisions**, and says so up front so the record
> stays honest:
> - **Wave 6's "fog is analytic, full stop."** Fog stays analytic *for its ambient term*; the medium
>   grows a directional (marched) term that is the shafts, and (Wave 3) an IBL-sourced ambient term.
>   Fog is no longer a standalone effect — it is one part of the medium.
> - **Wave 8's two-density split** (`shafts_density` deliberately separate from `fog_density`, "one
>   field meaning two things would make either un-authorable without the other"). That objection is
>   *answered*, not overruled: the merge keeps orthogonal fields (extinction, albedo, directional
>   intensity, anisotropy) and deletes the **redundant** one — the second extinction. See "The model".
>
> **Wave log — wave 1 delivered 2026-07-29.** Full `check.mjs` ALL GREEN; both `profile --check`
> invocations and `image-stats --check` exit 0. Amendments to the wave as written:
>
> - **The shaft gate went from two conditions to three, against the Risks section's "the gate is
>   unchanged".** That line addressed only the opposite hazard (getting a march you used to mute). Once
>   σ_s = α·σ_t, a scene with `fog_density: 0` marches 32 steps and runs a 4×4 blur to produce an exact
>   zero — and worse, `shafts: 0.19` on a sunlit scene with no medium became authored, validated,
>   silently inert **and reported live** by the editor, which is the precise trap `chain.ts` exists to
>   expose. `PostChain::for_scene` now requires strength **and** sun **and** extinction: measured 16 → 14
>   passes (two transient targets gone) on a shafts-without-medium frame.
> - **The editor models the medium as an *effect-level* precondition, not a scene one.** `requiresEffect:
>   'fog'` beside the existing `requires: 'sun'`, because "is there a medium" is already spelled exactly
>   once in that module as fog's own gate; a `'medium'` `Precondition` value would have put a second
>   `fog_density > 0` threshold in the file to drift from the engine's. It reads *engaged* rather than
>   *live*, and engagement counts only **gate** fields — so `fog_albedo: 0.5` with no density does not
>   fake a medium. `missing-sun` outranks `missing-medium` when both are absent.
> - **α = 1 keeps the `mix` intrinsic** rather than the algebraically equal explicit sum: `mix` lowers to
>   each backend's own blend (HLSL `lerp` is `x + s*(y−x)`, not `x*(1−s) + y*s`), so folding the algebra
>   would have moved the shipped fog's bytes. Decision 9's skip rule one level down.
> - **Decision 7 held — no new L-code.** schemars' `range` cannot express an exclusive bound, but
>   `ludion-scene`'s validator already honours `exclusiveMinimum`/`exclusiveMaximum`, so `fog_anisotropy`
>   carries them via `#[schemars(extend(...))]` and a breach is the ordinary `L1014`. Pinned by rejection
>   rows at the endpoints themselves (`±1.0`).
> - **`post-full`'s `shafts` stayed 0.02** rather than rising to the 0.03 that would have reproduced the
>   old +3.3/255 lift exactly: the beams now reach 3/0.014 = 214 units instead of stopping at 30, so the
>   ray tail past the colonnade is unshadowed fill, and the quieter value keeps the pillars' shadow bands
>   the darkest thing in the foreground. Neighbours re-measured on the same build — haze +2.42, halation
>   +2.62, vignette −2.22 — put 0.02 (+2.27) inside the band where the old configuration sat 1.4× above
>   it. **Vigil's went 0.19 → 0.34**, restoring the beams' shipped +0.42 contribution now that reach is
>   ten times longer; `fog_albedo`/`fog_anisotropy` left unauthored in both scenes, so the ambient term
>   stays byte-identical and the change stays bisectable.
> - **Two goldens moved, both with committed diff reports** (`Plan/artifacts/volumetric-medium-wave1-*`):
>   `post-full` SSIM 0.99838 (one damaged tile — the foreground road, exactly where the old private
>   extinction saturated near the lens), `district-plaza` 0.99991 with zero damaged tiles. Neither
>   breached its SSIM threshold, so the harness would not have written a report on its own; both were
>   produced through the committed emitter and reviewed by eye before blessing. macOS queued (cleared 2026-08-01 — see the Wave 4 log).
> - **`run.lrp` was re-recorded** — the committed replay embeds the scene text, so it failed `L1014` on
>   *load* until it moved. The `shafts` budget group measured 0.1382 ms against its 0.32 ceiling (0.1403
>   at calibration): flat, as a fixed 32-step march predicts, so no ceiling moved anywhere.
> - **Found, not fixed, and not this wave's:** `post-full.yaml`'s header records a `clipped_low` contrast
>   series of 0.048/0.100/0.162 at contrast 1.04/1.08/1.12; it measures 0.176/0.333/0.446 today. The
>   medium accounts for ~0.07 of that at 1.04, so the pre-merge configuration read ~0.16 there too — the
>   grade-decomposition revisions (403–404) are the likely cause. The series' conclusion (contrast cannot
>   deepen without crushing) is unaffected. Left for a separate look rather than absorbing another
>   change's drift into this one.
>
> **Wave 2 delivered 2026-07-30.** The medium's *geometry* is now shared as well as its σ_t/α/g: a new
> `shaders/snippets/medium.wgsl` holds `medium_optical_depth` (Wenzel, verbatim from the old
> `fog_factor`) and `medium_density_ratio`, composed by both `composite.wgsl` and `shafts.wgsl`. The
> march's per-step transmittance is `exp(-medium_optical_depth(…, t))` and each sample is weighted by
> the medium's local density. Amendments and findings:
>
> - **`fog_start` reaches the march, which the wave as written did not say.** It has to: the wave's own
>   agreement test asks the march-implied `T(d)` to match `fog_factor`'s `1 - T(d)`, and that is
>   impossible while one term starts at the lens and the other at `fog_start`. **The byte-identity gate
>   is therefore `fog_height_falloff == 0` AND `fog_start == 0`**, not falloff alone. Both were proven
>   on the shipped scene: `post-full` at those two overrides, and `post-full` with `shafts: 0` and the
>   height fields left authored (the `fog_factor` extraction alone), are sha256-identical across the
>   change.
> - **The Wenzel-collapse posture held with no branch.** `σ_s = α·σ_t` stays outside the loop and the
>   ratio multiplies inside, so the neutral case multiplies by an exact `1.0` and `medium_optical_depth`
>   returns exactly `density·t`. The ratio's boundary is `t < start`, not `t <= start`, so a `fog_start`
>   of 0 still admits the sample at the lens — `<=` would have zeroed the first step and moved the bytes.
> - **The march's bounds were left alone**, `SHAFT_OPTICAL_CUTOFF / σ_t` from the camera. Steps inside
>   `fog_start` contribute exactly zero through the ratio; re-distributing the 32 steps over
>   `[start, end]` would sample better and move every pixel of the neutral case for it.
> - **`fog_start` is not monotone on the beam**, found by a test that assumed it was. Clearing the medium
>   near the lens also clears the extinction that was dimming everything past it, so a start distance
>   short of the lit volume *brightens* the marched term (193 → 226 luma at `fog_start: 6` on the shafts
>   fixture). Only a start that eats into the lit volume itself reduces it. Recorded in the fixture.
> - **The height gate needed a new fixture.** On `shafts_scene` (σ_t 0.2 over a 15-unit reach, τ ≈ 3) the
>   low/high band ratio peaks at 0.507 near falloff 0.2 and *falls* past it — the denser half saturates
>   against its own extinction, so the fixture cannot separate "denser medium" from "more extinction".
>   `height_beam_scene` replaces it: a sun straight overhead through a ceiling gap, which makes the lit
>   volume a **vertical sheet** of equal thickness at every height, a back wall every ray ends on, and a
>   thin σ_t of 0.05. The ratio then runs 0.98 → 1.72 over falloff 0 → 0.4, monotone. `fog_anisotropy` is
>   0 there because the phase alone separates up-rays from down-rays several-fold.
> - **`Shafts` grew to 192 bytes** (`fog_start`, `fog_height`, `fog_height_falloff` after `anisotropy`,
>   then three pads); `shader_gen.rs` regenerated by `check.mjs --fix`.
> - **Both goldens moved, both reports committed** (`Plan/artifacts/volumetric-medium-wave2-*`):
>   `post-full` SSIM 0.99887, `district-plaza` 0.99952, **zero damaged tiles on either** — neither
>   breached its threshold, so both reports came from the committed emitter rather than a harness
>   failure. `image-stats --check` exit 0 and essentially unmoved (post-full banding 0.0730 → 0.0728,
>   plaza 0.0193 → 0.0189). macOS queued (cleared 2026-08-01 — see the Wave 4 log).
> - **The `shafts` budget group went 0.1403 → 0.1505 ms** (three 120-frame runs, medians) against its
>   0.32 ceiling. The other three groups moved 2–5% on the same runs with nothing behind them, so most of
>   that is the box; only the recorded measurements moved, no ceiling.
>
> **Wave 3 delivered 2026-07-30.** `L_ambient` is now the `Environment`'s own sky radiance along the
> view ray, behind a new `[0,1]` scalar `fog_sky`. **No golden moved and none was blessed** — every one
> of the sixteen gallery goldens and `district-plaza` re-render sha256-identical, which is the wave's
> whole claim. Amendments and findings:
>
> - **The IBL is not reached at all, and the plan's load-bearing risk therefore does not arise.** The
>   wave weighed a cube bind against a precomputed SH ambient and preferred the second; there is a third
>   that dominates both, and it exists only because this engine's sky is *procedural by decision and
>   will never be an image asset*. The sky radiance is a closed-form function of three authored colours
>   and a direction, so the composite evaluates it **exactly** for less arithmetic than an SH-L1
>   evaluation would cost, carrying three `vec3`s in the uniform it already binds. No new binding, no
>   layout change, no prefilter dependency, no truncation error — and the "per-pixel cube IBL" deferral
>   loses its trigger, because the low-frequency approximation it was the fallback for was never taken.
>   `sky_gradient` moved into `shaders/snippets/sky_gradient.wgsl` parameterized on the three colours
>   (a snippet cannot read a per-shader uniform); `sky.wgsl` gains its first snippet and its arithmetic
>   is verbatim, so `night-sky` is byte-identical.
> - **`fog_color` did NOT demote to a tint, and the wave as written is amended.** Both readings of
>   "multiplicative tint on the IBL sample" die on the arithmetic of the only two scenes that author
>   fog. A raw `sky * fog_color` is dimensionally wrong: `post-full` authors `fog_color`
>   `[0.055, 0.08, 0.14]` against a sky of the same magnitude, so the haze would land ~20× darker and
>   vanish. A luma-normalized tint (the `halation_gain` trick) fails the other way — `post-full`'s
>   `fog_color` *equals* its `sky_horizon` exactly, so the product's luma comes out ~11× the authored
>   value and the haze blows out. So `fog_color` keeps its meaning (the ambient radiance, and the
>   no-`Environment` answer) and the new axis is *how sky-coupled the medium is*:
>   `L_ambient = mix(fog_color, sky_gradient(view_dir), fog_sky)`. `fog_sky: 0` is exactly today's slab,
>   nothing needs migrating, no golden moves, and an author gets a dial rather than a semantics change
>   imposed on every scene that already authors fog.
> - **The view ray is the aerial-perspective approximation, said plainly in the shader.** For a
>   homogeneous medium under a fixed sky the true isotropic in-scatter is direction-independent; what an
>   eye reads as haze is the sky *behind* the surface showing through it. `fog_albedo` scales the term;
>   `Environment.ibl_intensity` deliberately does not — that knob grades how much the sky lights
>   *surfaces*.
> - **The branch's *placement* was worth one output step, and this is the wave's sharpest finding.**
>   The first shape computed the blend inside the `depth < 1.0` block off the fragment's own world
>   position (the same ray, one matrix multiply cheaper). It moved exactly **one pixel of `post-full`
>   by one step** with the branch never taken — the value of `ambient` was provably unchanged, so what
>   moved was how the backend scheduled the *rest* of the fragment. Bisected on a pristine HEAD build in
>   a scratch tree: the uniform growth alone reproduces the golden, the `let world` hoist alone
>   reproduces the golden, the branch between the depth load and the blend does not. Hoisting the blend
>   **out** of the depth branch and back onto the plan's own `world_from_depth(uv, 1.0)` reconstruction
>   restores byte-identity — the radiance does not depend on depth, so the neutral frame's arithmetic is
>   left alone. Recorded in the shader: a byte-identity claim is about generated code, not just values.
> - **No `Environment` is expressed by the recorder forcing `fog_sky = 0.0`** into the uniform (the
>   shadow wave's zeroed-uniform precedent), not by a second flag or a second pipeline. A scene
>   authoring `fog_sky` with no sky is byte-identical to one that omits it, and the field docs say so.
> - **Decision 7 held for the tenth wave running** — `fog_sky` is a `[0,1]` scalar, so a breach is the
>   schemars range's own `L1014`, with rejection rows at both ends.
> - **`Composite` grew 224 → 272 bytes**, which is the layout's minimum: 260 bytes of payload with a
>   64-byte matrix last forces the prefix to 208. `fog_sky` took `pad0`'s free lane; the three sky
>   triples are the one place the "every triple carries a scalar in its fourth lane" rule cannot hold,
>   so their spare lanes are spelled `pad0`/`pad1`/`pad2`. `shader_gen.rs` regenerated.
> - **The editor row shipped in this wave, not after it** (`fog_sky`, effect `fog`, neutral 0, not a
>   gate). `FIELDS` drives `neutralPatch`, so a missing row leaves the field contributing in the
>   hold-to-compare "off" half — the trap that has now caught `ssao_direct` and wave 1's
>   `shafts`-without-a-medium.
> - **Four fixtures on one new scene, `sky_haze_scene`** — an emissive-black wall filling the frame
>   under a gradient opposed in red and blue, `ibl_intensity: 0.0`, no sun. The wall hides the sky and
>   the zero intensity mutes the IBL, so the *only* route a sky colour has into the image is the haze;
>   that is what lets a test swap the whole gradient, or drop the `Environment`, and attribute every
>   moved byte to this term. The reading is `red − blue` per row band, which cancels the medium's own
>   thickness (a corner ray is longer than the centre ray, and that moves both channels together).
>   Gated: a zero blend never reads the gradient (guarded by the same pair at 1 differing); no
>   `Environment` is inert at 0.5 and 1.0 (guarded by a with-`Environment` render differing); the haze
>   takes the sky's direction (zenith band −0.170 blue-dominant vs horizon band +0.387 red-dominant,
>   both a flat 0.000 at `fog_sky: 0`); and the blend is monotone *and* affine 0 → 0.5 → 1.
> - **`image-stats --check` exit 0 and unmoved** (goldens unchanged), `profile --check` against
>   `budgets/pinned-rtx3090-post-full.toml` exit 0 with `composite` at 0.0707 ms and `shafts` at
>   0.1413 ms. No scene authors `fog_sky` yet, so nothing on disk pays for it — vigil's adoption is
>   wave 4's, along with the docs.
>
> **Wave 4 delivered 2026-07-30 — mechanized half. The exit sitting is the one thing still open.**
> Both scenes adopt the medium, both goldens re-blessed with committed reports, the two contact sheets
> and their command lines are in `Plan/artifacts/`, and the three stale doc topics are reconciled. Full
> `check.mjs` green; all four gate invocations at 0. **No ceiling moved anywhere** — the only edits to
> `budgets/image-stats-windows.toml` are recorded measurements and their prose.
>
> **macOS queue cleared 2026-08-01 (Lore rev 446).** The medium, the moon retune and `ssao_direct`'s
> renderer half were blessed on the M4 in one pass (`post-full`, the first `night-street`,
> `district-plaza`); all three M4 budget gates green with measurements re-recorded and no ceiling
> moved; `budgets/image-stats-macos.toml` created — the last gate that had existed on Windows only.
> J1's cross-OS half closed in the same pass (thread-identity 5/5, all three corpus replays
> bit-identical at 1/2/8 threads on arm64, no `run.lrp` re-record).
>
> - **The wave's headline finding is about vigil's *camera*, not about the medium, and it decides what
>   the sitting is actually judging.** The follow camera pitches 58° down with a 50° fov, so every eye
>   ray in a district runs 33–83° **below** the horizon: it never sees the sky, and the radiance
>   `fog_sky: 1.0` picks up is the below-horizon band (`sky_ground` easing toward `sky_horizon`),
>   near-black and cool. Worse for the beams, the view direction sits **93° off the moon's travel**
>   (cosine −0.06), and both are constants — the pitch and the moon never change — so the
>   Henyey–Greenstein forward lobe points away from the camera at every moment of every run. Swept
>   −0.4/−0.2/0/0.3/0.5/0.7, the beams' on/off frame-mean contribution runs
>   +0.27/+0.36/**+0.41**/+0.37/+0.27/+0.15 of 255: a flat maximum at **isotropic**, with the inherited
>   default 0.7 costing 2.75× of it. So `fog_anisotropy: 0.0` is authored, and it is a property of this
>   game's fixed camera rather than of this frame.
> - **And the beams still do not read as beams there, which is the sitting's real question.** The eye
>   ray crosses ~25 units of empty air above the city before reaching the ground and nothing shadows
>   that air, so the directional term is a flat veil however loud — measured up to `shafts: 8.0`
>   (+9.5 of 255), which brightens the frame without ever drawing a shaft. `shafts` stays **0.34**,
>   restoring at g = 0 the +0.42 the fragment has recorded since the merge and which the height medium
>   reaching the march had quietly taken to +0.15. **The user's opening complaint — "I don't see the
>   volumetric light" — is answered by the engine and not yet by the district**, and the candidates are
>   geometry the moon can cast through (an arcade, a colonnade, roof gaps) or a camera that sees more
>   than the ground. That is a game-side call, not an engine one.
> - **Follow-up 2026-07-31 (Lore rev 420; testbed rev 419) — the finding is now demonstrated, and the camera is confirmed
>   the dominant lever.** A built-in-registry testbed, `examples/gallery/night-street.yaml` (a street
>   canyon + a colonnade the moon casts through, authored at the raking-shaft look), exists because
>   `render --sweep` cannot drive a district. On it, a **camera-pitch sweep −12→−58** shows the beams
>   filling the frame at −12° and vanishing as the pitch approaches vigil's −58°: the shafts live in the
>   air above the street, which the steep follow-camera crops out. Four ingredients get to beams — a
>   camera that sees the air (dominant), a backlit azimuth (vigil's moon already has it), occluders in
>   the air column, a dark backdrop. Sheets: `Plan/artifacts/volumetric-medium-night-street-{pitch,moon,shafts}.png`
>   + `-sweeps.md`. **Per user direction the camera/arcade half is deferred to the game; the
>   surface-contrast half landed in `env-night.yaml`:** moon `intensity 0.3 → 0.8`, `ibl_intensity
>   1.2 → 0.9` — brighter moonlit ground and sharper raking building/character shadows the steep camera
>   *can* show, still nocturnal and torch-led. `shafts` stays 0.34 (raising it at this camera only
>   brightens the veil). `district-plaza` golden + `run.lrp` re-blessed.
> - **`fog_sky: 1.0` on both scenes, and on `post-full` it fixes a real inverted seam.** The retired
>   slab *was* the hand-match (`fog_color` is exactly `sky_horizon`), and a hand-match is only correct
>   in one direction: the towers stand ~11° above the horizon where the gradient has already eased a
>   third toward the zenith, so the matched slab is ~1.5× too bright there. Measured at the sweep's
>   thick end (`fog_density: 0.06`), `fog_sky: 0` buries the skyline at luma 66.3 against a 60.2 sky —
>   the towers come out **brighter than the sky behind them**, and it worsens with every extra unit of
>   density — while 1.0 converges on it from below (53.1). At the shipped 0.014 the medium is too thin
>   to show it; what the blend buys there is the direction dependence.
> - **A sky-coupled haze is frame-mean-neutral by construction, so this file's balance metric goes
>   blind to it.** `post-full`'s haze costs +0.7 of 255 against +2.4 at `fog_sky: 0`, and density does
>   not buy it back (+0.70/+0.76/+0.74/+0.61 across `fog_density` 0.014→0.04) — replacing a surface
>   with the sky it is seen against lands near the frame's own mean, so the effect is a chromatic
>   redistribution (red −0.9, blue +2.2) rather than a luma lift. The skyline reading above is what
>   gates it instead. Recorded in the scene header, because the number reads like a collapse and is not.
> - **`fog_color` is kept in both scenes and is exactly inert at `fog_sky: 1.0`** — `mix(x, y, 1)` is
>   `y`, so the frame is byte-identical with the field deleted. It is the radiance the medium falls
>   back to below 1 or with the `Environment` removed, and vigil's magenta is left there deliberately:
>   it is the colour the districts shipped with, one edit away.
> - **`fog_albedo` stays unauthored in both, measured rather than assumed.** On `post-full` it only
>   ever darkens, and past the point of removing the medium: 64.37/62.90/61.31/59.60 at 1.0/0.75/0.5/
>   0.25 against a no-medium 62.46, so below ~0.8 the medium takes out more than the in-scatter puts
>   back — the "dark smoke" the field exists for, and not this scene. On vigil it is nearly a free
>   choice: with a near-black in-scatter to scale it has almost nothing to scale (70.06/69.55/69.04).
> - **The stale contrast series is re-measured and rewritten, closing a standing item.** It now reads
>   `clipped_low` 0.0030/0.2066/0.3538/0.4776 at `contrast` 1.0/1.04/1.08/1.12 against a 0.02 ceiling —
>   ten times louder than the first series, same verdict and a sharper one. **The medium *lowers* the
>   crush** (0.2066 against the 0.2478 the same frame reads at `fog_density: 0`), because in-scattered
>   light lifts exactly the deep-shadow samples the metric counts — the Risks section's metric-gaming
>   direction, recorded rather than banked.
> - **The sheets are swept on `post-full`, not on a district, and the reason is a tooling boundary
>   worth knowing:** `render --sweep` cannot drive a district at all, because a district scene names
>   vigil's own components and the CLI carries only the built-in registry — a sheet the sitting cannot
>   regenerate from one command line is not the instrument the gate asked for. `--sweep` also takes one
>   pointer and lays values out near-square, so each 4×4 cross is four invocations re-cut into rows;
>   the albedo/anisotropy sheet pins `shafts: 0.1` (5× the authored value) because at 0.02 the whole
>   phase axis moves the frame by ±0.5 of 255 and the rows are indistinguishable.
> - **Goldens:** `post-full` SSIM 0.99457, 11 damaged tiles, worst 0.948 middle-right — the skyline,
>   exactly where the blend acts; `district-plaza` 0.99950, zero damaged tiles. `run.lrp` re-recorded
>   (the replay embeds the scene text); the full-run suite needed no re-probe. macOS queued (cleared 2026-08-01 — see the Wave 4 log).
> - **Docs:** the engine guide gains "Fog and light as one medium" and its `PostProcess` field list goes
>   from two fields with **pre-flip defaults** to all thirty-three with real ones; `ludion-renderer`
>   gains the seven missing shader rows, both new snippets and a renderer-side medium section;
>   `ludion-render-graph` gains the real pass counts and the render-vs-output extent rule.
>   `embedded-docs/` re-mirrored.

## Why (the one-paragraph case)

The engine ships two half-models of the same physical thing. `fog_density` is an analytic extinction
that buries surfaces in `fog_color` (uniform haze, no shadow interaction). `shafts_density` is a
ray-march of the sun's shadow map that adds in-scattered light (beams). They carry **two independent
densities** and never interact: you can author thick fog whose beams it does not dim, or strong beams
in a medium that attenuates nothing. Physically there is one medium with one extinction; the haze is
its *isotropic* in-scatter, the beams are its *directional* in-scatter, both are extinguished by the
one density, and both scale by the one scattering albedo. Unifying them makes thicker fog give more
haze **and** brighter, shadow-shaped beams (as reality reads), lets the fog dim its own beams, lets
height fog shape the god-rays, lets a low-albedo medium read as dark absorbing smoke — and the merge
step itself removes a field rather than adding one.

## The model

A single-scattering participating medium. For an eye ray reaching a surface at distance `d`, with
transmittance `T(t) = exp(−∫₀ᵗ σ_t ds)` and scattering coefficient `σ_s = α·σ_t`:

```
L = T(d)·L_surface  +  ∫₀ᵈ σ_s · T(t) · [ L_ambient(t) + p(θ)·V(t)·L_sun ] dt
    └─ extinction ─┘    └──────────────── in-scatter along the ray ─────────┘
```

The engine already computes both bracketed terms — it just computes them with **different `σ_t`**,
never shares the transmittance, and has no albedo (it assumes `α = 1`, pure scattering). Making it
coherent and complete is four moves, one per authorable axis:

- **Extinction `σ_t` = `fog_density`.** One density for the whole medium; the march reads it, so
  `shafts_density` is **deleted**. Beam reach `SHAFT_OPTICAL_CUTOFF / σ_t` now tracks fog thickness,
  which is what "how far can light travel through this medium" means.
- **Albedo `α` = `fog_albedo` ∈ [0, 1]** (`σ_s = α·σ_t`, absorption `σ_a = (1−α)·σ_t`). Scales
  **both** in-scatter terms, so a dark absorbing medium (low α) and a bright scattering one (high α)
  are one knob apart. **Default 1.0 is pure scattering — exactly today's fog**, which is what keeps
  the ambient term byte-identical when a scene does not author albedo.
- **Ambient in-scatter `∫ σ_s·T(t)·L_ambient dt`.** With a constant ambient radiance this has the
  closed form `α·(1−T(d))·L_ambient`, which at `α = 1` is exactly `composite.wgsl`'s
  `mix(surface, fog_color, 1−exp(−σ_t·d))` — **the analytic fog stays analytic and cheap.** At Wave 1
  `L_ambient = fog_color` (flat). At Wave 3 `L_ambient` becomes the `Environment` sky/IBL radiance so
  the haze in-scatters the *actual* sky, with `fog_color` demoted to a tint/fallback for the no-`Environment` case.
- **Directional (sun) in-scatter `∫ σ_s·T(t)·p(θ)·V(t)·L_sun dt`.** The march — already has the phase
  `p`, the shadow visibility `V`, and an `exp(−σ·t)` transmittance, but with `σ = shafts_density` and
  no `α`. It reads `σ_t` for the transmittance and scales by `σ_s`, so the beams live in and are
  dimmed by the same medium as the haze. `shafts` stays as the directional-term intensity/gate; `g`
  becomes `fog_anisotropy`.

### What stays several fields, and why Wave 8's objection is answered

The surviving axes are **orthogonal and independently authorable**:

| Field | Physical role | Default | "Fog, no beams" | "Dark smoke" | "Beams, thin haze" |
|---|---|---|---|---|---|
| `fog_density` (σ_t) | extinction — thickness | 0 (off) | any | high | small |
| `fog_albedo` (α) | scatter fraction (both terms) | 1.0 | any | low | high |
| `fog_color` (L_ambient) | ambient in-scatter radiance / IBL tint | — | authored | dark | — |
| `shafts` (directional intensity) | sun in-scatter strength + gate | 0 (off) | 0, or no sun | 0 | large |
| `fog_anisotropy` (g) | beams vs. uniform haze | 0.7 | — | — | high |

Wave 8 feared "one field meaning two things." The full model does the opposite: five orthogonal axes,
none overloaded. The field **deleted** (`shafts_density`) was a second spelling of extinction that
*contradicted* the first — removing it is CLAUDE.md's "remove the first before adding a second," not a
collapse. A medium with `shafts: 0` is today's fog; add a sun and `shafts > 0` for beams whose reach,
dimming and colour are all governed by the fog they live in.

## Decisions (settled with the user 2026-07-26 unless marked objectable)

1. **Share the extinction; delete `shafts_density`.** The march reads `fog_density`. The only real
   cost is that a scene can no longer author beams whose reach is decoupled from the haze — which is
   precisely the incoherence being removed.
2. **Keep the `fog_*` / `shafts` field names** (user pick). Coherence lives in the shared σ_t/α and
   the docs, not a `medium_*` prefix; a rename would be a large cosmetic diff across component, schema,
   shaders, docs and every authoring scene for no behavioural gain.
3. **Full PBR medium** (user pick): explicit `fog_albedo` (σ_s/σ_t) **and** sky/IBL-sourced ambient
   in-scatter are in-plan (Waves 1 and 3), not deferred behind triggers.
4. **`g` becomes authorable — `fog_anisotropy`.** *Extends the literal selection*: the user picked
   "full PBR medium" (which named albedo + IBL) over the standalone g-knob option, but a full medium
   with a hardcoded phase is incomplete and the cost is one scalar, so it is included. **Objectable:**
   say the word and `g` stays the constant 0.7 with the knob deferred behind a look-verdict trigger.
   Default **0.7** (today's constant), range validated to (−1, 1); the phase function already takes it.
5. **`fog_albedo` defaults to 1.0** (pure scattering = today's fog), so the ambient term is
   byte-identical when unauthored — the arc's skip-don't-compute rule, here bought by a compatible
   default rather than a branch.
6. **Neutral stays byte-identical; only medium-authoring scenes move.** A scene authoring no fog and
   no shafts renders exactly as today. The one golden that moves is `post-full.yaml` (re-authored:
   `shafts_density` gone, `shafts` retuned against the shared σ_t, `fog_albedo`/`fog_anisotropy` at
   their defaults unless the look wants otherwise), moved once with the SSIM diff report committed.
7. **New L-codes only where a new axis needs one.** Deleting `shafts_density` removes its rejections.
   `fog_albedo` is a scalar `[0,1]` → the schemars range's own `L1014`. `fog_anisotropy` is a scalar
   `(−1,1)` → same. No bounded-triple field is added, so `L1130` is not touched.
8. **With a cascaded sun the shafts march the cascades,** which cover the `3/fog_density` reach (Clockout 0011,
   `Plan/shadow-mapping.md` decisions 15 and 17). Owner approval 2026-10-29.

**Request 0009 (local-light scatter and GI in the haze; proxy-approved 2026-11-08, GI by the owner).**

9. **A per-pixel segment march, not froxels.** `medium_scatter` marches each eye ray over each
   selected light's sphere only, Bayer-jittered and blurred 4×4; no temporal accumulation, no volume
   texture. It is one pass beside the shafts with the same medium functions, so the three terms cannot
   drift, and it met the cost target (below). Steps per segment are `max(shaft_steps / 4, 2)`.
10. **`scatter` is an opt-in `f32` on `PointLight` and `SpotLight`** (default `0`, absent is
    byte-identical). `1.0` is the physical amount for the shared medium (σ_t, α, g, height and start
    are the medium's). A scene with fog and no scattering light is unchanged.
11. **At most 64 scattering lights per frame, heaviest first** by `scatter·intensity·max(color)·range·
    e^(−σd)`, ties to the lower light-list index (points, then spots); the survivors sum in list
    order. Past 64 the cost no longer scales as the bench's 16-light arm.
12. **The existing shafts/medium quality axis gates it** (no new axis). `Quality.shafts` masks local
    scatter and GI in the haze together; steps derive from `shaft_steps`.
13. **Shadowed scatter reads the local-shadow atlas, one exact texel per step.** A light that holds a
    grant is shadowed; one without scatters as an unshadowed glow. Scatter never changes who is
    granted.
14. **GI in the haze is default-on.** The same pass reads the `GiVolume` DC terms: the ambient is
    scaled by the in-scatter-weighted mean sky visibility (alpha), and the bounce is in-scattered,
    scaled by `GiVolume.bounce`. Strength 0, or no volume, is the off switch and byte-identical. It
    moves vigil's `GiVolume` + fog goldens and budgets; their pinned re-bless is owed.
15. **A viewmodel gun light may scatter**, at its render pose. Transparents match today's sun-shaft
    behaviour. No wire or replay move: `scatter` is a component field with a default, and the schema
    snapshot absorbs it.

## Waves

### Wave 1 — one medium: shared σ_t, albedo, anisotropy (the merge)
- The shaft march (`shafts.wgsl`) reads the medium extinction instead of a private `shafts_density`:
  the cutoff `3/σ_t` and the integrand's `exp(−σ_t·t)` share the fog's σ_t, and the estimate scales by
  `σ_s = α·σ_t`. `shafts.wgsl`'s doc block rewrites the "shafts add light and never remove it /
  coupling would make one field mean two" paragraph to the merged model. `g` is fed from `fog_anisotropy`.
- **Component:** **delete `PostProcess.shafts_density`**; **add `fog_albedo`** (default 1.0, `[0,1]`)
  and **`fog_anisotropy`** (default 0.7, `(−1,1)`). `shafts` stays (directional intensity + gate,
  `shafts > 0` **and** a `DirectionalLight`, unchanged). The ambient fog term gains its `α` factor;
  at `α = 1` it is byte-identical.
- `composite.wgsl`'s chain-order comment (steps 3–4) rewrites to name the terms of one medium; the
  ambient term becomes `T·surface + α·(1−T)·L_ambient` (`L_ambient = fog_color` this wave), the
  directional term stays an add. Both scale by α from the one place.
- **Re-author `post-full.yaml`**: drop `shafts_density`, retune `shafts` against `fog_density: 0.014`
  as the shared σ_t off a `--sweep` sheet (the wave-11 moon-rake frame-mean-luma balance is what to
  hold), leave `fog_albedo`/`fog_anisotropy` at defaults unless the sheet argues otherwise; re-bless
  the Windows golden with the diff report, queue macOS.
- **Tests:** neutral byte-identical; `fog_albedo: 1.0` ambient-term byte-identical to today's fog (a
  fixture); beam **reach tracks `fog_density`** (raise σ_t → beams shorten, via `--set`); the **"fog
  dims its own beams" A/B** (same `shafts`, two `fog_density`, thicker one darker down-ray); a **low-α
  darkens** fixture (high σ_t, α → 0 reads as absorbing smoke, not bright haze); `fog_anisotropy`
  moves the beam/haze balance (g → 0 makes the sun term read as uniform fill); cross-process byte
  identity through `--set`; sunless-asks-for-shafts still derives `false`, not an error.

### Wave 2 — height-coherent beams
- The march accumulates the height-varying optical depth (Wenzel per step, or a two-point analytic
  segment) so `fog_height` / `fog_height_falloff` shape the god-rays — beams thicken toward the ground
  under a height medium, matching the haze that already does.
- **At `fog_height_falloff = 0` the accumulation is `σ_t·t` exactly**, so the pass is byte-identical
  to Wave 1 there — the skip is the formula's own limit, not a branch (Wenzel-collapse posture).
- **Tests:** falloff-0 byte-identical to Wave 1; a low camera under a height medium gets denser beams
  near street level than at altitude; the march-implied `T(d)` matches `fog_factor`'s `1−T(d)` within
  step error (the ambient and directional terms agree on the same medium).

### Wave 3 — sky/IBL ambient in-scatter (the PBR ambient term)
- The ambient in-scatter's `L_ambient` becomes the `Environment` sky/IBL radiance instead of a flat
  `fog_color`, so the haze picks up the actual sky (blue high, warm at the horizon band) rather than
  one authored slab. **`fog_color` demotes to a tint/fallback**: used directly when no `Environment`
  is present, and as a multiplicative tint on the IBL sample when one is. This is the plan's one real
  architectural seam — the composite must reach the prefiltered IBL, which today is bound in the mesh
  path (group-based). Options weighed in the wave: bind the IBL cube to `composite`, or precompute a
  cheap directional ambient (SH/average) the composite reads from a small uniform. **Prefer the
  precomputed directional ambient** — the composite is a fullscreen pass and a per-pixel cube sample
  along each ray is cost the haze does not need; a few SH coefficients captured at extract carry the
  sky's low-frequency colour, which is all an isotropic in-scatter integrates against.
- **Gated:** no `Environment` ⇒ the flat `fog_color` path, byte-identical to Wave 2. `Environment`
  present but no medium ⇒ still byte-identical (the term is `σ_t`-gated).
- **Tests:** no-`Environment` byte-identical to Wave 2; with `Environment`, the haze colour varies
  with view direction toward the sky's own gradient (a fixture: haze toward the horizon band differs
  from haze toward zenith); `fog_color` as a tint composes multiplicatively (identity at white).

### Wave 4 — vigil adopts, docs, close
- Vigil authors the medium in the shared `env-night.yaml` fragment: the moon (already a
  `DirectionalLight`) throws god-rays across the districts, the night haze in-scatters the shared
  night sky — the concrete payoff the user opened with ("I don't see the volumetric light"). Plaza via
  the map fragment + regenerate + `map-drift` green; market and cathedral by direct edit; one
  `run.lrp` re-record; `district-plaza` golden re-bless; the macOS queue entry updated in place.
- Docs: the engine guide's "Post-processing" chapter gains a "Fog and light as one medium" section
  (the σ_t / α / g / L_ambient model, the field table above); `ludion-renderer` /
  `ludion-render-graph` fog+shafts prose reconciled; `embedded-docs/` re-mirrored via `check.mjs --fix`.
- **Exit gate:** one human sitting on the dressed build — the moon beams and the sky-tinted haze
  answering the opening question, judged against a committed `--sweep` sheet over
  `fog_density` × `shafts` (and a second over `fog_albedo` × `fog_anisotropy`).

## Status — request 0009

Local-light scatter and GI in the haze shipped (`medium_scatter`, `medium_scatter_blur`, composite
binding 6; gallery `examples/gallery/medium-scatter.yaml`; bench
`crates/ludion-engine/tests/scatter_bench.rs`). Pinned M4/3090 figures, blesses and the vigil
re-bless are owed (`Plan/owed.md`).

GPU frame-time delta against an identical scatter-off twin, **unpinned M5**, release, 1080p, three
runs (ranges). Not authoritative; per-pass medians overlap on a tiled GPU and are not summed
(`ludion devdocs profiling`).

| Arm | Delta (ms) |
|---|---|
| 1 light | 0.18–0.21 |
| 16 lights | 0.18–0.30 (meets the 0.5 ms target) |
| 64 lights | 1.82–1.83 |
| 16 shadowed spots | 0.39–0.75 |
| GI only | 0.22–0.26 |

## Deferred, with triggers

- **Multiple / area lights scattering into the medium.** **Fired — request 0009**: point and spot
  lights scatter (decisions 9–15), shadowed when granted. Area lights remain unbuilt.
- **~~Per-pixel cube IBL in-scatter~~ — retired, not deferred.** This was the fallback for the
  low-frequency SH/average ambient Wave 3 planned, and its trigger was "the approximation reads wrong
  against a sharp sky feature". Wave 3 took neither: the procedural sky's gradient is evaluated in
  closed form, so there is no approximation to read wrong and nothing to escalate to. It comes back
  only if the sky ever stops being analytic — which `CLAUDE.md` forecloses.
- **A froxel (volume-texture) medium** — the march is per-eye-ray single-scatter; a froxel volume
  buys multiple lights, transparency-correct ordering and temporal amortisation. Trigger: temporal
  amortisation is wanted, more than 64 scattering lights, or GI in-scatter needs more than the per-ray
  DC march (L1 bands, per-froxel irradiance); a froxel volume makes GI/irradiance-volume in-scatter
  nearly free.
- **Multiple scattering / a dual-lobe phase.** Single-scatter is the model; a second HG lobe or a
  cheap multi-scatter approximation is the next fidelity step. Trigger: a look verdict that thick fog
  reads too dark (single-scatter under-fills dense media).

## Risks

- **The march stops being an isolated opt-in.** After the merge the pass reads `fog_density`, so a
  scene authoring fog + a sun + `shafts > 0` gets a march it previously muted with `shafts_density: 0`.
  Mitigation: the gate is unchanged (`shafts > 0` is the switch), and `shafts_density: 0` no longer
  exists to be the mute — call it out in the field docs and the wave-1 commit. **Audit corrected
  2026-07-29:** the proposal's "`post-full` is the only in-repo author; vigil authors neither" went
  stale the day after it was written. The Art Director's shared night grade (Lore rev 401) authors
  `shafts: 0.19` **and `shafts_density: 0.225`** in `games/vigil/scenes/env-night.yaml`, which merges
  into all four district/budget scenes; the editor's `editor/app/src/editor/art/chain.ts` `FIELDS`
  table carries a `shafts_density` row and `chain.test.ts` mirrors it in the engine-field list. So the
  deletion is atomic across engine + six scenes + the editor table + two goldens + `run.lrp`, and
  vigil's wave-4 adoption is partly pulled forward into wave 1 (the field cannot be deleted and left
  authored).
- **Wave 3 is the load-bearing risk** — reaching the IBL from a fullscreen composite pass is a real
  binding-layout change (the shadow-wave "zeroed uniform + 1×1 fallback" precedent applies: no
  `Environment` must stay a single bind-group, not a second pipeline). The precomputed-SH option keeps
  it a uniform, which is why it is preferred; if it proves insufficient the cube bind is the fallback,
  measured before committing.
- **Height-coherent extinction in the march (Wave 2) is the one real numerical addition.** A per-step
  Wenzel evaluation is more arithmetic than `exp(−σt)`; mitigate with the two-point analytic segment
  and the byte-identical falloff-0 gate. Budget: the `shafts` group ceiling (dormant → live at the
  post-process flip) absorbs it; re-measure at Wave 2, widen only on a real breach.
- **Albedo changes the shaft magnitude vs. today**, so `shafts` in `post-full` must be retuned — a
  taste call, not a derivation. Do it off a `--sweep` sheet, commit the verdict in the wave log,
  re-bless once. Golden churn is bounded to that one scene.
- **Metric-gaming watch (the arc's standing hazard).** In-scattered light lifts deep-shadow gradients,
  which *lowers* `banding_score` for free (wave 8 saw this). Gate the medium on the coupling fixtures
  (reach-tracks-σ_t, fog-dims-beams, low-α-darkens), **not** on a banding improvement — a banding drop
  is a side effect, not proof of coherence.
