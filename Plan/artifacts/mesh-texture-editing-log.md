The convergence metric of `Plan/mesh-texture-editing.md` Wave 1 step 8: one line per surface edit
authored in the arc, attempts counted as `mesh apply` runs before the emissive channel read clean, read
against the two logs that bracket it — `Plan/artifacts/mesh-modeling-log.md` (authoring a form from
nothing) and `Plan/artifacts/comfy-generation-log.md` (buying one). This arc is the third case: keeping
bought surface and changing one thing about it deliberately. The question the column answers is whether
an agent can place a glow on a generated mesh, and *prove* it landed, without a human looking.

| subject | wave | attempts | renders read | tokens | wall time | authoring model and harness | source access | human intervention | review fidelity | verdict |
|---|---|---|---|---|---|---|---|---|---|---|
| idol — one glowing green eye (`idol-lit.mesh-edit.yaml` over the generated idol) | 1 | 4 `mesh apply` runs + 44 `mesh pick` casts | 14 images / 37 cells | ~120 k (estimated — not surfaced by this harness run) | ≈40 min (estimated; not instrumented — GPU time is a few seconds per sheet and a negligible share) | Claude Opus 5 subagent, Claude Code | the plan (Why, rulings, decisions 9/10/13, the document, the derived tree, the verbs, steps 8 and 12), `mesh --help`, `render --help`, `region.rs` read once for the `box` semantics; no craft docs (none exist yet) | none | six-view and single-view sheets read by the model; `apply`'s per-op report; `--channel emissive` scanned pixel-by-pixel by a throwaway script as well as read | **The author's read, not a ruling.** It works, and the two instruments decided every call. See below. |
| idol — the same eye by a seeded fill (`idol-fill.mesh-edit.yaml`, `scene-fill.yaml`) | 2 | 5 `mesh apply` runs + 9 `mesh pick` casts | 9 images read (the first back sheet, five emissive-beside-sheet crops, the pristine crop, a six-view emissive sheet, an idol-lit comparison) / 15 `render` runs | ~115 k (session token counter, whole run) | ≈5 min (clock, from the brief's timestamp to the last apply; GPU time again negligible) | Claude Sonnet 5 Coder instance, Claude Code | the brief, the Wave 1 log, `docs/mesh-editing.md`, `docs/mesh-edit-regions.md`, `mesh --help`, `render --help`; no source read | none | the emissive channel scanned by a throwaway script (non-black pixels per cell) as well as read; the plain sheet cropped 4× beside the pristine crop and beside idol-lit's | **The author's read, not a ruling.** The fill fits the eye better than the primitives did in the same number of applies; the cost moved from placing shapes to searching two numbers. See below. |

## The author's read — Wave 1, primitives

The eye reads. On the model stage the glow is one green almond in the socket of the figure's right eye,
the left eye still the dull olive the generator painted, and at `Mesh.emissive: 3.0` the bloom halo is
wide enough to be a *light* rather than a bright texel without washing the brow above it. Nothing else
on the model emits: the emissive channel over all six views is black but for that one blob, seen from
`back` and `left` and from nowhere else, which is exactly the categorical answer decision 10b promised
and is worth more than any amount of squinting at the lit render.

The report is the instrument that actually drove the loop. The op's model-space AABB landed at
x −0.0930..−0.0550, y 0.1567..0.1779 against an eye measured by pick at x −0.0914..−0.0552,
y 0.157..0.176 — within two thousandths of a unit on every axis, which is a sentence an author can act
on. Attempt 1 (one sphere) reported a y span 1.8× the eye's height before any render was made, and the
render then confirmed a vertical teardrop where an almond belonged. That ordering — report first,
picture second — is the thing this wave is for.

**What could not be verified blind.** Three things. (1) Whether the glow *looks* like an eye rather than
a painted-on light: it is the right shape and the right place, but "does it read as alive" is the
checkpoint's question, not the machine's. (2) The colour: `[0.05, 1.0, 0.35]` is the plan's example
green taken on faith, and against a cold grey stone it may be a shade too electric — the sweep that
would settle it is a taste sweep, not a metric. (3) Mip fade at distance: the plan's risk 5 says a small
blob on a 1024² map can fade as the GPU-built mips average it away, and every render here was
close-framed, so nothing in this run touched that. A stage that walks the idol away from the camera
would.

**One anomaly, recorded.** The emissive render has exactly one stray non-black pixel, about 50 px above
and left of the eye on the brow, at the head's silhouette: rgb ≈ (1, 49, 10) at 1600², (0, 41, 8) at
800², the same surface point at both extents. `apply` reports 400 painted texels in three UV islands and
the pick under that pixel names a texel tens of texels away from all three, so it is not the dilate pad —
almost certainly a coarse-mip sample on a near-edge-on triangle. One dim pixel is below any visual
threshold, but the emissive channel is sold as categorical, and a categorical instrument that reports a
pixel the report cannot explain is worth an entry below.

## Attempt by attempt — Wave 1

1. **One sphere** at the eye's surface centre, radius 0.014, feather 0.005 — the plan's own example
   shape. 349 texels. The report's box was 0.035 tall against the eye's 0.019 and only 0.027 wide
   against its 0.036: the head curves away hard at the outer corner, so a sphere big enough to reach it
   is far too tall. The render agreed — a vertical teardrop.
2. **One box**, `size [0.026, 0.019, 0.08]`, feather 0.003. 457 texels and the best AABB of the run,
   because a box cuts exactly where it is told in x and y and the z extent only has the face-side
   surface to find. But the render is a *square*: a hard axis-aligned edge on a curved face reads as a
   decal, not a glow.
3. **Capsule + corner sphere**, the capsule along the eye's long axis nudged 0.002 out of the surface so
   the arc's bulge splits either side of the chord. 315 + 64 texels, the right silhouette at last — but
   the capsule's cap spilled 0.009 past the inner corner onto the nose bridge, and a thin gap opened
   between the sphere and the capsule.
4. **The same pair, retuned** — capsule pulled in at the inner end, sphere grown to 0.007/0.003 to close
   the gap. 309 + 91 texels over 14 triangles, one contiguous blob, AABB matching the eye on every axis.
   Shipped.

The shape lesson is general: on a strongly curved surface a *sphere* controls the glow's extent along
the surface badly (curvature eats it unevenly) and a *box* controls it too well (its edge is visible).
A capsule following the surface arc is the primitive that fits a feature, and two of them are often one
feature.

## The scene — Wave 1

Intensity lives in the scene (decision 9) and the derived asset does not glow until one asks. The only
entity in the tree that points at this mesh is the import scene `gen accept` wrote beside it. The run
repointed that scene; the orchestrator restored it — `gen accept`'s tree is never rewritten (decision 2),
and the sitting wants the two idols side by side — and the lit placement is its sibling `scene-lit.yaml`:
the same entity, the path to the derived document, `Mesh.emissive: 0.0 -> 3.0`. `Mesh.color` there is
white, so the map's green passes through untinted; the value came from a six-cell
`--sweep /entities/0/components/Mesh/emissive=1,2,3,4,6,9` on that scene, where 1–2 is a bright sliver
with no halo, 4 starts spilling onto the brow, and 6 and 9 blow the core to white. 3 is the last value
where the core still reads green.

Nothing else moved: the pristine `mesh-0.mesh.yaml` and the three generated maps are byte-identical, the
gallery golden that renders the pristine mesh passes at SSIM 1.0 untouched, and no replay or sim hash is
involved — no game scene loads this tree.

## The author's read — Wave 2, the fill against the primitives

One `fill` did what took Wave 1 a capsule and a sphere, and fitted better where fit is the point. The seed
is a pick on the eye's colour; the fill then walks the yellow-olive patch the generator painted, so its
outline is the *painted* eye's outline. It lights the strip along the outer corner that idol-lit's pair
never reached (an olive band is left above the primitives' blob in the comparison crop), follows the
brow-side edge the generator drew, and crosses the UV seam that splits the eye (two picks on it have UVs
0.19 and 0.80) without the author having to know a seam exists. On effort it is one op with a six-line
region against two ops, and one pick against Wave 1's copy-two-picks-and-hand-nudge-the-z for the capsule.
On the *look* the primitive wins two points: the fill's edge is stepped by the texel grid and hard (its
`feather` acts only at the radius edge, which no region ever reaches — the colour edge is the real
boundary and nothing softens it), where the capsule's `feather` gave a rounded, slightly soft almond; and
the fill leaves a small dark spot at the pupil (base colour 37,48,8 is 0.37 from the seed, and any
tolerance wide enough to take it also floods the lower lid, which reads about 0.27) and a few spikes along
the upper lid's shadow. What the primitive could not do: follow the eye's own outline. What the fill could
not do: give a soft edge, take a pupil, or be told where its boundary is beyond one global number.

The report's box was less useful here than for primitives. A fill's hull is the hull of the *surface it
reached*, and the eye curves in z through 0.035 units, so even the best attempt's box came out 0.0065 taller
than the eye (y 0.155..0.1825 against the picked 0.157..0.176); with primitives the box was the region's own
extent. What it did do: `--view back` gave the box in pixels (701..745 × 606..639), the number the emissive
crop was compared against, and the triangle count named attempt 3's cut tip. The emissive channel again
shows one blob, in `back` and `left`; Wave 1's dim brow pixel is there too, and two or three dim pixels in
`front` besides (see the findings) — lit identically in idol-lit's channel, so neither region's doing.

**What could not be verified blind:** all three of Wave 1's (does it look alive; the green against stone;
mip fade at distance), and one more — whether a hard, jagged glow edge reads as a stone eye or as a
cut-out. At `Mesh.emissive: 3.0` the bloom will soften it, but no scene render was made in this run.

