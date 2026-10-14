---
description: Voxel models past hand-editing size: the ladder, voxel new/apply/upscale/diff, the cell budget
---

# Voxel ops

**Scope:** the write side for models above roughly 32³, where typing the slices stops working — the
scaffold, the ops file, upscaling and diffing. Building a model from a committed script is the
ladder's third rung and its own topic (`ludion docs voxel-generation`). The document these verbs
write is `ludion docs voxel-format`; the read side that verifies every rung is
`ludion docs voxel-authoring`; what to aim the extra resolution at is `ludion docs voxel-craft`.

## The loop

### Authoring above hand-editing size — the ladder

Hand-authoring works up to roughly **32³** and then stops: a `48³` model is 48 layers of 48 quoted
rows, and painting a shape into it means counting columns in fixed-width strings a hundred times
without one off-by-one. `ludion voxel <verb>` is the write side past that point.
**Take the rungs in order. Do not start at the bottom.**

1. **Hand-author at ≤32³.** The format is at its best here, and a small model that is *correct* is
   worth more than a large one that is nearly right.
2. **Upscale a proven low-res model, then sculpt it with ops.** `ludion voxel upscale` turns a
   verified 16³ into a correct, connected, palette-identical 48³ in one command; `ludion voxel apply`
   adds the detail the extra resolution is for. This is the normal path to a 48–64³ model.
3. **Generate from a committed script** — only when there is no low-resolution ancestor to grow;
   `ludion docs voxel-generation` is that rung.

The ladder decides *how* one grid is written. A model whose parts must move independently is a
different question — it becomes an assembly of small part files, each of them back at rung 1
(`ludion docs voxel-assemblies`).

The rung that is *not* on the ladder: writing a script that prints YAML — `ludion voxel from-json` is
the only sanctioned path from a script to a model file, and `ludion docs voxel-generation` has the
reason.

Every writing verb validates the whole result in memory — the same checks `check-scene` runs — and
writes temp-then-rename only on success. A rejected op leaves the file byte-identical, so a
half-applied model never reaches disk (and never reaches a watching viewer as a blank).

### `ludion voxel new` — the scaffold

```
ludion voxel new voxels/golem.voxel.yaml --size 16,24,16 --palette-from voxels/lantern.voxel.yaml
```

Writes an all-`.` model carrying the header docstring template, plus one `# y<N>` anchor comment per
layer so you never count layers to find where you are (a reshape or y-shift renumbers the anchor
with its layer). `--palette-from` copies an existing model's palette — how a new model joins a
set's colour language instead of inventing a ninth grey. It never overwrites an existing file.

### `ludion voxel apply` — the ops file

An **ops file** is a typed, strict-YAML list of coordinate-addressed edits — the one batch-edit
interface, with no flag-soup equivalent. A single cell is still a one-character text edit at a known
line and column.

```yaml
format: 1
kind: voxel-ops
ops:
  - op: ellipsoid          # the skull
    center: "8,20,8"
    radii: "4,3,4"
    glyph: F
  - op: box                # eye sockets, carved
    from: "6,20,0"
    to: "9,21,1"
    glyph: "."
  - op: box                # recolour the shins without moving a silhouette cell
    from: "0,0,0"
    to: "15,5,15"
    glyph: M
    mode: paint
  - op: mirror             # make symmetry true, not eyeballed
    axis: x
    keep: low
  - op: comment            # the anatomy changed, so the layer note does too
    y: 20
    text: "y20 — skull, sockets carved"
```

- **Coordinates are comma-joined quoted tokens** (`"8,20,8"`) because the strict-YAML subset rejects
  inline sequences; radii may be decimals. `glyph: "."` erases. `mode: paint` writes only over
  already-filled cells — detail onto a surface without touching the silhouette; the default `place`
  writes every cell in the shape.
- **Volume ops (`box`, `ellipsoid`, `cylinder`, `line`) clip to the grid; the point op (`set`) does
  not** — an out-of-range `set` is `L1132`, because an exact address that misses is a mistake, not
  something to swallow. `line`'s diagonal-step trap — an edge-touching cell that looks continuous in
  a slice but isn't connected — is the connectivity check in `ludion docs voxel-format`; the ops file
  doesn't change it.
