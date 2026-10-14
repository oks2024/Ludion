// The committed voxel generation library: build a grid, print the JSON intermediate.
//
// This is the substrate half of the from-scratch generation path (Plan/voxel-authoring-tools.md).
// Without it, every agent instance that needed a model too large to hand-author
// wrote its own `setCell`/`fillEllipse`/hand-rolled YAML emitter in a gitignored scratch script —
// ~280 lines of substrate per ~60 lines of actual model, thrown away each time. That substrate
// lives here once. What a generator script contains is now only the model.
//
// Two rules govern this file:
//
//   1. **It never writes YAML.** `emit()` prints a JSON intermediate; `ludion voxel from-json`
//      validates it and writes the canonical file. Canonical serialization stays in exactly one
//      place, the engine — see scripts/voxel/README.md.
//   2. **It is deterministic.** `Math.random` is banned; `speckle` takes an explicit seed and runs
//      its own PRNG. A generated model is committed and gated by `voxel-drift`, so the same script
//      must print the same bytes forever.
//
// The volume primitives deliberately mirror `ludion voxel apply`'s ops (crates/ludion-scene/
// src/voxel_edit.rs): same analytic inequalities, same clipping, same inclusive/exclusive ends —
// so a shape sculpted with an ops file and the same shape generated here land on the same cells.
//
// Zero npm dependencies, Node built-ins only. The one filesystem touch is `emit({ out })`, which
// writes the intermediate itself so no shell ever re-encodes it. Runs unchanged on Windows and macOS.

import { writeFileAtomic } from '../atomic-write.mjs';

/** The reserved glyph meaning "empty cell". */
export const EMPTY = '.';

/**
 * A dense voxel grid of single-character glyphs, addressed in voxel coordinates: x across
 * (width), y up (height, layer index), z back (depth, row index within a layer).
 */
export class Grid {
  /**
   * @param {number} w Width in cells (x).
   * @param {number} h Height in cells (y) — the layer count.
   * @param {number} d Depth in cells (z) — the rows per layer.
   */
  constructor(w, h, d) {
    this.w = w;
    this.h = h;
    this.d = d;
    this.cells = new Array(w * h * d).fill(EMPTY);
  }

