# Plan: Sprite authoring tools — less to write, palette discipline, baked self-light

> Status: **planned 2026-10-31. Design settled 2026-10-31 (grilled, three rounds); no wave
> started.** A follow-on to `Plan/sprite-engine.md` (W1–W4 mechanized): that plan built the sprite
> layer, and this one makes the `kind: sprite` document cheaper to write and its bake truer to
> pixel-art practice. All four waves run now, in order. This line becomes the shipping log as
> waves land.

## Problem

The sprite layer is complete as an engine feature: documents, the bake, the packer, the drift gate,
flipbooks, the renderer. What it lacks is **authoring leverage and pixel-art discipline**, and both
cost the North Star metric directly:

- **Every texel and every colour is written out in full.** One palette colour is six YAML lines
  of linear floats (`ship.sprite.yaml` spends 26 lines on 4 colours). A symmetric sprite is
  authored twice, once per half, and the halves can drift. Animation frames repeat every row when
  only a few texels change. A recolour means copying the whole document.
- **The bake leaves the palette.** `gradient`, `shade` and `noise` produce continuous colours that
  no palette entry names. A palette is a set of chosen colours, so the result reads as an upscaled
  painting rather than pixel art. Nothing in the format expresses the pixel artist's basic tool, a
  **ramp** (a dark-to-light run of palette colours that shading steps along).
- **Nothing lights a sprite's body.** `glow` lands only outside the silhouette, `shade` is a
  one-texel edge bevel, and a `radial` `gradient` is centred on the target's bounding box and can't
  be placed. A cockpit or thruster can't light the hull around it.
- **There is no taste guidance.** The sprite topics document mechanics; nothing says what makes
  pixel art read well. `voxel-craft`, `generation-craft`, `sculpt-craft` and `blender-craft` all
  exist; `sprite-craft` does not.
- **The verify loop can't show a single effect's contribution.** The contact sheet shows the
  final bake. Answering "which op made this seam" takes `--sweep` guesswork.

## Research summary — the external shmup's sprite corpus (2026-10-31)

The external shmup is the sprite layer's real user: 27 `kind: sprite` documents, 58 frames, sizes
from 8×8 to 480×48, every one including a shared `kind: palette`.

- **Symmetry:** 16 of 58 frames are exactly left-right symmetric and 19 top-bottom symmetric, almost
  all of them bullets and effects. The enemies and avatars are mostly asymmetric (a side-scrolling
  shmup's ships face one way). Mirroring is a modest win for this corpus and a large one for
  characters, items and front-facing art.
- **Frame duplication:** multi-frame avatars and enemies change 1–14 % of their texels from the
  first frame. Their later frames are almost entirely repeated rows. Explosion frames change
  30–75 % and gain nothing from patches.
- **Effects in use:** `gradient` 28, `outline` 23, `shade` 15, `glow` 11, `dither` 1, `noise` 1.
  The off-palette ops are the most used, so ramp mode matters.
- **Light sources already exist as data:** the two shared palettes mark 15 glyphs `emissive`.
- **Migration precedent:** the shmup's open request 28 (breaking changes without a deprecation
  path) praises the scene format's legacy-spelling alias (`dynamic` still loads as `Kinematic`)
  and asks for that policy everywhere. Decision 1 follows it.

## Decisions (settled 2026-10-31)

### Colour spelling

1. **Colours in `kind: sprite` and `kind: palette` documents are spelled `"#rrggbb"` or
   `"#rrggbbaa"`.** RGB is sRGB, decoded to linear through `ludion_core::math`. Alpha is linear,
   `byte / 255`. This covers palette `color` and every effect colour (`from`, `to`, `color`).
   - **Reader:** case-insensitive; the short `#rgb` form is refused (`L1153`/`L1154`, naming the
     expected shape).
   - **Writer:** emits lowercase `#rrggbb` when alpha is `ff`, else `#rrggbbaa`. `ludion import`
     writes hex.
   - **Why hex:** the bake's output is 8-bit sRGB art. That is the reason `kind: texture` recipes
     already take hex (`ludion_texgen::ops::parse_hex`). Pixel artists think in hex, and it turns
     six lines into one.
   - **Legacy spelling:** the old four-float sequence **still loads, silently, as a legacy
     spelling**, the scene format's `dynamic` precedent. The external game isn't broken on pull,
     and only hex is ever written. Removing the alias is a separate, recorded decision.
   - **In-repo migration:** the gallery's sprite and palette documents are rewritten to hex by a
     one-off scripted text edit that keeps their comments. There is no migration verb; a request
     from the external game would be its trigger.
   - **Scope:** voxel palettes and materials keep linear floats (they feed HDR shading, not an 8-bit
     image).
   - **One parser:** texgen's hex parser and the sprite's become one shared function, alpha added,
     in a crate both already depend on.
