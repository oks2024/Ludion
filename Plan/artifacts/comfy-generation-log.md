The convergence metric of `Plan/comfy-generation.md` decision 7: one line per subject generated in
the arc, candidates counted as submitted images or meshes before acceptance, read against the sculpt
baseline in `Plan/artifacts/mesh-modeling-log.md` — the idol cost 10 sheet renders and ~191 k tokens
(the whole subagent run, blessing included), the creature 11 sheets (16 renders) and ~155 k. A
generated idol that costs more than that, or needs more human taste per attempt, is the arc's answer
even if the picture is prettier.

| subject | wave | candidates | sheets read | tokens | wall time | authoring model and harness | source access | human intervention | review fidelity | verdict |
|---|---|---|---|---|---|---|---|---|---|---|
| idol — portrait run 1 (seed 7) | 1 | 8 | 1 | — | 94.1 s (cold: model load) | opus / claude-code subagent | recipe + docs, no engine source | none | sheet read by the model | Subject found on the first eight: crouching winged stone idol, strand-fringed mouth, green eye, dark plinth, every candidate usable as a picture. One prompt bug, unanimous across seeds — every candidate wears a peaked cap, because "a low dark plinth with a flat cap" binds *flat cap* to the head, not the plinth. Eye count wrong on 7 of 8 (a matched green pair, not one eye). |
| idol — portrait run 2 (seed 100) | 1 | 8 | 1 | — | 22.4 s | opus / claude-code subagent | recipe + docs, no engine source | none | sheet read by the model | Cap gone on all eight ("bare-headed with nothing on its head"; the plinth clause now says *flat square top*). The eye moved but did not resolve: 5 of 8 now carry a glowing green **third** eye above a normal pair, 1 shows no glow at all. Heads flattened to bald domes and the brow ridge the brief asks for was lost. |
| idol — portrait run 3 (seed 200) | 1 | 8 | 1 | — | 22.5 s | opus / claude-code subagent | recipe + docs, no engine source | none | sheet read by the model | "exactly one eye … no other eyes and no eye sockets" lands: 7 of 8 are true cyclopes under a heavy overhanging brow, strand mass correct, plinth correct. The remaining defect is the silhouette — wings tucked flat against the back and fused into the body mass on 5 of 8, which is exactly the edge an image-to-mesh mask stage has to find. |
| idol — portrait run 4 (seed 300) | 1 | 8 | 1 | — | 22.4 s | opus / claude-code subagent | recipe + docs, no engine source | none | sheet read by the model | **The accept run.** Asking for the wings "raised and spread clear of its back with a gap of background visible between each wing and the body" opened the silhouette on all eight, and the ground plane left the frame. 5 of 8 hold the single eye. **#7, seed 307** accepted: one glowing green eye, deep brow, strand-fringed mouth, wings spread symmetrically with clean background between wing and flank, four distinct limbs on a low dark plinth with a flat square top, whole statue inside the frame, evenest background of the eight. |
| idol — variation (img2img, denoise 0.55, seed 11) | 1 | 4 | 1 | — | 12.2 s | opus / claude-code subagent | recipe + docs, no engine source | none | sheet read by the model | The subject survives: all four keep the pose, the wing spread, the plinth and the strand mass — recognisably the same statue re-lit and re-detailed, which is what `docs/generation-craft`'s 0.5–0.6 row promises. Two drifts: 2 of 4 grow a second eye back, and all four warm the stone from weathered grey-green to pale sandstone. |
| **idol — total** | 1 | **36** (32 portrait + 4 variation) | **5** | **142 k** (the harness figure for the whole subagent run) | **173.6 s** of GPU across five runs, one cold load | opus / claude-code subagent | recipe + docs, no engine source | **none** | sheet read by the model | **The author's read, not a ruling.** Four runs and under three minutes of GPU to a reference that is on-brief in every clause and is a better image-to-mesh input than anything the sculpt path produces. Against the sculpt idol's 10 renders and ~191 k tokens, the machine is not where the cost is: three of the four runs were spent on two failures of *English* — a modifier binding to the wrong noun, and "a single eye" read as a distinction rather than a count — not on taste. What the loop does not give is the sculpt path's articulation: the accepted asset is one frozen 1024² picture with no parts, no bones and no palette. |
| idol — mesh (TRELLIS.2 textured, from the accepted reference) | 2 | 1 | 5 | **235 k** for the run, accept, sheets, golden and one code fix (the harness figure); plus a one-time **200 k** authoring and smoking the reusable workflow, not charged to the next subject | 485.5 s, one run, cold (≈5 GB of weights pulled over SMB at ≈6.5 MB/s; 485.17 s of it server-side) | opus / claude-code subagent | recipe + docs + engine source (the fresh accept path, one defect fixed in it) | none | both contact sheets read by the model, plus a two-scalar material probe | **The author's read, not a ruling.** One run, one candidate, nothing rerolled — the first mesh is shippable, and the reference's every clause survived the round trip: a crouching winged creature, strand-fringed mouth under a heavy brow, wings spread clear of the body with daylight between wing and flank, four distinct limbs, on a low dark plinth with a flat square top. It beats the authored sculpt on silhouette and on surface by a wide margin — baked vein detail on the wing membranes, weathered pitting on the stone, a legible face — and loses to it on two things the machine cannot supply: the green eye is *painted* (a dull olive-green in the base-color map, no emissive map in the chain at all) where the sculpt's two eyes *glow*, and the mesh is one closed 4190-vertex shell with no parts, no bones and no palette. The density rule held exactly: remesh 256 → decimate 5000 landed 4986 triangles with no `L1112`. The two known texture risks did not materialise in the render: at the importer's own `metallic: 1.0` the image differs from `metallic: 0.0` below what I can see at 800×600, and no black AO blotch is visible anywhere — so **no hand-edit was made**. Cost against wave 1's sculpt row: 1 candidate and 5 images here against the sculpt idol's 10 renders, for a better-looking prop with a worse-behaved asset (two size-allowlist entries, and the one thing a designer would ask for — a glowing eye — is not in it). |
| creature head — multi-view (Pixal3D, four `render --model` views of an authored blank) | 3 | 5 meshes (4 on the blank, 1 diagnostic) + 2 preview images | 20 renders, 15 images read | — (not surfaced by this harness run) | 1 478.8 s of GPU across six submissions, one cold (1 013.5 s of it a single structure-resolution-64 run that shattered) | opus / claude-code subagent | recipe + docs + engine source read-only (`model_sheet.rs` for the rig, `sculpt.rs` for the new part) | none | every sheet and every conditioning view read by the model | **The author's read, not a ruling — and it is a negative on the premise.** The round trip closes mechanically: the accepted mesh drops into `creature-generated.sculpt.yaml` as `shape: mesh` with `size: 0.72`, blends into the authored neck with no seam at `blend: 0.07`, meshes in 596 ms, and reads as one body. What it does not do is *buy* anything. Four-view conditioning **reconstructs a silhouette; it does not invent surface** — what came back has less detail than the five-primitive blank that went in, with the muzzle gone and the brow ridge stretched to twice the skull's width. The control experiment is the evidence: the same graph on the 22-part authored creature returns a recognisable quadruped (four legs, tail, horned head, every axis within 17 % of the authored box) but lumpy and softened, so the pipeline works and the fidelity ceiling is the finding, not a bug. Against wave 2's row — one prompt-driven TRELLIS.2 run that beat the sculpt idol on silhouette *and* surface — the multi-view path costs more (20 renders, 6 submissions, ≈25 min of GPU against 1 run) and returns less. **Where it is ahead of wave 2 is directability and cost of ownership:** the output lands where the author put the blank, and with no unwrap/bake the whole asset is one 391 KB `.mesh.yaml` that needs **no size-allowlist entry at all**, against the textured idol's two. |

