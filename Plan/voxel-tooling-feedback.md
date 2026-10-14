# Voxel tooling — field feedback from authoring `house-c`

> Status: **findings, not a plan.** Written 2026-08-05 immediately after authoring
> `games/vigil/scenes/voxels/house-c.voxel.yaml` end to end in one session, in the voxel-artist
> editor role. Every gap below was hit in that session; every claim was checked against
> `--help` output before being written down. Nothing here is speculative wishlist.

## The session this came from

A new city building — `house-c`, the crow-stepped stone house — authored at **16 voxels per world
unit** on vigil's frozen 8.0 x 7.0 house footprint, at the user's request to try the standard prop
density on a building. Result: `128x192x112 = 2,752,512` cells, 10.5x the `L1115` budget
(`budget_exempt: true`), a 2.9 MB model file, generated from a committed
`house-c.voxel.gen.mjs` over `scripts/voxel/grid.mjs`.

The density question resolved **in favour of 16/unit**, and the reasoning is worth recording because
it is not what the docs currently imply — see gap 4.

Time was spent roughly: 20% construction, 80% *looking* — establishing what the model would actually
read like. That ratio is the headline finding. **The making side of this toolchain is materially more
mature than the looking side**, and since the docs themselves state that value and colour are the
unsolved half of voxel authoring, that is where the next round of investment pays.

## What worked, and should not be disturbed

- **The script rung (`grid.mjs` + `voxel from-json`) gave complete fine-grain control.** The clearest
  proof: the arched doorway was written as an arbitrary predicate `inDoor(x, y)` — rectangle below
  the springing, circle above — and the voussoir ring was then swept by testing five offset
  directions against that same predicate. No ops file could express that, and a fixed shape
  primitive would have forced an approximation. `box`/`ellipsoid` + raw loops + `mirrorX` + `speckle`
  covered an entire building. **At no point was a cell-placement capability missing.**
- **Text-first checks genuinely replaced renders.** `check-scene --column 78,3` settled "does the
  hoist beam project clear of the plinth" in one command, no GPU. `assertConnected(grid)` before
  `emit()` meant a disconnected model was never round-tripped to discover.
- **`voxel from-json --validate` as a one-pipe loop** is the right shape — it let the 2.75M-cell
  feasibility question be answered in 0.9 s before any design work was committed to.
- **`--rig games/vigil/scenes/env-night.yaml` self-serve** worked exactly as documented and caught a
  real value error without needing the human.
- **The drift gate is correctly built.** `from-json --check` plus `scripts/voxel-drift.mjs` gave a
  clean, fast, unambiguous answer.

## Gaps, ranked by cost actually incurred

### 1. `--sweep` and `--set` are `--scene`-only, and value calibration is the whole remaining loop

**The largest gap by a wide margin.** `docs/voxel-craft` states outright that palette-value
calibration is the one recurring cost in this project's model history. `render --sweep` exists,
is described as "the taste half" and "for the judgement calls a metric cannot make" — and **cannot
be pointed at a model document.** Same for `--set`.

What was wanted, and does not exist:

```
ludion render --model voxels/house-c.voxel.yaml --rig games/vigil/scenes/env-night.yaml \
  --views game --sweep /palette/G/emissive=1.6,2.2,2.8
```

**Cost:** the two open taste questions on `house-c` (the emissive green's hue/intensity, and the
overall stone darkness) could not be resolved into a single labelled contact sheet. They became a
multi-turn conversation with the human instead — which is precisely the North Star metric
(*iterations before a human must look*) being spent on the one thing the tool was built to avoid.

**Proposed fix:** allow `--set`/`--sweep` with `--model`. The pointer machinery already resolves
"a field the document has"; a palette entry (`/palette/G/emissive`, `/palette/S/color/0`) is an
ordinary JSON pointer. This is close to a flag-gating change and would collapse the slowest loop in
the domain into one command.

### 2. "Exposed" is not "visible from the game camera" — and that gap produced the session's only real defect

