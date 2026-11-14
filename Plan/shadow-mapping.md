# Shadow mapping: the sun and its cascades, drop shadows and local lights

> Status: **Waves 1–5 landed 2026-07-17** (Lore revs 228–231 for Waves 1–4; Wave 5 docs +
> bookkeeping same day). Decisions settled with the user 2026-07-17 (shadow source = sun only;
> golden policy = one pre-authorized bless wave). This plan started the graphics-engine feature's
> second workstream. The roadmap's recorded trigger for shadow
> mapping (textured-PBR trigger, `Plan/roadmap-voxel-mesh-pbr.md`) was **jumped by user direction
> 2026-07-17** — the vigil arc's M6 city / M7 Spectacle is the anticipated content demand and this
> work ran ahead of it. Texture maps and IBL stay behind the original trigger. **Human gate PASSED
> 2026-07-17** — user looked at the sun-shadows result and confirmed shadows are working. **Open:**
> the two macOS re-blesses (`sun-shadows.png` new, `ui-hud-1080.png` re-bless) queued for the next
> mac session. **Decision 8 (Clockout 0006) mechanized 2026-10-29:** the `DropShadow` component, the
> `drop_shadow_occlusion` and `projector` passes and the `drop-shadow` gallery golden; the macOS twin is owed.
> **Clockout 0007 (local-light shadows) mechanized 2026-10-30 at Lore r1780–r1790** (decision 1 fired, 3 and 6
> extended, 9–11 new; replays re-pinned to the new wire hash at r1788); the macOS golden twin is owed, and the darkness look gate is the game's.
> **Clockout 0011 (sun cascades) mechanized 2026-10-30 at Lore r1792–r1800** (decisions 1, 3, 4, 6 amended; 12–17 new);
> the replays are re-pinned to the new wire hash at r1798, review fixes at r1800. The macOS twins, the M4 GPU tests and the iOS/Metal build are owed.
> **Clockout 0034 (local-light shadow distance fade) mechanized 2026-11-02 at Lore r2009:** `shadow_distance`/`shadow_fade` on `PointLight`/`SpotLight`, `L1322`/`L1323`, strength in `LocalView.params.w`; decisions 9, 11 amended; wire hash and goldens unmoved; the M4 run of the GPU sweep test is owed.

## Goal

A single directional "sun" light with one orthographic shadow map, received by the mesh path
only. Depth becomes a first-class frame-graph resource first (the prerequisite recorded in
`Plan/phase-11-pbr.md` "out of scope" and engine audit 2026-07-17 §2). Point lights stay
unshadowed unless they opt in (decision 9); sprites/billboards/text stay Lambert and untouched. The sun can
opt in to camera-fitted cascades (decisions 12–17); the anchored box stays the default.

## Current-code facts (gathered 2026-07-17)

