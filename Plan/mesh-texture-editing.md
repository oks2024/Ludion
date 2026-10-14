# Plan: Mesh texture editing — deliberate edits to a generated mesh's maps

> Status: **DESIGN SETTLED 2026-09-18 — rulings 1–4 are the user's, and the user agreed the proposed
> decisions and the seven answers under "Settled questions" the same day. Wave 1 is
> mechanized-complete 2026-09-18; checkpoint 1 passed (user, 2026-09-23); Wave 2 (the seeded flood
> fill) is mechanized-complete 2026-09-24; checkpoint 2 passed (user, 2026-09-28); Wave 3 waits on the user's word.** Current-code facts below were read from the tree
> on 2026-09-18 before Wave 1 (file:line pointers are of that date; re-grep before editing).

## Status

**Wave 1 — mechanized-complete 2026-09-18.** Checkpoint 1 **passed** (user, 2026-09-23); Wave 2 started on
the user's word.

**Wave 2 — mechanized-complete 2026-09-24.** Checkpoint 2 **passed** (user, 2026-09-28): the fill is kept.
Wave 3 waits on the user's word.

Wave 1 record:

- **Shipped.** `kind: mesh-edit` (`crates/ludion-scene/src/mesh_edit.rs`), `generated_from:` as the 18th
  `kind: mesh` key (after `name`), `mesh-edit` in `ENGINE_KINDS`, codes `L1173`–`L1177`; the
  `ludion-mesh-edit` crate (region, raster, paint, ray, derive — one bake shared by write and check);
  `ludion mesh pick` and `ludion mesh apply [--check]` with `--json`; `scripts/mesh-edit-drift.mjs` in
  `check.mjs`, `ci.yml` and the private provisioner; `render --model --channel emissive`;
  `check-scene --bounds` on a `kind: mesh` document; `docs/mesh-editing.md` and
  `dev-docs/ludion-mesh-edit.md`. No wire method, no schema-hash movement, no renderer change, no golden
  moved.
- **As built, where it differs from the draft.** The camera is one public entry point,
  `model_camera(view, min, max, cell_w, cell_h) -> ModelCamera`, which the sheet itself is built on;
  `ViewRig::compute` stayed private because it carries the light rig. `pick` casts through the pixel
  **centre** and reads a single-cell sheet (`--views <one view>`); a GPU test pins its silhouette to the
  rendered one to the pixel. `strength` is `[0, 1]`. A UV outside `[0, 1]` is refused (`L1176`) rather
  than wrapped; a map extent is capped at 4096; under `out/` the derive replaces its own
  `mesh.mesh.yaml` and `textures/<16 hex>.png` and refuses anything else. An authored colour is quantised
  to its sRGB byte so the one transfer function in `ludion-assets` serves. `--channel emissive` also
  turns bloom, tonemap, exposure and dither off and drops the caption, and gives the staged mesh unit
  intensity only when its document references an emissive map. `check-scene` has no `--json`, so the
  mesh `--bounds` is text like the sculpt one.
- **The content.** `games/vigil/scenes/meshes/idol-generated/meshes/idol-lit.mesh-edit.yaml` — a capsule
  along the eye plus a sphere at its receding outer corner, 400 texels of a created 1024² emissive map —
  and its derived tree, authored blind in 4 applies, 44 picks and 14 images read. The report's box sits
  within 0.002 of the picked eye on every axis; the emissive channel shows the blob in `back` and `left`
  and black in the other four views. `gen accept`'s `scene.yaml` is untouched; the lit placement is its
  sibling `scene-lit.yaml` at `Mesh.emissive: 3.0`, chosen from a sweep. The subject faces `+Z`, so the
  eye is in the `back` view. Log, the author's read and the ranked tool findings:
  `Plan/artifacts/mesh-texture-editing-log.md`.
- **Owed.** Checkpoint 1: the lit idol beside the pristine one on the model stage
  (`scene-lit.yaml` / `scene.yaml`) — does the eye read, is the glow the right size and colour, was the
  loop worth the machine; what the author could not judge blind is whether it reads as an eye rather than
  a decal, the green against the stone, and the mip fade at distance. The second pinned platform must
  reproduce `PLATE_EMISSIVE_HASH` (`crates/ludion-mesh-edit/tests/derive.rs`) — the exit criterion's
  cross-platform half. Top tool findings, unbuilt: a multi-pixel or scan form of `pick`; the report's
  box projected into a view; a surface-following region (evidence for Wave 2); an intensity for the
  ordinary sheet of a mesh with an emissive map; one unexplained dim pixel on the brow in the emissive
  channel (a coarse-mip sample at a silhouette, most likely).

Wave 2 record:

- **Shipped** (four subtasks, `lore history` revisions 840–844). The `key: { map, color, tolerance }` region
  filter and the polyline `capsule { points, radius }` replacing `from`/`to` (idol-lit migrated, its derived
  PNG byte-identical; the old spelling is `L1173` with the rewrite in its message); `region: fill`, the
  seeded flood fill over welded surface adjacency (`crates/ludion-mesh-edit/src/fill.rs`); `L1178`;
  `mesh apply --view <name> [--width --height]` printing each op's painted box in that view's pixels, through
  `ludion::math::world_to_pixel`, the inverse of `camera_ray`; the plain `render --model` sheet giving unit
  intensity to a mesh that references an emissive map; `docs/mesh-edit-regions.md` beside the updated
  `docs/mesh-editing.md`, and the fill module in `dev-docs/ludion-mesh-edit.md`. No wire method, no schema-hash
  movement, no golden or budget moved.
