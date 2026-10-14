# Plan: Phase 8 — Opening the World (assets & sound)

> **CLOSED — shipped; decision 6 and Wave 5 record the atlas half "superseded by
> `Plan/sprite-engine.md` (2026-08-20)", and CLAUDE.md §Current state lists glTF import and minimal
> audio among the shipped features (OGG Vorbis music shipped 2026-10-03, decision 7).**
> Status: **planned** (follows Phase 7 — `Plan/phase-7-designer-flow.md`). Question: can content come
> from outside the text formats without breaking the text-first truth? Imports are *compilers into*
> the canonical text/asset formats — imported content is diffable, validatable, and agent-editable
> like everything else. Exit gate (implementation-guide): an imported glTF scene passes
> `check-scene`, renders with goldens, and re-exports canonically. Minimal audio lands here too.
> This plan assumes Phase 7's asset watcher + real `asset.reload` exist.

## Current-code facts that shape the work

- The renderer's only meshes are the three built-in procedural primitives — one non-indexed vertex
  buffer per shape built at startup, drawn instanced (`crates/ludion-renderer/src/mesh.rs:5-6`,
  `render.rs:175-177`, `render.rs:364-368`). `MeshVertex` is object-space position + faceted normal
  (`mesh.rs:17-22`). There is **no arbitrary-mesh path** — no index buffers, no vertex data from
  assets — and building one is the real rendering work of the phase.
- The `Mesh` component is `{primitive: cube|plane|sphere, color}`
  (`crates/ludion-engine/src/components.rs:104-111`). Only three gallery scenes use it — no game,
  no `ludion new` template — so evolving its scene surface is cheap: `games/breakout/run.lrp`
  embeds a scene with no `Mesh`, and unchanged component *values* leave goldens byte-identical.
- `TextureImage.hash` is documented as "the renderer's texture-cache key"
  (`crates/ludion-renderer/src/scene.rs:110`) but **no cache exists**: `render.rs:344-347` re-uploads
  every texture every frame. Phase 7 (Wave 3) delivers the hash-keyed texture cache; mesh assets
  extend that same cache to vertex/index buffers.
- `ludion-assets` is CPU bytes + XXH3 + PNG→RGBA8 only, GPU-free by boundary rule
  (`crates/ludion-assets/src/lib.rs:17-18`). The deterministic PNG *encoder* lives on
  `RenderedImage` in `crates/ludion-render-graph/src/png_io.rs:27` — a wgpu crate the importer
  must not depend on. Reuse requires relocating the encode function into `ludion-assets`.
- `extract()` resolves sprite texture paths in first-encounter order, deduplicated, collecting all
  errors with JSON Pointers (`crates/ludion-engine/src/extract.rs:63-93`). Mesh-asset resolution
  slots into the same seam.
- No `import` command exists (`crates/ludion-cli/src/`: check_scene, docs, dump_graph, explain,
  new, render, replay, serve, sim_hash, test). Scene machinery is complete: strict-YAML parse,
  registry validation, canonical emit (`crates/ludion-scene/src/lib.rs:123-241`); `Parent` refs are
  by document position (`codes.rs` L1017) — glTF node trees map onto this directly.
- Wire surface is 25 methods today (26 after Phase 7 adds `engine.shutdown`). `ludion-protocol`
  never names component types (payloads are
  `serde_json::Value` + registry name), so the `Mesh` evolution moves `engine.describe` output but
  **not** `schema_hash` — no protocol rev, no snapshot/TS/run.lrp re-bless this phase.
- Audio: nothing exists — no crate, no component, no dep (workspace grep for cpal/rodio/audio is
  clean).
- L-code occupancy: L1001–L1018 and L1101–L1102 taken in the L1xxx scene/assets range
  (`crates/ludion-core/src/codes.rs:83-168`); Phase 7 claims L1103 (asset watcher) and
  L1019/L1020 (tilemaps); L1104+, L111x, L12xx free; no L5xxx range yet.

## Design decisions (settled by this plan)