The loft emissive panel was authored 1 world unit deep in its recess. `check-scene --exposure`
reported **703 exposed faces, 640 pointing `-Z`** — a perfect score by every available text metric.
It was still wrong: at the 58-degree gameplay camera pitch the lintel occludes a recess that deep,
and the glow arrived as two disconnected slivers instead of one window. It was found by rendering
and squinting, then fixed by shallowing the recess to 4 cells (re-check: 640 of 646 faces `-Z`,
one coherent window).

This is the **same class of bug** as the lightbar-glowing-upward case that motivated `--exposure` in
the first place. `--exposure` currently answers *"can light get out of this cell"*; the question that
decides the model is *"can the player see it"*.

**Proposed fix:** `--exposure --from game` (or `--visible-from <pitch>`), ray-testing exposed faces
against the gameplay camera direction and reporting occlusion:

```
emissive faces:
  glyph G (emissive 2.2): 646 exposed — +X 0, -X 0, +Y 0, -Y 0, +Z 6, -Z 640
    from game view (58 deg): 640 face the camera, 430 occluded by own geometry  <-- new
```

That line alone would have caught the defect as text, with no GPU and no human.

### 3. `--mirror-x` computes the answer and refuses to report it

Question asked: *"is my asymmetry confined to the two features I intended?"* — the exact question the
craft doc says separates a deliberate asymmetry from a lopsided accident.

`--mirror-x` answered `12443 asymmetric cells` plus 20 sample lines, **all 20 from the same
feature**, then `... and 12423 more`. Useless for the question. Resolution required writing a
throwaway Node script to parse the 2.9 MB YAML and compute a bounding box — to recover information
the checker had already computed and discarded.

**Proposed fix:** cluster differing cells into connected regions, print per-region bbox, count and
glyph set:

```
mirror-x: 12443 asymmetric cells in 2 regions
  11203 cells  x 66..95  y 124..165  z 0..23   glyphs . G Q W M
   1240 cells  x 24..35  y 138..147  z 7..16   glyphs . B
```

Two tight regions read as deliberate at a glance; forty scattered ones read as a bug. As shipped,
both print the same unusable integer.

### 4. The true-size guidance is character-calibrated and silently wrong for buildings

`docs/voxel-craft` instructs `--views game --width 200 --height 180` to see the model "near the
~100 px the model really gets". That figure derives from the 2.16 u statue.

`house-c` is 12 u tall — roughly **5.5x** the coverage. At 200x180 it renders as a black blob with a
green dot, and **had that framing been trusted, the correct conclusion would have been that 16
voxels/unit was wasted and the detail should be thrown away.** The correction had to be derived by
hand from the statue's 2.16 u / 106 px datum. Rendered at 1100x1000 — the honest coverage for a 12 u
building — the arch voussoirs, step copings, corbel table and quoins all read clearly, and 16/unit
is justified.

This is the most *dangerous* gap in the list, because it fails silently and in the direction of
destroying good work.

**Proposed fix:** `render --model` already parses the document's declared `scale:`. Compute the
framing: `--views game --true-size` frames the subject at the coverage its own declared scale earns
under the vertical slice's camera. Removes the arithmetic from every author, most of whom will not
notice it is needed.

### 5. Nothing renders a model in context, and context is where value lives

Every judgement in the session was made of one house alone on an infinite pale plane. But the value
question that actually mattered — *"is my facade lighter than house-a's?"* — was answered by
rendering two separate sheets and comparing across two images **from memory**. That is a
recollection, not a measurement, and it is the load-bearing judgement of the whole session.

Two related shortfalls:
- **No lineup.** `--model a.yaml,b.yaml,c.yaml` under one rig, one sheet, would make neighbour
  comparison a single look.
- **No value reference.** The model stage carries an `adventurer` for *scale* reference; there is no
  equivalent for *value*.
- **`--rig`'s ground plane is a stand-in.** It renders pale blue-white; the actual district floor is
  cobblestone. So "does the body separate from the ground plane" — the precise check `voxel-craft`
  names — is being judged against the wrong ground. The torch-falloff caveat is documented; this
  consequence is not.

