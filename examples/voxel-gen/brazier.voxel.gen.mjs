// A coal brazier: the worked example of the from-scratch voxel generation path, and the standing
// proof of the `voxel-drift` gate (`scripts/voxel-drift.mjs`).
//
//   node examples/voxel-gen/brazier.voxel.gen.mjs \
//     | ludion voxel from-json --out examples/voxel-gen/brazier.voxel.yaml
//
// It is deliberately small (20x16x20) and deliberately uses every primitive worth knowing: a
// `cylinder` shaft, three `lathe` sweeps (the bowl, the hollow carved out of it, the bed of coals),
// a hand-placed lug that `mirrorX` turns into a symmetric pair, and a seeded `speckle` that lights
// a fraction of the coals as embers. Nothing here writes YAML — `emit` prints JSON and the engine
// writes the model.

import { Grid, emit, speckle } from '../../scripts/voxel/grid.mjs';

const W = 20, H = 16, D = 20;
// The exact centre of an even-width grid is a half cell. Every radial shape is centred here, which
// is what makes the model symmetric analytically rather than by eye.
const C = (W - 1) / 2;

const grid = new Grid(W, H, D);

// Foot: two crossed bars, so the brazier stands on four points without four separate legs to keep
// connected to the shaft.
grid.box([4, 0, 9], [15, 1, 10], 'I');
grid.box([9, 0, 4], [10, 1, 15], 'I');

// Shaft: inclusive at the base, exclusive at the far end, so it occupies y 0..6 and meets the
// bowl's foot exactly at y=6.
grid.cylinder([C, 0, C], 'y', 2.2, 7, 'I');

// Bowl: a solid frustum flaring from the shaft to the rim...
grid.lathe([
  { y: 6, cx: C, cz: C, rx: 3.5, rz: 3.5 },
  { y: 13, cx: C, cz: C, rx: 6.5, rz: 6.5 },
], 'I');
// ...then hollowed by a second lathe writing the empty glyph. The offsets leave a one-cell wall at
// the rim and a solid floor below y=8.
grid.lathe([
  { y: 8, cx: C, cz: C, rx: 2.0, rz: 2.0 },
  { y: 13, cx: C, cz: C, rx: 5.5, rz: 5.5 },
], '.');

// The bed of coals, sitting on the bowl floor and staying just inside the hollow.
grid.lathe([
  { y: 8, cx: C, cz: C, rx: 2.0, rz: 2.0 },
  { y: 11, cx: C, cz: C, rx: 3.8, rz: 3.8 },
], 'C');

// One carrying lug, drawn on the low-x side only; `mirrorX` copies the whole half and gives the
// brazier its pair. (Everything above is already symmetric about C, so the mirror moves exactly
// these cells — which is the point: symmetry is made true, not eyeballed.)
grid.box([2, 10, 9], [5, 11, 10], 'I');
grid.mirrorX();

// Embers last, so they are NOT mirrored — a symmetric fire reads as a pattern, not a fire.
speckle(grid, { from: [0, 8, 0], to: [W - 1, 11, D - 1] }, 'C', 'E', 0.22, 20260801);

emit({
  grid,
  name: 'brazier',
  scale: 0.1,
  generatedFrom: 'brazier.voxel.gen.mjs',
  palette: {
    C: { color: [0.09, 0.07, 0.07, 1.0], roughness: 0.95 },
    E: { color: [1.0, 0.44, 0.12, 1.0], emissive: 4.0, roughness: 0.6 },
    I: { color: [0.22, 0.21, 0.2, 1.0], metallic: 0.85, roughness: 0.45 },
  },
  header: [
    'Brazier — a hollow iron fire bowl on a crossed foot, filled with coals (C) and live embers',
    '(E, emissive) that read at night under bloom. Two carrying lugs (I) at the rim sides.',
    'DELIBERATELY SYMMETRIC except the embers — a mirrored fire reads as a pattern.',
    'Front = row z=0 (meshes at world -Z, so -Z is the model\'s forward). Bottom-anchored at y=0.',
    'Intended in-game scale: 0.10 (grid height 16 x 0.10 = 1.6 world units).',
  ],
  layerComments: {
    0: 'crossed foot bars',
    2: 'shaft',
    6: 'bowl foot flares off the shaft',
    8: 'bowl floor; the bed of coals starts here',
    10: 'bowl wall; carrying lugs to the sides',
    12: 'upper wall, coals below the rim',
    13: 'rim',
    14: 'open air above the fire',
  },
});
