# GI wave 1 — the three sweeps that settled `gi-interior`'s `GiVolume`

`Plan/gi-wave1-sky-visibility.md` §9 requires `cell_size` and `probe_spacing` to be picked off a
sweep rather than by taste, and §14 leaves the shipped `probe_spacing` and `normal_bias` to this
work package. All three were settled on the sheets beside this note.

The sheets are **judging instruments, not goldens**. A `--sweep` cell is deliberately not
byte-identical to the same `--set` render (the label overlay schedules an extra pass), nothing reads
these PNGs at build or test time, and every command line below regenerates them. The region
statistics quoted here were measured on separate `--set` renders at the golden's own 960×540, not on
sheet cells.

## The shipped values

| field | shipped | why, in one line |
|---|---|---|
| `cell_size` | `0.25` | the coarsest cell at which the geometry is converged — halving to `0.125` moves the doorway wedge by 0.00 of 255 — and half the 0.5 wall thickness, so every wall is two cells thick and the DDA cannot step through one diagonally |
| `probe_spacing` | `0.5` | the coarsest spacing that does not leak: equal to the thinnest occluder, so no lattice edge ever spans a wall. `1.0` doubles the sealed roof soffit's luminance |
| `normal_bias` | `0.25` | half the probe spacing — the largest offset that clears the lattice plane the surface sits on while staying strictly inside the thinnest wall |
| `sky_visibility` | `1.0` | the full bake; the scene exists to show it |

`sky_visibility` needs no sweep — `0.0` is the arc's identity gate and `1.0` is the product. The two
ends are worth looking at once, and one command shows them:

```
ludion render --scene examples/gallery/gi-interior.yaml --out ends.png --width 960 --height 540 \
  --sweep /entities/0/components/GiVolume/sky_visibility=0.0,1.0
```

At `0.0` the sealed room is lit exactly as if it had no roof. That is the whole feature in one pair
of cells, and it is why the calibration scene is worth its golden.

## How the numbers were measured

Each cell of each ladder was also rendered on its own at the golden's resolution
(`--set` instead of `--sweep`) and four rectangles of the 960×540 frame were averaged as 8-bit luma.
The rectangles, in `(x0, y0)–(x1, y1)` pixels:

| region | rect | what it is | wanted |
|---|---|---|---|
| `roof_soffit` | (100, 95)–(400, 175) | the roof's underside over the back of the room — sealed, sees sky only obliquely through the door | low |
| `west_wall` | (30, 200)–(270, 300) | the west wall's inner face behind the torch — sealed | low |
| `wedge` | (420, 270)–(690, 330) | the floor immediately inside the doorway | high |
| `back_corner` | (10, 430)–(230, 535) | the floor in the corner the camera stands in | low |

`roof_soffit` and `west_wall` are the leak metrics: neither surface has any light source but the sky
it cannot see, so every unit of luminance there is sky that arrived through solid geometry.

## `gi-wave1-probe-spacing.png` — the leak ladder (deleted before publication; in Lore history)

2×2, reading `0.25 / 0.5` then `1.0 / 2.0`, on the golden's own camera.

```
ludion render --scene examples/gallery/gi-interior.yaml --out Plan/artifacts/gi-wave1-probe-spacing.png \
  --width 1920 --height 1080 \
  --sweep /entities/0/components/GiVolume/probe_spacing=0.25,0.5,1.0,2.0
```

| `probe_spacing` | roof_soffit | west_wall | wedge | back_corner | probes |
|---|---|---|---|---|---|
| 0.25 | 39.87 | 15.78 | 84.05 | 30.19 | 197 589 |
| **0.5** | **43.06** | **15.85** | **82.97** | **28.03** | **26 411** |
| 1.0 | 78.59 | 19.58 | 88.59 | 32.61 | 3 750 |
| 2.0 | 76.16 | 28.72 | 96.99 | 29.49 | 676 |

**The criterion, and where it breaks.** The walls are 0.5 units thick. At `probe_spacing ≤ 0.5` no
edge of the probe lattice spans a wall, so trilinear interpolation never mixes an outdoor probe into
an indoor sample: 0.25 and 0.5 agree to within 8% on the roof soffit and within 0.5% on the west
wall. At 1.0 the lattice steps straight over a wall and the soffit's luminance nearly doubles — that
is sky arriving through 0.5 units of solid concrete, and on the sheet it reads as the whole
roof/wall junction turning light grey. At 2.0 the interior is washed out entirely; its higher
`wedge` figure is leak, not light.

**Why not go finer.** 0.25 costs 197 589 probes against 0.5's 26 411 — 7.5× the bake — and buys
nothing: the sealed regions are already as dark as they get, and the sheet shows the cost, not a
gain. At 64 rays per probe the per-probe estimate carries visible variance, and at 0.25 the lattice
resolves that variance instead of averaging it out — the 0.25 cell is the mottled one, blotching the
floor and the west wall. 0.5 is the coarsest spacing with no leak, and it is also the smoothest.

## `gi-wave1-normal-bias.png` — the two-sided ladder (deleted before publication; in Lore history)

Eight cells, four columns `0.0 / 0.25 / 0.5 / 1.0`. **Top row: the golden's camera** (inside, where
too much bias leaks). **Bottom row: a diagnostic camera outside the room** (where too little bias
darkens outward-facing surfaces — the case the field exists for, which the golden's camera cannot
show). Built from two `--sweep` invocations whose 2×2 blocks were re-cut into rows and stacked; the
re-cut copies pixels and changes none, and every cell keeps the sweep's own burned label.

