# Plan: Phase 10 — Voxel Models

> Status: **implemented 2026-07-12** (Waves 1–6 all landed; started on user instruction while
> Phase 9's user-gated items stayed open — `codes.rs` was the only overlap, per the risk note.
> THE-test proof passed: `phase-10-voxel-proof.md`. The phase closes on the human gallery look.)
> Direction settled in
> `Plan/roadmap-voxel-mesh-pbr.md` (grilling session 2026-07-11); this is the wave-level plan that
> roadmap deferred until the Phase 8 mesh path existed. Sequence note: the roadmap's original
> order (voxel → PBR → hardening) was overtaken — hardening runs now as Phase 9 — so voxel models
> are Phase 10 and the PBR phase (planned separately, at this phase's close) is Phase 11.
> The thesis: a voxel grid is the most LLM-friendly 3D authoring representation there is —
> discrete, palette-indexed, spatially readable as text, diffable. This phase gives the agent a
> way to *author* real 3D geometry, as a producer of meshes on top of the Phase 8 mesh path —
> never a second render mode. Exit gate shape (roadmap): a fresh agent, from a one-line prompt,
> authors a voxel model and lands a green golden with zero human intervention.

## Current-code facts that shape the work

- **The mesh-doc module is the template.** `crates/ludion-scene/src/mesh.rs` owns
  `kind: mesh` docs end to end: `peek_kind` (mesh.rs:83) sniffs the top-level `kind:` key,
  `check-scene` dispatches on it (`crates/ludion-cli/src/check_scene.rs:38`), parse/validate/emit
  are `parse_mesh`/`validate_mesh`/`check_mesh`/`emit_mesh` + `mesh_budget_warning`
  (lib.rs:303–369), canonical emission uses the shared `scalar::format_float` (ryu) and
  `scalar_string` quoting, and byte-idempotence is proptested
  (`tests/mesh_roundtrip.rs`). A voxel module is a sibling, not a fork.
- **The strict-YAML subset rejects block scalars (L1006, `yaml.rs`).** The roadmap sketched
  slices as "one block scalar per layer" — that encoding is illegal in our own dialect. The
  in-repo precedent for 2D character grids is `Tilemap.rows`: a sequence of equal-width plain/
  quoted row strings (L1019 ragged / L1020 bad glyph). Slices must use that shape.
- **Diagnostics carry no severity.** L1112 (mesh vertex budget) is returned *out-of-band* as
  `Option<Diagnostic>` and printed `warning[L####]` by `check_scene.rs:101`, exit code untouched.
  The voxel size budget reuses this exact mechanism.
- **Free L-code space:** L1110–L1112 (mesh) and L1201–L1203 (import) are taken; **L1113–L1119 are
  free** and adjacent to the mesh block. Phase 9's in-flight codes are L3017/L4004/L4005 — no
  collision. (`crates/ludion-core/src/codes.rs`; `DECLARED_CODES` registration is mandatory and
  now bijection-tested by Phase 9.)
- **The engine's asset-mesh seam is one trait + one module.** `SceneAssets`
  (`crates/ludion-engine/src/extract.rs:38`) has `texture`/`mesh` methods with a disk impl and a
  cache impl; `place()` carries an unresolved `PlacedMeshSource` (extract.rs:256) so
  `render.bounds` never loads assets; `resolve_mesh` (extract.rs:540) dedups by path in
  first-encounter order and re-paths every doc diagnostic at the referencing `Mesh` component's
  JSON Pointer (extract.rs:570). `mesh_asset.rs` is bytes → `check_mesh` → f64→f32 narrowing +
  area-weighted faceted normals. All of this extends by addition.
- **`MeshSource` is externally tagged** (`components.rs:110`): primitives serialize as bare
  strings, `Asset(String)` as `{asset: path}` — a `Voxel(String)` variant serializes as
  `{voxel: path}` with zero serde ceremony. Adding a variant moves **neither WorldHash nor
  schema_hash**: WorldHash hashes serialized component *values* of live entities
  (`ludion-sim/src/hash.rs:99`), and component schemas are not in the protocol snapshot
  (`ludion-protocol/src/schema.rs:36` assembles wire types only — Phase 8's rename proved it).
  Wire surface stays 26 methods; no protocol re-bless.
- **Renderer meshes have no vertex color.** `MeshData` (`ludion-renderer/src/scene.rs:112`) is
  positions/normals/indices/hash; `MeshVertex` interleaves position+normal
  (render.rs:983); `mesh.wgsl` takes a **per-instance** flat color at `@location(6)` and
  multiplies it by Lambert shading. Voxel palettes need per-vertex (per-quad) color — the one
  shader-touching change of this phase. Primitives share the same pipeline (non-indexed), so the
  vertex layout change touches them too.
- **The GPU mesh cache is already content-hash-keyed** (`render.rs:153`, `HashMap<u64,
  MeshAssetGpu>`, retain-then-upload-on-miss per frame), and the CPU `AssetCache`
  (`ludion-engine/src/asset_cache.rs`) holds `images` + `meshes` BTreeMaps with slash-normalized
  root-relative keys; `asset.reload` resolves across the maps and re-loads through the same
  loader. Voxel meshing at load time slots into both with a third map and no policy changes.
- **`render.bounds` answers asset meshes with a model-transformed unit ±0.5 box** without loading
  the asset (bounds.rs:166–178, documented rule: bounds must answer even for a missing asset).
- **The editor inspector's variant widget supports at most ONE object variant.**
  `deriveVariantOptions` (`editor/app/src/viewer/inspect-fields.ts:189`) hard-guards
  `sawAsset` — a second single-string-property object variant makes the whole `Mesh.shape` field
  (including today's `asset:` editing) degrade to the raw-JSON textarea. Riding the fallback is
  a regression, not a degrade; the widget needs a small generalization.
- **Gallery convention:** one scene per feature in `examples/gallery/`, auto-paired golden
  `goldens/<stem>.png`, sub-dirs for assets (`meshes/crystal.mesh.yaml` is the Phase 8 example),
  README row, same-machine comparisons must be exactly SSIM 1.0
  (`ludion-testkit/tests/gallery.rs:50`).

## Design decisions (settled by this plan)

1. **Format: one `.voxel.yaml` per model, strict-YAML, row-string slices.** Document shape:
   `format: 1`, `kind: voxel`, optional `name`, `size: [x, y, z]` (positive integers),
   `palette:` mapping **single printable-ASCII characters** (0x21–0x7E, `.` reserved for empty ⇒
   93 usable entries) to `{color: [r, g, b, a]}` (finite f64s, same convention as
   `Mesh.color`), and `slices:` — a sequence of `size_y` layers, bottom-up (y=0 first), each a
   sequence of `size_z` row strings of exactly `size_x` characters (row r = z=r, column c = x=c).
   **This amends the roadmap's "block scalar" phrasing**: block scalars are L1006 in our own
   strict subset; row-string sequences are the proven `Tilemap.rows` encoding and read just as
   spatially (one row per line in canonical output). ASCII-only glyphs are deliberate — a stricter
   rule than Tilemap's any-Unicode-scalar: multi-byte glyphs break the column alignment that is
   the format's entire reason to exist. Unknown keys anywhere are strict errors with
   nearest-match suggestions (palette entries are `{color}` only until Phase 11 adds material
   fields the shader actually consumes — no dead schema, old files stay valid).
   Rejected: sparse voxel lists / RLE rows (roadmap: spatially unreadable — the exact failure
   mode voxels exist to avoid).
2. **The voxel module is a sibling of `mesh.rs` in `ludion-scene`.** `voxel.rs` reuses the
   `pub(crate)` `yaml::parse_document` tree, `scalar` emission, and `pointer_segment` escaping;
   public API mirrors the mesh set exactly: `parse_voxel`/`validate_voxel`/`check_voxel`/
   `emit_voxel`/`voxel_budget_warning` + `VoxelDocument` (name, size, palette in sorted-char
   order, slices; typed accessor `palette_index(x, y, z) -> Option<u8>`). `peek_kind` already
   returns any kind; `check-scene` gains the `"voxel"` arm beside `"mesh"` and
   `unrecognized_kind`'s suggestion list grows. Canonical emission: fixed field order, palette
   sorted by char, rows quoted by the shared `scalar_string` rule, byte-idempotent (proptested).
   **New codes: L1113** voxel document structure (parse-time, parallel to L1110), **L1114** voxel
   data consistency (size/slices/rows dimension mismatches, non-palette glyph, duplicate or
   non-ASCII or reserved palette char, non-finite color — parallel to L1111), **L1115** voxel
   size-budget warning (out-of-band like L1112; threshold: total cells > 262,144 = 64³ —
   generous; 16³–48³ is the prop sweet spot, worlds use chunks, not bigger files).
   Rejected: a new `ludion-voxel` crate (the doc layer belongs with the other canonical text
   formats; the crate graph gains nothing but a node).
3. **Greedy meshing: pure, exact, and pinned.** A `voxel_mesh` module in `ludion-engine` (sibling
   of `mesh_asset.rs`, same `load(root, rel) -> Result<MeshData, Vec<Diagnostic>>` signature:
   bytes → `check_voxel` → mesh). The mesher is a pure function of the document with a **pinned
   deterministic order**: face directions −X,+X,−Y,+Y,−Z,+Z; planes ascending along the axis;
   per-plane mask scanned row-major in a pinned axis→(u,v) table; at each unclaimed solid cell,
   extend along u then v; quads merge **only across cells sharing a palette index**
   (merged quads stay material-uniform); interior faces (solid neighbor) culled. Emission order
   IS the vertex stream order — same grid ⇒ bit-identical output, conformance-tested by double
   run. Normals are axis-aligned ±1 unit vectors (exact — no normalization, no float error);
   all coordinates are small integers ± half-integers (exact in f32). The grid is read through a
   bounds-checked lookup treating out-of-bounds as empty — the chunk-worlds door (roadmap
   decision 5), costing nothing today. Grid→model space per roadmap decision 6: bottom-center
   origin, 1 voxel = 1 unit — cell (x,y,z) spans
   `[x−sx/2, x+1−sx/2] × [y, y+1] × [z−sz/2, z+1−sz/2]`. World sizing via `Transform.scale`;
   no per-file voxel_size. Rejected: GPU meshing (breaks the CPU bit-exact tier); committed
   derived mesh files (two files describing one model will drift).
4. **Per-vertex color rides the ONE mesh pipeline — no second shader.** `MeshData` gains
   `colors: Vec<f32>` (RGBA, 4 per vertex, **always populated**: `mesh_asset::decode` and the
   primitive generator fill white); `MeshVertex` becomes position+normal+color; `mesh.wgsl` takes
   the color at a vertex attribute (instance attributes shift up one location) and the fragment
   computes `vertex_color × instance_color × shade`. Because every existing vertex gets exactly
   `[1,1,1,1]` and IEEE multiplication by 1.0 is exact, existing goldens are expected to stay
   byte-identical — **asserted, not hoped**: the wave lands only with the full gallery + game
   golden suites green at exactly SSIM 1.0. If the pinned GPU nonetheless shifts bytes (driver
   codegen), the rule is: one re-bless, in that wave, with the diff report committed as evidence
   of perceptual identity — decided here so it isn't judgment under way. `Mesh.color` therefore
   still tints voxel models (documented). Rejected: a separate voxel pipeline/shader variant
   (two shading paths = two ways; the roadmap's Phase 11 PBR upgrade must find one mesh path).
5. **`Mesh.shape` gains `Voxel(String)`; the seams extend by addition.** `{voxel: path}` beside
   `{asset: path}` (paths content-root-relative, convention `voxels/*.voxel.yaml`). `SceneAssets`
   gains a `voxel` method (disk impl → `voxel_mesh::load`; cached impl → `AssetCache::load_voxel`
   backed by a third BTreeMap, so `asset.reload` of a `.voxel.yaml` re-checks, re-meshes, returns
   the new content hash, and the hash-keyed GPU cache re-uploads — live text-editing of a voxel
   model updates a paused viewer through the existing watcher, no new plumbing).
   `PlacedMeshSource::Voxel(path)` resolves in the same first-encounter loop with diagnostics
   re-pathed at `/entities/N/components/Mesh/shape/voxel`. `render.bounds` answers with a
   model-transformed **bottom-centered unit box** (x,z ∈ ±0.5, y ∈ [0,1]) — same
   asset-load-free rule as `Asset`, but matching the origin convention; documented beside the
   existing rule in bounds.rs. WorldHash/schema_hash/wire surface untouched (fact above); the
   registry's scene-validation schema picks the variant up automatically.
6. **The inspector variant widget generalizes from "one object variant" to "N".**
   `deriveVariantOptions` drops the `sawAsset` singleton guard: any `oneOf` mixing bare-string
   unit variants with object variants *each* having exactly one required string property becomes
   a dropdown whose object choices each show the string input; commits emit `{property: value}`.
   Everything else still degrades to the JSON textarea (the three unsupported-shape tests stay).
   Editor-only, no wire change. Rejected: riding the existing fallback — it would regress
   today's `asset:` editing to raw JSON the moment the variant lands, a Phase 6 feature walking
   backwards.
7. **The file is the editor; no `voxel.*` wire commands.** The authoring loop is: edit
   `.voxel.yaml` → `ludion check-scene` → `ludion render`/`ludion test` → look at the PNG (or let
   the asset watcher live-update an attached viewer). Wire commands arrive only when the Tauri
   editor grows a voxel-painting UI, with inverse-carrying responses like every Phase 6 mutation
   — the parity gate stays clean because the editor ships nothing before then (roadmap
   decision 7).
8. **Docs go where the mesh format's docs live — no new topic.** The engine guide gains a
   "Voxel models" chapter (after Tilemaps: format walkthrough, palette discipline, the
   author→check→render loop, an authoring example an agent can copy); `docs/ludion-scene.md`
   documents the doc module beside the mesh one; `examples/README.md` gains the gallery row.
   Rejected: a standalone `docs/voxel-format.md` (the mesh format has no standalone topic;
   asymmetry for no reader benefit).
9. **The proof is THE-test applied to content.** A fresh agent, from a one-line prompt ("author a
   voxel lantern model and render it in a scene"), with only shipped docs, produces a valid
   `.voxel.yaml` + scene + green blessed golden, zero intervention. The run and its friction
   findings are written up (`phase-10-voxel-proof.md`, precedent: `phase-7-sokoban-proof.md`) —
   friction in the *format* is this phase's real output, since the format exists solely to be
   agent-writable. Companion human gate: eyeball the voxel gallery render once (windowed or the
   golden PNG).

## Waves

### Wave 1 — `kind: voxel` document module
- `ludion-scene/src/voxel.rs` per decisions 1–2: parse/validate/canonical-emit/budget,
  `VoxelDocument` + `palette_index` accessor, L1113–L1115 registered (codes table +
  `DECLARED_CODES`), `check-scene` voxel arm + updated `unrecognized_kind` suggestions.
- **Tests:** proptest emit→parse→emit byte-idempotence + LF-only (mirror `mesh_roundtrip.rs`);
  canonical-form golden pinning exact text; every L1114 misshape (ragged row, wrong layer/row
  count/width, unknown glyph, duplicate/multi-char/non-ASCII/reserved palette char, non-finite
  color) with JSON Pointer assertions; L1115 warns without failing; `check-scene` CLI
  integration on a good and a bad `.voxel.yaml`.

### Wave 2 — per-vertex color through the one mesh pipeline
- `MeshData.colors` (always populated), `MeshVertex` + `mesh.wgsl` per decision 4;
  `mesh_asset::decode` and the primitive generator fill white.
- **Tests:** upload/interleave unit tests; **the full gallery + game golden suites at exactly
  SSIM 1.0 on the pinned GPU** — the wave's entire point is proving the layout change invisible;
  the decision-4 re-bless rule applies only if this fails on driver grounds, with the diff
  report committed.

### Wave 3 — greedy mesher
- `ludion-engine/src/voxel_mesh.rs` per decision 3: pure mesher + `load` entry, colors from the
  palette per quad.
- **Tests:** conformance fixtures with exact expected streams — single voxel (24 verts/12 tris),
  2×1×1 same-entry (one merged box), 2×1×1 two entries (interior faces culled, no cross-entry
  merge), hollow 3³ shell (interior culling), checkerboard (no merging, worst case); double-run
  bit-identity on a nontrivial model; property tests (every emitted quad material-uniform;
  vertex/index counts consistent; all normals exact unit axes); out-of-bounds-reads-as-empty
  lookup unit test (the chunk door stays honest).

### Wave 4 — engine wiring + gallery
- `MeshSource::Voxel`, `SceneAssets::voxel`, `PlacedMeshSource::Voxel`, `AssetCache` third map +
  `asset.reload` dispatch, bounds rule, extract dedup/diagnostic re-pathing per decision 5.
- Gallery scene `examples/gallery/voxel-model.yaml` + hand-authored models under
  `examples/gallery/voxels/` (e.g. a lantern and a small tree — two models proves dedup and
  palette variety), blessed golden, README row.
- **Tests:** extract determinism (same scene ⇒ identical `RenderScene` twice); diagnostics from a
  broken voxel doc surface at the referencing Mesh's pointer; one-upload-per-hash at the GPU
  cache seam; wire test — paused engine, overwrite a `.voxel.yaml`, `asset.reloaded` fires, next
  streamed frame differs, WorldHash trace unchanged (mirror the Phase 7 texture-reload test);
  gallery suite green (new golden SSIM 1.0, all priors untouched); protocol snapshot
  byte-identical + parity surface still 26 (existing tests prove it by construction).

### Wave 5 — editor inspector generalization
- `deriveVariantOptions` N-object-variant support per decision 6; commit path emits
  `{property: value}` for the selected object variant.
- **Tests:** vitest — `Mesh.shape`-shaped schema with `asset` AND `voxel` variants renders the
  dropdown and commits both shapes; existing single-variant and unsupported-shape tests stay
  green; live integration smoke against a served voxel gallery scene.

### Wave 6 — docs + exit gates
- Engine-guide "Voxel models" chapter, `docs/ludion-scene.md` update, examples README per
  decision 8; frontmatter gate covers them by construction.
- **The agent proof** per decision 9: one-line prompt, shipped docs only, zero intervention ⇒
  model + green golden; transcript + friction table in `phase-10-voxel-proof.md`. Format
  friction found here feeds fixes *in this phase* (the format is the product).
- **Human gate:** one look at the voxel gallery render. Phase closes when both pass and the
  proof document is committed.

## Explicitly out of scope (triggers recorded)

- **Material palette fields** (`emissive`/`roughness`/`metallic`) — Phase 11 (PBR) adds them the
  moment the shader consumes them; decision 1 keeps the schema clean until then.
- **Chunked voxel worlds** (runtime grids, streaming, neighbor-aware remeshing, world-edit
  commands) — trigger: a game concept needing terrain/destructibility. Only decision 3's
  bounds-checked lookup seam is paid for now.
- **Voxel-painting editor UI + `voxel.*` wire commands** — trigger: the editor demands it;
  decision 7.
- **Textured PBR / UVs on voxel meshes** — the roadmap's textured-PBR trigger, unchanged.

## Risks

- **The shader/vertex-layout change is the one thing that can move existing pixels.** Contained
  by making it its own wave (2) with a green-at-SSIM-1.0 landing requirement and a pre-decided
  re-bless rule — never discovered mid-integration.
- **Mesher determinism is claimed, so it must be pinned, not emergent:** the sweep/emission order
  is specified in decision 3 and conformance-tested with exact expected streams; any later
  optimization (or the chunked-worlds rewrite) must reproduce the pinned order or re-bless
  deliberately.
- **Format ergonomics are unproven until an agent authors blind.** The Wave 6 proof is scheduled
  last but is the phase's actual gate; ambiguity found there (orientation confusion between
  rows/z and columns/x is the likely suspect) gets fixed in-format before close, not documented
  around.
- **Inspector regression:** decision 6 exists because the naive variant addition silently
  degrades Phase 6's asset editing to raw JSON. Wave 5's tests pin both variants editable.
- **Phase 9 is still in flight:** L1113–L1115 collide with nothing Phase 9 claims
  (L3017/L4004/L4005), and no shared files beyond `codes.rs` (append-only additions). Start
  after Phase 9 closes; if waves overlap for schedule reasons, `codes.rs` is the only merge
  point and the bijection test catches mistakes.
- **Scope gravity toward Minecraft** (roadmap risk, restated): authoring-first is settled; any
  world-scale ask waits for its trigger and its own plan.
