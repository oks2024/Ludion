// Parity tests for scripts/voxel/grid.mjs — `node --test scripts/voxel/` (wired as the `voxel-kit`
// step in scripts/check.mjs and .github/workflows/ci.yml).
//
// The one property worth a cross-language test: `components()`'s 6-connected flood fill must find
// the same pieces, in the same order, as the Rust connectivity checker behind `check-scene`'s
// `L1129` (crates/ludion-scene/src/voxel.rs `components_of`) — a generator script that calls
// `assertConnected` before printing is trusting this file to agree with the engine that will
// validate its output next. `KIT_FIXTURE` below is reproduced as a hand-authored `.voxel.yaml` in
// crates/ludion-scene/tests/voxel_edit.rs (`connectivity_matches_the_js_kit_fixture`) — the two
// tests assert the same sizes, samples, and order over the same grid.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import {
  Grid, components, assertConnected, ellipseCells, repeat, emit, speckle,
} from './grid.mjs';

/**
 * A 4x1x4 grid with three 6-connected pieces of distinct size, so sorting (ascending by size) and
 * `sample` (each piece's first cell in (y, z, x) scan order) are both exercised:
 *   - the 3-cell "main body", an L: (0,0,0)-(1,0,0)-(1,0,1) (two flood-fill hops, not a straight run)
 *   - a 2-cell piece: (3,0,0)-(3,0,1)
 *   - a 1-cell stray: (0,0,3)
 */
function fixtureGrid() {
  const grid = new Grid(4, 1, 4);
  grid.set(0, 0, 0, 'A');
  grid.set(1, 0, 0, 'A');
  grid.set(1, 0, 1, 'A');
  grid.set(3, 0, 0, 'A');
  grid.set(3, 0, 1, 'A');
  grid.set(0, 0, 3, 'A');
  return grid;
}

test('components() sorts ascending by size, then by sample [x, y, z]', () => {
  assert.deepEqual(components(fixtureGrid()), [
    { size: 1, sample: [0, 0, 3] },
    { size: 2, sample: [3, 0, 0] },
    { size: 3, sample: [0, 0, 0] },
  ]);
});

test('components() only connects across a shared face, not a diagonal touch', () => {
  const grid = new Grid(2, 1, 2);
  grid.set(0, 0, 0, 'A');
  grid.set(1, 0, 1, 'A'); // diagonal from (0,0,0) — not face-adjacent
  const found = components(grid);
  assert.equal(found.length, 2);
  assert.ok(found.every((c) => c.size === 1));
});

test('components() counts any non-"." glyph — a second glyph is still one component', () => {
  const grid = new Grid(2, 1, 1);
  grid.set(0, 0, 0, 'A');
  grid.set(1, 0, 0, 'B');
  assert.deepEqual(components(grid), [{ size: 2, sample: [0, 0, 0] }]);
});

test('assertConnected throws naming every piece on the disconnected fixture', () => {
  assert.throws(() => assertConnected(fixtureGrid()), (err) => {
    assert.match(err.message, /3 disconnected components/);
    assert.match(err.message, /1 cell at \(0, 0, 3\)/);
    assert.match(err.message, /2 cells at \(3, 0, 0\)/);
    assert.match(err.message, /3 cells at \(0, 0, 0\)/);
    return true;
  });
});

test('assertConnected is silent on a single connected piece', () => {
  const grid = new Grid(2, 1, 1);
  grid.set(0, 0, 0, 'A');
  grid.set(1, 0, 0, 'A');
  assertConnected(grid); // must not throw
});

test('assertConnected is silent on an empty grid (zero components)', () => {
  assertConnected(new Grid(3, 3, 3)); // must not throw
});

test('ellipseCells() matches the cell count Grid#ellipse actually writes', () => {
  const grid = new Grid(6, 3, 6);
  const profile = { y: 1, cx: 2.5, cz: 2.5, rx: 2, rz: 2 };
  const written = grid.ellipse(profile, 'A');
  const footprint = ellipseCells(profile);
  assert.equal(footprint.y, 1);
  assert.equal(footprint.cells.length, written);
  for (const [x, z] of footprint.cells) assert.equal(grid.get(x, 1, z), 'A');
});

