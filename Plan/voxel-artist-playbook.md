# Voxel-artist playbook — for a small local model

> Status: **operational guidance**, written 2026-08-05 alongside `Plan/voxel-tooling-feedback.md`.
> This is the scaffold a low-capability model follows in the voxel-artist editor window.
>
> **A small local model playing this role is an idea, not a plan**: running one through opencode in
> the editor's terminal to work this playbook is proposed but unscheduled — nothing is built and no
> capability-floor baseline has been measured.
>
> **This file does not duplicate `ludion docs`.** It tells you *which* topic to fetch, *when*, and
> *in what order* — plus the handful of things the shipped docs do not say or get wrong. The engine
> docs remain authoritative on the format itself.
>
> **Maintenance rule:** several sections below are workarounds for tooling gaps listed in
> `Plan/voxel-tooling-feedback.md`. When a gap closes, delete its workaround here. Sections carrying
> a `[GAP n]` tag are the ones to delete.

---

## 0. What you are doing

A human is looking at one voxel model in a 3D viewport under real game lighting. You edit
`.voxel.yaml` files on disk. A file watcher reloads the viewport when you save — **you never reload
it yourself.**

Your job has two halves:
- **Geometry** — solved by text tools. You can get this right without ever rendering. Do so.
- **Value and colour** — *not* solved. Judge with a render, then hand the final call to the human.

Success is measured as: **how few times the human has to look before it is right.**

---

## 1. NEVER — absolute rules, no exceptions

Violating any of these does damage you cannot see.

| Never | Why |
|---|---|
| `asset.reload` | The host watches the folder and reloads on save. Calling it races the watcher. |
| `scene.load` | That is the human's rig toggle. You would throw away the rig they chose. |
| `component.set` on the stage subject's model | Which model is on the stage is the human's click. Swapping desyncs their browser. |
| `Read` a `.voxel.yaml` file | They reach **3 MB**. One read destroys your context. See §2. |
| Hand-edit a file whose header has `generated_from:` | It is write-only. Edit the script, regenerate. A repo gate rejects the commit otherwise. |
| Write YAML from a script | Forbidden. Scripts write JSON via `emit({ out })`; `ludion voxel from-json --in` writes the file. |
| Pipe a generator's stdout through PowerShell | The pipe re-encodes the bytes (`ludion explain L1135`). Use `emit({ out })` + `--in`. |
| Invent a key, glyph rule or CLI flag from memory | Fetch the doc or run `--help`. Every invented flag costs an iteration. |
| Use `Math.random()` in a generator | Output is byte-gated. Use `speckle(..., seed)`. |

---

## 2. Context discipline — the thing that will kill you

You have limited context. Voxel models are enormous text files. Protect yourself:

> Commands here are written as `ludion …` for readability. **They do not run that way** — see
> §2.5 for the real invocation before you type any of them.

- **Never `Read` a `.voxel.yaml`.** To inspect one, use:
  - `ludion check-scene <model>` — size, fill, connectivity
  - `ludion check-scene <model> --slice 12` — one layer, annotated
  - `ludion check-scene <model> --column 40,8` — one vertical profile
  - `head -35 <model>` — the header docstring and palette **only**
- **Fetch exactly one doc topic at a time**, and only when you are about to do that thing.
  `ludion docs` bare (listing) is cheap; a topic fetch is a whole file.
- **Do not re-read a doc you already read this session.**
- **Do not print command output you do not need.** Pipe through `tail -20` or `grep`.

---

## 2.5 How to actually run the CLI — read this before your first command

**`ludion` is NOT on PATH in this checkout.** Every command below is written as `ludion …` for
readability, but you must actually invoke it as:

```bash
cargo run -q -p ludion-cli -- <subcommand> …
```

**Two rules that will otherwise cost you an iteration each:**

1. **`cargo` must run from the repo root**, but your working directory is `games/vigil`. The shell
   working directory **does not persist between commands**. So prefix every CLI call:

   ```bash
   cd /d/Work/Ludion && cargo run -q -p ludion-cli -- check-scene games/vigil/scenes/voxels/x.voxel.yaml
   ```

