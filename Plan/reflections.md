# Plan: Reflections — screen-space reflections over box and sphere probes

> Status: **W1 mechanized (2026-10-20); W2 mechanized (2026-10-22); W3 mechanized (2026-10-22); W4 waits on the user's word.** Design settled 2026-10-19. Decisions come from a grilling session with
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
   - The full-screen `reflections` pass evaluates SSR, box probes, sphere probes and the sky in one
     shader, blends them, and adds the result to `scene`.
   - With SSR off, the same pass runs with confidence 0. There is one code path, not an inline path
     plus a correction.
   - This also moves the cube bindings out of the mesh shader's nearly full texture budget.
6. **Receivers.**
   - Opaque meshes get the full chain.
   - Transparent meshes sample probes and the sky **inline**, through the same WGSL snippet the
     `reflections` pass uses. They get no SSR.
   - Sprites, particles, text and ground markers get no specular reflections.
7. **Specular occlusion is split by source.**
   - GI sky visibility darkens **only** the sky fill: the remainder after probe coverage, plus the
     sky-masked texels inside probes. Probes already contain their own occlusion.
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
   - The trace input is the opaque scene colour before reflections are added, so reflections are
     single-bounce.
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
      sky mask, so after prefiltering rgb is geometry radiance and alpha the sky fraction. The resolve
      is `rgb · ratio + a · sky_cube(r) · k`, the sky cube being the live sky specular cube along the
      reflection ray and `k` = `ibl_intensity` with an `Environment`, 0 without one.
    - `ratio` = live E / baked E per channel (1 where baked E ≤ 1e-6), E being the normal-free
      diffuse irradiance at the probe's capture point, composed like `mesh.wgsl`'s ambient term with
      `diffuse = 1` and no occlusion: `k · mean(irradiance cube at ±X ±Y ±Z) + ambient`, times
      `mix(1, sky visibility DC, sky_visibility)` with a `GiVolume`, plus `bounce · bounce DC` at
      `bounce > 0`. One kernel (`probe_relight.wgsl`) computes both: baked at capture, live per
      frame, so a static scene's ratio is x / x = 1 exactly.
    - Baked E is stored with the cube (a KTX2 key/value entry, bake version 4), not divided into the
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
- **Owed:** the M4 twins, and the decision-28 sitting in `Plan/owed.md`.
- **Exit:** the parity gate is green in `check.mjs`, and the sitting is queued.

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
  re-prefilter spike, in `Plan/owed.md`.
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
  pass time on the reference box (the Wave 2 Exit), in `Plan/owed.md`.
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
  never rebake. Known limits: a box fitted exactly to its walls gives those walls no weight (to be judged
  with decision 27's parity scene, against the deferred separate projection box); a probe re-frozen after a
  mid-session reload or a dev restore is keyed correctly but won't match a cold boot's entry. Owed:
  `examples/gallery/probe-rooms.yaml`'s golden on both pinned boxes and the cold-store capture cost, in
  `Plan/owed.md`. The Exit's reference-game district check moves to W4 with the real probe placement
  (user, 2026-10-22).

## Deferred, with triggers

- **Temporal SSR:** decision 12's trigger.
- **Probe streaming (camera-driven residency):** decision 21's trigger.
- **Cached probes on verification paths:** decision 22's trigger.
- **A separate projection box:** a real level whose influence volume and room shape differ, and a
  sitting judges the single box wrong.
- **Real-time probe recapture:** a lighting change the relight in decision 17 cannot express, such as
  a lamp switching off inside a box room, in shipped content.
- **Compressed HDR probe storage (BC6H / ASTC-HDR):** probe-store size becomes a problem for the
  export bundle.
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
