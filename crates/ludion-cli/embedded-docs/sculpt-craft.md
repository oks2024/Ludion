---
description: What makes a sculpt model read — proportion, blends, silhouette, emissive, resolution
---

# Sculpt craft

**Scope:** the art direction a valid sculpt document still needs, stated as checkable rules. The
schema is `ludion docs sculpt-format`, the loop and the flag reference are `ludion docs
sculpt-authoring`. Drawn from the first real authoring run —
`examples/gallery/sculpts/idol.sculpt.yaml`, a 14-part idol accepted after 10 sheet
renders — so every rule below is a defect that model hit, not a guess.

## The loop that converged

**Work big to small: masses and silhouette, then proportion, then limbs and blends, then carving,
then emissive detail in a close-up, then a resolution/triangle sweep last.** Fixing a silhouette
problem after the emissive detail is authored throws the detail work away with it; fixing it first
is free. `--bounds` earns its keep at the proportion step, the sheet at the silhouette step,
`--part`/`--isolate` close-ups only once the big shapes hold.

**Budget line count like a 20–30 line part.** Every vector is a block list — `[0, 1, 0]` inline is
`L1005` (`ludion docs scene-format`) — so a 6-key part with two vectors runs 8–10 lines and the
14-part idol runs to roughly 280 lines. Normal density for this format, not a sign to compress.

## Proportion is arithmetic once one bound is real

**Author every dimension in real world units from the start, and read the model's actual size back
with `--bounds` before eyeballing a fix.** `ludion check-scene <model>.sculpt.yaml --bounds` prints
each part's `min..max` and the whole model's bone-root box — the idol's finished height came from
reading that box and scaling the whole document to it, not from re-guessing individual part sizes.

**`mirror: x` costs nothing and composes with every op**, including a `subtract` — a pair of eye
sockets is one part (`op: subtract`, `mirror: x`), never two hand-mirrored copies to keep in sync. A
`subtract` box on a `sharp` part gives a clean recessed panel (the idol's plinth inscription is one
`box` `subtract`, no rounding to fight).

**`--palette-from` an existing model joins its colour language instead of guessing one.** The idol's
palette came from `--palette-from <model>.voxel.yaml` — a voxel
model's palette is the same shape a sculpt's is, so the two families share swatches for free.

## Thickness: `L1158` fires late

**Give anything that must read cleanly at least 3 grid cells across (radius `>= 1.5 / resolution`),
not the 2-cell floor `L1158` actually checks.** A part right at 2 cells — 0.07 world units at
32/unit — meshes with ragged, torn edges and raises **no warning at all**, because `L1158` only
fires *below* two cells; it catches invisible, not merely ugly. Read `L1158`'s message as a hard
floor, not a target.

## Blends: a blend radius eats what it touches

**Keep a limb's `blend` at roughly a third of its own radius or less, and keep a visible width gap
between a limb and the body it joins.** A blend near the limb's own radius, against a torso as wide
as the shoulders, does not "smooth the joint" — it merges both arms and the torso into one
rectangular front, the same failure a smooth union always risks (`ludion docs sculpt-format`'s
semantics section). What worked on the idol: shoulders at `x: 0.26`, arm `blend: 0.025` against arm
`radius: 0.07` (roughly a third), torso `radii.x: 0.21` — a real gap between the capsule's origin and
the torso's own edge before the blend ever runs.

## The armature convention

**One capsule chain per limb, one bone per capsule, joints at the blend, `head`/`tail` on the
capsule's own `from`/`to`.** A bone names the same two points the part it drives already has — no
second set of coordinates to keep in sync by hand. A worked upper arm, two capsules end to end, each
with its own bone:

```yaml
bones:
  - name: root
  - name: upper_arm
    parent: root
    head:
      - 0.26
      - 1.35
      - 0
    tail:
      - 0.26
      - 1.05
      - 0
  - name: forearm
    parent: upper_arm
    head:
      - 0.26
      - 1.05
      - 0
    tail:
      - 0.26
      - 0.78
      - 0
parts:
  - name: upper_arm
    shape: capsule
    radius: 0.07
    from:
      - 0.26
      - 1.35
      - 0
    to:
      - 0.26
      - 1.05
      - 0
    blend: 0.02
    material: S
    bone: upper_arm
  - name: forearm
    shape: capsule
    radius: 0.06
    from:
      - 0.26
      - 1.05
      - 0
    to:
      - 0.26
      - 0.78
      - 0
    blend: 0.02
    material: S
    bone: forearm
```

The joint is where the two capsules' `blend` overlaps — a small `blend` (a third of the limb's own
radius, the rule above) keeps the elbow a smooth crease instead of merging the two segments. Read
`--bounds` to confirm it: each bone's box should end close to where the next one's begins, not
swallow it.

## Deformers: bend, twist, taper

**Every deformer turns the part about its own centre** — the midpoint of its undeformed box — so
the same numbers mean the same shape wherever the part stands, a `from`/`to` capsule included.
**`bend` curves a straight primitive without re-authoring its geometry**: a capsule horn with
`deform: { bend: { axis: x, degrees: 90 } }` curls through a quarter turn over its own length, one
field instead of a chain of short segments approximating a curve. **`twist` only reads on a
cross-section that isn't round** — a ridged or faceted tendril twisting along its own axis; on a
capsule or sphere it changes nothing visible, since their cross-sections are rotationally symmetric.
**`taper` narrows or widens a cross-section along one axis** — a tail or strand thick at the root
and fine at the tip is one capsule with `taper: { axis: y, from: 1.0, to: 0.3 }`, not a chain of
shrinking capsules. `--bounds` catches an overshoot: a deformed part's box
grows with the deformation, so a box far larger than expected is the first sign of too many
`degrees` or a `taper` scale near zero.

