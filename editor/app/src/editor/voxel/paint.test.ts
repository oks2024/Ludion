import { describe, expect, it } from 'vitest';
import { glyphAt, parseVoxelDoc, type VoxelDoc } from './doc';
import { setCells, setPaletteColor, spliceSource, type CellPaint, type VoxelEditResult } from './edit';
import {
  clampLayer,
  EMPTY_UNDO,
  gridBounds,
  inBounds,
  layerCount,
  onionLayer,
  opensUndoEntry,
  pivotLayers,
  recordEdit,
  redoStep,
  sliceRows,
  stepLayer,
  strokeExtend,
  strokeStart,
  undoStep,
  UNDO_LIMIT,
  type UndoStack,
  type UndoStep,
} from './paint';
import { applySpliceSteps } from './patch';

// The frozen stage library again — the grids this panel draws are the authored ones.
const MODELS = import.meta.glob('../../../fixtures/stage/scenes/voxels/*.voxel.yaml', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

function model(name: string): string {
  const key = Object.keys(MODELS).find((path) => path.endsWith(`/${name}`));
  expect(key, `${name} is missing from the frozen stage fixture (editor/app/fixtures/stage)`).toBeDefined();
  return MODELS[key as string];
}

const NAMES = Object.keys(MODELS).map((path) => path.split('/').at(-1) as string);
const BRAZIER: VoxelDoc = parseVoxelDoc(model('brazier.voxel.yaml'));

describe('layer navigation', () => {
  it('reads the layer count from what parsed, not from `size`', () => {
    expect(layerCount(BRAZIER)).toBe(16);
    expect(layerCount(parseVoxelDoc('format: 1\n'))).toBe(0);
  });

  it('clamps into [0, count)', () => {
    expect(clampLayer(-3, 16)).toBe(0);
    expect(clampLayer(0, 16)).toBe(0);
    expect(clampLayer(15, 16)).toBe(15);
    expect(clampLayer(99, 16)).toBe(15);
  });

  it('clamps to 0 for an empty document and a nonsense index', () => {
    expect(clampLayer(4, 0)).toBe(0);
    expect(clampLayer(Number.NaN, 16)).toBe(0);
  });

  it('steps without wrapping at either end', () => {
    expect(stepLayer(4, 1, 16)).toBe(5);
    expect(stepLayer(4, -1, 16)).toBe(3);
    expect(stepLayer(15, 1, 16)).toBe(15);
    expect(stepLayer(0, -1, 16)).toBe(0);
  });

  it('re-clamps a stale index when the document shrinks under it', () => {
    // Selecting a smaller model must not leave the painter pointing past the top.
    expect(stepLayer(30, 0, 16)).toBe(15);
  });
});

describe('pivotLayers', () => {
  it('a pivot inside a cell crosses one layer, a whole number crosses the two it divides', () => {
    expect(pivotLayers(3.5, 8)).toEqual([3]);
    expect(pivotLayers(3.0, 8)).toEqual([2, 3]);
  });

  it('the grid\'s own faces belong to the one layer that exists there', () => {
    expect(pivotLayers(0, 8)).toEqual([0]);
    expect(pivotLayers(8, 8)).toEqual([7]);
  });

  it('a height outside the grid draws nothing — a clamped crosshair would lie', () => {
    expect(pivotLayers(-0.5, 8)).toEqual([]);
    expect(pivotLayers(9, 8)).toEqual([]);
    expect(pivotLayers(Number.NaN, 8)).toEqual([]);
    expect(pivotLayers(2, 0)).toEqual([]);
  });
});

describe('onion-skin selection', () => {
  it('is the layer BELOW — models are authored bottom-up', () => {
    expect(onionLayer(BRAZIER, 4)).toBe(3);
    expect(onionLayer(BRAZIER, 8)).toBe(7);
  });

  it('is null at the ground layer', () => {
    expect(onionLayer(BRAZIER, 0)).toBeNull();
  });

  it('is null for a document with no layers', () => {
    expect(onionLayer(parseVoxelDoc('format: 1\n'), 0)).toBeNull();
  });

  it('clamps a stale index before choosing, so it never names a layer that is not there', () => {
    expect(onionLayer(BRAZIER, 99)).toBe(14);
  });
});

describe('the grid', () => {
  it.each(NAMES)('%s: bounds match the declared size', (name) => {
    const doc = parseVoxelDoc(model(name));
    expect(doc.size).not.toBeNull();
    const size = doc.size as readonly number[];
    expect(gridBounds(doc)).toEqual({ width: size[0], depth: size[2] });
  });

  it('falls back to the widest row when there is no size', () => {
    const doc = parseVoxelDoc(
      ['format: 1', 'slices:', '  -', '    - "..A."', '    - ".B.."', '  -', '    - "CC.."', ''].join('\n'),
    );
    expect(gridBounds(doc)).toEqual({ width: 4, depth: 2 });
  });

  it.each(NAMES)('%s: every drawn row is exactly `width` glyphs and agrees with glyphAt', (name) => {
    const doc = parseVoxelDoc(model(name));
    const bounds = gridBounds(doc);
    for (let y = 0; y < layerCount(doc); y += 1) {
      const rows = sliceRows(doc, y, bounds);
      expect(rows).toHaveLength(bounds.depth);
      // One assertion per row, not per cell: a budget-exempt HD model runs to ~10^6 cells, and
      // per-cell expect() overhead alone blows the test budget.
      for (const [z, row] of rows.entries()) {
        let expected = '';
        for (let x = 0; x < bounds.width; x += 1) {
          expected += glyphAt(doc, x, y, z);
        }
        expect(row).toBe(expected);
      }
    }
  });

  it('pads a short row rather than shrinking the panel', () => {
    const doc = parseVoxelDoc(
      ['format: 1', 'size:', '  - 4', '  - 1', '  - 2', 'slices:', '  -', '    - "AB"', ''].join('\n'),
    );
    expect(sliceRows(doc, 0, gridBounds(doc))).toEqual(['AB..', '....']);
  });

  it('knows what is on the grid', () => {
    const bounds = { width: 3, depth: 2 };
    expect(inBounds(bounds, 0, 0)).toBe(true);
    expect(inBounds(bounds, 2, 1)).toBe(true);
    expect(inBounds(bounds, 3, 0)).toBe(false);
    expect(inBounds(bounds, -1, 0)).toBe(false);
    expect(inBounds(bounds, 0, 2)).toBe(false);
  });
});

describe('a drag accumulates paints', () => {
  const bounds = { width: 9, depth: 9 };

  it('opens with the pointer-down cell', () => {
    const stroke = strokeStart(bounds, 4, 'F', 2, 3);
    expect(stroke.cells).toEqual([{ x: 2, y: 4, z: 3, glyph: 'F' }]);
  });

  it('opens empty when the pointer went down off the grid', () => {
    expect(strokeStart(bounds, 4, 'F', -1, 3).cells).toEqual([]);
  });

  it('returns the same stroke when the pointer has not left its cell', () => {
    const stroke = strokeStart(bounds, 4, 'F', 2, 3);
    expect(strokeExtend(stroke, bounds, 2, 3)).toBe(stroke);
  });

  it('fills the gap between two samples, so a fast drag paints a line', () => {
    let stroke = strokeStart(bounds, 0, 'F', 0, 0);
    stroke = strokeExtend(stroke, bounds, 4, 0);
    expect(stroke.cells.map((cell) => cell.x)).toEqual([0, 1, 2, 3, 4]);
    expect(stroke.cells.every((cell) => cell.z === 0)).toBe(true);
  });

  it('fills a diagonal gap without holes', () => {
    let stroke = strokeStart(bounds, 0, 'F', 0, 0);
    stroke = strokeExtend(stroke, bounds, 3, 3);
    expect(stroke.cells.map((cell) => [cell.x, cell.z])).toEqual([
      [0, 0],
      [1, 1],
      [2, 2],
      [3, 3],
    ]);
  });

  it('drops repeats, so re-crossing painted cells does not grow the batch', () => {
    let stroke = strokeStart(bounds, 0, 'F', 0, 0);
    stroke = strokeExtend(stroke, bounds, 3, 0);
    stroke = strokeExtend(stroke, bounds, 0, 0);
    expect(stroke.cells).toHaveLength(4);
  });

  it('clips to the grid — setCells is all-or-nothing, so an off-grid cell may never enter', () => {
    let stroke = strokeStart(bounds, 0, 'F', 8, 0);
    stroke = strokeExtend(stroke, bounds, 12, 0);
    stroke = strokeExtend(stroke, bounds, 8, 0);
    expect(stroke.cells).toEqual([{ x: 8, y: 0, z: 0, glyph: 'F' }]);
    // The anchor still tracks the pointer off-grid, so re-entering resumes from where it left.
    expect(stroke.x).toBe(8);
  });

  it('a whole stroke applies against ONE document, changing exactly one character per cell', () => {
    const bounds20 = gridBounds(BRAZIER);
    let stroke = strokeStart(bounds20, 0, '.', 0, 0);
    for (let x = 1; x < bounds20.width; x += 1) {
      stroke = strokeExtend(stroke, bounds20, x, 0);
    }
    const changed = stroke.cells.filter((cell) => glyphAt(BRAZIER, cell.x, cell.y, cell.z) !== '.').length;
    const result = setCells(BRAZIER, stroke.cells);
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.edit.source).toHaveLength(BRAZIER.source.length);
      expect(result.edit.splices).toHaveLength(changed);
    }
  });
});

