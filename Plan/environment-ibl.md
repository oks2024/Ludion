# Plan: Environment, Procedural Sky, and IBL (the next render arc)

> Status: **mechanized-complete 2026-07-22** (executed as planned in four orchestrated waves, Lore
> revs 288–292; see the Status section at the end). The arc's exit gate — the queued G3–G6
> human-gate batch playing under the night sky — remains open; the arc closes on that sitting.
> Direction was settled 2026-07-21 (grill-me session with the user; every decision below was
> answered explicitly). This arc is the "PBR render arc" continuation after Phase 11 scalar
> PBR and shadow mapping: **IBL-led and G7-driven**. IBL leaves the textured-PBR bundle by user
> direction (the shadow-mapping precedent — `Plan/roadmap-voxel-mesh-pbr.md` trigger note
> updated); **texture maps alone stay behind the original trigger** — nothing here samples an
> authored image. Sequencing: **arc first, then the queued G3–G6 human-gate batch** — the batch
> plays once on the post-arc build, judging gameplay and the new look in one sitting, and the G0
> camera caveat ("wants a better viewing environment before calling it final") is resolved
> against the real sky. Runs under the graphics-engine feature stream as a standalone plan (the
> `Plan/shadow-mapping.md` shape), not a numbered engine phase.

## Decisions settled in the session (2026-07-21)

1. **Scope: IBL-led + G7's renderer needs.** Environment lighting (fixes metals-go-dark and the
   flat ambient), a visible sky, bloom/ambient knobs, and the occlusion-experiment plumbing.
   Texture maps stay behind their trigger; vigil's palette content has not demanded one.
2. **Environment source: procedural sky, never an image asset.** An analytic gradient authored
   as text, GPU-prefiltered at load into the IBL inputs (irradiance + specular mip chain + BRDF
   LUT). These are the mesh shader's first sampled textures, but they are runtime-generated from
   deterministic text inputs — no binary content, nothing an agent can't author or diff. HDRI
   was rejected explicitly (binary assets, half-fires the texture trigger through the back door).
3. **Authoring surface: an opt-in `Environment` component**, lowest-EntityId singleton (the
   `DirectionalLight` sun precedent, extract.rs:311). **Absent ⇒ today's pipeline byte-for-byte**
   (hardcoded ambient, clear color, no sky, no IBL) — the conditional-pass pattern E3 proved.
   The golden blast radius inverts: existing goldens are *asserted unchanged*; only content that
   adopts the component blesses. Fields-on-DirectionalLight and always-on-with-default-sky were
   rejected (conflation / whole-suite churn respectively).
4. **Sky model: authored gradient.** Zenith / horizon / ground colors (linear RGB, HDR-capable)
   plus an optional sun/moon disc (size, intensity; direction read from the existing sun entity).
   No physical daylight model — vigil's districts are night scenes (torch PointLights + moon
   fill) and turbidity-style parameters are opaque to agents; the author picks colors, exactly
   like palette discipline. A physical preset alongside was rejected (two ways to say one thing).
5. **`Environment` is the scene's one lighting-environment surface.** It also carries the flat
   `ambient` (default = today's extract constant `[0.05, 0.05, 0.06]` — and the extract-vs-
   `RenderScene::empty` wart at extract.rs:33 / scene.rs:255 gets unified while touched), an
   `ibl_intensity` scalar, and bloom `threshold` / `strength` (defaults 1.0 / 0.6 — today's
   literals in threshold.wgsl:13 and tonemap.wgsl:26). Every field optional with
   behavior-preserving defaults: authoring only a sky changes nothing else. A separate
   PostProcess component was rejected (second singleton before a second post feature exists).
