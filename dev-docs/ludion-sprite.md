---
description: Offline sprite baking: glyph grids to pixels, the seven CPU effects, the atlas packer, PNG import
---
# ludion-sprite

**Scope:** the crate that turns sprite text into pixels and back — `bake()` (a `kind: sprite`
document to one image per frame, effects applied), `pack()` (frames plus loose PNGs to a packed image
and the derived `kind: atlas` document), and `import_png()` (a decoded PNG back to an editable
document). The *text* halves of all three families live in `ludion-scene`
(`ludion devdocs scene-kind-documents`); the pipeline end to end, from text to the renderer, is
`ludion devdocs sprite-pipeline`. The formats are `ludion docs sprite-format` and
`ludion docs sprite-sheets`.

## Layout
- `crates/ludion-sprite/src/lib.rs` — the three entry points re-exported in pipeline order, plus
  `structure_error`, the `L1153` this crate raises for the defects only a bake can see (a duplicate
  region name, an unresolvable flipbook frame, an image too large to be a sprite).
- `crates/ludion-sprite/src/image.rs` — `FrameImage`, the working format, and the one sRGB transfer
  pair (`srgb_encode`/`srgb_decode`).
- `crates/ludion-sprite/src/bake.rs` — painting the glyph grid and the seven ops, one private
  function each, plus `texel_random`/`splitmix64`.
- `crates/ludion-sprite/src/pack.rs` — `AtlasInput`/`AtlasInputFrame` in, `AtlasBake` out: name
  minting, the flipbook cross-check, the shelf layout, the composite and the PNG encode.
- `crates/ludion-sprite/src/import.rs` — `IMPORT_GLYPHS`, the median cut, `QuantizeReport`.
- `crates/ludion-cli/src/generate_atlas.rs` — the only caller that does IO for `pack`; `render.rs`
  and `ludion-engine`'s `sprite_sheet.rs` are `bake`'s.

## Dataflow
`bake(&SpriteDocument)` re-runs `ludion_scene::validate_sprite` first and refuses on any diagnostic —
an out-of-range colour has no defined image, and a guess would land in a committed PNG. It then, per
frame: builds the **glyph grid** (`Option<char>` per texel), paints it from a 128-entry palette
lookup, and folds each `effects:` entry whose `frames:` filter admits the frame, clamping to `0..=1`
after each. Out comes a `BakedFrame` with the pivot already resolved to the document's default, so
the packer never reaches back to the document.

`pack(&AtlasSourceDocument, generated_from, &[AtlasInput])` mints every region name
(`base/frame`, or `base` alone for a PNG input), collects every defect rather than the first, checks
the source's flipbooks against the names actually produced, lays the entries out, composites them
with their gutters, encodes the PNG, and returns `AtlasBake { image, png, doc }`. `image.hash` is the
content hash of `png`, so a fresh bake and the committed file loaded back share one identity — which
is what lets the drift gate compare them.

`import_png(&Image, name, quantize)` counts distinct RGBA8 values (a fully transparent texel is `.`
and counts for nothing), refuses with `L4014` past `SPRITE_GLYPH_BUDGET` when `quantize` is `None`,
else median-cuts to the clamped target and reports the loss.

## Rules
- **Bit-reproducibility is the crate's whole contract.** A packed atlas is committed and CI
  regenerates it on Windows x86-64 and macOS arm64 and byte-compares (`generate-atlas --check`). So:
  `f32`/`f64` `+ - * /` and integer arithmetic only, every transcendental through
  `ludion_core::math`, no `mul_add`, no hash-container iteration, no reduction whose order depends on
  anything but texel order. Break any of it and every committed atlas re-bakes, silently, on one OS.
- **Randomness is addressed, never sequential.** `texel_random(seed, frame, x, y, channel)` hashes
  its address through `splitmix64` — there is no stream position to advance and nothing to get out of
  step, so a texel's value cannot depend on the order texels are visited or on how many ops ran
  before. `DeterministicRng` is the wrong tool here for exactly that reason. The mixer's constants
  are pinned by a unit test, because an accidental edit would otherwise surface as a golden failure
  three crates away.
- **`FrameImage` is straight alpha in linear light**, `[f32; 4]` per texel. Straight because an op is
  authored in terms of a texel's own colour (a gradient multiplies the colour, not the colour times
  its coverage); linear because averaging or lerping encoded bytes darkens the result by its own
  gamma; unquantized between ops because a blur reads neighbours. `get(x, y)` returns transparent
  black out of bounds — a *value*, not an error, since all four neighbourhood ops read past the edge
  and all four want that answer.
