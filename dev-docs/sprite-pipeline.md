---
description: The sprite pipeline end to end: text, bake, pack, commit, extract, renderer, and its codes
---
# The sprite pipeline

**Scope:** the five hops a sprite takes from text to a drawn quad, and which crate owns each. The
document formats are `ludion devdocs scene-kind-documents` (`kind: sprite`/`atlas-source`/`atlas` in
`ludion-scene`); the bake and packer internals are `ludion devdocs ludion-sprite`; the two
CPU-composed sheets are `ludion devdocs engine-image-paths`; PNG decode and the premultiply
convention are `ludion devdocs ludion-assets`. This topic owns the seams *between* them.

## Layout
- `crates/ludion-scene/src/sprite.rs`, `palette.rs`, `atlas.rs` — the text: parse, validate, canonical
  emit for all four families. No IO, no pixels.
- `crates/ludion-sprite/` — the pixels: `bake`, `pack`, `import_png`. No IO either.
- `crates/ludion-cli/src/generate_atlas.rs` — the one place that resolves paths, loads inputs, writes
  the committed pair, and `--check`s it.
- `scripts/atlas-drift.mjs` — the gate; steps in `scripts/check.mjs`, `.github/workflows/ci.yml` and
  `ci/teamcity/provision.mjs` named `atlas-drift`. Keep the four in step.
- `crates/ludion-engine/src/extract.rs` — `AtlasAsset`, `AtlasCache`, `load_atlas`,
  `atlas_color_path`, `resolve_atlas_frame`, `check_atlas_reference` (the seam `check-scene`
  re-enters through), and `flipbook_frame_name` (an entity's `Animation` clip resolved against the
  atlas's flipbooks, beside `animation_pose`'s assembly twin). `asset_cache.rs` is the hosted
  per-frame twin.
- `crates/ludion-engine/src/components.rs` — `Sprite`, `TileAppearance`, and
  `atlas_reference_diagnostics`, the one home of the three `L1151` rules.
- `crates/ludion-renderer/src/render.rs` — `SpriteInstanceRaw`, `build_sprite_draws`;
  `shaders/sprite.wgsl` — the unlit/lit branch.

## Dataflow

**Text → pixels.** `ludion_scene::check_sprite` parses and validates; `ludion_sprite::bake`
re-validates and paints. Nothing in `ludion-scene` bakes, and nothing in `ludion-sprite` reads a
file, so the two halves are independently testable and the CLI is the only stage that touches disk.

**The shared-palette include.** A sprite may carry `palette: { from, overrides }` instead of an inline
map (`kind: palette`, `ludion docs sprite-format`). `ludion_scene::resolve_sprite_include` merges
the two documents and reads nothing; the single IO half is
`ludion_engine::resolve_sprite_palette(sprite_path, doc)` in `sprite_sheet.rs`, which reads `from`
relative to the sprite's own directory and re-points the palette's own defects at `/palette/from`
(the report is headed with the sprite). Every stage that turns a sprite file into pixels calls it
before anything else sees the document — `check-scene`'s `run_sprite`, `generate_atlas.rs`'s input
load, and the contact sheet — so the bake, the packer and the drift gate never learn includes exist,
and an included sprite is byte-identical to an inline twin.

**Pack → commit.** `generate_atlas.rs` resolves each `inputs:` entry **relative to the source file's
directory**, bakes each `.yaml`/`.yml` sprite document and decodes each `.png` (whose whole image is
one region, pivoted at its centre), calls `pack`, emits the document with `emit_atlas`, and writes
`<stem>.atlas.yaml` + `<stem>.png` into `--out-dir` or the source's own directory. Before writing it
**re-parses and re-emits its own output** and refuses on a mismatch: a derived document is write-only
for authors, so nothing downstream would ever catch a non-canonical emit — the drift gate would just
fail forever on a file no edit could fix. `--check` byte-compares both files instead of writing.
`atlas-drift.mjs` is source-driven: it scans `games/` and `examples/` for `*.atlas-source.yaml` and
`*.font-source.yaml` (no manifest), spawns `generate-atlas --check` per source and parses the paths
the verb prints — the CLI is the only thing that knows how to regenerate *both* files. It then pairs
both ways with the baked documents' `generated_from:` (`scripts/drift-lib.mjs`, shared with
`map-drift` and `mesh-edit-drift`, which spawn their `--check` verbs the same way): a document at a
non-default path, or a verified one that lost its provenance, fails. Baked fonts share the verb and
the gate, against their `kind: font-source`.

