import { describe, expect, it } from 'vitest';
import { parseVoxelDoc, type VoxelDoc } from './doc';
import { invertSplices, setCells, setPaletteColor, spliceSource, type CellPaint, type Splice } from './edit';
import { gridBounds, layerCount } from './paint';
import { applySpliceSteps, composeSplices, patchVoxelDoc } from './patch';

// A frozen copy of the stage library (`editor/app/fixtures/stage`), not hand-built grids: the fast
// path's whole claim is that it agrees with `parseVoxelDoc` on the files people actually paint.
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

/**
 * Asserts the two documents are the same document.
 *
 * One assertion per LAYER rather than per row: a budget-exempt model carries ~21k rows, and
 * per-row `expect()` overhead alone blows the test budget. `toEqual` on the row array compares
 * every field — text (byte for byte), span, `quoted`, `paintable`.
 */
function expectSameDoc(patched: VoxelDoc, reparsed: VoxelDoc): void {
  expect(patched.source).toBe(reparsed.source);
  expect(patched.name).toBe(reparsed.name);
  expect(patched.size).toEqual(reparsed.size);
  expect(patched.scale).toEqual(reparsed.scale);
  expect(patched.palette).toEqual(reparsed.palette);
  expect(patched.layers).toHaveLength(reparsed.layers.length);
  for (const [y, layer] of patched.layers.entries()) {
    expect(layer.y).toBe(reparsed.layers[y].y);
    expect(layer.rows).toEqual(reparsed.layers[y].rows);
  }
}

/** Cells scattered over several layers and rows — a stroke's shape, not one cell. */
function scatter(doc: VoxelDoc, glyph: string, seed: number): CellPaint[] {
  const bounds = gridBounds(doc);
  const layers = layerCount(doc);
  const out: CellPaint[] = [];
  for (let n = 0; n < 8; n += 1) {
    const y = (n * 3 + seed) % layers;
    const z = (n * 5 + seed) % bounds.depth;
    for (let x = seed % 2; x < Math.min(bounds.width, 7); x += 2) {
      out.push({ x, y, z, glyph });
    }
  }
  return out;
}

/** Paints `cells` and asserts the patched document is what a full re-read would have produced. */
function paintAndCheck(doc: VoxelDoc, cells: readonly CellPaint[]): VoxelDoc {
  const result = setCells(doc, cells);
  expect(result.ok).toBe(true);
  if (!result.ok) {
    return doc;
  }
  const patched = patchVoxelDoc(doc, result.edit.splices, result.edit.source);
  expect(patched, 'a cell edit must take the fast path').not.toBeNull();
  expectSameDoc(patched as VoxelDoc, parseVoxelDoc(result.edit.source));
  return patched as VoxelDoc;
}

