# Plan: Phase 11 — PBR Mesh Shading

> **CLOSED — shipped; the Wave 4 bless event is recorded in `Plan/phase-11-wave4-bless.md`
> ("The BRDF swap … land here"), and a private roadmap records "Phase 11 PBR
> landed".**
> Status: **authored 2026-07-12, ready to start** (Phase 10 closed the same day). Direction
> settled in `Plan/roadmap-voxel-mesh-pbr.md` decisions 8–9 (grilling session 2026-07-11); this
> is the wave-level plan that roadmap deferred until voxel content existed to judge shading
> against. The thesis: upgrade the whole mesh path **once** — a single GGX/Cook-Torrance
> point-light + ambient BRDF in `mesh.wgsl` for ALL meshes (primitives, asset meshes, voxel
> models), palette/doc/component-driven material parameters, emissive riding the existing
> HDR/bloom pipeline with zero new passes. Texture maps, IBL, and shadow mapping stay out
> (textured-PBR trigger, unchanged) — that bound keeps the slice at ~a hundred lines of WGSL
> plus data plumbing. Exit gate shape (roadmap): material-park gallery scene (roughness ×
> metallic sweep spheres + emissive voxel model), blessed goldens on the pinned GPU, all prior
> mesh goldens re-blessed exactly once, `ludion import` factor mapping verified against the
> Phase 8 fixture, human gate: a look at the material park.

## Current-code facts that shape the work

- **The mesh shader is one small file and the only shading-model change site.**
  `crates/ludion-renderer/src/shaders/mesh.wgsl` (83 lines): one-sided Lambert + ambient, ranged
  attenuation squared, `final = vertex_color × instance_color × shade` (vs_main multiplies the
  colors at line 60). `Camera.cam_pos` is uploaded every frame (render.rs:358) and **never read**
  — the view vector GGX needs is already on the GPU. `Light.color.w` and `Lights.ambient.w` are
  spare lanes; ambient is applied as `ambient.rgb × color` (the shade loop seeds `acc` with
  ambient), a behavior worth preserving through the swap.
- **The lighting model is duplicated, not shared: `sprite.wgsl` has its own `shade_two_sided()`.**
  Sprites/billboards/text/tilemaps do not touch `mesh.wgsl`. A change confined to the mesh shader
  moves only mesh-bearing goldens; the `Camera`/`Lights` uniform *layouts* are copy-pasted into
  both files, so those structs must not change shape or the sprite path re-blesses too.
- **One vertex format for all three mesh kinds, tightly packed.** `MeshVertex`
  (`ludion-renderer/src/mesh.rs:15`, 40 bytes, position+normal+color, layout asserted by
  `mesh_vertex_layout_is_position_normal_color` at render.rs:1414) is shared by primitives, asset
  meshes, and voxel meshes; `MeshData` (`scene.rs:114`) carries always-populated flat
  positions/normals/colors/indices + content hash. Fill sites: primitive generator (mesh.rs:48,
  opaque white), `mesh_asset.rs:51` (white), `voxel_mesh.rs` `mesh()` (palette per-quad, colors
  emitted at ~line 226). Widening the format is a one-place change with four writers.
- **`@location` numbering is shared across the two vertex buffers**: 0–2 per-vertex, 3–7
  per-instance (`vertex_attr_array` at render.rs:1222–1223). `MeshInstanceRaw` (render.rs:64) is
  model mat4 + color vec4, 80 bytes, no spare fields.
- **The bloom threshold is a hardcoded `max(c − 1.0, 0)` in `threshold.wgsl:27`.** Scene target
  is `Rgba16Float`; tonemap is ACES with `scene + bloom × 0.6`. Any mesh fragment above 1.0
  blooms today — emissive needs zero new passes and zero new uniforms, exactly as roadmap
  decision 9 predicted.