// --- History ------------------------------------------------------------------------------------ //
//
// The stack is exercised through a miniature of `useVoxelEdit`'s edit loop, because the semantics
// that matter (a gesture is one step, a commit ends it, an undo clears the key) live in the loop,
// not in any one function. `reference` is the encoding this stack replaced — whole source strings —
// so the equivalence test below is the real safety net: the new stack must be indistinguishable.

type Edit = (doc: VoxelDoc) => VoxelEditResult;

interface Session {
  readonly source: () => string;
  readonly apply: (make: Edit, coalesce: string | null, commit: boolean) => void;
  readonly undo: () => void;
  readonly redo: () => void;
  readonly canUndo: () => boolean;
  readonly canRedo: () => boolean;
}

/** The stack under test, driven exactly as the hook drives it. */
function spliceSession(start: string): Session {
  let doc = parseVoxelDoc(start);
  let stack: UndoStack = EMPTY_UNDO;
  let key: string | null = null;
  return {
    source: () => doc.source,
    apply(make, coalesce, commit) {
      const result = make(doc);
      expect(result.ok).toBe(true);
      if (result.ok && result.edit.splices.length > 0) {
        stack = recordEdit(stack, result.edit.splices, opensUndoEntry(key, coalesce));
        key = coalesce;
        doc = applySpliceSteps(doc, [result.edit.splices]);
      }
      if (commit) {
        key = null;
      }
    },
    undo() {
      const stepped = undoStep(stack);
      if (stepped === null) {
        return;
      }
      stack = stepped.stack;
      doc = applySpliceSteps(doc, stepped.steps);
      key = null;
    },
    redo() {
      const stepped = redoStep(stack);
      if (stepped === null) {
        return;
      }
      stack = stepped.stack;
      doc = applySpliceSteps(doc, stepped.steps);
      key = null;
    },
    canUndo: () => stack.past.length > 0,
    canRedo: () => stack.future.length > 0,
  };
}