  /** @returns {number} The flat index of a cell known to be in range. */
  #index(x, y, z) {
    return (y * this.d + z) * this.w + x;
  }

  /** @returns {boolean} Whether integer coordinates address a cell of this grid. */
  inBounds(x, y, z) {
    return x >= 0 && x < this.w && y >= 0 && y < this.h && z >= 0 && z < this.d;
  }

  /**
   * The glyph at a cell, or `.` when the coordinate is out of range — an out-of-range read is
   * empty, the same convention `VoxelDocument::palette_index` follows.
   * @returns {string} One character.
   */
  get(x, y, z) {
    if (!this.inBounds(x, y, z)) return EMPTY;
    return this.cells[this.#index(x, y, z)];
  }

  /**
   * Writes one cell. Out of range is a silent no-op (volume ops clip; this keeps a stray
   * coordinate from ending a long generation run), and `mode: 'paint'` writes only over a cell
   * that is already filled — recolouring detail onto a surface without changing the silhouette.
   * @param {number} x
   * @param {number} y
   * @param {number} z
   * @param {string} g The glyph to write (`.` erases).
   * @param {{mode?: 'place'|'paint'}} [options]
   * @returns {number} 1 when the cell changed, 0 otherwise.
   */
  set(x, y, z, g, { mode = 'place' } = {}) {
    x = Math.round(x); y = Math.round(y); z = Math.round(z);
    if (!this.inBounds(x, y, z)) return 0;
    const i = this.#index(x, y, z);
    if (mode === 'paint' && this.cells[i] === EMPTY) return 0;
    if (this.cells[i] === g) return 0;
    this.cells[i] = g;
    return 1;
  }

  /**
   * Fills an inclusive axis-aligned box. Either corner may be the low one.
   * @param {[number,number,number]} from
   * @param {[number,number,number]} to
   * @param {string} g
   * @param {{mode?: 'place'|'paint'}} [options]
   * @returns {number} Cells changed.
   */
  box(from, to, g, options = {}) {
    let count = 0;
    for (let y = Math.min(from[1], to[1]); y <= Math.max(from[1], to[1]); y++) {
      for (let z = Math.min(from[2], to[2]); z <= Math.max(from[2], to[2]); z++) {
        for (let x = Math.min(from[0], to[0]); x <= Math.max(from[0], to[0]); x++) {
          count += this.set(x, y, z, g, options);
        }
      }
    }
    return count;
  }

  /**
   * Fills an axis-aligned ellipsoid: every cell whose integer coordinates satisfy
   * `sum(((c - center) / radii)^2) <= 1`. Centre and radii are decimal — a centre on a half-cell
   * (`(w - 1) / 2`) is how an even-width model stays symmetric.
   * @param {[number,number,number]} center
   * @param {[number,number,number]} radii
   * @param {string} g
   * @param {{mode?: 'place'|'paint'}} [options]
   * @returns {number} Cells changed.
   */
  ellipsoid(center, radii, g, options = {}) {
    if (radii.some((r) => r <= 0)) return 0;
    let count = 0;
    for (const [x, y, z] of this.#boundingCells(center, radii)) {
      const dx = (x - center[0]) / radii[0];
      const dy = (y - center[1]) / radii[1];
      const dz = (z - center[2]) / radii[2];
      if (dx * dx + dy * dy + dz * dz <= 1) count += this.set(x, y, z, g, options);
    }
    return count;
  }

  /**
   * Fills a cylinder of `radius` running `height` cells along one axis from `base`. Inclusive at
   * the base, **exclusive at the far end**, so stacking two cylinders of height h covers 2h layers
   * with no doubled seam (the same rule `voxel apply`'s `cylinder` op follows).
   * @param {[number,number,number]} base The centre of the base cap.
   * @param {'x'|'y'|'z'} axis
   * @param {number} radius
   * @param {number} height
   * @param {string} g
   * @param {{mode?: 'place'|'paint'}} [options]
   * @returns {number} Cells changed.
   */
  cylinder(base, axis, radius, height, g, options = {}) {
    if (radius <= 0 || height <= 0) return 0;
    const a = AXES[axis];
    if (a === undefined) throw new Error(`cylinder axis must be 'x', 'y', or 'z', got ${axis}`);
    const radii = [radius, radius, radius];
    radii[a] = height;
    const center = [...base];
    center[a] = base[a] + height / 2;
    const [u, v] = OTHER_AXES[a];
    let count = 0;
    for (const cell of this.#boundingCells(center, radii)) {
      if (cell[a] < base[a] || cell[a] >= base[a] + height) continue;
      const du = (cell[u] - base[u]) / radius;
      const dv = (cell[v] - base[v]) / radius;
      if (du * du + dv * dv <= 1) count += this.set(cell[0], cell[1], cell[2], g, options);
    }
    return count;
  }

  /**
   * Draws a 3D integer Bresenham line, both endpoints included.
   * @param {[number,number,number]} from
   * @param {[number,number,number]} to
   * @param {string} g
   * @param {{mode?: 'place'|'paint'}} [options]
   * @returns {number} Cells changed.
   */
  line(from, to, g, options = {}) {
    const delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    const extent = delta.map(Math.abs);
    const step = delta.map(Math.sign);
    let major = 0;
    for (let axis = 1; axis < 3; axis++) if (extent[axis] > extent[major]) major = axis;
    const [u, v] = OTHER_AXES[major];

    const cell = [...from];
    let count = this.set(cell[0], cell[1], cell[2], g, options);
    let errorU = 2 * extent[u] - extent[major];
    let errorV = 2 * extent[v] - extent[major];
    for (let i = 0; i < extent[major]; i++) {
      if (errorU > 0) { cell[u] += step[u]; errorU -= 2 * extent[major]; }
      if (errorV > 0) { cell[v] += step[v]; errorV -= 2 * extent[major]; }
      errorU += 2 * extent[u];
      errorV += 2 * extent[v];
      cell[major] += step[major];
      count += this.set(cell[0], cell[1], cell[2], g, options);
    }
    return count;
  }

  /**
   * Fills one ellipse in the XZ plane of a single layer — the cross-section {@link Grid#lathe}
   * stacks, available on its own for the layer whose shape no neighbour shares (an off-centre pair,
   * a one-layer accent, a band the profile above and below must not interpolate through).
   *
   * The profile entry is the lathe's, so a shape moves between the two without re-typing: `y` names
   * the layer, `cx`/`cz` the centre, `rx`/`rz` the radii, all decimal. A non-positive radius fills
   * nothing.
   *
   * @param {{y: number, cx: number, cz: number, rx: number, rz: number}} profile
   * @param {string} g
   * @param {{mode?: 'place'|'paint'}} [options]
   * @returns {number} Cells changed.
   */
  ellipse(profile, g, options = {}) {
    checkProfile(profile, 'ellipse');
    const { y, cx, cz, rx, rz } = profile;
    if (rx <= 0 || rz <= 0) return 0;
    return this.#ellipseXZ(Math.round(y), cx, cz, rx, rz, g, options);
  }

  /**
   * Sweeps a stack of interpolated ellipses up the y axis — the loft primitive organic models are
   * actually made of, and the reason this library exists.
   *
   * A profile entry pins the cross-section at one y: its centre (`cx`, `cz`) and its radii (`rx`,
   * `rz`). Every integer y between the lowest and highest entry gets an ellipse whose four
   * parameters are linearly interpolated between the two entries bracketing it, so a creature's
   * legs → hips → torso → shoulders → neck → head is one call with six entries rather than six
   * hand-tuned `for` loops. Entries need not be sorted or integral. A layer whose interpolated
   * `rx` or `rz` is <= 0 is skipped (a profile can taper to nothing).
   *
   * The per-layer ellipse uses the same inequality as {@link Grid#ellipsoid}, so a lathe and an
   * ellipsoid of matching parameters agree cell for cell.
   *
   * @param {{y: number, cx: number, cz: number, rx: number, rz: number}[]} profiles
   *   At least two entries.
   * @param {string} g
   * @param {{mode?: 'place'|'paint'}} [options]
   * @returns {number} Cells changed.
   */
  lathe(profiles, g, options = {}) {
    if (profiles.length < 2) {
      throw new Error(`lathe needs at least two profile entries, got ${profiles.length}`);
    }
    for (const p of profiles) checkProfile(p, 'lathe');
    const sorted = [...profiles].sort((a, b) => a.y - b.y);
    const yLo = Math.max(0, Math.ceil(sorted[0].y));
    const yHi = Math.min(this.h - 1, Math.floor(sorted[sorted.length - 1].y));
    let count = 0;
    for (let y = yLo; y <= yHi; y++) {
      const { cx, cz, rx, rz } = interpolate(sorted, y);
      if (rx <= 0 || rz <= 0) continue;
      count += this.#ellipseXZ(y, cx, cz, rx, rz, g, options);
    }
    return count;
  }

  /**
   * Fills a sloped extrusion — the building-side counterpart to {@link Grid#lathe}, and what a roof,
   * a ramp or a buttress weathering actually is. A wedge is a box whose *top* rises linearly along
   * one horizontal axis: a gable (`ridge: 'center'`), a shed roof (`ridge: 'min'` or `'max'`).
   *
   * `from` and `to` are the inclusive corners of the wedge's bounding box, the same pair
   * {@link Grid#box} takes, rounded to cells. The lower y is the **base**, filled in every column;
   * the upper y is the **ridge**, reached only by the columns at `ridge`. `eaves` is the top y at
   * the far edge of the slope and defaults to the base, so by default the wedge comes to a point
   * there; raise it for a roof that starts from a thickness rather than from nothing.
   *
   * Each column along `axis` fills base..`round(eaves + (ridge y - eaves) * t)` inclusive, where
   * `t` is 1 at the ridge and 0 at the slope's far edge. For `ridge: 'center'` it is symmetric
   * about the (possibly half-cell) centre, so an even span stays symmetric the way an even-width
   * lathe does. The other horizontal axis is uniform — the profile extrudes across it unchanged.
   * A column whose top lands below the base fills nothing.
   *
   * `t` samples a line, so on an **even** span the centre falls between two columns and the ridge y
   * is a limit those two approach rather than reach (an odd span lands on it exactly) — the same
   * half-cell honesty an even-width lathe has, and not something to correct for by widening.
   *
   * @param {{from: [number,number,number], to: [number,number,number], axis?: 'x'|'z',
   *   ridge?: 'center'|'min'|'max', eaves?: number}} span
   * @param {string} g
   * @param {{mode?: 'place'|'paint'}} [options]
   * @returns {number} Cells changed.
   */
  wedge(span, g, options = {}) {
    const { from, to, axis = 'x', ridge = 'center', eaves } = span ?? {};
    const a = AXES[axis];
    if (a !== 0 && a !== 2) throw new Error(`wedge axis must be 'x' or 'z', got ${axis}`);
    if (!RIDGES.includes(ridge)) {
      throw new Error(`wedge ridge must be 'center', 'min', or 'max', got ${ridge}`);
    }
    checkCorner(from, 'from');
    checkCorner(to, 'to');
    const lo = [0, 1, 2].map((i) => Math.round(Math.min(from[i], to[i])));
    const hi = [0, 1, 2].map((i) => Math.round(Math.max(from[i], to[i])));
    const eavesY = eaves === undefined ? lo[1] : eaves;
    if (!Number.isFinite(eavesY)) throw new Error(`wedge \`eaves\` must be finite, got ${eaves}`);
    const other = a === 0 ? 2 : 0;
    let count = 0;
    for (let c = lo[a]; c <= hi[a]; c++) {
      const top = Math.round(eavesY + (hi[1] - eavesY) * wedgeRise(c, lo[a], hi[a], ridge));
      if (top < lo[1]) continue;
      const near = [0, 0, 0];
      const far = [0, 0, 0];
      near[a] = c; far[a] = c;
      near[other] = lo[other]; far[other] = hi[other];
      near[1] = lo[1]; far[1] = top;
      count += this.box(near, far, g, options);
    }
    return count;
  }

  /**
   * Makes the model X-symmetric by copying one half over the other. On an odd width the centre
   * column mirrors onto itself and is left alone.
   *
   * The copy **replaces** whatever the discarded half held, which is this library's one ordering
   * trap: speckle the whole grid, or cut an asymmetric detail, and then mirror, and half that work
   * is silently gone. Any such loss is counted and reported on `console.warn` — a warning, not a
   * throw, matching `L1134`, the same lint on the ops-file path (`ludion explain L1134`), because a
   * mirror deliberately squaring off hand-placed detail is legal.
   *
   * @param {{keep?: 'low'|'high'}} [options] Which half survives — `low` (default) is x < w/2.
   * @returns {{changed: number, overwritten: number, glyphs: string[]}} `changed` counts every cell
   *   the copy rewrote; `overwritten` is the subset that held a *different, non-empty* glyph — the
   *   deliberate writes the mirror discarded — and `glyphs` lists those discarded glyphs, sorted.
   */
  mirrorX({ keep = 'low' } = {}) {
    let changed = 0;
    let overwritten = 0;
    const discarded = new Set();
    for (let y = 0; y < this.h; y++) {
      for (let z = 0; z < this.d; z++) {
        for (let x = 0; x < Math.floor(this.w / 2); x++) {
          const xm = this.w - 1 - x;
          const [sx, tx] = keep === 'low' ? [x, xm] : [xm, x];
          const before = this.get(tx, y, z);
          // Plain `place`: the mirror overwrites its half whatever is there, which is the whole
          // point of it — and `set` returns 1 only when the cell actually differed, so the same
          // call reports both halves of the answer.
          const wrote = this.set(tx, y, z, this.get(sx, y, z));
          changed += wrote;
          if (wrote === 1 && before !== EMPTY) {
            overwritten += 1;
            discarded.add(before);
          }
        }
      }
    }
    const glyphs = [...discarded].sort();
    if (overwritten > 0) {
      const discardedHalf = keep === 'low' ? 'high' : 'low';
      console.warn(
        `mirrorX discarded ${overwritten} cell${overwritten === 1 ? '' : 's'} `
        + `(glyph${glyphs.length === 1 ? '' : 's'} ${glyphs.join(' ')}) written into the `
        + `${discardedHalf}-x half before the mirror ran — run speckle and asymmetric detail after `
        + 'mirrorX, or bound them to the kept half',
      );
    }
    return { changed, overwritten, glyphs };
  }

  /**
   * The grid as the intermediate's `layers`: one array of row strings per y, bottom-up.
   * @returns {string[][]}
   */
  layers() {
    const out = [];
    for (let y = 0; y < this.h; y++) {
      const rows = [];
      for (let z = 0; z < this.d; z++) {
        let row = '';
        for (let x = 0; x < this.w; x++) row += this.cells[this.#index(x, y, z)];
        rows.push(row);
      }
      out.push(rows);
    }
    return out;
  }

  /** One ellipse in the XZ plane of layer `y` — the lathe's per-layer fill, clamped to this grid's
   * width/depth. Shares its integerization with {@link ellipseCells}, the grid-free inspector. */
  #ellipseXZ(y, cx, cz, rx, rz, g, options) {
    const { cells } = ellipseXZFootprint(cx, cz, rx, rz, this.w - 1, this.d - 1);
    let count = 0;
    for (const [x, z] of cells) count += this.set(x, y, z, g, options);
    return count;
  }

  /**
   * Every in-grid cell of a centre/radii shape's bounding box, in (y, z, x) scan order — the same
   * order the Rust ops engine walks, so a shape fills identically wherever it is evaluated.
   */
  *#boundingCells(center, radii) {
    const lo = (i) => Math.max(0, Math.ceil(center[i] - radii[i]));
    const hi = (i) => Math.min([this.w, this.h, this.d][i] - 1, Math.floor(center[i] + radii[i]));
    for (let y = lo(1); y <= hi(1); y++) {
      for (let z = lo(2); z <= hi(2); z++) {
        for (let x = lo(0); x <= hi(0); x++) yield [x, y, z];
      }
    }
  }
}

