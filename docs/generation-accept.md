---
description: What gen accept freezes: image or mesh, the origin provenance block and where it is carried
---

# Accepting a generation candidate

**Scope:** what `ludion gen accept` writes — a re-encoded image or a mesh import tree — and the `origin` provenance block it records. The verbs, the run folder and `run.json` are `ludion docs generation-runs`; the recipe is `ludion docs generation`.

## What an accept writes

An accepted **image** is re-encoded, not copied: decoded and written back through the engine's one
deterministic PNG path, which writes only `IHDR`/`IDAT`/`IEND`, so the file carries no server-written
ancillary chunk and hashes identically to the same pixels arriving through `ludion import`.

An accepted **mesh** goes through the same `ludion-import` library, called in-process in the mode the
caller asks for: strict by default, `--allow-lossy` for the subset that maps, exactly as
`ludion import` reads a `.glb` from disk. **A mesh generator's `SaveGLB` node writes
`doubleSided: true` on every material, which strict mode refuses with `L1202`** — so an accepted
generated mesh is in practice an `--allow-lossy` accept, and the flag's warning says what was left
out: back-face culling, which is what a closed remeshed surface wants. Strict stays the default
because a candidate that needs the flag should say so once, out loud, rather than silently losing
something on every accept. `--out` names a
**directory** instead of a file: it must be absent or already empty, else `L1172`. The import writes
`model.prefab.yaml` + `meshes/*.mesh.yaml` + `textures/<content-hash>.png` exactly as `ludion import` would,
and the importer's own diagnostics pass through unchanged — `L1112` (over the vertex budget) among
them, whose meaning here is "lower the generation graph's `Decimate` target," never
`budget_exempt: true`: density is fixed in the graph, never on the Ludion side.

Provenance has one shape and **two carriers**: a `kind: origin` sidecar named
`<asset>.png.origin.yaml` beside an accepted image (a PNG has no text place for it, and a `tEXt`
chunk is exactly what the importer strips), and an `origin:` map spliced into every produced
`.mesh.yaml`, right after `name` — at the top where a diff shows it, before the bulk float lists.
Every mesh document without one stays byte-identical, and the texture-reference keys
(`base_color_texture`, `metallic_roughness_texture`, `normal_texture`, `occlusion_texture`,
`emissive_texture`) coexist with `origin:` and round-trip byte-stably. A mesh needs no sidecar of its
own.

`recipe` and `workflow` are **project-relative** — relative to the project root, the folder holding
`generation/` — so the block survives the project moving:

```yaml
origin:
  tool: comfyui
  comfy_version: "0.3.x"
  recipe: generation/recipes/idol-portrait.gen.yaml
  recipe_hash: "..."
  run_id: "20260918T113000Z-1a2b3c4d"
  date: 2026-09-18
  seed: 10    # omitted when the run could not know it
  workflow: generation/workflows/txt2img.json
  workflow_hash: "..."
  checkpoints:
    - flux-2-klein-4b.safetensors
  images:
    - name: subject
      path: generation/references/idol-reference.png
      hash: f312fd2a2ea2fa9c
  prompt: "A small weathered stone idol, ..."
  inputs:
    Empty Latent:
      height: 1024
      width: 1024
```

Provenance is built from `run.json` alone: the recipe is **not** read at accept time, so editing,
renaming or deleting it afterwards changes nothing an accept of that run records. `date` is the run's
date (`started`), not the day of the accept. `recipe_hash` is the recipe file's content hash as the
run read it; it is omitted from blocks accepted before it existed. A `run.json` written before the
`prompt`/`inputs` snapshot re-reads its recipe, but only while the recipe's and workflow's hashes
still equal the report's; otherwise the accept is `L1172` naming both hashes — restore the file, or
run the recipe again and accept from the new run. A run older than the `images` record has it
rebuilt from the copies in the run folder's `inputs/`.

`prompt` is the text of the encoders whose output reaches a `positive` input, following links
upstream (through `FluxGuidance`, `ConditioningCombine`, `ControlNetApplyAdvanced`; never through an
input named `negative`), joined in node order — the one field a human actually reads a year later. An
encoder is a node whose class contains `TextEncode`; its text is the string inputs `text`, `prompt`,
`text_g`, `text_l`, trimmed, a value repeated within one node taken once. A graph with no `positive`
input (Flux's `BasicGuider`) counts every encoder. A prompt fed through a link stays unrecorded.
`checkpoints` is every `*_name` string input, on any node, whose value ends in a model extension
(`.safetensors .ckpt .pt .pth .bin .gguf`). `inputs` is the recipe's resolved overrides,
snapshotted.

`images` is what the asset was made **from**: one entry per image the run uploaded, with the recipe's
own name for it, its project-relative path, and a content hash of the uploaded bytes — so an edited
source image is as detectable as an edited workflow. An `inputs` snapshot only records *that* a node
was fed `images.subject`; this records which file that was. The key is written **only when the run
uploaded something**, so every text-to-image asset's provenance stays exactly as it was. A `--image`
scratch override is recorded by file name alone when it sits outside the project: a provenance block
gets committed and pasted, and never carries an absolute machine path.

A mesh accept's object adds `meshes` (each `{path, vertices, triangles}`, in glTF document order)
and `textures` (each written path) to an image accept's four keys; they are never present on an
image accept or a failure.

The contact sheet composes images only: `gen sheet` on a mesh run is `L1171`, pointed at the
model stage — look at an accepted mesh with `render --model`.

## Next

- `ludion docs generation-runs` — the verbs, run folder, diagnostics.
- `ludion docs materials` — what an accepted mesh's maps mean to the renderer.