### 6. Library gaps — architectural primitives, and one unguarded ordering trap

`lathe` is explicitly *the organic primitive* and is excellent for creatures. **There is no
architectural counterpart.** The roof was hand-rolled as a per-column loop over a `roofTop(x)`
function. A `wedge`/`prism` op (sloped extrusion between two edges) is the building-side `lathe`.

There is also no `repeat`/`array` helper, so the quoins, the corbel table and the six crow steps
were each a hand-written `for` loop with index arithmetic — exactly where off-by-ones live.

**One unguarded trap worth a warning:** `speckle` must run **before** `mirrorX`. Speckle the full
grid and then mirror, and half the weathering is silently discarded. The ops-file path already has
`L1134` for this exact hazard ("mirror discarded cells an earlier op deliberately wrote"); the script
library has no equivalent. A `Grid` that tracked whether a write preceded `mirrorX` could warn.

### 7. The docs contradict a shipped script, at the spot an agent looks first

`docs/voxel-ops` states: *"Every generator that ever templated YAML in this repo has been deleted."*

`scripts/gen-houses.mjs` is a hand-rolled YAML emitter (its own `emit()` builds key order, float
formatting and comment conventions by hand), it is still present, and it is authoritative for
**house-a and house-b — the nearest prior art for anyone authoring a third house.**

It was ignored here in favour of the sanctioned `grid.mjs` + `from-json` path, but it is the first
thing a search for "how were the other houses made" surfaces, and a less suspicious author would
clone the forbidden pattern because it is what the neighbours do. Either port `gen-houses.mjs` to
the sanctioned pipeline, or amend the doc sentence to name it as a known exception.

### 8. No cost number to inform the `budget_exempt` decision

This model was opted 10x over `L1115`, and **what it costs to render is still unknown.** `L1115`
counts cells. `--exposure` offers 130,419 surface cells as a proxy. Neither is the number that
decides anything — that is the greedy-meshed quad/triangle count.

A house is mostly flat walls, so greedy meshing should collapse it enormously, and the honest
expectation is that this model is far cheaper than its 2.9 MB file suggests. But that is a guess.
Before placing thirty of these in a district, `check-scene` should print the meshed primitive count
— which would also turn `budget_exempt` from a shrug into an informed call.

## Summary — proposed changes ranked by value

| # | Change | Effort | Why it pays |
|---|---|---|---|
| 1 | `--set`/`--sweep` accept `--model` | Low — flag gating + pointer into the palette | Collapses the domain's slowest loop (value calibration) into one command |
| 4 | `render --model --true-size` | Low — `scale:` is already parsed | Fixes a silent failure that argues for deleting good work |
| 3 | `--mirror-x` region clustering | Low | Turns an unusable integer into an actual check |
| 2 | `--exposure --from game` | Medium — needs occlusion ray-tests | Catches the emissive-visibility class as text; extends `--exposure`'s original thesis |
| 5 | Multi-model lineup render | Medium | Makes neighbour value comparison a look, not a recollection |
| 8 | Meshed primitive count in `check-scene` | Medium | Makes `budget_exempt` an informed decision |
| 6 | `wedge`/`prism` + `repeat`; warn on write-after-`mirrorX` | Medium | Architectural parity with `lathe`; closes a silent data-loss trap |
| 7 | Reconcile `voxel-ops` with `gen-houses.mjs` | Trivial | Stops the nearest prior art teaching a deleted pattern |

## Cross-references

- The small-local-model idea (`Plan/voxel-artist-playbook.md`'s note) — gaps 1, 3 and 4 bear directly
  on it. Each is a place where a small model must currently supply judgement the tooling could supply
  as text, which is exactly the capability those models lack. Closing them lowers the role's
  capability floor.
- `Plan/voxel-artist-playbook.md` — the operational workaround for every gap above, written for a
  low-capability model. Each workaround there should be **deleted** as the corresponding gap closes.
