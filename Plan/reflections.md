# Plan: Reflections — screen-space reflections over box and sphere probes

> Status: **W1 mechanized (2026-10-20); W2 mechanized (2026-10-22); W3 mechanized (2026-10-22); W4 mechanized (2026-10-22).** Design settled 2026-10-19. Decisions come from a grilling session with
> the user on 2026-10-19. Current-code facts were read from the tree on that date; the file:line
> pointers are from that date too, so re-grep before editing. This plan answers
> `Plan/post-process-pipeline.md`'s deferred "Screen-space reflections" entry.

## Trigger

The reference game is wet: rain, puddles, muddy ground. Its art bible says "everything is wet,
always", using baked low roughness, and its water-surface request notes "there is no screen-space
reflection". Glossy surfaces currently reflect only the procedural sky's 64² prefiltered cube, so a
puddle in a street cannot show the buildings around it.

The driving surfaces are **puddles and wet ground**, **rain-wet walls and props**, and **polished
interiors**. Large open water is not a driver.

## What exists (2026-10-19)

- **Clustered forward, no G-buffer.**
  - `scene` is `Rgba16Float` and `scene_depth` is `Depth32Float`; both can be sampled.
  - The depth+normal prepass exists only when SSAO is on (`graph.rs:162-169`). It writes world
    normals and no roughness.
  - MRT is deferred in `post-process-pipeline.md`, and this plan is its trigger: the first pass that
    genuinely writes two colour targets.
- **Pass order:**
  - shadow → [prepass, ssao] → [gi_inject] → scene → bloom → [shafts] → [dof] → composite → resample
    → [overlay].
  - Inside `scene`, the draw order is meshes, sky (fullscreen at far depth), ground markers, sprites,
    particles, text.
- **No history buffer.**
  - TAA was rejected (post-process decision 5).
  - `docs/ludion-renderer.md` says "no temporal accumulation — test mode is the only mode".
- **Sky IBL** (`src/ibl.rs`, `shaders/ibl.wgsl`):
  - All three textures are `Rgba16Float`:
    - a 32² irradiance cube;
    - a 64² specular cube with 5 mips, GGX-prefiltered at `roughness = mip/4`;
    - a 128² split-sum BRDF LUT.
  - The cubes are re-prefiltered whenever the sky colours change (a content hash with a
    single-entry cache).
  - The sun disc is excluded from the cubes.
  - `mesh.wgsl:394-402` samples `lod = roughness * 4`.
  - GI sky visibility multiplies the whole IBL term, specular included (`:457`).
- **GI** is diffuse-only SH-L1 probe volumes, from a load-time voxel bake (`global-illumination.md`).
  `mesh.wgsl:446` says "indirect specular … stays the prefiltered cube's job".
- **No Hi-Z.**
  - Render-graph transients are always one mip (`ludion-render-graph/src/execute.rs:475`).
  - Only three formats exist (`resource.rs:65-76`).
- **Texture budget:** the mesh fragment stage binds 15 of the 16 sampled textures allowed by the
  WebGPU floor that iOS is pinned to.
- **Precedents this plan follows:**
  - the component pattern: `SpotLight` (`components.rs:491`, `Plan/spot-light.md`);
  - the content-addressed derived store: `Plan/cooked-assets.md` decisions 1–6;
  - the load-time GPU bake: GI;
  - `render.debug` debug-draw categories;
  - `--dump-gi`;
  - `Quality` / `QualityParams` (`crates/ludion-renderer/src/quality.rs`).

## Decisions

### Shape of the system

1. **Scope is reflection transport only.** The wet *look* (a wetness scalar, puddle masks, ripple
   normals) belongs to a sibling plan. **Planar reflections are never built**: SSR plus probes is the
   system.
2. **Resolution chain, per pixel:**
   1. SSR, where the trace hits with confidence;
   2. box probes;
   3. sphere probes;
   4. the procedural sky's specular IBL as the global floor.

   A scene with zero probes still reflects the sky.
3. **The two paths are designed together.** An exact match is not the goal, because each technique
   has known limits. The goal is that, by construction, SSR and probes are as close as possible
   wherever both are valid. Decisions 4, 5 and 18 serve this.
