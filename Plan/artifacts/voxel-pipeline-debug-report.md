# Qwen 3.6

# Voxel Pipeline Debugging Report — statue-hd Connectivity

**Date:** 2026-08-02
**Model:** `scenes/voxels/statue-hd.voxel.yaml` (24×28×20, generated)
**Issue:** 3 disconnected components instead of 1

---

## What happened

The statue-hd generator script produced a model with 3 disconnected 6-connected
components: the main body (torso/legs/head/spine), and two detached arms+hands
(left and right). The `L1129` warning flagged sample cells `(2,0,1)` and
`(19,0,1)` — the left and right hands at ground level.

## How I found the root cause

The diagnosis took **six regeneration cycles** and roughly 45 minutes of
iterative narrowing. Here is the path:

### Step 1: Assumed hand-to-arm gap (wrong)

The initial hypothesis was that the hand ellipses at y=0-3 didn't connect to
the arm lathe ending at y=3. I extended the arm endpoint from y=3 to y=2,
then to y=0, and extended the hand loop — all without effect. The model still
had 3 components.

### Step 2: Slice-by-slice inspection

I used `check-scene --slice` on every layer from y=0 through y=19 to trace
where filled cells appeared. At y=4 the arm (F) was at z=2-3 and the hand (D)
was at z=1-3 — they shared z and were adjacent in y. This *should* have been
connected, but wasn't.

### Step 3: Manual flood fill on the JSON intermediate

I wrote a Node.js script that parsed the JSON intermediate (`statue-hd.json`)
and ran the same 6-connected flood fill the Rust checker uses. The result: the
hand *was* connected to the arm in the JSON data (141 cells from hand to arm
tip). But the YAML validation still reported 3 components.

**This was the turning point** — it meant the connectivity algorithm was
working correctly and the gap was elsewhere in the model, not at the
hand/arm junction I'd been staring at.

### Step 4: Component enumeration

I extended the flood fill script to enumerate *all* components. The result:

| Component | Cells | Sample | What it is |
|-----------|-------|--------|------------|
| 1         | 141   | (2,0,1)  | Left hand + arm |
| 2         | 141   | (19,0,1) | Right hand + arm |
| 3         | 1058  | (5,0,14) | Everything else (torso, legs, head, spine) |

Each arm+hand is one piece of 141 cells — connected to itself but **not
connected to the torso**. The gap is at the shoulder, not the wrist.

### Step 5: Shoulder junction inspection

Looking at slices y=17-19, the arm shoulder root at `{cx:3, cz:5, rx:2.5,
rz:1.5}` fills x=0-5, z=3-6 while the torso at that height is at x=4-18,
z=5-10. The arm's x-range (0-5) and the torso's x-range (4-18) overlap at
x=4-5, but the arm's z-range (3-6) and the torso's z-range (5-10) only
overlap at z=5-6. Checking the actual cells: the arm fills (4-5, y, 5-6)
but the torso doesn't — the torso lathe at y=19 interpolates to cz=7 with
rz=2, filling z=5-9 but starting at x=4 only at the edge where rx tapers.
The actual filled cells don't share a face.

### Step 6: Fix — widen the shoulder

Changed the shoulder root from `{cx:3, cz:5, rx:2.5, rz:1.5}` to
`{cx:4, cz:6, rx:3, rz:2}` — moving it toward the torso center and widening
both radii. This created face-adjacent overlap at x=5-7, z=5-7. One
regeneration later: **1 component**.

---

## Issues encountered

### 1. L1129 sample cell misdirects the investigation

The warning reports `(2,0,1)` — a cell in the hand. This is correct (it *is*
in the smallest stray component) but it sends the investigator to the wrong
end of the disconnected piece. The actual gap is 19 layers away at the
shoulder. **There is no information in the diagnostic about where the
disconnect actually occurs.**

### 2. No way to query "which two components are closest"

The flood fill identifies components but doesn't report *where* they fail to
connect. The nearest cells between component 1 (arm) and component 3 (torso)
are at y=18-19, z=5-6 — but discovering this requires manually inspecting
every slice or writing a custom script.

### 3. Lathe interpolation is opaque

The `lathe()` function interpolates cx, cz, rx, rz between profile endpoints.
Given a profile, there is no way to query "what cells does this fill at y=18?"
without running the generator and inspecting output. The `profileAt()` helper
exists in `grid.mjs` but it returns decimal parameters (cx, cz, rx, rz) — not
the integer cell ranges that actually get filled. An author tuning a shoulder
position is reasoning about cells, not about ellipse parameters that will be
rasterized to cells by an inequality test.

### 4. Overwrite order matters silently

