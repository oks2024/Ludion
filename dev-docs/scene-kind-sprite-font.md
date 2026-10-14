---
description: kind: font/font-source and sprite/palette/atlas-source/atlas text layers in ludion-scene
---

# ludion-scene: font, sprite and atlas documents

**Scope:** Changing `font.rs`, `font_source.rs`, `sprite.rs`, `palette.rs` or `atlas.rs`. Hub: `ludion devdocs scene-kind-documents`; bake side: `ludion devdocs ludion-font`, `ludion devdocs ludion-sprite`.

## Rules

### Font documents (`kind: font`)

`font.rs` owns the text half of a face: integer font-unit metrics, a glyph table strictly ascending
by code point, `aliases`, sparse `kerning` and `missing`, all under `L1250`. Chars are single-char
strings and go through `scalar_string`, so `" "`, `":"`, `"-"` and `"1"` quote; a bare `1` parses as a
number and is rejected. The strict YAML subset forbids non-empty flow collections, so rects and bounds
are block sequences. Lookups are `ludion devdocs ludion-font`'s.

`font_source.rs` is the authored bake input beside `atlas-source` (`L1251`): `file`, `mode`, `em_px`,
`spread_px` (required iff `sdf`, `1..=em_px`, rejected for `bitmap`) and a `charset` of named ranges
(`ascii`, `latin-1`, `general-punctuation`; an unknown keyword gets a nearest-match help) plus loose
`chars`. `charset.ranges` is a block list; emit sorts ranges by keyword and dedupes, `chars` stays as
authored. It reads no file — `ludion devdocs ludion-font`'s bake does.

### Sprite and atlas documents (`kind: sprite` / `palette` / `atlas-source` / `atlas`)

`sprite.rs`, `palette.rs` and `atlas.rs` own the text halves of the sprite pipeline
(`Plan/sprite-engine.md`).
A sprite document is the 2D sibling of a voxel model: one `size` in texels, a glyph `palette` (`.`
reserved for transparent), an **ordered** mapping of named `frames` all of that one size, and a
doc-level `effects:` list. An atlas source lists the inputs one packed image is built from and the
flipbooks it carries; an atlas is derived, written by `ludion generate-atlas`, and committed. Nothing
here bakes — this crate owns the text and `ludion-sprite` turns it into pixels against these types.

Five things not recoverable from the signatures:

- **A palette can live in its own document, and the include is resolved outside this crate.** A
  `kind: palette` document (`palette.rs`) is the sprite's glyph map with an envelope around it, parsed
  and validated by `sprite.rs`'s own entry routines under `L1147` so the two families cannot disagree
  about what a glyph is. A sprite then carries *either* an inline `palette:` map or an include —
  `palette: { from, overrides }`, told apart by the `from` key, which no one-character glyph can spell
  — and an unresolved include suspends only the frame/effect glyph-membership checks, which the
  resolved document faces on the way to the bake. `resolve_sprite_include` merges the two documents
  and reads nothing; the IO half is `ludion_engine::resolve_sprite_palette`.

- **`effects:` is a closed op set, and `L1154` is its own code.** `gradient`/`outline`/`shade`/
  `shadow`/`dither`/`noise`/`blur`, applied in list order. `op` is read *first* in `parse_effect`
  because it decides the entry's whole key set — a `radius` on a `gradient` is an unknown key with a
  nearest match over the gradient's keys, not a stray field. The code split is positional: the
  `effects` key's own shape is `L1153` (document structure), everything under `/effects/<i>` is
  `L1154`.
- **Canonical means *minimal* here, unlike voxel.** A defaulted field equal to its default is not
  written (`direction`, `blend`, `side`, `light`, `strength`, `offset`, a shadow's `color`,
  `density`, an atlas's `filter`, a flipbook's `loop`), and an empty `effects`/`flipbooks` list is
  not written at all. So `emit(parse(x)) == x` holds for *canonical* x while a hand-written document
  spelling a default out loud re-emits shorter once and then holds; the always-true property is
  byte-idempotence.
- **What the text layer deliberately refuses to check.** An atlas source does **not** resolve its
  flipbook frame names — it has not read the sprite documents its `inputs` point at, so only the bake
  can. An atlas **does** resolve them, against its own `regions`. Region *overlap* is never checked:
  the packer owns the layout, and one rect aliased under two names is legal content. Input-file
  existence and `generate-atlas --check` drift are the bake's, reported under the same `L1153`.
- **`AtlasDocument.regions` is a `BTreeMap` because name order *is* canonical order**, so a packer
  change moves rects without reshuffling the file — the property the `atlas-drift` gate leans on.
  `region()` plus `nearest_region_name()` are the pair a `Sprite`'s `frame` lookup reports as
  `L1151` (the atlas reference itself does not resolve) and `L1152` (it resolved, but has no such
  region — with a did-you-mean).


## Verify

```
cargo test -p ludion-scene
```