- **As built, where it differs from the draft.** Open question 4's ruling is the metric: each texel against
  the **seed** colour by Euclidean sRGB distance, `key` the same; `max_radius` and `tolerance` are required
  (the draft had the radius optional) and `map` defaults to base colour. The walk starts at the covered
  texel nearest the seed, welds vertices by exact position so a UV seam does not stop it (seam bridges are
  sampled between the two sides), never reaches a texel no triangle covers, and its result is independent of
  visit order (membership is reachability). `feather` on a fill fades the weight *inside* `max_radius` — the
  fill never paints past it. A seed must lie within a thousandth of the bounds diagonal of the surface and
  inside its radius, else `L1178`. Two conformance hashes pin the derive for the second machine:
  `PLATE_EMISSIVE_HASH` and `STRIP_FILL_HASH` (`crates/ludion-mesh-edit/tests/derive.rs`).
- **The content.** `games/vigil/scenes/meshes/idol-generated/meshes/idol-fill.mesh-edit.yaml` — the same eye
  by one `fill` op (tolerance 0.13, `max_radius` 0.034, feather 0.004, seeded from a pick at the eye's
  middle), 382 painted texels of a created 1024² emissive map — its derived tree, and the sibling
  `scene-fill.yaml` at `Mesh.emissive: 3.0` beside `scene-lit.yaml`. Authored blind in 5 applies and 9 picks;
  the emissive channel shows one blob in `back` and `left`. The fill fits the painted eye's outline and
  crosses the seam where the primitives needed two ops; it gives a hard, texel-stepped edge and leaves the
  pupil unlit. The size gate's allowlist gained the derived mesh. Log, the author's read and the ranked
  findings: `Plan/artifacts/mesh-texture-editing-log.md`.
- **Checkpoint 2 verdict (user, 2026-09-28, the Models view, close up).** The fill is preferred and kept:
  it takes the almond of the painted eye and fills the socket, but over-paints — a tail runs past the
  socket where the dark paint continues, with a hard, texel-stepped edge. `idol-lit` under-paints: a
  clean, soft-edged bar too thin for the socket and blind to its shape. Each has the other's strength;
  the next experiment is a primitive bounding the socket with a base-colour `key` (shape from the
  primitive, outline from the colour), authored the same day as `idol-key.mesh-edit.yaml` + `scene-key.yaml`:
  one op, a three-point capsule (radius 0.011, feather 0.003) along the eye with a base-colour `key` at the
  eye paint's mean colour (tolerance 0.11 — 0.15 leaked a sliver up the lid crease), 568 texels in 2 applies
  and 0 picks. Sheet read: the fill's outline without its ragged top; the edge is still the key's hard
  colour step except at the capsule ends. The user's look (2026-09-28, Models view): better than both —
  shape plus key is the eye's best form so far. The M4 reproduced both conformance hashes (`Plan/owed.md`, cleared
  2026-09-26). Unbuilt tool findings: a fill report that says whether the radius or the tolerance
  stopped it; a soft edge at a fill's colour boundary; a multi-pixel `pick`; the dim emissive pixels of
  the brow and `front`.

## Why

`Plan/comfy-generation.md` Wave 2 landed a generated textured mesh in vigil's tree
(`games/vigil/scenes/meshes/idol-generated/`: `meshes/mesh-0.mesh.yaml` with an `origin:` block, three
content-hash PNGs under `textures/`), and the user's checkpoint-2 verdict was that it looks good and this
is the direction to pursue. It arrived at 4 190 vertices / 4 986 triangles with base-colour,
metallic-roughness, normal and occlusion maps — and **no emissive map**: the TRELLIS.2 chain bakes none,
so the idol's eye is painted green in base colour and does not glow. The sculpt idol it replaces had a
glowing eye (`Plan/artifacts/comfy-generation-log.md`, wave-2 reading).

A generated asset is *bought* surface, and the one thing the buyer must be able to do afterwards is **say
what glows**. Today there is one answer and it is the wrong one: the `shape: mesh` sculpt part re-meshes
the triangle soup through a 64-cell field and **drops UVs and every texture** — form, not surface. An
agent holding a generated mesh cannot change one texel of it.

The engine side exists. `crates/ludion-scene/src/mesh.rs` carries an `emissive_texture:` slot
(`MESH_KEYS:46-64`), `crates/ludion-engine/src/mesh_asset.rs::load_textures:69-141` binds it, and
`crates/ludion-renderer/src/shaders/mesh.wgsl:549-556` multiplies it into the term that feeds bloom.
`Plan/textured-pbr.md` shipped all of it; **nothing authors it.** That plan's decision 1 — textures arrive
only through `ludion import`, no authoring surface — was right while textures came only from glTF assets
an artist had finished, and is the fence this plan asks to move, once, for a named reason.

## The user's rulings (settled 2026-09-18)

1. **Emissive is never detected automatically.** No heuristic reads a base-colour map and decides
   something should glow — "we would end up with glowing things that should not be emissive". Emissive is
   **authored on request**; Ludion's job is tools that let an LLM edit the emissive map deliberately and
   **verify the result** without a human looking at the screen.