/** The whole-source stack, kept here as the oracle it is: `past` holds the file before each gesture. */
function referenceSession(start: string): Session {
  let source = start;
  let past: string[] = [];
  let future: string[] = [];
  let key: string | null = null;
  return {
    source: () => source,
    apply(make, coalesce, commit) {
      const result = make(parseVoxelDoc(source));
      expect(result.ok).toBe(true);
      if (result.ok && result.edit.splices.length > 0) {
        if (opensUndoEntry(key, coalesce)) {
          past = [...past, source].slice(-UNDO_LIMIT);
          future = [];
        }
        key = coalesce;
        source = result.edit.source;
      }
      if (commit) {
        key = null;
      }
    },
    undo() {
      const previous = past.at(-1);
      if (previous === undefined) {
        return;
      }
      future = [...future, source];
      past = past.slice(0, -1);
      source = previous;
      key = null;
    },
    redo() {
      const next = future.at(-1);
      if (next === undefined) {
        return;
      }
      past = [...past, source];
      future = future.slice(0, -1);
      source = next;
      key = null;
    },
    canUndo: () => past.length > 0,
    canRedo: () => future.length > 0,
  };
}

/** One cell of a stroke, as `SlicePainterPanel` sends it: the whole accumulated batch every time. */
function batch(cells: readonly CellPaint[]): Edit {
  return (doc) => setCells(doc, cells);
}

