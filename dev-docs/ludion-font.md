---
description: Font faces: the kind: font document's lookups and the embedded 8x8 face
---
# ludion-font

**Scope:** the pure crate over a `kind: font` document — `FontFace` (alias-resolved glyph, advance and
kerning lookups), `layout()`, and the embedded 8×8 bitmap face. Every text path — `measure_ui_text`,
`wrap_ui_text`, extract's `UiText`/`UiList`/`Text` quads, `bounds.rs` — goes through `layout()`; the
renderer draws the `GlyphQuad`s verbatim and knows no character. The text format is `ludion_scene`'s
(`ludion devdocs scene-kind-documents`, `L1250`). Sim-tier determinism applies
(`ludion devdocs determinism`): sorted `Vec`s with binary search, no hash maps.

## Files
- `crates/ludion-font/src/lib.rs` — `FontFace`, `FontFace::embedded()`, `EMBEDDED_PNG`.
- `crates/ludion-font/src/stamp.rs` — `stamp_label()`, `LABEL_CELL`, `LABEL_PITCH`: the CPU label stamp.
- `crates/ludion-font/src/layout.rs` — `layout()`, `TextLayout` (`extent()`, `quads()`), `YAxis`.
- `crates/ludion-font/assets/default.font.yaml` + `default.png` — the embedded face; the renderer
  uploads the PNG as its text atlas. Two rows, 62 glyphs, each `rect = [col×8, row×8, 8, 8]`; a glyph's
  column is not its code-point index. Space has a glyph and rect; `notdef` has an advance and no rect,
  so unknown chars draw blank.
- `crates/ludion-font/tests/embedded_face.rs` — the second row pinned against the bitmaps it replaced,
  the one-full-cell invariant, and the stamp.
- `crates/ludion-font/tests/legacy_cell_parity.rs` — the old per-character loops frozen as oracles.

## Layout
`layout(face, text, size, wrap_width) -> TextLayout` shapes lines; `extent()` measures them and
`quads(origin, draw_size, axis)` positions the inked glyphs (draw size is separate from the layout size).
`layout` breaks lines at `\n`, `\r\n` and a lone `\r` and lays a tab out as a space (one `normalized` pass);
`fold_line_breaks` turns every break into a space, for text that must stay on one row (a `UiList` line).
Parity with the pre-font square-cell paths is **bitwise** for the 8×8 face (`legacy_cell_parity.rs`
for quads, `tests/font_layout_parity.rs` in `ludion-engine` for wrap/measure; the one intended
difference is the hyphen break), so the arithmetic is pinned — a moved golden is a bug, never a re-bless:
- `unit = size / line_units`, computed once; a conversion is `units as f32 * unit` (integer sum first).
- Extent is `[widest_units * unit, line_count * size]`; the line height is `size` itself.
- The pen walks sequentially: `pen_x += advance_units * unit` (kerning folded into the advance, one
  add), lines step `pen_y ± size`. Never recompute a position from a line start.
- Wrap counts advance units: `limit = floor((wrap_width / size) * line_units)`; a line takes at least
  one glyph; `\n` is a paragraph break; space runs collapse and edge spaces drop only when wrapping; an
  over-long word hard-breaks. A word also breaks after `-` and `—`, never at U+00A0.
- Empty text, or text that wraps to nothing, has zero lines (extent `[0, 0]`).

## Label stamp
`stamp_label(atlas, canvas, size, at, text, scale, color)` burns a label into a CPU RGBA8 canvas: each
texel of a glyph's rect with alpha ≥ 128 becomes a `scale`×`scale` block, clipped to the canvas, and the pen
advances `LABEL_PITCH * scale` (9: the 8-texel cell plus one of tracking) per character, mapped or not.
The caller passes the decoded `EMBEDDED_PNG` (this crate ships the bytes without decoding). Every CPU
sheet label goes through it: the voxel/GI, sprite, atlas and font sheets, and `ludion-gen`'s contact sheet.

## Rules
- The embedded face is authored: no `generated_from:`, no generator. Grow it only through
  `ludion_scene::emit_font` + `ludion_assets::encode_png` (a throwaway `#[ignore]` test, deleted after
  one run), append into a new atlas row so every existing rect and `u` stays put, and pin the new glyphs
  in `tests/embedded_face.rs`. Never hand-edit the YAML or PNG.
- `include_str!`/`include_bytes!` targets stay inside the crate, or `cargo package` fails.
- The renderer's atlas test (`the_embedded_atlas_png_matches_the_face_and_carries_coverage`) pins the
  PNG to the face's declared atlas size; changing either asset without the other fails it.

