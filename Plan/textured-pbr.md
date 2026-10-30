# Plan: Textured PBR — glTF texture-map import fidelity

> Status: **settled direction, TRIGGER FIRED 2026-09-17 — authored 2026-07-23 (grill-me session).**
> The firing asset is a ComfyUI-generated GLB with baked texture maps (`Plan/comfy-generation.md`,
> decision 4); this arc runs first, as written, and that plan's Wave 2 depends on it. This is
> the plan of record for the last unshipped feature of the PBR arc. It was **not scheduled** until
> the trigger fired: the first external textured asset worth importing with its texture
> maps intact (the sharpened trigger — see below). G7 Spectacle deliberately does **not** consume
> it (voxel authoring is being pushed for G7 instead); this plan waits for a concrete asset, so its
> exit gate is real content rather than a synthetic fixture. Direction settled in
> `Plan/roadmap-voxel-mesh-pbr.md` (the textured-PBR trigger) and refined here.

## What already shipped (so this plan is small)

The PBR arc is almost entirely delivered. `crates/ludion-renderer/src/shaders/mesh.wgsl` is already
full **Cook-Torrance GGX/Smith/Schlick** with per-vertex + per-instance scalar material
(roughness/metallic/emissive), **IBL split-sum** (`@group(3)` env), **sun shadows** (`@group(2)`
PCF), and **emissive through the bloom pipeline**. Phase 11 (scalar PBR) closed and was
human-validated; shadow mapping (`Plan/shadow-mapping.md`, 2026-07-17) and IBL
(`Plan/environment-ibl.md`, 2026-07-22) each left the textured-PBR bundle and shipped as their own
arcs. **The one feature still behind the trigger is texture maps.** Every glTF texture reference is
a structured `L1202` today; `--allow-lossy` re-encodes the PNG bytes into the import bundle but
**nothing samples them**, and no UVs are sampled anywhere on the mesh path.

## The sharpened trigger

**First external textured asset worth importing with its texture maps intact.** Narrowed from the
roadmap's "first content that palette-driven per-vertex materials can't express" by the 2026-07-23
grilling: the driver is **import fidelity**, not an authoring need. Voxel + palette + scalar PBR +
IBL is the agent-authoring story and is not superseded by this; textures earn their place only
because *silently dropping* a texture an artist baked into a glTF violates "unsupported features are
structured errors, never silent drops," and imported assets are the one place external content
legitimately arrives pre-textured. If a future need is instead **surface detail on voxel geometry**,
that is a *different* trigger (voxel texturing — recorded deferred below), not this one.

## Settled decisions (2026-07-23 grill-me session)

1. **Import fidelity only — never an authoring surface.** Textured PBR is a *consumer* of glTF
   assets that already carry UVs + PNGs. Geometry and scalar material stay text-authored and
   diffable; textures are referenced binary assets that arrive **only** through `ludion import`.
   Rejected: any texture-authoring surface (a texture the agent can't read/edit/diff fights every
   pillar and duplicates the voxel/palette path).
   *Revisited 2026-10-23 by `Plan/procedural-textures.md`: a text recipe is diffable source with generated, drift-gated maps, which removes this fence's reason.*
2. **Full five-map glTF metallic-roughness set, in one arc:** `baseColorTexture`,
   `metallicRoughnessTexture` (G = roughness, B = metallic, packed), `normalTexture`,
   `occlusionTexture`, `emissiveTexture`. Chosen over an incremental base-color-first split on user
   direction ("I already know I want the full set long-term") — normal maps and their tangent
   requirement are therefore **core to this arc, not a fast-follow**.
3. **Binding & vertex format, by the established precedents.** Widen the one `MeshVertex` once more
   — add `uv: vec2` + `tangent: vec4` (xyz + handedness sign), **always-populated with sentinel
   defaults** on voxel/primitive vertices (the exact "widen once, default-fill" pattern the material
   lanes already use; `MeshVertex` grows 52 → 76 bytes). Add **one `@group(4)` material-texture
   group** (the five texture views + sampler(s) + a small factor/flag uniform) set **per-draw** in
   the mesh loop — exactly like the sprite path already sets `SPRITE_TEX_GROUP`. Untextured meshes
   bind **1×1 default textures** (white base-color, flat normal `(0.5,0.5,1)`, white MR/occlusion,
   black emissive) so the untextured path stays **byte-identical** — the IBL-fallback move
   (`env.params.x` off binds 1×1 fallbacks today). **One pipeline, no draw re-batching**: mesh draws
   are already instanced per `MeshData` hash, and a textured asset mesh is one `MeshData` = one
   material set, so the per-draw bind aligns with existing batching. Each map multiplies into the
   shader term that already exists. Rejected: a second textured pipeline / a second vertex format
   (two ways); base64-inlined textures (below).