## Runs

Run folders are ignored scratch under `games/vigil/generation/runs/idol-portrait/` and
`.../idol-variation/`; the run ids are in each `run.json` and in the accepted sidecar. Sheets read at
full size: `Plan/artifacts/comfy-generation-idol-sheet.png` (run 4) and
`Plan/artifacts/comfy-generation-idol-variation-sheet.png`.

Accepted: run 4 candidate **#7, seed 307** → `games/vigil/generation/references/idol-reference.png`
plus its `kind: origin` sidecar. `ludion gen check` reported `offline: ok` / `live: ok` with an empty
diagnostic list on every one of the five submissions; the live pass found no recipe or workflow
defect, and the workflow was never edited — every iteration was a recipe diff, as
`docs/generation-craft` requires.

**The prompt that converged** (the committed `idol-portrait.gen.yaml`, run 4):

> A small weathered stone idol: a crouching winged creature with exactly one eye, a single glowing
> green eye set deep under a heavy overhanging brow ridge, the rest of its face bare stone with no
> other eyes, a mass of short strands hanging over its mouth, two bat wings raised and spread clear
> of its back with a gap of background visible between each wing and the body, squatting on a low
> dark plinth with a flat square top. Single object, centered, the whole statue inside the frame with
> clear space around it, flat even grey backdrop with no ground plane and no horizon line, soft
> frontal lighting, no cast shadow, no text.

