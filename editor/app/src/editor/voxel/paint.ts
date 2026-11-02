// The slice painter's pure half: the x×z character grid one layer of a model is, how a drag turns
// into a batch of cell paints, and the local undo stack.
//
// A SLICE IS LITERALLY A GRID OF CHARACTERS. `slices[y][z]` is one quoted fixed-width row and
// character `c` of that row is `x = c`, so the painter draws the file rather than a projection of
// it — rows run down the panel in FILE order (z = 0 first, which the format's own convention makes
// the front face, +Z). That is what makes the surgical edit obvious: the cell under the pointer and
// the character in the file are the same thing.
//
// A DRAG IS ONE WRITE. `edit.ts` states that a cell edit replaces one character with one character,
// so no earlier paint moves a later paint's offset and a whole stroke applies against ONE document.
// So a stroke accumulates cells here and hits the disk once, on release (`save.ts`). Two properties
// this module is responsible for:
//
//   - CLIPPING. `setCells` is all-or-nothing: one unaddressable cell refuses the whole stroke. A
//     drag that leaves the grid must therefore drop those cells here, not discover them at write
//     time — which is why every stroke function takes the grid's bounds.
//   - GAP FILLING. Cells report a pointer entering them, so a fast drag skips cells between two
//     samples. `strokeExtend` walks the segment, so a quick diagonal paints a line rather than a
//     dotted one.
//
// UNDO IS SPLICES. An entry holds what each edit of a gesture DID — its splices — never the file it
// replaced. Whole sources are the obvious encoding and the wrong one: 64 of them on a budget-exempt
// model is ~190 MB retained to remember 64 painted cells. Splices carry the exact characters they
// replaced, so restoring is still byte-exact; see `invertSplices` in `edit.ts` for the inversion,
// and `patch.ts` for how an undo reaches the document without re-reading the file.
//
// Pure: no React, no Tauri, no DOM.

import { sliceRow, EMPTY_GLYPH, type VoxelDoc } from './doc';
import { invertSplices, type CellPaint, type Splice } from './edit';
import { UNDO_LIMIT } from '../undo';

/** The x/z extent of a layer, in cells. */
export interface GridBounds {
  readonly width: number;
  readonly depth: number;
}

// --- Layers ------------------------------------------------------------------------------------ //

/** How many layers the document actually carries (not what `size` claims — the painter draws what
 *  parsed, and a disagreement between the two is `check-scene`'s to report, not this module's). */
export function layerCount(doc: VoxelDoc): number {
  return doc.layers.length;
}

/** A layer index forced into `[0, count)`. `count` of 0 yields 0 — there is no valid layer, and the
 *  caller renders an empty grid rather than a negative index. */
export function clampLayer(y: number, count: number): number {
  if (!Number.isFinite(y) || count <= 0) {
    return 0;
  }
  return Math.min(count - 1, Math.max(0, Math.trunc(y)));
}

/** The `[◀][▶]` step: clamped at both ends, never wrapping. Wrapping would make "up from the top"
 *  land at the ground plane, which reads as a jump rather than a limit. */
export function stepLayer(y: number, delta: number, count: number): number {
  return clampLayer(clampLayer(y, count) + Math.trunc(delta), count);
}

/**
 * The layer drawn as onion-skin under the current one: the one BELOW, or `null` at the ground.
 *
 * Below and not above, because a voxel model is authored bottom-up — every vigil model's header
 * says so — and the layer you are placing sits on the one you just finished.
 */
export function onionLayer(doc: VoxelDoc, y: number): number | null {
  const below = clampLayer(y, layerCount(doc)) - 1;
  return below >= 0 && below < layerCount(doc) ? below : null;
}

/**
 * Which slice layers a continuous height crosses — the layers a part's pivot crosshair is drawn on.
 *
 * A pivot is float voxel coordinates, so a `.5` sits inside one cell while a whole number sits on the
 * BOUNDARY between two and belongs to both: cell `y` spans `[y, y+1]`, and `3.0` is the top face of
 * layer 2 and the bottom face of layer 3 at once. Showing it on only one of them would hide the joint
 * from whichever slice the author happens to be looking at.
 *
 * A height outside the grid yields no layer at all; a crosshair at a clamped position would claim the
 * pivot is somewhere it is not.
 */