test('ellipseCells() is unbounded — a footprint far outside any grid size is still reported', () => {
  const footprint = ellipseCells({ y: 0, cx: 100, cz: 100, rx: 2, rz: 2 });
  assert.deepEqual(footprint.xRange, [98, 102]);
  assert.deepEqual(footprint.zRange, [98, 102]);
  assert.ok(footprint.cells.length > 0);
});

test('ellipseCells() fills nothing for a non-positive radius, same as the rasterizer', () => {
  assert.deepEqual(ellipseCells({ y: 0, cx: 5, cz: 5, rx: 0, rz: 2 }).cells, []);
  assert.deepEqual(ellipseCells({ y: 0, cx: 5, cz: 5, rx: 2, rz: -1 }).cells, []);
});

test('ellipseCells() rejects a profile missing a finite field, same as ellipse()/lathe()', () => {
  assert.throws(() => ellipseCells({ y: 0, cx: 0, cz: 0, rx: 2 }), /finite `rz`/);
});

// --- mirrorX ---------------------------------------------------------------------------------
//
// The trap this section pins: `speckle` (or any deliberate write) landing in the half `mirrorX` is
// about to discard. The mirror still wins — that is its job — but the loss is now counted and
// reported, the script-side twin of the ops path's `L1134`.

/** Runs `body` with `console.warn` captured, returning every warning it emitted. */
function withWarnings(body) {
  const original = console.warn;
  const warnings = [];
  console.warn = (message) => warnings.push(String(message));
  try {
    body();
  } finally {
    console.warn = original;
  }
  return warnings;
}

test('mirrorX reports changed cells and stays silent when it discards nothing', () => {
  const grid = new Grid(4, 1, 1);
  grid.set(0, 0, 0, 'A');
  let result;
  const warnings = withWarnings(() => { result = grid.mirrorX(); });
  assert.deepEqual(result, { changed: 1, overwritten: 0, glyphs: [] });
  assert.deepEqual(warnings, []);
  assert.equal(grid.get(3, 0, 0), 'A');
});

test('mirrorX counts and names the deliberate writes it discards, and still mirrors', () => {
  const grid = new Grid(4, 1, 1);
  grid.set(0, 0, 0, 'A');
  grid.set(3, 0, 0, 'B'); // a deliberate write in the half `keep: low` throws away
  grid.set(2, 0, 0, 'C');
  let result;
  const warnings = withWarnings(() => { result = grid.mirrorX(); });
  assert.deepEqual(result, { changed: 2, overwritten: 2, glyphs: ['B', 'C'] });
  assert.equal(grid.get(3, 0, 0), 'A', 'the mirror still wins — this is a warning, not a veto');
  assert.equal(grid.get(2, 0, 0), '.');
  assert.equal(warnings.length, 1);
  assert.match(warnings[0], /discarded 2 cells \(glyphs B C\) written into the high-x half/);
  assert.match(warnings[0], /run speckle and asymmetric detail after mirrorX/);
});

test('mirrorX ignores an overwrite of a cell that already held the mirrored glyph', () => {
  const grid = new Grid(4, 1, 1);
  grid.box([0, 0, 0], [3, 0, 0], 'A'); // symmetric to begin with
  let result;
  const warnings = withWarnings(() => { result = grid.mirrorX(); });
  assert.deepEqual(result, { changed: 0, overwritten: 0, glyphs: [] });
  assert.deepEqual(warnings, []);
});