Wave 2's mesh run is `runs/idol-mesh/20260918T074634Z-9df3e6b8` at seed 400, one candidate, accepted
into `games/vigil/scenes/meshes/idol-generated/` — `scene.yaml`, one `meshes/mesh-0.mesh.yaml`
carrying its `origin:` block, and three content-hash PNGs (base color, ORM, normal; the occlusion
reference shares the ORM file, glTF's packing). A mesh run has no contact sheet — `gen sheet` refuses
one with `L1171` — so the two model sheets are the read:
`Plan/artifacts/comfy-generation-idol-mesh-sheet.png` against
`Plan/artifacts/comfy-generation-idol-sculpt-sheet.png`, same 900×600 extent, same six views, same
synthesized rig.

Base seeds were bumped per run (7 → 100 → 200 → 300) so a sheet's own labels say which run it came
from. Worth keeping as a convention: `docs/generation-craft` already warns that a seed is not stable
across a prompt edit, so the label is not a comparison — it is the only mark on the image that
identifies the run.

## Tool improvements, most time lost first

1. **The size gate refuses every file this loop produces.** `gen accept` of a 1024² candidate writes
   1.33 MB; the two contact sheets are 2.40 MB and 1.38 MB. Decision 20's line is 1 MiB and none of
   the three is allowlisted — `node scripts/size-gate.mjs --list` names all three as "not
   allowlisted". Neither `gen accept` nor `gen sheet` has a downscale, extent or quality knob (their
   `--help` offers `--threads`, `--quality` and `--render-size`, none of which touch the written
   file), so there is no answer inside the tool: the arc either adds allowlist entries, or `accept`
   grows a re-encode-at-N-pixels option, or the committed reference is produced at a smaller latent
   than the one the taste loop reads. Found only at the gate, after the run.
2. **`gen check` without `--json` never says whether the live pass ran.** The text form prints
   `kind:` and the resolved overrides and stops. A live failure degrades to an `L1167` *warning*, so
   "the server answered and has every class" and "nothing answered, offline only" print the same
   thing. Every check in this loop had to go through `--json` to read `live`. Fix: two lines in the
   text form — `offline: ok`, then `live: ok` or `live: skipped (L1167)`.
