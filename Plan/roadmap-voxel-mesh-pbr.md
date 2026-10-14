# Roadmap: Voxel Models, Mesh Path, and the PBR Arc

> Status: **settled direction, multi-phase** (grilling session 2026-07-11). This document records
> the long-term arc — voxel authoring → PBR shading → textured materials → (trigger-gated) chunked
> voxel worlds — and the design decisions already settled for it. It is NOT a wave-level phase
> plan: detailed plans for the voxel and PBR phases are authored when Phase 8 closes (the
> "current-code facts" they need don't exist until then). Phase 8 (`phase-8-assets-and-sound.md`)
> is the foundation this arc builds on and lands as written.

## Why (the one-paragraph case)

A voxel grid is the most LLM-friendly 3D authoring representation available: discrete, palette-
indexed, spatially readable as text, diffable, with no continuous coordinates to hallucinate.
Ludion has no way for an agent to author real 3D geometry (three procedural primitives; glTF
import is a Phase 8 compiler for *existing* assets). Voxel models fill that gap as a *producer*
of meshes on top of Phase 8's arbitrary-mesh path — not a second renderer. Separately, this
session reframed the renderer's ceiling: **HD-2D is a stylistic choice, not an engine
restriction**. The renderer's long-term destination is a full PBR pipeline, textures included,
grown feature-by-feature under real content demand — no restrictions, just priority.

## Phase sequence

Phase 7 (done) → Phase 8 (done) → **Phase 9 Hardening** (in flight — it overtook this roadmap's
original slot, so the numbering settled the other way) → **Phase 10 Voxel Models**
(`Plan/phase-10-voxel-models.md`, authored 2026-07-12) → **Phase 11 PBR** (plan authored at
Phase 10's close). Beyond the sequence, two recorded triggers:

- **Textured PBR** (UVs sampled, PBR texture maps, glTF `baseColorTexture` et al. stop being
  L1202): **design SETTLED and SHELVED ready 2026-07-23 — `Plan/textured-pbr.md` is the plan of
  record**, fires on demand. Trigger **sharpened** from "first content that palette-driven
  per-vertex materials can't express" to **first external textured asset worth importing with its
  texture maps intact** — the driver is *import fidelity*, not an authoring need (voxel + palette +
  scalar PBR + IBL remains the agent-authoring story). Settled shape: import-only, the full five-map
  glTF metallic-roughness set in one arc, referenced/normalized/deduped PNGs, tangents
  imported-else-computed and committed to `.mesh.yaml`, anisotropy 16, mesh-path only. Voxel
  texturing is a *separate* recorded deferred possibility (surface detail on voxel geometry — a
  different trigger), not this. Shadow mapping left this bundle and **shipped 2026-07-17 by user
  direction**, ahead of the trigger and ahead of the vigil content that was expected to motivate it
  (`Plan/shadow-mapping.md`) — a sun-only directional light + one orthographic PCF shadow map on the
  mesh path. **IBL left the bundle 2026-07-21 by the same precedent** (grill-me session;
  `Plan/environment-ibl.md` — an opt-in `Environment` component, procedural-gradient sky prefiltered
  at load, no image assets). Texture maps alone stay behind the (now sharpened) trigger.
- **Chunked voxel worlds** (runtime grids, chunk streaming, neighbor-aware remeshing, world-scale
  editing): trigger = a game concept that needs terrain/destructibility, not before. The
  authoring-phase decisions below deliberately keep this door open; nothing else is pre-built.
- **Voxel assemblies** (`kind: assembly` doc: a prop composed of part models placed at *integer
  voxel offsets*, meshed as one composite through decision 5's bounds-checked-accessor seam with
  palettes resolved to materials, referenced as `shape: {assembly: …}`): trigger = the first
  game/content that wants a reusable multi-part prop (a building, a vehicle). Until then the
  composition path is convention, not format: scene hierarchy (parent + child entities, one part
  model each) with integer-voxel offsets — works today, dedups identical parts, and lets real
  friction (lattice cracks, reuse-across-scenes, junction faces) accumulate as the evidence the
  format is designed from. Settled 2026-07-15.

One near-term item rode ahead of any trigger (settled 2026-07-15, **shipped 2026-07-16**): the
**model view harness** — see the sketch below. Authoring feedback is the loop everything else
turns on; it shipped as a small standalone slice, not a phase.

## Settled decisions

1. **Voxel models are a content source for the mesh path, not a render mode.** No dedicated
   voxel pipeline (raymarched SVO rejected: a second render path sharing nothing with meshes,
   hostile to goldens). No GPU meshing (rejected for now: breaks the CPU bit-exact tier; revisit
   only if chunked worlds make CPU meshing a measured bottleneck).
2. **One `.voxel.yaml` per model — canonical strict-YAML, ASCII-slice encoding.** `size: [x,y,z]`;
   `palette:` maps single printable chars to entries; `slices:` is one block scalar per horizontal
   layer, bottom-up (y=0 first), rows = z, columns = x, `.` = empty, fixed row width (the
   canonical writer never trims). An LLM *sees* the shape — it can draw, mirror, and spot-fix
   spatially. Palette capped by the printable-char set (~90 entries/model) — a feature, not a
   limit, for palette discipline. Validated by `check-scene` dispatching on a `kind: voxel` key,
   exactly as Phase 8's mesh docs dispatch on `kind: mesh`. Rejected: sparse voxel lists and RLE
   rows (canonical and compact, but spatially unreadable — the exact failure mode voxels exist to
   avoid; RLE may return later as an *optional* encoding for world chunks).
3. **Voxel-phase palette entries are `{color}` only; material fields are additive later.**
   Unknown keys are strict errors. The PBR phase adds optional `emissive`/`roughness`/`metallic`
   the moment the shader consumes them — no dead schema fields ever, old files stay valid.
4. **Load-time meshing, one source of truth.** `Mesh.shape` gains a `voxel:` variant beside
   Phase 8's `asset:`; extract resolves the file, runs the mesher, and the resulting `MeshData`
   joins the same content-hash-keyed GPU cache. No committed derived mesh files (rejected: two
   files describing one model will drift; derived diffs are noise). Meshing a model-scale grid is
   milliseconds; runtime meshing is also what chunked worlds need, so this is the door-open choice.
5. **Greedy meshing, per-material, deterministic.** Culled interior faces merged into maximal
   rectangles only across voxels sharing a palette entry (merged quads stay material-uniform).
   Fixed sweep order — bit-exact CPU output, conformance-testable. Faceted normals, matching the
   existing primitive style. Chunk door: the mesher reads the grid through a bounds-checked
   lookup treating out-of-bounds as empty, so neighbor-chunk context can slot in later without
   rewriting the sweep.
6. **Grid-to-model-space: bottom-center origin, 1 voxel = 1 unit.** Origin at the center of the
   footprint, y=0 at the bottom (props sit on the ground; rotation pivots the visual center).
   World sizing via `Transform.scale` — no per-file `voxel_size` (one number fewer to drift).
7. **The file is the editor (authoring phase).** The agent loop is edit `.voxel.yaml` →
   `ludion render`/`ludion test` → look at the PNG; live viewers ride Phase 7's asset reload. No
   `voxel.*` wire commands until the Tauri editor grows a voxel-painting UI — the parity gate
   stays clean because the editor ships nothing before then. When that UI lands, commands arrive
   with inverse responses like every Phase 6 mutation.
8. **The PBR phase upgrades the whole mesh path once — one shading model.** A single GGX/
   Cook-Torrance point-light + ambient BRDF in `mesh.wgsl` for ALL meshes: primitives get a
   default material, voxel meshes feed palette-driven per-vertex/per-quad params, `.mesh.yaml`
   gains optional per-vertex material attributes, glTF import maps material *factors*
   (`baseColorFactor`, `metallicFactor`, `roughnessFactor`, `emissiveFactor`) instead of L1202.
   Texture maps, IBL, and shadow mapping stay out until the textured-PBR trigger — that bound is
   what keeps the first PBR slice at ~a-hundred lines of WGSL plus upload plumbing. Every golden
   re-blesses exactly once, in this phase. Rejected: a voxel-only PBR pipeline (two shading models
   = two ways); schema-now-shader-later (dead fields, silent appearance change on landing).
9. **Emissive rides the existing HDR/bloom pipeline** when it arrives (PBR phase): palette
   `emissive` is an HDR intensity multiplier feeding the bloom threshold — lava, lamps, runes
   glow with zero new passes.
10. **HD-2D reframed: style default, not engine ceiling** — pillars/CLAUDE.md amended (this
    session) to say the renderer grows toward full PBR by priority, and Phase 8's "PBR anti-goal"
    is scoped to Phase 8 itself with a pointer here.
11. **Content density convention: 16 voxels per world unit** (settled 2026-07-15 from a three-way
    side-by-side — the same lantern authored blind by three agents at 8/16/32 voxels tall, scaled
    to equal world height; the user picked the 32-tall register, ~13/unit, rounded up to 16 for
    power-of-two scales). Standard props author at 16/unit (`Transform.scale: 0.0625`); large
    structures halve to 8/unit; the 16³–48³ single-file sweet spot is the hero ceiling — above it,
    assemble from parts or generate the YAML with a throwaway script (observed in the experiment:
    at 32-tall the authoring agent switched to a generator script unprompted; 8- and 16-tall were
    hand-written and validated first try). Interiors stay solid: `.` and a glyph cost the same
    byte, and the mesher emits faces only at filled/empty boundaries, so hollowing saves nothing
    and adds hidden inward-facing quads. Format compression stays rejected (decision 2); the
    generator script IS the sanctioned compression — structure lives in code, the canonical text
    stays spatially readable. Trigger: if agents routinely burn context reading/editing >48³
    models, revisit *authoring-side ergonomics* (a generator-script convention, slice-view
    tooling), never the canonical encoding.

    **Addendum (2026-08-01): the trigger fired, and the answer shipped.** Agents authoring the
    `-hd` models did burn context on hand-edits above ~32³, so the *authoring-side ergonomics*
    escape hatch this decision reserved was taken — never the encoding.
    `Plan/voxel-authoring-tools.md` is that work (waves 1–4, mechanized-complete 2026-08-01): the
    `ludion voxel <new|apply|upscale|diff|from-json>` verb family, the `generated_from:` +
    `voxel-drift` gate pattern for script-owned models, and the committed
    `scripts/voxel/grid.mjs` generation library — which turns this decision's "generate the YAML
    with a throwaway script" into a *sanctioned, committed, byte-gated* script, with the
    canonical writer still in exactly one place (scripts print JSON; the engine writes the YAML).
    The 16-voxels-per-unit density convention and the assemble-from-parts rule for architecture
    are unchanged. What that plan puts back on the table for a human ruling is only the **stated
    single-file ceiling for single-piece models** (`L1115`'s threshold and the "16³–48³ sweet
    spot" wording) — a monster does not assemble from parts the way a building does. The concrete
    proposal is drafted verbatim in `Plan/voxel-authoring-tools.md` ("Proposal for the sitting —
    the L1115 ceiling"); until it is ruled on, the budget stands as written and an oversize model
    declares `budget_exempt: true`.

## Phase sketches (to be expanded into real plans at Phase 8 close)

- **Model view harness (settled 2026-07-15, SHIPPED 2026-07-16)** — landed as sketched:
  `ludion render --model` in `ludion-engine::model_sheet` (new L1117 for non-model input), plus
  golden-per-model via `GoldenTest`'s `kind:` dispatch (`ludion test` discovers/blesses top-level
  model docs; gallery lantern sheet golden committed at SSIM 1.0). Two elaborations beyond the
  sketch, settled at implementation: grid extends to 3×2 at five/six views (default = all six), and
  the key/fill light rig is **per-view camera-relative** (a world-fixed rig would leave the back
  view unlit); the model normalizes to a unit bounding sphere so one hand-tuned rig serves every
  model. Original sketch, for the record: —
  `ludion render --model <doc.voxel.yaml|doc.mesh.yaml> [--views front,back,left,right,top,iso]
  [--out …]`: synthesizes the standard turntable scene internally (per-view camera, ground
  plane, neutral key + fill) around a single model doc and writes **one contact-sheet PNG** — a
  fixed grid (2×2 at four views, 1×N under four), deterministic stitch, documented view order, no
  burned-in labels — so the agent author loop collapses to `check-scene` → `render --model` →
  look at one image. One sheet ≈ one image's vision-token cost regardless of view count (separate
  PNGs cost per image), all angles adjacent for cross-view judgment, one golden per model (the
  tiled-SSIM diff already localizes a regression to a tile = a view); need full-res detail →
  request fewer views (`--views iso` is a one-view sheet), not a second output mode. Zero new
  render features: pure CLI convenience over the existing scene renderer; deterministic
  camera/light placement derived from the model's bounds so sheets are golden-testable. Rationale: vision-in-the-loop is already how voxel authoring verifies (Phase 10
  THE-test, the 2026-07-15 density experiment); this removes its only friction.

- **Voxel Models phase** — exit gate shape: `kind: voxel` doc module (parse/validate/canonical
  emit, proptest byte-idempotence, new L-codes in the free L111x/L12xx space); greedy mesher with
  determinism conformance tests (same grid → identical vertex stream, twice); `shape: voxel`
  extract path with hash-dedup + JSON-Pointer errors; per-vertex color through the existing lit
  pipeline (the one lambert-era shader change: vertex color replaces the flat instance color for
  voxel meshes); gallery scene with hand-authored voxel models, blessed golden; **the proof**:
  a fresh agent, from a one-line prompt ("author a voxel lantern and render it"), produces a
  model + green golden with zero human intervention — the THE-test pattern applied to content.
- **PBR phase** — decision 8 above; exit gate shape: material-park gallery scene (roughness ×
  metallic sweep spheres + emissive voxel model), blessed goldens on the pinned GPU, all prior
  goldens re-blessed, `ludion import` factor mapping verified against the Phase 8 fixture,
  human gate: a look at the material park.

## Risks

- **Golden churn**: the PBR phase re-blesses every golden. Contained by doing all shading change
  in one phase (never per-feature dribble), after voxel content exists to judge it against.
- **Editor inspector meets `shape: {voxel: …}`** — same data-carrying-enum risk Phase 8 wave 2
  already schedules for `asset:`; the voxel variant rides whatever fallback lands there.
- **Slice-format ergonomics at depth**: a 64³ model is 64 slices of 64×64 chars (~270k chars).
  Fine for props (16³–48³ sweet spot); a validation *warning* above a generous size budget
  (analog of L1112) keeps authoring honest without capping the format. Worlds use chunks, not
  bigger files.
- **Scope gravity toward Minecraft**: "both, authoring first" is settled — any world-scale work
  (chunking, streaming, runtime edit commands) waits for its trigger and its own plan. The only
  world concessions made now are decisions 4 and 5's seams, which cost nothing today.
