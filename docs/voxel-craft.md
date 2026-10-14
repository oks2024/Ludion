---
description: What makes a voxel model read at game scale: silhouette, value, density, pose, emissive
---

# Voxel craft

**Scope:** the art direction a valid model still needs, stated as checkable rules. The schema is
`ludion docs voxel-format`, the commands each check names are `ludion docs voxel-authoring`, and the
ladder for models above hand-editing size is `ludion docs voxel-ops`.

## Craft — what makes a model read

Passing every check in `ludion docs voxel-format` makes a model *valid*; none of it makes one
*good*. The recurring defects in this project's model history were craft defects, not format errors
— models that pass every check and still read as a smudge in the game view. What follows is the
craft that can be *checked*: each rule states what to do, why it binds at this resolution, and the
command that says whether you did it.

**Start from how many pixels a model actually gets.** Measured in the current vertical slice
(a twin-stick vertical slice at 1080p under its follow camera — pitched 58° down, `fov_y` 50°, subject
~27 units out); the numbers are camera-dependent, so another game re-measures, but the order of
magnitude is the point. A character-scale model — the shipped 9³ statue at `Transform.scale: 0.24`,
2.16 units tall — covers about **80 × 106 px** where the player stands and **50 × 71 px** at far
combat range; a standard-density 1-unit prop (16 cells at `scale: 0.0625`) covers **46 × 59 px**. Per
voxel that is a **~2.9 × 2.7 px** top face at standard density (~11 px at the statue's `0.24`) and a
~1.1 px vertical edge. The pitch does the rest: one world unit of *height* spans ~17 px against ~43
px for one unit of *depth*, so **top faces carry the read** and side faces compress about 2.5×.

**The contact sheet flatters, and `--true-size` is the correction.** `render --model` normalizes its
subject to the unit bounding sphere, so the subject fills about half a sheet cell *whatever the grid
size* — roughly `320 / N` px per voxel for an N-cell dimension on the default six-view sheet. A 32³
model looks exactly as big there as a 9³ one, and its voxels get ~10 px against ~3 px in game: the
sheet flatters detail by 3–7×. **It also flatters a 12-unit building exactly as much as a 2-unit
character**, which is the dangerous case — judged at the same cell size, a building's detail looks
like waste and the honest conclusion is to throw good work away.

```
ludion render --model <path> --views game --true-size
```

`--true-size` sizes the sheet cell from the model's own declared `scale:` so the subject covers the
pixels it earns under the gameplay camera — the 2.16-unit statue lands near ~106 px in a ~389 × 219
cell, a 12-unit house near ~589 px in a ~1665 × 937 one. `--width`/`--height` then set the cell's
*aspect* only, so passing either explicitly alongside it is an error. A model that declares no
`scale:` is read as one world unit per voxel; that is a reason to declare one, not to guess a size by
hand.

### Silhouette

**A model must be identifiable from its outline alone, at the size it is actually drawn.** At
~80 × 106 px an enemy is an outline with a color wash inside it; interior detail resolves to nothing,
and what tells one creature from another at combat range is the profile — a hunch, a raised limb, a
horn, a skirt of strands. Aim for one dominant mass plus at most two subordinate shapes that break
its edge; a symmetrical blob with detail painted on it is the failure mode.

**The check:** `ludion render --model voxels/model.voxel.yaml --views game --true-size`, then name
the model without looking at the filename — and again beside the model it must never be confused
with. If the two outlines are the same outline, no palette change will rescue the shape.

### Value structure

**Author the palette as values first, and judge them under the rig the model will be seen in.** At
50 px a hue difference does nothing a value difference does not do better, and the 58° camera shows
the model mostly against the ground plane — the read *is* the value step between model and floor. For
a game consuming models at night that means a much darker albedo than it feels like it should be:
weathered stone around `0.1–0.2` linear, near-black creatures around `0.03–0.08`, with [emissive
accents](#emissive-discipline) rather than albedo carrying the night read. The shipped statue is the
standing counterexample — 4–10× above that band, near-white under the night rig.

**The check:** render the sheet under the target scene's own lighting (see
`ludion docs voxel-sheets` for what `--rig` lifts and its one caveat) —

```
ludion render --model voxels/model.voxel.yaml --rig examples/gallery/night-street.yaml
```

— and ask whether the body separates from the ground plane with the emissive off the table. When the
answer is "somewhere between these two", sweep it rather than re-rendering by hand: `--set` and
`--sweep` address the model's own palette, so one command produces one labelled contact sheet of the
candidates side by side —

```
ludion render --model voxels/model.voxel.yaml --rig examples/gallery/night-street.yaml \
  --views game --true-size --sweep /palette/G/emissive=1.6,2.2,2.8
```

— the pointer shapes being `/palette/<glyph>/color/<0..3>` and
`/palette/<glyph>/{emissive,metallic,roughness}`, nothing on disk changes, and a sweep takes exactly
one view because it spends its cells on values instead.
`ludion image-stats model.sheet.png` is the numeric backstop: on a night sheet `clipped_high` above a
trace means something is blowing out under a rig that should barely light it (an accent may account
for a little, a body glyph for none), and `clipped_low` climbing as you darken means you overshot.

**Judge value against the neighbours, not against memory.** The question that decides a palette is
almost never "is this dark enough" in isolation — it is "is this facade lighter than the house next
door". Pass several models to `--model` and they render into **one** cell, side by side on one ground
plane under one rig, each captioned:

```
ludion render --model voxels/house-a.voxel.yaml,voxels/house-b.voxel.yaml,voxels/house-c.voxel.yaml \
  --rig examples/gallery/night-street.yaml --views game --out houses.png
```

Sizes are honest by construction — the row is normalized as a whole from each model's declared
`scale:`, so a 12-unit building really does stand 5.5× the 2.16-unit creature beside it, and the
lineup answers proportion as well as value. Up to 8 models, one view, `--out` required.

### Detail hierarchy and density

**Spend cells on the largest features first, and cut anything below one voxel of top-face relief.**
At standard density a voxel's top face is ~2.9 px, so a one-cell notch is a ~3 px feature: crisp on
the sheet at ~10 px per voxel, absent in game. Under that threshold detail does not read as detail,
it reads as noise, and it spends the cells a feature that *would* have read was going to need. Pick
the resolution from the smallest feature that must survive — the density
convention (`ludion docs voxel-format`) came out of exactly that judgement, a lantern authored at
several vertical registers and picked by eye at the 32-tall one.

**The check:** `ludion check-scene voxels/model.voxel.yaml --exposure`, whose per-glyph enclosed
counts price the detail that can never be seen from any angle — on a detail glyph those are cells to
reclaim, on the structural glyph they are the ordinary solid interior. Then do the arithmetic: a
feature under about two voxels across lands under ~6 px in game however sharp it looks at `320 / N`.

### Proportion and pose

**Judge proportion and attitude in the top-down three-quarter view, never in the front elevation.**
The pitch compresses height against depth by about 2.5× (~17 px per world unit up, ~43 px per unit
deep), so a model that reads tall and menacing in `front` reads squat and wide in game, while a
forward hunch or an extended arm — depth, not height — survives intact. A voxel model is one static
mesh: its whole attitude is in the cell layout, and there is no pose to add later.

**The check:** `--views game,iso` on the sheet — `game` is the gameplay angle, `iso` the closest of
the default six to it — and read the proportions there, not in `front`.
`ludion check-scene <model> --mirror-x` then separates a *deliberate* asymmetry (a dropped shoulder,
one raised hand) from stray cells that merely make a model lopsided: the intended one is a short
readable list of differing cells, the accidental one is scattered. `--column x,z` shows where the
mass actually sits.

### Emissive discipline

**Give a model one emissive read, and check which way its faces point.** Emissive accents carry a
model at night — at 50 px the eye finds the glow before the shape — but the bloom footprint is *part*
of the silhouette, not an addition to it: the statue's lit footprint including its eye bloom is about
1.5× its geometry, 106 px tall becoming 167. Two competing emissive areas therefore merge into one
bright blob at combat range and stop being a read. One accent, placed where the silhouette wants it.

**The check:** `ludion check-scene voxels/model.voxel.yaml --exposure` again, this time its
`emissive faces` line — read as the loop in `ludion docs voxel-authoring` describes, it settles
direction without a render. One direction taking
every face is the lightbar bug, a strip meant to glow at the player escaping only upward, which
before `--exposure` existed cost a GPU render to notice; zero exposed faces is an accent buried
entirely. Then render under the target rig and hold the bloom footprint against the silhouette.

**Material scalars are a separating tool, not a finish.** `metallic` and `roughness` buy almost
nothing at ~3 px a face — a metal face is mostly its reflection, and with no texture maps there is
only sky and ambient to reflect
(`ludion docs materials --section "Metals under ambient-only light"`) — so here their real job is
to hold one glyph apart from another: a `metallic: 1.0`,
`roughness: 0.25` trim against a rough body, as the palette example in `ludion docs voxel-format`
does. Spread across a whole model they read as an unexplained value shift. And mind the precedence:
one entry authoring any material field makes the `Mesh` component's material fields inert model-wide.

**Stair-stepping is a decision, because the mesher will not soften it.** Faces are greedily meshed
with faceted normals and nothing is smoothed, so every 1-cell step is a hard lit edge. A curve
approximated by irregular steps (3 cells, then 1, then 2) reads as noise at ~3 px per voxel; the same
curve on a regular rhythm — a consistent 2-cell rise, a repeated 1-in-2 chamfer — reads as style.
Check the rhythm as text with `ludion check-scene <model> --slice all`, and confirm a re-cut moved
only the cells you meant with `ludion voxel diff <before> <after>`.

**Not craft: style.** Whether a game's models are grimy or pastel, chunky or ornate is that game's
look, and it belongs with the game — its voxel-artist persona and its own docs — not here.

## Next

- `ludion docs voxel-authoring` — the loop and the text checks the rules above lean on.
- `ludion docs voxel-sheets` — every `render --model` flag the checks above use.
- `ludion docs voxel-format` — palette, orientation, and the density convention.
- `ludion docs voxel-ops` — re-cutting a shape with ops instead of by hand.
- `ludion docs voxel-assemblies` — pivots, joint plugs, and the humanoid part convention.