## Attempt by attempt — Wave 2

The seed was picked at the eye's visible centre (`--view back --pixel 722,625`); tolerance, radius, seed and
feather were then iterated. The op is one `paint` on `emissive`, colour `[0.05, 1.0, 0.35]`, `size: 1024`,
as idol-lit's.

1. **`tolerance 0.12, max_radius 0.03`, feather 0.004.** 325 texels, box 705..745 × 606..638 px. A green
   almond with its outer corner strip unlit, and the pupil hole. The strip's colour (92,85,35) is 0.11 from
   the seed, inside the tolerance, so it was the radius that cut it: the strip is 0.035 away in 3D, the eye
   curving through z. Nothing in the report says which constraint stopped the walk; I found it by picking
   the strip and computing distances by hand.
2. **`tolerance 0.2`, same radius** — the guess that the tolerance was too tight. 624 texels, 17 triangles,
   box down to y 0.137: it flooded the lower lid's skin and left the strip *still* unlit. The wrong lever.
3. **The seed moved to the eye's middle, `tolerance 0.15, max_radius 0.028`.** 398 texels. The outer strip
   lit at last; the inner tip was now past the radius, and a spike along the upper lid's shadow lit.
4. **`tolerance 0.17, max_radius 0.034`.** 560 texels, box 699..747 × 592..639. The whole eye, but the
   upper-lid spike grew to 12 px above it.