export function pivotLayers(height: number, count: number): readonly number[] {
  if (!Number.isFinite(height) || count <= 0) {
    return [];
  }
  const out: number[] = [];
  for (let y = Math.max(0, Math.ceil(height) - 1); y <= Math.min(count - 1, Math.floor(height)); y += 1) {
    out.push(y);
  }
  return out;
}

// --- The grid ---------------------------------------------------------------------------------- //

/**
 * The grid's extent: the declared `size` when the document has one, else the widest row and deepest
 * layer that parsed.
 *
 * `size` wins so a model with a short row (the `L1114` the status bar is already showing) still
 * paints on the grid it declares, and the missing cells refuse individually instead of shrinking the
 * whole panel.
 */
export function gridBounds(doc: VoxelDoc): GridBounds {
  if (doc.size !== null && doc.size[0] > 0 && doc.size[2] > 0) {
    return { width: Math.trunc(doc.size[0]), depth: Math.trunc(doc.size[2]) };
  }
  let width = 0;
  let depth = 0;
  for (const layer of doc.layers) {
    depth = Math.max(depth, layer.rows.length);
    for (const row of layer.rows) {
      width = Math.max(width, row.text.length);
    }
  }
  return { width, depth };
}

/** One layer as `depth` strings of exactly `width` glyphs, missing cells reading as empty — the same
 *  lenient read `glyphAt` does, so a half-typed file still draws. */
export function sliceRows(doc: VoxelDoc, y: number, bounds: GridBounds): readonly string[] {
  const rows: string[] = [];
  for (let z = 0; z < bounds.depth; z += 1) {
    const text = sliceRow(doc, y, z)?.text ?? '';
    rows.push(text.slice(0, bounds.width).padEnd(bounds.width, EMPTY_GLYPH));
  }
  return rows;
}

/** True when `(x, z)` is a cell of the grid. */
export function inBounds(bounds: GridBounds, x: number, z: number): boolean {
  return x >= 0 && x < bounds.width && z >= 0 && z < bounds.depth;
}

// --- Strokes ----------------------------------------------------------------------------------- //

/** A drag in progress: the glyph being painted and every in-bounds cell it has covered. */
export interface Stroke {
  readonly y: number;
  readonly glyph: string;
  /** The last cell the pointer reported, in-bounds or not — the anchor the next segment starts at. */
  readonly x: number;
  readonly z: number;
  /** Every in-bounds cell covered so far, in visit order. Repeats are dropped, so re-crossing a
   *  painted cell does not grow the batch. */
  readonly cells: readonly CellPaint[];
}

/** Opens a stroke at the pointer-down cell. Out-of-bounds starts are legal (a drag can begin in the
 *  panel's padding and enter the grid); they simply contribute no cell. */
export function strokeStart(bounds: GridBounds, y: number, glyph: string, x: number, z: number): Stroke {
  const cells = inBounds(bounds, x, z) ? [{ x, y, z, glyph }] : [];
  return { y, glyph, x, z, cells };
}

/**
 * Extends a stroke to `(x, z)`, painting every cell on the segment from the last sample.
 *
 * Returns the same object when the pointer has not left its cell, so a stationary pointer costs
 * nothing and the caller's `===` check can skip a re-render.
 */
export function strokeExtend(stroke: Stroke, bounds: GridBounds, x: number, z: number): Stroke {
  if (x === stroke.x && z === stroke.z) {
    return stroke;
  }
  const seen = new Set(stroke.cells.map((cell) => cell.x * 65536 + cell.z));
  const cells = [...stroke.cells];
  for (const [cx, cz] of segment(stroke.x, stroke.z, x, z)) {
    if (!inBounds(bounds, cx, cz) || seen.has(cx * 65536 + cz)) {
      continue;
    }
    seen.add(cx * 65536 + cz);
    cells.push({ x: cx, y: stroke.y, z: cz, glyph: stroke.glyph });
  }
  return { ...stroke, x, z, cells };
}