- **The sphere primitive is `sphere(12, 8)`, faceted** (mesh.rs:89, sole call site mesh.rs:35).
  A faceted 12×8 sphere cannot display a roughness gradient — the material park needs smooth
  normals and more tessellation. Primitive buffers are built once at startup (render.rs:204–208),
  not hash-cached; asset/voxel meshes are cached by `MeshData.hash` = source-doc content hash, so
  new doc fields re-key the GPU cache and `asset.reload` keeps working for free.
- **Components serialize *all* fields** (`#[serde(default)]` everywhere, no
  `skip_serializing_if` at the component layer — that convention is protocol-only). Adding fields
  to `Mesh` (components.rs:134: `shape` + `color` only) changes the canonical form of every
  serialized `Mesh` component. Blast radius checked: **no game scene contains a `Mesh`** (all
  Sprite/Tilemap/Text) — game goldens, replays, and `run.lrp` WorldHashes are untouched. The one
  committed canonical fixture with meshes is `examples/import/expected/scene.yaml` (the exit-gate
  test asserts `scene.load → scene.save` byte-identity against it). Component schemas are not in
  the protocol snapshot (Phase 10 fact) — schema_hash and the wire surface (26) don't move.
- **Import reads exactly one material field today.** `mesh_reference` (ludion-import/lib.rs:344)
  maps `baseColorFactor → Mesh.color` and flags `baseColorTexture` as L1202;
  `metallicFactor`/`roughnessFactor`/`emissiveFactor` and every other material field
  (metallicRoughness/normal/occlusion/emissive textures, alphaMode, doubleSided) are **silently
  ignored** — never read, no diagnostic. That's a standing violation of "unsupported features are
  structured errors, never silent drops" that this phase inherits the moment it starts reading
  materials properly. The fixture (`examples/import/scene.gltf`) has two materials with
  `baseColorFactor` only; the `gemstone` texture is what forces `--allow-lossy`.
- **Golden inventory:** 13 blessed goldens — 9 gallery + 3 game + 1 import. Mesh-bearing (will
  move): gallery `billboards`, `lights`, `meshes`, `mesh-asset`, `voxel-model` + import `scene`
  (6). Non-mesh (must not move): gallery `sprites`, `text`, `tilemap`, `breakout-mock` + 3 game
  goldens (7). Gallery discovery is filesystem-driven, zero-config (`testkit/src/suite.rs`), same-
  machine comparisons assert exactly SSIM 1.0. **The import golden has no self-service bless
  path** — `cli/tests/import.rs` is compare-only; it re-blesses via
  `ludion render examples/import/expected/scene.yaml -o examples/import/goldens/scene.png`.