2. **Paths change meaning with the `cd`.** After `cd /d/Work/Ludion`, a model is at
   `games/vigil/scenes/voxels/x.voxel.yaml`, **not** `scenes/voxels/x.voxel.yaml`. Getting this
   wrong produces a confusing "file not found", not a helpful error.

Commands that are **not** the ludion CLI (`cat`, `ls`, `head`, `node`) run fine from `games/vigil`
without a `cd` — but `node` scripts that import the shared library need repo-root-relative paths too,
so prefix those the same way.

**Copy this template for every CLI call:**

```bash
cd /d/Work/Ludion && cargo run -q -p ludion-cli -- <verb> games/vigil/<path> [flags]
```

---

## 3. Orient — do this first, every time

```bash
cat .ludion/editor-context.json
```

This tells you the selected model path, the active rig, and the camera. **Read it before acting on
any "this", "that", "here" or "the one I'm looking at".** Re-read it whenever the human uses such a
word again — it changes as they click.

Then, if you have not yet this session:

```bash
cd /d/Work/Ludion && cargo run -q -p ludion-cli -- docs voxel-format
# the schema. Read this before your FIRST edit, always.
```

---

## 4. Which doc to fetch, and when

Fetch **one**, at the moment you need it. Do not pre-load them all.

| You are about to… | Fetch |
|---|---|
| Make any edit at all, first time this session | `ludion docs voxel-format` |
| Decide colours, values, silhouette, detail level | `ludion docs voxel-craft` |
| Build anything larger than ~32x32x32 | `ludion docs voxel-ops` |
| Run a check and need the text views | `ludion docs voxel-authoring` |
| Render a sheet, lineup, or sweep and need the flags | `ludion docs voxel-sheets` |
| Write a generator script | `ludion docs voxel-generation` + `cat scripts/voxel/README.md` |
| Build or edit a model whose parts must move | `ludion docs voxel-assemblies` |
| Write or judge an animation clip | `ludion docs voxel-animation` |

If a flag is uncertain: `ludion check-scene --help`, `ludion render --help`, `ludion voxel --help`.
**Running `--help` is always cheaper than a wrong guess.**

---

## 5. Decide the size — arithmetic, not judgement

Two numbers decide everything: **world size** (how big it is in the game) and **density** (voxels per
world unit). Multiply them for the grid.

```
cells = (X_units x density) x (Y_units x density) x (Z_units x density)
```

`L1115` warns above **262,144** cells. That is a warning, not an error.

**Density by object type:**

| Object | Density | `scale:` to declare |
|---|---|---|
| Standard prop (crate, barrel, lantern) | 16 / unit | `0.0625` |
| Character / creature | pick so the model is 9–48 cells tall | varies |
| Large structure (house, wall), budget-conscious | 5–8 / unit | `0.2` – `0.125` |
| Large structure, detail matters more than file size | 16 / unit + `budget_exempt: true` | `0.0625` |

**Worked examples — copy the closest row:**

| World size | Density | Grid | Cells | Verdict |
|---|---|---|---|---|
| 1 x 1 x 1 u | 16 | 16x16x16 | 4,096 | Ideal. Hand-author. |
| 2 x 2 x 2 u | 16 | 32x32x32 | 32,768 | Fine. Top of hand-authoring. |
| 3 x 3 x 3 u | 16 | 48x48x48 | 110,592 | Fine. Use upscale + ops. |
| 8 x 12 x 7 u | 5 | 40x60x35 | 84,000 | Fine. This is house-a. |
| 8 x 12 x 7 u | 8 | 64x96x56 | 344,064 | Over budget. Needs `budget_exempt`. |
| 8 x 12 x 7 u | 16 | 128x192x112 | 2,752,512 | 10x over. Works, but **script-only**, ~2.9 MB file. This is house-c. |

**Rule:** if cells > 262,144 you must set `budget_exempt: true`, and you must use a generator script
— there is no other way to produce that many cells correctly.

---

## 6. Pick your rung — never start at the bottom