describe('patchVoxelDoc — it reads what a full parse would have read', () => {
  it.each(NAMES)('%s: successive strokes agree with parseVoxelDoc', (name) => {
    const doc = parseVoxelDoc(model(name));
    const glyph = doc.palette[0].glyph;
    let working = doc;
    working = paintAndCheck(working, scatter(working, glyph, 0));
    working = paintAndCheck(working, scatter(working, '.', 1));
    // Re-painting cells the previous stroke already set: the splice list shrinks, the doc must not.
    working = paintAndCheck(working, [...scatter(working, glyph, 0), ...scatter(working, glyph, 2)]);
    expect(working.source).not.toBe(doc.source);
    expect(working.source).toHaveLength(doc.source.length);
  });

  it('keeps the identity of everything it did not touch', () => {
    const result = setCells(BRAZIER, [{ x: 0, y: 3, z: 2, glyph: 'C' }]);
    expect(result.ok).toBe(true);
    if (!result.ok) {
      return;
    }
    const patched = patchVoxelDoc(BRAZIER, result.edit.splices, result.edit.source) as VoxelDoc;
    expect(patched).not.toBe(BRAZIER);
    expect(patched.layers).not.toBe(BRAZIER.layers);
    // The palette and the header are untouched objects, so a memo keyed on them must not fire.
    expect(patched.palette).toBe(BRAZIER.palette);
    expect(patched.size).toBe(BRAZIER.size);
    expect(patched.scale).toBe(BRAZIER.scale);
    // Only layer 3 moved, and only its row 2.
    for (const [y, layer] of patched.layers.entries()) {
      if (y === 3) {
        expect(layer).not.toBe(BRAZIER.layers[y]);
        expect(layer.rows).not.toBe(BRAZIER.layers[y].rows);
        continue;
      }
      expect(layer).toBe(BRAZIER.layers[y]);
    }
    for (const [z, row] of patched.layers[3].rows.entries()) {
      if (z === 2) {
        expect(row).not.toBe(BRAZIER.layers[3].rows[z]);
        expect(row.text).not.toBe(BRAZIER.layers[3].rows[z].text);
        continue;
      }
      expect(row).toBe(BRAZIER.layers[3].rows[z]);
    }
  });

  it('never mutates the document it was given', () => {
    const before = BRAZIER.layers[0].rows[0].text;
    const result = setCells(BRAZIER, [{ x: 4, y: 0, z: 0, glyph: 'C' }]);
    expect(result.ok).toBe(true);
    if (!result.ok) {
      return;
    }
    patchVoxelDoc(BRAZIER, result.edit.splices, result.edit.source);
    expect(BRAZIER.layers[0].rows[0].text).toBe(before);
    expect(BRAZIER.source[result.edit.splices[0].offset]).toBe(result.edit.splices[0].removed);
  });

  it('is a no-op for an empty splice list', () => {
    expect(patchVoxelDoc(BRAZIER, [], BRAZIER.source)).toBe(BRAZIER);
  });
});

describe('patchVoxelDoc — what it refuses (the caller re-reads instead)', () => {
  /** The splices of an edit that is expected to succeed. */
  function splices(result: ReturnType<typeof setPaletteColor>): readonly Splice[] {
    expect(result.ok).toBe(true);
    return result.ok ? result.edit.splices : [];
  }

  it('refuses a splice that changes length — every later offset would be stale', () => {
    const edit = setPaletteColor(BRAZIER, 'C', [0.5, null, null, null]);
    expect(edit.ok).toBe(true);
    if (!edit.ok) {
      return;
    }
    expect(edit.edit.source).not.toHaveLength(BRAZIER.source.length);
    expect(patchVoxelDoc(BRAZIER, edit.edit.splices, edit.edit.source)).toBeNull();
  });

  it('refuses a length-preserving splice outside a slice row — a palette number is not a row', () => {
    // `0.09` → `0.99` is the same width, so only the SITE tells the two edits apart.
    const edit = setPaletteColor(BRAZIER, 'C', [0.99, null, null, null]);
    const list = splices(edit);
    expect(list).toHaveLength(1);
    expect(list[0].removed).toHaveLength(list[0].inserted.length);
    expect(patchVoxelDoc(BRAZIER, list, spliceSource(BRAZIER.source, list))).toBeNull();
  });

  it('refuses a glyph that would re-tokenize the row', () => {
    const offset = BRAZIER.layers[0].rows[0].span.offset;
    for (const glyph of ['"', "'", '\\']) {
      const splice = [{ offset, removed: BRAZIER.source[offset], inserted: glyph }];
      expect(patchVoxelDoc(BRAZIER, splice, spliceSource(BRAZIER.source, splice)), glyph).toBeNull();
    }
  });

  it('refuses a splice whose recorded text is not what is there', () => {
    const offset = BRAZIER.layers[0].rows[0].span.offset;
    const splice = [{ offset, removed: 'Z', inserted: 'C' }];
    expect(patchVoxelDoc(BRAZIER, splice, spliceSource(BRAZIER.source, splice))).toBeNull();
  });

  it('refuses a splice running past the end of its row', () => {
    const row = BRAZIER.layers[0].rows[0];
    const offset = row.span.offset + row.span.length - 1;
    const splice = [{ offset, removed: BRAZIER.source.slice(offset, offset + 3), inserted: 'CCC' }];
    expect(patchVoxelDoc(BRAZIER, splice, spliceSource(BRAZIER.source, splice))).toBeNull();
  });

  it('refuses a row that carries an escape — its characters do not map onto the source', () => {
    const doc = parseVoxelDoc('format: 1\nkind: voxel\nslices:\n  -\n    - "A\\\\B"\n');
    expect(doc.layers[0].rows[0].paintable).toBe(false);
    const offset = doc.layers[0].rows[0].span.offset;
    const splice = [{ offset, removed: 'A', inserted: 'C' }];
    expect(patchVoxelDoc(doc, splice, spliceSource(doc.source, splice))).toBeNull();
  });
});

