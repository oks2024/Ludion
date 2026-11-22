---
description: Building a voxel model from a committed script: grid.mjs, emit/from-json, and the drift gate
---

# Voxel generation

**Scope:** the ladder's third rung — a model whose source of truth is a committed Node script rather
than its own text. Take it only when there is no low-resolution ancestor to grow: the first two rungs,
and every verb that edits a model file in place, are `ludion docs voxel-ops`. The document a script
ends up writing is `ludion docs voxel-format`; checking the result is `ludion docs voxel-authoring`.

## The loop

A generator builds the grid over the shared library and writes **one JSON object**; the engine
validates it and writes the YAML.

**The generator writes the JSON; `from-json` reads it back.** End the script with
`emit({ ..., out: process.argv[2] })` and run the two steps:

```
node voxels/lantern.voxel.gen.mjs voxels/lantern.voxel.json
ludion voxel from-json --in voxels/lantern.voxel.json --validate
ludion voxel from-json --in voxels/lantern.voxel.json --out voxels/lantern.voxel.yaml
```

`--validate` swaps in for `--out`: same checks (`L1115`/`L1129`, an exposure note), nothing written —
the fast half of the loop. The script sits beside the model it owns as `<name>.voxel.gen.mjs`, the
way a generated scene sits beside its `<name>.map.yaml` source; the `.voxel.json` between them is a
build artifact, not something to commit.

**Never move the intermediate through a shell.** PowerShell re-encodes a process's stdout, so both
`node gen.mjs | ludion voxel from-json` and `node gen.mjs > model.json` can turn every non-ASCII
character into mojibake — an em-dash arriving as `â€”` and then committed, which has happened here
once. `from-json` warns with `L1135` on the signature; `out` is the fix, because the bytes never
reach a shell. Take the path as an argument rather than hard-coding it: `voxel-drift` spawns the
generator bare and reads its stdout, which is safe (no shell) and must keep working, so
`process.argv[2]` leaves one script serving both callers with no branch.

**The library.** `scripts/voxel/grid.mjs` is plain ESM with no dependencies — a `Grid` carrying the
ops file's shape primitives, `mirrorX`, a seeded `speckle`, `lathe` (interpolated ellipse
cross-sections, the organic primitive a creature wants), `wedge` (its architectural counterpart, the
sloped extrusion a roof or ramp is), `repeat` (an arithmetic march, for the quoin/corbel/tread
rhythm that is otherwise a hand-indexed loop) and `emit()`.
`scripts/voxel/README.md` is the reference; `examples/voxel-gen/brazier.voxel.gen.mjs` is the
runnable worked example, its committed output the drift gate's standing proof. Both live in the
**engine checkout** (the library is build tooling, not a shipped crate), so a script's `import` is
relative to its own location: inside the engine checkout, count the `../`; in a scaffolded project, go through the engine link — from `scenes/voxels/x.mjs`, `import { Grid } from '../../.ludion/engine/scripts/voxel/grid.mjs';`. Call **`assertConnected(grid)`**
right before `emit()`: it runs the same 6-connectivity check as `L1129` and throws the same per-piece
report, so a disconnected grid fails the generator instead of round-tripping through `--validate` to
find out.

## Format

A generator ends with
`emit({ grid, name, scale, generatedFrom, palette, header, layerComments, out })` — every field
optional but `grid` — which writes the snake_case JSON intermediate to `out`, or prints it to stdout
when `out` is absent.
`ludion explain L1133` prints its shape when you need the contract (`format`, `kind: voxel-gen`,
`size`, `palette` and `layers` required, the rest optional). What the shape alone won't tell you:
`layers[y][z]` is one row string per `z` within layer `y` — the same orientation as the YAML
`slices`, so the grid passes straight through — and `layer_comments` keys are layer indices as
decimal strings, JSON having no integer keys. `L1133` covers the intermediate's own shape only: once
it parses, the described model runs the ordinary checks, so a short row is the usual `L1114` and an
oversized grid the usual `L1115`.

Unlike `ludion voxel new` and `upscale`, `from-json` **overwrites** its `--out` — regenerating is the
point.

## Rules

**Scripts never write YAML.** Not a style preference: a hand-rolled emitter is a second source of
truth for the canonical form — key order, ryu float formatting, comment conventions — and it drifts
from `emit_voxel` the first time the format grows a key. `ludion voxel from-json` is the only
sanctioned path from a script to a model file. One legacy exception survives and is **not** prior
art: a legacy hand-rolled emitter in the private reference game stays authoritative for `house-a`/`house-b`, which
carry no `generated_from:` and sit outside the drift gate. Read it for the *models*, never the
*method* — a third house goes through `grid.mjs` + `from-json`, the way `house-c` did.

**`Math.random` is banned** in the library and in generator scripts — the output is byte-gated, so
scattered detail uses `speckle(..., seed)`, which runs its own PRNG.

### `generated_from:` and the drift gate

A model whose *source of truth is its script* declares `generated_from: <name>.voxel.gen.mjs`
(`emit()` writes it from the `generatedFrom` field). That key does two things:

- **It marks the file write-only.** Never hand-edit a model that carries it — edit the script and
  regenerate, the same rule a `generated_from:` scene follows. `voxel apply` refuses such a file
  (`L1132`); `--dry-run` still previews.
- **It enrols the model in the `voxel-drift` gate** (`scripts/check.mjs`, and CI). The gate enumerates
  every `*.voxel.gen.mjs` under `games/` and `examples/`, re-runs it, and byte-compares its output with
  the `<name>.voxel.yaml` beside it through `ludion voxel from-json --check <model>`, which writes
  nothing. It fails a script with no model, a model naming a missing or non-`.voxel.gen.mjs` script,
  and a copy away from its script; a script that emits no `generatedFrom` is a scaffold and is
  skipped. Drift fails the commit.

```
node scripts/voxel-drift.mjs                                 # the gate, standalone
node voxels/lantern.voxel.gen.mjs voxels/lantern.voxel.json
ludion voxel from-json --in voxels/lantern.voxel.json \
  --check voxels/lantern.voxel.yaml                         # one model, byte-compared
```

**Not every generator wants the key.** A script that was one-shot scaffolding for hand-sculpting
afterwards stays in `.ludion/scratch/` and omits `generatedFrom`; the *file* is authoritative from
then on. Presence of the key is the whole marker.

## Gotchas

- **`L1135` — the intermediate was encoded twice.** A warning; the model is still written, with the
  mojibake in it. Regenerate through `emit({ out })` + `--in`, above.
- **A generated model over the cell budget still warns** (`L1115`, reported by `--validate` like any
  other check): the opt-out is a `budget_exempt: true` in the emitted document, and it is an author's
  call, not the script's convenience — `ludion docs voxel-ops` has the rule.

## Next

- `ludion docs voxel-ops` — the first two rungs, and the ops file that sculpts what you generated.
- `ludion docs voxel-format` — the schema `from-json` validates the described model against.
- `ludion docs voxel-authoring` — checking and rendering a regenerated model.