```
ludion render --scene examples/gallery/gi-interior.yaml --out nb-in.png --width 1920 --height 1080 \
  --sweep /entities/0/components/GiVolume/normal_bias=0.0,0.25,0.5,1.0

ludion render --scene examples/gallery/gi-interior.yaml --out nb-out.png --width 1920 --height 1080 \
  --set /entities/1/components/Transform/translation/0=0.0 \
  --set /entities/1/components/Transform/translation/1=3.0 \
  --set /entities/1/components/Transform/translation/2=-12.0 \
  --set /entities/1/components/Transform/rotation/0=7.36 \
  --set /entities/1/components/Transform/rotation/1=180.0 \
  --sweep /entities/0/components/GiVolume/normal_bias=0.0,0.25,0.5,1.0
```

| `normal_bias` | roof_soffit | west_wall | wedge | back_corner | exterior wall face |
|---|---|---|---|---|---|
| 0.0 | 39.92 | 14.66 | 85.22 | 27.20 | 164.54 |
| **0.25** | **43.06** | **15.85** | **82.97** | **28.03** | **167.00** |
| 0.5 | 44.33 | 16.21 | 83.72 | 30.54 | 168.47 |
| 1.0 | 45.09 | 22.79 | 78.22 | 33.25 | 169.06 |

The last column is the south wall's *outer* face on the diagnostic camera — a surface with the whole
sky above it, whose correct answer is "as bright as the value saturates to".

**The criterion.** Every surface in this room lies exactly on a probe plane (the lattice starts at
the grid origin and steps 0.5, and the floor, the ceiling and all four walls land on multiples of
it), so at `normal_bias = 0.0` a surface samples the probes buried in its own wall — the ones the
bake pushed out — and outward-facing surfaces come back 2.5 levels of 255 dark against where the
value saturates. Bias fixes that, and the recovery is nearly complete by 0.25: 0.25 buys 57% of the
0 → 1 travel, 0.5 buys 82%, 1.0 buys the rest.

Bias in the other direction is a leak, because an offset the size of a wall pushes the sample point
through it. That is what the top row shows: the west wall's sealed inner face holds at 14.7 / 15.9 /
16.2 through 0.5 and then jumps to 22.8 at 1.0 — a 44% step, once the offset equals two wall
thicknesses.

So the rule the value comes from: **half the probe spacing.** It is the largest offset that clears
the lattice plane the surface sits on, and it stays strictly inside the thinnest occluder in the
scene — 0.25 < 0.5 on both counts. It happens to be the component's default, which is a coincidence
worth naming rather than leaning on: the default was chosen before any scene authored a volume.

## `gi-wave1-cell-size.png` — the convergence ladder (deleted before publication; in Lore history)

2×2, reading `0.125 / 0.25` then `0.5 / 1.0`, on the golden's own camera.

```
ludion render --scene examples/gallery/gi-interior.yaml --out Plan/artifacts/gi-wave1-cell-size.png \
  --width 1920 --height 1080 \
  --sweep /entities/0/components/GiVolume/cell_size=0.125,0.25,0.5,1.0
```

| `cell_size` | roof_soffit | west_wall | wedge | back_corner | grid |
|---|---|---|---|---|---|
| 0.125 | 43.07 | 16.32 | 82.97 | 27.43 | 192×40×192 |
| **0.25** | **43.06** | **15.85** | **82.97** | **28.03** | **96×20×96** |
| 0.5 | 41.88 | 13.92 | 80.43 | 26.29 | 48×10×48 |
| 1.0 | 37.07 | 12.36 | 70.05 | 24.37 | 24×5×24 |

**The criterion — this ladder fails by over-occlusion, not by leaking.** Voxelization fills a cell
when its centre lies inside the solid, so a coarse grid fattens every wall to a whole cell and eats
into the opening it frames. At 1.0 the doorway loses 16% of its wedge (70.05 against 82.97) and the
room goes uniformly darker: the door itself has been partly bricked up. At 0.5 the loss is 3%.

At 0.25 and 0.125 the wedge is **identical to two decimal places** and the roof soffit agrees to
0.01 — the geometry is converged, and the extra 8× of grid buys nothing. 0.25 is also exactly half
the 0.5 wall thickness, which is the coarsest cell at which every wall is two cells thick; a
one-cell wall is what lets a diagonal DDA step slip through a corner.

## Two things that are not sweeps but were settled here

**The scene authors a `PostProcess` for one field only, `lift: 0.002`.** A fully-occluded surface
receives exactly zero ambient, and AgX maps zero to zero, so the un-lifted frame put **10.1% of its
colour channel samples on the black rail** — against the 2% shared ceiling in
`budgets/image-stats-windows.toml`, and, worse, a region a golden can no longer resolve a change
inside. A 0.002 lift takes `clipped_low` to exactly 0.0000 and is invisible by eye. The component is
otherwise inert here: rendered at `lift: [0, 0, 0]` the scene is **byte-identical** to authoring no
`PostProcess` at all, verified with `cmp`, so the grade is the whole of what it does.

**The `2/3`-vs-`1.0` L1 reconstruction question (§14) is not answered here** — it is a shader edit
and out of this package's scope. Nothing in these three ladders reads wrong for it: the wedge points
out of the doorway and into the room on every cell of every sheet, at every value of all three
fields. If it were being measured, this scene and this camera are the fixture to measure it on.
