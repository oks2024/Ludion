The convergence metric of `Plan/mesh-modeling.md` decision 17: one line per model authored in the arc, attempts counted as sheet renders before acceptance.

| model | wave | attempts | tokens | meshed ms | authoring model / harness | source access | human intervention | review fidelity | verdict |
|---|---|---|---|---|---|---|---|---|---|
| idol (`examples/gallery/sculpts/idol.sculpt.yaml`) | 1 | 10 | ~191 k (the whole subagent run, blessing included) | 179 (157-179 over the runs at 48/unit) | Claude Opus 5 subagent, Claude Code | plan (decisions 1-3, 8, 9, 17, 18, 21, "The document", Wave 1 step 6), `ludion explain L1156`-`L1160`, `sculpt.rs` doc comments, CLI `--help`; no craft docs, no example models | none | six-view sheet PNGs and `--part head` close-ups read by the model; `check-scene --bounds`/`--silhouette` text | pending checkpoint-1 sitting |
| creature (`examples/gallery/sculpts/creature.sculpt.yaml`) | 3 | 11 sheets (16 renders with the 5 close-ups) | ~155 k (the whole subagent run, blessing included) | 624 (611-2544 over the runs at 48/unit; the 2544 was a bend whose conservative box tripled the lattice) | Claude Opus 5 subagent, Claude Code | `docs/sculpt-authoring`, `docs/sculpt-format`, `docs/sculpt-craft`, this log, the idol document, `Plan/mesh-modeling.md` Status; `field.rs`/`grid.rs` read once to explain the bend (see note 1 below) | none | six-view sheet PNGs and `--part skull`/`--part belly` close-ups read by the model; `check-scene --bounds` text | pending checkpoint-2 sitting |

## Creature authoring notes (Wave 3)

A quadruped creature at rest. 20 parts, 9 bones, 1.585 units tall, `48`/unit,
`triangles: 8000`, 432 lines. Wave 3 keys exercised: `noise` (amplitude `0.032`, scale `0.12`, one
seed) on torso/chest/haunch/neck; `deform: taper` on the tail (`z`, `1.0 -> 0.5`), the horns
(`y`, `1.0 -> 0.55`) and the underside (`z`, `1.0 -> 0.7`); `head`/`tail` on all eight non-root
bones, each pair copied from the capsule the bone drives. `bend` and `twist` are **not** in the
final document — why is item 1 below.

### Render by render

1. First full document (18 parts, cone horns, tail taper `1.0 -> 0.25`). Read as a quadruped
   immediately — the big-shape brief survived first contact. `L1158` on `haunch` (no surface),
   `L1159` two 4-vertex strays at the horn tips, 790 ms.
2. Softer tail taper (`0.45`), noise up to `0.032`, horn bend moved to `axis: y`. `haunch` came back
   (the taper had been stealing it), warnings cleared but the horns read as stubby ears.
3. Longer swept horns, a `repeat: 6` dorsal spike ridge, tucked belly, thicker tail. The ridge is the
   first thing that made it read creature rather than dog. Horn tips detached again (`L1159`).
4. Horn re-authored as a tapered capsule + `bend: { axis: y, degrees: 140 }`. **Both horns flew off
   the model** and landed on the ground as two detached blades; 279-vertex stray, lattice 188³,
   2544 ms. (Read as a sheet before I understood it.)
5. Bend deleted, tapered capsule horns kept. Clean check, 626 ms; the silhouette is a horned quadruped.
6. `--part belly --views front` close-up: identified the nub hanging under the chest as the belly
   capsule's front hemisphere — the craft doc's "anatomy nobody meant to author", and invisible from
   `iso`/`top`.
7. Belly front cap buried in the torso, eye pulled back out of the socket floor. Nub smaller.
8. Heavier shoulders than hips, ridge raised onto the new back line, heavier muzzle. `L1158`: the
   wider chest had swallowed the neck.
9. Neck re-aimed forward/up so it emerges ahead of the chest. Clean check; the whole-body read holds
   from all six views.
10. Tapers softened (horn `0.35 -> 0.55`, belly `0.55 -> 0.7`) after `--bounds` showed the horn owning
    508 surface vertices across the whole head and the neck down to 7. Neighbours recovered
    (neck 28, skull 97, horn 329).
11. `--part skull` close-up: the eyes are blown-out white craters the sheet's bloom had hidden
    entirely.
12. Emissive `6.75 -> 3.2`, eye `0.05 -> 0.043`, socket `0.08 -> 0.065` and moved onto the face, plus
    a `brow` capsule. Face has structure; the glow is still a ragged faceted patch.