| Grid size | Method |
|---|---|
| Up to ~32³ | **Hand-author** the YAML. Or `ludion voxel new` to scaffold it. |
| 32³ – 64³ | **`ludion voxel upscale`** an existing verified smaller model, then sculpt with `ludion voxel apply` ops. |
| Above 64³, or no smaller ancestor exists | **Generator script** over `scripts/voxel/grid.mjs`. |
| Any size, but the parts must move independently | **Assembly** — `kind: assembly` + one `kind: part` file per part. §12. |

Going straight to a generator for a small model is a mistake. Going straight to hand-authoring for a
large one is a worse one — you will miscount columns and never recover.

---

## 7. The workflow — follow in order, do not skip gates

### Step 1 — Design on paper first (in your reply, before any file)

Write down, in plain text, before touching anything:
1. **World size** in units, and **density**. Compute the grid with §5.
2. **The silhouette in one sentence.** One dominant mass + at most two shapes that break its edge.
3. **The one emissive accent**, and where. Exactly one. Never two.
4. **Vertical registers** — a list like `y0-7 plinth, y8-99 walls, y100-117 slab, y118-191 roof`.

If you cannot write these four, you are not ready to build. Do not build.

### Step 2 — Check it will not collide with an existing model

```bash
ls games/vigil/scenes/voxels/
head -35 games/vigil/scenes/voxels/<nearest-existing>.voxel.yaml
```

Read the **header docstring only** (that is what `head -35` is for). If your silhouette description
matches an existing model's, change yours. Two models with the same outline is a failure.

### Step 3 — Build it

For a generator script, place it beside the model as `<name>.voxel.gen.mjs`. Import path from
`games/vigil/scenes/voxels/` is:

```js
import { Grid, repeat, speckle, assertConnected, emit } from '../../../../scripts/voxel/grid.mjs';
```

End it with `emit({ ..., out: process.argv[2] })` so it writes the JSON intermediate itself — never
move it through a shell pipe (§1).

Build order that avoids the known traps:

1. Build only the **left half** (`x <= (W-1)/2`) for anything symmetric.
2. Apply `speckle(...)` for weathering — **still on the half only**.
3. Call `g.mirrorX({ keep: 'low' })`.
4. **Only now** add deliberately asymmetric features.
5. Call `assertConnected(g)` immediately before `emit(...)`.

`mirrorX` warns on stderr when it discards non-empty cells from the overwritten half. Any
`mirrorX discarded N cells` line means your build order is wrong — treat it as a gate failure.

### Step 4 — GATE A: does it validate?

```bash
cd /d/Work/Ludion && node games/vigil/scenes/voxels/<name>.voxel.gen.mjs \
  games/vigil/scenes/voxels/<name>.voxel.json \
  && cargo run -q -p ludion-cli -- voxel from-json \
  --in games/vigil/scenes/voxels/<name>.voxel.json --validate
```

Nothing but the intermediate is written. **Do not proceed until this prints a line ending
`validated, nothing written`.** If it errors, go to §9 and fix. Do not write the model file "to see
what happens". The `.voxel.json` is a build artifact — never commit it.

### Step 5 — Write the file

```bash
cd /d/Work/Ludion && cargo run -q -p ludion-cli -- voxel from-json \
  --in games/vigil/scenes/voxels/<name>.voxel.json \
  --out games/vigil/scenes/voxels/<name>.voxel.yaml
```

### Step 6 — GATE B: is the structure right? (no GPU, cheap, do all of these)

```bash
cd /d/Work/Ludion && cargo run -q -p ludion-cli -- check-scene \
  games/vigil/scenes/voxels/<name>.voxel.yaml --exposure --from game
cd /d/Work/Ludion && cargo run -q -p ludion-cli -- check-scene \
  games/vigil/scenes/voxels/<name>.voxel.yaml --mirror-x
```

Read the output against this table. **Every row must pass before you render.**

