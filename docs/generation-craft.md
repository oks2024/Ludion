---
description: Prompts, graph settings and seed budgets that make a text-to-image generation run usable
---

# Generation craft

**Scope:** what a *valid* recipe still needs to produce something worth accepting — the graph
settings that matter, the prompt shape for a subject meant to become a mesh, and how many seeds to
spend where. The mesh reduction order and the textured bake graph are
`ludion docs generation-craft-mesh`. The document format and the loop are `ludion docs generation`;
the verbs, the run folder and `gen accept` are `ludion docs generation-runs`; the authored-form
alternative this loop composes with is `ludion docs sculpt-craft`.

Every number below is marked **measured** (read off a run on the reference box — a 24 GB card) or
**convention** (a starting point to be confirmed or overturned by the next sitting). Nothing here is
a guess dressed as a rule.

## The text-to-image graph

The committed `txt2img.json` is FLUX.2 [klein] 4B, and its node titles are the override surface a
recipe writes to:

| title | class | what a recipe changes |
|---|---|---|
| `Positive Prompt` | `CLIPTextEncode` | `text` — the whole subject. |
| `Empty Latent` | `EmptyFlux2LatentImage` | `width`, `height`, `batch_size`. |
| `Scheduler` | `Flux2Scheduler` | `steps`, **and `width`/`height` again**. |
| `Sampler Select` | `KSamplerSelect` | `sampler_name`. |
| `CFG Guider` | `CFGGuider` | `cfg`. |
| `Noise` | `RandomNoise` | `noise_seed` — filled from the recipe's `seed` automatically. |
| `Model`, `Text Encoder`, `VAE` | loaders | the checkpoint filenames. Leave them alone: they are what provenance records. |

**Size lives in two nodes, and both must agree** (measured — it is why the committed idol recipe
overrides `Empty Latent` *and* `Scheduler` with the same 1024/1024). The latent node sizes the
tensor; the scheduler's `width`/`height` shift the model's sigma schedule for the resolution. Set one
and not the other and the image degrades quietly rather than erroring.

**Steps 4 at cfg 1 is the model's distilled operating point** (measured: 1024² in 4.2 s warm, 18.3 GB
peak; first load 76 s). This is a distilled model — the negative prompt is zeroed through
`Conditioning Zero Out` and cfg is 1 by construction. Raising cfg does not sharpen it; it burns it.
More steps buy very little: 4 is the point, not a floor.

**A cold model costs about 76 s** (measured). Batch your iterations: one `--count 8` pays that once,
eight separate `--count 1` runs pay it once if nothing else evicts the model in between and eight
times if something does.

## Prompting a subject that will become a mesh

Every mesh path is image-to-mesh, so the image is the real artifact. An image meant to feed a mesh
run is not a picture — it is a **reference photograph of one object**. The pattern, converged over
four idol runs (§ below) and verbatim from the accepted `idol-portrait.gen.yaml`'s tail:

> `<subject described in one clause, with its distinguishing features>. Single object, centered, the
> whole subject inside the frame with clear space around it, flat even grey backdrop with no ground
> plane and no horizon line, soft frontal lighting, no cast shadow, no text.`

Each tail clause earns its place (**convention**, from the constraints the mesh stage imposes, except
where marked **measured**):

- **Single object, centered, the whole subject inside the frame** — a second object in frame becomes
  a second component in the mesh, or is fused into the first; a cropped one loses geometry the mesh
  stage never gets back.