2. **Region vocabulary: 3D primitives, plus a seeded flood fill, staged.** Regions are the sculpt
   vocabulary (`sphere`/`capsule`/`box` with `at`, radii, a feather) — what an agent already writes and a
   human already reads. A **flood fill** is interesting but unproven: its own wave, its own checkpoint,
   deletable; when it exists it is **always seeded by an explicit pick and bounded by a tolerance**. A
   **colour key is a filter inside an explicit region**, never a global detector — that is rule 1 through
   the back door.
3. **The edit document lives beside the accepted mesh; the generated mesh and its maps stay pristine**, so
   provenance is intact and one can return to the generated asset and branch another version. Edits are an
   **ordered, replayable op list in canonical text**; the edited PNG(s) are generated from it, write-only,
   behind a drift gate — the `generate-atlas --check` precedent.
4. **"Baked meshes" are a recorded long-term possibility, deferred with a trigger** — an optimised
   shipping asset with every modification flattened into one mesh and one map set.

## What exists today (verified)

- **The mesh document** (`crates/ludion-scene/src/mesh.rs`): 17 keys; a `TextureRef` is
  `{ path, wrap?, scale?|strength? }`, `path` **relative to the document's own directory** (`:124-137`);
  any texture needs `uvs`, `normal_texture` needs `tangents` (`:1012-1072`). Every optional key emits only
  when present, so a document authoring none is byte-identical to what it was before the key existed.
- **The generated idol** authors `positions, normals, uvs, tangents`, four texture references and **no
  per-vertex material list**, so `mesh_asset::decode`'s `authored_material` is **false** (`:170-172`) and
  the `Mesh` component's scalars apply. Its scene sets `Mesh.emissive: 0.0`, `metallic: 1.0`.
- **The emissive term** (`mesh.wgsl:549-556`): `emissive = base * max(in.material.z, 0)` then
  `* textureSample(emissive_tex, …)` behind a flag, `base` = instance colour × vertex colour.
  **Load-bearing consequence:** a map alone changes nothing — the intensity lane must be non-zero, and
  with no authored material lists that lane is the scene's `Mesh.emissive`. Equally, with a map black
  everywhere but the eye, raising that one scalar lights **only** the eye, tinted by `Mesh.color`.
- **`render --model`** (`crates/ludion-engine/src/model_sheet.rs`) renders a `kind: mesh` document
  standalone (`load_model:1517-1545`); its camera is **pure in bounds and cell size**: seven views with
  fixed direction/up (`direction_and_up:297-318` — `front` sits on **−Z**, the documented voxel
  convention), perspective `fov_y 45°`, near `0.05`, far `100`, a rig scaling by `1/half-diagonal` with
  the AABB bottom-centre at the origin (`NormalizeRig::from_bounds:2478-2492`), and
  `fit_distance_for_aspect:2772-2778` = `1.1 / sin(min(half_fov_y, half_fov_x))`. `--views front` makes
  the cell the whole image. All three helpers are private; `--part` is sculpt-only.
- **A screen→world ray already exists on the CPU**: `ludion::math::camera_ray`
  (`crates/ludion/src/math.rs:133-172`) — translation, Euler-XYZ degrees, projection, near/far, a pixel
  (top-left origin, +Y down), viewport — over `ludion_core::math` libm rather than glam trig, so it is
  platform-identical, and `ViewRig::compute:2733-2761` produces exactly the pair it wants. **No
  ray-triangle test and no GPU pick exist**: `readback.rs` reads whole frames only, and `ludion-sculpt`'s
  `TriangleSoup` (`src/soup.rs:56`) has a fixed-order BVH but is `pub(crate)`, unexported, and answers
  closest-point and winding number only.
- **The drift-gate shape** (`crates/ludion-cli/src/generate_atlas.rs`): one bake shared by generate and
  check; `--check` re-derives **in memory**, byte-compares each output, reports `DRIFT <path>: <reason>`
  under the owning document's **existing structure code** (`:134-140`); the output carries
  `generated_from:` at column 0, which is how `scripts/atlas-drift.mjs` *discovers* what to check; the
  step sits in `check.mjs`'s heavy lane, `ci.yml` and the private provisioner; `:191` self-checks the
  emitted document's parse→emit round trip first.
- **PNG seams** (`crates/ludion-assets/src/lib.rs`): `decode_png`/`load_png`/`encode_png`/`content_hash`
  are the only image seams — RGBA8, IHDR/IDAT/IEND only, deflate pure in its input, "the one PNG encoder
  in the engine"; `Image.rgba` is tightly packed, top row first; the sRGB↔linear helpers are private.
  Import names a map by the `content_hash` of the **re-encoded** bytes, `{hash:016x}.png`, in a
  `textures/` folder sibling to `meshes/`. Over 1 MiB, `scripts/size-gate.mjs` demands a glob in
  `scripts/size-allowlist.txt`, and a **stale entry matching nothing is itself a failure**.
- **Codes** (`crates/ludion-core/src/codes.rs`): `L1161`–`L1172` are the generation arc's, the last taken
  in `L11xx`; `L1201`–`L1203` is the import band.

## Decisions

**[user]** = ruled 2026-09-18. **[proposed]** = this draft's reasoning, awaiting settlement.