describe('the undo stack', () => {
  it('starts empty', () => {
    expect(undoStep(EMPTY_UNDO)).toBeNull();
    expect(redoStep(EMPTY_UNDO)).toBeNull();
  });

  it('caps the history, dropping the oldest entry', () => {
    let stack = EMPTY_UNDO;
    for (let index = 0; index < UNDO_LIMIT + 5; index += 1) {
      stack = recordEdit(stack, [{ offset: index, removed: 'a', inserted: 'b' }], true);
    }
    expect(stack.past).toHaveLength(UNDO_LIMIT);
    expect(stack.past[0][0][0].offset).toBe(5);
  });

  it('never mutates the stack it was given', () => {
    const stack = recordEdit(EMPTY_UNDO, [{ offset: 0, removed: 'a', inserted: 'b' }], true);
    undoStep(stack);
    recordEdit(stack, [{ offset: 1, removed: 'c', inserted: 'd' }], false);
    expect(stack.past).toHaveLength(1);
    expect(stack.past[0]).toHaveLength(1);
    expect(EMPTY_UNDO).toEqual({ past: [], future: [] });
  });

  it('folds a gesture into one entry and separates two', () => {
    const first = recordEdit(EMPTY_UNDO, [{ offset: 0, removed: 'a', inserted: 'b' }], true);
    const folded = recordEdit(first, [{ offset: 1, removed: 'c', inserted: 'd' }], false);
    expect(folded.past).toHaveLength(1);
    expect(folded.past[0]).toHaveLength(2);
    const second = recordEdit(folded, [{ offset: 2, removed: 'e', inserted: 'f' }], true);
    expect(second.past).toHaveLength(2);
  });

  it('collapses one gesture into one entry, and separates two', () => {
    // A stroke: pointer-down opens the entry, every move inside it does not.
    expect(opensUndoEntry(null, 'stroke-1')).toBe(true);
    expect(opensUndoEntry('stroke-1', 'stroke-1')).toBe(false);
    // The next stroke, and a different control mid-gesture, both open their own.
    expect(opensUndoEntry('stroke-1', 'stroke-2')).toBe(true);
    expect(opensUndoEntry('color:F', 'emissive:F')).toBe(true);
    // A commit clears the key, so dragging the SAME control again is a second step back.
    expect(opensUndoEntry(null, 'color:F')).toBe(true);
    // An unkeyed edit is always its own entry.
    expect(opensUndoEntry('stroke-1', null)).toBe(true);
    expect(opensUndoEntry(null, null)).toBe(true);
  });

  it('an entry restores the file byte-exactly without holding it', () => {
    const painted = setCells(BRAZIER, [{ x: 0, y: 0, z: 0, glyph: 'C' }]);
    expect(painted.ok).toBe(true);
    if (!painted.ok) {
      return;
    }
    const stack = recordEdit(EMPTY_UNDO, painted.edit.splices, true);
    const back = undoStep(stack);
    expect(back).not.toBeNull();
    // The whole stack is far smaller than the file it restores — the point of storing splices.
    expect(JSON.stringify(stack).length).toBeLessThan(BRAZIER.source.length);
    let source = painted.edit.source;
    for (const step of (back as UndoStep).steps) {
      source = spliceSource(source, step);
    }
    expect(source).toBe(BRAZIER.source);
  });

  it('a multi-cell stroke is ONE undo, back to the source it started from', () => {
    const bounds = gridBounds(BRAZIER);
    const session = spliceSession(BRAZIER.source);
    let stroke = strokeStart(bounds, 4, 'C', 0, 4);
    // A stroke re-applies its whole accumulated batch on every pointer move, under one key.
    session.apply(batch(stroke.cells), 'stroke-1', false);
    for (let x = 1; x < bounds.width; x += 1) {
      stroke = strokeExtend(stroke, bounds, x, 4);
      session.apply(batch(stroke.cells), 'stroke-1', x === bounds.width - 1);
    }
    const painted = session.source();
    expect(painted).not.toBe(BRAZIER.source);
    expect(painted).toHaveLength(BRAZIER.source.length);
    expect(parseVoxelDoc(painted).layers[4].rows[4].text).toBe('C'.repeat(bounds.width));

    session.undo();
    expect(session.source()).toBe(BRAZIER.source);
    expect(session.canUndo()).toBe(false);
    session.redo();
    expect(session.source()).toBe(painted);
  });

  it('matches the whole-source stack it replaced, over interleaved gestures', () => {
    const sessions = [spliceSession(BRAZIER.source), referenceSession(BRAZIER.source)];
    const run = (step: (session: Session) => void, label: string): void => {
      for (const session of sessions) {
        step(session);
      }
      expect(sessions[0].source(), label).toBe(sessions[1].source());
      expect(sessions[0].canUndo(), label).toBe(sessions[1].canUndo());
      expect(sessions[0].canRedo(), label).toBe(sessions[1].canRedo());
    };
    const stroke = (key: string, y: number, z: number, glyph: string): void => {
      const cells: CellPaint[] = [];
      for (let x = 0; x < 20; x += 1) {
        cells.push({ x, y, z, glyph });
        const last = x === 19;
        run((s) => s.apply(batch([...cells]), key, last), `${key} x=${x}`);
      }
    };

    stroke('stroke-1', 2, 3, 'C');
    stroke('stroke-2', 2, 3, '.');
    run((s) => s.undo(), 'undo the erase');
    run((s) => s.undo(), 'undo the paint');
    run((s) => s.undo(), 'undo past the bottom');
    run((s) => s.redo(), 'redo the paint');
    // A colour drag: offset-moving splices under one key, so the entry cannot compose to one list.
    for (const value of [0.3, 0.42, 0.5]) {
      run((s) => s.apply((doc) => setPaletteColor(doc, 'C', [value, null, null, null]), 'color:C', false), `color ${value}`);
    }
    run((s) => s.apply((doc) => setPaletteColor(doc, 'C', [0.6, null, null, null]), 'color:C', true), 'color commit');
    run((s) => s.undo(), 'undo the colour drag');
    run((s) => s.redo(), 'redo the colour drag');
    // A new edit after an undo must drop the redo branch in both.
    run((s) => s.undo(), 'undo again');
    run((s) => s.apply(batch([{ x: 4, y: 0, z: 0, glyph: 'C' }]), null, true), 'unkeyed edit');
    run((s) => s.redo(), 'nothing to redo');
    run((s) => s.undo(), 'undo the unkeyed edit');
  });
});