The hand ellipses (D) are drawn after the arm lathe (F), overwriting F→D at
y=0-3. This is intentional (the hand should be dark claws), but it means the
arm's contribution at those layers is invisible in the output. If the hand
loop had a bug and stopped at y=2 instead of y=3, the arm would show through
as F — a visual clue that something is wrong. But with the overwrite, both
the generator author and the slice viewer see only D and have no record that
the arm passes through those layers.

### 5. No preview before full regeneration

Each hypothesis requires: `node script > json && ludion voxel from-json --out yaml && check-scene yaml` — three commands, ~5 seconds each. There is no "dry-run connectivity check" on the JSON intermediate that skips the YAML write step. The `--check` flag exists but requires a committed file to compare against; it doesn't answer "is this connected?" in isolation.

### 6. Slice display is ASCII-only

The `--slice` output is an ASCII grid that requires careful character counting
to verify x/z positions. For a 24-wide model, distinguishing x=4 from x=5 by
eye is error-prone. The column header `012345678901234567890123` helps but
doesn't eliminate the cognitive load of mapping characters to coordinates.

---

## Suggestions for tooling improvements

### High priority (blocks agent autonomy)

**A. Report the gap, not just the orphan**

When L1129 fires, compute the closest pair of cells between each pair of
components and report them:

```
warning[L1129]: 3 disconnected components (1058, 141, 141 cells)
  gap between component 1 and 3: nearest cells (6,18,6) and (7,18,6), distance 1
  gap between component 2 and 3: nearest cells (17,18,6) and (18,18,6), distance 1
  --> /slices
```

This tells the author exactly where to look. The computation is cheap — for
two components of size A and B, it's O(A×B) but both are small in the stray
case (141 × 1058 ≈ 150K distance checks, trivial).

**B. Connectivity check on JSON intermediate**

Add `ludion voxel from-json --validate` (or a new `ludion voxel
check-intermediate <file>`) that runs the L1129 check on the JSON without
writing YAML. This cuts the feedback loop by one command and ~2 seconds.

### Medium priority (reduces iterations)

**C. Lathe profile preview**

Add a `--profile` flag to `check-scene` that prints the interpolated
cross-section at each y for every lathe in a generated model's header, or
better: add a `profileAt()`-style query to the CLI:

```
ludion voxel profile --at 18 scenes/voxels/statue-hd.voxel.yaml
  y=18: cx=11.50 cz=8.00 rx=7.00 rz=2.50 → x=[4..18] z=[5..10]
```

Showing the integer cell range (not just the decimal parameters) is what an
author needs to verify overlap.

**D. Highlight overwritten cells**

When `from-json` processes a generator's output, track which cells were
written by more than one primitive and report the overwrite count:

```
info: 12 cells were overwritten by a later primitive (arm F → hand D at y=0-3)
```

This doesn't indicate a problem per se but confirms that the author's layering
intent is executing as expected.

### Lower priority (ergonomics)

**E. Richer slice display**

Color-coded terminal output (arm=F in one color, hand=D in another) or a
`--slice --highlight <glyph>` flag to dim all other glyphs would make the
ASCII grid readable at 24 columns. Alternatively, `ludion render --model
--views front` could serve as the visual slice tool — it already exists but
isn't mentioned in the connectivity debugging workflow.

**F. Generator self-check mode**

Allow a generator script to run a connectivity assertion before `emit()`:

```js
import { Grid, emit, checkConnectivity } from '../../../../scripts/voxel/grid.mjs';
// ... build model ...
const comps = checkConnectivity(g);
if (comps !== 1) throw new Error(`Expected 1 component, got ${comps}`);
emit({ grid: g, /* ... */ });
```

This fails the script with a clear message before any YAML is written, and
the error includes the component sizes and sample cells. The `checkConnectivity`
function would be a thin wrapper around the same flood-fill logic in
`grid.mjs` — keeping the connectivity check in one language (JS) for the
generator path and the other (Rust) for the validation path, both producing
the same result.

---

## Summary

The core problem is that **the diagnostic names the symptom (a stray cell) but
not the cause (a gap 19 layers away)**. Every other issue compounds the search
space: opaque lathe interpolation, no intermediate validation, ASCII-only
visualization, and a 15-second regeneration loop. Fixing A and B alone would
reduce this kind of debugging from ~45 minutes to ~5.


# Sonnet 5