3. **The kit's launch command as written does not work.** `powershell -NoProfile -ExecutionPolicy
   Bypass -File <path>/run.ps1 -Detach` dies before the script body: `$PSScriptRoot` is empty while
   Windows PowerShell 5.1 binds `param()` defaults, so `[string] $LogDir = (Join-Path $PSScriptRoot
   'logs')` throws a `Join-Path` empty-string error. Relative and absolute `-File` paths fail alike.
   It works under pwsh 7 invoked as `& <path>/run.ps1 -Detach`. Fix: default `$LogDir` and
   `$ComfyRoot` to `$null` and resolve them in the body (or use `$PSCommandPath`), so the script runs
   under either PowerShell edition.
4. **The tool's own path output is mixed-separator and mixed-root.** `gen run` prints a run folder
   with forward and back slashes inside one path; `gen sheet` prints `<run-id>\sheet.png`;
   `gen check --json` gives `recipe` with forward slashes and `workflow` with backslashes in the same
   object. Worse, the accepted sidecar records `recipe: generation/recipes/idol-portrait.gen.yaml`
   and `workflow: generation/workflows/txt2img.json` — **game-root-relative** — where
   `docs/generation.md`'s provenance example shows
   `games/vigil/generation/recipes/idol-portrait.gen.yaml`, repo-relative. One of the two is wrong,
   and the doc is what someone trusts a year later.
5. **Nothing hands you the run folder in a pasteable form.** The only route from `gen run` to
   `gen sheet` is to copy the first output line and repair its separators by hand, once per run.
   `gen run` could print the literal `ludion gen sheet <dir>` next step, or `gen sheet` could take a
   recipe path and default to that recipe's newest run.
6. **`gen sheet` and `gen accept` advertise three render knobs they cannot use.** `--threads`,
   `--quality` and `--render-size` each carry a full paragraph in `--help` on two verbs that compose
   a PNG grid and re-encode one file. `--url` at least says "accepted for uniformity … a sheet never
   contacts a server"; the other three say nothing, and they are noise at exactly the moment a
   first-time reader is scanning that help for the downscale knob of item 1.
7. **`docs/generation-craft.md` has no clause-level prompt debugging, and that is where three of the
   four runs went.** Two failures, both of English rather than taste, each costing a whole run:
   - **A modifier binds to its nearest preceding noun.** "seated on a low dark plinth with a flat
     cap" was meant as the plinth's flat top; all eight candidates wore a peaked cap. Describe a
     secondary object in its own clause, or its adjectives migrate to the subject.
   - **A uniqueness claim needs a count.** "a single glowing green eye" produced a glowing green
     *third* eye above a normal pair in 5 of 8; "exactly one eye … no other eyes and no eye sockets"
     produced 7 of 8 true cyclopes. The craft doc's "describe form, not mood" rule is right and was
     followed; it is silent on how a uniqueness claim has to be spelled.
8. **The documented tail clause does not produce the reference an image-to-mesh run wants.** Two
   gaps, both measured here:
   - The shipped tail ("plain neutral grey background … no cast shadow") yields a studio photograph
     with a visible floor plane, a soft contact shadow and a background gradient — through three
     runs. "flat even grey backdrop with no ground plane and no horizon line" removed all three at
     once.
   - The largest single silhouette win is not in the doc at all: **ask for the appendages spread
     clear of the body with a gap of background between them.** Runs 1–3 produced wings fused flat
     against the back — the one edge the mask stage has to find and cannot. One clause fixed all
     eight candidates.
   Both belong in the craft doc's tail-clause table, marked measured.
9. **img2img at 0.55 re-decides whatever the source states weakly.** The pose, wing spread, plinth
   and strand mass held perfectly across all four variations; the single-eye constraint came back
   as two eyes on 2 of 4 even with the constraint clauses carried into the variation prompt, and the
   stone warmed from grey-green to pale sandstone on all four. The craft doc's denoise table
   describes what is *kept*; it owes a line on what is silently re-rolled.
10. **A recipe pair meant to share a subject can silently diverge and nothing notices.** The shipped
    `idol-variation.gen.yaml` carried run 1's flat-cap prompt and a comment saying its `images:` path
    was not committed yet. It checks green offline either way — a recipe that cannot run is a
    legitimate placeholder — but after four prompt edits on the portrait recipe the two prompts
    described different statues, and only reading both files catches it.
11. **The sheet's label band is carrying the whole read.** Cells fill row-major with `#0` top-left,
    which is inferable only because every cell is labelled. That is the right design; it is worth
    stating in `docs/generation` that the label, not the position, is the index, because the sheet is
    the one artifact a human and a model read together.

