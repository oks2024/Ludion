---
description: What makes a sprite read at game scale: silhouette, marks, few shades, glyphs, ops
---

# Sprite craft

**Scope:** the art direction a valid sprite still needs, stated as checkable rules. The document
format is `ludion docs sprite-format`, the commands each rule names are `ludion docs sprite-authoring`, the ops are
`ludion docs sprite-effects`, and the layer end to end is `ludion docs sprites`.

A sprite that passes `check-scene` is *valid*, not *good*. These rules are what separates a sprite
that reads at ship size from a smudge; each names the command that says whether you followed it.

## Silhouette first

Author the silhouette as **one flat glyph** and judge it before any interior detail: render at ship
size and read block B (the true-size row) of the sheet.
```
ludion render --sprite <doc> --scale 4 --out target/sprite.sheet.png
```
If the shape does not read as a flat blob at that size, no interior mark will rescue it. Only when
the silhouette holds do you split regions into glyphs.

## Marks need a cause

Every mark on a sprite has a cause you could name: construction lines, courses with offset joints,
streaks under ledges, grime rising from the ground, rust under brackets, cracks from a corner. A
mark with no cause is filler noise, and it reads as a dirty sprite at small scale. A flat region
beats a speckled one. The `noise` op is for granular materials
(rock, sand, cloth) and sparingly; it jitters dark glyphs badly, see `ludion docs sprite-effects
--section "Gotchas"`.

## Shades: 2-4 per material

Give each material a hand-chosen dark-to-light run of **two to four** palette colours. More than that
reads as a painting rather than pixel art, and the extra colours blur the silhouette the first rule
protects. Choose the run in the palette, not with per-texel tweaks.

## Mnemonic glyphs

Name glyphs so the grid reads without the palette beside it: lowercase is the light shade and
uppercase the dark shade of one hue (`r`/`R` for red). `.` is reserved for the transparent texel.
`check-scene --rows` then shows the form as text.

## Effects come from ops, never painted

Outlines, shading, glow and shadow live in `effects:`, not in the grid. A painted highlight cannot be
swept and does not follow a palette change. The rule and its reason are in
`ludion docs sprite-format --section "Gotchas"`.

## Repetition

Do not copy a document to vary it:

- A symmetric shape authors half with `mirror` — `ludion docs sprite-format --section "Mirror"`.
- A frame that differs by a few texels is `base` + `patches` — `ludion docs sprite-format --section "Patches"`.
- A recolour is an atlas-source variant (*Recolour variants* in `ludion docs sprite-sheets`) or, for a
  standalone document, a palette include's `overrides` — `ludion docs sprite-format --section "One palette, many sprites"`.

`ludion check-scene <doc> --rows` prints the resolved grid, so each is checked before it is baked.

## The verify loop

- `ludion check-scene <doc> --rows` — the exact texel grid, with ruler and row numbers.
- `ludion render --sprite <doc> --scale N` — the read at size, on the contact sheet.
- `ludion render --sprite <doc> --steps [--frame <name>]` — which op made a mark.
- `ludion render --sprite <doc> --set <pointer>=<value>` or `--sweep <pointer>=a,b,c` — tune a value.

## Next

- `ludion docs sprite-format` — the document, mirror, patches, palette includes.
- `ludion docs sprite-authoring` — the loop: check, contact sheet, tuning.
- `ludion docs sprite-effects` — the eight ops, their parameters, and their gotchas.
- `ludion docs sprites` — the hub: documents, verbs and components at a glance.