describe('composeSplices', () => {
  it('takes the first `removed` and the last `inserted` at each offset', () => {
    const composed = composeSplices([
      [{ offset: 4, removed: 'a', inserted: 'b' }],
      [{ offset: 4, removed: 'b', inserted: 'c' }],
    ]);
    expect(composed).toEqual([{ offset: 4, removed: 'a', inserted: 'c' }]);
  });

  it('drops a character written back to what it was', () => {
    expect(
      composeSplices([
        [{ offset: 4, removed: 'a', inserted: 'b' }],
        [{ offset: 4, removed: 'b', inserted: 'a' }],
      ]),
    ).toEqual([]);
  });

  it('joins adjacent offsets into one splice, and sorts', () => {
    expect(
      composeSplices([
        [{ offset: 6, removed: 'c', inserted: 'z' }],
        [{ offset: 4, removed: 'a', inserted: 'x' }],
        [{ offset: 5, removed: 'b', inserted: 'y' }],
      ]),
    ).toEqual([{ offset: 4, removed: 'abc', inserted: 'xyz' }]);
  });

  it('refuses when a splice changes length — offsets stop being comparable', () => {
    expect(composeSplices([[{ offset: 4, removed: '0.15', inserted: '0.2' }]])).toBeNull();
  });
});

describe('applySpliceSteps', () => {
  it('applies a gesture forward and its inverse back, byte for byte', () => {
    const first = setCells(BRAZIER, [{ x: 0, y: 1, z: 1, glyph: 'C' }]);
    expect(first.ok).toBe(true);
    if (!first.ok) {
      return;
    }
    const middle = applySpliceSteps(BRAZIER, [first.edit.splices]);
    const second = setCells(middle, [{ x: 1, y: 1, z: 1, glyph: 'E' }]);
    expect(second.ok).toBe(true);
    if (!second.ok) {
      return;
    }
    const end = applySpliceSteps(middle, [second.edit.splices]);
    expect(end.source).toBe(second.edit.source);

    const back = applySpliceSteps(end, [
      invertSplices(second.edit.splices),
      invertSplices(first.edit.splices),
    ]);
    expect(back.source).toBe(BRAZIER.source);
    expectSameDoc(back, parseVoxelDoc(BRAZIER.source));
  });

  it('falls back to a full read when a step moved offsets, and still lands on the right text', () => {
    const colour = setPaletteColor(BRAZIER, 'C', [0.5, null, null, null]);
    expect(colour.ok).toBe(true);
    if (!colour.ok) {
      return;
    }
    const shifted = applySpliceSteps(BRAZIER, [colour.edit.splices]);
    expect(shifted.source).toBe(colour.edit.source);
    expectSameDoc(shifted, parseVoxelDoc(colour.edit.source));
    // The rows all moved, so the fast path could not have produced this document.
    expect(shifted.layers[0].rows[0]).not.toBe(BRAZIER.layers[0].rows[0]);

    const back = applySpliceSteps(shifted, [invertSplices(colour.edit.splices)]);
    expect(back.source).toBe(BRAZIER.source);
  });

  it('is a no-op for no steps', () => {
    expect(applySpliceSteps(BRAZIER, [])).toBe(BRAZIER);
  });
});