/** The cells of the segment `(x0, z0)` → `(x1, z1)`, excluding the start (it is already recorded)
 *  and including the end. Integer Bresenham — no floating point, no gaps. */
function segment(x0: number, z0: number, x1: number, z1: number): readonly (readonly [number, number])[] {
  const out: [number, number][] = [];
  const dx = Math.abs(x1 - x0);
  const dz = Math.abs(z1 - z0);
  const sx = x0 < x1 ? 1 : -1;
  const sz = z0 < z1 ? 1 : -1;
  let error = dx - dz;
  let x = x0;
  let z = z0;
  while (x !== x1 || z !== z1) {
    const doubled = error * 2;
    if (doubled > -dz) {
      error -= dz;
      x += sx;
    }
    if (doubled < dx) {
      error += dx;
      z += sz;
    }
    out.push([x, z]);
  }
  return out;
}

// --- Undo -------------------------------------------------------------------------------------- //

/**
 * ONE GESTURE: the forward splices of every edit made under one coalesce key, oldest first.
 *
 * THE INVARIANT the whole stack rests on: an entry is the COMPLETE list of edits made since its
 * gesture opened. So applying `invertSplices` to each step in reverse order restores the source
 * from before the gesture — one stroke is one step back however many cells it painted, and the text
 * it lands on is byte-identical to the one it started from, because each splice records the exact
 * characters it replaced. Redo needs no separate encoding: it replays the same steps forward.
 */
export type UndoEntry = readonly (readonly Splice[])[];

/** Gestures past and future. See the module doc — an entry is splices, never a file. */
export interface UndoStack {
  /** Gestures already applied, oldest first. */
  readonly past: readonly UndoEntry[];
  /** Gestures undone away, most recently undone LAST (so `redo` pops the end of both lists). */
  readonly future: readonly UndoEntry[];
}

export const EMPTY_UNDO: UndoStack = { past: [], future: [] };

/**
 * Records what an edit did. `opens` (from `opensUndoEntry`) starts a new entry; otherwise the edit
 * folds into the open one, extending the gesture it belongs to.
 *
 * A new edit clears the redo branch either way — the standard linear-history rule, and the only one
 * that cannot present a future that never existed.
 */
export function recordEdit(
  stack: UndoStack,
  splices: readonly Splice[],
  opens: boolean,
  limit: number = UNDO_LIMIT,
): UndoStack {
  const open = stack.past.at(-1);
  if (!opens && open !== undefined) {
    return { past: [...stack.past.slice(0, -1), [...open, splices]], future: [] };
  }
  const past = [...stack.past, [splices]];
  return { past: past.slice(Math.max(0, past.length - limit)), future: [] };
}

/**
 * Whether an edit opens a NEW history entry, given the key of the edit before it.
 *
 * A gesture is one step back, not sixty: every edit of one stroke (or one colour drag) carries the
 * same key and folds into the entry the first of them opened. An unkeyed edit always opens its own,
 * and the caller clears the key when a gesture commits, so two consecutive drags of the same control
 * are two steps rather than one.
 */
export function opensUndoEntry(previousKey: string | null, key: string | null): boolean {
  return key === null || key !== previousKey;
}

/** The result of stepping the history: the new stack, and the splice lists to apply IN ORDER to the
 *  current source (see `applySpliceSteps`). */
export interface UndoStep {
  readonly stack: UndoStack;
  readonly steps: readonly (readonly Splice[])[];
}

/** Steps back over one gesture, moving it onto the redo branch. `null` when there is nothing to
 *  undo. */
export function undoStep(stack: UndoStack): UndoStep | null {
  const entry = stack.past.at(-1);
  if (entry === undefined) {
    return null;
  }
  return {
    stack: { past: stack.past.slice(0, -1), future: [...stack.future, entry] },
    steps: [...entry].reverse().map(invertSplices),
  };
}

/** Steps forward again, replaying the gesture as it was made. `null` when there is nothing to
 *  redo. */
export function redoStep(stack: UndoStack): UndoStep | null {
  const entry = stack.future.at(-1);
  if (entry === undefined) {
    return null;
  }
  return {
    stack: { past: [...stack.past, entry], future: stack.future.slice(0, -1) },
    steps: entry,
  };
}