## Voxel parts: reusing existing content

**A `shape: voxel` part reads an existing `.voxel.yaml` model's occupancy as a field, so a `blend`
against it smooths a hard edge and `op: subtract` carves it, without re-authoring the model in
cubes.** The `blend` and `--bounds` rules apply unchanged.

## Skin noise

**Amplitude around 1–3 grid cells, scale a few cells, on parts that read as organic skin, not
plinths or panels.** At `48/unit` that is `amplitude: 0.02..0.06`, `scale: 0.08..0.2`: enough to
break a primitive's perfect surface into a skin-like read without eating carved detail. Past about
4–5 cells of `amplitude` the noise tears the surface into disconnected bumps instead of texture —
the zero crossing moves past where neighbouring cells still agree, the same failure shape as a
too-thin part (`L1158`, `ludion docs sculpt-format`), self-inflicted this time. Keep `scale` above
two cells for the same reason: below that the lattice out-runs the sampling and reads as error, not
skin.

## Silhouette before detail, and check for accidents

**The big-shape read decides what creature this is; settle it before any carving or emissive
work.** Horns plus round wings read as a cat or a mouse; a back-tilted cranium ellipsoid (an "egg
head", `rotation: [-25, 0, 0]` on the idol's `cranium`) plus a skirt of strands reads as a
cephalopod. No palette or detail pass fixes a silhouette of the wrong genre — the `--views
game,iso` sheet is the check, worth a full render before any part past the torso is authored.

**Check the FRONT view specifically for anatomy nobody meant to author.** Two limb capsules whose
`to` ends rest in front of the chest read as breasts from the front, purely from where the geometry
stops. Route a limb down past the torso (to the knee) or out to the side, not to a point floating in
front of it, and confirm on the `front` view — the accident is invisible from `iso` or `top`.

## Emissive parts need real size, and bleed along their neighbours

**Give an emissive part at least 4 grid cells across, and carve a socket visibly larger than it.**
Below that, the glow is only a handful of decimated triangles and reads as a star-shaped patch
instead of a point of light — the idol's `eye` (`radius: 0.045`, 4.3 cells at 48/unit) sitting a bit
proud inside a `socket` (`radius: 0.07`, `op: subtract`) a full 1.5× larger is what read clean.

**Emissive material bleeds along a long decimated triangle into whatever part shares it**, as a
streak into a neighbour (light leaking into the idol's strand roots). Moving the emissive part
later in file order shifts this only a little: the leak is a triangle-shape problem, not a
fold-order one — a larger part, more triangles, or a higher `resolution` are the real levers.

## Resolution and triangles: what they're each worth

**`resolution` decides whether small carved and emissive detail survives at all; `triangles` decides
whether flat and curved surfaces stay smooth once it does.** Measured sweeping the idol
(`--sweep /resolution=…` and `/triangles=…`, one view per cell): at `resolution: 24` the plinth's
edges round visibly and the body lumps together; `32` is near-crisp with slightly rough thin rims;
`48` is the cleanest and pays `ludion-sculpt`'s cost model for it — roughly 3× the mesh time of `32`
(measured ~50 ms to ~170 ms here). `triangles: 2000` shows faceting and blocky glows;
`5000` (the format's own default) is clean; `10000` is slightly smoother still and is what a hero
earns. The user's checkpoint-1 ruling made `48` the default: the extra mesh time buys crisp edges
and surviving facial detail on every model, so drop to `32` only for a plain prop whose load cost
you have measured — that is a resolution decision, triangle count stays at the default either way.

**`sharp: true` fixes an edge's shading, never its silhouette** (`ludion docs sculpt-format`
Gotchas) — surface nets rounds every edge by about one grid cell regardless. At `48`/unit that one
cell is small enough that a `sharp` plinth reads crisp on the sheet with only a faint corner bevel;
at `32` and below the same edge reads rounded no matter how the part is authored. A resolution
decision, not a format one.

## Facing and close-ups

**Author the face toward `−Z`, the sheet's own `front`.** A scene placing the model under a camera
that sits on `+Z` turns the entity to match — `examples/gallery/sculpt-idol.yaml` sets the idol's
`Transform.rotation` to `[0, 180, 0]` rather than re-authoring the model to face the other way.

**Judge a small detail in a close-up framed on its parent part.** The idol's eye and socket were
judged with `--part head`: the face around the eye is the context a star-shaped glow or a buried orb
is read against. `--part eye` also works (a tiny part's box is padded, the camera kept outside the
model), but views from behind the head show nothing. The full sheet's bloom hides an
emissive defect a close-up shows plainly; check both.

## Reading `--bounds`

**`--bounds` is load-bearing, not a convenience** — beyond proportion, a detail whose box is only a
sliver deep along one axis is mostly buried in its neighbour even though it validated and meshed
cleanly; the fix is visible in the numbers before it is visible in a render.

## Rendering a sculpt for image-to-mesh

**Four `--views front|left|back|right` renders at one square `--width`, cropped and masked in the
generation graph.** A feature survives the reconstruction only if it clears its neighbours **in
profile** — `ludion docs generation-craft-mesh`.

## Not craft: style

Whether a game's sculpts are grotesque or serene, organic or hard-edged is that game's own look; the
rules above are what keeps any of them readable, not what they should look like.

## Next

- `ludion docs sculpt-authoring` — the loop and the flags the checks above use.
- `ludion docs sculpt-format` — the document schema and its blend/material semantics.
- `ludion docs voxel-craft` — the same craft questions over the faceted grid format.
- `ludion devdocs ludion-sculpt` — the mesher's cost model and determinism rules.