13. `triangles: 5000 -> 8000`, eye up to `0.048`. Smoother body, glow still faceted.
14. Socket moved *in front of* the eye (the idol's arrangement: a carved cavity with the orb sitting
    at its back) instead of concentric with it. The eyes finally read as two lights under a brow.
15. Full sheet: accepted from every view.
16. `--views front,iso` after raising the belly flush with the torso's underside — the last of the
    chest nub gone. Accepted.

### Tool improvements, most time lost first

1. **`bend`/`twist` pivot at the model origin for a `capsule`, not at the part** — the single
   biggest time sink (renders 4-5 plus a code read). A capsule's `from`/`to` are baked in model
   coordinates (`grid.rs::primitive`), so its "local frame" *is* the model frame and
   `degrees * q[axis]` uses the absolute coordinate: a horn standing at `y ≈ 1.3` with
   `degrees: 140` rotates ~180° and lands on the floor. Every other shape is placed by `at`, whose
   frame origin is the part's own centre, so the same key behaves as documented there. Fixes, in
   order of preference: measure a capsule's bend from its own `from` (make the frame part-relative
   for every shape), or — cheapest — have `check-scene` warn when a deformer displaces a part's box
   centroid by more than its own extent, and say so in `sculpt-format`. The craft doc's worked
   example ("a capsule horn with `bend: { axis: x, degrees: 90 }` curls through a quarter turn over
   its own length") is only true at the origin, and is what I trusted.
2. **A `taper` steals its neighbours' surface and colour, silently.** The conservative Lipschitz
   divisor shrinks the whole part's distance, so a tapered part wins the smooth-min share far
   outside its own geometry: the first tail (`to: 0.25`) blanked the `haunch` into an `L1158`
   "hidden inside another part" that named the wrong culprit, and the horn at `to: 0.35` owned 508
   vertices over the entire head while the neck fell to 7. The mesh looked fine; only `--bounds`
   showed it. Wanted: the divisor computed per-sample rather than one constant per part, or failing
   that a `check-scene` note when a part's attributed box exceeds its primitive's box by some factor
   — and a craft-doc sentence "keep a taper's smaller scale above ~0.5 unless the part is alone".
3. **`--bounds` prints attribution, not geometry, and never says which it is.** Every deformed
   part's line is far larger than the shape on screen (the tail's box reached the floor while the
   tail was a needle). The craft doc leans on `--bounds` to catch "a deformer that overshot", but
   after a deformer *everything* looks overshot, so the signal I actually used was the surface-vertex
   count, not the box. Wanted: both numbers on the line (primitive box and attributed box), or a
   marker on the deformed ones.