test('mirrorX with keep:high names the low half, and speckle before it is the reported trap', () => {
  const grid = new Grid(6, 1, 1);
  grid.box([0, 0, 0], [5, 0, 0], 'S');
  speckle(grid, { from: [0, 0, 0], to: [5, 0, 0] }, 'S', 's', 1.0, 7); // weathering, everywhere
  grid.set(3, 0, 0, 'S'); // one kept-half cell back to plain stone, so the mirror has work to do
  let result;
  const warnings = withWarnings(() => { result = grid.mirrorX({ keep: 'high' }); });
  assert.equal(result.overwritten, 1);
  assert.deepEqual(result.glyphs, ['s']);
  assert.match(warnings[0], /written into the low-x half/);
});

// --- wedge -----------------------------------------------------------------------------------
//
// The acceptance case is house-c's roof (games/vigil/scenes/voxels/house-c.voxel.gen.mjs:156-166):
// a hand-rolled per-column loop over a `roofTop(x)` function, at the building's real span and rise.
// `wedge`'s linear semantics reproduce it exactly, so the primitive is a drop-in for the shape it
// was fired by — asserted here column by column rather than by eye.

const HOUSE_C = { w: 128, cx: 63.5, x0: 8, x1: 119, base: 118, ridge: 177 };

/** house-c's own roof-height function, copied verbatim as the oracle. */
function houseCRoofTop(x) {
  const { cx, x0, base, ridge } = HOUSE_C;
  const t = Math.min(1, Math.abs(x - cx) >= cx - x0 ? 0 : 1 - Math.abs(x - cx) / (cx - x0));
  return base + Math.round((ridge - base) * t);
}

/** The highest y holding `glyph` in one column, or -1 when the column is empty of it. */
function columnTop(grid, x, z, glyph) {
  for (let y = grid.h - 1; y >= 0; y--) if (grid.get(x, y, z) === glyph) return y;
  return -1;
}

test('wedge reproduces house-c\'s hand-rolled gable roof column for column', () => {
  const { w, x0, x1, base, ridge } = HOUSE_C;
  const grid = new Grid(w, 192, 3);
  const filled = grid.wedge({ from: [x0, base, 0], to: [x1, ridge, 2] }, 'S');

  const expected = new Grid(w, 192, 3);
  let hand = 0;
  for (let x = x0; x <= x1; x++) hand += expected.box([x, base, 0], [x, houseCRoofTop(x), 2], 'S');

  assert.equal(filled, hand);
  assert.deepEqual(grid.cells, expected.cells);
});

test('wedge places the eave, mid-slope and ridge columns at their exact heights', () => {
  const { w, x0, x1, base, ridge } = HOUSE_C;
  const grid = new Grid(w, 192, 3);
  grid.wedge({ from: [x0, base, 0], to: [x1, ridge, 2] }, 'S');

  assert.equal(columnTop(grid, x0, 1, 'S'), 118, 'the eave sits at the base, one cell of slate');
  assert.equal(columnTop(grid, 36, 1, 'S'), 148, 'mid-slope, halfway up by construction');
  // The span is even, so the ridge line falls between columns 63 and 64: both approach 177 and
  // land on 176. Documented behaviour, not an off-by-one.
  assert.equal(columnTop(grid, 63, 1, 'S'), 176);
  assert.equal(columnTop(grid, 64, 1, 'S'), 176);
  assert.equal(grid.get(x0 - 1, base, 1), '.', 'nothing spills past the eave');
  assert.equal(grid.get(x1 + 1, base, 1), '.');
});

test('wedge with ridge:center is symmetric about the span, even on an even span', () => {
  const { w, x0, x1, base, ridge } = HOUSE_C;
  const grid = new Grid(w, 192, 1);
  grid.wedge({ from: [x0, base, 0], to: [x1, ridge, 0] }, 'S');
  for (let x = x0; x <= x1; x++) {
    assert.equal(
      columnTop(grid, x, 0, 'S'),
      columnTop(grid, x0 + x1 - x, 0, 'S'),
      `column ${x} and its mirror must reach the same height`,
    );
  }
});