- **No new L-codes needed.** New optional fields fall inside the existing structure/consistency
  pairs: mesh docs L1110/L1111, voxel docs L1113/L1114 (unknown-key rejection with nearest-match
  suggestions already centralized: `voxel.rs:441` palette-entry match, `mesh.rs:264` top-level).
  L1116+ stays free. The L1202 *explanation text* (codes.rs:1221–1248) hardcodes the mapping
  table ("material `baseColorFactor` → `Mesh.color`", "the mesh pipeline is flat-color this
  milestone") and must be updated — codes.rs edits don't move schema_hash (wire types only).

## Design decisions (settled by this plan)

1. **Material = three scalars, named the same on every surface.** `roughness` (perceptual,
   [0, 1]), `metallic` ([0, 1]), `emissive` (≥ 0, an HDR intensity multiplier on the surface's
   own color — roadmap decision 9's exact semantics, applied uniformly). Default material:
   `roughness 1.0, metallic 0.0, emissive 0.0` — matte non-metal, deliberately the closest GGX
   gets to today's Lambert so default content moves least. Three author surfaces:
   `Mesh` component fields (primitives, imported assets), voxel palette entry fields (per palette
   entry → per quad), and `.mesh.yaml` per-vertex lists (hand-authored meshes). Emissive is a
   scalar everywhere, never an RGB: it multiplies the surface color the author already picked
   (palette color / `Mesh.color` × vertex color) — one concept, no second color to drift.
2. **Precedence is selection, not multiplication.** A mesh that authors any of its own material
   (any palette entry field, any mesh-doc material list) uses it entirely; the `Mesh` component's
   material fields apply only to meshes that author none (primitives, plain asset meshes) and are
   inert otherwise (documented). Rationale: the color rule (vertex × instance, white-neutral)
   does not transfer — metallic's sane default (0) is multiplication's absorbing element, so any
   multiplicative composition makes either the component-driven sweep spheres or palette-driven
   metals inexpressible. Selection is resolved per-mesh at extract into a per-instance flag; the
   shader blends branch-free (`mix(instance_mat, vertex_mat, flag)`). Color keeps its existing
   multiply rule untouched — color is a tint, material is a value. Rejected: `Option<f32>`
   override fields on the component (nullable schema surface, same semantics, more ceremony).
3. **The BRDF, pinned.** Cook-Torrance: D = GGX/Trowbridge-Reitz, V = Smith height-correlated,
   F = Schlick with `F0 = mix(vec3(0.04), albedo, metallic)`; diffuse lobe
   `(1 − metallic) × albedo`; α = roughness², perceptual roughness clamped to ≥ 0.045 in-shader
   (GGX singularity); normals renormalized per-fragment; view vector from the already-uploaded
   `cam_pos`. **Light units keep today's convention**: a light's premultiplied color is the
   Lambert response at ndl = 1 (no 1/π on the diffuse lobe), and the specular lobe is scaled by π
   (`π·D·V·F`) so the two lobes stay physically proportioned — existing scenes keep their
   brightness, gaining highlights instead of darkening 3×. Ambient stays `ambient.rgb × albedo`
   (today's exact behavior; metals under ambient-only light go dark — accepted until IBL).
   Emissive adds `albedo × emissive` to the fragment (HDR > 1.0 blooms through the untouched
   threshold pass). Attenuation (`atten²` ranged), the 16-light uniform, and both uniform struct
   layouts are untouched — the sprite path must come back byte-identical, asserted.
4. **Data plumbing: widen the one vertex format once, land it invisibly.** `MeshData` gains
   `materials: Vec<f32>` (3 per vertex: roughness, metallic, emissive — always populated,
   default-material fill) + `authored_material: bool`; `MeshVertex` gains `material: [f32; 3]`
   (→ 52 bytes, layout test updated); vertex attr `@location(3)` Float32x3, instance attrs shift
   to 4–8, new instance attr `@location(9)` vec4 = (roughness, metallic, emissive,
   use_vertex_material ∈ {0,1}); `MeshInstanceRaw` + material vec4 → 96 bytes. Fill sites:
   primitive generator and `mesh_asset::decode` write the default material (and doc lists when
   present), `voxel_mesh` writes palette values per quad exactly parallel to its color emission;
   extract sets the instance material from the component fields and the flag from
   `authored_material`. This wave lands **under Lambert** (new attributes unused) with the full
   gallery + game + import suites green at exactly SSIM 1.0 — the Phase 10 Wave 2 pattern,
   including its pre-decided rule: if the pinned GPU shifts bytes on driver-codegen grounds, one
   re-bless in that wave with the diff report committed as evidence of perceptual identity.
5. **Text formats: additive optional fields, existing codes, byte-idempotence preserved.**
   Voxel palette entries gain optional `emissive`/`metallic`/`roughness` keys (finite; ranges per
   decision 1 — violations L1114, structure L1113; the palette-entry suggestion list grows).
   Canonical emit order within an entry: `color`, then the optional fields alphabetically,
   emitted only when authored (absent stays absent — old files byte-stable, roadmap decision 3
   honored: fields arrive the same phase the shader consumes them). `.mesh.yaml` gains optional
   `emissive:`/`metallic:`/`roughness:` flat lists, 1 float per vertex, counts validated against
   positions (L1111/L1110), emitted between `uvs` and `indices` in that alphabetical order.
   A doc/palette that authors some channels gets the default-material value for the rest.
   Proptests and canonical-form goldens extend in place (`mesh_roundtrip.rs`,
   `voxel_roundtrip.rs`).
6. **`Mesh` gains three plain fields; the canonical churn is scheduled, not discovered.**
   `roughness: f32` (default 1.0), `metallic: f32` (0.0), `emissive: f32` (0.0) — plain
   `#[serde(default)]` fields with schemars range annotations, matching every existing component
   (components serialize fully; skip-default is a protocol convention, deliberately not imported
   here). Consequences, all verified in advance: `examples/import/expected/scene.yaml` re-emits
   once (three lines per `Mesh`; the exit-gate byte-identity test forces it the moment the fields
   exist); hand-authored scenes stay valid unchanged; no game scene contains a `Mesh`, so no
   replay or WorldHash fixture moves; schema_hash and wire surface don't move; the registry's
   scene-validation schema and the editor inspector (plain numeric fields, generic widgets) pick
   the fields up automatically — verified with a vitest/smoke check, no variant-widget work.
   Material edits ride the existing `component.set` — parity holds by construction, no new wire
   commands.
7. **Import maps the factors and stops silently dropping the rest of the material.**
   `metallicFactor → Mesh.metallic`, `roughnessFactor → Mesh.roughness` (same f32 round-trip rule
   as color). `emissiveFactor`: `[0,0,0] → 0.0`; a non-zero factor maps to scalar `k` only when
   it equals `k × baseColorFactor.rgb` exactly on the f32 values (our emissive is
   intensity-times-own-color; an independent emission tint is inexpressible) — otherwise L1202
   naming the material, help text "set emissiveFactor to a scalar multiple of baseColorFactor, or
   pass `--allow-lossy`" (lossy mode skips the emissive, warns, imports the rest). Now that the
   importer reads the material surface, the silently-ignored features become structured L1202s:
   metallicRoughness/normal/occlusion/emissive texture references, non-OPAQUE `alphaMode`,
   `doubleSided` (the mesh pipeline back-face culls). The fixture gains
   metallic/roughness factors on both materials and a proportional emissive on one; the expected
   tree updates; the crate-doc mapping table (lib.rs:20–34), CLI docs, and the L1202 explanation
   in codes.rs all revise together. Under Lambert (this lands before the swap) the new fields are
   render-inert — the import golden is asserted byte-identical here and re-blesses exactly once,
   at the swap.
   *Amended 2026-10-30 (owner approval 2026-10-29, Clockout 0010): glTF `MASK` (with `alphaCutoff`) and `doubleSided` leave the L1202 surface and map onto `alpha_mode`, `alpha_cutoff` and `double_sided`; `BLEND` stays L1202, and the lossy fixture now rides its `BLEND` material.*
   *Amended 2026-10-30 (owner approval 2026-10-29, Clockout 0014): a non-proportional emissiveFactor leaves the L1202 list — the importer bakes it into an emissive map (Mesh.emissive = the largest factor/base ratio, the map carries the hue; decision 1 unchanged); L1202 keeps a tint over a base channel below 1/255 and a tint on a primitive without TEXCOORD_0.*
8. **The sphere primitive upgrades in the swap wave: `sphere(24, 16)`, smooth analytic normals**
   (normal = normalized local position — exact, deterministic, CPU-side). A faceted 12×8 sphere
   cannot show a roughness gradient, and the park is the phase's gate; riding the swap wave means
   mesh goldens still re-bless exactly once. Cube and plane stay faceted (flat faces — faceted IS
   correct there). Rejected: a second smooth-sphere primitive (two ways); sweep spheres as asset
   docs (hundreds of unreadable float rows to dodge a 2-line primitive change).
9. **One bless event, blast radius pre-declared.** The swap wave is the only golden-moving wave.
   Expected: the 6 mesh-bearing goldens re-bless once + the new `material-park` golden blesses,
   every diff report committed as evidence; the 7 non-mesh goldens are asserted byte-identical —
   a diff there means the shared uniform layouts or the sprite path moved, which is a bug, not a
   re-bless. The import golden's out-of-band bless command is written into the wave. Waves 2 and
   3 assert *all* goldens unchanged.
10. **The material park is the exit-gate content.** `examples/gallery/material-park.yaml`: a
    6×6 roughness × metallic sweep of spheres driven purely by component fields (roughness 0→1
    across, metallic 0→1 down), a new emissive voxel model
    (`examples/gallery/voxels/beacon.voxel.yaml` — emissive glass + metallic trim palette, so
    palette material fields are exercised; the existing lantern/tree/house stay untouched), a
    ground plane, and point lights placed to make highlights and the bloom legible. Golden is
    auto-discovered; README row added. Docs: the engine guide gains a "Materials" chapter
    (fields on all three surfaces, precedence rule, emissive/bloom, a copyable sweep example) and
    the voxel chapter's palette section extends; `docs/ludion-renderer.md` pins the BRDF;
    `docs/ludion-scene.md` and the import/CLI docs update. Rejected: a standalone PBR topic (the
    guide is the single source of truth for authors; crate docs bind crates — Phase 10 decision 8
    shape).

## Waves

### Wave 1 — material fields in the text formats + component
- Voxel palette optional fields, mesh-doc optional per-vertex lists per decision 5; `Mesh`
  component fields + schema ranges per decision 6; `examples/import/expected/scene.yaml`
  re-canonicalized (grep for any other canonical fixed-point fixture containing a `Mesh`).
- **Tests:** roundtrip proptests + canonical-form goldens extended (both formats); every new
  misshape (out-of-range, non-finite, wrong list length, unknown key suggestion) with JSON
  Pointer assertions; old files parse unchanged and emit byte-identically; import exit-gate
  byte-identity green against the re-emitted fixture; import golden byte-identical (fields are
  render-inert); inspector renders/commits the three numeric fields (vitest); full `check.mjs`.

### Wave 2 — GPU plumbing under Lambert
- `MeshData.materials` + `authored_material`, `MeshVertex`/`MeshInstanceRaw`/attr layouts, four
  fill sites, extract instance material + flag, per decision 4. Shader untouched except consuming
  the new locations into dead inputs (or not at all — pick whichever keeps goldens still).
- **Tests:** layout/stride/offset assertions updated (render.rs:1414 family); upload/interleave
  unit tests; voxel mesher emits palette material per quad (extend the material-uniform
  proptests); extract determinism (same scene ⇒ identical `RenderScene` twice); **full gallery +
  game + import golden suites at exactly SSIM 1.0** — the wave's point is proving the layout
  change invisible; the decision-4 driver-grounds re-bless rule applies only on failure.

### Wave 3 — import factor mapping
- Factor mapping + proportional-emissive rule + the material-surface L1202 audit per decision 7;
  fixture gains factors; expected tree updates; docs/mapping-table/L1202-text revisions.
- **Tests:** ludion-import unit tests per factor (incl. the exact-proportionality accept/reject
  pair and lossy-mode skip+warn); the existing default-mode-L1202 and shared-mesh tests updated;
  exit-gate green (tree byte-identity, check-scene, golden **byte-identical** — still Lambert,
  load→save fixed point); error-audit bijection/reachability untouched (no new codes).

### Wave 4 — the swap (the one bless event)
- `mesh.wgsl` BRDF per decision 3; sphere upgrade per decision 8; `material-park.yaml` +
  `beacon.voxel.yaml` + README row per decision 10.
- **Bless:** 6 mesh-bearing goldens re-blessed + material-park blessed, diff reports committed;
  import golden re-blessed via `ludion render examples/import/expected/scene.yaml -o
  examples/import/goldens/scene.png`; the 7 non-mesh goldens asserted byte-identical (decision
  9). Eyeball the lights/meshes diff reports for brightness parity (the π convention's check).
- **Tests:** full suites green post-bless; `ludion profile` smoke on the park scene (36 spheres +
  a voxel model is small; budgets stay green); replay/WorldHash suite green (shading is not sim).

### Wave 5 — docs + exit gates
- Engine-guide "Materials" chapter, voxel-chapter palette extension, crate/CLI doc updates per
  decision 10 (frontmatter gate covers them by construction).
- **Exit gates (roadmap):** material-park golden green on the pinned GPU; all prior goldens
  re-blessed exactly once (waves 2–4 logs are the evidence); import factor mapping verified
  against the fixture (wave 3); **human gate: a look at the material park** (windowed or the
  golden PNG). Phase closes on the human pass. No THE-test this phase — the roadmap's gate for
  shading is the human look; agent-authorability was Phase 10's gate.

## Explicitly out of scope (triggers recorded)

- **Texture maps, IBL, shadow mapping** — the textured-PBR trigger (roadmap): first content that
  palette/per-vertex materials can't express. Nothing here pre-builds for it beyond glTF factors.
  When shadow mapping lands, the frame graph must first grow depth as a first-class resource:
  today the depth target is a private per-frame texture in `ludion-renderer/src/render.rs` (~:948)
  and the graph's `TextureFormat` has only color variants, so the declarative graph cannot express
  a depth attachment or order passes around one (engine audit 2026-07-17 §2). Make that the
  phase's first wave.
- **Sprite/billboard/tilemap shading** — stays Lambert two-sided; roadmap decision 8 scopes the
  upgrade to the mesh path. Mixing stylized flats with PBR meshes is the HD-2D look, not a bug.
- **Configurable ambient / environment component** — ambient stays the hardcoded extract const.
  (Known wart: `extract.rs:32` and `RenderScene::empty` disagree, 0.05/0.05/0.06 vs
  0.03/0.03/0.04 — harmless, invisible in practice; unify only if touched anyway.)
- **Bloom threshold/strength controls** — hardcoded 1.0 / 0.6 stay; emissive works within them.
- **`material.*` wire commands / editor material UI** — component fields ride the existing
  inspector + `component.set`; dedicated tooling waits for editor demand, parity gate clean.
- **Chunked voxel worlds, voxel-painting UI** — unchanged Phase 10 triggers.

## Risks

- **The BRDF swap is the one pixel-moving change and taste is judged after the fact.** Contained
  by pinning the math (decision 3) and the brightness convention (π-scaling) in advance, landing
  it in one wave with committed diff reports, and pre-deciding the failure path: if the human
  gate says the park looks wrong, tune *content* (lights, sweep values) first; re-pinning the
  BRDF is a deliberate second bless with its own diff evidence, never a silent tweak.
- **Layout-change invisibility (wave 2) is claimed, so it's asserted** — same driver-codegen
  caveat and same pre-decided re-bless rule as Phase 10 wave 2.
- **Canonical churn from component fields** is scheduled into wave 1 where the exit-gate test
  forces it; risk is a *missed* canonical fixture — mitigated by grepping for committed scenes
  containing `Mesh` before landing (known set: import expected tree; gallery scenes are
  hand-authored inputs, not save outputs, and stay untouched).
- **Metals may read dark** (no IBL: specular from ≤16 point lights, ambient tints by albedo).
  Accepted physics-of-scope; the park's lighting is authored to compensate, and the guide chapter
  says so, so agents don't fight it.
- **The exact-proportionality emissive rule will reject some real glTF assets.** That's the
  designed behavior (structured L1202 with an actionable fix beats silently changing the render);
  `--allow-lossy` imports the rest. If real content hits this often, an RGB emissive is a
  recorded possible extension — decided then, not now. Resolved import-side by Clockout 0014 (decision 7 amendment).
- **Sequencing vs open user gates:** Phases 8/9's open items (sound listen, publish decisions,
  CI standup) share no files with this phase except docs; no L-code collision (this phase adds
  none). Safe to run in parallel with those gates staying open.
