# Generating voxel models

The sanctioned from-scratch path for a voxel model too large to hand-author: a committed generator
script builds the grid over `grid.mjs` and writes a JSON intermediate; **the engine writes the
YAML**.

```
node examples/voxel-gen/brazier.voxel.gen.mjs examples/voxel-gen/brazier.voxel.json
ludion voxel from-json --in examples/voxel-gen/brazier.voxel.json \
  --out examples/voxel-gen/brazier.voxel.yaml
```

The generator writes the `.json` itself, through `emit({ ..., out: process.argv[2] })`. **Do not
pipe it.** PowerShell re-encodes a process's stdout — and `>` redirection with it — so
`node gen.mjs | ludion voxel from-json` can double-encode every non-ASCII character in the
intermediate; an em-dash reached a committed model as `â€”` once that way. `from-json` warns with
`L1135` when it sees the signature.

Take the path as an argument rather than hard-coding it. Omitting `out` prints to stdout, which is
correct wherever no shell moves the bytes — a POSIX pipe, and one program spawning another, which is
exactly what the `voxel-drift` gate does when it runs a generator bare. `process.argv[2]` therefore
leaves one script serving both callers with no branch.

Convention: the script sits next to the model it owns and is named `<name>.voxel.gen.mjs`, the way
a generated scene sits next to its `<name>.map.yaml` source. The `.voxel.json` between them is a
build artifact — not committed.

## The contract

- **Scripts never write YAML.** `emit()` prints JSON; `ludion voxel from-json` validates it
  (`ludion explain L1133` prints the shape) and runs the one canonical writer. Every hand-rolled
  YAML emitter is a second source of truth for the format, its float precision, and its comment
  conventions — that is exactly what this pipeline exists to delete.
- **The script is the source of truth; the model file is write-only.** Declare
  `generatedFrom: '<name>.voxel.gen.mjs'` in `emit()` and the emitted model carries
  `generated_from:`, which enrols it in the repo's **`voxel-drift`** gate (`scripts/check.mjs`,
  `.github/workflows/ci.yml`): every committed model bearing that key must regenerate
  byte-identical or the commit is rejected. Never hand-edit such a file — edit the script and
  regenerate.
- **Determinism is the price of that gate.** `Math.random` is banned in this library and in
  generator scripts. Scattered detail uses `speckle(..., seed)`, which runs its own PRNG.
- **Not every script wants the key.** A script that was one-shot scaffolding for later hand
  sculpting stays in `.ludion/scratch/`, omits `generatedFrom`, and the *file* becomes
  authoritative from then on — the same rule the maps already teach.

Generation is the fallback, not the first move. The documented ladder for a 48–64³ model is:
hand-author ≤32³ → `ludion voxel upscale` → sculpt with `ludion voxel apply` ops. Generate from
scratch only when there is no low-resolution ancestor to grow.

## The library

`grid.mjs`, plain ESM, zero dependencies:

| Export | What it does |
|---|---|
| `new Grid(w, h, d)` | Dense grid of `.`; x across, y up (layers), z back (rows). |
| `grid.get(x, y, z)` | Glyph, or `.` out of range. |
| `grid.set(x, y, z, g, {mode})` | One cell. Out of range is a no-op. |
| `grid.box(from, to, g, {mode})` | Inclusive axis-aligned box. |
| `grid.ellipsoid(center, radii, g, {mode})` | `sum(((c - center)/radii)^2) <= 1`. |
| `grid.cylinder(base, axis, radius, height, g, {mode})` | Inclusive base, exclusive far end. |
| `grid.line(from, to, g, {mode})` | 3D Bresenham, both ends included. |
| `grid.ellipse({y, cx, cz, rx, rz}, g, {mode})` | One cross-section, in one layer. |
| `grid.lathe(profiles, g, {mode})` | **The organic primitive**: stacked interpolated ellipses. |
| `profileAt(profiles, y)` | The cross-section a `lathe` fills at `y` — for hanging details off it. |
| `grid.wedge({from, to, axis, ridge, eaves}, g, {mode})` | **The architectural primitive**: a sloped extrusion — gable (`ridge: 'center'`) or shed (`'min'`/`'max'`). |
| `repeat({count, start, step}, fn)` | An arithmetic march: `fn(i, x, y, z)` per iteration, numeric returns summed. |
| `grid.mirrorX({keep})` | Copy one half across x; makes symmetry true, not approximate. Returns `{changed, overwritten, glyphs}` and warns when it discarded deliberate writes. |
| `grid.layers()` | The grid as `layers` row strings. |
| `components(grid)` | The grid's 6-connected pieces: `{size, sample}[]`, smallest (likeliest-stray) first — matches `check-scene`'s `L1129` checker cell for cell. |
| `assertConnected(grid)` | Throws, naming every piece, when `components(grid)` finds more than one — the pre-emit one-liner. |
| `ellipseCells(profile)` | The integer XZ footprint `ellipse`/`lathe` would rasterize at one cross-section — inspect a lathe layer without regenerating the grid. |
| `speckle(grid, region, from, to, fraction, seed)` | Deterministic seeded scatter. Run it **before** `mirrorX` only for the kept half. |
| `mulberry32(seed)` | The PRNG, if a script needs its own draws. |
| `emit({..., out})` | Writes the JSON intermediate to `out` (summary on stderr), or prints it to stdout when `out` is absent. |