- **Flat even grey backdrop with no ground plane and no horizon line** — the background-mask stage has
  to find the silhouette. **Measured:** the earlier tail ("plain neutral grey background … no cast
  shadow") yielded a studio photograph with a visible floor plane, a soft contact shadow and a
  background gradient, through three runs; this wording removed all three at once.
- **Soft frontal lighting, no cast shadow** — a hard shadow on the ground plane reads as geometry, and
  a strong key bakes its own highlight into the base-color map.
- **No text** — lettering survives into the texture and is unreadable at game size.
- **Appendages spread clear of the body, with a gap of background visible between each and the
  body** — for any subject with limbs, wings or other protrusions. **Measured: the single largest
  silhouette win found.** Without it, wings fused flat against the back on 5 of 8 candidates — the one
  edge the mask stage cannot find; this clause fixed all eight.

**Describe form, not mood.** "A crouching winged creature with one glowing green eye, on a low dark
plinth" is a shape the mesh stage can resolve; "ominous", "ancient", "foreboding" are not. Style
words belong in a style clause after the form clause, never instead of it.

**Name the material once.** "Weathered stone" changes the surface everywhere; a per-part material
description does not survive into the geometry.

## Debugging a prompt clause by clause

Two failures cost a whole run each on the idol reference, both of English rather than taste
(**measured**):

- **A modifier binds to its nearest preceding noun.** "seated on a low dark plinth with a flat cap"
  was meant as the plinth's flat top; every candidate instead wore a peaked cap. Describe a secondary
  object in its own clause, or its adjectives migrate onto the subject.
- **A uniqueness claim needs a count.** "a single glowing green eye" produced a glowing green *third*
  eye above a normal pair on 5 of 8 candidates; "exactly one eye … no other eyes and no eye sockets"
  produced 7 of 8 true cyclopes. "Describe form, not mood" above is still right; it is silent on how a
  uniqueness claim has to be spelled — state the count and deny the alternative.

## Seed budget: iterate before you spend

**`--count 8` on the image is cheap; one mesh run is not** (measured: text-to-image 4.2 s per
candidate against ≈150 s for the full mesh chain and ≈82 s for the lighter one — a mesh run is 20–35
image candidates). The loop that respects that:

1. `gen run <recipe> --count 8` — eight seeds, one sheet.
2. Read the sheet. If the *subject* is wrong, change the prompt and run eight more. If the subject is
   right and only one of eight is clean, the prompt is doing its job; pick that one.
3. Only then chain the pick into a mesh run with `gen run <mesh-recipe> --image <run>/0003.png`,
   which uses the scratch candidate without accepting it.

**Do not accept an image just to pass it along.** `--image` takes a path inside a run folder. Accept
only what you want to keep — a reference you will come back to, or an asset the game ships.

**Convention, from the idol run's five submissions:** bump the base seed per run (7 → 100 → 200 →
300) rather than reusing one. A seed is not stable across a prompt edit, so the label is not a
comparison across runs — it is the only mark on the image that identifies which run produced it.

## Image-to-image: variation without redrawing

The committed `img2img.json` is the same model, with the input image encoded through `VAE Encode` and
the schedule split by the `Denoise` node (`SplitSigmasDenoise`). One number controls everything:

| `denoise` | effect (**convention**, from the smoke runs) |
|---|---|
| 0.2–0.4 | Re-render: same composition, same pose, cleaner surface. |
| 0.5–0.6 | Variation: recognisably the same subject, freely re-lit and re-detailed. The committed recipes sit at 0.55 and 0.6. |
| 0.7+ | A new image loosely inspired by the old one. Past this, use text-to-image. |

Measured: the 8-step schedule at denoise 0.6 costs 6.2 s warm at 18.5 GB.

**At 0.55, whatever the source image states weakly gets re-decided** (measured, four idol
variations): pose, wing spread, plinth and strand mass held on 4 of 4 — strongly stated in every
carried clause. The single-eye constraint, carried into the variation prompt too, still came back as
two eyes on 2 of 4, and the stone colour warmed from grey-green to pale sandstone on all 4. Rule: at
denoise ≥ 0.5, restate every count and name the material colour explicitly — carrying the clause
forward is not enough on its own.

The subject prompt stays; change only the clause you want changed — "cast in weathered green bronze
with verdigris patina" over the same form description gives a material study of one accepted
silhouette.

## Rules

- **A generation run and `node scripts/check.mjs` never overlap on a GPU that blesses goldens.** A
  run is multi-minute and takes the whole card; contention there is the known cause of device-lost
  failures in the test lanes. Serialize by hand.
- **Iterate the recipe, never the workflow, for anything a title can reach.** A workflow edit
  invalidates every run's `workflow_hash` and has to go back through the UI or a hand-written graph;
  a recipe edit is a diff.
- **One prompt per recipe.** Two subjects in one recipe means you cannot tell which seed range
  belongs to which; make a second recipe.
- **A recipe pair sharing a subject shares its prompt by hand.** Nothing checks that a variation
  recipe's prompt still describes the portrait recipe's subject — after editing one, diff the other.

## Gotchas

- **The first candidate of a cold run is not slower than the rest — the whole run is.** Model load is
  charged once, before candidate 0, so a `--count 8` run's wall time is load plus 8 × per-candidate,
  and `run.json`'s `duration_ms` includes the load.
- **A prompt change that reads small can move the subject entirely.** Seeds are not stable across
  prompts: the same `seed: 7` under an edited prompt is a different image, not the old one adjusted.
- **An `INT` input refuses `1024.0`.** Width, height, steps and seeds keep their integer spelling in
  the recipe (`L1165` from the server's schema, or `L1162` locally).

## Next

- `ludion docs generation` — the recipe document, the workflow file, the loop.
- `ludion docs generation-runs` — the verbs, the run folder, provenance and `gen accept`.
- `ludion docs generation-craft-mesh` — the reduction order, the textured bake graph, and the
  four-view loop that conditions a run on an authored sculpt instead of a prompt.
- `ludion docs sculpt-craft` — the authoring path a generated form composes with.
- `ludion docs materials` — what an accepted mesh's maps mean to the renderer.