How the process went

  I scaffolded with ludion voxel new
  --palette-from statue.voxel.yaml (joining the  
  existing statue colour language rather than    
  inventing a ninth grey), then built the whole 
  anatomy as one 41-op file — ellipsoid for the 
  body masses, box for the stepped spine ridge, 
  cylinder+ellipsoid for limbs, set for claws,  
  mirror for symmetry. --dry-run caught a real  
  bug before anything touched disk: a line op   
  with a diagonal Bresenham step produced a tail
  cell that was only edge-adjacent, not
  face-adjacent, and check-scene's connectivity 
  check flagged it as a stray component
  immediately. Swapped it for a face-adjacent   
  staircase of set ops and it came back clean — 
  one iteration, no render needed to catch it.  

  Shape was right in one pass, exactly as the   
  docs promised. Colour wasn't: I'd inherited   
  the base statue's palette via --palette-from,  
  and it turns out that palette itself reads    
  near-white under the real night rig — I       
  confirmed this isn't something I introduced by
  rendering the shipped statue.voxel.yaml        
  through the same rig, and it comes out equally
  washed out. I darkened statue-sonnet's
  flesh/claw/mold values into the guide's       
  "near-black creature" range and re-rendered;  
  it now reads as a dark, mottled shape with the
  eyes as the only reliable tell in low light,  
  which is what a shadow-lurking horde monster  
  should do.

  Tooling notes

  Worked well: voxel apply --dry-run +
  check-scene's connectivity/--mirror-x views   
  made the whole geometry phase a zero-render   
  loop — I only opened an image after the shape 
  was already provably correct. render --model's
  six-view sheet needed no scene authoring at   
  all. The --set JSON-pointer override on render
  --scene let me preview the model through the  
  exact shipped night rig
  (model-stage-night.yaml) without ever touching
  the live stage — I copied nothing, just       
  pointed the CLI at that file with overridden  
  Mesh.shape.voxel/Transform.scale, fully       
  respecting the "never touch the live stage"   
  rules in one-off headless renders.

  Friction:
  - The diagonal-line connectivity trap is a    
  sharp edge worth naming directly in the line  
  op's own help text — right now you only       
  discover it by reading a generic L1129 warning
  and reasoning out which op caused it (it only 
  names a sample cell, not the op index).       
  - There's no primitive for an angled/capsule  
  limb — only axis-aligned cylinder. Fine for   
  this blocky genre (I faked joints with        
  ellipsoid+cylinder pairs), but a real
  expressiveness gap if someone wants a
  diagonally-braced limb.
  - Getting a true night-rig preview required   
  knowing that model-stage-night.yaml exists and
  manually indexing into its entities array by  
  position for the --set pointers — a documented
  one-liner recipe for "preview a candidate     
  model under the shipped night rig without     
  touching the live stage" would've saved the   
  reconstruction. (Also hit a pure Git-Bash     
  gotcha: leading /entities/... pointers get    
  mangled into Windows paths unless you set     
  MSYS_NO_PATHCONV=1 — not an engine issue, but 
  cost a failed call.)
  - --palette-from is convenient for joining a  
  colour language, but it silently propagates   
  whatever calibration state the source model is
  already in — including, as I found here, a    
  pre-existing miscalibration. Worth flagging to
  whoever owns statue.voxel.yaml: it reads       
  near-white on the real night rig too.

  # Opus

   Process report

  Cost: 3 ops applies, 5 GPU renders, zero      
  rejected ops, zero failed applies. The        
  blockout came out 1-component on the first    
  dry-run.

  What worked

  - mirror: axis x, keep: low is the best thing 
  in the toolset. I authored only x 0..9 and got
  exact symmetry for free. It halved the work   
  and deleted an entire error class — I never   
  once verified symmetry by counting.
  - --dry-run + per-op changed-cell counts + the
  trailing now 20x20x20, 1464 filled, 1
  component is a real verify loop with no GPU.  
  Combined with check-scene
  --slice/--column/--mirror-x, shape was solved 
  as text, exactly as the persona promises.     
  - Surgical edits are genuinely surgical. My   
  17-line header docstring and all 20 layer     
  comments survived three apply runs
  byte-for-byte. Verified, not assumed.
  - The comment op — relabelling layers from    
  inside the ops file that made the change true 
  — is the right design. The comment can't drift
  from the edit.

  Friction, in order of what it cost me

  1. The contact-sheet rig is the wrong rig, and
  closing that gap is manual. This was ~half my 
  total time. The docs' own calibration
  paragraph admits the sheet runs 2–3× bright   
  and asks the author to do arithmetic — so I   
  hand-built .ludion/scratch/night-check.yaml,  
  copying Environment/PostProcess/moon out of   
  env-night.yaml. Three of five renders debugged
  my rig, not the model: I put the moon-fill at 
  z=0 instead of the plaza's z=−56 and over-lit 
  everything, then had the ground albedo wrong. 
  render --model X --rig scenes/env-night.yaml  
  would have removed all of it. This is the one 
  feature I'd actually ask for.

  2. --sweep can't reach a model's palette. It's
  scene-JSON-pointer only. The single loop the  
  docs name as the recurring project-wide cost —
  palette value calibration — is the one        
  --sweep can't drive. I had to copy the model  
  to scratch, patch it with Python, and place   
  two entities side by side to get an A/B.      

  3. Emissive proportion isn't checkable as     
  text, and that's the one defect I shipped into
  a render. The mould crest read as a glowing   
  green lightbar from the game camera — 12      
  emissive cells on the +Y face, which is the   
  face a steep camera stares at. check-scene    
  could report emissive outward faces by        
  direction ("22 of 28 emissive faces point +Y")
  and I'd have caught it before the GPU. That's 
  a new capability, not a polish item.

  4. mode: paint before a mirror is an
  undocumented footgun. My paint ops happened to
  stay in x 0..9. Had any written x ≥ 10, the   
  later mirror would have silently discarded    
  them with no warning. A lint — "op 14 wrote 6 
  cells at x≥10, overwritten by op 20 (mirror   
  keep: low)" — is cheap and catches a real     
  mistake.

  5. ludion docs engine-guide prints the whole  
  ~1500-line doc. I used --path + sed to read   
  the three sections I needed. Fine for me;     
  section-anchored fetch would help a tighter   
  context.

  One process note against myself: I eyeballed  
  the value A/B and called the two "nearly      
  identical." Measuring said 86 vs 70 — a real  
  19% difference the ACES curve compresses at   
  the dark end. At night values, measure; don't 
  look.

  Not done

  The model isn't wired into the game (no kind: 
  enemy data-doc, no roster entry) — you asked  
  for the model, and adding it would perturb the
  committed run's seeded draws. It's
  uncommitted; say the word and I'll commit it  
  via commit.mjs path-scoped. Scratch artifacts 
  are in .ludion/scratch/ and .ludion/sheets/   
  (both gitignored).

  # Fable

  Process report

  Overall: the tooling carried this. The whole  
  model took one upscale, one grow, ~40 ops     
  across three files, two one-line hand-edits   
  (palette entry, header), and four renders. No 
  hand-counting of columns in 20-char strings,  
  ever. Honest findings:

  What worked well
  - voxel upscale is exactly as advertised — a  
  correct, connected, palette-identical 18³ base
  in one command, with comments carried and     
  scale halved so world footprint was never at  
  risk. Coordinate math from base to upscaled   
  (2x+offset) was trivial to do in my head.     
  - --dry-run + per-op cell counts answered "did
  the ellipsoid land" without a render every    
  time; a count of 4 where I expected 2 would   
  have flagged an off-by-one instantly.
  - check-scene --slice with the ruler and      
  front/back markers made verifying coordinates 
  before writing ops nearly free — I confirmed  
  eye and muzzle positions in one print instead 
  of trusting arithmetic.
  - The author-symmetric → mirror →
  asymmetric-pass workflow is excellent:        
  --mirror-x then serves as an audit that lists 
  (2x+offset) was trivial to do in my head.     
  - --dry-run + per-op cell counts answered "did
  the ellipsoid land" without a render every    
  time; a count of 4 where I expected 2 would   
  have flagged an off-by-one instantly.
  - check-scene --slice with the ruler and      
  front/back markers made verifying coordinates 
  before writing ops nearly free — I confirmed  
  eye and muzzle positions in one print instead 
  of trusting arithmetic.
  - The author-symmetric → mirror →
  asymmetric-pass workflow is excellent:        
  --mirror-x then serves as an audit that lists 
  precisely the deliberate asymmetry.
  - Surgical edits are real: every apply        
  reported "every other byte unchanged," and the
  layer comments I set via comment ops sit      
  exactly where the anatomy is.

  Friction / missing
  1. set has no mode: paint. Volume ops paint;  
  the point op doesn't. I worked around it with 
  single-cell box ops, which reads oddly in the 
  ops file. The L1131 error naming the accepted 
  keys made this a 30-second fix, but a
  paint-capable point op would be the natural   
  shape.
  2. No visibility/exposure query — the gap I   
  actually felt. Before painting mottle I had to
  hand-reason whether each cell's faces were    
  exposed, and several first candidates turned  
  out fully enclosed (invisible). The base statue
  itself carries interior M flecks that do      
  nothing. An apply warning like "2 painted     
  cells are fully enclosed" (or a check-scene   
  --exposed) would have saved the most tedious  
  reasoning of the session.
  3. render --model has only the bright neutral 
  rig. The persona notes the night rig exists to
  retire palette-value arithmetic, but the CLI  
  can't render it — so value verification still 
  requires you at the stage. A --rig night on   
  the contact sheet would close that loop       
  agent-side.
  4. Small one: dry-run prints counts but no    
  mini-slice preview; pairing it with --slice   
  covers this, but a combined "slice after" view
  would shave a round-trip.

  Scratch artifacts (ops files, sheets) are in  
  .ludion/scratch/ and .ludion/sheets/, both    
  gitignored. Nothing committed — say the word  
  if you want it in Lore once you've judged it  
  at night