/** Axis-aligned face steps a 6-connected flood fill walks. Order does not affect the result (a
 * component's cell set is order-independent) — only the *outer* scan order below does. */
const FACE_STEPS = [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]];

/**
 * The 6-connected components of `grid`'s filled cells — the JS-side twin of the Rust connectivity
 * checker (`ludion-scene`'s `components_of`, behind `check-scene`'s `L1129`), matched on every
 * observable: cells are visited in (y, z, x) order so a component's `sample` is its first cell in
 * that order, only face-adjacent cells connect (a diagonal touch does not), and the result is
 * sorted by size ascending then by `sample` lexicographic ([x, y, z]) — smallest (likeliest-stray)
 * piece first, deterministic order. `grid.test.mjs` pins a fixture against both implementations.
 *
 * The one difference: this counts *any* non-`.` glyph as filled, where the Rust checker additionally
 * requires the glyph to name a real palette entry. Generator scripts only ever write palette glyphs
 * (`ludion voxel from-json` rejects an unknown one at `L1114`), so the two agree in practice.
 *
 * @param {Grid} grid
 * @returns {{size: number, sample: [number, number, number]}[]}
 */
export function components(grid) {
  const { w, h, d, cells } = grid;
  const index = (x, y, z) => (y * d + z) * w + x;
  const visited = new Array(cells.length).fill(false);
  const found = [];
  for (let y = 0; y < h; y++) {
    for (let z = 0; z < d; z++) {
      for (let x = 0; x < w; x++) {
        const start = index(x, y, z);
        if (cells[start] === EMPTY || visited[start]) continue;
        visited[start] = true;
        const stack = [[x, y, z]];
        let size = 0;
        while (stack.length > 0) {
          const [cx, cy, cz] = stack.pop();
          size += 1;
          for (const [dx, dy, dz] of FACE_STEPS) {
            const nx = cx + dx;
            const ny = cy + dy;
            const nz = cz + dz;
            if (!grid.inBounds(nx, ny, nz)) continue;
            const n = index(nx, ny, nz);
            if (cells[n] !== EMPTY && !visited[n]) {
              visited[n] = true;
              stack.push([nx, ny, nz]);
            }
          }
        }
        found.push({ size, sample: [x, y, z] });
      }
    }
  }
  found.sort((a, b) => a.size - b.size
    || a.sample[0] - b.sample[0] || a.sample[1] - b.sample[1] || a.sample[2] - b.sample[2]);
  return found;
}