## Wave 2 tool findings

The mesh half, ranked most time lost first. Items 1–3 are one defect and its two aftershocks in the
same code path.

1. **`gen accept` could not accept a TRELLIS.2 mesh at all, and the first accept failed.**
   `accept_mesh` called `ludion_import::import(…, false)` — strict mode, hardcoded — and `SaveGLB`
   always writes `doubleSided: true` on its material, which is `L1202`. Every mesh this pipeline can
   produce hit it, so the arc's whole point was blocked on line one of the accept. **Fixed here:**
   `gen accept` grew `--allow-lossy`, spelled and documented exactly as `ludion import` spells it,
   threaded through `accept()`'s signature; strict stays the default.
2. **`L1202`'s help names an action the reader cannot perform, and keeps naming it after they
   comply.** "Re-export without it … or pass `--allow-lossy`" is addressed to someone holding a
   Blender file. A generator's `.glb` has no export to go back to, and `docs/generation-craft`'s own
   rule forbids editing the workflow for anything a recipe can reach — which a material flag is not.
   Worse, under `--allow-lossy` the accept succeeds and *still* prints the same sentence as a
   warning, advising the flag you just passed. The importer should carry a second help line for the
   generated case, or the accept should rewrite the help it re-prints.
3. **The accept's warnings print after its success lines, so a green accept reads as a failure.**
   `print_accepted` writes the destination and the tree to stdout; the `L1202` warning goes to
   stderr afterwards, so in a terminal the last thing on screen is `warning[L1202]` and the first
   read is "it did not work". Warnings belong before the destination, or the text form needs a
   closing verdict line the way `gen run` has its `next:`.
4. **A mesh run's provenance records `prompt: ""` and nothing about the image it was made from.** An
   image-to-mesh graph has no positive text encoder, so `run::prompt_text` correctly finds nothing —
   and the one field `docs/generation` calls "the one field a human actually reads a year later" is
   empty for precisely the asset class whose origin question has a *hard* answer: the recipe's
   `images:` entry. `origin.inputs` carries `Load Subject: image: image: subject`, which is the
   reference's **name**, not its path. The block should record each resolved `images:` path, and
   ideally its content hash, so an accepted mesh names the accepted image it came from.
5. **A generated mesh is two size-gate offenders, and one of them is not a texture.**
   `meshes/mesh-0.mesh.yaml` is 1.21 MB — 4190 vertices of canonical ryu floats — beside a 1.21 MB
   base-color PNG and a 1.02 MB normal map, all three over decision 20's 1 MiB line at the extent the
   recipe deliberately chose (1024 bakes, 5000-face decimate, already the small settings for an HD-2D
   prop). That is two allowlist entries per accepted mesh, forever, one of them for *text*. Either
   the gate learns a rule for an accepted-asset tree, or a generated mesh's float lists need a
   carrier that is not canonical YAML. Found only at the gate, after the accept — same shape as wave
   1's item 1, now for the mesh half.
6. **`FRONT -Z` on a generated mesh shows the back of its head.** TRELLIS.2 writes the subject facing
   +Z, so the model sheet's `front` cell renders the nape and the `back` cell renders the face —
   the six-view sheet is the one instrument that gates the asset, and it leads with its worst cell.
   Nothing is broken and nothing says so: `gen accept` does not mention orientation and
   `render --model` has no yaw. A `--yaw` on the sheet, or a stated convention in
   `docs/generation-craft` that an image-to-mesh subject arrives 180° from the voxel/sculpt
   convention, pays for itself on the first asset.
7. **`gen check`'s text form garbles a nested image reference.** Under `Load Subject:` it prints
   `image: image: subject` on one line — which reads as a key `image` whose value is the *string*
   `image: subject`, not as the nested map the recipe actually spells and `L1162` actually validates.
   The resolved-override view is the one place a recipe's image binding is confirmed before a
   multi-minute run; it should print the two lines, nested.