4. **A `cone` apex always sheds a 4-vertex `L1159` island.** Two attempts (renders 1 and 3) lost to
   it; no `resolution` fixes a mathematical point. Either blunt the apex in the mesher, or say
   plainly in `sculpt-format` that an exposed `cone` tip is a stray piece and a tapered `capsule` is
   the spike/horn/tooth idiom. The `L1159` help text ("move the part toward the named point, grow
   it, or raise the blend") suggests three fixes, none of which is the real one.
5. **The sheet's bloom hides an emissive defect completely.** Renders 1-10 showed two tidy glowing
   eyes; the first `--part skull` close-up showed blown-out craters that had been wrong the whole
   time. That is the craft doc's advice working, but it cost ten sheets of false confidence — a
   `--part <name>` close-up belongs in the loop *before* the first emissive part is accepted, and
   `render --model` could cheaply warn when a cell's emissive pixels clip.
6. **A small emissive part still reads as a faceted patch at `triangles: 8000`.** Wave 1's note
   recurred unchanged: the decimator spends its budget on the big smooth masses and leaves the eye
   4-6 triangles. A per-part triangle floor (or decimation weighted by emissive) is the fix; raising
   the global count from 5 000 to 8 000 bought smoothness everywhere *except* where it was needed.
7. **`L1158` names the victim, never the cause.** "part `haunch` contributes no surface — it is
   hidden inside another part" was raised by a *tail* two parts later; "part `neck`" was raised by
   the *chest*. The mesher knows which part won the field there — naming it ("hidden by `tail`")
   would have saved a render each time.
8. **The bone `head`/`tail` pair duplicates the capsule's `from`/`to` by hand, 8 times.** The craft
   convention is "copy the two points", and every limb edit means editing four vectors (part
   `from`/`to`, bone `head`/`tail`) that must stay equal. Wanted: `head`/`tail` defaulting to the
   `from`/`to` of the parts that name the bone, with the explicit form kept as an override — or a
   `check-scene` warning when they disagree. Nothing checks it today, so a drifted armature is
   invisible until Wave 4 poses it.
9. **`mirror: x` and per-side bones are mutually exclusive, and nothing says so.** One part + one
   bone covers both sides (as the brief predicted), so this creature's "four limb chains" are two
   authored chains of two bones each. That is fine for a rest sheet and wrong for skinning: a
   mirrored foreleg cannot be posed independently of its twin. Wave 4 has to decide whether the
   mirror copy gets a derived mirrored bone, and `sculpt-craft`'s armature section should state the
   rule either way.
10. **Deformed parts inflate the lattice super-linearly.** 611 ms at rest, 2544 ms with one
    over-bent horn, against the idol's 179 ms — the grown boxes push the grid out in every axis at
    once. Nothing is wrong, but a `check-scene` line naming the part whose box grew the lattice would
    turn a mysterious 4× into a one-line diagnosis, and the cost model in `dev-docs/ludion-sculpt`
    should carry the deformer term.
11. **No way to ask "which part is this pixel?"** Renders 6 and 11 were spent identifying geometry I
    could see but not name. `--part <name> --isolate` answers it one guess at a time; a
    `--views front --by-part` false-colour render, or `--part` accepting several names, would answer
    it in one.
12. **`taper`'s `from`/`to` are "near/far along the axis", which needs a re-read every time.** Naming
    them `low`/`high` (or documenting them as "at the part's minimum and maximum coordinate along
    `axis`") would remove the coin flip; I got it right once and checked the doc twice more.
13. **`render --model` writes `<stem>.sheet.png` beside the document unless `--out` is given** — a
    scratch-only workflow has to pass `--out` on every single render. A `--out-dir` (or honouring an
    env var) would shorten the loop's most repeated command.

### Was `--silhouette` decisive?

No. It was never reached for during the 16 renders, and a deliberate trial after acceptance
(`--silhouette 60`) confirmed why: the `±X` profiles do show a legible quadruped with a tail and a
swept horn, but every defect this run actually fixed was invisible in them — the pendant under the
chest (a centreline detail hidden behind the legs in profile), the blown-out eye sockets (value, not
outline), the horn tips shed as strays (four vertices), the taper stealing a neighbour's surface and
colour (attribution, not shape). For a `mirror: x` creature `+X`/`-X` are identical by construction
and `+Z`/`-Z` nearly so, so six views buy two. `--bounds` plus a `--views front,iso` render answered
the same question in one image and also answered the shading half. A second independent vote to
delete the flag.

### After the divisor fix

Findings 1, 2 and 7 were fixed straight after this run: deformers now act about the part's own
centre for every shape (a capsule included), the per-part Lipschitz divisor is gone — the
conservative bound lives only in the grown box — and `L1158` names the part that owns the field
over the victim ("hidden by part `torso`"). The creature was re-meshed and re-blessed against it.

Mesh: 30856 triangles before decimation (was 32236), 8000 after, 3980 vertices, ~628 ms at 48/unit
on the RTX 3090 (was 627 ms — the divisor never touched the lattice size, only attribution). Grid
unchanged at 48x93x126.

Surface vertices, before → after: horn **554 → 388**, neck **32 → 82**, skull **136 → 180**,
tail **347 → 212**, belly **180 → 22**. The horn had been holding a third of the head; the neck and
skull took that share back, and the tail dropped to the geometry it actually has. The three tapered
parts (tail, horn, belly) were all mis-placed as well as over-weighted: a capsule's `from`/`to` are
model coordinates, so the taper had been dividing their *absolute* perpendicular coordinates — the
tail's y ≈ 0.9 divided by 0.5 put its far half somewhere it was never authored, which is why its
box reached the floor.

On the sheet: the same creature at rest from all six views — stance, dorsal ridge, glowing eyes and
horn sweep unchanged. The horns are now tapered spikes instead of broad flattened blades, and the
tail is a clean tapered needle sweeping back and slightly down instead of a thick drooping tube.
One retune was needed: with the taper correct the `belly` capsule sat entirely inside the torso and
raised the new `L1158` ("hidden by part `torso`"), so its two `y` coordinates were lowered by
`0.06` (`from` 0.9 → 0.84, `to` 0.85 → 0.79). It now reads as a slim underside line, 22 surface
vertices, with no return of render 6's chest nub from the front. Nothing else in the document moved.

Finding 3's complaint dissolves with finding 2: `--bounds` boxes are attribution boxes, and now that
a deformed part no longer wins the field outside its own geometry they track the shape again — the
tail's box is `(-0.102, 0.864, 0.493)..(0.102, 1.084, 1.138)` against `(-0.206, 0.435, 0.420)..
(0.189, 1.083, 1.138)` before. Finding 4's cone sentence and finding 12's taper wording are now in
`docs/sculpt-format.md`; the craft doc's capsule-horn `bend` example is true as written.