**Commit → runtime.** `load_atlas(root, rel)` reads the document, `check_atlas`es it, and decodes
`maps.color` through `atlas_color_path` — the colour map is written relative to the *atlas document*,
so the pair moves together. The decoded image is **not** run through `Image::premultiplied`: the bake
already wrote premultiplied pixels, and premultiplying twice darkens every translucent texel again.
Loose `texture:` PNGs *are* premultiplied at load, in `extract::load_texture`. That asymmetry is the
one thing to remember at this seam.

`AtlasCache` holds each resolution for one placement-and-extract pass, keyed by document path,
**including failures** — so a missing atlas costs one failed read rather than one per quad, and
`place()` (which wants the region's pixel size to derive a quad with no authored `size`) and
`extract()` (which wants the UV rect and the image) cannot resolve to different documents.
`AtlasAsset::frame` is the one place a frame name becomes geometry:
`uv = [x, y, x+w, y+h] / packed image extent` and `size = [w, h] / pixels_per_unit`. The extent is
the **image's**, never the document's; a rect outside it is `L1153` from `check-scene`'s on-disk
pass, not silently clamped here. An atlas dedups through the same `path_index` a loose texture does
(an `*.atlas.yaml` and a `*.png` path cannot collide), so every quad of one atlas lands on one
`RenderScene::textures` entry.

**Runtime → renderer.** `SpriteInstance` carries `uv: [f32; 4]` and `lit: bool`.
`sprite_order` sorts by `layer` (the primary key), then back-to-front by view depth
(`total_cmp` places a NaN depth by its sign bit, so it can sort to either end and the platform
picks — render-only, the quad's vertices are NaN and it draws nothing; tie-break on scene order),
and `build_sprite_draws` then batches **consecutive runs of the same texture index**; `lit` rides
`SpriteInstanceRaw.flags` as `shader_gen::FLAG_LIT` (bit 0, hoisted from the WGSL `const`), so a
lit and an unlit quad of one texture stay in one batch and one pipeline. `sprite.wgsl` maps the
unit quad UV into the instance's sub-rectangle — the identity for `(0, 0, 1, 1)`, which is what an
untextured or whole-texture quad carries — and branches on the flag: unlit is
`texel.rgb * tint.rgb * tint.a`, lit multiplies that by the clustered-forward two-sided lobe.
The texel is premultiplied, so the tint's alpha scales colour as well as coverage.

## Rules
- **One region name, one mint site.** `<sprite document name>/<frame>` or a PNG's file stem, minted
  in `pack::collect`. A duplicate is `L1153` against `/inputs/<i>`. Nothing downstream re-derives a
  name.
- **The derived pair is regenerated, never edited.** Both files are committed; `atlas-drift` is what
  makes that a fact rather than a convention.
- **`check-scene` and `extract` resolve references through the same function.**
  `check_atlas_reference` is public for exactly that: a scene that checks clean cannot fail
  differently at extract.
- **The three `L1151` rules live in one function.** `atlas_reference_diagnostics` takes a
  `present(field)` closure and a pointer prefix, so `Sprite`, a screen-space `UiRect` and each
  `Tilemap` legend entry cannot disagree about what a half-written reference is.
- **`uv` is derived, never authored.** No component field carries it; it exists only from
  `AtlasAsset::frame` to the instance buffer.

## Where each code is raised
| Code | Site |
|---|---|
| `L1151` | `components.rs::atlas_reference_diagnostics` (schema semantics, `Sprite` + `UiRect` + legend), `extract.rs::atlas_unresolved`/`atlas_without_frame` (unreadable or unparseable atlas) |
| `L1152` | `extract.rs::frame_unknown`, suggestion from `AtlasDocument::nearest_region_name` |
| `L1153` | `scene/sprite.rs` + `scene/atlas.rs` (text), `ludion-sprite::structure_error` (defects only the bake sees), `cli/check_scene.rs::run_atlas` (colour map missing, rect outside the image), `cli/generate_atlas.rs` (`DRIFT`, and the emit self-check) |
| `L1154` | `scene/sprite.rs` — everything under `/effects/<i>` |
| `L4008` | `engine/sprite_sheet.rs::apply_sprite_overrides` — a `--set`/`--sweep` pointer outside `SPRITE_POINTER_SHAPES` |
| `L4014` / `L4015` | `ludion-sprite/import.rs`; the warning is printed by `cli/import.rs` |

## Test map
| File | Pins |
|---|---|
| `ludion-scene/tests/sprite_roundtrip.rs`, `atlas_roundtrip.rs` | value round trip, emit idempotence, the committed canonical fixtures, region name order |
| `ludion-sprite/tests/effects.rs` | each of the seven ops, exact texels, no tolerance |
| `ludion-sprite/tests/pack.rs` | the pinned layout, the gutter/extrude, the derived document |
| `ludion-sprite/tests/import.rs` | the lossless round trip, the quantized one, the `L4014` between them |
| `ludion-cli/tests/generate_atlas.rs` | the pair lands, both files are byte-deterministic, `--check` catches a drift in *either*, a broken `inputs:` entry points at itself |
| `ludion-cli/tests/check_scene_sprite.rs` | `kind:` dispatch, the atlas's on-disk pass, the scene pass over every `atlas` + `frame` |
| `ludion-cli/tests/render_sprite.rs`, `import_sprite.rs` | the CLI surface: flag conflicts, the whole-number `--scale` refusal, the stdout echo, defaulted `--out` |
| `ludion-engine/tests/sprite_atlas.rs` | one `textures` entry per atlas, the per-instance UV, the region-derived quad size, the schema refusal of a half-written reference |
| `ludion-engine/tests/sprite_flipbook.rs` | `Animation` selecting a flipbook frame is hash-neutral (identical `WorldHash` trace and `.lrp` bytes with and without extracting), a mid-clip time actually moves the UV *and* the quad extent, a one-shot holds its last frame, `hold` stretches its frame, an unbound clip and `speed: 0.0` degrade to the authored frame |
| `ludion-engine/tests/sprite_sheet.rs`, `tilemap.rs` | the two sheets' exact pixels; the legend on an atlas |

## Gotchas
- **Do not premultiply an atlas image at load.** It is already premultiplied; the second pass is
  invisible on opaque art and darkens everything translucent. The comment at `load_atlas` says so
  because the symmetric-looking `extract::load_texture` line right above it *does* premultiply.
- **A batch splits on the texture index, not on `lit`.** Adding a per-instance property that needs a
  different pipeline would split batches instead, which is the cost the flag lane exists to avoid.
- **The engine consumes `flipbooks` but not `filter`.** `filter` is carried on the atlas document —
  parsed, validated, emitted — and the sprite pass binds a nearest sampler for every atlas regardless.
  `flipbooks` is read at extract: `extract.rs::flipbook_frame_name` resolves an entity's `Animation`
  clip against `AtlasDocument::flipbook`, then `ludion_scene::flipbook_frame` picks the showing
  region, replacing the authored `frame` for both `place()`'s derived size and `extract()`'s UV rect.
- **`--out-dir` moves both files or neither**: `pack` derives the colour map's file name from the
  same stem, so writing the PNG elsewhere commits a document naming a file that is not there.

## Verify
```
cargo test -p ludion-sprite
cargo test -p ludion-scene --test sprite_roundtrip --test atlas_roundtrip
cargo nextest run -p ludion-engine --test it -E 'test(/^(sprite_atlas|sprite_flipbook|sprite_sheet|tilemap)::/)'
cargo nextest run -p ludion-cli --test it -E 'test(/^(generate_atlas|render_sprite|import_sprite|check_scene_sprite)::/)'
node scripts/atlas-drift.mjs
cargo run -p ludion-cli -- render --scene examples/gallery/sprite-atlas.yaml --out target/sprite-atlas.png
cargo run -p ludion-cli -- render --scene examples/gallery/sprite-tilemap-atlas.yaml --out target/sprite-tilemap-atlas.png
cargo run -p ludion-cli -- render --scene examples/gallery/sprite-flipbook.yaml --frames 23 --out target/sprite-flipbook.png
```