| Look for | Must be | If not |
|---|---|---|
| `1 component` | exactly 1 | An `L1129` warning names the gap and the bridge cells. Fix it. |
| `emissive faces` direction | the bulk pointing `-Z` for a front-facing accent | If `+Y` dominates, your accent glows at the sky. Move it to a vertical face. |
| `emissive faces` count | > 0 | Zero exposed = your accent is buried inside solid cells. |
| `from game view` occlusion | most facing faces NOT occluded | A deep recess hides the accent from the 58° camera even when every face is exposed. Shallow the recess (~4 cells). |
| Enclosed count on a **detail** glyph | near zero | Detail you carved that can never be seen. Reclaim those cells. |
| Enclosed count on the **body** glyph | large is fine | Solid interiors are correct and cost no geometry. |
| `meshed N quads` | no target — note it | The real render cost, and the number that justifies (or questions) `budget_exempt`. Quote it in your report. |

`--mirror-x` prints asymmetric cells clustered into **regions** with bounding boxes and glyph sets.
A few tight regions matching your declared deliberate asymmetries = pass; many scattered regions =
a build bug. Quote the region lines in your report instead of guessing.

### Step 7 — GATE C: how does it actually look?

Always render under the **night rig**, never the default bright sheet:

```bash
cd /d/Work/Ludion && cargo run -q -p ludion-cli -- render \
  --model games/vigil/scenes/voxels/<name>.voxel.yaml \
  --rig games/vigil/scenes/env-night.yaml \
  --out games/vigil/.ludion/sheets/<name>-night.png
```

Then the gameplay angle at **true size** — `--true-size` computes the framing from the model's
declared `scale:`, so the render shows the model at the pixel coverage it actually earns in game.
Never pick `--width`/`--height` by hand for this step:

```bash
cd /d/Work/Ludion && cargo run -q -p ludion-cli -- render \
  --model games/vigil/scenes/voxels/<name>.voxel.yaml \
  --rig games/vigil/scenes/env-night.yaml --views game --true-size \
  --out games/vigil/.ludion/sheets/<name>-gamesize.png
```

Then **look at the image** and answer three questions in writing:
1. Can you name the object from its outline alone?
2. Does the body separate from the ground, with the emissive ignored?
3. Is there exactly one bright read, not two competing ones?

### Step 8 — GATE D: calibrate against a neighbour

Render your model and its nearest existing neighbour into **one lineup sheet** — it is not
optional, it is the single most common source of a wrong result:

```bash
cd /d/Work/Ludion && cargo run -q -p ludion-cli -- render \
  --model games/vigil/scenes/voxels/<name>.voxel.yaml,games/vigil/scenes/voxels/<neighbour>.voxel.yaml \
  --rig games/vigil/scenes/env-night.yaml --views game \
  --out games/vigil/.ludion/sheets/<name>-lineup.png
```

One image, both models side by side under one rig at honest relative sizes. **Is your model's lit
face brighter or darker than the neighbour's?** If brighter, your albedo is too high — see §8.

### Step 9 — Hand it to the human

State the **concrete result**, never the intention:

> Good: `"S 0.112 -> 0.078, trim Q 0.098 now lighter than the body; emissive faces 640 of 646
> pointing -Z; 1 component; the arch and step copings read at 1100x1000 game size."`
>
> Bad: `"I made it darker and it should look better now."`

Then ask them to select the model on the stage and switch to the night rig, and name the **specific**
taste calls you want (a colour, an intensity) — not "what do you think".

---

## 8. Palette starting values — so you do not need taste

This project's game is dark. **Author colours 2–3x darker than the read you want.** These are linear
RGB, night-calibrated for vigil. Start here, then adjust.

| Material | Linear value | Notes |
|---|---|---|
| Creature body, near-black | `0.03` – `0.08` | Emissive carries the read, not albedo |
| Weathered stone, facade **broken up** by dark elements | `0.10` – `0.15` | e.g. dark roof + timber + windows present |
| Stone, facade **uniform** with nothing dark in it | `0.07` – `0.09` | A uniform surface reads a full step lighter |
| Dressed trim (arch, quoins, copings) | body value **+ 0.02** | Lighter than the body so trim *draws* the shape |
| Roof slate | `0.04` – `0.05` | |
| Dark timber | `0.05` – `0.07` | |
| Interior void / soot | `0.015` – `0.02` | |
| Fallen rubble | `0.04` – `0.06` | |