1. **[user] Emissive is authored, never inferred** (ruling 1). Every emissive texel exists because an op
   said so, at a named place, in a named colour.
2. **[user] The pristine asset is immutable; the edit is a separate document beside it** (ruling 3).
   `gen accept`'s tree is never rewritten, so a second variant is one more edit document, never an undo.
3. **[proposed] The edit document is `kind: mesh-edit`; its derived output is a sibling directory it
   owns.** It names a `source:` mesh document (relative to itself — the `TextureRef` rule), an `out:`
   directory and an ordered `ops:` list. `ludion mesh apply` writes into `out/` and **nowhere else**:
   `mesh.mesh.yaml` (carrying `generated_from:`) plus `textures/<hash>.png` per changed map. Maps the ops
   did not touch are **referenced, not copied** — one new emissive map costs one new PNG, not four.
   *Why a derived document rather than patching the source's texture keys in place:* a drift gate can only
   own a whole file; a half-authored, half-generated document cannot be byte-compared and gives an agent
   no answer to "may I edit this file". **Rejected likewise:** a sidecar the loader merges (two sources of
   truth, and the engine learns about editing); a `MeshSource` variant reading the edit document (an
   engine feature, and a level that fails to load on a bad op list); rewriting the pristine document and
   keeping a copy (ruling 2). **Honest cost:** the derived document duplicates the source's float lists —
   1.21 MB here — for a change that touches no vertex. The alternative, a mesh document inheriting
   geometry from another, makes `kind: mesh` no longer self-describing for `check-scene`, the loader, the
   model sheet and every golden. **Recorded, not taken;** trigger: the third variant of one heavy asset,
   or per-asset size-allowlist lines outnumbering the assets.
4. **[proposed] A scene selects the edited variant by pointing at it.** `Mesh.shape: asset:
   meshes/idol-lit/mesh.mesh.yaml` — an ordinary content-root-relative path to an ordinary mesh document
   (`MeshSource::Asset`, `crates/ludion-engine/src/components.rs:290-291`). No resolution rule, no
   override table, no variant registry, no engine change; the pristine path keeps rendering the pristine
   asset, and switching is a one-line scene diff.
5. **[user] Ops are ordered and replayable; the PNGs are write-only and drift-gated.** `mesh apply`
   re-derives from (source bytes, edit bytes, pristine PNG bytes); `--check` re-derives in memory and
   byte-compares. `scripts/mesh-edit-drift.mjs` discovers derived trees by their `generated_from:` line as
   `atlas-drift.mjs` does, and joins `check.mjs`'s heavy lane, `ci.yml` and the private provisioner in the
   same change. The derived tree is never hand-edited.
6. **[proposed] The derive is bit-exact CPU work, conformance-tested on both pinned platforms.** Derived
   PNG bytes are committed and byte-compared on Windows x86-64 and macOS arm64, so the crate takes
   `ludion-sprite`'s **strict** clippy denylist (no std `HashMap`/`HashSet`, no clock, no threads) and
   every transcendental goes through `ludion_core::math`. Texel math is `f64`, rounded to bytes once; a
   texel covered by more than one UV triangle resolves to the **lower triangle index** (the
   `TriangleSoup` rule, `soup.rs:203`). A conformance hash over a fixture lands from one pinned box and
   **must be reproduced on the other before any bless counts**.
7. **[proposed] `ludion mesh pick` is a CPU ray cast reusing the model sheet's exact camera.** That camera
   is already pure in `(view, bounds, cell_w, cell_h)` and `camera_ray` already turns a pixel into a ray,
   so no GPU, readback or device is involved. The three private helpers become `pub` and are **called,
   never re-derived** — two camera definitions would disagree silently with the picture the author is
   looking at. The ray-triangle test (Möller–Trumbore, nearest `t`, ties to the lower index) is new and
   lives in this plan's crate, brute force over the triangle list: 5 k triangles per pick is microseconds,
   and the `TriangleSoup` BVH is there when picks become batched. **The one trap:** the sheet renders the
   **normalized** model, so `pick` reports the **model-space** point by default — the space regions are
   written in — and names the view's axis as the cell label does (`FRONT -Z`), because calling the
   authored front `+Z` has produced 180°-wrong results twice (`model_sheet.rs:275-280`).
8. **[user, staged] Primitives first, flood fill second.** Wave 1 ships `sphere`, `capsule`, `box`, each
   with an optional `feather` (world units, linear falloff outside the surface). Wave 2 ships `fill`,
   pick-seeded and tolerance-bounded, deleted if its checkpoint says the primitives sufficed. A colour
   `key:` filters *within* a region and is never a region itself.
9. **[proposed] Intensity is not the edit document's business.** The glow's shape and colour live in the
   map; its intensity lives where intensity already lives — the scene's `Mesh.emissive`, or a per-vertex
   `emissive` list when a mesh authors one. **Rejected:** writing a constant per-vertex `emissive` list
   into the derived document to make the asset self-contained. It would flip `authored_material` to true
   and silently make that scene's `Mesh.roughness`/`metallic`/`emissive` inert — vigil's idol sets
   `metallic: 1.0`, which would become `0.0` for reasons nothing in the diff explains. The derive
   therefore **never invents a material list**; the trap is a docs line and a test. (Open question 5.)