/**
 * Throws when `grid`'s filled cells split into more than one 6-connected component — the pre-emit
 * one-liner a generator calls right before {@link emit}, so a disconnected model is a build break
 * here instead of a round trip through `ludion voxel from-json --validate` to discover the same
 * `L1129` the committed file would have carried.
 * @param {Grid} grid
 */
export function assertConnected(grid) {
  const parts = components(grid);
  if (parts.length <= 1) return;
  const pieces = parts
    .map((c) => `${c.size} cell${c.size === 1 ? '' : 's'} at (${c.sample.join(', ')})`)
    .join('; ');
  throw new Error(`grid has ${parts.length} disconnected components: ${pieces}`);
}

/** The five numbers a cross-section is made of, shared by `ellipse` and `lathe`. */
const PROFILE_KEYS = ['y', 'cx', 'cz', 'rx', 'rz'];

/** Rejects a cross-section that is missing a number, at the call site rather than 300 cells later. */
function checkProfile(profile, what) {
  for (const key of PROFILE_KEYS) {
    if (typeof profile?.[key] !== 'number' || !Number.isFinite(profile[key])) {
      throw new Error(`${what} profile entry is missing a finite \`${key}\`: ${JSON.stringify(profile)}`);
    }
  }
}

/** Where a {@link Grid#wedge}'s top is highest. */
const RIDGES = ['center', 'min', 'max'];