test('wedge ridge:min and ridge:max are the two shed roofs, high at that edge', () => {
  const low = new Grid(11, 12, 1);
  low.wedge({ from: [0, 0, 0], to: [10, 10, 0], ridge: 'min' }, 'A');
  assert.equal(columnTop(low, 0, 0, 'A'), 10);
  assert.equal(columnTop(low, 5, 0, 'A'), 5);
  assert.equal(columnTop(low, 10, 0, 'A'), 0);

  const high = new Grid(11, 12, 1);
  high.wedge({ from: [0, 0, 0], to: [10, 10, 0], ridge: 'max' }, 'A');
  assert.equal(columnTop(high, 0, 0, 'A'), 0);
  assert.equal(columnTop(high, 10, 0, 'A'), 10);
});

test('wedge eaves raises the far edge instead of tapering it to a point', () => {
  const grid = new Grid(11, 12, 1);
  grid.wedge({ from: [0, 0, 0], to: [10, 10, 0], ridge: 'max', eaves: 4 }, 'A');
  assert.equal(columnTop(grid, 0, 0, 'A'), 4);
  assert.equal(columnTop(grid, 5, 0, 'A'), 7);
  assert.equal(columnTop(grid, 10, 0, 'A'), 10);
});

test('wedge on axis z slopes along z and extrudes across x unchanged', () => {
  const grid = new Grid(4, 12, 11);
  grid.wedge({ from: [0, 0, 0], to: [3, 10, 10], axis: 'z', ridge: 'max' }, 'A');
  for (let x = 0; x < 4; x++) {
    assert.equal(columnTop(grid, x, 10, 'A'), 10);
    assert.equal(columnTop(grid, x, 0, 'A'), 0);
  }
});

test('wedge clips to the grid and rejects a bad axis, ridge or corner', () => {
  const grid = new Grid(4, 4, 4);
  // Spans x -6..9 and z 0..9 against a 4-cell grid: the slope is measured over the declared span,
  // the cells outside are dropped by `set`, exactly as the volume ops clip.
  grid.wedge({ from: [-6, 0, 0], to: [9, 3, 9], ridge: 'max' }, 'A');
  assert.equal(columnTop(grid, 3, 3, 'A'), 2, 'column 3 is 9/15 of the way up a rise of 3');
  assert.equal(columnTop(grid, 0, 0, 'A'), 1);
  assert.throws(() => grid.wedge({ from: [0, 0, 0], to: [1, 1, 1], axis: 'y' }, 'A'), /wedge axis/);
  assert.throws(
    () => grid.wedge({ from: [0, 0, 0], to: [1, 1, 1], ridge: 'middle' }, 'A'),
    /wedge ridge/,
  );
  assert.throws(() => grid.wedge({ from: [0, 0], to: [1, 1, 1] }, 'A'), /wedge `from`/);
});

// --- repeat ----------------------------------------------------------------------------------

test('repeat reproduces house-c\'s six crow steps, hand index arithmetic and all', () => {
  const [RISE, IN, STEPS, BASE, X0, X1] = [10, 8, 6, 118, 8, 119];

  const hand = new Grid(128, 192, 16);
  for (let i = 0; i < STEPS; i++) {
    const y0 = BASE + RISE * i;
    hand.box([X0 + IN * i, y0, 8], [X1 - IN * i, y0 + RISE - 1, 15], 'S');
    hand.box([X0 + IN * i, y0 + RISE - 2, 7], [X1 - IN * i, y0 + RISE - 1, 15], 'Q');
  }

  const marched = new Grid(128, 192, 16);
  const changed = repeat({ count: STEPS, start: [X0, BASE, 8], step: [IN, RISE, 0] },
    (i, x, y, z) => marched.box([x, y, z], [X1 - IN * i, y + RISE - 1, z + 7], 'S')
      + marched.box([x, y + RISE - 2, z - 1], [X1 - IN * i, y + RISE - 1, z + 7], 'Q'));

  assert.deepEqual(marched.cells, hand.cells);
  assert.ok(changed > 0);
});