4. **Textures live as referenced, normalized, deduped PNG blobs.** The mesh doc references a texture
   by relative path (`base_color_texture: textures/<hash>.png`), resolved against the scene content
   root (the voxel/asset-doc rule). Import **re-encodes** every source image to a canonical
   normalized PNG — fixed color type/bit-depth, ancillary chunks stripped, deterministic zlib, no
   timestamps — so identical pixels yield **byte-identical committed bytes** across exporters and
   machines (keeps the content-hash dedup sound and the drift gate honest; extends the Phase 8
   deterministic-re-encode posture the import test already asserts). Identical image bytes commit
   **once** and upload **once** through a texture cache keyed like `MeshData.hash`. Rejected:
   base64-inlining into the YAML (a one-pixel edit rewrites a megabyte of un-diffable scalar,
   defeating the minimal-git-diff pillar); verbatim byte-preservation (exporter metadata quirks fork
   the bytes of identical images, breaking dedup + determinism).
5. **Determinism / sampling tier — GPU-side, image tier, mostly by precedent.** Texture sampling is
   GPU-side ⇒ the **image tier** (SSIM + tolerance; bit-exact only on the pinned GPU; per-OS golden
   sets already absorb cross-vendor divergence), exactly where the goldens and the GPU IBL
   prefiltering already sit.
   - **Mipmaps: GPU-generated at upload, NOT committed** (the roadmap decision-4 "no committed
     derived files" parallel — the base PNG is the source of truth, mips are a GPU-cache detail).
     Trilinear. *Marked revisitable* per user (a CPU/committed-mip path can replace it later if a
     need appears); no drift-gate impact either way since mips are never a committed artifact.
   - **Color space per map role, spec-mandated, no knob:** base-color + emissive → `Rgba8UnormSrgb`
     (hardware linearizes on sample); metallic-roughness, normal, occlusion → `Rgba8Unorm` (linear).
   - **Sampler:** honor the glTF sampler wrap mode via a small shared set (repeat / clamp,
     trilinear); no per-texture sampler objects. **Anisotropy 16** (one named constant; user wants
     the fidelity, cost is negligible against E6 headroom, requires the mip chain which we generate
     anyway, no wgpu feature flag needed at ≤16 with linear filtering). **Recorded caveat:** aniso is
     more sensitive to *driver updates* than plain trilinear — a driver bump may force a per-OS
     golden re-bless at a few grazing-angle texels. Re-blessable, not a blocker.
6. **Tangents: import when present, else compute; committed to `.mesh.yaml`.** glTF `TANGENT` is
   imported when the asset carries it; when a `normalTexture` is present without `TANGENT`, tangents
   are computed CPU-side by a deterministic MikkTSpace-equivalent generator (positions + UVs +
   normals → tangent `vec4` with handedness). Either way the tangent is written into the canonical
   `.mesh.yaml` as **committed bit-exact text** — tangents are geometry (the bit-exact CPU tier), so
   the generator is same-input-same-bytes **conformance-tested** exactly like the greedy voxel
   mesher. Rejected: always-compute-ignore-`TANGENT` (discards authored/mirrored tangents an artist
   baked to match their normal map — a fidelity loss under the import-fidelity driver).
7. **Import stops silently dropping the material surface (completing the standing L1202 audit).**
   The five supported maps are read and re-encoded; every *still*-unsupported material feature stays
   a structured `L1202` naming the material (the existing surface — `metallicRoughnessTexture`,
   `normalTexture`, `occlusionTexture`, `emissiveTexture`, base-color texture, non-OPAQUE
   `alphaMode`, `doubleSided` — is already coded and tested in `crates/ludion-import`, so this arc
   *removes* the texture entries from L1202 rather than adding to it). `--allow-lossy` still imports
   the rest. The L1202 explanation text and the crate mapping table update together.
   *Amended 2026-10-30 (owner approval 2026-10-29, Clockout 0010): `MASK` and `doubleSided` leave the L1202 surface (they map onto `alpha_mode`/`alpha_cutoff`/`double_sided`); `BLEND` stays.*

## Fences (explicitly out of scope; triggers recorded)

- **Mesh path only.** Sprite / billboard / tilemap / text shading stays Lambert-flat and
  byte-identical (roadmap decision 8). Stylized flats beside textured meshes *is* the HD-2D look.
  Non-mesh goldens are asserted unchanged.
- **Voxel & primitive meshes stay palette/scalar-material only.** They carry no UVs, so no texture
  fields on voxel palette entries or the `Mesh` component. **Voxel texturing is a recorded deferred
  possibility, not a never** — the widened vertex format already reserves the UV lane, so a future
  voxel-UV-unwrap story fills it with no format change. Trigger: content that palette-per-quad
  genuinely can't express *and* wants surface detail on voxel geometry specifically.
- **No texture-authoring surface / no `texture.*` wire commands / no editor texture UI.** Textures
  arrive only through `ludion import`; a texture reference is a path set at import time. Scalar
  material edits keep riding `component.set`; the parity gate stays clean.
  *Revisited 2026-10-23 by `Plan/procedural-textures.md`: a text recipe is diffable source with generated, drift-gated maps, which removes this fence's reason.*
- **The atlas/sprite-sheet pipeline (deferred since Phase 8) stays deferred.** Mesh textures are
  per-material standalone images, unrelated to sprite-atlas packing. This arc does not fire it.
- **`.mesh.yaml` may reference textures** (it is the format import writes, so the fields exist) but
  hand-authoring one is legal-not-promoted.

## Waves (shape, to be grounded in current-code facts at fire time)

Follows the Phase 11 pattern — land the data/plumbing invisibly, then one bless event.

1. **Text formats + import material read.** `.mesh.yaml` gains optional texture-reference fields
   (canonical order, emitted only when present, old docs byte-stable) + committed tangent list;
   import re-encodes the five maps to normalized PNGs with content-hash dedup, reads factors, and
   drops the texture entries from the L1202 surface. Tangent generator + conformance test. Roundtrip
   proptests + canonical goldens extend. Lands render-inert.
2. **GPU plumbing under the current shader.** Widen `MeshVertex` (uv + tangent, renumber
   locations), widen the material uniform, the `@group(4)` texture group + 1×1 defaults + the
   texture cache, per-draw bind. Layout/stride assertions updated. **Full gallery + game + import
   suites at exactly SSIM 1.0** — the change is invisible until the shader samples (Phase 10/11
   wave-2 invisibility pattern; the driver-codegen re-bless rule applies only on failure).
3. **The sample (the one bless event).** `mesh.wgsl` samples the five maps into the existing terms
   (base-color × albedo, MR → roughness/metallic, normal → tangent-space→world perturbation,
   occlusion × ambient/IBL, emissive × the emissive term); mips + samplers + aniso wired. Mesh-
   bearing goldens re-bless once with diff reports; non-mesh goldens asserted byte-identical.
4. **Docs + exit gate.** Guide "Materials" chapter gains a textures section; import/CLI docs + the
   BRDF/renderer doc update. **Exit gate:** the fire-triggering real textured asset imports and
   renders as authored; **human gate: a look at it**; the import golden re-blessed to show it
   textured; one bless event, blast radius pre-declared. No THE-test (import fidelity is judged by
   the human look, as Phase 11 shading was).

## Risks

- **Ready-but-unconsumed inventory** — mitigated by (ii)/fire-on-demand: the plan is settled and
  shelved, built only against a real asset, so the exit gate is content not a synthetic park.
- **Tangent-generator determinism** — the one genuinely new bit-exact CPU artifact; contained by a
  conformance test (same input → identical committed tangent bytes, twice) like the greedy mesher.
- **Aniso driver-update re-bless** — recorded above; per-OS, re-blessable, not a blocker.
- **glTF assets with a normal map but no `TANGENT`** — handled by the compute path (spec-required);
  the fixture at fire time should exercise both the imported-`TANGENT` and computed-tangent cases.

## Status

**Fired 2026-09-17; grounded 2026-09-18 (rev 748).** Waves 1–4 below are the schedule of record; the
sibling `Plan/comfy-generation.md` (decisions 2, 4, 13) adds `origin:` to `.mesh.yaml` in its own wave
and this arc's keys coexist with it (`origin` after `name`; this arc's keys after the geometry lists).