2. **The `--set`/`--sweep` pointer domain addresses whole hex strings.**
   - `/palette/<glyph>/color` and an effect's `/from`, `/to` and `/color` take a hex string, and a
     sweep takes a list of them.
   - The per-component pointers (`/palette/<glyph>/color/<0..3>`, `/effects/<i>/from/<j>`) are
     removed. A pointer addresses what is in the file, and a float component would write a value the
     file can't hold.
   - `/effects/<i>/light/<j>` and `/effects/<i>/offset/<j>` stay.

### Less to write

3. **`mirror: x | y | xy` on a frame that authors `rows:`.**
   - The frame authors the kept half: with `x`, rows are `ceil(width / 2)` characters, and an odd
     width's authored half includes the shared centre column. `y` halves the rows the same way.
   - The bake expands the frame before anything else, so the result is symmetric by construction,
     not by eye.
   - Effects run on the full frame, so lighting stays asymmetric: a symmetric shape with a
     directional bevel is correct pixel art.
   - A row of the wrong width is `L1153`, and the message names the half-width it expected.
4. **Frame patches.** A frame may author `base: <frame>` plus `patches:` in place of `rows:`.
   - Each patch is `{ at: [x, y], rows: [...] }` in the base frame's full (expanded) coordinates.
   - In a patch row, a **space** keeps the base texel, `.` writes transparent, and a glyph writes
     that glyph. Space is already outside the glyph range (`!`–`~`), so this reserves nothing and
     shrinks no budget.
   - `base` must name a frame that authors `rows:` (no chains).
   - A patched frame never authors `mirror`. A patch applies to its base's expanded frame and may
     break its symmetry, a scar on one side for example.
   - A patch outside the frame, a `base` naming no `rows:` frame, or `mirror` on a patched frame is
     `L1153`.
5. **Recolour variants live on the atlas source.**
   - An `inputs:` entry may be a mapping `{ path, name, palette }`. `palette` is an overrides map
     with exactly the include's `overrides` semantics, and `name` sets the minted region prefix
     (`ship-red/idle`).
   - The plain path string stays the no-options case.
   - The sprite document doesn't change, so one document of rows yields any number of palette
     variants, and a duplicate minted name is the existing `L1153`.
   - A variant's flipbooks are explicit `flipbooks:` entries; nothing is derived. Flipbook names are
     free-form, so derivation would need a naming rule.

### Palette discipline

6. **`ramps:`** is a list of strings, each a dark-to-light glyph sequence (`- "kDdu"`). It can sit
   on a sprite document or on a `kind: palette`.
   - A sprite's own `ramps:` **replaces** an included palette's; absent, it inherits them.
   - Every glyph must exist in the merged palette, and a ramp needs at least two glyphs (`L1153`).
   - A glyph may sit in several ramps (the darkest outline colour usually ends every ramp). Stepping
     *from* a glyph uses the **first ramp that lists it**.
7. **Ramp mode** is `mode: ramp` on `shade`, `gradient` and `light`. `mode: color` is the default
   and is today's behaviour.
   - A ramp-mode op steps texels along their ramp and repaints them from the palette, so its output
     is always a palette colour.
   - **`steps:`** is one signed integer key on all three ops:
     - `shade` moves the lit edge up and the dark edge down by `steps`;
     - `gradient` runs from 0 at its start to `steps` at its end (negative is darker);
     - `light` moves a receiver up by at most `steps`.
   - Steps are whole. Where a falloff lands between two steps, the 4×4 Bayer matrix the `dither` op
     already uses decides per texel.
   - **The glyph plane:** the bake carries a per-texel **current glyph** plane, separate from the
     immutable authored grid that the `glyphs:` filters test. Ramp-mode ops read it and write it,
     and `dither` updates it (its A→B swap is a palette operation). A texel with no glyph (one an
     `outline` or `shadow` created) is skipped.
   - **Ordering is enforced:** a ramp-mode op repaints from the palette and would discard earlier
     colour work, so every ramp-mode op must precede every colour op in `effects:`, or it is
     `L1154`. `dither` is the one exception: because it tracks glyphs, it may come before ramp ops,
     so a dithered two-tone material can then be shaded.
   - A ramp-mode op in a document with no ramps, or on a target glyph in no ramp, is `L1154`.
   - A colour key on a ramp-mode op (`light`'s `color`, `gradient`'s `from`/`to`) is `L1154` with
     the nearest match, never silently unused.