- **`op: mirror`** copies the kept half onto the other half of `axis`; an odd extent's centre slice
  mirrors onto itself. Order it last unless you mean the overwrite: `L1134` warns, per earlier
  writer, when a mirror discards cells that op deliberately wrote.
- **`grow` / `crop`** reshape the grid; `shift` moves cells inside it; everything else edits cells
  in place.

```
ludion voxel apply voxels/golem.voxel.yaml --ops ops/skull.yaml --dry-run
```

Dropping `--dry-run` applies it. Either way it prints the cells each op changed, then the resulting
size, fill and component count — "did the ellipsoid land where I meant" answered without a render.
**Cell-level edits are surgical**: only the changed cell characters are rewritten, so the
header docstring, every layer comment, blank lines and float formatting survive byte-for-byte. A
reshape (`grow` / `crop`) or a y-shift cannot be, so it re-emits canonically: layer comments follow
their content and a leading `y<N>` (or range `y<N>-y<M>`) is renumbered. A comment whose layer leaves the grid is reported,
not silently dropped. A model carrying `generated_from:` is refused (`L1132`) — edit its script;
`--dry-run` still previews.

### `ludion voxel upscale` — the second rung, and the highest-leverage verb

```
ludion voxel upscale voxels/lantern.voxel.yaml --factor 3
```

Nearest-neighbor: each source cell becomes a `factor³` block (factor `2..=8`), so the result is *the
same model* one resolution up — correct, connected and palette-identical by construction, the base
that is expensive to produce any other way. What carries:

- `size` multiplies by the factor; a declared `scale` **divides** by it, so the intended world
  footprint is unchanged. The header comment carries verbatim plus an `upscaled x<factor>` line, and
  each layer's anatomy comment moves onto the first of its `factor` output layers.
- `budget_exempt` carries over; `generated_from` is deliberately **dropped** — the upscale is not that
  script's product, and keeping the key would make the drift gate reject the file
  (`ludion docs voxel-generation`).
- The source model is never touched, and an existing file at the `--out` path stops the command.

Then sculpt with `apply` ops: the detail the blocks can now hold is the whole point of 48³ over 16³.

### `ludion voxel diff` — did my edit land

```
ludion voxel diff voxels/lantern-x3.voxel.yaml voxels/lantern-hd.voxel.yaml
```

Palette changes, then per-layer changed-cell counts with `(x,z) 'A'->'B'` samples; a size mismatch
diffs the overlapping region. Exit codes follow `diff(1)`: **0** identical, **1** differ, **2** a
model cannot be read or validated — the review tool for a regenerated model.

## Rules

### `budget_exempt:` — the explicit opt-out

`L1115` warns above 262,144 cells (64³); `budget_exempt: true` suppresses it for that file. Only
`true` is valid — `budget_exempt: false` is an `L1114` telling you to remove the key, since its
absence already means "warn me". It is an author's explicit call: no tool ever adds it for you, and
`upscale` still warns when its result crosses the budget.

## Gotchas

- **`L1115` — over budget** is a warning, not an error: `check-scene` still succeeds. Author-scale
  models live around 16³–48³, and world-scale content is chunked rather than one giant file;
  deliberately large is [`budget_exempt: true`](#budget_exempt--the-explicit-opt-out).
- **`L1131` / `L1132` — the ops file.** `L1131` is the ops document's own structure (a misspelled
  `op:`, a missing field, a bad coordinate token), reported with a JSON Pointer like `/ops/2/op` and
  a nearest-match suggestion; `L1132` is an op that parsed but does not fit the model (a `set`
  outside the grid, a glyph with no palette entry, an inverted `crop`). Neither ever leaves a
  half-applied file — see [the ladder](#authoring-above-hand-editing-size--the-ladder).

## Next

- `ludion docs voxel-generation` — the third rung: generator scripts, `from-json`, the drift gate.
- `ludion docs voxel-authoring` — checking and rendering what these verbs write.
- `ludion docs voxel-format` — the schema every written model is validated against.
- `ludion docs voxel-craft` — spending the resolution `upscale` bought you.
- `ludion docs voxel-assemblies` — several small grids that move, instead of one large one.