### Grounding (current-code facts the waves are built on)

- **Import** (`crates/ludion-import`, `gltf 1.4.1` with `import` off — `sources.rs` resolves buffers
  and images by hand): `build_mesh_document` reads POSITION, NORMAL, TEXCOORD_0, indices; **never
  TANGENT or COLOR_0**. `build_textures` (`lib.rs`) re-encodes *every* `document.images()` entry through
  `ludion_assets::encode_png` regardless of `--allow-lossy`, names it `textures/<sanitized name>.png`
  (dedup by *name*, no content hash), and nothing references the files. The five texture slots are
  `L1202` at `mesh_reference` (`lib.rs:392-424`). `ludion_assets::{decode_png, encode_png,
  content_hash}` are the only image seams (`png 0.18`, IHDR/IDAT/IEND only, RGBA8). Determinism
  obligations: document order, `BTree*`, floats stored as `f64::from(f32)`, no `glam` trig.
- **Mesh document** (`crates/ludion-scene/src/mesh.rs`): ten keys in `MESH_KEYS`, emit order
  `format, kind, name, positions, normals, uvs, emissive, metallic, roughness, indices`; L1110 structure,
  L1111 data, L1112 budget. `uvs` is already carried by the importer for TEXCOORD_0 — and dropped by
  `mesh_asset::decode`: `MeshData` (`ludion-renderer/src/scene.rs`) has no `uvs`, no `tangents`.