4. **One roughness → footprint function.**
   - `cone_half_angle(roughness)` is derived from the GGX lobe.
   - It decides three things:
     - the roughness each cube mip stores: the lobe matched to **K texels** (K = 32π, so r = 1 fills
       the 8×8 face); mip 0 is the mirror and the chain stops at 8×8. A 1×1 face cannot hold a wide
       lobe (Filament's precedent), and `night-sky`'s rough metals showed it;
     - the cube lookup `lod`;
     - the colour-pyramid level SSR reads: `half_angle × hit_distance`, projected to screen.
   - The function is unit-tested. The sky IBL migrates onto it, replacing the linear
     `mip/(N−1)` convention.
5. **One blend site.**
   - The scene pass splits into **opaque → `reflections` → transparents**.
   - Opaque meshes write two extra colour targets:
     - normal + roughness;
     - F0 + specular occlusion.
   - Opaque meshes no longer add indirect specular themselves.
   - The full-screen `reflections` pass evaluates box probes, sphere probes and the sky (the fill) in
     one shader and adds it to `scene`. With SSR on it also writes the fill to `refl_fill`, the colour
     pyramid seeds from the lit scene, and `ssr_blend` adds `confidence × (hit·DFG − fill)`
     (decision 29). There is one fill, not an inline path plus a correction.
   - This also moves the cube bindings out of the mesh shader's nearly full texture budget.
6. **Receivers.**
   - Opaque meshes get the full chain.
   - Transparent meshes sample probes and the sky **inline**, through the same WGSL snippet the
     `reflections` pass uses. They get no SSR.
   - Sprites, particles, text and ground markers get no specular reflections.
7. **Specular occlusion is split by source.**
   - GI sky visibility darkens only the sky fill's remainder after probe coverage. A probe's sky
     fraction already is the sky visibility from its capture point, so it is not darkened again.
     Probes already contain their own occlusion.
   - SSAO and occlusion maps apply to every indirect-specular source.
   - SSR hits get no occlusion term.
8. **The directional light becomes a disc area light for specular.**
   - It uses representative-point specular, widened by the light's angular radius, so a mirror
     puddle shows the moon as a soft disc.
   - The IBL keeps excluding the disc, so nothing is counted twice.

### SSR

9. **Hi-Z cone-traced SSR.**
   - One mirror ray per pixel is marched through a Hi-Z depth pyramid.
   - Roughness comes from reading a pre-convolved scene-colour mip pyramid at the cone footprint
     given by decision 4.
   - It is deterministic and noise-free in a single frame.
   - The trace reads the scene after the probe and sky fill and before transparents. It is still
     single-bounce: a hit never carries SSR.
10. **The trace resolution is output extent × `ssr_scale`**, independent of `render_scale`, followed
    by a depth- and normal-aware upsample to render extent. SSAA buys edge quality; reflections are
    roughness-blurred, so their cost follows what the player sees.
11. **The roughness ceiling is configurable.**
    - SSR fades out linearly between `ssr_roughness_fade = [0.45, 0.65]`, which are the defaults.
    - Above the ceiling, only probes and the sky apply.
12. **Temporal accumulation is "not yet", not "never".**
    - Cone tracing ships first.
    - **Trigger for temporal SSR:** the reference game's sitting judges cone-traced SSR on wet
      surfaces at r 0.1–0.4 unsatisfying (blocky, smeared or leaking), and no change to the cone or
      the pyramid fixes it within one wave.
    - Test mode would still disable it (Pillar 3).
    - The absolute wording in `global-illumination.md` C1 is softened to match.
13. **Graph mip chains are first-class.** The Hi-Z and colour pyramids are mipped transients:
    - mips are declared on the resource;
    - each mip has its own view;
    - per-mip reads and writes are visible in `--dump-graph`.

    The culling plan's HZB (`gpu-driven-culling.md` rung 3b) reuses this. Probe cube arrays stay
    renderer-owned resources outside the graph, as the sky IBL is.

### Probes

14. **The component.** `ReflectionProbe { shape, blend, priority }` on an entity. Its `Transform`
    gives the capture point and the orientation.
    - `shape` is either `box { extent }` or `sphere { radius }`.
    - **One oriented box** is both the influence volume and the parallax proxy; there is no separate
      projection box.
    - Spheres are parallax-corrected against their own sphere.
    - `blend` is an outward fade distance: weight 1 inside the volume, 0 at `blend` outside it (amended
      2026-10-22, so a box fitted to its room gives its walls full weight).
    - The component is registered last, because the registry is append-only.
15. **Overlap is resolved by coverage accumulation.**
    - Probes are ranked by `priority`, then smallest volume, then `EntityId`.
    - Each probe contributes `weight × (1 − coverage so far)`, at most 4 per pixel.
    - Boxes rank before spheres, and the sky fills whatever remains.
    - Probes are binned into the existing clustered grid.
    - The ranking is a static per-probe key, not camera distance, so a static reflection never
      changes as the camera moves. The camera only decides residency (decision 21).
16. **Capture contents:**
    - meshes, the sky and all lighting (shadows and GI included);
    - the world at frame 0;
    - linear HDR, before any post-processing;
    - no sprites, particles, text or ground markers.

    Moving content is SSR's job.
17. **Relight, so the run clock never forces a rebake.**
    - The capture stores geometry only: rgb is 0 where the depth is untouched (sky) and alpha is the
      sky mask, so after prefiltering rgb is geometry radiance T and alpha the sky fraction. A probe
      that sees emissive (or hit-flash) surfaces also stores that part E, a second layer from the
      capture's fourth scene target, prefiltered through the same chain. The resolve is
      `T · ratio + E · (1 − ratio) + a · sky_cube(r) · k`, equal to `lit · ratio + E + …`: emission
      never follows the ambient. The sky cube is the live sky specular cube along the fitted dominant direction
      (decision 31) and `k` = `ibl_intensity` with an `Environment`, 0 without one.
    - `ratio` = live E / baked E per channel (1 where baked E ≤ 1e-6), E being the normal-free
      diffuse irradiance at the probe's capture point, one value per cube axis (+X −X +Y −Y +Z −Z),
      each composed like `mesh.wgsl`'s ambient term with `diffuse = 1` and no occlusion:
      `k · irradiance cube along the axis + ambient`, times
      `mix(1, sky visibility DC, sky_visibility)` with a `GiVolume`, plus `bounce · bounce DC` at
      `bounce > 0`. One kernel (`probe_relight.wgsl`) computes both: baked at capture, live per
      frame, so a static scene's ratio is x / x = 1 exactly. Each probe texel resolves the ratio
      with Valve ambient-cube weights `n · n` over the three axes of its facing `n = −normalize(dir)`,
      `dir` being the lookup's hit relative to the capture point: a surface the probe sees roughly
      faces back toward it.
    - Known limit: direct light on the captured geometry (sun/moon, point and spot lights) is frozen at
      capture; the per-channel ambient ratio scales it too (emissive does not), so a clock that raises ambient N× brightens
      lantern-lit walls in reflections N× while the lanterns stay put, and moving a `DirectionalLight`
      or switching a lamp does not follow until a rebake.
    - Known limit: only the opaque mesh pass writes the emissive target; transparents add into the
      lit target alone. A transparent emissive (glass sign, particles) lands in T, so it still fades
      with the ambient ratio; an opaque emissive seen through a transparent keeps its full E while
      its T is attenuated, so at ratio < 1 it reflects brighter than captured and at ratio > 1 can
      fall to the `max(…, 0)` clamp.
    - Known limit: the ratio is exact for Lambertian content facing the capture point; metals and
      surfaces facing away still drift under a sky-gradient change (zenith ≠ horizon).
    - Baked E is stored with the cube (a KTX2 key/value entry, bake version 6), not divided into the
      texels: a dark capture's tiny E would overflow fp16.
    - Amended 2026-10-22: E is computed with or without a `GiVolume` (the old "ratio 1 without one"
      would freeze the ambient and `ibl_intensity` the clock moves). The dynamic injection is
      excluded: it is a graph transient, and it carries moving lights, not the clock.
    - Clock check (`games/vigil/src/clock.rs`): the moon direction and colour and the sky colours
      change in quantized steps (GI rebake, IBL re-prefilter) and ambient and `ibl_intensity` follow
      the clock continuously; E reads all of them live and the sky fraction reads the live sky cube,
      so every clock quantity is tracked. Probes capture the frozen frame-0 world, so the store key
      never sees the clock.
    - Known limits: the moon's direct light on captured geometry is tracked only through the bounce
      term; a strong local light change inside a box room, which the reference game does not do.
18. **The sky cube matches the probe face size**, with a chain down to 8×8 on decision 4's function. It is
    a probe at infinity, prefiltered by the same code path, so the hand-off from probes to the sky is
    seamless.

### Storage, baking, budget

19. **Only text is committed.**
    - The scene holds only `ReflectionProbe` components.
    - A probe's prefiltered cube lives in the ignored store, `<project>/.ludion/cooked/probes/<key>`,
      as KTX2.
    - **The key hashes the extracted capture input:**
      - the render list the capture draws: mesh ids, transforms, material parameters, texture content
        keys, lights, `Environment`, the GI result's key;
      - plus the probe's capture point (shape, blend and priority shade, they are not captured);
      - plus the quality parameters (the store is a per-machine ignored directory, so an adapter term
        only guarded a GPU swap on one machine; decision 6 holds exactly on one machine and up to GPU
        float noise across machines, the perceptual tier images already gate on, decided 2026-10-22);
      - plus a bake-version constant.

      Anything not extracted cannot affect the capture, so staleness is impossible by construction.
    - **On a miss, the engine captures at load and writes the store**, so the pixels are identical
      whether the cache exists or not. This keeps `cooked-assets.md` decision 6.
    - `ludion bake-probes <scene> [--probe <id>]` and `ludion sync` warm the store.
    - `--probes rebake` on `run`, `dev` and `serve` ignores the store.
    - Each pinned machine bakes its own probes; goldens are per-OS anyway.
20. **Export.**
    - `ludion export` bakes every probe of every shipped scene and ships the store entries.
    - It bakes against the written bundle (the game binary's `--bake-probes`, at `[ios] quality`), so
      the cooked maps' ios cook keys match the device's.
    - It **fails** with `L1281` naming the scene and probe if a key is missing from the bundle.
    - The runtime still captures on a miss, with a warning diagnostic, as a safety net.
21. **Budget.**
    - Faces are 256², `Rgba16Float`, with a mip chain down to 8×8, about 4 MB each. LOW and iOS use 128².
    - A scene may hold **at most 32 probes**; more is an `L####` error naming them.
    - The shader addresses probes through a **probe → slot indirection** from day one, so
      camera-driven residency (streaming) can be added without rewriting probe addressing.
    - **Streaming trigger:** a reference-game scene needs more than the cap.
22. **Verification paths capture fresh.**
    - `ludion render`, `ludion test`, goldens, `--render-out` and `--replay-frames` ignore the store,
      so committed images never depend on a cache.
    - **Trigger to use the store there:** W3's `ProfileReport` shows probe capture cost hurting the
      render loop. The switch then lands with a cold-vs-warm byte-identity gate on the reference
      game's probe scenes, which proves the key is complete.
23. **Scene checks** (`SemanticValidate`, each with an `L####` code, JSON Pointer paths to every
    probe involved, and a suggested fix):
    - a **warning** for an equal-rank overlap (same priority and volume, so the order falls to
      `EntityId`; suggest `priority`);
    - a **warning** for a shadowed probe, one that is fully covered at weight 1 by higher-ranked
      probes and so never contributes;
    - an **error** for a near-duplicate (same shape, extent and transform within epsilon).

### Tunables and observability

24. **Look versus cost.**
    - **The look** is authored per scene in the existing `PostProcess` component: the SSR fade band,
      the trace's maximum distance, the thickness, and the reflection intensity.
    - **The cost** is a `Quality` matter: an `ssr` toggle (off at LOW, on at MEDIUM and above, and
      off in the iOS export preset), plus `ssr_scale` and the probe face size in `QualityParams`.
    - **Amended (W2, 2026-10-21):** the look is opt-in per scene through `PostProcess.ssr` (default 0, scaling
      the confidence). There is no iOS preset: "off on iOS" is `[ios] quality = "low"`. The probe
      face size stays W3's axis.
    - All of it is render-only: no `WorldHash` or replay effect.
25. **Agent observability:**
    - **debug views:** reflection-only, SSR confidence, the winning probe per pixel, and the
      footprint mip;
    - **`--dump-probes`:** each probe's captured and prefiltered cube as PNG strips, plus a JSON
      manifest (key, slot, hit/miss, timings);
    - **probe volumes** as a `render.debug` category: wireframe boxes and spheres plus capture
      points;
    - **`ProfileReport` fields:** the `reflections` pass GPU time, probe bake milliseconds on a miss,
      and cache hits.
26. **Editor.**
    - `ReflectionProbe` joins the palette and is edited through the existing component editing.
    - Volumes show through the `render.debug` category.
    - Nothing is editor-private. With the store, an edit followed by a reload rebakes, so no Rebake
      button is needed.

### Verification

27. **The parity gate.**
    - The scene is a closed box room with a box probe fitted exactly to its walls, where parallax
      correction is exact, and floor strips at several roughness bands, one of them inside the SSR
      fade band.
    - It renders SSR-only and probe-only, then SSIM-compares each band over the region where SSR is
      fully confident.
    - **The floors are calibrated on the first green implementation** and committed per band with a
      small margin. A regression fails the build; an improvement is tightened by hand.
    - The gate runs in `check.mjs` and CI.
28. **The human gate** is one sitting on a rain street in the reference game.

### PBR correctness

29. **SSR sees the fill; `reflections` is split, not duplicated.**
    - The SSR colour pyramid used to seed from the scene before any indirect specular, so a metal
      reflected by SSR went black and popped as confidence faded into the probe path.
    - `reflections` keeps the one fill and, with SSR on, writes it to two targets (`scene` additively,
      `refl_fill` replacing). The pyramid seeds from the lit scene. `ssr_blend` then adds
      `confidence × (hdr_safe(hit·(F0·ab.x + ab.y)) − fill)`, which ends at `mix(fill, hit, confidence)`.
    - Why not add the fill inside the seed: the fill is still evaluated once (the extras are an 8 B/px
      MRT write and `ssr_blend`'s upsample, against a second full fill evaluation per render pixel plus
      a 13-binding seed); the pyramid is exactly the pixel an SSR-off camera shows; and there stays one
      fill.
    - Containment: SSR-off frames and probe captures run the unchanged single-target path, and a
      zero-confidence pixel adds exactly 0. Probe-store keys moved once with the shader text.
    - Codegen note: factoring the fill into a function drifts Environment scenes by at most 1 LSB on a
      few pixels, and one captured probe cube by the same last bit (verified math-neutral: inlining the
      old body restores byte identity).
    - Limit: probe-capture faces carry no probes, so a metal inside a capture holds only the sky's
      fill. In an enclosed room a reflected metal can therefore still differ between SSR and the probe
      (decision 17's multi-bounce limit, out of scope). The parity test over open sky
      (`needs_gpu_ssr_over_probes_hands_off_without_a_pop`) measures the hand-off.

30. **One DFG, energy-compensated.**
    - The split-sum LUT integrates the direct lobe's own height-correlated Smith visibility
      (`snippets/dfg.wgsl`), so R + G is the single-scatter albedo at F0 = 1.
    - Every specular site (point/spot loop, sun, sun disc, sky fill, probes, SSR's `hit·DFG`) is scaled by
      `1 + F0·(1/(a+b) − 1)`; with an `Environment`, the IBL diffuse and ambient floor take `1 − (F0·a + b)`.
    - The direct lobes compensate with or without an `Environment`: multi-scatter is a BRDF property, so
      env-less scenes with specular move too (dielectrics by under 1% of their highlight, rough metals most).
      The mesh pass binds the LUT at `@group(3) @binding(2)`. Render-only: no bake-version bump, the store
      keys move with the shader text.
31. **Cube and probe lookups follow the lobe.**
    - The sky cube, the probe cubes and the probe's sky sample are read along
      `normalize(mix(R, n, dominant_lerp(α, ndv)))`, not the mirror ray. The bend is a four-coefficient
      closed form (`lobe.rs`, WGSL twin in `lobe.wgsl`) fitted to the full-BRDF integral over a floor
      (`fit_dominant_lerp`); Filament's and Frostbite's formulas were measured and rejected.
    - SSR keeps tracing along R: it hits real geometry on the mirror ray. Render-only; the store keys move
      with the shader text, no bake-version bump.

### Probes spawned at runtime

32. **The probe world is frozen at load and refrozen only on request** (owner approval 2026-11-14,
    Serious Isaac request 0041). A sim system sets the render-only `ProbeRefreeze` request (unhashed,
    like `ProbeSource`), and the engine refreezes at the start of the next tick as a load would, so
    probes spawned with a runtime prefab capture the world that holds them. The refreeze is explicit,
    never inferred from `Static` changes: the frozen world holds every mesh and light, and only the game
    knows the tick at which its world is complete (a room swap spans a despawn and a spawn). Bakes reach
    runtime worlds through a game-provided bake hook on the builder: the game binary's `--bake-probes`
    asks the hook for each world in turn, refreezes and bakes the probes present, and a coverage query
    (per probe: store key, hit or miss) lets a game test fail on a stale store. Partial refreezes and a
    static-change trigger are deferred, with the trigger: more than one live room, or a refreeze cost
    that breaks a fade window.

### Receivers on the surface

33. **A receiver on a probe's surface gets full weight** (owner approval 2026-11-15, Serious Isaac
    request 0047). `probe_weight` treats a point within a depth-relative tolerance
    ε(d) = `max(1e-4 · d, 1e-3)` of the volume as inside, d the point's distance from the camera, for
    box faces and sphere shells alike; with `blend > 0` the fade starts after ε. The reflections pass
    rebuilds the receiver from depth, so a floor exactly on a box face fell either side of it by float
    error and flickered between the probe and the sky fill. The projection is standard-Z
    `perspective_rh` into `Depth32Float`, whose reconstructed depth has relative error about
    `(z / near) · 2⁻²⁴`: ≈ 6e-6 at 10 m and 6e-5 at 100 m with `near = 0.1`, so 1e-4 covers receivers
    to ~170 m at the tightest near plane in use, with headroom at room scale (the error projected on a
    face normal is never larger than along the ray). The 1 mm floor covers the `inv_view_proj`
    multiply's absolute rounding (`|world| · 1.2e-7`, under 1 mm out to ~8 km) and orthographic views.
    ε is at most 1 cm out to 100 m, so no seam shows. The cluster binning sphere grows by the
    tolerance's reach (`(r0 + MIN + REL·dc) / (1 − REL)`), so a point near a box corner is never binned
    out. Constants are twinned in `probe.rs` and `snippets/probes.wgsl`; no uniform changes. The
    tolerance is no authoring margin: with `blend: 0` the weight is a hard step just past ε.

    **Proposed (Serious Isaac 0047, owner ruling owed). A coplanar-face diagnostic.** `check-scene`
    warns when a box face lies within ε of a static mesh face, the way decal receivers are checked.
    Not built: the tolerance makes the documented setup safe.

## Waves

### Wave 1 — Foundation: one blend site

- **Render graph:**
  - mipped transients (decision 13);
  - MRT (`PassDecl.color` as a list);
  - the formats the extra targets need.
- **Scene pass split:**
  - opaque writes the two extra targets and drops inline indirect specular;
  - the `reflections` pass evaluates the sky only;
  - transparents follow, sampling the sky inline through the shared snippet.
- **Lighting changes:**
  - decision 4's function, with the sky IBL migrated onto it;
  - decision 18's sky size;
  - decision 7's occlusion split.
- **Tests:**
  - the lobe function's unit test;
  - graph-shape tests for mips and MRT;
  - in a scene with no glossy content, every golden stays byte-identical, or the re-bless is
    explained pixel by pixel.
- **Re-bless:** every glossy or metal golden is re-blessed once on the reference box. The M4 half is
  owed.
- **Docs:**
  - a new `renderer-reflections` devdoc;
  - amendments to `renderer-ibl`, `renderer-frame-graph` and `renderer-passes`.
- **Exit:** `check.mjs` green on the reference box, and the frame dump shows the pass split.

### Wave 2 — SSR

- The Hi-Z pyramid and the pre-convolved colour pyramid.
- The cone trace (decision 9) at the decision-10 resolution, the fade band, and the confidence terms:
  screen edge, ray length, thickness, back-facing hits.
- The `PostProcess` look fields and the `Quality` cost fields.
- The SSR-confidence and footprint debug views, and the `ProfileReport` pass time.
- **Tests:**
  - a mirror-floor gallery golden;
  - the confidence falling to 0 off-screen, against an oracle;
  - SSR off giving the same bytes as W1.
- **Exit:** the gallery golden is blessed, and the pass time is recorded at 1080p on the reference
  box.

### Wave 3 — Probes

- **The component:** decisions 14 and 23, with validators and codes.
- **Capture and storage:** capture (decision 16), the prefilter on the shared function, the
  content-addressed store (decision 19), and capture on miss.
- **Commands:** `ludion bake-probes`, `sync` warming, `--probes rebake`, and export (decision 20).
- **Shading:** the cube array with slot indirection and the cap (decision 21); box and sphere
  parallax; coverage accumulation binned into the clustered grid (decision 15).
- **Observability:** `--dump-probes`, the probe-index view, the `render.debug` volume category, and
  the bake and cache fields in `ProfileReport`.
- **Tests:**
  - key stability: the same input gives the same key, and each input class changes it;
  - a miss and a hit give identical bytes on one machine;
  - the overlap, shadowed and duplicate diagnostics;
  - export failing on a missing key.
- **Docs:** a `docs/` topic on placing probes; the editor palette entry.
- **Exit:** a gallery scene with box and sphere probes is blessed, and a reference-game district loads
  with probes from a warm store.

### Wave 4 — Relight, the moon, the gate, the reference game

- The relight (decision 17) and the disc-light specular (decision 8).
- The parity scene and its calibrated floors (decision 27).
- Probe placement in the reference game, recorded in its own private tree.
- **Owed:** the M4 twins, and the decision-28 sitting in the owed ledger.
- **Exit:** the parity gate is green in `check.mjs`, and the sitting is queued.

### Wave 5 — PBR correctness

- A PBR-correctness review of W1–W4 (2026-10-23).
- Fix 1–4: the SSR pyramid without indirect specular (decision 29), the relight scaling emissive, the
  DFG LUT's uncorrelated visibility, no multi-scatter compensation.
- Verify 5–9 first and fix only what is confirmed; refuted ones are recorded: distance-based probe
  roughness, dominant direction, specular-occlusion form, decision 7's text, relight colour limits.
- Shipped item 2: the capture graph's scene pass writes a fourth target (`fs_capture`), a flagged probe
  stores T and E (a two-layer KTX2 entry, bake version 5; the emissive cubes are allocated only when a
  probe sees emissive), and the resolve is `T · ratio + E · (1 − ratio)` (decision 17). The probe dump
  (`read_probes`) still shows T only.
- Shipped items 3–4 (decision 30): the LUT's correlated visibility and multi-scatter energy on every
  specular site and the IBL diffuse.
- Item 5: defect CONFIRMED vs SSR (near-wall S 0.22/0.50 at r 0.2/0.35); Lagarde's full and min forms
  overshoot (near S 1.84/3.22; parity SSIM 0.92→0.66, 0.97→0.84, 0.98→0.91); not shipped.
- Shipped item 6 (decision 31): the dominant direction.
- Item 7 REFUTED: Lagarde's specular-occlusion form vs raw AO, mean error 0.200 vs 0.236 (cone) and
  0.195 vs 0.204 (crease); not adopted (`ibl.rs` pins it). Item 8: decision 7's text amended. Item 9:
  documented as decision 17's second known limit.
- Shipped item 9(b): the relight ratio is per cube axis, resolved along the surface's facing; an open
  floor under a moved zenith relights to within 0.5% of its own change where the mean ratio missed
  by 50% in red (decision 17).
- **Owed:** the M4/3090 blesses.
- **Exit:** 1–4 shipped with provisional goldens; 5–9 each verified and either fixed or recorded.

## Status

- W1 (2026-10-20): the render graph gained mipped transients (`ResourceId::mip`) and multi-colour-target
  passes (`PassDecl.color` a list); the lobe function (`lobe.rs`, `snippets/lobe.wgsl`) with a GPU test
  holding the twins together; the sky cube as `QualityParams::ibl_specular_size` (256², 128² at LOW);
  `scene` split into the opaque MRT pass → `reflections` → `transparents`, with the sky's specular and the
  decision-7 occlusion split moved out of `mesh.wgsl`. Env-less goldens are byte-identical; the gallery
  goldens pass SSIM on the M5. Decisions taken in planning: the formats (normal + roughness
  `Rgba16Float`, F0 + specular occlusion `Rgba8UnormSrgb`); passes that write the same subresource run
  in declaration order (WAW/WAR edges), because `reflections` reads `scene_depth` while `transparents`
  attaches it; `scene` keeps its name for the opaque pass so budgets keep their meaning, and
  `transparents` covers sky, markers, sprites, particles and text because no transparent meshes exist
  (L1202); the sky snippet stays binding-free; decision 4 is amended: the lobe matches K texels, so
  r = 1 lands on the 8×8 face and the chain stops there (the literal one-texel match sent rough metal
  to 1×1 faces and made a cube-face artifact on rough metal).
  Owed: the re-bless on both pinned boxes, the `transparents`/`reflections` budgets and the 256²
  re-prefilter spike, in the owed ledger.
- W2 (2026-10-22): screen-space reflections, opt-in per scene. Two half-resolution input pyramids
  (`hiz`, an R32Float min-Z pyramid of the opaque depth; `ssr_color`, the opaque colour reduced by a
  1-3-3-1 tent whose blur matches the lobe function's footprint per level); the `ssr_trace` pass marches
  one mirror ray per texel through `hiz` with a fixed iteration cap (no jitter, no history) and writes
  radiance plus a confidence that falls off at the screen edge, with ray length, on thickness misses and
  back faces; `reflections` upsamples it depth-, normal- and confidence-aware and blends it against the
  sky fill with the same split-sum term. Decisions taken in implementation: the look is opt-in through
  `PostProcess.ssr` (0–1, default 0), which scales the confidence so 0 is exactly the sky and no
  unauthored frame moves (decision 24 amended); `Quality::ssr` (off at LOW) and `QualityParams::ssr_scale`
  carry the cost, and an iOS export turns SSR off through the low preset; SSR skips all of `spec.a`
  (decision 7), so the hit flash no longer gates it — accepted, a short white emissive swamps the term.
  Observability: `ludion render --debug-view ssr_confidence|ssr_footprint` and `render.debug`'s `view`
  over the wire; the SSR passes are budgetable (an `ssr` group); `QualityProfile` reports `ssr`,
  `ssr_scale` and `ibl_specular_size`. `L1273` validates the authored fields. Every SSR-off golden is
  byte-identical. Owed: `examples/gallery/mirror-floor.yaml`'s golden on both pinned boxes and the 1080p
  pass time on the reference box (the Wave 2 Exit), in the owed ledger.
- W3 (2026-10-22): reflection probes. `ReflectionProbe { shape: box | sphere, blend, priority }` ranks by a
  static key (priority, then boxes before spheres, then smaller volume, then entity id); scene checks
  L1274–L1278 cover invalid shapes, equal-rank overlaps, shadowed probes, near-duplicates and the 32-probe
  cap. Each probe captures six world-axis faces of the world at frame 0 (meshes, sky, all lighting; sky
  visibility in alpha) and prefilters them through the chain the sky cube uses into a slotted cube array.
  `reflections` resolves probes between SSR and the sky: a per-cell rank bitmask in the clustered grid, at
  most four probes per pixel by coverage accumulation, box and sphere parallax, and only the sky-visible
  fraction scaled by `ibl_intensity` so the hand-off to the sky fill matches. The content-addressed store
  (`.ludion/cooked/probes/`, KTX2) is keyed on the extracted capture input, the quality, the shader code and a
  bake version; hosts load from it and capture on a miss (L1279), verification paths always capture fresh.
  Commands: `ludion bake-probes` and a game binary's `--bake-probes` mode, `sync` warming, `--probes rebake`,
  and `export` baking against the bundle at the target quality (L1281 on a missing entry). Observability:
  `--dump-probes`, the `probe_index` view and a `probes` debug-draw category (CLI and wire), probe bake time
  and hits in `ProfileReport`. The SSR trace's comb fringes were fixed on the way (level-0 hits resolve
  against the render pixels). Decisions taken in implementation: the store key carries no GPU adapter (the
  store is per machine; export bakes on the host like cooked textures, exact on one machine and within GPU
  float noise across machines); the probe face size reuses `ibl_specular_size`; shape, blend and priority
  never rebake. Known limits: a probe re-frozen after a
  mid-session reload or a dev restore is keyed correctly but won't match a cold boot's entry. Owed:
  `examples/gallery/probe-rooms.yaml`'s golden on both pinned boxes and the cold-store capture cost, owed. The Exit's reference-game district check moves to W4 with the real probe placement
  (user, 2026-10-22).

- W4 (2026-10-22): disc light, outward fade, live relight, parity gate. `DirectionalLight.angular_radius` (opt-in,
  degrees, default 0, L1282) gives the sun's specular a representative-point lobe, so a mirror floor shows the
  moon as a soft disc; at 0 the shader and every hash are unchanged (decision 23). A probe's `blend` now fades
  outward: weight 1 everywhere inside the volume, its surface included, ramping to 0 over `blend` outside, so a
  box fitted exactly to its walls gives them full weight with exact parallax (decision 14 amended; L1275 counts
  the fade bands, L1276 tests the container's full volume; keep `blend` at or below the wall thickness between
  two fitted rooms). Probe captures store geometry only, the sky masked to zero with its coverage in alpha, and
  the resolve fills the sky fraction from the live sky cube and scales the geometry by the live-to-baked diffuse
  irradiance ratio, so a clock stepping the sky or ambient no longer forces a rebake (decision 17; bake version
  4). The `probe_relight` compute pass evaluates that irradiance, once at capture and once per frame into a
  graph-owned `probe_live` transient, timed and budgetable like any pass. Presentation-entity probes are baked
  and frozen like simulated ones. The parity gate (decision 27) renders a closed box room twice, SSR-only and
  probe-only, at four floor roughness bands, and compares them with a masked SSIM over the texels where SSR is
  fully confident, against per-band floors in `budgets/reflection-parity-<os>.toml`; the same file records the
  sky-to-probe seam per band, reported, never checked (decision 18). The reference game has agent-placed probes
  on one rain street. Known limits: direct light on captured geometry is frozen at capture and only scaled by the
  ambient ratio, so moving a light or switching a lamp waits for a rebake (decision 17); the sky-to-probe seam is
  measured, not fixed. Owed: the parity floors on both pinned boxes (Windows fails with a message naming it
  until then) and the re-blessed provisional probe-rooms golden, in the owed ledger.

- Reference box (2026-10-27): the RTX 3090 half of W1–W5's owed items. Windows goldens blessed: the new
  `mirror-floor`, `probe-rooms` and `ssr-probe-metal`; `material-park`, `night-sky` and `post-full` (W5's
  lookup table); every other specular golden at ±1 LSB (29 gallery scenes in all, the voxel contact sheets,
  both import exit gates); sprite and text goldens byte-identical. Parity floors calibrated in
  `budgets/reflection-parity-windows.toml`, each band 0.0015–0.008 above the M5's. The Wave 2 Exit's pass
  time at 1080p: the `ssr` group 0.433 ms (`ssr_trace` 0.241, `ssr_blend` 0.086), `reflections` 0.051,
  gated by the new `budgets/pinned-rtx3090-mirror-floor.toml`. The opaque/transparent split re-cut the
  stress budgets (particles and sprites now cost `transparents`) and moved `lights.yaml`'s `scene`
  0.061 → 0.082 ms, the MRT targets' bandwidth. A cold probe capture costs 11–14 ms per probe at full,
  about 9 at `low`, ~4% of the load, so decision 22's trigger is not met. The fp16 sum (One+One over a
  ceiling-bright surface) reads the ceiling on the 3090 (`needs_gpu_a_reflection_over_a_bright_surface_reads_the_ceiling`).
- Pinned M4 (2026-10-26): the M4 twins. The macOS goldens, provisional M5 candidates until now, are
  re-blessed on the M4 after an unblessed run under the full gate: every specular golden had drifted
  from the M5 by ±1 LSB (SSIM 0.9987–0.9999), and the one red, `night-sky`'s centre tile (worst-window
  0.78 on a flat sky gradient, colour error 0.16), showed no visible difference. The sprite goldens,
  the gallery's `text`, the game goldens, `material-park` and both import exit gates were byte-identical to the M5's and did
  not move. Parity floors in `budgets/reflection-parity-macos.toml` read within 0.0001 of the M5's. The
  new `budgets/pinned-apple-m4-mirror-floor.toml` gates the SSR group and `reflections` (Metal's smeared
  per-pass readings: `ssr` 14.9 ms summed, not a frame time), the budgets-macos configuration runs it,
  and the `lights` and stress files gain `reflections`/`transparents` lines; `lights.yaml`'s `bloom`
  ceiling is re-cut because the smear through the new passes ahead of it moved its reading 1.94 → 3.59 ms
  with no bloom change. The 256² sky re-prefilter spike costs 2.7 ms of a 1080p frame (1.3 ms at 128²,
  the `low` preset). A cold probe capture costs ~16 ms per probe at full and ~9 at `low`, 31–33 ms of
  `probe-rooms`' 79 ms load; that is load time, not the render loop, so decision 22's trigger is still not
  met. Metal saturates the fp16 store at ±65504 like the 3090 (stores of 1e5 and 1e30 and a One+One sum
  past range all read `0x7BFF`), so `HDR_CEILING` and `hdr_safe` are defensive on both pinned GPUs.
  Owed: the spike on an iOS device, with `Plan/ios-target.md` M3.

- Decision 32 (2026-11-14, Serious Isaac 0041, r2351–r2354): `ProbeRefreeze`, drained in
  `freeze_probe_world_before_tick`; `EngineBuilder::probe_bake_worlds` walks hook worlds under
  `--bake-probes`, labelling each probe's `world` in the report; `Engine::probe_coverage` +
  `ProbeCoverage::check` (`L1279`). The store key had followed entity-slot history, so a room spawned
  after a different room order missed: `world_key` now hashes canonically sorted records with assets and
  bone ranges by content (r2353), and the GI voxelizer combines shared cells in signature order rather
  than slot order (r2354). GPU tests hold a runtime-swapped room's key and faces to a fresh world's, with
  and without a `GiVolume`. Usage: `ludion docs reflection-probes` "Probes spawned at runtime". Owed:
  pinned GI re-check and export reach.
  0042: a probe's store key cannot be predicted from a prefab, so the prewarm does not touch probes; the swap-tick refreeze plus a store hit is the probe path (relay-approved 2026-11-14).

- Decision 33 (2026-11-16, Serious Isaac 0047): `probe_weight` takes the view distance and counts a
  point within `max(1e-4 · d, 1e-3)` of the volume as inside (`probe.rs` twin, `probes.wgsl`, both
  callers); `probe_masks` bins the widened reach. A GPU test sweeps 31 poses over a floor on a box's
  bottom face. Usage: `ludion docs reflection-probes` Rules. Owed: pinned re-bless of any golden or
  parity floor with a receiver on a probe face (`probe-rooms`, `ssr-probe-metal`, `decals`).

## Deferred, with triggers

- **Temporal SSR:** decision 12's trigger.
- **Probe streaming (camera-driven residency):** decision 21's trigger.
- **Cached probes on verification paths:** decision 22's trigger.
- **Partial or static-driven probe refreeze:** decision 32's trigger.
- **A separate projection box:** a real level whose influence volume and room shape differ, and a
  sitting judges the single box wrong.
- **Real-time probe recapture:** a lighting change the relight in decision 17 cannot express, such as
  a lamp switching off inside a box room, in shipped content.
- **Compressed HDR probe storage (BC6H / ASTC-HDR):** probe-store size becomes a problem for the
  export bundle.
- **Distance-based probe roughness (Lagarde):** a non-SSR oracle (a CPU ray-traced room) to choose the form.
- **GTSO-style specular occlusion (cone–cone, bent normal):** a reviewed crease or contact look that
  raw AO visibly fails.
- **Specular from voxel cone tracing:** `global-illumination.md`'s existing trigger, unchanged.

## Risks

- **The Wave 1 re-bless is whole-suite for glossy content.** If a golden moves beyond what the
  occlusion split and the lobe migration explain, that is a regression, not a re-bless.
- **Fragment-stage texture budget.** The transparent path's inline probe sampling has to fit the
  16-texture floor once group 3's cube bindings move.
- **Capture cost on a cold store.** 32 probes × 6 faces of full lit scene rendering at 256²
  lands on scene load (`Plan/scene-load-speed.md`'s `load` report). It needs measuring in W3 before
  the budgets move.
- **An incomplete cache key** would show a stale probe on interactive hosts only. Goldens capture
  fresh, so they cannot catch it; decision 22's identity gate is the eventual proof.
