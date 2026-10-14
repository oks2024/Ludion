# Plan: Comfy generation — the `kind: generation` recipe, the accept step, and the mesh loop

> Status: **DESIGN SETTLED (2026-09-17, grilling session — every ruling the user's). Wave 0 (provisioning)
> DONE 2026-09-18; Wave 1 mechanized-complete 2026-09-18 (steps 1–6, 8–10; step 7's live loop and
> step 11's sitting owed); `Plan/textured-pbr.md` shipped in full the same day (revs 752–755); Wave 2
> mechanized-complete 2026-09-18 (checkpoint 2 PASSED 2026-09-21); Wave 4 and then
> Wave 3 mechanized-complete the same day (checkpoint 3's sitting owed).**
>
> **Wave 3 outcome (mechanized-complete 2026-09-18; checkpoint 3's sitting owed).** The part:
> `SculptShape::Mesh { path, size? }` — `size` an optional positive scalar, world units per document unit,
> default `1`, never written back when absent — with the `shape: voxel` diagnostics exactly (`L1157`, `L1101`,
> the mesh document's own `L1110`/`L1111`, re-pointed at `/parts/<i>/path`). The field
> (`crates/ludion-sculpt/src/soup.rs`): one fixed-order BVH per distinct source, closest-triangle distance with
> ties to the lower index, a hierarchical generalised winding number for sign (`w > 0.5` inside), **baked once
> onto a grid 64 cells along the mesh's longest axis** into the EDT's own `SignedDistanceGrid` and
> `Arc`-shared — so a mesh part folds at a voxel part's cost and carries form, not fine surface or textures.
> The bake runs through `ludion_core::par` with bytes identical at 1, 2 and 8 workers; the 4 986-triangle
> generated idol bakes in ≈126 ms at 8. `SculptSources` holds both kinds (`insert_voxel` / `insert_mesh`);
> `read_sculpt_sources` reads both, so the loader, `check-scene` and the model sheet followed. The conformance
> hash in `tests/mesh_part.rs` is landed from the Windows box — **owed: the M4 must agree before it counts.**
> Step 3: `/parts/<i>/size` is a `--set`/`--sweep` pointer that creates an omitted `size`; **found on the way:
> the editor's Models view (named Sculpt at the time) had never given a voxel part's scalar `size` a slider** — `doc.ts` now reads an inline
> `size` as a scalar, for both shapes. **The demo** (`Plan/artifacts/comfy-generation-log.md`, wave 3): a
> five-primitive head blank, its four `render --model` views, `pixal3d-multiview.json` (41 titled nodes, no
> unwrap/bake tail — a sculpt part reads no texture) and `creature-head-mesh.gen.yaml`; six submissions,
> ≈24.6 min of GPU, 20 renders; the accepted head (1 997 vertices) is a `shape: mesh` part of
> `examples/gallery/sculpts/creature-generated.sculpt.yaml` at `blend: 0.07`. **The author's read, both halves:**
> the blend is an unqualified success — no seam, the sheet reads as one body — and the *value* is not shown:
> four-view conditioning reconstructs the silhouette it is given and invents no surface, so the head came back
> with less detail than its blank (a control run on the full authored creature proved the pipeline, not the
> subject, sound). Directability holds; bought surface does not, through this loop. Sheet for the sitting:
> `Plan/artifacts/comfy-generation-creature-mesh-sheet.png`. Ten ranked tool findings in the log, first among
> them that `render --model` is the conditioning rig and has no rig mode, and that a mesh part's scalar `size`
> cannot correct a generator's proportion error. Docs: `sculpt-format` split off `sculpt-format-sources`,
> `dev-docs/ludion-sculpt` split off `sculpt-sources`, the multi-view loop in `generation-craft-mesh`.
>
> **Checkpoint 3 verdict (the user, 2026-09-18, on the creature sheet): the result is not liked and the
> multi-view workflow is not explored further.** Nothing is reverted and no tool is removed — `shape: mesh`,
> the Pixal3D workflow and the recipe stay as working capabilities. The direction is the textured generated
> mesh kept as-is and edited in place, texture edits first (the generated idol "looks really good" at ≈5 k
> triangles): `Plan/mesh-texture-editing.md`.
>
> **Wave 4 outcome (mechanized-complete 2026-09-18, run ahead of Wave 3; the editor sitting owed).** Wave 3's
> files then held an uncommitted sculpt follow-up, so Wave 4, which shares no file with it, ran first. The view: `editor/app/src/views/GenerationView.tsx` over pure modules in
> `editor/app/src/editor/generation/` — the `AssetBrowser` over `generation/recipes`, a recipe form that splices
> one token of the file per edit through the Models view's session machinery (a prompt written as one
> double-quoted line, line breaks folded, integer spelling kept), Check / Generate, a run picker, the candidate
> grid read from the run folder's own `run.json` (so a run the view never submitted fills it identically), and
> accept with a destination prompt and a mesh-only `--allow-lossy` box. The host: `src-tauri/src/generation.rs` —
> `gen_invoke` allowlists the seven verbs, owns `--json` and `--url`, spawns the CLI with the project as its
> working directory and never persists the address; `read_editor_image` and `editor_file_size` are the two read
> helpers the grid needed. No wire method, no schema movement. `CLAUDE.md`'s non-negotiables carry decision 15's
> line. **Owed — the sitting's click path:** `npm run tauri dev` in `editor/app`, project `games/vigil` →
> Generation → `idol-portrait` (status `image · offline ok · live skipped` with no server) → drag `count`, retype
> the prompt, confirm a one-token diff → with ComfyUI up and no check lane on that GPU, Generate, eight labelled
> cells → Accept one → repeat on `idol-mesh` for the mesh cell and the lossy box.
>
> **Wave 2 outcome (mechanized-complete 2026-09-18; checkpoint 2 passed 2026-09-21).** It opened by fixing the first
> loop's ranked tool defects (`gen check`'s `offline:`/`live:` lines, forward-slash paths everywhere, the `next:`
> line, the render knobs out of every `gen` help, the measured prompt rules in the craft doc). Step 1 was almost all
> already there — the client read `outputs[node]["3d"]` from the start — so the wave added the mock suite's mesh
> happy path. `gen accept` on a mesh run takes a **directory**, refuses a non-empty one (`L1172`), calls
> `ludion_import::import` in-process and splices `origin:` into every produced mesh document before the single
> write; `accept` takes the component registry as a parameter because the builtin set lives engine-side and
> `ludion-gen` must not depend on the engine. **Rulings made during the build:** `gen accept --allow-lossy`
> exists and strict stays the default, because `SaveGLB` always writes `doubleSided: true` (`L1202`) and a
> generator has no re-export; provenance gains `images` (name, project-relative path, content hash), since a
> mesh run's `prompt` is empty and the reference image *is* its brief; the accepted idol lives at
> `games/vigil/scenes/meshes/idol-generated/` and the gallery harness reaches it there rather than copying it;
> each accepted mesh earns its own reasoned pair of size-allowlist lines (vertex text and maps both cross 1 MiB
> at Decimate 5 000 / 1024² bakes). **Content:** `trellis2-img2mesh.json` — the full textured TRELLIS.2 chain,
> 36 titled nodes, green on its first live submission: reduce → unwrap → three bakes → `SaveGLB`, with
> `drop_inverted_components` on the remesh (the UDF remesh leaves an inverted inner shell the small-component
> filter misses); `idol-mesh.gen.yaml` binds the accepted reference and bakes at 1024. **One run, one candidate,
> nothing rerolled:** 485 s cold, 4 190 vertices / 4 986 triangles, no `L1112`, three maps (occlusion packed with
> metallic-roughness). The author's read, not a ruling (`Plan/artifacts/comfy-generation-log.md`, the wave-2 row
> and findings): the generated idol beats the sculpt on silhouette and surface and loses the glowing eye — the
> chain bakes no emissive map — and arrives as one shell with no parts. Sheets for the sitting:
> `Plan/artifacts/comfy-generation-idol-mesh-sheet.png` beside `…-idol-sculpt-sheet.png`; note the subject faces
> +Z, so the sheet's `FRONT -Z` cell shows its back. `idol_generated_sheet_test` first-blessed on the RTX 3090,
> stable over three runs; **owed:** the M4's `examples/gallery/goldens/macos/idol-generated-sheet.png`. `docs/generation.md` split into `generation` /
> `generation-runs`, the mesh craft into `generation-craft-mesh`; `dev-docs/ludion-import.md` sits ≈150 bytes over
> its budget.
>
> **Checkpoint 1 verdict (the user, 2026-09-18): the image half "looks great and seems useful, we keep it."**
>
> **Checkpoint 2 PASSED (the user, 2026-09-21).** The sheet verdict came first (2026-09-18: "looks really
> good" — Wave 3 ran — and "looks good from far" on the editor's model stage); the close look was blocked until
> `Plan/editor-stage-camera.md` put a free camera on the stage, and with it the user's verdict on the generated
> idol up close is **"really great"**. The image-to-textured-mesh chain is accepted as the mesh path. The same
> look is the textured-PBR exit gate's human half (`Plan/textured-pbr.md` Owed), which it also pays.
>
> **The DINOv3 credit line, ruled by the user 2026-09-18: credit it.** The licence's display duty attaches to
> distributing the *DINO Materials or derivative works thereof* — defined as Meta's weights, code and
> documentation, with "output and results" named separately — and Ludion distributes none of them, so the duty
> arguably never triggers; the credit is given anyway because it costs one line. `THIRD-PARTY-NOTICES.md` carries
> "Built with DINOv3" and lists each generated mesh, `licenses/DINOv3-LICENSE.txt` is the agreement's copy, the
> public README links the notices when the publication plan writes it, and a game shipping a generated mesh
> outside this repository repeats the phrase in its credits or about screen. Each newly accepted mesh adds its
> line to the notices.
>
> **Wave 1 outcome.** Codes `L1161`–`L1172`; `ludion-scene/src/generation.rs` (the recipe and the `Origin` block,
> one walk and one writer for both carriers; `origin:` is the mesh document's eleventh key, after `name`,
> emitted only when present; `ScalarValue` keeps four spellings because a ComfyUI `INT` refuses `1024.0`);
> `generation` and `origin` registered as engine kinds; the `ludion-gen` crate (`recipe`, `workflow`, `client`,
> `run`, `sheet`, `accept`, `clock`) over `ureq` 3 without TLS and the workspace's synchronous `tungstenite`,
> with a mock ComfyUI server in its tests covering the happy path, every code, the polling fallback and the
> `--no-wait` + `fetch` pair, and the live smoke as an ignored test; `ludion gen` with all seven verbs and
> `--json` on each (`crates/ludion-cli/src/gen_cmd.rs` — `gen` is a keyword in edition 2024); the FLUX.2 klein
> text-to-image and image-to-image workflows with role titles and the two idol recipes under
> `games/vigil/generation/`; `docs/generation.md`, `docs/generation-craft.md`, `dev-docs/ludion-gen.md`.
> **Rulings made during the build:** the address resolves flag → `LUDION_COMFY_URL` → ComfyUI's loopback
> default (decision 3 amended); a `{ image: x }` reference is a nested block map and a prompt is one
> double-quoted line (the strict subset refuses flow maps and block scalars); a run completes on
> `execution_success` or `executing` with a null node, never on `executed` (one per output node); `gen check`'s
> offline pass applies overrides with placeholder upload names so an image-bound recipe is title-checked
> without a server; `gen accept` refuses a mesh candidate (Wave 2 routes it through `ludion import`); the
> sheet's label font is a built-in 5×7 table (the renderer's glyph atlas is GPU-side); `accept` re-encodes
> through `ludion_assets::encode_png`, which is the engine's one canonical PNG writer, so no separate
> normalizer exists. **Step 7's live loop** ran 2026-09-18: four portrait runs and one variation run, 36 candidates, 5 sheets, 142 k
> tokens, under three minutes of GPU, to an accepted reference (`games/vigil/generation/references/idol-reference.png`,
> run 4 seed 307) — rows, the converged prompt and eleven ranked tool findings in
> `Plan/artifacts/comfy-generation-log.md`; the sheets for the sitting are `Plan/artifacts/comfy-generation-idol-*.png`.
> **Ruling:** an accepted reference keeps the generator's full extent (shrinking it changes what the next run sees), so
> `games/vigil/generation/references/*.png` and the checkpoint sheets are size-gate allowlisted by glob, like the
> other arcs' sitting evidence. **Shared-checkout finding:**
> three consecutive check lanes failed only on fixed-scratch or timing tests colliding with another
> instance's lane, every one green in isolation; lanes are now announced between instances before they start.
>
> **Wave 0 outcome.** The private kit `infra/comfyui/` (setup, headless launch with detach / web-UI / remote-bind
> switches, health check, stop, five smoke graphs with transcripts, and a README carrying every weight's source,
> hash and licence). Every model file lives on one shared network model root reached through ComfyUI's
> `extra_model_paths.yaml`, so every ComfyUI on the network sees one model set; the checkout's own `models/`
> holds nothing. **Text-to-image pick (decision 14): FLUX.2 [klein] 4B**, Apache-2.0 end to end (model, Qwen3-4B
> text encoder, VAE — the klein repo's own VAE, not the identically named non-commercial one the stock template
> links), 1024² in 4 steps at cfg 1, ≈4 s warm; Z-Image Turbo (Apache-2.0, ≈10 s) provisioned as the alternate.
> Mesh side: TRELLIS.2 and Pixal3D checkpoints and VAEs are MIT; BiRefNet (background mask) and MoGe (FOV
> estimate) MIT. **Licence caveat, owed to the user:** both mesh generators condition on Meta's DINOv3 encoder,
> under the DINOv3 licence — commercial use allowed, but a shipping product must display "Built with DINOv3"
> and redistribution carries the agreement; there is no drop-in alternative encoder. **The density finding,
> now the craft rule:** the raw TRELLIS.2 decode is ≈22.5 M faces and 99.3 % two-manifold; `DecimateMesh` straight
> off it shatters (456 components, half the edges on a boundary — the first GLB the user saw), while
> `RemeshMesh` (256, drop small components) **then** `DecimateMesh` lands 4 962 faces in 9 components with zero
> non-manifold edges. Measured on the reference box: text-to-image 4.2 s warm at 18 GB peak; TRELLIS.2 full chain
> ≈150 s at 17 GB; Pixal3D ≈82 s at 10 GB; the remesh+decimate pass ≈21 s. Not smoked: the TRELLIS.2 texture
> stage (its VAE is in place — Wave 2's first job), the web-UI switch, the Git Bash launcher twin.
>
> **Dependencies.** Wave 2 depends on `Plan/textured-pbr.md`, whose trigger — "the first external
> textured asset worth importing with its texture maps intact" — **FIRED with this arc on 2026-09-17**
> (decision 4): a generated mesh arrives as a `.glb` with base-color, metallic-roughness, normal and
> occlusion maps embedded, and dropping them is exactly the silent loss that plan exists to refuse.
> That arc runs first, on its own, import-only as written; nothing in it is reshaped by this plan.
> Wave 3 depends on `Plan/mesh-modeling.md` (the `ludion-sculpt` crate, its `SculptSources` reader and
> the `shape: voxel` precedent) being through checkpoint 2. Wave 1 depends on neither and starts in
> parallel with the sculpt arc's checkpoint-2 sitting (decision 18).
>
> **Relationships.** This is a **step of the modelling loop, not a second pipeline** (decision 2).
> `Plan/mesh-modeling.md` authors form from text; this arc buys form and surface from a diffusion model
> and freezes the chosen candidate into the same asset folders the same renderer already reads. It
> reopens that plan's decision 11 ("textures are out of this arc", trigger: the first generated texture
> worth applying to a sculpt) from the import side only. It touches no wire method and moves no schema
> hash (decision 5). `Plan/voxel-authoring-tools.md` and `Plan/sprite-engine.md` own the other
> authoring lanes and are untouched.
>
> **How to read this plan.** Decisions 1–19 are the rulings; *The recipe document*, *The run folder*,
> *Provenance*, *The verbs* and *The client* are the specification; each Wave is a numbered step list an
> instance executes top to bottom, with its tests, docs, verify commands and exit criterion. A wave
> closes by appending its outcome to this Status block (checkpoint verdicts are recorded as decisions,
> never as prose), refreshing the root `CLAUDE.md` Current-state paragraph, and pruning the notebook.
>
> Every claim of current-code fact below was read from the tree on 2026-09-17 (file:line pointers are of
> that date; re-grep before editing). Every ComfyUI fact was read from the local ComfyUI checkout at
> upstream HEAD the same day; machine facts — where that checkout lives, the server's address, which
> models are installed — belong to the private provisioning kit, `infra/comfyui/`, and appear nowhere in
> this repo's public tree.

## Why

The sculpt arc measured what agent-authored form costs: the gallery idol took 10 sheet renders and
~191 k tokens, the creature 11 sheets (16 renders) and ~155 k (`Plan/artifacts/mesh-modeling-log.md`).
That already beats the Blender-MCP baseline by an order of magnitude, and it produces a text source of
truth that diffs. What it cannot produce is **surface**: a stone idol's weathering, moss in a carving,
the grain of a plank. Per-vertex scalar material and a palette give value and glow, never detail. The
user has a local ComfyUI install, a 24 GB card, and the core Trellis2 / Pixal3D / Hunyuan3D
image-to-mesh stack already in it.

The honest framing is that diffusion is the opposite of everything this repo rests on: not
deterministic, not diffable, not verifiable by a test. So the integration keeps the non-determinism
**outside** the committed tree. What is committed is a *recipe* — text naming a workflow and its
overrides, iterated like any other source — and, after a human or an agent picks a winner, a *frozen
asset*: an ordinary `.mesh.yaml` or `.png` with a provenance block saying where it came from. Between
them sits an explicit `accept`. Nothing in a scene points at a recipe; no level load waits on a GPU; no
drift gate ever asks a diffusion model to reproduce itself.

The second reason is loop shape. ComfyUI is an excellent *human* tool and a poor agent tool: its surface
is a graph in a browser. But its HTTP API is complete and `/object_info` is a full typed schema of every
installed node — exactly what an agent reads well and a human reads badly. An agent that can list node
types, validate a graph before submitting it, submit N seeds and read back a labelled contact sheet has
the change→verify→correct loop the North Star counts, with a human needed only at the taste step.

## What exists today (verified)

### ComfyUI (the local checkout, upstream HEAD, surveyed 2026-09-17)

- **HTTP.** `POST /prompt` with `{prompt: <API graph>, client_id?, prompt_id?}` → `{prompt_id, number,
  node_errors}`, or `400` with `{error, node_errors}` — validation happens at submit, so a bad graph
  fails immediately and structurally. `GET /history/{prompt_id}` → `{<id>: {prompt, outputs, status}}`,
  where `outputs[<node_id>]` holds `{images: [...]}` or `{3d: [...]}`, each entry `{filename, subfolder,
  type}`. `GET /view?filename&subfolder&type=` (`output` | `input` | `temp`) → bytes. `POST
  /upload/image`, multipart `image` plus `subfolder`/`type`/`overwrite` → `{name, subfolder, type}`.
  `GET /object_info[/{class}]` → per class `{input: {required, optional}, output, output_name,
  display_name, category, output_node}`. `GET /ws?clientId=` streams `status`, `executing {node}`,
  `progress`, `executed`, `execution_error`. Also `/queue`, `/interrupt`, `/system_stats`, and a newer
  `/api/jobs[/{id}]`. A machine-readable `openapi.yaml` ships in the checkout; default port 8188, bound
  to the loopback address unless told otherwise.
- **Two graph formats, one submittable.** The **API format** is flat — `{"<id>": {"class_type",
  "inputs", "_meta": {"title"}}}` — and is the only thing `/prompt` accepts. The UI/litegraph format
  (nodes array, links array, widget values by position) is the browser's default save;
  File → **Export (API)** writes the other.
- **Image-to-mesh is core, not a custom node.** `comfy_extras/nodes_trellis2.py` ships
  `Trellis2Conditioning`, `Pixal3DConditioning` (fov default 49.13°, pad 1.1),
  `Pixal3DMultiViewConditioning` (optional front / left / back / right 1024² views on a 90° orbit rig,
  fov default 20°), `Trellis2ShapeStage` / `TextureStage` / `UpsampleStage`, and
  `VaeDecodeShapeTrellis` → `MESH`. Hunyuan3D 2.1 is core too (`nodes_hunyuan3d.py`) and is the one 3D
  blueprint that ships saved. **Every path is image-to-mesh**: there is no text-to-mesh node, which is
  why decision 10 makes the image loop first-class rather than an implementation detail.
- **Export.** `SaveGLB` is an output node writing a `.glb` with UVs and embedded PBR base-color,
  metallic-roughness, normal and occlusion textures; `filename_prefix` defaults to `3d/ComfyUI`, so
  outputs land at `output/3d/ComfyUI_00001_.glb`.
- **Mesh post-processing is core.** `comfy_extras/nodes_mesh_postprocess.py`: `DecimateMesh`
  (`target_face_count` default 200000, quadric placement, feature-edge weights), `RemeshMesh`
  (narrow-band dual contouring, `resolution` default 512), `UnwrapMesh`, `BakeTextureFromVoxel` (2048),
  `FillHoles`, `WeldVertices`, `MeshSmoothNormals`.
- **No saved Trellis2 or Pixal3D workflow ships.** We author them as API JSON (decision 11).

### Ludion

- **`ludion import` already takes both halves accept needs.** `crates/ludion-cli/src/import.rs`
  implements `ludion import <input.gltf|.glb|.png> --out <dest>`; `run_gltf` (`import.rs:154`) drives
  `crates/ludion-import`, producing `scene.yaml` + `meshes/*.mesh.yaml` + `textures/*.png`. **`.glb` is
  supported today** (`crates/ludion-import/src/lib.rs:162`). Any texture reference is an `L1202`
  (`lib.rs:41-45`); `--allow-lossy` drops it — precisely what `Plan/textured-pbr.md` removes.
- **The mesh document.** `crates/ludion-scene/src/mesh.rs:38-49`: `MESH_VERTEX_BUDGET = 10_000` and the
  ten-key canonical set `format, kind, name, positions, normals, uvs, emissive, metallic, roughness,
  indices`. No vertex-colour lane. `L1112` is the non-fatal vertex-budget warning.
- **`Mesh.shape` is `MeshSource`** (`crates/ludion-engine/src/components.rs:283`): `Cube | Plane |
  Sphere | Asset | Voxel | Assembly | Sculpt`. This arc adds no variant — a generated mesh is an
  `Asset`, a generated sculpt part lives inside the sculpt document.
- **The sculpt crate's source seam is Wave 3's template.** `ludion_sculpt::mesh_with(doc,
  &SculptSources)` (`crates/ludion-sculpt/src/lib.rs:200`) takes the *parsed* documents a `shape:
  voxel` part names, one transform per distinct path, `Arc`-shared by every copy; the crate reads no
  file. `ludion_engine::read_sculpt_sources` (`crates/ludion-engine/src/sculpt_mesh.rs:63`, re-exported
  at `lib.rs:137`) is the one reader, shared by the loader, `check-scene` and the model sheet
  (`model_sheet.rs:1050`, `:1573`).
- **The model stage.** `render --model` synthesizes its scene at
  `crates/ludion-engine/src/model_sheet.rs:2786`; the editor's stage is
  `games/vigil/scenes/model-stage.yaml`; `editor/app/src/editor/voxel/AssetBrowser.tsx` already takes
  `root` and `ext` as props (two kinds share it), and `editor/app/src/views/{SculptView,
  VoxelArtistView}.tsx` are the stage views a gallery view imitates.
- **Sheet goldens** live in `crates/ludion-testkit/tests/gallery.rs` (`idol_sheet_test:72`,
  `creature_sheet_test:91`) → `goldens/<os>/<name>-sheet.png`.
- **The wire is unchanged by this plan.** 29 methods, schema hash `73b28b8a495cd4b5`
  (`crates/ludion-protocol/tests/schema_snapshot.rs:22`), every command synchronous, no job model;
  `crates/ludion-mcp` derives one MCP tool per method.
- **No HTTP client exists in the workspace.** No `reqwest`, no `ureq`, no `hyper`; `tokio` only in
  `ludion-server`; `tungstenite 0.24` is a workspace dependency (`Cargo.toml:34`) used **synchronously**
  by `ludion-client`. Process spawning is already normal host-side (`ludion dev`, and the Tauri host at
  `editor/app/src-tauri/src/stage.rs`). The `--json` precedent is `ludion image-stats --json`
  (`crates/ludion-cli/src/image_stats.rs:115`, `main.rs:758`): one object on stdout, diagnostics on stderr.
- **Codes.** `crates/ludion-core/src/codes.rs`: the highest in the `L11xx` band is `L1160` (the sculpt
  bone tree); `L1201`–`L1204` is the import band. `L1161`–`L1172` is free (Reserved codes, below).
- **Ignore files.** `.gitignore` and `.loreignore` both carry the noise rules; the delta between them is
  the privacy boundary, so a new ignored scratch folder goes in **both** (decision 13).

## Decisions

1. **The committed form is a recipe plus an explicit accept; a run is never an engine input.** The
   source of truth is a `kind: generation` **recipe** — text, diffable, iterated like any other source
   — and a separate `ludion gen accept` freezes one chosen candidate into the project as an **ordinary
   asset** (a `.mesh.yaml`, a `.png`) carrying a provenance block. **Nothing in a scene ever points at
   a recipe, a run folder or a candidate index**; the engine never learns ComfyUI exists. Because
   diffusion is not reproducible — the same graph, seed and checkpoint can diverge across driver and
   library versions — **regeneration is never a gate**: `generated_from:` and the `generate-map` /
   `generate-atlas` drift rules do **not** apply to accepted assets. **Rejected:** a `Mesh.shape:
   generated` variant resolving through a live server (a level that fails to load because a GPU is
   busy); a committed candidate corpus (megabytes of binary per iteration).
2. **ComfyUI is a step of the existing modelling loop, not a parallel pipeline.** A generated mesh lands
   in one of exactly two places, both already existing: **(i)** a textured `kind: mesh` document through
   `ludion import` — built first, and the reason `Plan/textured-pbr.md`'s trigger fires; **(ii)** a
   **sculpt part** (`shape: mesh`, Wave 3), so a bought form composes with authored form under the same
   blends, mirrors, deformers and materials. **Mesh density is handled inside the Comfy graph** by its
   core `DecimateMesh` / `RemeshMesh` nodes, already better than anything we would write and already
   sitting where the mesh is. Ludion builds **no decimator** here; the sculpt crate's quadric decimator
   expects a closed manifold and is not reused on generator output. **Rejected:** a third asset lane
   bypassing `import` and the mesh document.
3. **Ludion connects to an already-running ComfyUI server and never spawns Python.** The address
   resolves from a `--url` flag, else `LUDION_COMFY_URL`, else ComfyUI's own default of the loopback
   address on port 8188 (a product fact, not a machine fact), **never from a committed file** — a per-machine
   property exactly like `LUDION_THREADS`, so it belongs in the environment. A missing or unreachable
   server is **one** coded diagnostic (`L1167`) naming all three sources and which one supplied the address. Because the server may be on another
   machine, **inputs go up through `POST /upload/image` and outputs come back through `GET /view`**: no
   code path assumes a shared filesystem, and **no artifact of this arc — committed or scratch — ever
   records the server's address** (provenance records the ComfyUI version and checkpoint filenames
   instead). **Rejected:** managing a Python process (a second runtime to supervise, install-shaped
   failure modes, and the user already runs the server).
4. **The textured-PBR trigger has fired (2026-09-17).** A `SaveGLB` output *is* "the first external
   textured asset worth importing with its texture maps intact": base-color, metallic-roughness, normal
   and occlusion baked in, UVs authored. That plan **runs first, as its own arc, import-only exactly as
   written** — this one adds no scope to it, and in particular grows no texture-authoring surface (its
   decision 1 stands: textures arrive only through `ludion import`, and `gen accept` for a mesh *is* an
   `import`). Wave 2 is gated on it.
5. **CLI-first: `ludion gen`. No wire methods, no schema-hash movement, no engine HTTP client.** The
   engine gains no async job model, no generation command family, no dependency on a network service.
   The protocol stays 29 methods at hash `73b28b8a495cd4b5`, and `crates/ludion-mcp` — one tool per
   method — remains the engine's agent surface unchanged. Agents drive generation through the shell,
   exactly as they drive `import`, `generate-map` and `generate-atlas`. **Rejected:** `generation.*`
   wire methods (the first long-running, non-deterministic, externally-failing commands on a wire whose
   every command is synchronous and deterministic); an MCP server of our own wrapping ComfyUI (a second
   engine beside the wire — the `Plan/mesh-modeling.md` decision-20 argument).
6. **The gallery is a CLI contact sheet first.** `ludion gen sheet` composes one PNG grid of a run's
   candidates, each cell labelled with its index and seed — the instrument `render --model`'s six-view
   sheet already is, readable by an agent in one image. The **editor** gallery view is the last wave,
   built only if the CLI sheet proves insufficient. **Rejected:** building the editor view first (the
   human is the judge, but the agent is the iterator, and the agent's loop must close without a GUI).
7. **Three recorded checkpoints, any of which may end the arc** — the `Plan/mesh-modeling.md`
   decision-18 shape: **(1)** the image loop, a sheet of candidates judged by the user; **(2)** the
   textured idol beside the sculpt idol on the model stage, judged on quality **and** on render/token
   count against the sculpt baseline (idol: 10 renders, ~191 k tokens); **(3)** the sculpt-part loop on
   the creature. A checkpoint that fails is written into this Status with its evidence, and the arc
   stops there.
8. **Driver: `games/vigil`.** The first prop is **the idol, regenerated from a text prompt matching the
   sculpt brief**, judged on the same stage in the same views against the authored one — the cleanest
   possible A/B: identical subject, identical framing, two authoring paths, one already logged. The
   external shmup does not consume this arc.
9. **One recipe kind, `kind: generation`; what it produces is read, never declared.** The output kind
   (image or mesh) comes from the graph's **output nodes** — `SaveImage` and friends mean images,
   `SaveGLB` means a mesh — and is reported by `gen check`. A recipe never carries a `produces:` key: a
   declared kind is a second source of truth that can disagree with the graph, and the graph is the one
   that runs. No output node, or both kinds in one graph, is `L1171`.
10. **Standalone text-to-image and image-to-image recipes are first-class, not scaffolding.** Every mesh
    path in core ComfyUI is image-to-mesh, so the image is the real artifact to iterate: get the idol
    *drawing* right over many cheap seeds, then spend one expensive mesh run. A recipe therefore names
    only **committed** images (`images:`), `gen accept` works for images exactly as for meshes, and a
    `--image <path>` override on `gen run` chains a **scratch** candidate from a previous run straight
    into a mesh run without accepting it — so the common loop (sheet → pick → mesh) costs no commit.
    Accepting the intermediate image is a choice, not a requirement.
11. **Workflows are committed API-format JSON, verbatim; overrides are keyed by node title.** The API
    format is what `/prompt` accepts, so it is what we commit, under the project, **never hand-edited by
    Ludion** — the recipe names the file and overrides leaf inputs into it. Overrides key on
    **`_meta.title`**, not node id: a title survives a re-export from the UI, a numeric id need not, and
    a title is what a human typed to say what the node is *for*. An absent or ambiguous title is
    `L1166`, listing every title in the graph. A UI/litegraph graph is refused with `L1163` naming the
    **File → Export (API)** menu item — the likeliest first-run mistake, worth its own code.
    **Authoring is not ours alone:** `ludion gen nodes [filter]` prints a compact typed listing from the
    live server's `/object_info` so an agent can write or repair a graph without a browser, and
    `gen check` validates a recipe's graph against that same schema — unknown node, missing required
    input, type mismatch, ambiguous title — **before any submit**, so the failure is a local diagnostic
    with a JSON pointer rather than a 400 from a machine.
12. **`gen run` blocks by default.** Submit `POST /prompt`, follow progress on `/ws`, read the result
    from `GET /history/{id}`, download every output through `/view` into the run folder, print a report.
    `--timeout <s>` bounds it (`L1170`). `--no-wait` prints the job id and returns, with `gen status`
    and `gen fetch` as the other half — the submit/poll split `profile.capture` already has. `--count N`
    submits N jobs at `seed`, `seed+1`, … onto one labelled sheet. Blocking is the right default because
    the caller is a loop that wants the answer, and an agent that must poll burns a turn.
13. **Runs are ignored scratch; recipes and workflows are committed; accepted assets carry provenance.**
    `generation/runs/<recipe>/<run-id>/` goes into **both** `.gitignore` and `.loreignore` (noise, not a
    private path — the delta between those files is reserved for privacy). Committed:
    `generation/recipes/*.gen.yaml`, `generation/workflows/*.json`. Accepted assets go to the
    **existing** asset folders — meshes to the project's mesh library, images to its assets — with a
    provenance block recording the recipe snapshot with resolved overrides, the workflow file hash, the
    seed, the prompt text, the ComfyUI version, the model checkpoint filenames, the run id and the date.
    It is a **record of origin**, not a reproduction recipe (decision 1).
14. **The text-to-image model is the provisioning agent's pick, with the user's veto.** One freely
    downloadable model that fits a 24 GB card and is **never resident together with the Trellis2 stack**
    (the two must not compete for one card in one graph), recorded in the private kit's README under
    `infra/comfyui/`. Model and checkpoint **names** are public facts — a committed workflow's loader nodes name
    them, and provenance records them; the private facts are download locations, licences' local copies and
    machine paths, which never appear in this repo's public tree.
15. **The parity-gate ruling: pipeline verbs are CLI commands, and that satisfies Pillar 2.**
    `CLAUDE.md`'s non-negotiable — "nothing ships in the editor unless it already exists as a protocol
    command" — is satisfied for the **pipeline** family (`import`, `generate-map`, `generate-atlas`, and
    now `gen`) by their being CLI commands the Tauri host invokes as a subprocess with **structured JSON
    output** (`--json`): those verbs are batch producers of committed files, not live world manipulation,
    and the host already spawns processes (`editor/app/src-tauri/src/stage.rs`). The rule's purpose — no
    editor-private capability — is met: every one is available to any client with a shell. Wave 4 rests
    on this ruling, and **one line is added to `CLAUDE.md`'s non-negotiables when that wave lands**, not
    before.
16. **A generation run and `node scripts/check.mjs` never overlap on the reference box.** That box's GPU
    blesses the Windows goldens, and GPU contention there is already the known cause of device-lost
    failures in the test lanes. A generation run is a multi-minute, full-VRAM job. The two are
    serialized by hand — a rule in the docs and in this plan, obeyed by whoever starts either one. No
    scheduler, no lock file.
17. **`shape: mesh` is the sculpt part that eats an imported mesh** (Wave 3). It names an imported
    `.mesh.yaml`; the mesher computes the signed distance from the triangle soup at load —
    closest-triangle distance for magnitude, **generalised winding number** for sign, so an open,
    self-intersecting or otherwise messy generator output still gets a usable inside/outside — cached
    per distinct path exactly like the existing `shape: voxel` transform through `SculptSources`,
    deterministic, with **no derived file committed**. Every common part key applies: `blend`, `op`,
    `mirror`, `repeat`, `material`, `bone`, `deform`, `noise`. Its exit demo is the **Pixal3D multi-view
    loop**: sculpt a rough form, `render --model` its front / left / back / right views, feed them to
    `Pixal3DMultiViewConditioning`, bring the result back as a part — the authored silhouette
    controlling the generated surface, the one workflow that makes a diffusion mesh *directable*.
18. **Order.** Provisioning now (the private kit, decision 14). **Wave 1** (the CLI core with image
    recipes) starts now, in parallel with the sculpt arc's checkpoint-2 sitting — it shares no file with
    that arc. The **textured-PBR arc** runs before **Wave 2** (mesh recipes, textured accept through
    `import`, checkpoint 2). **Wave 3** (`shape: mesh` + the multi-view loop, checkpoint 3) follows.
    **Wave 4** (the editor gallery view, decision 15) is last and optional.
19. **Shape of the code.** A new crate **`ludion-gen`** holds the recipe's consumers, the Comfy client
    and the sheet composition, with the CLI thin over it — the `ludion-sprite` / `ludion-sculpt`
    precedent. It is a **host-side** crate: no determinism denylist, it may read a wall clock (a run
    id), and nothing it produces enters a `WorldHash`. Dependencies: **`ureq`** (small, synchronous, no
    async runtime — the workspace has no HTTP client and must not acquire tokio outside `ludion-server`)
    and the workspace's existing **`tungstenite 0.24`** used synchronously, as `ludion-client` uses it.
    Tests run against a **mock HTTP/WS server** with canned `/object_info`, `/history` and `/view`
    fixtures, in CI; the **live smoke is a manual lane**, never in hosted CI. Docs: `docs/generation.md`
    (format + verbs), `docs/generation-craft.md` (the authoring playbook grown from the runs — the
    `sculpt-craft` pattern), `dev-docs/ludion-gen.md`.

## The recipe document

Canonical key order (what `emit_generation` writes, block style like every other kind; maps sorted by
key on emit, as the voxel palette is): `format`, `kind`, `name`, `workflow`, `seed`, `count`, `images`,
`inputs`.

```yaml
format: 1
kind: generation
name: idol-portrait
workflow: ../workflows/sdxl-txt2img.json    # relative to THIS document's directory
seed: 7                                     # integer >= 0; the base seed of a --count run
count: 4                                    # optional, default 1
images:                                     # named committed images, uploaded before submit
  subject: ../../sculpts/renders/idol-front.png
inputs:                                     # node title -> input name -> value
  Empty Latent:
    width: 1024
    height: 1024
  Load Subject:
    image:                                  # the one structured form: a reference into `images:`
      image: subject
  Positive Prompt:
    text: "a weathered stone idol on a low plinth, moss in the carving, one emissive green eye, neutral grey background"
```

Block style throughout: the strict YAML subset refuses flow collections (`L1005`) and block scalars
(`L1006`), so the image reference is a nested map and a long prompt is one double-quoted string.

**Keys.** `format` and `kind` required as everywhere. `name` optional (defaults to the file stem; it
names the run folder). `workflow` **required**, a relative path resolved against the recipe's own
directory — the sculpt `shape: voxel` rule, and what lets a recipe folder move as a unit. `seed`
optional (default `0`), integer `>= 0`. `count` optional (default `1`), `>= 1`. `images` optional, a map
of name → relative path to a **committed** image, each uploaded through `POST /upload/image` before
submit with the returned server-side name substituted. `inputs` optional, a map of **node title** →
input name → value.

**Values.** A scalar (string, integer, number, boolean — an integer keeps its integer spelling, since a
ComfyUI `INT` input refuses `1024.0`) is passed verbatim into the node's `inputs`. The **one**
structured form is `{ image: <name> }`, resolving to the uploaded name of `images.<name>`; any other map
or a list is `L1162`, because a graph edge is the workflow's business, not the recipe's. A recipe never
rewires the graph; it sets leaf inputs.

**Seeds.** The base `seed` is written into every input named `seed` or `noise_seed` on nodes the recipe
does not override explicitly; an explicit override wins. Run `i` of a `--count N` submit uses `seed + i`,
so a candidate's index and its seed are the same fact — what the sheet labels and what `gen accept`
records.

**Validation.** `L1161` is *structure* (missing or wrong-typed keys, an unknown key with its nearest
match — the `mesh.rs` / `voxel.rs` split); `L1162` is *data consistency* (finite numbers, `count >= 1`,
`seed >= 0`, every `images` path existing and readable, the `workflow` path existing, an `{ image: x }`
naming a declared entry). The **workflow** passes are separate codes because they concern a different
file: `L1163` (not API format), `L1164` (unknown node class), `L1165` (a required input missing or a
type mismatch), `L1166` (an override title absent or ambiguous), `L1171` (no output node, or images and
a mesh in one graph). `L1164` and `L1165` need the live `/object_info` and are the **live** half of
`gen check`; the rest are answerable offline from the graph alone.

**`kind: generation` is registered in `ENGINE_KINDS`** (`crates/ludion-engine/src/docs.rs`) so the
engine's data-doc walk skips it rather than reporting `L1119` under a content root — the `sculpt`
precedent. No loader, component or `MeshSource` variant reads it: registration is the *only* engine-side
fact about this kind.

## The run folder

```
generation/runs/<recipe-name>/<run-id>/
  run.json          the report (below)
  inputs/           every uploaded source image, copied verbatim
  0000.png          candidate 0 (or 0000.glb for a mesh graph)
  0001.png
  sheet.png         written by `gen sheet`
```

`<run-id>` is `<UTC timestamp, compact>-<8 hex of the resolved graph hash>` — sortable, collision-free
across recipes, its hash half saying at a glance whether two runs submitted the same graph. The whole
tree is ignored by both VCSs (decision 13).

`run.json` has a stable key order, the canonical-writer discipline, so a diff between two runs reads:

```json
{
  "run_id": "...", "recipe": "generation/recipes/idol-portrait.gen.yaml",
  "recipe_hash": "...", "workflow": "generation/workflows/sdxl-txt2img.json",
  "workflow_hash": "...", "graph_hash": "...",
  "seed_base": 7, "count": 4, "kind": "image",
  "comfy_version": "...", "checkpoints": ["..."], "prompt_ids": ["..."],
  "outputs": [{ "index": 0, "seed": 7, "file": "0000.png", "node": "9", "prompt_id": "..." }],
  "started": "...", "finished": "...", "duration_ms": 123456, "status": "ok"
}
```

`comfy_version` and `checkpoints` come from `GET /system_stats` and the resolved graph's
checkpoint-loader inputs. **The server address is not a field** (decision 3).

## Provenance on accepted assets

One struct, two carriers, one writer:

- **`.mesh.yaml` gains an optional `origin:` map**, emitted immediately after `name` — at the top of the
  document where a diff shows it, before the bulk float lists. `mesh.rs`'s key set grows by one, emitted
  **only when present**, so every existing mesh document stays byte-identical (`Plan/textured-pbr.md`
  decision 3's "widen once, default-fill" posture applied to text).
- **An accepted image gets a sidecar `<name>.png.origin.yaml`**, a `kind: origin` document whose body is
  the same map. A PNG has no text place for it, and a tEXt/eXIf chunk is exactly what
  `Plan/textured-pbr.md` decision 4 strips to keep imported PNG bytes canonical and dedup-sound — so
  writing provenance *into* the image would either break that rule or be silently deleted by the next
  import. The sidecar kind is registered in `ENGINE_KINDS` like the recipe kind.

```yaml
origin:
  tool: comfyui
  comfy_version: "..."
  recipe: generation/recipes/idol-portrait.gen.yaml
  run_id: "..."
  date: 2026-09-17
  seed: 9
  workflow: generation/workflows/sdxl-txt2img.json
  workflow_hash: "..."
  checkpoints: ["..."]
  prompt: "a weathered stone idol on a low plinth, moss in the carving, ..."
  inputs:                       # the recipe's resolved overrides, snapshotted
    Empty Latent: { width: 1024, height: 1024 }
```

`prompt` is the text of every input the resolved graph feeds to a positive-conditioning encoder, joined
in node order — the one field a human actually reads a year later. An asset carrying `origin:` is
otherwise **entirely ordinary**: the loader, the renderer, `check-scene` and every golden treat it as
any other mesh or image, and it is hand-editable after acceptance (it is no longer generated output; it
is content).

## The verbs

Every verb takes `--url <u>` (overriding `LUDION_COMFY_URL`) and `--json`. **`--json` prints exactly one
JSON object on stdout**, diagnostics to stderr as always (the `image-stats --json` precedent) — this is
what decision 15 rests on, so each verb's object schema is snapshot-tested, not ad hoc.

| verb | does |
|---|---|
| `gen nodes [filter] [--class C]` | compact typed listing from `/object_info`: class, category, required inputs with types and defaults, outputs. `filter` is a case-insensitive substring over class and category. |
| `gen check <recipe>` | the offline passes (document, workflow format, titles, output kind), then the live pass against `/object_info`. Prints the produced kind and the resolved override table. An unreachable server degrades the live pass to an `L1167` warning after the offline result; the exit code is the offline result's, so CI can run the offline half. |
| `gen run <recipe> [--count N] [--seed S] [--image P] [--timeout S] [--no-wait]` | check, upload `images:` (and `--image`, a scratch path overriding the *first* declared image), submit, follow `/ws`, download, write `run.json`, print the report. `--no-wait` prints the job ids and returns. |
| `gen status [<run-id>]` | the run's state from `/history/{id}` and `/queue`; bare, the queue plus the most recent run of every recipe. |
| `gen fetch <run-id>` | download a finished `--no-wait` run's outputs and complete its `run.json`. |
| `gen sheet <run-id> [--cols N] [--out P]` | compose the candidate grid, one cell per output labelled `#<index> seed <n>`, cells scaled to a common size, letterboxed, deterministic layout. Default out is the run folder's `sheet.png`. |
| `gen accept <run-id> <index> --out <path>` | freeze one candidate: an image is re-encoded to the canonical normalized PNG and written with its `.origin.yaml` sidecar; a `.glb` is handed to `ludion import` (`--out` a directory) and the resulting `.mesh.yaml` gains its `origin:` map. Never overwrites (`L1172`). |

**Not built** (the "one obvious way" rule): a `gen new` scaffold — a recipe is eight lines and `gen
check` is the loop's entry; it is added only if authoring one from `docs/generation.md` proves
unreliable in Wave 1's transcript. No `gen delete`, no `gen ls` (a run folder is a folder).

## The client (`ludion-gen`)

Modules: `recipe.rs` (resolution of paths, seeds and overrides into a concrete graph), `workflow.rs`
(API-format parse, title index, the `L1163`–`L1166`/`L1171` passes, `/object_info` validation),
`client.rs` (`ureq` over the six endpoints plus the synchronous `tungstenite` progress stream), `run.rs`
(submit → follow → download → `run.json`), `sheet.rs` (grid composition and labelling), `accept.rs` (the
provenance writer and the two carriers), `lib.rs`.

**Failure is the product here**, so each mode is its own code: unreachable (`L1167`); an HTTP error or a
`/prompt` 400 with `node_errors` reprinted structurally (`L1168`); an `execution_error` frame or an
error status in history, naming the node and its class (`L1169`); `--timeout` elapsed with the job id
and the `gen status` / `gen fetch` recovery line (`L1170`).

**The mock-server test strategy.** A `tests/mock.rs` harness binds a `std::net::TcpListener` on port `0`
in a thread and serves fixture files: `/object_info` (a trimmed but real capture, committed), `/prompt`,
`/history/{id}`, `/view`, `/upload/image`, and a `/ws` handshake replaying a canned frame sequence. One
case for the happy path end to end (submit → two progress frames → executed → history → two downloads →
`run.json`), then **one case per code `L1163`–`L1172`**, each asserting the code and its JSON pointer or
named subject. The **live smoke** is an `#[ignore]`d test plus a documented manual command, gated on
`LUDION_COMFY_URL` being set, never in hosted CI (no GPU there, and decision 16 forbids it on the
reference box during a check run).

## Waves

### Wave 0 — provisioning (private, in flight)

The kit under `infra/comfyui/` (private, decision 14): the checkout, the model set, the launch recipe,
and the README recording the text-to-image pick with the user's veto and the rule that it is never
resident with the Trellis2 stack. **Nothing about it enters this repo's public tree** — this plan's only
public statement about the machine is `LUDION_COMFY_URL`.

### Wave 1 — the recipe, the client, the image loop (checkpoint 1)

1. **Codes.** `crates/ludion-core/src/codes.rs`: `L1161`–`L1172` per the Reserved section, each with the
   documented what/where/why/fix paragraph the table's entries carry; `ludion explain L1161` … `L1172`
   must print (the `explain` smoke step).
2. **`crates/ludion-scene/src/generation.rs`** (the `mesh.rs` sibling — the smaller shape).
   `GenerationDocument { name: Option<String>, workflow: String, seed: u64, count: u32, images:
   Vec<(String, String)>, inputs: Vec<(String, Vec<(String, InputValue)>)> }` with
   `InputValue::{Scalar(..), ImageRef(String)}`; `parse_generation` (`L1161`), `validate_generation`
   (`L1162`), `check_generation`, `emit_generation` (canonical order, ryu floats, byte-idempotent:
   `emit(parse(emit(doc))) == emit(doc)` tested). Re-export from `lib.rs`; register `generation` and
   `origin` in `crates/ludion-engine/src/docs.rs::ENGINE_KINDS`. The `origin:` map is one struct here
   (`Origin`) with its emitter, used by both carriers.
3. **`crates/ludion-gen`** (new; copy `ludion-sculpt`'s `Cargo.toml` metadata; **no** determinism clippy
   denylist — decision 19 — and say why in the crate docs; `#![forbid(unsafe_code)]`,
   `#![warn(missing_docs)]`; deps `ludion-core`, `ludion-scene`, `ludion-assets` (content hash and the
   canonical PNG re-encode), `ureq`, `tungstenite`). The modules of *The client*, above.
4. **`.mesh.yaml` gains `origin:`** in `crates/ludion-scene/src/mesh.rs` — the eleventh key, emitted only
   when present, the round-trip and canonical-golden tests extended, every existing mesh document
   asserted byte-identical.
5. **CLI.** `crates/ludion-cli/src/gen.rs` plus the `Command::Gen { command: GenCommand::{Nodes, Check,
   Run, Status, Fetch, Sheet, Accept} }` arm in `main.rs`, thin over `ludion-gen`. `--json` on every
   subcommand, one snapshot test per object shape.
6. **Ignore rules.** `generation/runs/` added to **both** `.gitignore` and `.loreignore`, with the
   comment both files' conventions require (noise, present in both — the delta is reserved for privacy).
7. **Content.** `generation/workflows/txt2img.json` (authored by this wave as API JSON, titles chosen for
   override: `Positive Prompt`, `Negative Prompt`, `Empty Latent`, `Sampler`, `Checkpoint`) and
   `generation/recipes/idol-portrait.gen.yaml` — the idol's sculpt brief re-expressed as a prompt
   (decision 8). Iterate: `gen check` → `gen run --count 8` → `gen sheet` → read → edit the recipe.
   **Every sheet and every token count is logged** (Artifacts).
8. **Tests.** The mock-server suite; the document's per-code tests with their JSON pointers; `sheet.rs`'s
   layout as a pure function (a fixed set of candidate sizes → a committed cell geometry, no GPU, no
   golden image); the `emit` round trip; `accept` writing a sidecar and an `origin:` map byte-exactly.
9. **Docs** (the `ludion-docs` skill; `node scripts/check.mjs --fix` re-mirrors `embedded-docs/`). New:
   `docs/generation.md` (the recipe, the verbs, `LUDION_COMFY_URL`, the run folder, provenance,
   **decision 16's serialization rule**), `docs/generation-craft.md` (started from this wave's
   transcript: which prompts and workflows produced usable candidates, which did not),
   `dev-docs/ludion-gen.md` (layout, the endpoint map, the failure taxonomy, the mock harness). Rows:
   `docs/project-anatomy.md` (the `generation/` folder), `docs/scene-format.md` (the kind list),
   `dev-docs/scene-kind-documents.md`, `dev-docs/engine-content-kinds.md`, `dev-docs/crate-graph.md`,
   `dev-docs/ludion-cli.md`. Root `CLAUDE.md`: the CLI list gains `gen`; the Current-state paragraph at
   wave close.
10. **Verify.** `node scripts/check.mjs --rust-only`, then the full suite (decision 16: **not**
    concurrently with a generation run on the reference box); `cargo test -p ludion-gen`; the live smoke
    by hand.
11. **Exit (checkpoint 1).** The sitting: a sheet of idol candidates on screen, the user's verdict on
    whether the image half is worth the machine. Verdict appended to Status as a decision; the log's
    first rows complete; the notebook pruned. **A "no" here ends the arc before any mesh work.**

### Wave 2 — meshes through `import` (checkpoint 2)

**Gated on `Plan/textured-pbr.md` closing.** Until it does, an accepted `.glb` loses its maps to `L1202`
or `--allow-lossy`, which is the whole point of the trigger.

1. **Mesh output support** in `run.rs`: the `outputs[node]["3d"]` entry shape beside `["images"]`, the
   `.glb` download, `kind: "mesh"` in `run.json`, `L1171` for a graph mixing the two.
2. **`gen accept` for a mesh**: hand the downloaded `.glb` to `ludion import --out <dir>` in-process (the
   import crate, not a subprocess), then write the `origin:` map into the produced `meshes/*.mesh.yaml`.
   Report the imported vertex count and let `L1112` speak if it is over budget — the signal that the
   graph's `DecimateMesh` target needs lowering (decision 2: density is fixed in the graph, never here).
3. **Content.** `generation/workflows/trellis2-img2mesh.json` — authored as API JSON from the core node
   set (`Trellis2Conditioning` → shape → texture → upsample → `VaeDecodeShapeTrellis` → `DecimateMesh` →
   `SaveGLB`), titles chosen for override; `generation/recipes/idol-mesh.gen.yaml` chaining Wave 1's
   chosen candidate by `--image` (decision 10). The accepted mesh lands in vigil's mesh library and goes
   on the model stage beside the sculpt idol.
4. **Goldens.** `idol_generated_sheet_test()` in `crates/ludion-testkit/tests/gallery.rs` →
   `goldens/<os>/idol-generated-sheet.png`, first-blessed on the pinned GPUs (owed, not done in the wave
   — the particles precedent: it reports `L2201` until blessed).
5. **Docs.** `docs/generation.md` (the mesh half, the density rule), `docs/generation-craft.md` (the
   image→mesh findings), the import topic's row for `origin:`.
6. **Exit (checkpoint 2).** The sitting: the textured generated idol beside the authored sculpt idol on
   the model stage, same views, same lighting. Judged on quality **and** on the count — renders and
   tokens for this idol against the sculpt idol's 10 renders and ~191 k tokens
   (`Plan/artifacts/mesh-modeling-log.md`). Verdict as a decision. The two honest failure verdicts to be
   willing to write: "the generated mesh is worse and costs more" and "it is better but not directable".

### Wave 3 — `shape: mesh` sculpt parts and the multi-view loop (checkpoint 3)

1. **The part.** `crates/ludion-scene/src/sculpt.rs`: `SculptShape::Mesh { path, size? }` beside `Voxel`,
   validated by the existing `L1157`; `crates/ludion-sculpt`: a triangle-soup distance field —
   closest-triangle distance for magnitude, **generalised winding number** for sign (decision 17) —
   built once per distinct path and `Arc`-shared by every copy, exactly like `edt.rs`'s transform.
   `SculptSources` gains the mesh documents; `ludion_engine::read_sculpt_sources`
   (`sculpt_mesh.rs:63`) reads them, so the loader, `check-scene` and the model sheet follow with no new
   reader. A missing source is `L1101`, an unparsable one `L1113`, both at `/parts/<i>/path` — the
   `shape: voxel` precedent exactly.
2. **Determinism.** The field is a pure function of the mesh document's bytes: fixed triangle order, ties
   broken on `(distance, triangle index)`, `f32` through `ludion_core::math`. A conformance hash in
   `crates/ludion-sculpt/tests/` over a fixture, verified on **both** pinned boxes before any bless — a
   disagreement is a bug in the field, never a per-OS constant (the `Plan/mesh-modeling.md` Wave 1 step 3
   rule).
3. **The `--set`/`--sweep` domain and the editor's slider table** gain the new part's numeric fields, as
   Wave 3 of the sculpt arc did for its own.
4. **The demo (the exit).** Sculpt a rough creature form; `render --model` its front / left / back /
   right views; a `generation/workflows/pixal3d-multiview.json` recipe binding those four as `images:`;
   accept the result; bring it back as a `shape: mesh` part inside the sculpt document with a `blend`
   onto the authored body. That round trip is the thing this arc is for.
5. **Docs.** `docs/sculpt-format.md` (the part), `docs/sculpt-craft.md` (the multi-view convention: which
   four views, what framing, what the conditioning wants), `dev-docs/ludion-sculpt.md` (the
   winding-number field and its cost), `docs/generation-craft.md` (the loop).
6. **Exit (checkpoint 3).** The sitting: the creature carrying a generated part that reads as one body.
   Verdict as a decision. If the generated surface cannot be blended into authored form, that conclusion
   is written into Status with its evidence and the arc ends at Wave 2's verdict.

### Wave 4 — the editor gallery view (optional, under decision 15)

1. **The view.** `editor/app/src/views/GenerationView.tsx`: an `AssetBrowser` over `generation/recipes`
   with `ext: '.gen.yaml'` (the component already takes `root` and `ext` as props), a recipe editor (the
   `SculptView` slider/splice pattern for numerics, a textarea for prompts), a **Generate** button, and a
   candidate grid reading the run folder. The Tauri host invokes `ludion gen … --json` as a subprocess
   and parses the object (decision 15) — the shape `stage.rs` already uses to spawn.
2. **Accept from the grid**: a click runs `gen accept`, and the asset appears in the Models
   view's browser on the next watcher tick.
3. **`CLAUDE.md`**: the one line added to the non-negotiables recording decision 15's ruling. **This is
   the only edit this arc makes to that section, and it lands with this wave, not before.**
4. **Docs.** `Plan/editor-views.md` gains the view's row; the editor topic names it.
5. **Exit.** An editor sitting: pick a recipe, type a prompt, generate, accept, see it on the stage.

## Engine/game boundary

Everything above is tooling. The engine gains exactly three facts: two kind names in `ENGINE_KINDS` (so
the data-doc walk skips them), one optional `origin:` key on `kind: mesh`, and — in Wave 3 — one sculpt
part shape. No component, no wire method, no system, no renderer change. Vigil consumes an accepted
asset exactly as it consumes any other mesh or image.

## Deferred / non-goals

Text-to-mesh (core ComfyUI has none; if one lands it is a workflow, not a plan). Training, LoRAs,
fine-tuning. A second generation backend or a backend abstraction (one server, named by one variable).
Animation or rigging of a generated mesh (`Plan/mesh-modeling.md` Wave 4 owns skinning; a generated mesh
becomes a sculpt part and inherits it). Texture *generation* onto existing UVs (a different trigger: it
needs the UV lane and a bake step). Video and audio nodes. A queue manager, a job database or any
scheduling (decision 16 is a rule, not a mechanism). Batch recipes over a directory. `gen new` (above).
Committing candidates. Regenerating an accepted asset as a gate — permanently rejected (decision 1).

## Risks / open questions

- **Non-determinism leaking into a gate.** Containment is structural (decision 1): the only committed
  artifacts are text the human chose and binary the human accepted. The risk is a future convenience
  that reopens it — a `--regenerate` flag, a scene field naming a recipe — so both are named non-goals.
- **API drift.** ComfyUI is fast-moving and unversioned in practice; `/api/jobs` already shadows the
  older `/history` path. The client uses the narrow, oldest-stable six endpoints, the mock fixtures are
  real captures with a dated comment, and `gen check` reports `comfy_version`, so drift shows up as a
  diagnostic rather than as a wrong answer.
- **Titles as the override key.** A re-export preserves titles only if the human did not rename a node.
  `L1166` lists every title in the graph, turning a rename into a one-line fix — but it is the arc's
  likeliest recurring friction, and the craft doc gets a paragraph on naming nodes before exporting.
- **GPU contention on the reference box** (decision 16) — a rule obeyed by hand, with the known failure
  mode (device-lost in the GPU test lanes) recorded so a red run is diagnosed in one step.
- **Mesh quality versus directability.** Checkpoint 2 may conclude that a generated idol looks better and
  takes worse direction than an authored one. Wave 3's multi-view loop is the designed answer — but if
  checkpoint 2 says "worse and dearer", Wave 3 does not run.
- **Vertex budget.** A Trellis2 mesh is hundreds of thousands of triangles before `DecimateMesh`;
  `MESH_VERTEX_BUDGET` is 10 000 and `L1112` is a warning, not a wall. The knob is in the graph
  (decision 2); the risk is an author reaching for `budget_exempt`, which the craft doc names as the
  wrong move.
- **`ureq` as a new dependency** — the workspace's first HTTP client, and its first TLS-capable one if a
  remote server is ever reached over TLS. Kept to `ludion-gen`, which nothing in the engine depends on;
  the `--full` lane's crates.io packaging step checks it did not leak into the facade.

## Reserved diagnostic codes

`L1161`–`L1172`. **Evidence:** the highest code in the `L11xx` band is `L1160` (the sculpt bone tree,
`Plan/mesh-modeling.md`); `L1201`–`L1204` is the separate import band (`Plan/phase-15-data-docs.md`
records `L1204+` as that band's free line); no code in `L1161`–`L1189` appears anywhere in the tree, in
`codes.rs` or in any plan's Reserved section.

| code | for |
|---|---|
| `L1161` | **Generation document structure** — missing or wrong-typed key, unrecognized `kind`, an unknown key with its nearest match. |
| `L1162` | **Generation document data** — `count < 1`, a negative or non-integer `seed`, a non-finite number, an `images` or `workflow` path that does not resolve, an `{ image: x }` naming an undeclared entry, a structured value that is not the image-reference form. |
| `L1163` | **Workflow not in API format** — a UI/litegraph export; names the **File → Export (API)** menu item and the expected flat shape. |
| `L1164` | **Unknown node class** — a `class_type` absent from the server's `/object_info`; names the class and the nearest installed match. |
| `L1165` | **Workflow input invalid** — a required input missing, or an override whose value does not match the input's declared type or enum; names node title, input and expected type. |
| `L1166` | **Override title unresolved** — a recipe override names a `_meta.title` that is absent or shared by two nodes; lists every title in the graph. |
| `L1167` | **ComfyUI server unreachable** — nothing answering at the resolved URL; names both `LUDION_COMFY_URL` and `--url`, and never writes the address into a file. |
| `L1168` | **Server rejected the request** — a `/prompt` 400 with its `node_errors` reprinted structurally, or any other endpoint's HTTP error. |
| `L1169` | **Run failed** — an `execution_error` frame or an error status in history; names the node, its class and the server's message. |
| `L1170` | **Run timed out** — `--timeout` elapsed; names the job id and the `gen status` / `gen fetch` recovery. |
| `L1171` | **Output kind unusable** — no output node in the graph, images and a mesh in one graph, or an accept asking for a kind this run did not produce. |
| `L1172` | **Accept refused** — index out of range, destination already exists, run folder missing or incomplete, provenance unwritable. |

## Artifacts

`Plan/artifacts/comfy-generation-log.md` — the convergence log, created with Wave 1's first row, shaped
like `Plan/artifacts/mesh-modeling-log.md` so the two compare directly: `subject | wave | candidates |
sheets read | tokens | wall time | verdict`, plus the provenance columns (authoring model and harness,
source access, human intervention, review fidelity). **Decision 7's comparison metric is read off this
table against that one**: the sculpt idol cost 10 sheet renders and ~191 k tokens, the creature 11 sheets
(16 renders) and ~155 k. A generated idol that costs more than that, or needs more human taste per
attempt, is the arc's answer even if the picture is prettier.

## Docs owed (by wave)

W1: `docs/generation.md`, `docs/generation-craft.md`, `dev-docs/ludion-gen.md` + the six rows in Wave 1
step 9. W2: the mesh half of both `docs/` topics and the import row for `origin:`. W3: the
`sculpt-format` / `sculpt-craft` / `dev-docs/ludion-sculpt` additions and the multi-view section.
W4: `Plan/editor-views.md` and the `CLAUDE.md` non-negotiables line (decision 15).