8. **`run.ps1 -Detach` prints "ready" and never releases the shell.** Wave 1's item 3 fixed the
   PowerShell *edition* problem; this is the one underneath it. Under a non-interactive harness the
   detached child inherits stdout, so the pipe stays open and the launcher appears hung for its full
   timeout — five minutes here — after the server is already answering `/system_stats`. The detach
   needs its child's stdout redirected away from the parent's handle, not just teed to a log.
9. **Nothing reports progress during a 485 s run.** `gen run` is silent from submit to finish. The
   only way to tell a cold run pulling 5 GB over SMB from a wedged one is to tail the server's own
   `logs/comfyui.err.log` and read tqdm bars through their carriage returns; `gen status <run-dir>`
   must be polled by hand and says "running" the whole time. One line per completed node, off
   `/history` or the websocket the client already speaks, would remove the guesswork — and it is the
   difference between trusting a mesh run and babysitting it.
10. **The sheet cannot show the material the scene will use.** `render --model` synthesizes its
    `Mesh` component with `shape` alone, so `metallic` falls to its `0.0` default and a baked
    metallic map is *inert in the one image that gates the asset*; the importer's own `scene.yaml`
    authors `metallic: 1.0`, where it is not. Judging this asset's material meant hand-writing a
    probe scene beside a copy of the tree. The sheet should take the asset's own scene material, or
    `render --model` should accept the scalars.

## Wave 3 — the multi-view round trip

The demo of `Plan/comfy-generation.md` Wave 3 step 4: sculpt a rough form, render its four orbit
views, condition `Pixal3DMultiViewConditioning` on them, accept the result, and bring it back as a
`shape: mesh` sculpt part blended onto the authored body.

**The subject.** `examples/gallery/sculpts/creature-head-blank.sculpt.yaml` — the head of the gallery
creature, restated in five primitives (cranium, muzzle, jaw, brow, a mirrored tapered horn) with no
carving, no emissive and no noise. A deliberately rougher head rather than a rough copy of the whole
creature, because the plan's claim is about *one part* where bought form should beat authored form:
generation was asked for the skull's surface, and the body it attaches to is the authored creature's,
part for part, so the two documents are an A/B on one animal.
`examples/gallery/sculpts/creature-generated.sculpt.yaml` carries the result; `creature.sculpt.yaml`
and its golden were not touched.

**The committed pair.** `games/vigil/generation/workflows/pixal3d-multiview.json` (41 titled nodes)
and `games/vigil/generation/recipes/creature-head-mesh.gen.yaml`. The graph is four `LoadImage` →
one shared fixed `ImageCrop` rect → BiRefNet `RemoveBackground` → `ImageCompositeMasked` over a black
`EmptyImage` → `Pixal3DMultiViewConditioning` (`fov: 34.16`) → the Pixal3D structure/shape/upsample
chain → `RemeshMesh` 256 → `DecimateMesh` 4 000 → `MeshSmoothNormals` → `SaveGLB`. **It deliberately
stops short of the TRELLIS.2 workflow's unwrap/bake tail:** a `shape: mesh` sculpt part samples the
triangles as a distance field on a 64-cell grid and never reads a texture, so three baked maps would
be GPU time and two size-allowlist entries spent on bytes the consumer discards. The reduction order
of `ludion docs generation-craft-mesh` is unchanged and held: remesh before decimate, 3 984 triangles
against a 4 000 target, 1 997 vertices, no `L1112`.

**The runs**, all at seed 500, under `games/vigil/generation/runs/`:

| # | run id | what changed | wall | result |
|---|---|---|---:|---|
| A | `creature-head-mesh/20260918T115334Z-c3ee2f89` | blank v1; views carried as RGBA (`InvertMask` then `JoinImageWithAlpha`) | 168.1 s (cold) | A tadpole: one bulb and one long spike, 2 188 v / 3 993 t. Normalized box 0.456 / 0.463 / 1.000 against the blank's 0.571 / 0.870 / 1.000 — the vertical collapsed and the horn pair fused. |
| B | `creature-head-mesh/20260918T115950Z-01ba527b` | same blank, views composited onto black instead | 79.4 s | **Identical within 2 vertices** (2 190 v / 3 992 t). Proves the preprocessing was not at fault, and settles alpha-versus-black as a free choice. |
| — | `diag-creature/20260918T120300Z-3b8a80a4` | the **authored 22-part creature's** four views, same graph | 81.1 s | The control. A recognisable quadruped: four legs, a tail, a horned head, normalized box 0.308 / 0.578 / 1.000 against the authored 0.269 / 0.695 / 1.000. Lumpy and softened, but unmistakably the animal. **The pipeline works; the subject was the problem.** |
| C | `creature-head-mesh/20260918T120618Z-01ba527b` | blank v2 — horns raised steeply and thinned, muzzle tapered, a brow ridge added, every feature made to clear its neighbours **in profile** | 131.3 s | **Accepted.** Two clean upright horns and the brow recovered; the muzzle still lost and the brow stretched to twice the skull's width. 1 997 v / 3 984 t. |
| D | `creature-head-mesh/20260918T120942Z-e1a9c767` | `Structure Decode` at `resolution: "64"` | **1 013.5 s** | A shattered open shell of loose sheets — 2 361 v / 3 289 t, not a solid. Twelve times the cost for a worse mesh; the override was removed from the recipe again. |

A sixth submission, a throwaway two-view image recipe, cost 5.4 s and confirmed the crop and the mask
before any mesh run — it is the reason B and C were not also spent debugging preprocessing.

**The fit.** The accepted mesh's own box is 0.982 / 0.946 / 0.557, fitted on height at `size: 0.72`,
`at: [0, 1.25, -0.86]`, `blend: 0.07`, `bone: head`. The blend is the part that simply worked: no
seam, no fold artifact, the `shape: mesh` field folding into the authored neck exactly as a capsule
would. `check-scene` is green at 14 parts / 9 bones, 8 000 triangles after decimation from 29 808,
596 ms. The sheet for the sitting is `Plan/artifacts/comfy-generation-creature-mesh-sheet.png`.

**Accepted asset:** `examples/gallery/meshes/creature-head-generated/` — `scene.yaml` plus one
391 KB `meshes/mesh-0.mesh.yaml` carrying its `origin:` block (recipe, run id, workflow hash, seed,
and the four `images:` entries with their content hashes). `--allow-lossy` as always, for `SaveGLB`'s
`doubleSided`. **No size-allowlist entry was needed** — the first accepted mesh of this arc to clear
the 1 MiB line unaided, because an untextured tail duplicates no vertices at UV seams.

## Wave 3 tool findings

Ranked most time lost first.

1. **`render --model` *is* the conditioning rig, and nothing says so or helps you use it as one.**
   The good news is in the source: every view shares one camera distance and one target, sits at
   elevation 0 on a horizontal axis at `fov_y` 45°, so the four cells are a true orbit at one scale —
   precisely what `Pixal3DMultiViewConditioning` assumes. Everything else fights it. There is no
   `--yaw`, so four images means four `--views <single>` invocations; each is a one-cell sheet under
   a yellow label banner, over a ground plane, against a sky, with the subject fitted to its
   *bounding sphere* so it fills about two thirds of the frame. Undoing that took four extra node
   types in the graph plus a crop rect re-derived per subject from a probe render and paired **by
   hand** with a `fov` nobody can verify. Fix: `render --model --rig` — N views, square, black, no
   label, no ground, subject framed to extent divided by 1.1, and the effective horizontal fov
   printed so it can be pasted straight into the recipe.