### Baked self-light

8. **`op: light` lights a sprite's own texels from its light-source glyphs.**
   - **Sources** are every glyph with `emissive > 0` in the merged palette, or an explicit
     `sources:` glyph list that overrides them. A document with neither is `L1154`.
   - **Receivers** are the opaque texels within `radius_px` (fractional, `0 < r <= 32`) of a source
     texel, narrowed by the ordinary `glyphs:` filter.
   - **Falloff** is `glow`'s parabolic `1 - d²/r²`, one shared function. Several sources combine by
     per-channel **max**, so overlapping lights don't blow out.
   - **Light is anchored to glyphs:** it follows its sources in every frame, so a cockpit that
     shifts in a bank frame takes its light with it, and a mirrored or patched frame lights itself
     correctly.
   - Sources themselves are left untouched, as `glow` leaves its core.
   - **Blend:** additive only. `multiply` toward an `ambient` floor is deferred: it darkens one sprite
     against neighbours under the same scene light, which is scene mood.
   - **Parameters:** `radius_px`, `strength` (`>= 0`), optional `color` (absent, each texel takes its
     strongest source's colour), optional `bands: N` (the falloff quantized into crisp rings), and
     `mode: color | ramp` (ramp mode takes `steps:`, decision 7).
   - **Not scaled by `emissive`:** that value is an HDR render intensity, and coupling the bake to it
     would make a renderer tuning change silently re-bake every atlas. The bake reads `emissive` only
     to choose sources.
   - **No occlusion:** on a sprite-sized frame, self-shadowing reads as noise.
   - **Unlit sprites:** the op is for `lit: false` sprites (the default, and every bullet). A
     `lit: true` sprite would be lit twice, so `sprite-craft` says to keep baked light to emissive
     self-light there.
9. **Light from the environment stays the renderer's.** A room's lamp, a muzzle flash, or the sun on
   a tile is a `PointLight` or `SpotLight` and `lit: true`. Baked into a sprite, it would move with
   the sprite and repeat on every tile. No scene-scale baked lighting, no blocker layers.

### Seeing the work

10. **`ludion render --sprite <doc> --steps`** composes, per frame, the authored paint followed by
    the image after each effect, cumulatively. Each cell is captioned with that effect's echo line.
    - `--frame <name>` narrows it to one frame. It reuses the sweep sheet's 16-cell cap and works
      with `--set`.
    - Combining it with `--sweep` is refused with the CLI's existing flag-conflict diagnostic.
    - CPU-only, like every sprite sheet.
11. **`ludion check-scene <sprite doc> --rows`** prints each frame's **resolved** glyph grid (mirror
    and patches applied) to stdout, with a column ruler above and row numbers down the left. It's
    cheaper than an image for exact texel fixes, and it is the check that a mirror or a patch
    produced what was meant.
12. **Contact-sheet legibility:** block A draws a checkerboard behind transparent texels. At an
    upscale of 6 sheet pixels per texel or more it also draws a one-pixel texel grid, every eighth
    line darker. A transparent texel and the background then can't be confused, and positions can
    be counted across the whole tile, not only along the first tile's ruler.

### Guidance