/** Rejects a box corner that is not three finite numbers, at the call site rather than mid-fill. */
function checkCorner(corner, what) {
  if (!Array.isArray(corner) || corner.length !== 3 || corner.some((n) => !Number.isFinite(n))) {
    throw new Error(`wedge \`${what}\` must be three finite numbers: ${JSON.stringify(corner)}`);
  }
}

/**
 * A {@link Grid#wedge} column's height fraction: 1 at the ridge, 0 at the slope's far edge. The
 * `center` case measures from the span's midpoint, which lands on a half cell for an even span —
 * the same trick an even-width lathe uses to stay symmetric. A single-column span is all ridge.
 */
function wedgeRise(c, lo, hi, ridge) {
  const span = hi - lo;
  if (span <= 0) return 1;
  if (ridge === 'min') return (hi - c) / span;
  if (ridge === 'max') return (c - lo) / span;
  return 1 - Math.abs(c - (lo + hi) / 2) / (span / 2);
}

/**
 * The exact integer footprint of one XZ ellipse — {@link Grid#ellipseXZ}'s rasterization rule,
 * factored out so {@link ellipseCells} can answer "what cells does this fill" without a grid to
 * write into. `xMax`/`zMax` clamp the upper end (a real grid's `w - 1`/`d - 1`); the lower end is
 * always clamped at 0, a cell coordinate is never negative regardless of grid size. Undefined
 * `xMax`/`zMax` leaves the upper end unclamped — {@link ellipseCells}'s case, no grid to bound it.
 *
 * A non-positive radius yields no cells: `ceil(c - r) > floor(c + r)` once `r <= 0` (or the
 * division by a zero radius is `NaN`, which no inequality satisfies), so the empty case falls out
 * of the same formula rather than needing its own branch.
 */