- **Depth today**: `const DEPTH_FORMAT: Depth32Float` (`render.rs:42`, "renderer-managed, not a
  graph resource"); allocated fresh per frame inside `record_scene` (`render.rs:1112-1126`),
  `RENDER_ATTACHMENT`-only, cleared to 1.0, `StoreOp::Discard`. Only the scene pass touches it.
- **Graph can't express depth**: `TextureFormat` has only `Rgba8UnormSrgb`/`Rgba16Float`
  (`resource.rs:61-70`); `TextureUsage` = RenderAttachment/TextureBinding/CopySrc; `PassDecl`
  writes exactly one color target; the executor's `RecordedPass::device` escape hatch exists
  precisely so recorders can self-allocate depth (`execute.rs:460-462`).
- **Pass list**: scene → bloom_threshold → bloom_blur_h → bloom_blur_v → tonemap (+ conditional
  overlay when UI present; UI-free scenes keep the exact 5-pass graph — the conditional-pass
  precedent this plan reuses for the shadow pass).
- **Lighting**: `PointLight` is the only light component. `Lights` uniform = 544 bytes at
  `@group(1) @binding(0)`, shared by mesh **and sprite** via the `lights.wgsl` snippet;
  changing its layout moves sprite goldens too — hence decision 3 below.
- **Bindgen**: WGSL is source of truth; `shader_gen.rs` generated; `ShaderId` (9 variants) ↔
  bindgen `COMPOSITIONS` are mirrored registries with a byte-comparing drift test. Adding a
  shader touches: `ShaderId` enum/ALL/file_name/body_source/snippets, `COMPOSITIONS`, BGL
  fields + `Bgls`, pipeline build/set match arms, graph pass label + `add_external_pass` +
  recorder match arm, regenerate.
- **Goldens**: ~36 PNGs, per-OS (`goldens/windows/` RTX 3090, `goldens/macos/` M4). Phase 11
  precedent for a pixel-moving wave: verify non-movers byte-identical (git status untouched +
  SSIM 1.0000), re-bless movers with committed `.diff.json` evidence, macOS re-bless queued for
  the next mac session (`Plan/phase-11-wave4-bless.md`).
- **No renderer settings surface**: tunables enter as component fields → extract → plain
  `RenderScene` data. Renderer sees no ECS types (crate boundary).

## Settled decisions

1. **Sun only.** New `DirectionalLight` component; one 2048² `Depth32Float` orthographic shadow
   map, PCF 3×3 via a comparison sampler, consumed by `mesh.wgsl` only. Point-light shadows
   (cube maps, array textures) got a recorded trigger, first content where the sun can't sell
   the scene; it **fired** with Clockout request 0007 (approved 2026-10-28), spots included
   (decisions 9–11). The map may instead be up to 4 cascades in one 2D atlas, each a `shadow_map_size` tile (decision 13). Sprites/billboards receiving shadows: recorded
   extension, decided if the HD-2D grounding visibly suffers at M6 (amending Phase 11's
   "sprites stay Lambert" is a deliberate act, not a default).
2. **One pre-authorized pixel-moving wave** (Phase 11 precedent). Sun shading + shadow sampling
   land together in Wave 3; movers classified empirically, re-blessed once on the pinned RTX
   3090 with committed diff reports; sunless scenes are *expected* byte-identical (+0.0
   contributions) but this is verified, not assumed. macOS re-bless queued for the next mac
   session. No shader permutations — one shading model (roadmap decision 8).
   *Amended 2026-10-30 (owner approval 2026-10-29, Clockout 0010): the caster pass gains a `shadow_cutout` variant whose fragment stage binds the asset's material, and a cull-none variant, selected per draw by `draw_casters` for every caster pass. The shading model stays one (flag bits plus pipeline state, like the mirrored twin), so roadmap decision 8 holds.*
3. **The sun never touches the shared `Lights` layout.** Sun params (direction, color·intensity,
   light-space matrix) + shadow map + comparison sampler live in a new mesh-only bind group
   (group 2) fed by a new `sun.wgsl` snippet or mesh-local declarations (Wave 3 picks, guided
   by bindgen unification rules). Sprite/text BGLs, pipelines, and goldens stay untouched by
   construction. The depth-only shadow shader carries its own light-space uniform. Local-light shadows bind in the
   same mesh-only group 2, beside the sun (bindings 3–5), never in group 1: `Light` stays 48 bytes and
   the GI and sprite layouts are untouched. The cascade lanes stay in the group-2 `Sun` and the `Shafts` uniform (decision 14).
4. **`DirectionalLight` carries an explicit `direction: [x,y,z]` field** (normalized at
   extract, L-coded validation error if zero), not Transform rotation — quaternion authoring is
   hostile to hand-edited YAML and the component is agent-legible as text. The entity's
   `Transform.translation` anchors the shadow frustum *center* (amended 2026-07-17 at Wave 2
   brief time, from "position is ignored": still explicit — no scene-dependent auto-fit — but a
   game can parent the sun to the player so the shadow window follows the action; a static scene
   just leaves it at the origin). Shadow tunables are explicit fields with defaults: `shadow_extent` (ortho
   half-size, default 40.0), `shadow_near`/`shadow_far` (1.0/200.0), `shadow_bias` (default
   pinned in Wave 3 against acne/peter-panning on the gallery scene). No auto-fit frustum —
   explicit beats scene-dependent wobble, and it's deterministic trivially. "No auto-fit frustum" is
   superseded for an opt-in camera fit (decisions 12 and 15); the anchored box stays the default.
5. **Multiple suns: extract takes the lowest-EntityId one** (pinned deterministic order),
   ignores the rest. No hard error — same posture as the 16-light cap.
6. **The shadow pass is conditional on a sun being present** (the UI-overlay precedent):
   sunless scenes keep the exact 5/6-pass graph. `RenderScene.sun: Option<SunData>`. The
   `local_shadow` pass is likewise conditional on at least one granted light, and the same `shadows`
   quality toggle masks it. With cascades the same pass and transient hold a larger atlas (decision 13).
7. **Trigger bookkeeping**: shadow mapping leaves the textured-PBR trigger bundle; texture
   maps + IBL remain behind it (`roadmap-voxel-mesh-pbr.md` amended in Wave 5).
8. **A drop shadow is a projection, not a light** (Clockout request 0006, approved 2026-10-28).
   The `DropShadow` component, the conditional `drop_shadow_occlusion` pass and the renderer's generic
   `projector` pass sit beside the sun:
   - It does not touch the shared `Lights` layout or the sun's mesh group 2, so decision 3 holds and
     `mesh.wgsl` is unchanged.
   - It follows decision 6's conditional-pass rule: no `DropShadow` means a byte-identical graph.
   - It does not move decision 1's light-shadow trigger in either direction. Light shadows are
     request 0007's scope.
   - It is exempt from the `shadows` quality knob.
   - The drop shadow's screen pass is the renderer's generic projector pass, which Clockout request
     0012's decals extend; the first-surface occlusion stays a drop-shadow stage, keyed per projector
     instance (amendment A1, approved 2026-10-29). Shipped: the projector family gained decal clients
     (multiply, add) and a second slot, `decals_lit`, before `reflections` for lit decals. Order: lit decals,
     then multiply and add, then every drop shadow; ascending layer, lowest EntityId last within a layer.

   *Why:* a readability cue must not depend on the scene having a sun, on where the sun points, or
   on the machine's quality.
9. **Local-light shadows are opt-in under a camera-ranked caster budget** (Clockout request 0007).
   `cast_shadows` on `PointLight` and `SpotLight`. Each frame, candidates are the opted-in lights whose
   cluster footprint touches the camera frustum (a light past its `shadow_distance` is not a candidate), ranked by `max(0, |camera − centre| − radius)`, ties
   to light-list order. They are granted whole sets of tiles from an 8×8-cell atlas: a spot a 2×2
   block, a point light (or a spot over 60°) six one-cell cube faces; a light that does not fit is
   skipped and later candidates are still tried.

   *Why:* the plan is deterministic per frame. It is the first render decision that depends on the
   camera, so a golden's casters depend on its pose.
10. **The budget is quality-invariant.** 16 spot tiles at every quality; only `local_shadow_atlas_size`
    scales (4096 full/high, 2048 medium; `low` masks it with the sun).

    *Why:* a slower machine never changes which lamps cast.
11. **The open questions, settled.** Six cube faces in the atlas (paraboloids bend straight edges); a
    spot wider than 60° takes the omni path (authoring never fails); no hysteresis, and an opt-in per-light distance
    fade (Clockout 0034: its level has 365 lamps and casts from 84 to stay in budget; a stateless fade
    keeps the reason for rejecting hysteresis, no history between frames; per light only, decision 10
    stands, no quality axis scales the distance); no static
    caching; distance rank; per-view CPU culling in the `cull` module, which Clockout request 0011 reuses.
    Out of scope kept: sprites, fog, GI.
12. **An opt-in camera fit sits beside the explicit box** (Clockout request 0011, amendment 1). `cascades`,
    `shadow_distance` and `cascade_splits` on `DirectionalLight`; absent, the anchored box is byte-identical.

    *Why:* the fit follows the camera, not the scene, and snapping removes the wobble, so both of decision 4's
    reasons for refusing an auto-fit are met.
13. **Up to 4 cascades in one 2D atlas** (amendment 2): tiles of `shadow_map_size` laid out 1×1, 2×1 or 2×2, no
    array textures. The sun keeps its own atlas, apart from decision 9's, so lamps never compete with it.
14. **The data lives in group 2 and the shafts uniform** (amendment 3), with the same pass and transient.
    `mesh.wgsl` and `shafts.wgsl` sample the cascades; the `Sun` uniform grew to 528 B and `Shafts` to 560 B.
15. **The map is fitted to the active camera** (amendment 4): the slice's bounding sphere, a world-locked snap with
    a 1-texel margin, practical splits at λ 0.75, the last cascade extended to the shaft reach.

    *Why:* this extends decision 9's camera dependence from which lamps cast to where the sun's map lies.
16. **The cascade count is per quality level** (amendment 5, owner ruling): cap 4/3/2/2, overridable in TOML,
    splits re-fitted. It departs from decision 10 for the sun only, and the last far bound is invariant.
17. **The open questions, settled.** The cascade is picked by view depth (one rule for meshes and shafts); a 10%
    seam band and end fade on meshes, a hard pick in the shafts; bias held per cascade at 80/2048 texels; the box
    carries a 1-texel margin; the sun keeps its own atlas.

## Waves

- **Wave 1 — Depth as a first-class graph resource** (zero-pixel, the recorded prerequisite).
  `ludion-render-graph`: `TextureFormat::Depth32Float`, depth-capable usage, `TextureDesc`
  constructor; `PassDecl` gains an optional depth attachment (write) and depth resources can be
  read (sampled) by later passes; scheduler orders on depth edges like color; aliasing policy
  for depth textures; executor creates/attaches graph-owned depth; `--dump-graph` schema grows
  deliberately (snapshot tests updated). `ludion-renderer`: scene pass's private depth texture
  migrates into the graph (`StoreOp` stays Discard for the main depth — nothing reads it yet).
  Exit: all goldens byte-identical, dump-graph snapshot diff reviewed, `check.mjs` green.
- **Wave 2 — Sun data path, no pixels.** `DirectionalLight` component (register, schema, canonical
  YAML, inspector palette entry), extract → `RenderScene.sun: Option<SunData>` (normalized
  direction, premultiplied color, light-space matrix math in `ludion-renderer`'s scene types or
  extract — deterministic, unit-tested), validation L-code for zero direction. No shader reads
  it yet (consumed next wave, same feature — the no-dead-fields rule is satisfied at feature
  close, per Phase 11's own wave split). Exit: component round-trips scene YAML byte-stably,
  WorldHash unmoved for existing scenes, goldens byte-identical.
- **Wave 3 — Shadow pass + sun shading (THE pixel-moving wave).** *Amended 2026-10-30 (owner approval 2026-10-29, Clockout 0010): the caster pass later gained `shadow_cutout.wgsl` and a cull-none variant (decision 2).* New depth-only `shadow.wgsl`
  (all registry touchpoints incl. hot-reload `ShaderId`), conditional `shadow` graph pass
  writing a graph-owned 2048² depth resource sampled by the scene pass; mesh group-2 bindings
  (sun uniform + shadow map + comparison sampler); `mesh.wgsl` gains the sun BRDF term (same
  Cook-Torrance, no attenuation) × PCF shadow factor; bindgen regenerate + drift gates. Movers
  classified: non-movers proven byte-identical, movers re-blessed with committed diff JSONs.
  Exit: `check.mjs --full` green, bless evidence committed, macOS queue note recorded.
- **Wave 4 — Content + perf.** Gallery `sun-shadows` scene (voxel/mesh content casting long
  shadows, a point light coexisting) + new blessed golden; vigil untouched (its sun arrives
  with M6 content); `ludion profile` sanity on the vigil budget scene (shadow pass cost
  recorded; budget TOMLs unchanged unless the number says otherwise). Exit: new golden green at
  SSIM 1.0, profile numbers recorded here.
- **Wave 5 — Docs + bookkeeping.** Engine-guide chapter ("Sun and shadows"), agent-docs topics
  (`ludion-renderer` shader-authoring + pass list, `ludion-render-graph` depth resources,
  `designer-flow` sun tuning), roadmap amendment (decision 7), feature notebook, this plan's
  status header. Human gate queued: a look at the sun-shadows golden (windowed or PNG).

## Risks

- **Shader-count growth vs hot reload**: `shadow.wgsl` is the 10th `ShaderId`; the 1:1
  shader→pipeline rule holds (depth-only pipeline). Reload interface-immutability already
  rejects binding-shape edits (L2009) — new bindings ship by rebuild, as designed.
- **Acne/peter-panning taste**: bias defaults are pinned in Wave 3 against the gallery scene;
  the failure path is the Phase 11 rule — tune content/defaults first, re-pin with a deliberate
  second bless only if the human gate fails.
- **dump-graph schema is consumed by snapshot tests** and possibly external readers; Wave 1
  changes it once, deliberately, with the diff in the commit message.
- **schema_hash moves** (new component) — in-repo clients rebuild together (E1/E3 precedent);
  the audit's strict-connect-gate concern is unchanged by this plan.

## Wave 4 outcomes (landed 2026-07-17, RTX 3090)

- **Gallery golden**: `examples/gallery/sun-shadows.yaml` → `examples/gallery/goldens/windows/sun-shadows.png`
  (960×540), blessed and re-verified at **SSIM 1.0000** (threshold 0.99), twice. A raking sun
  (`direction [0.55, -0.42, -0.30]`, intensity 2.4) throws long shadows rightward: a tall pillar's
  shadow falls across a lower block (object-on-object), the voxel lantern + a rough and a metallic
  sphere ground their own shadows, the metallic sphere catches the sun specular; one warm `PointLight`
  (intensity 2.6) coexists on the lantern, unshadowed. macOS twin queued for the next mac session
  (no `goldens/macos/sun-shadows.png` created — the ui-hud precedent: directory discovery pairs only
  the current platform's golden, so the Windows suite is unaffected by the absent macOS file).
- **Shadow bias**: default `0.002` reads clean in this scene — no acne, no peter-panning at 2048².
  No per-scene `shadow_bias` override.
- **Budget gate** (`cargo run --release -p vigil -- --scene scenes/budget-horde.yaml --profile 120
  --profile-check budgets/pinned-rtx3090.toml`, from `games/vigil/`): **GREEN**, all timings within
  budget. budget-horde has no sun → no shadow pass. sim frame total median **0.838 ms**; GPU `scene`
  pass 0.231 ms. Unchanged from the M1 measurement (~0.82 ms / 0.22 ms).
- **Shadow-pass cost** (scratch copy of budget-horde with one sun added, same 120-frame profile):
  GPU `shadow` pass **0.084 ms**; `scene` pass grows **0.231 → 0.301 ms** (+0.070 ms for shadow
  sampling + the sun BRDF term); sim frame total **0.838 → 0.852 ms** (sim-side sun cost negligible).
  Total added GPU ≈ 0.15 ms. Budget TOMLs unchanged.
- **Perf accounting**: `pass::SHADOW` added to `KNOWN_PASS_NAMES` in
  `crates/ludion-engine/src/profile.rs` (a `[passes] shadow` budget line on a sunless scene is now
  producible-but-unmeasured, not an L4006 — covered by `producible_but_unmeasured_pass_is_not_a_violation`).

## Drop shadow outcomes (Clockout 0006)

- **Gallery golden**: `examples/gallery/drop-shadow.yaml` → `examples/gallery/goldens/windows/drop-shadow.png`
  (960×540), blessed with `cargo run -p ludion-cli -- test --dir examples/gallery --filter drop-shadow --bless`,
  re-verified at **SSIM 1.0000**. No sun; three point lights. Each character is an unscaled root, a scaled
  body child and a `DropShadow` child at the soles (the footprint takes the world scale). Cases: (a) a
  two-level step, the disc cut at the lip and continuing on the lower floor; (b) a thin floating platform,
  the blob on its top and the floor beneath clean; (c) a six-metre pit open toward the camera, the blob on
  its floor; (d) a 15° ramp, the whole blob on the ramp; (e) a faded case (`max_distance 4`,
  `fade_distance 2`, soles 3.4 m up), a faint blob; (f) a `GroundMarker` half over a blob, the marker on
  top; (g) a yawed segment on the floor. `--quality low` shows every blob.
- **Slope fix**: the first golden attempt read a ramp with its uphill half cut, because `kept` tested the
  pixel's own height against the anchor. The projector now tests the receiver plane extended to under the
  anchor (`y_under`), so a ramp the caster stands on takes the whole blob and a step top above it never does;
  `needs_gpu_a_drop_shadow_wraps_a_slope_and_spares_a_step_above` pins both.
- **Cost** (`cargo run --release -p ludion-cli -- profile --scene examples/gallery/drop-shadow.yaml --frames 120`,
  1080p, full preset, RTX 3090; the scratch twin has the component removed from the seven children):
  GPU `drop_shadow_occlusion` **0.004 ms**, `projector` **0.010 ms**; `scene` 0.104 ms both ways; the
  summed pass medians 0.253 ms with and 0.238 ms without. The occlusion pass draws every mesh per caster
  (seven casters here); a scene with hundreds of casters would be where to cull.
- **Follow-up, not done**: the occlusion pass draws every mesh per caster; cull per caster to the tile's
  footprint box, measured before the game adds many blob casters.
- **Plane-test normal**: the `y_under` test reads a normal rebuilt from the depth buffer (two taps per
  side, the side that extrapolates to the centre's depth), so normal maps and smooth normals no longer cut
  a blob's rim; where an axis has no such side (a sliver or concave corner) it falls back to the shaded
  normal. `needs_gpu_a_drop_shadow_ignores_a_perturbed_shading_normal` pins it.

## Local-light shadow outcomes (Clockout 0007)

- **Gallery golden**: `examples/gallery/local-shadows.yaml` → `examples/gallery/goldens/windows/local-shadows.png`
  (960×540), blessed with `cargo run -p ludion-cli -- test --dir examples/gallery --filter local-shadows --bless`,
  re-verified at **SSIM 1.0000**. No sun, no environment: a shadowed spot (36° outer) throws a crate and the
  mannequin, a shadowed point light ringed by three pillars throws radial shadows onto the floor and the side
  and back walls, and an unshadowed blue point light behind a short wall passes straight through it. No acne,
  light leak, cube-face seam or tile bleed seen.
- **Profile panel**: `ludion profile` carries `local_shadows` (`requested`, `granted`, `skipped`, `views`,
  `atlas_edge`) and `quality.local_shadow_atlas_size`. On the gallery scene: requested 2, granted 2,
  skipped 0, views 7 (one spot, six cube faces), atlas 4096. At `low` the panel grants nothing.
- **Cost** (`cargo run --release -p ludion-cli -- profile --scene examples/gallery/local-shadows.yaml --frames 120`,
  1080p, full preset, RTX 3090; the scratch twin has every `cast_shadows` removed): GPU `local_shadow`
  **0.031 ms**; `scene` 0.166 ms with and 0.104 ms without; the summed pass medians 0.314 ms with and
  0.237 ms without.
- **No-opt-in scenes**: `examples/gallery/lights-stress.yaml` with
  `--check budgets/pinned-rtx3090-lights-stress.toml` stays green, `scene` 0.948 ms, panel all zeros.
  vigil's `budget-horde` (`--profile 120 --profile-check budgets/pinned-rtx3090.toml`) stays green,
  `scene` 1.514 ms, panel all zeros, no `local_shadow` pass.
- Budget TOMLs unchanged.

## Sun cascade outcomes (Clockout 0011)

- **Gallery goldens**: `examples/gallery/sun-cascades.yaml` → `examples/gallery/goldens/windows/sun-cascades.png`,
  and `sun-cascades-view.png` for the `sun_cascades` render view, both re-verified at **SSIM 1.0000**. A raking
  sun, a 100 m colonnade and a figure near the camera, four cascades.
- **Profile panel**: `ludion profile` carries `sun_cascades`. On the gallery scene the splits are
  6.69 / 14.88 / 32.09 / 100 m, the texels 0.0070 / 0.0144 / 0.0310 / 0.0987 m, the casters 7 / 13 / 27 / 42.
- **Cost** (release, 120 frames, full preset; GPU ms for `shadow` / `scene` / summed pass medians): the gallery
  scene with 4 cascades 0.035 / 0.087 / 0.2365, with the single anchored box 0.0087 / 0.0737 / 0.1971.
- **Opt-in breach, informational**: vigil's `budget-horde` is 0.0809 / 1.661 / 2.563 with the single box; a
  scratch copy with 4 cascades is 0.3246 / 1.748 / 2.989, casters 311 / 506 / 640 / 1055. That is over the
  pinned-rtx3090 budget for `shadow` (0.150) and for `scene`. No committed scene opts in; one run.
- Budget TOMLs unchanged.