13. **A `sprite-craft` docs topic**, the sprite sibling of the other craft topics:
    - **Marks need a cause:** no filler noise; every mark on a surface needs a cause you could name
      (construction lines, courses with offset joints, streaks under ledges, grime rising from the
      ground, rust under brackets, cracks from a corner). A flat region beats a speckled one, and
      `noise` is for materials that really are granular, sparingly.
    - **Shades:** 2–4 per material, along ramps.
    - **Glyph naming:** mnemonic glyphs (lowercase light, uppercase dark of one hue).
    - **Repetition:** write half of anything symmetric; recolours are variants, small frame changes
      are patches.
    - **Effects:** outlines and light come from ops, never painted.
    - **Order of work:** silhouette first, judged at block B's game scale.
    - **Ramps vs colour mode:** when to choose a ramp over a colour-mode gradient.
    - **Light:** self-light from emissive glyphs vs scene light from the renderer.

    It's written with the `ludion-docs` skill and grows a section per wave.

## Waves

| Wave | Content |
| --- | --- |
| **1. Guidance and seeing** | `sprite-craft` (decision 13, current ops), `render --sprite --steps` (10), `check-scene --rows` (11), contact-sheet checkerboard and grid (12). No format change. |
| **2. Less to write** | Hex colours with the legacy spelling (1), the pointer domain (2), `mirror` (3), frame patches (4), atlas-source variants (5). `--rows` shows the resolved grid. The in-repo gallery documents are rewritten to hex by script; their atlases re-bake. Docs: `sprite-authoring`, `sprite-sheets`, `sprite-craft`. |
| **3. Palette discipline** | `ramps:` (6), ramp mode on `shade` and `gradient` with the current-glyph plane, `dither` on the plane, and the enforced ordering (7). A gallery document in ramp mode, beside its colour-mode twin. Docs: `sprite-effects`, `sprite-authoring`, `sprite-craft`. |
| **4. Baked self-light** | `op: light`, additive, in colour and ramp modes (8). The gallery ship lit from its cockpit and thrusters. Docs: `sprite-effects`, `sprite-craft` (with decision 9's boundary). |
| **Exit** | An authoring round re-authors the gallery's sprites with the new tools and records each document's line count before and after in Status (recorded, not targeted). The human look verdict (ramp mode and `light` beside their colour-mode twins) goes in `Plan/owed.md`. The external game's adoption is its own and doesn't gate the exit. |

## Codes

No new `L####` code is planned. Document structure (hex shape, mirror widths, patches, ramps) is
`L1153`; effects (hex shape, ramp mode, ordering, `light`) are `L1154`; palette includes are
`L1147`; the pointer domain is `L4008`. If implementation finds a case none of them states honestly,
it allocates from `crates/ludion-core/src/codes.rs` then, one `codes.rs` subtask at a time.

## Risks

- **Hex quantizes colours.** A float colour rewritten to hex moves by up to half an 8-bit step
  before effects run, so a migrated document's atlas can differ by a byte wherever an effect
  modulates the colour. The drift gate catches every such atlas. Re-bake, and re-bless any golden
  that moves (each bless announced).
- **The current-glyph plane is new bake state.** It must stay a pure function of the document, and
  the cross-platform byte-identity of every committed atlas must hold.
- **Sheet goldens move in Wave 1** (checkerboard, grid). Re-bless them on the pinned boxes only, as
  `Plan/sprite-engine.md`'s W1 ruling requires.

## Out of scope (with triggers)

- **Stock palettes** (`from: "std:<name>"` — PICO-8, Sweetie 16, DB16, Endesga 32): no current
  consumer needs them, index glyphs fight the mnemonic-glyph guidance, and each palette's license
  needs checking. Trigger: a new game starts without a palette of its own.
- **`multiply` blend with an `ambient` floor on `op: light`**: trigger, an authoring round asks for
  baked darkness on a sprite.
- **Normal maps for sprites** (relief that `lit: true` shades): the atlas's `maps:` key was built to
  take another map. Trigger: the slice needs lit, relief-shaded sprites.
- **A palette `glyph:` in place of `color:` on `outline`, `shadow` and `glow`**, so a palette change
  follows: trigger, an authoring round asks for it.
- **Derived variant flipbooks**: trigger, a variant-heavy authoring round complains about the
  duplication.
- **A `ludion sprite migrate` verb**: trigger, the external game asks to migrate its documents.
- **`ludion import` detecting symmetry and writing `mirror:`**: trigger, imports of symmetric art in
  volume.
- **Procedural or expression-driven frames**: never. Rust is the only Turing-complete layer.
- **Scene-scale baked lighting**: never (decision 9).