1. **Mesh assets are canonical text**, one strict-YAML document per mesh (`meshes/*.mesh.yaml`):
   `format: 1`, `kind: mesh`, optional `name`, flat `positions` (3 floats/vertex, required),
   optional `normals` (absent → faceted normals computed at load, a pure function of
   positions+indices), optional `uvs` (stored losslessly; the flat-color mesh pipeline ignores
   them until textured meshes are demanded — recorded trigger), required `indices` (triangle
   list). Ryu floats, canonical writer, byte-idempotent; the file's XXH3 content hash is the GPU
   upload key. HD-2D low-poly keeps these small; Pillar 4 says imports compile into diffable text;
   an agent can author or patch a mesh by hand. Style guardrail: validation *warns* (L1112) above
   **10,000 vertices** — the sphere primitive is ~600, so 10k is already suspicious for the style.
   `check-scene` validates mesh docs too, dispatching on the top-level `kind:` key (scenes have
   none; their unknown-key rule L1010 keeps it that way). New codes: L1110 (document
   structure/shape), L1111 (data consistency: counts, index range, non-finite), L1112 (budget,
   warning). *Rejected:* binary mesh blobs with text manifests (breaks text-first — the mesh
   itself becomes unreviewable); meshes inlined in scene files (bloats every scene diff, defeats
   reuse); a separate `check-mesh` command (`check-scene`'s job is "validate a Ludion document").
2. **`Mesh` component: `primitive:` becomes `shape:`, and the enum gains an `asset` variant**
   carrying a project-root-relative path (`shape: cube` | `shape:` / `asset: "meshes/rock.mesh.yaml"`
   — serde external tagging; the engine enum becomes `MeshSource`, loses `Copy`). *Rejected:* an
   additive `asset: Option<String>` field — it serializes `asset: null` into every existing `Mesh`,
   changing WorldHash for all mesh scenes, and creates two fields expressing one meaning;
   keeping the `primitive:` key name — a field named "primitive" holding an asset path is a
   permanent lie read by every future agent, and the rename is cheapest now (3 gallery scenes,
   zero games). *Consequences:* gallery scenes rewritten mechanically (values unchanged → goldens
   byte-identical, run.lrp untouched); `engine.describe` grows a data-carrying enum variant — the
   editor inspector's schema widgets must degrade gracefully (see Risks); primitives stay.
3. **`ludion import <input.gltf|.glb> --out <dir>` is a CLI compiler, like `ludion new`** — no
   engine process. Deterministic: same input → byte-identical output tree (canonical writers,
   deterministic PNG re-encode, glTF document order everywhere, node names deduped with stable
   index suffixes; unnamed nodes → `node-<index>`). Mapping: node tree → entities with `Transform`
   (TRS; quaternions → Euler XYZ degrees) + `Parent` by document position; meshes → mesh docs;
   images → PNG re-encode; materials → `baseColorFactor` → `Mesh.color` **only** (no PBR pipeline
   expansion — anti-goal); perspective camera → `Camera`; `KHR_lights_punctual` point →
   `PointLight` (both one-line correspondences to existing components; the anti-goal is scoped to
   this phase — the deliberate PBR arc, factors included, is `Plan/roadmap-voxel-mesh-pbr.md`).
   Every unsupported feature —
   skins, animations, morphs, other extensions, `baseColorTexture` (the mesh pipeline is
   flat-color), JPEG images, orthographic glTF cameras — is **L1202, naming the
   node/mesh/accessor** (rustc bar), never a silent drop. Default: error; `--allow-lossy`
   downgrades L1202 to warnings and compiles what maps. L1201 = unreadable/invalid glTF; L1203 =
   output write failure. *Rejected:* an `asset.import` wire command — import is a build-time
   compiler, not a live-engine capability; the parity gate binds *editor* capabilities, and the
   trigger to revisit is the editor growing an import UI.
4. **Crate placement: new `ludion-import`** (deps: core, scene, assets — *not* engine/renderer; it
   is a compiler over text formats), CLI wires it up (bins may be fat, §2). Parse dep: the **`gltf`
   crate, default features off** — no `image` feature (our `png` dep decodes; JPEG is L1202, not a
   second decoder). Justification: accessor/stride/sparse/component-type minutiae are exactly where
   a hand-rolled parser silently misreads, the opposite of the rustc bar; `gltf` is the
   reference-quality, most-exercised Rust reader with the loader layer separable from any image
   stack. *Rejected:* hand-rolled serde parse (silent-misread risk), `easy-gltf`/`goth-gltf`
   (thinner ecosystems, or they drag their own image dependencies).
5. **Renderer arbitrary-mesh support:** `RenderScene` gains `mesh_assets: Vec<MeshData>`
   (positions/normals/indices + content hash — plain data, mirror of `TextureImage`);
   `MeshShape` gains `Asset(usize)` indexing it; extract resolves `shape: asset` paths via
   `ludion-assets` bytes → mesh-doc parse, deduped by hash, errors carrying the JSON Pointer to the
   `Mesh` component (same seam as textures). Renderer adds an **indexed** draw path (primitives
   stay non-indexed) through the existing lit pipeline — same `MeshVertex` position+normal layout,
   same lambert+ambient shading, faceted normals preserved from the asset — instanced by grouping
   instances per mesh index, exactly as per-shape grouping works today (`render.rs:364-368`).
   Mesh vertex/index buffers join the content-hash-keyed GPU cache Phase 7 introduces for textures
   (upload on miss, same eviction rule), finally honoring `scene.rs:110` across both asset kinds.
   Goldens for a mesh-asset gallery scene on the pinned GPU.
6. **Atlas/image pipeline: not built now — SUPERSEDED 2026-08-20 by `Plan/sprite-engine.md`
   (trigger fired: `Tilemap` shipped, a second game wants per-region sprites).** Nothing in the gallery or games uses sprite sheets
   (two standalone PNGs total). Recorded design shape so the decision is pre-made: `kind: atlas`
   text doc (named regions → pixel rects) + packed PNG, `Sprite` gains an optional region name,
   importer accepts sprite sheets. **Trigger:** Phase 7's second game / tilemap work needing
   per-region sprites. No speculative packing algorithms.
7. **Audio, minimal, determinism-preserving.** Sim side emits *events as plain data*: game systems
   push `AudioEvent { sound: String, gain: f32 }` into an `AudioQueue` ECS resource
   (engine-defined, in the prelude); `Engine::tick` drains it at the frame boundary into a
   frame-stamped in-engine event log plus an optional crossbeam sender — the audio analog of the
   sim→render handoff, strictly one-directional. Playback: new **`ludion-audio`** crate (deps:
   core, assets) hosting a sink thread over **cpal** (rodio rejected: a mixer + symphonia decoder
   stack on top of cpal; our mixer is ~100 lines of gain-and-sum over decoded PCM). Assets: **WAV
   only** (`ludion_assets::load_wav`, PCM16/mono/stereo → f32; L1104 on bad WAV); OGG Vorbis
   music shipped 2026-10-03 on the trigger (external shmup request 10): streamed via lewton in `ludion-audio`. Playback never feeds
   back into sim; denylists untouched; events are a pure function of the sim, so a replay
   regenerates the exact stream — nothing extra is recorded. Verification is text-first: testkit
   assertions on the event log (which sound, which frame); **no audio goldens**. Resources are
   excluded from WorldHash, so no re-bless. No-device environments (CI) get a warning and a silent
   sink, never a failure — new **L5xxx audio range**, L5001 = device unavailable. **Protocol
   story: deferred.** No `audio.events` stream this phase — the log is testable in-process and
   growth is additive when needed; trigger: the second game's iteration loop (or the editor)
   needing over-the-wire audio verification.

## Waves

### Wave 1 — Mesh asset text format (no renderer change)
- `ludion-scene` mesh-document module: parse (shared strict-YAML layer) / validate / canonical
  emit; codes L1110–L1112; `check-scene` dispatches on `kind:`.
- **Tests:** proptest emit→parse→emit byte-idempotence; hand-authored fixture round-trip;
  L1111 on bad indices/counts; L1112 warning above the vertex budget; `check-scene` CLI on both
  document kinds.

### Wave 2 — Renderer mesh assets + component evolution
- `Mesh.shape` rename + `asset` variant; extract resolution (hash-dedup, JSON-Pointer errors);
  `RenderScene.mesh_assets`; indexed instanced draw; mesh buffers join Phase 7's hash-keyed GPU
  cache; gallery scenes rewritten to `shape:`; new gallery scene with a hand-authored mesh asset —
  proving agent authorship *before* any importer exists.
- **Tests:** extract determinism (two extractions equal); goldens — existing gallery byte-identical
  (values unchanged), new mesh-asset scene blessed on the pinned GPU; cache keyed correctly
  (same asset twice → one upload); editor inspector renders a `Mesh` without crashing.

### Wave 3 — `ludion import`
- Relocate deterministic PNG encode into `ludion-assets` (render-graph delegates; bytes unchanged).
- `ludion-import` crate + CLI command per decisions 3–4; committed small fixture
  (`examples/import/`): hand-made .gltf, 3-node hierarchy, two low-poly meshes with distinct
  base-color factors, one PNG-textured material (exercises the L1202 lossy path + PNG re-encode),
  perspective camera, one point light.
- **Tests:** import twice → byte-identical output tree (compiler determinism); every unsupported
  fixture feature → L1202 naming its node/accessor; `--allow-lossy` compiles with warnings;
  imported scene passes `check-scene`.

### Wave 4 — Audio
- `ludion_assets::load_wav` (L1104); `AudioEvent`/`AudioQueue` + tick-boundary drain into the
  frame-stamped log + optional sender; `ludion-audio` cpal sink (L5001 warning path);
  facade/`serve` wiring; a sound in `games/hello-paddle` (bounce) as the proof content.
- **Tests:** testkit event-stream assertion (scripted input → expected `(frame, sound)` pairs);
  determinism (double run → identical event log; WorldHash unaffected); replay regenerates the
  stream; no-device path is a warning, suite green on CI.

### Wave 5 — Atlas (conditional) — superseded by `Plan/sprite-engine.md` (2026-08-20)
- Built only if the Phase 7 trigger (decision 6) has fired by the time waves 1–4 land; otherwise
  the recorded design ships as plan text only.

### Wave 6 — Exit criteria
- **Mechanized gate** (`ludion-import` integration test + testkit): fixture → `ludion import` →
  `check-scene` green → blessed golden render on the pinned GPU → `scene.load`→`scene.save`
  byte-equal (canonical re-export) → re-running import byte-identical.
- Audio: event-stream assertion test committed (wave 4); **human gate (user):** run hello-paddle
  with sound, listen once.
- Parity audit unchanged by construction (no new wire methods this phase) — assert surface still
  26 (25 + Phase 7's `engine.shutdown`).

## Risks

- **Editor inspector meets a data-carrying enum variant** (`shape: {asset: …}`): today's
  schema-driven widgets cover unit-enum strings; the union must at minimum degrade to a readable
  fallback, not crash. Scheduled inside wave 2, where the schema changes.
- **`gltf` dependency tree** — dependency discipline is a pillar-adjacent value. Mitigation:
  default-features off, audit the resolved tree in the wave-3 commit; the crate touches only
  `ludion-import` (a leaf the engine never depends on).
- **quat→Euler conversion** is total but not round-trip-exact against the glTF source; import
  determinism is unaffected (pure function), and visual fidelity is absorbed by the fixture golden.
- **Text mesh size**: a 10k-vertex mesh is ~80k lines of YAML. The L1112 budget warning plus the
  HD-2D style keep real content far below it; if it ever binds, the answer is styling content
  down, not a binary format.
- **cpal device variability**: playback sits outside every determinism gate (no audio goldens,
  sink never in tests); the only CI-visible surface is the no-device warning path — tested
  explicitly so a headless runner can never fail on audio.
- **Material scope creep** — "just one more glTF material field" is how a PBR pipeline arrives
  uninvited. Decision 3's mapping table is exhaustive; anything outside it is L1202 by
  construction. The vertical slice that demands more now has a name — the PBR phase in
  `Plan/roadmap-voxel-mesh-pbr.md` — and material expansion happens there, not here.
- **Phase 7 parallelism**: this plan assumes Phase 7's watcher/`asset.reload` and defers atlases
  to its tilemap demand. If Phase 7 slips, reload degrades to restart-with-snapshot (already
  works) — no wave here blocks on it.