`mode: 'paint'` (on every shape op) writes only over already-filled cells — detail onto a surface
without changing the silhouette. The shape ops mirror `ludion voxel apply`'s ops cell for cell, so
a shape generated here and the same shape applied as an op land identically.

`lathe` is the reason the library exists. An organic model is a stack of cross-sections: give it
the four ellipse parameters at the y levels where the anatomy changes and it interpolates every
layer between them.

```js
grid.lathe([
  { y: 0,  cx: 8, cz: 8, rx: 3.0, rz: 3.5 },  // feet
  { y: 6,  cx: 8, cz: 8, rx: 2.5, rz: 2.5 },  // legs
  { y: 12, cx: 8, cz: 8, rx: 5.0, rz: 4.0 },  // torso
  { y: 19, cx: 8, cz: 8, rx: 3.0, rz: 3.0 },  // head
], 'H');
```

A creature is not only its silhouette, though — the arm root sits at the torso's edge, the gill at a
fraction of the neck's width, the mouth on the head's front row. `profileAt` samples the same
interpolation the sweep ran, so those numbers stay *derived* from the body instead of becoming a
second copy of the taper that drifts the first time the body is retuned. `grid.ellipse` then places
the detail in that one layer.

```js
for (let y = 12; y <= 18; y++) {
  const torso = profileAt(BODY, y);
  grid.ellipse({ y, cx: torso.cx - torso.rx, cz: torso.cz - torso.rz + 1, rx: 1.5, rz: 1.5 }, 'G');
}
```

## Buildings: `wedge` and `repeat`

`wedge` is `lathe`'s architectural counterpart. Where a creature is a stack of cross-sections, a
building is a box with a sloped top: the columns along `axis` fill from the base up to a linearly
interpolated height, 1 at the ridge and 0 at the slope's far edge. `ridge: 'center'` is a gable,
`'min'`/`'max'` the two shed roofs, and `eaves` raises the far edge instead of tapering it to a
point. On an even span the centre falls between two columns, so the declared ridge y is a limit they
approach rather than reach — the same half-cell honesty an even-width lathe has.

```js
grid.wedge({ from: [8, 118, 8], to: [119, 177, 103] }, 'K');   // a gable roof, ridge on centre
grid.wedge({ from: [0, 0, 0], to: [15, 6, 9], ridge: 'max' }, 'S');  // a shed lean-to
```

`repeat` marches a coordinate so a rhythm — quoins up a corner, a corbel table, the treads of a
crow-stepped gable — stops being a hand-indexed `for` loop, which is where the off-by-ones live. It
sums whatever the body returns, so it reports cells changed like every other primitive.

```js
repeat({ count: 6, start: [8, 118, 8], step: [8, 10, 0] }, (i, x, y, z) =>
  grid.box([x, y, z], [119 - 8 * i, y + 9, z + 7], 'S'));   // six steps, each inset 8
```

## The one ordering trap: write, then mirror

`mirrorX` **replaces** the half it discards. Speckle the whole grid, or cut an asymmetric detail,
and then mirror, and half that work is silently gone. Bound deliberate writes to the kept half, or
run them after the mirror:

```js
speckle(grid, { from: [0, 0, 0], to: [cx - 0.5, h - 1, d - 1] }, 'S', 's', 0.2, 0x5c3a17);
grid.mirrorX({ keep: 'low' });   // everything after this line is deliberate asymmetry
```

Get it wrong and `mirrorX` says so on `console.warn`, and returns
`{changed, overwritten, glyphs}` — `overwritten` counts the deliberate writes it discarded and
`glyphs` names them. It is a warning, not a throw, matching `L1134` on the ops-file path: a mirror
squaring off hand-placed detail is legal, just rarely what was meant.

## A complete generator

```js
import { Grid, emit } from '../../scripts/voxel/grid.mjs';

const grid = new Grid(9, 12, 9);
const c = 4;                                     // (9 - 1) / 2, the centre column
grid.cylinder([c, 0, c], 'y', 1.2, 8, 'W');      // trunk
grid.ellipsoid([c, 9, c], [3.5, 3, 3.5], 'L');   // canopy
grid.mirrorX();                                  // symmetry is exact, not eyeballed

emit({
  grid,
  name: 'sapling',
  scale: 0.25,
  generatedFrom: 'sapling.voxel.gen.mjs',
  out: process.argv[2],                       // a path writes the file; bare still prints to stdout
  palette: {
    L: { color: [0.24, 0.52, 0.22, 1.0], roughness: 0.8 },
    W: { color: [0.32, 0.24, 0.16, 1.0], roughness: 0.9 },
  },
  header: ['A sapling: one trunk, one canopy. Front = row z=0.'],
  layerComments: { 0: 'trunk base', 9: 'canopy' },
});
```

`layerComments` maps a layer index to its anatomy note; a plain array works too, since its indices
*are* the layer numbers — the shorter spelling when a model comments every layer. The house
convention is that the note names the layer it sits on (`y7 — knees, joint bulge`); the text is
emitted verbatim, so write the prefix you want.

## Verify connectivity before emit

A disconnected model is a mistake caught late if the first sign of it is `check-scene`'s `L1129`
after a full round trip through the engine. Catch it in the script instead, right before `emit`:

```js
import { Grid, assertConnected, emit } from '../../scripts/voxel/grid.mjs';

const grid = new Grid(20, 20, 20);
// ... build the model ...

assertConnected(grid); // throws here, naming every piece, if the sculpt left a stray cell
emit({ grid, /* ... */ });
```

`components(grid)` is the same check without the throw — `{size, sample}[]`, smallest first —
useful when a script wants to log a piece count rather than fail the build outright.

## Inspect a lathe cross-section

"What cells does this lathe fill at y=18" without regenerating the whole grid to look:

```js
import { profileAt, ellipseCells } from '../../scripts/voxel/grid.mjs';

const cs = ellipseCells(profileAt(BODY, 18));
console.log(`y=18: x ${cs.xRange[0]}..${cs.xRange[1]}, z ${cs.zRange[0]}..${cs.zRange[1]}, ${cs.cells.length} cells`);
```

Handy for placing a detail relative to a cross-section's true rasterized edge rather than the
profile's decimal radius — the two differ by up to half a cell at an integer boundary.

Verify what came out without rendering: `ludion check-scene <model>` (validity, fill count, the
`L1129` connectivity warning — which names the nearest pair of cells across each gap, not just a
sample), `--slice <y>` for an annotated layer, `--column x,z` for a vertical profile, `--mirror-x`
for symmetry, `--exposure` for what is actually visible (buried detail cells, and which way each
emissive glyph's faces point). `ludion render --model <model>` when you want to see it.

Or validate the generator's *output* directly, standalone, with nothing written and no `--out`
path needed yet:

```
node examples/voxel-gen/brazier.voxel.gen.mjs /tmp/brazier.voxel.json
ludion voxel from-json --in /tmp/brazier.voxel.json --validate
```

This runs the same `L1115`/`L1129` checks and exposure note `--out` would, over the in-memory
result — the fast way to eyeball a script's output before it earns a committed path.

The worked example lives in `examples/voxel-gen/` — a coal brazier exercising `lathe`, `cylinder`,
`speckle`, `mirrorX`, and an emissive palette entry. It is also the `voxel-drift` gate's standing
proof.
