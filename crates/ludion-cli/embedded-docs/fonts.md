---
description: Baked TrueType faces for Text/UiText/UiList: font-source, generate-atlas, fonts.yaml, font field
---

# Fonts

**Scope:** drawing text with a real face instead of the embedded 8×8 bitmap — authoring and baking a
face, naming it in the content root's `fonts.yaml`, and putting it on `Text`/`UiText`/`UiList`. The
components' other fields are `ludion docs components-ui`; the canvas and wrapping are `ludion docs screen-ui`.

## The loop

1. Commit the `.ttf` **and its licence file** beside it (`fonts/Inter-LICENSE.txt`). A face is only ever
   an open-licensed file in the repo — never a system font, so every machine draws the same pixels.
2. Author `fonts/<name>.font-source.yaml`.
3. `ludion generate-atlas fonts/<name>.font-source.yaml` writes `<name>.font.yaml` (metrics, kerning,
   glyph table) and `<name>.png` (the atlas). Both are generated: never hand-edit them.
   `generate-atlas --check` byte-compares both and is the drift gate.
4. `ludion render --font fonts/<name>.font.yaml --out specimen.png` draws a CPU-only specimen sheet
   (atlas, metric band, kerned vs unkerned lines, charset, pangram); `ludion check-scene` on either
   document prints a one-line summary.
5. List the face in `fonts.yaml` and name it with `font:` on a component.

## Format

```yaml
# fonts/inter.font-source.yaml
format: 1
kind: font-source
name: inter
file: Inter-Regular.ttf
mode: sdf            # sdf (smooth, scalable) | bitmap (pixel faces)
em_px: 56            # bake size, 1..=256
spread_px: 6         # sdf only — required for sdf, rejected for bitmap, <= em_px
line_height: 1250    # optional, font units: replaces the ascender so the line height is exact
charset:
  ranges:
    - ascii
    - latin-1
    - general-punctuation
  chars: "×—…‘’“”"   # extra characters on top of the ranges
```

```yaml
# fonts.yaml at the content root
format: 1
kind: fonts
fonts:
  default: fonts/inter.font.yaml    # optional; the face for any text with no `font`
  body: fonts/inter.font.yaml
  pixel: fonts/silkscreen.font.yaml
```

`font: body` is an optional field on `Text`, `UiText` and `UiList`. Without a `default` entry, text
with no `font` keeps drawing with the embedded 8×8 face, so adding the table moves no existing scene.
`default` is reserved: setting it re-faces every unnamed text in the project.

## Rules

- **`size` is the line height** (canvas units for UI, world units for `Text`), whatever the face, and must be
  positive (`L1118`); width follows the face's proportional advances. A character the face lacks draws the face's `.notdef`
  glyph (a box; the embedded 8×8 face draws it blank) — it never crashes. The embedded face covers
  A–Z (lowercase folds to uppercase), 0–9, space and `! " # % ' ( ) * + , - . / : ; < = > ? @ [ \ ] _ ~`.
- **Line breaks.** `\n`, `\r\n` and a lone `\r` break lines in every text path, a tab lays out as a
  space, and a `UiList` line folds its breaks to spaces.
- **Charset is baked**: only `ranges` + `chars` get a glyph. Text outside it is `L1256` (a
  `check-scene` warning, for table faces only); add the characters to `charset.chars` and re-bake.
  A charset holds at most 4096 characters (`L1251`).
- **sdf vs bitmap.** `sdf` stays smooth at any size. `bitmap` is for pixel faces and is crisp only at
  integer multiples of the source pixel: set `line_height` so one source pixel is a whole number of
  font units (Silkscreen: `em_px: 8`, `line_height: 1250` → 10 source pixels per line → crisp at
  `size` 10, 20, 30…). At 1080p, size 30 is exactly 3×3 screen pixels per source pixel; at half
  resolution the same text lands on 1.5 and is uneven.
- **Outline.** An SDF face outlines up to its `spread_px` less one screen pixel; a bitmap face dilates by whole source pixels (1 to 4), so an outline is crisp where the face is. Past either limit it clamps with `L2304` (`ludion docs screen-ui`).
- **Small SDF text** (≈12 px on screen) loses thin strokes; keep body text at 14 px or more.
- **Measure with the face you draw with.** Every helper — `measure_ui_text`, `wrap_ui_text`,
  `measure_ui_list`, `list_hit`, `resolve_ui_text`/`resolve_ui_list` — takes the resolved `&FontFace`
  first: `let lib = world.resource::<FontLibrary>().cloned().unwrap_or_default(); lib.face(Some("body"))`.
  A list's hit test then matches its drawn rows exactly, proportional or not (`cargo run -p ludion --example font_text`).
- **The table loads once, when the world is built.** `asset.reload` of a face document rebuilds it,
  and is refused with `L1255` while a recording is armed — measurement is sim-visible, so a mid-recording
  face swap would change replay hashes.
- **Generated files are committed and drift-gated** — `generate-atlas --check` runs in CI.

## Diagnostics

| Code | Meaning |
|---|---|
| `L1251` | `font-source` malformed (`em_px` range, `spread_px` vs mode, bad charset range, non-positive `line_height`, a charset over 4096 characters) |
| `L1252` | the `.ttf` cannot be read or baked (a glyph flattening past 16384 segments is refused) |
| `L1253` | `fonts.yaml` malformed |
| `L1254` | a `font` name is not in the table (a `check-scene` error; runtime reports once per name) |
| `L1255` | a listed face fails to load, or a reload is refused while recording |
| `L1256` | authored text has characters the face lacks (warning) |

## Next

- `ludion docs screen-ui` — the canvas, anchors, draw order and wrapping.
- `ludion docs ui-list` — the menu widget and `list_hit`.
- `ludion docs components-ui` — the component fields.