10. **[proposed] Verification is two machine-readable instruments.** (a) **A per-op report** from
    `mesh apply --json`: texels painted, share of the map, the world-space AABB of what was painted,
    triangles touched, and per map the non-black texel count. An op painting **zero** texels is a coded
    error — for a blind author a region that misses the surface is the likeliest failure and the hardest
    to see. (b) **`render --model … --channel emissive`**: the synthesized stage with lights, ambient and
    environment at zero, so the only non-black pixels are emissive. A composition of the stage's own
    scene, not a shader mode, pass or uniform — and it answers "the eye glows **and nothing else does**"
    categorically rather than by inspection.
11. **[proposed] These are pipeline verbs, so the CLI is the parity gate.** Both are batch producers of
    committed files with `--json` — `Plan/comfy-generation.md` decision 15 exactly — so no wire method, no
    schema-hash movement, and any client with a shell has the capability. An editor view is last and
    optional, as the generation view was.
12. **[proposed] The crate is a new host-side sibling, `ludion-mesh-edit`.** Deps `ludion-core`,
    `ludion-scene`, `ludion-assets` — nothing below the render-graph line, no wgpu, no engine (the
    `ludion-sprite`/`ludion-sculpt` shape; `dev-docs/crate-graph.md`). It owns the region algebra, the UV
    rasteriser, op application, the ray test and the derived-tree writer; `ludion-cli` is thin over it and
    supplies the camera from the engine side. **Rejected:** `ludion-import` (foreign formats),
    `ludion-sprite` (2D atlas pipeline), `ludion-sculpt` (the field-based path this plan exists to avoid).
13. **[proposed] The driver is vigil's generated idol, authored blind.** One glowing green eye, placed
    through `pick` + a `sphere`, verified by the report and the emissive channel, renders and tokens
    logged as the sculpt and generation arcs logged theirs. The human sitting comes after the tools say it
    worked, never instead of them.

## The edit document

`kind: mesh-edit`, file `<name>.mesh-edit.yaml`, beside the mesh it edits. Canonical key order
`format, kind, name, source, out, ops`. Block style throughout — the strict subset refuses flow
collections (`L1005`) and block scalars (`L1006`).

```yaml
format: 1
kind: mesh-edit
name: idol-lit
source: mesh-0.mesh.yaml        # relative to THIS document's directory
out: idol-lit                   # the derived directory, relative; default: this document's stem
ops:
  - op: paint
    map: emissive
    size: 1024                  # only on the op that creates a map the source does not have
    color:
      - 0.05
      - 1.0
      - 0.35
    strength: 1.0               # optional, default 1
    region:
      sphere:
        at:
          - 0.118
          - 1.342
          - 0.203
        radius: 0.035
        feather: 0.012          # optional, world units, default 0
```

- **`source`** required, a `kind: mesh` document carrying `uvs` — a map is sampled at vertex UVs, and an
  edit without them has nowhere to paint. **`out`** optional, relative, may not escape the document
  (`..` refused) and may hold nothing the derive did not write.
- **`ops`** ordered, each reading the result of the one before — the replayable half of ruling 3.
  `op: paint` is Wave 1's only verb. **`map`** is one of the five slot names; Wave 1 accepts `emissive`
  only (the others are a wave, not a format change). **`size`** is the created map's square extent, legal
  only on the first op targeting a map the source lacks, defaulting to the base-colour map's dimensions.
- **`color`** three floats in `[0, 1]`, the authored sRGB value; **`strength`** scales the op's weight,
  not the colour.
- **`region`** a one-key map: `sphere { at, radius }`, `capsule { from, to, radius }`, `box { at, size }`
  — the sculpt part vocabulary, in the source mesh's own coordinates — each taking an optional `feather`:
  weight `1` inside the surface, falling linearly to `0` over `feather` world units outside it.
  **`key`** (Wave 2, optional, inside `region`) is `{ map, color, tolerance }`, decision 8's filter.

**How a paint op resolves.** Rasterise the source's UV triangles into the target map's grid at texel
centres `((x + 0.5)/w, (y + 0.5)/h)`, `v` measured downward from the top row (glTF's convention and
`Image.rgba`'s row order). For each covered texel, interpolate the triangle's vertex positions
barycentrically to a **surface point**, evaluate the region's signed distance there, turn it into a weight
through `feather`, apply `key` when present, and blend `color` with `weight × strength`. Blending is in
**linear light** through the sRGB helpers `ludion-assets` already owns beside `premultiply` (made `pub`
there — one definition of the transfer function, in the crate that owns PNG colour). After the last op,
**dilate** each written map by `pad` texels (default 2) into texels no triangle covers, so bilinear
filtering and the first GPU-generated mip do not pull the background across a UV island border.

**Validation.** `L1173` structure (missing/wrong-typed keys, unknown key with its nearest match, unknown
`op`/`map`/region shape — the `mesh.rs`/`sculpt.rs` split). `L1174` data (non-finite numbers, non-positive
radius or extent, colour outside `[0, 1]`, unresolvable `source`/`out`, `..` in `out`, `size` on a map
that already exists). `L1176` "this edit cannot apply to this source". `L1177` an op that painted nothing.
Drift reuses `L1173`, the atlas precedent.

## The derived tree

```
meshes/
  mesh-0.mesh.yaml            pristine — from gen accept, never rewritten
  idol-lit.mesh-edit.yaml     the edit document
  idol-lit/                   wholly generated; the drift gate owns every byte
    mesh.mesh.yaml            generated_from: ../idol-lit.mesh-edit.yaml
    textures/8f3c….png        the derived emissive map (content-hash named, 16 hex)
textures/                     pristine maps, unchanged, referenced from the derived document
```

The derived document is the source's text with three differences: `generated_from:` (the **18th**
`kind: mesh` key, emitted only when present, so every existing mesh document stays byte-identical — the
`origin:` precedent), the edited slots repointed at the derived PNGs, and every untouched slot repointed
at the pristine PNG relatively (`../../textures/<hash>.png`). `origin:` rides through verbatim: the
derived asset still says where its geometry came from, and now also what was done to it. The derive
self-checks the emitted document's parse→emit round trip and refuses to write outside `out/`.

