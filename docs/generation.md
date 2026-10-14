---
description: The kind: generation recipe document, its workflow file, the server it drives, and the loop
---

# Generation

**Scope:** the `kind: generation` recipe document that drives an already-running ComfyUI server — its
format, the workflow file it names, the server address, and the `ludion gen` loop that turns a recipe
into candidates. Running a recipe, the scratch run folder it produces and what `gen accept` freezes
(an image or a mesh) are `ludion docs generation-runs`. Which prompts and graph settings produce
usable candidates is `ludion docs generation-craft`; the strict-YAML subset every recipe obeys is
`ludion docs scene-format`; what an accepted mesh's maps do is `ludion docs materials`.

## What a recipe is, and is not

**A recipe and a run are never engine inputs.** Nothing in a scene, a component or an asset path
points at a recipe, a run folder or a candidate index. The only engine-side fact about these
documents is that `generation` and `origin` are registered kinds, so the content-root walk skips them
instead of reporting `L1119`.

**Acceptance is the one bridge.** `gen accept` freezes one candidate into an ordinary asset — a
canonical `.png`, or a `.mesh.yaml` through `ludion import` — carrying a provenance block. From that
moment it is content: hand-editable, golden-tested, indistinguishable to the loader from an authored
asset. **Regeneration is never a gate:** the same graph, seed and checkpoint diverge across driver
and library versions, so an accepted asset carries no `generated_from:` and the drift rules
`generate-map` / `generate-atlas` enforce do not apply to it.

## The loop

Run from the game's project directory:

```
ludion gen check  generation/recipes/portrait.gen.yaml
ludion gen run    generation/recipes/portrait.gen.yaml --count 8
ludion gen sheet  generation/runs/portrait/<run-id>
ludion gen accept generation/runs/portrait/<run-id> 3 \
                  --out scenes/assets/portrait-reference.png
```

`check` first, always: it answers offline (document, paths, workflow format, override titles, output
kind), then against the server's `/object_info` — so a bad graph is a local diagnostic with a JSON
pointer instead of a 400 from a machine minutes later. Read the sheet, edit the recipe, run again.
Accept once. A green offline check with no server says nothing about whether the classes the graph
names are installed; the text form prints both halves — `offline: ok`, then `live: ok`,
`live: failed`, or `live: skipped (L1167)` when nothing answers or the address is malformed.

`gen run` ends its text output with a `next:` line naming the literal follow-up command —
`ludion gen sheet <run-dir>`, `gen fetch <run-dir>` after `--no-wait`, `gen accept <run-dir> <i>`
for a mesh run, or `gen run <recipe>` after a failed run. Every path a verb prints uses forward
slashes on every OS.

### The editor's seat at the loop

The editor's **Generation** view is the human's seat at exactly this loop, and it runs exactly these
verbs: the host spawns `ludion gen <verb> … --json` as a subprocess and reads the one object it
prints, on failure too, so the view has no capability a shell does not and nothing in the editor
ever speaks to the server itself. It lists `generation/recipes/` on the left; the centre is the
recipe's own values — one control per number, a box for a `seed` (a pick, not a tunable), a
full-width field per prompt — each edit splicing that one token in the file and leaving every other
byte alone, with a `gen check` status line above it reporting `offline`, `live`, the output kind and
every `L####` with its pointer. **Generate** runs the recipe as committed: no count or seed override
rides in from the UI, because the file is what makes a run reproducible. Under it, the candidate
grid of the selected run (newest by default, older ones in a picker) reads the run folder straight
off the disk, each cell labelled `#<index> seed <n>` (`seed ?` when the run could not know it, as on
the sheet), with the run's failed or lost jobs listed under it; a mesh run shows one cell and its
file size instead, since there is nothing to compose. A run that timed out, came back partial or
failed stays selected with the verb's diagnostics beside its grid. **Status** on a `submitted` run
says which jobs are still queued, and **Fetch** completes it once every job has a verdict;
**Generate** takes a session-only timeout field of whole seconds up to a week; an invalid entry
disables Generate. **Accept** asks for a project-relative destination — a file for an image, a
directory for a mesh, with the `--allow-lossy` box a mesh candidate in practice needs — and shows
the refusal verbatim when there is one. The server address is a field for **this session only**: it
is never written to a recipe or remembered, and leaving it empty is the ordinary case, where the
subprocess inherits `LUDION_COMFY_URL`.

## The server address

`--url` beats `LUDION_COMFY_URL` beats ComfyUI's own default, the loopback address on port 8188. The
address is a per-machine property exactly like `LUDION_THREADS`, so it lives in the environment,
never in a committed file — and **no artifact this pipeline writes records it**, not `run.json`, not
a provenance block; those record the ComfyUI version and the checkpoint filenames instead. Nothing
answering is `L1167`, which names all three sources and which one supplied the address. Plain
`http://` only: the client is built without TLS, so an `https://` address is `L1167` before
anything is tried.