2. **The crop rect and the conditioning `fov` are one fact split across two inputs, with no check.**
   `fov = 2·atan(tan(fov_y/2) · rect / width)`. Get it wrong and nothing errors: the reconstruction
   just distorts, and a distorted reconstruction is indistinguishable from a bad subject until a
   control experiment separates them — which is exactly what the diagnostic run had to be spent on.
   The `--rig` mode above removes the pair; failing that, the two belong in the recipe as
   neighbouring overrides with the formula documented between them.
3. **`Structure Decode` at `resolution: "64"` costs twelve times as much and shatters.** 1 013.5 s
   against 131.3 s on the same graph and subject — the upsample sampler goes from about 1 s to
   **about 85 s per step**, so a 12-step stage alone is 17 minutes — and what comes out is an open
   shell of loose triangle sheets, strictly worse than resolution 32. Not a Ludion defect: a measured
   trap, now a rule in `ludion docs generation-craft-mesh`, and the reason the committed recipe
   carries no such override.
4. **`--set` with a JSON Pointer is unusable from Git Bash, and `L4008` blames the user.**
   `--set /parts/6/size=0.72` arrives as `--set C:/Program Files/Git/parts/6/size=0.72`: MSYS path
   conversion rewrites the leading slash. The diagnostic says "the pointer … does not start with
   `/`", which is true and useless — the pointer it prints is the mangled one, and the help then
   offers another leading-slash example that will mangle identically. Every `--set`/`--sweep` on a
   model is affected, and the repo's default shell is the one that breaks it. Fix: detect a pointer
   that has acquired a drive-letter prefix and name the shell in the help.
5. **A `shape: mesh` part has a scalar `size` and no way to correct a generator's proportions.** The
   accepted head's brow ridge came back at twice the skull's width (normalized x 1.04 against the
   blank's 0.53), so fitting on height leaves two rods sticking out of the head and fitting on width
   buries the horns. Every other part shape carries per-axis control — `radii`, `from`/`to`,
   `height` — and this one carries a single number, on exactly the shape whose source is a machine
   that gets proportions wrong. A three-component `size`, or honouring `deform: taper` on it, would
   have absorbed the whole defect.
6. **`check-scene --bounds` refuses a `kind: mesh` document, so fitting a `shape: mesh` part has no
   numbers.** Placing the part needs the mesh's own box; `check-scene <m>.mesh.yaml --bounds`
   answers "`--bounds` … apply only to sculpt documents". Both boxes in this run were read with a
   throwaway Node script over the YAML float lists. `--bounds` on a mesh document is the same
   question with a simpler answer, and it is the document whose extents a `size`/`at` fit is computed
   from.
7. **`gen accept` cannot re-accept into a directory it already wrote.** `L1172` refuses a non-empty
   `--out`, which is right the first time and wrong for the loop this arc is: generate, look,
   regenerate, look. Four accepts here were preceded by a hand-typed recursive delete. Either a
   `--force`, or the refusal's help should name the one command that clears the way.
8. **Nothing previews the conditioning input.** The four cropped, masked views *are* the brief for a
   mesh run, and the only way to see them was to hand-build a throwaway image recipe whose graph
   duplicated the mesh graph's first four stages, run it, and delete it. That cost 5.4 s against the
   250 s of GPU runs A and B spent proving the preprocessing innocent. A `gen run --stop-after <node
   title>`, or a documented convention of a preview recipe committed beside every mesh recipe, would
   make it the first step instead of the fourth.
9. **`gen check`'s override echo drops a COMBO value's quotes.** `resolution: "64"` prints as
   `resolution: 64` — the exact integer-versus-string distinction the docs warn about for `INT`
   inputs. The spelling was preserved and the run was correct, but the one line that exists to let
   you verify a resolved override is the line that hides the difference.
10. **The `docs/` mirror gate makes the whole CLI unbuildable while another instance edits a topic.**
    `cargo build -p ludion-cli` panics on any drift between `docs/` and `embedded-docs/`, so a second
    instance's half-saved edit blocks every `ludion` invocation in a shared checkout — four times in
    this run, including once for a file this task was told not to touch. The gate is right in CI;
    locally it could warn and embed the repo copy rather than refuse to build.