6. **Occlusion plumbing: per-instance fade + game-side policy.** The renderer gains one generic
   per-instance fade input driving **deterministic screen-door dither** (Bayer-matrix discard —
   opaque pipeline, blend and depth-write untouched) plus an optional **world-height threshold**
   (fade only fragments above it). A render-only `Fade` component carries `(fade, height_min)`;
   WHO fades is game policy (ludion-spatial raycast camera→player, deterministic hysteresis,
   game-side — G7's job). All three G7 candidates become game-code experiments with zero further
   shader churn: dither the whole building (`fade<1`), top-fade stories (`height_min` set),
   cutaway (= fully fade the occluding wall). Shader-hardcoded cylinder cutaway and
   defer-entirely were rejected. The authored `Mesh.color` alpha is deliberately NOT repurposed
   (it is a tint channel, currently unblended on the opaque path — render.rs:2072 — and stays
   so); fade rides a new instance lane.
7. **Vigil adopts a minimal night sky in-arc** (the arc's last wave): one shared, tasteful
   moonlit-gradient `Environment` in all three districts — plaza via its map fragment
   (`district-plaza.map.yaml` `fragments:`, the hud.yaml precedent) + regenerate; market and
   cathedral by direct edit (they are hand-authored — their map conversion stays a later item).
   Costs one `run.lrp` re-record and the `district-plaza` golden re-bless (the only vigil
   golden), both proven hermetic operations; the mac re-bless queue entry updates in place.
   Full emissive set-dressing and anything theme-dependent stay G7.
8. **Plan shape: this standalone doc, four waves, one new-bless event + one vigil re-bless.**
   The human gate rides the queued G3–G6 batch sitting — no separate viewing session.

## Decisions resolved at plan-authoring (stated, objectable)

- **No sun/IBL double-counting:** the sun/moon disc renders in the sky background only and is
  **excluded from the prefilter** — the IBL integrates the gradient alone; direct sun light
  stays the analytic `DirectionalLight` (with its shadow). Otherwise sun specular counts twice.
- **The sky draws inside the existing scene pass** — a final fullscreen triangle at far depth
  (LessEqual), emitted only when an environment is extracted. The frame graph (`graph.rs:74`)
  does not change shape; env-less scenes keep the exact clear-color path (render.rs:1271).
- **One mesh shader serves both worlds** via the sunless-fallback precedent (render.rs:1299): a
  1×1 fallback env binding + a uniform flag. Flag off ⇒ the exact current ambient math
  (`lights.ambient.rgb * albedo`, mesh.wgsl:146); flag on ⇒ irradiance×albedo diffuse +
  prefiltered specular (split-sum). Byte-identity of env-less goldens is asserted; the standing
  driver-codegen caveat applies — if the pinned GPU shifts bytes on codegen grounds, one
  re-bless in that wave with the diff report committed (the Phase 10/11 rule, pre-decided).
- **Sprites keep flat ambient** (sprite.wgsl:51 untouched, fed by `Environment.ambient` when
  present via the existing `Lights` uniform — layout unchanged, so no sprite re-bless). Mixing
  stylized flats with IBL-lit meshes is the HD-2D look, per the standing roadmap decision.
- **Prefilter runs as compute at environment load/change**, keyed by a content hash of the
  environment's fields (the mesh-cache precedent) so `asset.reload`/live edits re-prefilter for
  free. Deterministic inputs; image gating stays perceptual-on-pinned-GPU as ever. Test mode
  needs no new carve-out (nothing temporal).
- **Instance layout:** `MeshInstanceRaw` (160 B, render.rs:54) gains one vec4
  `@location(14)` = `(fade, height_min, reserved, reserved)`, defaults `(1.0, -f32::MAX, 0, 0)`
  → 176 B; layout tests (render.rs:2428/2513/2869 family) extend. Defaults are render-identical
  by construction and asserted so.
- **Wire surface stays 26.** `Environment` and `Fade` ride the registry (append-only ×2,
  components.rs:872 tail — prior world hashes stay byte-identical) and `component.set`; the
  inspector picks both up via schemars doc-comments (registry.rs:278). No `environment.*`
  commands; dedicated tooling waits for editor demand.
- **L-codes:** `Environment` gets semantic validation (finite colors, ranges, non-negative
  intensities) under the next free L11xx at implementation time; `Fade` range-checks likewise.
  JSON Pointer paths as everywhere.

## Waves

### Wave 1 — components, render-inert
- `Environment` (sky gradient, disc, ambient, ibl_intensity, bloom threshold/strength) +
  `Fade` (fade, height_min): plain `#[serde(default)]` fields, schemars ranges, semantic
  validation, registry appended. Extract carries them into `RenderScene`
  (`environment: Option<…>`, per-instance fade) but the renderer ignores them.
- **Tests:** validation misshapes with JSON Pointer assertions; canonical churn = zero
  (no committed canonical fixture contains either component — grep to confirm at landing);
  inspector renders/commits the fields (vitest); extract determinism; full `check.mjs`;
  **all goldens byte-identical**.

### Wave 2 — sky + IBL (the new-bless wave)
- Prefilter compute (irradiance + specular mips + BRDF LUT, content-hash keyed); sky triangle in
  the scene pass; mesh.wgsl IBL behind the flag; instance lane added (fade inputs dead this
  wave); ambient-wart unification; bloom knobs feed threshold/tonemap (defaults preserve bytes).
- New gallery scene `night-sky.yaml`: gradient night sky + the material-park sweep spheres
  (metals now alive under IBL) + the emissive beacon — **one new golden blessed**; README row.
- **Tests:** existing gallery + game + import goldens **asserted byte-identical** (the
  decision-3 point); prefilter determinism (same env twice ⇒ identical GPU inputs);
  `ludion profile` smoke on night-sky.

### Wave 3 — fade end-to-end
- Dither discard in mesh.wgsl (Bayer, fade + height_min semantics); `occlusion_fade` example
  (a wall between camera and a marker, faded three ways — the copyable G7 pattern);
  optionally a small gallery golden if the example warrants one.
- **Tests:** dither determinism (same scene ⇒ identical pixels twice on the pinned GPU);
  defaults asserted invisible across all prior goldens; example smoke.

### Wave 4 — vigil night sky + docs + close
- Shared night-sky `Environment` into the three districts per decision 7 (plaza fragment +
  regenerate + map-drift green; market/cathedral direct edits); `run.lrp` re-recorded
  hermetically; `district-plaza` golden re-blessed with diff report committed; mac queue entry
  updated in the private macOS queue.
- Docs: engine-guide "Environment & sky" chapter (component fields, sky authoring, fade/dither,
  a copyable night-sky example), renderer crate doc pins the IBL math (split-sum, prefilter
  shapes) next to the BRDF, `docs/determinism.md` note on runtime-generated env textures.
- **Exit gates:** `check.mjs` full green; all non-adopting goldens never moved (waves 1–3 logs
  are the evidence); **human gate = the queued G3–G6 batch sitting** — the batch plays under
  the night sky; the G0 camera caveat gets its answer there. Phase closes on that sitting.

## Explicitly out of scope (triggers unchanged or new)

- **Texture maps** — the original textured-PBR trigger, now texture-only: first content that
  palette/per-vertex materials can't express. Nothing here pre-builds for it.
- **HDRI environments** — rejected this session; revisit only if a real look demand defeats the
  gradient (recorded, not expected).
- **Occlusion *policy*** (who fades, hysteresis, the dither-vs-top-fade-vs-cutaway verdict) —
  G7's eye-settled experiment, game-side, on this arc's plumbing.
- **Emissive set-dressing, juice, theme-dependent dressing, OGG/music trigger** — G7 proper.
- **Sprite/billboard/tilemap IBL** — flats stay Lambert two-sided (HD-2D mix, standing rule).
- **Tonemap/vignette/exposure controls** — bloom knobs only; a post stack waits for demand.

## Risks

- **The IBL look is judged after the fact, in the same sitting as four game gates.** Contained
  by: opt-in (a bad sky is a content edit, not a pipeline revert), the pre-pinned math, and the
  Phase-11 failure path — tune content first; re-pinning math is a deliberate second bless with
  diff evidence, never a silent tweak.
- **"Absent ⇒ byte-identical" is claimed, so it's asserted three waves running** — the flag-off
  shader path and default instance lane are exactly the kind of change driver codegen can shift;
  the pre-decided re-bless rule caps the damage at one evidenced bless.
- **The gate-batch sitting is now large** (G3–G6 verdicts + genre + placement + camera + the new
  look). Accepted by the sequencing decision; the plan's only mitigation is that everything the
  sitting needs ships in one build with one notebook checklist.
- **Prefilter on integrated GPUs / mac** — compute at load is small (a gradient, not an HDRI),
  but the mac session inherits one more re-bless (district-plaza) and the night-sky golden to
  bless fresh; queue recorded in Wave 4.

## Status (2026-07-22): mechanized-complete, executed as planned

Four orchestrated waves, Lore revs 288–292, 2026-07-21 → 2026-07-22. Every wave landed per the
plan; deviations were zero at the decision level. Outcomes of record:

- **Wave 1 (rev 288):** `Environment` + `Fade` registered append-only after `DirectionalLight`
  (registry now 15), L1128 on the color arrays only — the array-element invariant schemars
  `range` cannot reach; `Fade` needs no dedicated code (`fade`'s [0,1] is pure schema → L1014).
  Extract → `RenderScene.environment` / per-instance fade; renderer inert; goldens byte-identical.
- **Wave 2 (revs 289–290):** prefilter compute (`ibl.rs`/`ibl.wgsl`): irradiance 32²×6 stored as
  mean radiance (matches the point-light diffuse convention), specular 64²×6 across 5 mips, BRDF
  LUT 128² computed once (environment-independent); cache key = the three sky colors (36 bytes).
  Sky triangle in-pass (after opaque meshes, **before sprites** so alpha flats blend over sky);
  split-sum in mesh.wgsl behind the flag; instance lane → 176 B (dead until Wave 3); ambient wart
  unified as `pub const DEFAULT_AMBIENT`; bloom knobs as uniforms. `night-sky` golden blessed
  (reframed once in review so the disc — the feature's only visual coverage — is in frame).
  A shader-bindgen emitter fix (rustfmt-idempotent wrapping of >100-col BGL signatures) rode
  along; **the driver-codegen re-bless caveat never fired** — all prior goldens byte-identical.
- **Wave 3 (rev 291):** 4×4 Bayer discard (max threshold 15/16 ⇒ fade 1.0 can never discard —
  byte-identity by construction); shadows deliberately fade-independent (doc-comment in
  shadow.wgsl); `occlusion_fade` example + `occlusion-fade` gallery golden (the visual pin of the
  dither pattern).
- **Wave 4 (rev 292):** shared `env-night.yaml` (no disc — the districts carry no
  `DirectionalLight`; flagged for the human gate) into all three districts (plaza via fragment +
  regenerate, market/cathedral verbatim direct edits); `run.lrp` re-recorded hermetically;
  `district-plaza` re-blessed with the diff report committed
  (`artifacts/environment-ibl-wave4-district-plaza.diff.json`: SSIM 0.9976, damage confined to the sky
  corners); mac queue updated. Docs: engine-guide "Environment & sky"
  chapter, renderer-doc IBL math pin, determinism note; two stale pre-arc "no IBL" claims fixed.
- **Rode along (post-wave, same arc close):** breakout/vigil replay tests renamed their temp
  recordings crate-prefixed (`CARGO_TARGET_TMPDIR` is workspace-shared; the bare `run.lrp` names
  collided under concurrent multi-instance test runs — found by the Wave 4 agents racing).

**Open to close the arc:** the queued G3–G6 human-gate batch plays under this build's night sky
(also answers the G0 camera viewing-environment caveat and the disc question above); the mac
session owes `night-sky.png` + `occlusion-fade.png` fresh blesses and the `district-plaza.png`
re-bless (queued privately).