- **Renderer**: `MeshVertex` (`ludion-renderer/src/mesh.rs`) is 52 bytes, locations 0–3; instance
  lanes 4–15 (`MeshInstanceRaw`, 192 B). Four construction sites, all through `upload_mesh_asset` +
  the three procedural primitives. `prepass.wgsl` declares all sixteen locations (shares
  `MESH_VERTEX_ATTRS`); `shadow.wgsl` reads 0 and 4–7. The mesh pass sets groups 0–3 once per pass —
  **no per-draw bind**; `MeshAssetDraw` has no material field. `SpriteDraw::tex_bg` is the per-draw
  precedent. `Renderer::white` is a 1×1 sRGB white; `upload_texture` hardcodes `Rgba8UnormSrgb`, one
  mip, `TEXTURE_BINDING | COPY_DST`; **no mip generation exists anywhere**, every sampler is
  ClampToEdge with `mipmap_filter: Nearest`, no `Repeat`, no `anisotropy_clamp`. Group 3 is the
  11-entry texture+sampler+uniform bindgen precedent; `@group(4)` is free; the device requests
  `adapter.limits()` (both pinned GPUs report `max_bind_groups` ≥ 8). Adding a layout = `LayoutId`
  variant + `LAYOUT_COUNT` + `LAYOUTS` row + the `SHADERS` row for `Mesh`; the frame graph is untouched.