5. **`tolerance 0.13, max_radius 0.034`.** 382 texels, 7 triangles, box 701..745 × 606..639. One blob that
   matches the yellow patch of the pristine crop, the spike down to a few texels. Shipped.

The lesson is the reverse of Wave 1's. There the shape was hard to make and the tools told the truth about
it; here the shape is free and two numbers are the whole craft: tolerance trades holes against leaks along
a shaded boundary, and the radius (a 3D chord, not a surface distance) needs the seed near the middle of the
thing. The seed's position is a parameter nobody said was one.

## The scene — Wave 2

`scene-fill.yaml` is `scene-lit.yaml` with the derived path repointed to `meshes/idol-fill/mesh.mesh.yaml`;
`Mesh.emissive` is 3.0 unchanged — the sweep that chose it was over the same green. The pristine tree and
`idol-lit` are untouched; `mesh apply --check` is green for both and `scripts/mesh-edit-drift.mjs` checks
both documents. The derived mesh is 1.27 MB, so the size gate wants an allowlist line (added beside
idol-lit's); the derived emissive map is 7 KB.

## Tool improvements, most time lost first

The built ones (the polyline capsule, `apply --view`, the glow on the plain sheet, the separator fix) left
this list when Wave 2 shipped them.

1. **A fill's report does not say what stopped it.** Four of five attempts were a blind search over
   `tolerance`, `max_radius` and the seed's position, and attempt 2 turned the wrong knob because nothing
   said the radius, not the tolerance, was binding. **Fix:** on a fill op print the texels bounded by colour
   against those bounded by the radius, and the nearest miss of each ("22 stopped by radius, nearest 0.004
   past; 140 by colour, nearest distance 0.14"). That turns the search into two readable steps, as `L1178`'s
   "nearest distance" already does for `key`.
2. **A fill's edge is hard and there is no way to soften it.** `feather` acts only at the radius edge; the
   colour boundary — the edge every fill actually has — is a texel staircase at full weight. **Fix:** a
   fill option that feathers inward from the reached set's own boundary (a small world distance or a
   texel-radius blur), so a fill can carry the soft almond a capsule does.
3. **`mesh pick` answers one pixel per process** (a Wave 1 finding, unbuilt). This run needed 9 casts, 5 of
   them probing the colours at the spots a render showed failing. **Fix:** repeat `--pixel` or add a scan
   form, the camera rebuilt once.
4. **Nothing summarises the colours a fill met.** After each attempt the question was "what is the base
   colour at the texels the fill missed or leaked into", answered pixel by pixel through pick. **Fix:** a
   fill's report could print the base-colour mean and extent over the reached texels and the colours of
   its nearest rejected neighbours.
5. **The report's box is a surface hull, not the region.** On a curved patch it overshoots (0.0065 here), so
   it cannot confirm a fill's fit tightly. **Fix:** also print the reached texels' UV box, which is what
   the emissive map's pixels show.
6. **The emissive channel's stray pixels are not the region's, and they are in `front` too.** Wave 1's dim
   brow pixel (about 50 px above and left of the eye at 1600², in `back`) is lit identically under idol-lit
   and idol-fill, and the 800² six-view sheet also has two dim pixels in `front` and three near the `left`
   brow. Same texels either way, so it is the texture's or the mip's, not an op; Wave 1's "nowhere else"
   held for its larger cells only. **Fix:** as before — sample the channel at base level, or have `apply`
   name the other triangles that use the texels an op touched.
7. **`pick` still omits the surface normal**, and no region follows the surface for a feature with no colour
   edge: a fill needs a colour boundary, and a glow on flat stone still wants a
   capsule or a sphere.
8. **`check-scene` does not know `kind: mesh-edit`** (unbuilt): `L1110` on an edit document lists every
   other kind and does not name `mesh apply --check`.