function ellipseXZFootprint(cx, cz, rx, rz, xMax, zMax) {
  const xLo = Math.max(0, Math.ceil(cx - rx));
  const xHi = xMax === undefined ? Math.floor(cx + rx) : Math.min(xMax, Math.floor(cx + rx));
  const zLo = Math.max(0, Math.ceil(cz - rz));
  const zHi = zMax === undefined ? Math.floor(cz + rz) : Math.min(zMax, Math.floor(cz + rz));
  const cells = [];
  for (let z = zLo; z <= zHi; z++) {
    for (let x = xLo; x <= xHi; x++) {
      const dx = (x - cx) / rx;
      const dz = (z - cz) / rz;
      if (dx * dx + dz * dz <= 1) cells.push([x, z]);
    }
  }
  return { xLo, xHi, zLo, zHi, cells };
}

/**
 * The cross-section a {@link Grid#lathe} of `profiles` fills at layer `y` — the same interpolation
 * the sweep itself runs, exposed because a model's *details* hang off its silhouette: the arm root
 * sits at the torso's edge, the gill slit at a fraction of the neck's width, the mouth at the head's
 * front. Sampling the profile keeps those numbers derived from the one shape rather than a second
 * copy of the taper that drifts the first time the body is retuned.
 *
 * ```js
 * const TORSO = [{ y: 14, cx: 15.5, cz: 15.5, rx: 10, rz: 7 },
 *                { y: 21, cx: 15.5, cz: 15.5, rx: 8.5, rz: 6 }];
 * grid.lathe(TORSO, 'H');
 * for (let y = 14; y <= 20; y++) {
 *   const torso = profileAt(TORSO, y);
 *   grid.ellipse({ y, cx: torso.cx - torso.rx, cz: torso.cz, rx: 2, rz: 2 }, 'A'); // arm at the edge
 * }
 * ```
 *
 * A `y` outside the profile's range yields its nearest end cross-section.
 *
 * @param {{y: number, cx: number, cz: number, rx: number, rz: number}[]} profiles
 * @param {number} y
 * @returns {{y: number, cx: number, cz: number, rx: number, rz: number}} A profile entry, so the
 *   result feeds straight back into {@link Grid#ellipse}.
 */
export function profileAt(profiles, y) {
  if (profiles.length === 0) throw new Error('profileAt needs at least one profile entry');
  for (const p of profiles) checkProfile(p, 'profileAt');
  return interpolate([...profiles].sort((a, b) => a.y - b.y), y);
}

/**
 * The integer XZ footprint {@link Grid#ellipse}/{@link Grid#lathe} would rasterize at one
 * cross-section — "what cells does this lathe fill at y=18", read straight off a `profileAt`
 * result without regenerating the grid to inspect it. Uses the exact same integerization
 * `Grid#ellipseXZ` does, factored into `ellipseXZFootprint` so the two can never drift apart.
 *
 * Unbounded: with no grid to clamp against, only the lower edge is enforced (a cell coordinate is
 * never negative) — the upper edge is not clipped to any particular grid width/depth. Compare
 * against a real grid's `w - 1`/`d - 1`, or filter through `grid.inBounds`, if that matters.
 *
 * @param {{y: number, cx: number, cz: number, rx: number, rz: number}} profile A `profileAt`
 *   result (or any object with the same five fields).
 * @returns {{y: number, xRange: [number, number], zRange: [number, number],
 *   cells: [number, number][]}}
 */