**Emissive intensity — do not exceed these:**

| Use | `emissive` |
|---|---|
| Quest beacons (game-owned, do not compete) | `4.0` |
| A lit window | `2.0` – `2.5` |
| A dim lamp | `1.5` – `2.0` |

**The rule that matters most:** a uniform surface reads lighter than a broken-up one at the *same*
albedo. If your model is mostly one material, go darker than the table says and put the lighter value
on the trim.

To pick between candidate values, sweep them into one labelled sheet instead of re-rendering one at
a time — one command answers "which of these three":

```bash
cd /d/Work/Ludion && cargo run -q -p ludion-cli -- render \
  --model games/vigil/scenes/voxels/<name>.voxel.yaml \
  --rig games/vigil/scenes/env-night.yaml --views game \
  --sweep /palette/<glyph>/emissive=1.6,2.2,2.8 \
  --out games/vigil/.ludion/sheets/<name>-sweep.png
```

`--set /palette/<glyph>/...=<v>` previews a single value the same way; neither touches the file —
when the human picks a winner, write it into the palette (or the generator) yourself.

---

## 9. Failure playbook — symptom to exact fix

| Symptom | Cause | Fix |
|---|---|---|
| `L1114` ragged row / wrong layer count | Grid does not match `size` | Pointer names `/slices/y/z`. If generating, your loop bounds are wrong. |
| `L1114` glyph names no palette entry | Used a glyph you did not define | Add it to `palette`, or make the cell `.` |
| `L1113` unknown key | Typo in a top-level key | The error suggests the nearest match. Use it. |
| `L1115` over budget | Too many cells | Expected if deliberate — add `budget_exempt: true`. Otherwise lower density. |
| `L1129` disconnected | A stray piece | The warning names the **nearest cell pair and the bridge**. Add face-adjacent `set` cells there. A diagonal `line` looks connected and is not. |
| `L1006` block scalar rejected | Used `\|` or `>` in YAML | Use a sequence of quoted row-strings. |
| `L1132` set outside grid | An exact coordinate missed | Volume ops clip; `set` does not. Fix the coordinate. |
| `L1133` | Bad generator JSON | `ludion explain L1133` prints the required shape. |
| Model vanished from the viewport | Parser rejected it; reload dropped the asset | Run `check-scene` — it names the diagnostic. Fix, save again. |
| Emissive glows upward, not at the player | Accent on a top face | `--exposure` shows `+Y` dominant. Move the accent to a vertical face. |
| Emissive reads as two slivers | Recess too deep for the 58° camera | `--exposure --from game` shows facing faces occluded by own geometry. Shallow the recess to ~4 cells until occluded is near zero. |
| Detail invisible in game but crisp on the sheet | Feature under ~2 voxels | The sheet flatters 3–7x. Delete the detail or make it bigger. |
| Model reads pale under the night rig | Albedo too high for a uniform surface | Darken body, raise trim above it. See §8. |

---

## 10. Stop conditions — read this when you are stuck

Small models fail in two ways. Both have a hard stop:

**Flailing** — many attempts, no progress:
- **If the same gate fails 3 times, stop editing.** Report to the human: what you tried, the exact
  error, and what you think is wrong. Do not attempt a 4th variation.
- **Never re-derive something already established in this session.** If you computed a grid size, it
  is still that size.
- Change **one thing** per iteration. Two changes and a passing test tell you nothing.

**Premature confidence** — declaring done without checking:
- **You may not say "done", "fixed" or "it works" unless you have run Gate A, B and C** and can quote
  their output.
- "It should look better now" is not a result. Quote numbers.
- If you did not render it, you do not know what it looks like. Say so.

---

## 11. Final checklist before you report

Every line must be checkable, not believed:

- [ ] `check-scene` prints `OK` and `1 component`
- [ ] `--exposure` shows the emissive accent exposed, facing the right way
- [ ] Exactly **one** emissive read on the model
- [ ] Rendered under `--rig games/vigil/scenes/env-night.yaml`, not the default sheet
- [ ] Rendered the gameplay view with `--true-size` (§7 Step 7)
- [ ] Compared against a neighbour model under the same rig (§7 Step 8)
- [ ] If the file has `generated_from:`, the generator is committed alongside it and
      `node scripts/voxel-drift.mjs` passes
- [ ] Header docstring states: silhouette intent, world size + density, front-face convention,
      and the deliberate asymmetries
- [ ] You did **not** call `asset.reload`, `scene.load`, or swap the stage's model
- [ ] Your report quotes concrete before/after numbers

---

## 12. Assemblies and clips — a model whose parts move

An **assembly** is one `kind: assembly` file (the palette, the placements, the hierarchy, the clip
bindings) plus one small `kind: part` file per part. Fetch `ludion docs voxel-assemblies` before your
first edit and `ludion docs voxel-animation` before you touch a clip; this section is only what the
first build of one taught us.

**Context.** Part files are lean — reading one is cheap, unlike a 3 MB `.voxel.yaml`. The assembly
file's header docstring is where the model's conventions live: `head -60 <model>.assembly.yaml`
before anything else.

**Building the rig**

- **Two segments per limb, never one.** At 32 cells tall a knee and an elbow are each ~7 cells of
  travel. One-segment limbs animate like scissors.
- **Use the humanoid names exactly** (`torso`, `head`, `upper-arm-l`, `lower-arm-l`, …). A shared
  clip file addresses parts by name and simply skips names it does not find, so a typo is a limb that
  silently never moves. Anything outside the convention (a lantern, a scabbard) is model-specific,
  rides its parent, and no shared clip will drive it.
- **One file per mirrored pair**: author the `-l` part and give its `-r` twin `mirror: x`. Pivots are
  authored **unmirrored**, so both sides quote the *same* pivot numbers.
- **Pivots sit on the anatomical joint**, and a shoulder or hip quotes the *inboard* end — the x of a
  pivot does not affect an X-axis swing, and an inboard pivot keeps the plug cluster tight.
- **Joint plugs**: author the child's last two courses inside the parent's volume, so rotation slides
  geometry through geometry instead of opening a seam. That overlap is expected and silent.

**Gate B, for an assembly** — `ludion check-scene <model>.assembly.yaml`:

- [ ] `1 component` at rest and **zero `L1143`** warnings. That pair is the acceptance test for every
      new limb and every moved pivot. An `L1143` naming a pivot distance is almost always a pivot
      authored at the wrong end of the limb.
- [ ] `--exposure` still passes the one-emissive rule. It measures **direction, not depth**: a flame
      recessed one cell behind its frame scored four exposed faces and was invisible in game. Use
      `--from game`.

**Gate C, for a clip** — `ludion render --model <model>.assembly.yaml --clip walk --strip 6`:

- [ ] every phase reports `1 component`. More than one means a joint came apart mid-swing.
- [ ] use `--strip` to judge the **arc** (travel, detachment) and `--time <s>` to read a single pose
      at full sheet size. They are different questions; running the strip first is cheaper.
- [ ] **author keyframe times on 60 Hz tick multiples** (`0.05`, `0.1`, `0.25`). Clip time is always a
      whole number of ticks, so a `t` between them is never sampled exactly.

**Two things no check catches — say so in your report and ask for a look:**

- **Hinge direction.** A knee flexes −X and an elbow flexes +X. Reversed, it validates, meshes,
  connects, and reads as a broken puppet.
- **Value.** `--set`/`--sweep` refuse an assembly (`L4008`) — the palette must be edited in the file
  by hand and re-rendered. Settle geometry and pivots *first*; every value iteration is expensive
  until that deferral is lifted.

**Wiring.** An assembly declares no `scale:`, so the entity's `Transform` scale sets its world height.
State the intended world height and the scale you used, and check it against the model it replaces —
matching collision, camera and cast reach to the previous model is a decision for the human, not a
default.