## Bake (feature `bake`)
`src/bake/` reads a `.ttf`/`.otf` into `FaceOutlines` (metrics, flattened outlines, pair kerning, no
pixels); only `ludion-cli` enables it, the engine never parses a font. Failures are `L1252`.
- Outlines are drawn unscaled and unhinted, and curves flatten with a fixed step count (8 quad, 12
  cubic) using only `+ - *` in `f32`, so the polyline is identical on x86 and ARM. Never make it adaptive.
- Kerning shapes every ordered char pair through harfrust (LTR, Latin set explicitly, never guessed);
  a pair counts only when exactly the two cmap glyphs come back, adjust = shaped advance − hmtx advance.
  Ligating or substituting pairs are skipped, not kerning.
- `skrifa` and `harfrust` are pinned with `=` to versions sharing one `read-fonts` (0.43.x); skrifa 0.47
  would duplicate it. Moving either pin moves bake bytes.

- `bake(source, font_bytes, generated_from, stem) -> FontBake { doc, image, png }` is pure and byte-
  deterministic. Atlas pixels are RGBA8, RGB 255, the value in alpha, both modes (the embedded face's
  convention). Texel box per inked glyph: `floor(x0*em_px/upem) - pad .. ceil(x1*em_px/upem) + pad`
  in `i64` (`pad` = `spread_px` for sdf, 0 for bitmap); the doc's `bounds` are that box mapped back
  (rounded half away from zero), so layout draws the quad over exactly the rect.
- `line_height` (font units, optional on the source) moves the ascender: `ascender = line_height +
  descender - line_gap`, so a pixel face lands on whole pixels at multiples of its line height; the
  atlas and glyph `bounds` are untouched. An ascender ≤ 0 is `L1251` at `/line_height`.
- SDF: exact distance to the flattened segments, sign from the nonzero winding number;
  `byte = 0.5 + d/(2*spread_px)` clamped, edge 127.5. Bitmap: signed-area coverage, no thresholding
  (Silkscreen's inset counters read ~10/~20, not 0). Only `+ - * /` and `sqrt` — never `mul_add`.
- Packing: inked glyphs by code point, `.notdef` last; 1-texel gutter, width = multiple of 4 holding
  the widest rect and `isqrt(area)`, left-to-right shelves (the shared `ludion_assets::shelf::place`);
  over 4096 a side is `L1251`. 4096 is a deliberate, stricter bound than the loader's
  `MAX_PNG_SIDE`, asserted `<=` it at compile time.
- Every glyph's texel box is sized and packed before any pixel is drawn, so raster work is bounded
  by the 4096² atlas: a box over 4096 a side (or unrepresentable) is `L1251` at `/em_px`, and a
  charset with nothing to ink is `L1251` at `/charset`.
- SDF distance counts only boundary segments: an edge inside another same-wound contour (overlapping
  composites like `Ç`, `Ø`, `Ð`) has distance 0 but is not the outline and would draw a seam. The
  sign is still the full nonzero winding.
- A cmap entry at or past `maxp.numGlyphs` is a missing char, not a zero-advance glyph.
- The Inter ASCII and Silkscreen PNG hashes in `tests/bake_atlas.rs` are the cross-arch canaries;
  moving one is a bake change.

## Faces at runtime
`ludion_engine::FontLibrary` (`crates/ludion-engine/src/fonts.rs`) resolves a component's optional
`font` name through the content root's `fonts.yaml` (`kind: fonts`, `L1253`; the reserved name
`default` is the face for unnamed text, else the embedded 8×8). It loads the whole table eagerly when a
world is built — extract holds only `&World` — and is never hashed. An unknown name or failed face falls
back to the default face; `report_font_faults` raises `L1254`/`L1255` once each, and `check-scene`
(`scene_font_diagnostics`) errors on unknown names and warns `L1256` on chars a table face lacks.
The `ui` measurement helpers take the resolved `&FontFace` first; there are no default-face shorthands.
Extract resolves each component's face through the world's `FontLibrary` (a bare `extract()` loads the
table from the content root), lays out with it, and — for a table face — resolves its atlas PNG through
the shared texture dedup into a `FontDraw`; a missing atlas is `L1101`/`L1102` at the component's `/font`.
`asset.reload` of `fonts.yaml` or a held face document rebuilds the library (`Engine::reload_fonts`),
refused with `L1255` while a recording is armed; a re-bake changes the PNG too, which reloads through the
asset cache.

## Verify
`cargo test -p ludion-font`, `cargo test -p ludion-renderer --lib atlas`, `cargo nextest run -p ludion-engine --test it -E 'test(/^font_layout_parity::/)'`, `cargo test -p ludion-scene --lib font`, `cargo test -p ludion-engine --lib fonts`, `cargo test -p ludion-cli --test check_scene_fonts`, `cargo test -p ludion-font --features bake`, `cargo test -p ludion-cli --test generate_atlas_font`.