export function ellipseCells(profile) {
  checkProfile(profile, 'ellipseCells');
  const { y, cx, cz, rx, rz } = profile;
  const { xLo, xHi, zLo, zHi, cells } = ellipseXZFootprint(cx, cz, rx, rz);
  return { y: Math.round(y), xRange: [xLo, xHi], zRange: [zLo, zHi], cells };
}

/**
 * The cross-section at `y` of a y-sorted profile list. The bracketing pair is the last entry at or
 * below `y` and the one after it; `t` is clamped, so sampling off either end holds that end's shape
 * rather than extrapolating a taper past where the author drew it.
 */
function interpolate(sorted, y) {
  let i = 0;
  while (i + 2 < sorted.length && sorted[i + 1].y <= y) i++;
  const a = sorted[i];
  const b = sorted[Math.min(i + 1, sorted.length - 1)];
  const span = b.y - a.y;
  const t = span === 0 ? 0 : Math.min(1, Math.max(0, (y - a.y) / span));
  return {
    y,
    cx: a.cx + (b.cx - a.cx) * t,
    cz: a.cz + (b.cz - a.cz) * t,
    rx: a.rx + (b.rx - a.rx) * t,
    rz: a.rz + (b.rz - a.rz) * t,
  };
}

/** Axis name to index, for the ops that take one. */
const AXES = { x: 0, y: 1, z: 2 };

/** The two axes that are not `a`, ascending. */
const OTHER_AXES = [[1, 2], [0, 2], [0, 1]];

/**
 * mulberry32: a 32-bit PRNG in four lines. Not cryptographic and not meant to be — it exists so
 * scattered detail is a pure function of its seed, because a generated model is committed and the
 * `voxel-drift` gate re-runs the script. `Math.random` in a generator is a build break waiting for
 * the next commit.
 * @param {number} seed
 * @returns {() => number} A generator of floats in [0, 1).
 */