- **Premultiplication happens exactly once, on the way out.** `to_srgb8_premultiplied` weights RGB by
  alpha *in linear light* and then sRGB-encodes: the sprite pass blends `One, OneMinusSrcAlpha` and
  the sampler decodes sRGB before the blend sees it. `to_srgb8_straight` is the human-facing form
  (contact sheets, the import round trip), and it is an exact inverse of `from_srgb8_straight` at
  8 bits — which is what makes `bake(import(png)) == png` a testable claim.
- **An effect's `glyphs:` filter tests the *authored* grid, never the current pixels.** A texel an
  earlier op repainted keeps its authored glyph; a texel an op created out of transparency belongs to
  no glyph. Without the filter an op targets what is currently opaque — except `blur`, which targets
  the whole frame, because the spread into transparency is the glow. Change this and two ops naming
  one glyph silently mean different sets.
- **The packer's order is a total order.** Height desc, then width desc, then **name** asc; names are
  unique by construction, so no two entries compare equal and sort stability is never relied on.
  Position follows from size and name and nothing else, so a layout change is readable in a diff. A
  tighter packer that reshuffled on an unrelated edit would re-bake every committed atlas for
  nothing.
- **The gutter is an extrude, not padding.** `GUTTER = 1` on all four sides, filled by clamping the
  read into the region, so a `linear` sampler half a texel past an edge finds that region's colour.
  The written `rect` excludes it. Both axes round up to `EXTENT_ALIGN = 4` — a stride alignment,
  deliberately **not** a power-of-two requirement, which would waste up to half an atlas.
- **An atlas side is capped at `ludion_assets::MAX_PNG_SIDE` (8192).** A larger packed extent is
  `L1153` at `/inputs` before the pixel buffer exists; the loader would refuse the PNG later
  anyway. Placement is `ludion_assets::shelf::place`, shared with the font packer; sort order,
  gutter policy and width rule stay per crate because each moves committed bytes.
- **IO-free by design.** The crate reads no file and writes none: the caller resolves paths, loads
  inputs and writes results, so every function is a pure function of its arguments and unit-testable
  without a temp directory. `output_stem` honours the CLI's "derive from the file name" fallback
  without doing IO, because `generated_from` *is* that path.
- **`emissive` is not colour-map input.** `palette_lookup` reads `color` alone; the emissive value
  rides the document for a map that bakes from the same source.

## Gotchas
- **`ceil_sqrt` corrects its own float seed with integer loops.** The atlas width depends on it, so a
  rounding difference between platforms would move every rect. Do not "simplify" it back to
  `sqrt(x).ceil()`.
- **`signum` here is not `f64::signum`**, which reports `1.0` for `+0.0`: a `shade` light with one
  zero component would then bevel along an axis it does not point down.
- **`shade` and `shadow` read a clone of the image taken before the op.** A one-texel bevel and a
  rigid shadow offset both need the silhouette as it stood, or the effect floods.
- **`blur` averages in premultiplied space** and un-premultiplies afterwards; averaging straight
  colours pulls fully transparent texels' colour into the result and halos the silhouette.
- **The 93-glyph budget is the format's point, not a limitation to route around.** A grid whose
  glyphs run out stops being readable and editable as text, which is the only reason the document
  exists. `IMPORT_GLYPHS` is ordered heaviest-looking first so the text reads as a rough greyscale;
  a duplicate character there would silently merge two colours, so a test pins its length and
  uniqueness.

## Verify
```
cargo test -p ludion-sprite                      # effects (exact texels), pack layout, import round trip
cargo run -p ludion-cli -- generate-atlas --check examples/gallery/sprites/gallery.atlas-source.yaml
cargo run -p ludion-cli -- render --sprite examples/gallery/sprites/ship.sprite.yaml --scale 4 --out target/ship.sheet.png
cargo run -p ludion-cli -- render --atlas examples/gallery/sprites/gallery.atlas.yaml --out target/gallery.atlas-sheet.png
cargo run -p ludion-cli -- import crates/ludion-cli/tests/fixtures/atlas/spark.png --out target/spark.sprite.yaml
node scripts/atlas-drift.mjs                     # every committed pair, the CI gate
```
Effect tests assert **exact** texel equality, never a tolerance: the bake feeds a committed PNG, so
"close enough" is not a property this crate has.