test('repeat sums its body\'s numeric returns and tolerates a body that returns nothing', () => {
  const grid = new Grid(8, 1, 1);
  assert.equal(repeat({ count: 4, step: [2, 0, 0] }, (i, x) => grid.set(x, 0, 0, 'A')), 4);
  assert.equal(repeat({ count: 3 }, () => {}), 0);
  assert.equal(repeat({ count: 0 }, () => 5), 0);
});

test('repeat passes the marched coordinate alongside the index', () => {
  const seen = [];
  repeat({ count: 3, start: [1, 2, 3], step: [10, 0, -1] }, (...args) => seen.push(args));
  assert.deepEqual(seen, [[0, 1, 2, 3], [1, 11, 2, 2], [2, 21, 2, 1]]);
});

test('repeat rejects a bad count, march or body', () => {
  assert.throws(() => repeat({ count: -1 }, () => {}), /non-negative integer `count`/);
  assert.throws(() => repeat({ count: 1.5 }, () => {}), /non-negative integer `count`/);
  assert.throws(() => repeat({ count: 2 }, 'nope'), /needs a function/);
  assert.throws(() => repeat({ count: 2, step: [1, 2] }, () => {}), /repeat `step`/);
});

// --- emit ------------------------------------------------------------------------------------

/** The smallest model `emit` accepts: one filled cell and a palette entry for it. */
function tinyModel(extra) {
  const grid = new Grid(1, 1, 1);
  grid.set(0, 0, 0, 'A');
  return { grid, name: 'tiny', palette: { A: { color: [1, 0, 0, 1] } }, ...extra };
}

/** Runs `body` with stdout and stderr captured, returning what each received. */
function withStreams(body) {
  const written = { stdout: '', stderr: '' };
  const originals = { stdout: process.stdout.write, stderr: process.stderr.write };
  process.stdout.write = (chunk) => { written.stdout += chunk; return true; };
  process.stderr.write = (chunk) => { written.stderr += chunk; return true; };
  try {
    body();
  } finally {
    process.stdout.write = originals.stdout;
    process.stderr.write = originals.stderr;
  }
  return written;
}

test('emit without `out` still prints the intermediate to stdout and nothing else', () => {
  const written = withStreams(() => emit(tinyModel()));
  assert.equal(written.stderr, '');
  // `out: process.argv[2]` is the documented idiom, so an explicit `undefined` must behave exactly
  // like an absent key — that is what keeps one script serving both the author and the drift gate.
  const explicit = withStreams(() => emit(tinyModel({ out: undefined })));
  assert.equal(explicit.stdout, written.stdout);
  assert.deepEqual(JSON.parse(written.stdout), {
    format: 1,
    kind: 'voxel-gen',
    name: 'tiny',
    size: [1, 1, 1],
    palette: { A: { color: [1, 0, 0, 1] } },
    layers: [['A']],
  });
});

test('emit `out` writes the file itself, keeps stdout clean, and round-trips an em-dash', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'ludion-grid-emit-'));
  try {
    const file = path.join(dir, 'tiny.voxel.json');
    // The exact character the PowerShell pipe double-encoded on the way to a committed model.
    const header = ['Tiny — the emit fixture. Front = row z=0.'];
    const written = withStreams(() => emit(tinyModel({ out: file, header })));

    assert.equal(written.stdout, '', 'stdout stays free for a script that is also piped');
    assert.match(written.stderr, /tiny\.voxel\.json: 1x1x1, \d+ bytes/);

    const bytes = readFileSync(file);
    assert.ok(bytes.includes(Buffer.from([0xe2, 0x80, 0x94])), 'the em-dash is UTF-8, encoded once');
    assert.deepEqual(JSON.parse(bytes.toString('utf8')).header, header);

    // Both modes must produce the same bytes — `out` is a delivery choice, not a second format.
    const piped = withStreams(() => emit(tinyModel({ header })));
    assert.equal(bytes.toString('utf8'), piped.stdout);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