- **Goldens**: the one asset-mesh gallery scene is `mesh-asset.yaml` (`crystal.mesh.yaml`); the import
  fixture is `examples/import/scene.gltf` (hand-made, base64 URIs, the gem has a base-color texture —
  the fixture's *only* lossy feature) with `expected/` tree identity + `goldens/<os>/scene.png` asserted
  at SSIM exactly 1.0 by `ludion-cli/tests/import.rs::exit_gate`. Whole-gallery exactness lives in
  `ludion-testkit/tests/gallery.rs::all_gallery_scenes_pass_against_committed_goldens_at_exactly_one`.
- **Codes**: L12xx holds exactly L1201–L1203; L1161–L1172 are the generation arc's. **This arc needs
  no new code**: tangent/texture-reference validation is L1111, an unresolvable texture path L1101.
- **Deps**: no `mikktspace`, no `image` (deliberate), no `deny.toml` yet; `Cargo.toml` entries carry a
  justification comment each. No third-party fixture licence file exists.

### Rulings at fire time (grounding the settled decisions)

- **G1 — no new dependency for tangents.** The generator is in-house (`ludion-import/src/tangents.rs`):
  per-triangle tangent from the UV derivative, accumulated per vertex in `f64`, Gram–Schmidt against
  the normal, handedness from the bitangent sign; degenerate UV triangles contribute nothing, a vertex
  with no contribution falls back to a normal-orthogonal axis. Computed only when `normalTexture` is
  present and `TANGENT` absent; imported verbatim when present; **absent otherwise** (no normal map ⇒
  no `tangents` key — the renderer default-fills `(1,0,0,1)`). Conformance: a pinned hash of the
  committed tangent bytes.
- **G2 — the `.mesh.yaml` texture keys.** `tangents` (4 floats per vertex) emits after `uvs`, before
  `emissive`, since it is geometry; the five map keys emit after `roughness`, before `indices`, in
  this order: `base_color_texture`, `metallic_roughness_texture`, `normal_texture`, `occlusion_texture`,
  `emissive_texture`, each a map `{ path: textures/<xxh3-16-hex>.png, wrap?: repeat|clamp|mirror,
  scale?/strength? }` — `wrap` emitted only when not `repeat` (glTF's default), `scale` (normal)
  and `strength` (occlusion) only when not `1`. Paths resolve like a sculpt part's `path`: relative
  to the mesh document's own directory. Validation (L1111): a texture needs `uvs`; `normal_texture`
  needs `tangents`; `tangents.len() == 4 × vertex_count`, `w ∈ {−1, 1}`; every path non-empty.
- **G3 — import emits referenced textures only**, named by the content hash of the canonical PNG
  bytes (identical pixels ⇒ one file, whatever the exporter's chunks or names), and every remaining
  glTF image is simply not written (it was dead weight). Texture slots using `texCoord ≠ 0`, or a
  sampler with unequal `wrapS`/`wrapT`, stay `L1202`. Factors keep their existing mapping
  (base-color factor → `Mesh.color`; MR factors → the per-vertex lists; the proportional-emissive
  rule unchanged, so emission = `color × emissive × emissive_texture`).
- **G4 — the shader multiplies behind per-draw flags, not through 1×1 defaults alone.** The group-4
  uniform carries a `flags` word (which of the five maps is bound) plus `normal_scale`,
  `occlusion_strength`; an untextured draw takes the old arithmetic path exactly (no sample, no
  multiply-by-one), so the untextured suite is byte-identical **by construction** on every driver —
  a flat-normal texel `(128,128,255)` decodes to a non-zero tangent-space xy, so the plan's
  "multiply by defaults" alone could not have been exact. The 1×1 defaults still exist so every
  bind group is complete.
- **G5 — locations renumbered:** per-vertex 0–5 (`position, normal, color, material, uv, tangent`),
  per-instance 6–17; `mesh.wgsl`, `prepass.wgsl`, `shadow.wgsl` and the four attribute arrays move
  together; the generated `*_VS_MAIN_INPUTS` pin them. `MeshVertex` 52 → 76 bytes.
- **G6 — mips by a blit chain** (a `mip_blit` shader through the bindgen registry, textures created
  with `RENDER_ATTACHMENT`), trilinear, `anisotropy_clamp: 16`; two samplers (`repeat`, `clamp`)
  plus `mirror` on demand. Mesh textures are **never premultiplied** (the sprite path's
  `Image::premultiplied` is not taken). Cache key: `(content hash, colour space)` so one PNG bound
  as sRGB and linear cannot alias.
- **G7 — fixtures.** `examples/import-textured/`: a hand-made two-cube glTF (five maps on both, one
  cube carrying `TANGENT`, the other not; the maps shared so dedup is exercised), mirrored by the
  `examples/import/` harness (expected tree, `goldens/<os>/scene.png`, an exit-gate test). The
  existing `examples/import/` fixture stays the lossy-path fixture: its `stone` material becomes
  `doubleSided` so `L1202` still fires once the base-color texture is supported; its gem renders
  textured at Wave 3 (the planned one re-bless). *Amended 2026-10-30 (owner approval 2026-10-29, Clockout 0010): `doubleSided` no longer fires L1202; the fixture's lossy trigger is its `BLEND` material.* **Real-asset check:** Khronos `Avocado` (CC0 1.0,
  glTF-Sample-Assets) imported from an ignored scratch path and rendered to `Plan/artifacts/` — the
  8 MB GLB and its 2048² maps are not committed (the 1 MiB size gate); the fire-triggering ComfyUI
  asset owes the human look at the sitting.

### Wave log

- **Waves 1–4 mechanized-complete 2026-09-18** (Lore revs 752 formats + import, 753 renderer + loader +
  goldens, 754 docs + close).
  - **Wave 1.** `.mesh.yaml` per G2 (`crates/ludion-scene/src/mesh.rs`: 17 keys, one slot table shared
    by emit and validate, L1110/L1111 only; the proptest strategy covers tangents and references;
    old documents byte-stable). Import per G1/G3 (`ludion-import`: `tangents.rs` with a pinned
    conformance hash, referenced-only content-hash-named PNGs, `TANGENT` verbatim else generated
    beside computed faceted normals when a primitive has none, per-`(mesh, primitive)` diagnostic
    dedup so a shared mesh reports once). L1202's texture entries deleted; still L1202: a map on UV
    set ≠ 0, unequal `wrapS`/`wrapT`, a textured primitive without `TEXCOORD_0`, a referenced JPEG.
    Fixtures per G7: `examples/import/` (now the double-sided lossy fixture) and
    `examples/import-textured/` (generator script + two cubes; the computed tangents equal the
    authored ones to the last bit on every face).
  - **Waves 2–3, folded into one commit.** With every sample behind a flag bit (G4), the renderer
    change is invisible until the loader feeds a texture, so plumbing and sampling landed together:
    `MeshVertex` 76 bytes (locations 0–5 / 6–17), `MeshData.uvs/tangents/textures`, `@group(4)`
    (`MATERIAL_GROUP`: five `texture_2d`, one sampler, a 16-byte `MeshMaterial`), `mipgen.rs` (a blit
    chain reusing `blit.wgsl`, two pipelines by colour space), three samplers (repeat/clamp/mirror,
    trilinear, aniso 16), the `(hash, srgb)` cache, `MeshAssetGpu.material_bg` built at upload; the
    loader (`mesh_asset::load`) resolves references against the document's directory, never
    premultiplies, L1101/L1102 at the map's key. **Proof:** the whole gallery, every model sheet
    and vigil's goldens at SSIM exactly 1.0 with no re-bless — the untextured suite is untouched.
  - **Wave 4.** `docs/materials.md` "Texture maps"; `dev-docs/{renderer-shading,ludion-import,
    scene-kind-documents,ludion-renderer,ludion-shader-bindgen,crate-graph,engine-image-paths}.md`
    (the bindgen's vertex-format revisit trigger recorded as fired). Goldens: `import-textured`
    first-blessed and `import` re-blessed on the RTX 3090 (the gem's change is its specular tint —
    its emissive 0.5 / metallic 0.6 material leaves the checker invisible, verified by a scratch
    render with both zeroed where it shows; the fixture stays as the factor-mapping oracle).
    **Real-asset check:** Khronos `Avocado` (CC0 1.0) imports in default mode with zero diagnostics
    (three maps, authored `TANGENT`) and renders textured — `Plan/artifacts/textured-pbr-avocado.png`.
- **Fact worth keeping:** the mesh pipeline now uses 16 fragment-sampled textures (1 shadow + 10
  env/GI + 5 material) — exactly WebGPU's guaranteed minimum — and five bind groups; the device
  requests `adapter.limits()`, fine on both pinned GPUs, a downlevel adapter would refuse the pipeline.
- **`check-scene` validates a texture reference's shape but never opens the PNG** — a missing file
  surfaces at load (`mesh_asset::load`), not from the checker. Recorded in `dev-docs/ludion-import.md`;
  a `check-scene` read of mesh textures is a small follow-up if the sitting wants it.

### Owed

- ~~The human look~~ **passed (the user, 2026-09-21)**: the fire-triggering ComfyUI asset
  (`Plan/comfy-generation.md` Wave 2 accept) seen up close on the model stage — "really great". The exit
  gate's human half; the Avocado render and the textured-cube golden were the mechanized evidence.
- **macOS goldens** for `examples/import/goldens/macos/scene.png` (re-bless) and
  `examples/import-textured/goldens/macos/scene.png` (first bless) on the M4.
- Docs debt unchanged in kind: `dev-docs/scene-kind-documents.md`, `ludion-renderer.md`,
  `ludion-shader-bindgen.md` were over budget before this arc and grew by a few lines each.