## The recipe document

Canonical key order — what the writer emits and the order to author in: `format`, `kind`, `name`,
`workflow`, `seed`, `count`, `images`, `inputs`. Maps are sorted by key on emit.

```yaml
format: 1
kind: generation
name: idol-portrait
workflow: ../workflows/txt2img.json
seed: 7
count: 8
inputs:
  Empty Latent:
    height: 1024
    width: 1024
  Positive Prompt:
    text: "A small weathered stone idol, a crouching winged creature with one glowing green eye, on a low dark plinth. Single object, centered, plain neutral grey background, soft even lighting, no cast shadow, no text."
  Scheduler:
    height: 1024
    width: 1024
```

| key | required | meaning |
|---|---|---|
| `format` | yes | `1`, as in every Ludion document. |
| `kind` | yes | `generation`. |
| `name` | no | Defaults to the file stem. Names the run folder; must be a plain file stem (`L1162`). |
| `workflow` | yes | Relative path to the API-format graph, resolved against **this document's own directory** — so a recipe folder moves as a unit. |
| `seed` | no | Base seed, integer `>= 0`, default `0`. |
| `count` | no | Candidates per run, `>= 1`, default `1`. |
| `images` | no | Name → relative path to a **committed** image, uploaded before submit. |
| `inputs` | no | Node title → input name → value. |

**Values are leaf inputs, never edges.** A scalar (string, integer, number, boolean) is passed
verbatim into the node's `inputs`, and **an integer keeps its integer spelling** — a ComfyUI `INT`
input refuses `1024.0`. The one structured form is a reference into the recipe's own `images` map;
any other map, or a list, is `L1162`, because a graph edge is the workflow's business.

**Block style throughout.** The strict YAML subset refuses flow collections (`L1005`) and block
scalars (`L1006`), so an image reference is a nested map and a long prompt is one double-quoted
string on one line:

```yaml
images:
  subject: ../references/idol-reference.png
inputs:
  Load Subject:
    image:
      image: subject
```

**Every `images:` entry must be bound** by an `image:` override: an unbound one would run the
workflow's baked-in image, so `gen check` and `gen run` refuse it as `L1217` at `/images/<name>`.
`run.json` records per image which node and input bound it (`bound_to`).

**Seeds.** The base `seed` is written into every input named `seed` or `noise_seed` that the recipe
does not override explicitly; an explicit override wins and is the seed recorded. Candidate `i` of a
`--count N` run uses `seed + i`, so a candidate's index and its seed are the same fact — what the
sheet labels and what provenance records. A seed fed by a link or a primitive node, or pins that
disagree, records `null` (`?` on the sheet and in the CLI text; the origin omits the key). When
`count` is above 1 and no seed input varies, every job submits the same graph and `gen check` and
`gen run` warn `L1266`.

The one-to-one rule has a caveat: one job that writes several files (`batch_size` above 1, two
savers) gives several candidates at one seed, told apart by `batch_index` in `run.json`. A
`PreviewImage` writes only a temporary file; a run never downloads it, so it is not a candidate.

## The workflow file

A workflow is committed **API-format JSON, verbatim**, and Ludion never hand-edits it. Two ways to
get one: in the ComfyUI UI, build the graph and use **File → Export (API)** — a plain UI/litegraph
export is `L1163`, which names that menu item, the likeliest first-run mistake; or, without a
browser, write one against `ludion gen nodes [filter] [--class C]`, a typed listing from the live
server's `/object_info` (class, category, inputs with types, defaults and enum options, outputs).

**Overrides key on `_meta.title`, not node id.** A title survives a re-export from the UI, a numeric
id need not, and a title is what a human typed to say what the node is *for*. Give every node a
recipe reaches a stable, distinct title; an absent or duplicated one is `L1166`, listing every title
in the graph.

**The output kind is read, never declared.** A recipe carries no `produces:` key: `SaveImage` and
friends mean images, `SaveGLB` means a mesh, and `gen check` reports which. No output node, or both
kinds in one graph, is `L1171`.

## Rules

- **A generation run and `node scripts/check.mjs` never overlap on a GPU that blesses goldens.** A
  run is a multi-minute, full-VRAM job, and GPU contention on that box is the known cause of
  device-lost failures in the test lanes. The two are serialized by hand, by whoever starts either
  one — no scheduler, no lock file.
- **Commit the recipe and the workflow; never a run.** `generation/recipes/*.gen.yaml` and
  `generation/workflows/*.json` are source; `generation/runs/` is ignored.
- **Never write the server address into a file** — not a recipe key, not a prompt, not a comment.

## Next

- `ludion docs generation-runs` — the verbs, the run folder, and what `gen accept` freezes.
- `ludion docs generation-craft` — prompts, sizes, seeds and the image-to-image loop.
- `ludion docs project-anatomy` — where `generation/` sits in a project.
- `ludion docs materials` — what an accepted mesh's maps do.