export function mulberry32(seed) {
  let a = seed >>> 0;
  return function next() {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * Scatters `toGlyph` deterministically over the cells of `region` that currently hold `fromGlyph`
 * — glowing embers through a bed of coal, rust across a hull, moss on a wall.
 *
 * Candidate cells are visited in (y, z, x) order and each draws once, so the result is a pure
 * function of the grid, the region, the fraction, and the seed. Change the seed to reroll the
 * pattern; the same seed reproduces it forever, which is what the drift gate requires.
 *
 * @param {Grid} grid
 * @param {{from: [number,number,number], to: [number,number,number]}} region Inclusive box.
 * @param {string} fromGlyph Only cells holding this glyph are candidates.
 * @param {string} toGlyph
 * @param {number} fraction Chance in [0, 1] that a candidate converts.
 * @param {number} seed
 * @returns {number} Cells changed.
 */
export function speckle(grid, region, fromGlyph, toGlyph, fraction, seed) {
  const rng = mulberry32(seed);
  const { from, to } = region;
  let count = 0;
  for (let y = Math.min(from[1], to[1]); y <= Math.max(from[1], to[1]); y++) {
    for (let z = Math.min(from[2], to[2]); z <= Math.max(from[2], to[2]); z++) {
      for (let x = Math.min(from[0], to[0]); x <= Math.max(from[0], to[0]); x++) {
        if (grid.get(x, y, z) !== fromGlyph) continue;
        if (rng() < fraction) count += grid.set(x, y, z, toGlyph);
      }
    }
  }
  return count;
}

/**
 * Runs `fn` `count` times along an arithmetic march — the rhythm helper architecture is made of:
 * quoins up a corner, a corbel table along an eaves line, the six treads of a crow-stepped gable.
 * Each iteration gets its index *and* the marched coordinate, so the body places a shape relative
 * to one point instead of respelling `start + step * i` inline, which is where the off-by-ones live.
 *
 * ```js
 * repeat({ count: 6, start: [8, 118, 8], step: [8, 10, 0] }, (i, x, y, z) =>
 *   grid.box([x, y, z], [119 - 8 * i, y + 9, z + 7], 'S'));   // six crow steps, each inset 8
 * ```
 *
 * The march is unbounded on purpose — a coordinate off the grid is the ops' usual silent clip, not
 * an error here. `fn`'s numeric returns are summed, so a body made of grid ops reports cells
 * changed the way every other primitive does; a body that returns nothing reports 0.
 *
 * @param {{count: number, start?: [number,number,number], step?: [number,number,number]}} march
 * @param {(i: number, x: number, y: number, z: number) => number|void} fn
 * @returns {number} The sum of `fn`'s numeric returns.
 */
export function repeat({ count, start = [0, 0, 0], step = [0, 0, 0] } = {}, fn) {
  if (!Number.isInteger(count) || count < 0) {
    throw new Error(`repeat needs a non-negative integer \`count\`, got ${count}`);
  }
  if (typeof fn !== 'function') throw new Error('repeat needs a function to run each iteration');
  for (const [name, value] of [['start', start], ['step', step]]) {
    if (!Array.isArray(value) || value.length !== 3 || value.some((n) => !Number.isFinite(n))) {
      throw new Error(`repeat \`${name}\` must be three finite numbers: ${JSON.stringify(value)}`);
    }
  }
  let total = 0;
  for (let i = 0; i < count; i++) {
    const changed = fn(i, start[0] + step[0] * i, start[1] + step[1] * i, start[2] + step[2] * i);
    if (typeof changed === 'number') total += changed;
  }
  return total;
}

/**
 * Writes the JSON intermediate — the script's single output, and the only thing a generator ever
 * writes. `ludion voxel from-json` validates it (`L1133`) and writes the canonical `.voxel.yaml`;
 * nothing here knows what YAML looks like, by design.
 *
 * With `out` the bytes go straight to that path in UTF-8 and the summary line goes to stderr;
 * without it they go to stdout, unchanged. `out` exists because a PowerShell pipe — and `>`
 * redirection with it — re-encodes a process's stdout, which is how a double-encoded em-dash once
 * reached a committed model (`ludion explain L1135`). Writing the file here keeps every byte away
 * from a shell, and `ludion voxel from-json --in <path>` reads it back.
 *
 * Both modes have to stay available to one script: the `voxel-drift` gate spawns a generator bare
 * and reads its stdout (no shell, so nothing to re-encode there), while an author on Windows needs
 * the file. Take the path as an argument and a generator serves both, with no branch:
 *
 * ```js
 * emit({ grid, palette, out: process.argv[2] });   // `node gen.mjs x.json` writes; bare prints
 * ```
 *
 * @param {object} model
 * @param {Grid} model.grid The finished grid (supplies `size` and `layers`).
 * @param {string} [model.name] The model name.
 * @param {number} [model.scale] The intended world scale (grid height x scale = world height).
 * @param {string} [model.generatedFrom] This script's file name — declaring it makes the model
 *   write-only and enrolls it in the `voxel-drift` gate. Set it unless the output is a one-shot
 *   scaffold you intend to hand-sculpt afterwards.
 * @param {boolean} [model.budgetExempt] `true` to opt out of the L1115 cell-budget warning.
 * @param {Record<string, {color: [number,number,number,number], emissive?: number,
 *   metallic?: number, roughness?: number}>} model.palette
 * @param {string[]} [model.header] The header docstring, one entry per line.
 * @param {Record<number|string, string>} [model.layerComments] Layer index to anatomy note.
 * @param {string} [model.out] Write the intermediate to this path instead of printing it to stdout.
 */
export function emit({
  grid, name, scale, generatedFrom, budgetExempt, palette, header, layerComments, out,
}) {
  if (!(grid instanceof Grid)) throw new Error('emit() needs a `grid`');
  if (!palette) throw new Error('emit() needs a `palette`');
  const doc = { format: 1, kind: 'voxel-gen' };
  if (name !== undefined) doc.name = name;
  doc.size = [grid.w, grid.h, grid.d];
  if (scale !== undefined) doc.scale = scale;
  if (generatedFrom !== undefined) doc.generated_from = generatedFrom;
  if (budgetExempt !== undefined) doc.budget_exempt = budgetExempt;
  doc.palette = palette;
  if (header !== undefined) doc.header = header;
  if (layerComments !== undefined) {
    // JSON object keys are strings; the engine parses them back to layer indices. Sorted
    // numerically so the intermediate itself reads in layer order.
    doc.layer_comments = {};
    for (const key of Object.keys(layerComments).sort((a, b) => Number(a) - Number(b))) {
      doc.layer_comments[String(key)] = layerComments[key];
    }
  }
  doc.layers = grid.layers();
  const text = `${JSON.stringify(doc, null, 2)}\n`;
  if (out === undefined) {
    process.stdout.write(text);
    return;
  }
  writeFileAtomic(out, text, 'utf8');
  // stderr, so a script that also gets piped somewhere keeps stdout to itself.
  const [w, h, d] = doc.size;
  process.stderr.write(
    `${out}: ${w}x${h}x${d}, ${Buffer.byteLength(text)} bytes — `
    + `next: ludion voxel from-json --in ${out} --validate\n`,
  );
}