## The verbs

Both take `--json` — one object on stdout, diagnostics on stderr (the `image-stats` precedent).

| verb | does |
|---|---|
| `ludion mesh pick <mesh.yaml> --view <name> --pixel X,Y [--width W] [--height H]` | Rebuilds the exact `render --model` camera for that view and cell size (defaults matching `render`'s `1920×1080`), casts one CPU ray, prints the hit: model-space position, the normalized position the sheet drew, triangle index, barycentrics, UV, the texel coordinate in each referenced map, and the base colour under it. A miss is `L1175`, naming the view's axis and the extent assumed. |
| `ludion mesh apply <edit.yaml> [--check]` | Re-derives the whole `out/` tree from the three inputs. Without `--check`, writes it and prints the per-op report (decision 10a). With `--check`, writes nothing and byte-compares, reporting `DRIFT <path>: <reason>`. |

**Not built** (one obvious way): a `mesh new` scaffold — an edit document is twelve lines and the docs
carry the template (the `gen new` precedent); a `mesh undo` — an op list is edited by deleting a line; a
preview verb — the report and the emissive channel are the preview.

## Waves

### Wave 1 — one glowing eye, primitives only

1. **Codes.** `L1173`–`L1177` in `crates/ludion-core/src/codes.rs` with the what/where/why/fix paragraph
   each entry carries; `ludion explain L1173` … must print. **"Next free" at drafting** — re-grep the tree
   and every plan's Reserved section before assigning, and hold `L1178`–`L1180` for Waves 2–4.
2. **The document.** `crates/ludion-scene/src/mesh_edit.rs` beside `mesh.rs`: parse (`L1173`), validate
   (`L1174`), canonical emit, `emit(parse(emit(d))) == emit(d)` tested; `generated_from:` added to
   `kind: mesh` with every existing document asserted byte-identical; `mesh-edit` registered in
   `ENGINE_KINDS` (`crates/ludion-engine/src/docs.rs`) — the only engine-side fact about the kind.
3. **`crates/ludion-mesh-edit`.** Modules `region` (primitives, signed distance, feather), `raster` (UV
   rasterisation, texel→surface point, the tie rule, the dilate pass), `paint` (op application in linear
   light), `ray` (Möller–Trumbore and the hit record), `derive` (the out-tree writer, the byte-compare
   half, the report). Strict `clippy.toml` copied from `ludion-sprite`; `ludion-assets`' sRGB helpers made
   `pub`.
4. **Camera reuse.** `direction_and_up`, `fit_distance_for_aspect`, `ViewRig::compute` made `pub` and
   re-exported through the facade; `mesh pick` composes them with `camera_ray` and the crate's ray test. A
   test asserts the reconstructed camera equals the sheet's for all seven views at two cell sizes — the
   anti-drift check for the one fact whose divergence would be invisible.
5. **`mesh apply` + the drift gate.** `scripts/mesh-edit-drift.mjs` on the `atlas-drift.mjs` pattern
   (discover by `generated_from:`, dedupe by resolved edit document, spawn `--check`, read the exit code),
   a step in `check.mjs`'s heavy lane, the matching `ci.yml` job and the private provisioner's mirror —
   all four in the same change (`dev-docs/crate-graph.md`).
6. **`check-scene --bounds` on a `kind: mesh` document.** Pays a logged finding
   (`Plan/artifacts/comfy-generation-log.md`, wave-3 item 6: `--bounds` refuses a mesh document, so the
   last run read both boxes with a throwaway script). Placing a region primitive is that question exactly.
7. **`render --model … --channel emissive`** (decision 10b): the synthesized stage with lights, ambient
   and environment zeroed. One flag, one branch in `synthesize_scene`, no renderer change.
8. **Content.** `games/vigil/scenes/meshes/idol-generated/meshes/idol-lit.mesh-edit.yaml` and its derived
   tree; the idol's entity repointed at the derived document and given a non-zero `Mesh.emissive`
   (decision 9). Authored **blind**: pick from a `front`/`left` sheet, write the region, apply, read the
   report, look at the emissive channel, iterate. Every render and token count logged.
9. **Tests.** Region distances against hand-computed values; a rasteriser fixture whose texel sits on a
   shared triangle edge (the lower-index rule); the dilate pass at an island border; a conformance hash
   over the derived PNG bytes (landed from one pinned box, **owed** on the other); the pick camera
   equality test; `apply --check` green on the committed tree and red after a one-byte change; a
   zero-texel op as `L1177`; the derived document's round trip.
10. **Docs** (the `ludion-docs` skill; `check.mjs --fix` re-mirrors `embedded-docs/`). New
    `docs/mesh-editing.md` (document, verbs, derived tree, decision 9's intensity rule and its trap) and
    `dev-docs/ludion-mesh-edit.md` (layout, the rasterisation contract, the determinism obligations);
    rows in `docs/{scene-format,project-anatomy,materials}.md` and
    `dev-docs/{scene-kind-documents,engine-content-kinds,crate-graph,ludion-cli}.md`; the root CLI list
    gains `mesh`.
11. **Verify.** `node scripts/check.mjs`, and the size gate consulted **before** committing the derived
    PNG (a mostly-black 1024² emissive map compresses far under the line; an allowlist entry, if ever
    needed, lands in the same commit as the file).
12. **Exit.** Mechanized: the report says N texels painted inside an AABB matching the eye, the emissive
    channel shows one lit blob and nothing else, the drift gate is green, both pinned boxes agree on the
    conformance hash. **Checkpoint 1 (the sitting):** the lit idol on the model stage beside the pristine
    one — does the eye read, is the glow the right size and colour, was the loop worth the machine? A "no"
    ends the arc here; the verdict is recorded as a decision with its evidence.

### Wave 2 — the seeded flood fill (checkpoint; may be deleted)

`region: fill { at: <a picked surface point>, tolerance, map?, max_radius? }` and decision 8's `key:`
filter. Seeded only by an explicit pick; bounded by a tolerance **and** an explicit radius, so a bad
threshold cannot eat the model. Open question 4 settles the metric and connectivity before a line is
written. **Checkpoint 2:** a fill authored blind on the same idol against the primitive that did the job
in Wave 1 — two author votes to delete it if the primitive was as good.

### Wave 3 — the other maps (the extension, not a new mechanism)

The same document and ops, one more legal `map:` value at a time: a base-colour tint, a roughened patch,
an occlusion darkening. Nothing new in the format; the cost is per-map colour-space and packing care
(metallic-roughness is `G`=roughness, `B`=metallic; a normal map is a frame, not a colour, and a paint op
on it is almost certainly refused). `--channel` grows the matching views. **Checkpoint 3:** one edit per
map that an author actually wanted, or the wave is one map wide.

### Wave 4 — vertex deformers (tier 2)

`op: deform` — `bend`/`twist`/`taper`/non-uniform `scale`, the sculpt deformer vocabulary, applied to the
mesh's **own vertices** under a region mask, with **topology and UVs untouched** so every texture survives
and normals and tangents recomputed. Further ops in the same document and the same derived tree; the
derived `positions`/`normals`/`tangents` then genuinely differ from the source's, which is where decision
3's duplication stops being waste. **Checkpoint 4:** deforming bought geometry against re-generating it.

## Engine/game boundary

Everything above is tooling. The engine gains four facts: one optional `generated_from:` key on
`kind: mesh`, one name in `ENGINE_KINDS`, three already-pure functions made `pub`, and one debug flag on
`render --model`. No component, no system, no wire method, no schema-hash movement, no renderer change.
Vigil consumes a derived mesh document exactly as it consumes any other mesh.

## Deferred / non-goals (with triggers)

- **[user] "Baked meshes"** — an optimised shipping asset with every modification flattened into one mesh
  and one map set, provenance recorded rather than replayable. **Trigger:** a shipping build where the
  derived trees' load cost, file count or duplicated geometry is measured to matter. Until then the source
  of truth is the op list and the flattening is the drift gate's job.
- **Tier 3: carve / blend with local re-mesh and texture re-projection.** Changing topology invalidates
  the UV layout, so it needs a UV unwrapper **and** a transfer bake — neither of which Ludion has, both
  real projects. **Trigger:** content that needs a hole or a fused addition in a generated mesh *and*
  keeps its texture, twice, with the `shape: mesh` sculpt path proved inadequate for both. Core ComfyUI
  already ships `UnwrapMesh` and a bake stage: the honest first answer may be a generation recipe.
- **Painting from an image** (projecting a generated patch through a camera onto the map) — a real want, a
  different mechanism (a projector, not a region), and it reopens ruling 1's edge, since an image can say
  "glow" without anyone deciding it should. **Trigger:** the first edit a region cannot express.
- **An editor view** — optional and last, under decision 11.
- **Texture *generation* onto existing UVs** — already a named non-goal of `Plan/comfy-generation.md`.
- **Automatic emissive detection, in any form** — permanently rejected (ruling 1).

## Risks

- **A blind author paints in the wrong place and nothing says so.** Why `L1177` is an error and the report
  prints a world-space AABB. The residual risk is an op that paints somewhere, just not the eye; the
  emissive channel render answers that in one image.
- **Derived-geometry duplication** (decision 3) — 1.21 MB per variant against a 1 MiB gate whose allowlist
  already carries a line for this asset. A cost, not a defect, until a third variant exists.
- **Cross-platform byte identity of the derived PNG** — the one genuinely new bit-exact CPU artifact here,
  contained as the sculpt field and the tangent generator were: `ludion_core::math` only, `f64` texel
  math, a conformance hash reproduced on the second pinned box **before** any bless.
- **Camera drift between `pick` and the sheet** would make every pick a few pixels off and the author blame
  their region. Contained by calling the same functions and by the equality test in Wave 1 step 4.
- **The intensity trap** (decision 9): an emissive map with `Mesh.emissive: 0` looks exactly like a broken
  paint. First line of the docs, a `next:` line on `apply`'s report, and a test on the fixture scene.
- **Seams and mips.** A 2-texel dilate covers bilinear sampling and the first mip; a small blob on a 1024²
  map can still fade at distance, since mips are GPU-generated from the base level (`Plan/textured-pbr.md`
  decision 5). Measurable at the sitting; the fix is a larger `pad` or a committed mip chain.
- **Scope creep into a paint program.** Every wave after the first is gated on a checkpoint, and three
  capabilities (image projection, topology edits, automatic detection) are named non-goals up front.

## Reserved diagnostic codes

`L1173`–`L1177`, proposed as next free at drafting (`L11xx`'s last taken is `L1172`; `L1201`–`L1203` is
the separate import band). **Re-grep before assigning.** `L1178`–`L1180` held for Waves 2–4.

| code | for |
|---|---|
| `L1173` | **Mesh-edit document structure** — missing or wrong-typed key, unknown key with its nearest match, unknown `op`, `map` or region shape. Also carries the drift report (the `generate-atlas` precedent). |
| `L1174` | **Mesh-edit document data** — non-finite number, non-positive radius/extent, colour outside `[0, 1]`, unresolvable `source`/`out`, `..` in `out`, `size` on a map that already exists. |
| `L1175` | **Pick unresolved** — no triangle under that pixel, an unknown view name, or a pixel outside the assumed extent; names the view's axis and the extent. |
| `L1176` | **Edit inapplicable to this source** — the source has no `uvs`, a target map cannot be created for want of a `size`, or `out` holds files the derive did not write. |
| `L1177` | **Op painted nothing** — the region covers no texel of the surface; names the op index and the region's world box against the mesh's. |

## Artifacts

`Plan/artifacts/mesh-texture-editing-log.md` — created with Wave 1's first row, the columns of
`Plan/artifacts/comfy-generation-log.md` and `Plan/artifacts/mesh-modeling-log.md` so the three compare
directly (`subject | wave | attempts | renders read | tokens | wall time | verdict`, plus the provenance
columns), and a ranked tool-friction list per run.

## Settled questions

The user agreed these answers on 2026-09-18; the questions are kept below for their reasoning.
**1** `kind: mesh-edit` / `<name>.mesh-edit.yaml`. **2** Accept the duplicated float lists; the
geometry-inheriting mesh document stays a recorded alternative with its trigger. **3** `pick` is a CPU ray
cast. **4** Settled for Wave 2 (user, 2026-09-23): each texel is compared to the **seed** colour by Euclidean sRGB `[0, 1]` distance (`key` uses the same metric); the walk is surface connectivity with vertices welded by exact position, so UV seams do not stop it; `max_radius` (3D straight-line distance from the seed) and `tolerance` are both required; `map` defaults to base colour. **5** Intensity stays in
the scene. **6** `render --model --channel emissive`, no second render verb. **7** `map:` is a closed set of
five with four refused until Wave 3; whether Wave 3 runs is decided after checkpoint 1.

1. **The edit document's kind and file name.** `kind: mesh-edit` / `<name>.mesh-edit.yaml` as drafted, or
   something shorter (`kind: paint`? `<name>.edit.yaml`?) — the name is read far more often than typed,
   and it must still fit when Wave 4 puts vertex deformers in the same file.
2. **How a scene selects the edited variant.** Decision 4 makes it an ordinary path to the derived
   document, at the cost of duplicating the source's float lists per variant (decision 3). The
   alternative — a mesh document that inherits geometry from another by `source:` — removes the
   duplication and makes `kind: mesh` no longer self-describing. Accept the duplication now, or take the
   format change up front?
3. **Is `pick` CPU or GPU?** Drafted CPU (decision 7): deterministic, testable, no device, reusing a
   camera helper that already exists. A GPU pick would need a triangle/entity-ID attachment and a
   single-pixel readback, neither of which exists — but it would be exact against the pixels actually
   drawn rather than against a reconstruction of them.
4. **The flood fill's metric and connectivity** (settled — see above). Colour distance in **which** space —
   sRGB bytes (what the author sees in the map), linear light (what the shader samples), or a perceptual
   space (what "looks like the same green" means)? And does it walk **UV neighbours** (cheap, stops dead
   at every island border, so an eye split across two islands fills half) or **surface neighbours** across
   triangle adjacency (right, needs an adjacency structure and a rule for texels no triangle covers)?
5. **Should an edited asset carry its own glow intensity?** Decision 9 says no — intensity stays in the
   scene, because writing a per-vertex `emissive` list into the derived document would silently make that
   scene's `Mesh.roughness`/`metallic`/`emissive` inert. The cost is that the edited idol does not glow
   until a scene asks it to, and every scene using it must remember.
6. **Where does the emissive-only view belong?** Drafted as `render --model --channel emissive` (decision
   10b), one more flag on an already wide command; the alternative is a verb of its own, at the price of a
   second way to render a model.
7. **Wave 3's scope.** Are the other four maps wanted as a wave at all, or is emissive the whole need? The
   answer decides whether Wave 1 builds `map:` as a closed set of five with four refused, or as a
   one-value key that grows.
