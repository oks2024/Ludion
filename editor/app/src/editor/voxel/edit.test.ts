import { describe, expect, it } from 'vitest';
import { glyphAt, paletteEntry, parseVoxelDoc, sliceRow, type VoxelDoc } from './doc';
import {
  addGlyph,
  formatVoxelFloat,
  setCell,
  setCells,
  setPaletteColor,
  setPaletteMaterial,
  type VoxelEdit,
  type VoxelEditResult,
} from './edit';

// Tested against a frozen copy of real authored models, like `doc.test.ts` — the whole point of the
// module is that hand-authored files survive an edit, and a synthetic fixture has none of what has
// to survive: the header docstring, the comments between layers, a palette order
// that is NOT sorted (torch-post), the mixed optional material scalars.
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
const BRAZIER = model('brazier.voxel.yaml');

/** Unwraps a successful edit, failing with the refusal's own message when it was refused. */
function edit(result: VoxelEditResult): VoxelEdit {
  if (!result.ok) {
    throw new Error(`expected an edit, got ${result.error.reason}: ${result.error.message}`);
  }
  return result.edit;
}

/** Every index at which two strings differ. Lengths must match — the caller asserts that first. */
function diffIndices(before: string, after: string): number[] {
  const out: number[] = [];
  for (let index = 0; index < Math.max(before.length, after.length); index += 1) {
    if (before[index] !== after[index]) {
      out.push(index);
    }
  }
  return out;
}

/** A cell in `doc` that can be painted, with a glyph different from the one already there. */
function repaint(doc: VoxelDoc): { x: number; y: number; z: number; glyph: string } {
  const target = doc.palette[0].glyph;
  const current = glyphAt(doc, 0, 0, 0);
  return { x: 0, y: 0, z: 0, glyph: current === target ? '.' : target };
}

describe('setCell — the one-character property', () => {
  it.each(NAMES)('%s: painting one cell changes exactly one character', (name) => {
    const doc = parseVoxelDoc(model(name));
    const paint = repaint(doc);
    const result = edit(setCell(doc, paint));

    expect(result.source).toHaveLength(doc.source.length);
    expect(diffIndices(doc.source, result.source)).toHaveLength(1);
    expect(result.splices).toEqual([
      { offset: diffIndices(doc.source, result.source)[0], removed: glyphAt(doc, 0, 0, 0), inserted: paint.glyph },
    ]);
    // It re-parses, and the change landed exactly where it was asked for.
    const reparsed = parseVoxelDoc(result.source);
    expect(glyphAt(reparsed, paint.x, paint.y, paint.z)).toBe(paint.glyph);
    expect(reparsed.palette.map((e) => e.glyph)).toEqual(doc.palette.map((e) => e.glyph));
    expect(reparsed.size).toEqual(doc.size);
  });

  it.each(NAMES)('%s: painting a cell back yields the original file byte-identical', (name) => {
    const doc = parseVoxelDoc(model(name));
    const paint = repaint(doc);
    const before = glyphAt(doc, paint.x, paint.y, paint.z);
    const painted = edit(setCell(doc, paint)).source;
    const restored = edit(setCell(parseVoxelDoc(painted), { ...paint, glyph: before })).source;
    expect(restored).toBe(doc.source);
  });

  it('every comment, blank line and key order outside the cell survives', () => {
    const doc = parseVoxelDoc(BRAZIER);
    // y8 z9 is `......IICECEII......` — the embers. Paint the left ember out.
    const result = edit(setCell(doc, { x: 9, y: 8, z: 9, glyph: 'C' }));
    const before = doc.source.split('\n');
    const after = result.source.split('\n');
    expect(after).toHaveLength(before.length);
    for (const [index, line] of before.entries()) {
      // Exactly one line differs, and it is the row that was painted.
      if (line !== after[index]) {
        expect(line).toBe('    - "......IICECEII......"');
        expect(after[index]).toBe('    - "......IICCCEII......"');
      }
    }
    expect(after.filter((line, index) => line !== before[index])).toHaveLength(1);
    expect(result.source).toContain('# Brazier — a hollow iron fire bowl');
    expect(result.source).toContain('# bowl wall; carrying lugs to the sides');
    expect(result.source).toContain('# Intended in-game scale: 0.10');
    expect(result.source.startsWith('format: 1\nkind: voxel\nname: brazier\n')).toBe(true);
  });

  it('painting the glyph already there is a no-op, not a write', () => {
    const doc = parseVoxelDoc(BRAZIER);
    const result = edit(setCell(doc, { x: 9, y: 8, z: 9, glyph: 'E' }));
    expect(result.splices).toEqual([]);
    expect(result.source).toBe(doc.source);
  });

  it('CRLF line endings do not shift the painted character', () => {
    const doc = parseVoxelDoc(BRAZIER.replace(/\n/g, '\r\n'));
    const result = edit(setCell(doc, { x: 9, y: 8, z: 9, glyph: 'C' }));
    expect(diffIndices(doc.source, result.source)).toHaveLength(1);
    expect(result.source).toContain('\r\n');
    expect(parseVoxelDoc(result.source).layers[8].rows[9].text).toBe('......IICCCEII......');
  });
});

describe('setCells — a drag stroke in one write', () => {
  const doc = parseVoxelDoc(BRAZIER);

  it('changes exactly one character per painted cell, all in the dragged row', () => {
    const stroke = [6, 7, 8, 9, 10].map((x) => ({ x, y: 8, z: 9, glyph: '.' }));
    const result = edit(setCells(doc, stroke));
    expect(result.source).toHaveLength(doc.source.length);
    const changed = diffIndices(doc.source, result.source);
    expect(changed).toHaveLength(5);
    expect(changed.map((offset, index) => offset - changed[0] - index)).toEqual([0, 0, 0, 0, 0]);
    expect(parseVoxelDoc(result.source).layers[8].rows[9].text).toBe('...........EII......');
  });

  it('spans layers, and each splice replaces one character', () => {
    const result = edit(
      setCells(doc, [
        { x: 6, y: 8, z: 9, glyph: 'C' },
        { x: 9, y: 0, z: 4, glyph: '.' },
      ]),
    );
    expect(result.splices).toHaveLength(2);
    expect(result.splices.map((s) => s.removed.length)).toEqual([1, 1]);
    expect(result.splices[0].offset).toBeLessThan(result.splices[1].offset);
    const reparsed = parseVoxelDoc(result.source);
    expect(glyphAt(reparsed, 6, 8, 9)).toBe('C');
    expect(glyphAt(reparsed, 9, 0, 4)).toBe('.');
  });

  it('drops cells already carrying the target glyph, and resolves repeats last-write-wins', () => {
    const result = edit(
      setCells(doc, [
        { x: 9, y: 8, z: 9, glyph: 'E' }, // already E
        { x: 10, y: 8, z: 9, glyph: 'I' },
        { x: 10, y: 8, z: 9, glyph: 'E' }, // same cell again
      ]),
    );
    expect(result.splices).toHaveLength(1);
    expect(result.splices[0].inserted).toBe('E');
    expect(parseVoxelDoc(result.source).layers[8].rows[9].text).toBe('......IICEEEII......');
  });

  it('a stroke that ends where it started is a no-op', () => {
    const result = edit(
      setCells(doc, [
        { x: 10, y: 8, z: 9, glyph: 'I' },
        { x: 10, y: 8, z: 9, glyph: 'C' }, // back to the authored glyph
      ]),
    );
    expect(result.splices).toEqual([]);
    expect(result.source).toBe(doc.source);
  });

  it('one unaddressable cell refuses the whole batch — never a partial stroke', () => {
    const result = setCells(doc, [
      { x: 0, y: 0, z: 0, glyph: 'C' },
      { x: 99, y: 0, z: 0, glyph: 'C' },
    ]);
    expect(result.ok).toBe(false);
    expect(result.ok === false && result.error.reason).toBe('out-of-bounds');
  });
});

describe('setCell — it refuses rather than corrupts', () => {
  const doc = parseVoxelDoc(BRAZIER);

  it('x outside the row', () => {
    const result = setCell(doc, { x: 20, y: 8, z: 9, glyph: 'C' });
    expect(result.ok === false && result.error.reason).toBe('out-of-bounds');
    expect(result.ok === false && result.error.message).toContain('20 cells wide');
  });

  it('a layer or row the model does not have', () => {
    expect(setCell(doc, { x: 0, y: 99, z: 0, glyph: 'C' }).ok).toBe(false);
    const result = setCell(doc, { x: 0, y: 0, z: 99, glyph: 'C' });
    expect(result.ok === false && result.error.reason).toBe('out-of-bounds');
    expect(result.ok === false && result.error.message).toContain('y=0, z=99');
  });

  it('a glyph that names no palette entry', () => {
    const result = setCell(doc, { x: 0, y: 0, z: 0, glyph: 'Z' });
    expect(result.ok === false && result.error.reason).toBe('unknown-glyph');
    expect(result.ok === false && result.error.message).toContain('paint with `.` to erase');
  });

  it('a glyph that is not one printable-ASCII character', () => {
    for (const glyph of ['', 'AB', ' ', '\n', '\u00e9']) {
      const result = setCell(doc, { x: 0, y: 0, z: 0, glyph });
      expect(result.ok === false && result.error.reason).toBe('unknown-glyph');
    }
  });

  it('a row whose characters do not map 1:1 onto the source', () => {
    const escaped = parseVoxelDoc(
      'format: 1\nkind: voxel\npalette:\n  A:\n    color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\nslices:\n  -\n    - "A\\\\A"\n',
    );
    expect(sliceRow(escaped, 0, 0)?.paintable).toBe(false);
    const result = setCell(escaped, { x: 0, y: 0, z: 0, glyph: 'A' });
    expect(result.ok === false && result.error.reason).toBe('not-paintable');
    expect(result.ok === false && result.error.message).toContain('edit it by hand');
  });
});

describe('setPaletteColor', () => {
  const doc = parseVoxelDoc(BRAZIER);

  it('rewrites only the digits it must, leaving indentation and the rest of the block alone', () => {
    const result = edit(setPaletteColor(doc, 'C', [0.18, null, null, null]));
    const before = doc.source.split('\n');
    const after = result.source.split('\n');
    expect(after).toHaveLength(before.length);
    const changed = before.flatMap((line, index) => (line === after[index] ? [] : [index]));
    expect(changed).toHaveLength(1);
    expect(before[changed[0]]).toBe('      - 0.09');
    expect(after[changed[0]]).toBe('      - 0.18');
    const red = paletteEntry(doc, 'C')?.color[0];
    expect(result.splices).toEqual([
      { offset: red?.span.offset, removed: '0.09', inserted: '0.18' },
    ]);
    expect(result.source).toContain('    roughness: 0.95');
    expect(parseVoxelDoc(result.source).palette.map((e) => e.glyph)).toEqual(['C', 'E', 'I']);
  });

  it('a whole-color patch rewrites every component that moved and no other', () => {
    const result = edit(setPaletteColor(doc, 'I', [0.31, 0.21, 0.28, 1.0]));
    // Green (0.21) and alpha (1.0) are unchanged in VALUE, so their authored text is untouched.
    expect(result.splices.map((s) => `${s.removed}->${s.inserted}`)).toEqual([
      '0.22->0.31',
      '0.2->0.28',
    ]);
    const entry = paletteEntry(parseVoxelDoc(result.source), 'I');
    expect(entry?.color.map((c) => c.value)).toEqual([0.31, 0.21, 0.28, 1.0]);
  });

  it('an unchanged value never reformats the authored text — `0.20` stays `0.20`', () => {
    // torch-post is the one frozen model that spells a component with a trailing zero.
    const post = parseVoxelDoc(model('torch-post.voxel.yaml'));
    const result = edit(setPaletteColor(post, 'D', [null, 0.2, null, null]));
    expect(result.splices).toEqual([]);
    expect(result.source).toBe(post.source);
    expect(result.source).toContain('      - 0.20');
  });

  it('a shorter patch leaves the components it does not name', () => {
    const result = edit(setPaletteColor(doc, 'E', [0.7]));
    expect(result.splices).toHaveLength(1);
    expect(paletteEntry(parseVoxelDoc(result.source), 'E')?.color.map((c) => c.value)).toEqual([
      0.7, 0.44, 0.12, 1.0,
    ]);
  });

  it('refuses an unknown entry, a non-finite value, and an out-of-range component', () => {
    expect(setPaletteColor(doc, 'Z', [1]).ok === false && setPaletteColor(doc, 'Z', [1])).toMatchObject({
      error: { reason: 'no-such-entry' },
    });
    expect(setPaletteColor(doc, 'C', [Number.NaN])).toMatchObject({
      error: { reason: 'bad-number' },
    });
    expect(setPaletteColor(doc, 'C', [null, null, null, null, 1])).toMatchObject({
      error: { reason: 'no-such-field' },
    });
  });

  it('refuses when a component is unaddressable — a trailing comment must not shift indices', () => {
    // `doc.ts` deliberately does not follow a trailing `#`, so this entry reads three components.
    const commented = parseVoxelDoc(
      'format: 1\nkind: voxel\npalette:\n  A:\n    color:\n      - 1.0  # skin\n      - 0.5\n      - 0.5\n      - 1.0\nslices:\n  -\n    - "A"\n',
    );
    expect(paletteEntry(commented, 'A')?.color).toHaveLength(3);
    const result = setPaletteColor(commented, 'A', [null, 0.6, null, null]);
    expect(result.ok === false && result.error.reason).toBe('no-such-field');
    expect(result.ok === false && result.error.message).toContain('3 addressable color components');
  });
});

describe('setPaletteMaterial', () => {
  const doc = parseVoxelDoc(BRAZIER);

  it('rewrites the scalar in place', () => {
    const result = edit(setPaletteMaterial(doc, 'E', 'emissive', 4.25));
    expect(result.splices).toEqual([
      { offset: doc.source.indexOf('emissive: 4.0') + 'emissive: '.length, removed: '4.0', inserted: '4.25' },
    ]);
    expect(result.source).toContain('    emissive: 4.25');
    expect(result.source.split('\n')).toHaveLength(doc.source.split('\n').length);
    expect(paletteEntry(parseVoxelDoc(result.source), 'E')?.emissive?.value).toBe(4.25);
  });

  it('writes an integral value as a float, never as a YAML integer', () => {
    const result = edit(setPaletteMaterial(doc, 'C', 'roughness', 1));
    expect(result.source).toContain('  C:\n    color:\n      - 0.09\n      - 0.07\n      - 0.07\n      - 1.0\n    roughness: 1.0\n');
    expect(paletteEntry(parseVoxelDoc(result.source), 'C')?.roughness?.value).toBe(1);
  });

  it('an unchanged value is a no-op', () => {
    const result = edit(setPaletteMaterial(doc, 'E', 'emissive', 4.0));
    expect(result.splices).toEqual([]);
    expect(result.source).toBe(doc.source);
  });

  it('refuses a field the entry does not declare rather than inserting a line', () => {
    const result = setPaletteMaterial(doc, 'C', 'emissive', 2);
    expect(result.ok === false && result.error.reason).toBe('no-such-field');
    expect(result.ok === false && result.error.message).toContain('no addressable `emissive`');
  });

  it('refuses an unknown entry and a non-finite value', () => {
    expect(setPaletteMaterial(doc, 'Z', 'roughness', 1)).toMatchObject({
      error: { reason: 'no-such-entry' },
    });
    expect(setPaletteMaterial(doc, 'C', 'roughness', Number.POSITIVE_INFINITY)).toMatchObject({
      error: { reason: 'bad-number' },
    });
  });
});

describe('addGlyph', () => {
  /** The inserted block, checked against the reported splice: the edit must be ONE pure insertion,
   *  and cutting it back out must give the original file byte-identical. */
  function inserted(before: string, result: VoxelEdit): string {
    expect(result.splices).toHaveLength(1);
    const { offset, removed, inserted: block } = result.splices[0];
    expect(removed).toBe('');
    expect(result.source.slice(offset, offset + block.length)).toBe(block);
    expect(result.source.slice(0, offset) + result.source.slice(offset + block.length)).toBe(before);
    return block;
  }

  it('a SORTED palette stays sorted, and nothing else in the file moves', () => {
    const doc = parseVoxelDoc(BRAZIER); // C E I
    const result = edit(
      addGlyph(doc, { glyph: 'G', color: [0.5, 0.4, 0.3, 1], roughness: 0.6 }),
    );
    expect(inserted(doc.source, result)).toBe(
      '  G:\n    color:\n      - 0.5\n      - 0.4\n      - 0.3\n      - 1.0\n    roughness: 0.6\n',
    );
    const reparsed = parseVoxelDoc(result.source);
    expect(reparsed.palette.map((e) => e.glyph)).toEqual(['C', 'E', 'G', 'I']);
    expect(reparsed.layers).toHaveLength(16);
    expect(result.source).toContain('# open air above the fire');
    expect(result.source).toContain('# Brazier — a hollow iron fire bowl');
  });

  it('lands before the FIRST entry when the glyph sorts first', () => {
    const doc = parseVoxelDoc(BRAZIER);
    const result = edit(addGlyph(doc, { glyph: 'A', color: [0, 0, 0, 1] }));
    expect(result.source).toContain(
      'palette:\n  A:\n    color:\n      - 0.0\n      - 0.0\n      - 0.0\n      - 1.0\n  C:\n',
    );
    expect(parseVoxelDoc(result.source).palette.map((e) => e.glyph)).toEqual([
      'A',
      'C',
      'E',
      'I',
    ]);
  });

  it('appends when the glyph sorts last', () => {
    const doc = parseVoxelDoc(BRAZIER);
    const result = edit(addGlyph(doc, { glyph: 'z', color: [1, 1, 1, 1] }));
    expect(result.source).toContain('    roughness: 0.45\n  z:\n    color:\n');
    expect(result.source).toContain('      - 1.0\nslices:\n');
    expect(parseVoxelDoc(result.source).palette.map((e) => e.glyph)).toEqual([
      'C',
      'E',
      'I',
      'z',
    ]);
  });

  // This case needs a real model whose palette is genuinely NOT alphabetical, so it pins the
  // authored-order behaviour against live game content rather than a synthetic fixture. That couples
  // it to one specific model: it named house-a until house-a was re-authored by a generator that
  // emits a sorted palette. The frozen stage copy holds torch-post as it stands; if a fixture swap
  // breaks this, repoint it at any still-unsorted model rather than weakening the assertion.
  it('an UNSORTED palette keeps its authored order and gets the entry appended', () => {
    const doc = parseVoxelDoc(model('torch-post.voxel.yaml')); // D H F f — grouped, not alphabetical
    expect(doc.palette.map((e) => e.glyph)).toEqual(['D', 'H', 'F', 'f']);
    const result = edit(addGlyph(doc, { glyph: 'B', color: [0.1, 0.1, 0.1, 1] }));
    expect(inserted(doc.source, result)).toBe(
      '  B:\n    color:\n      - 0.1\n      - 0.1\n      - 0.1\n      - 1.0\n',
    );
    expect(parseVoxelDoc(result.source).palette.map((e) => e.glyph)).toEqual([
      'D',
      'H',
      'F',
      'f',
      'B',
    ]);
  });

  it.each(NAMES)('%s: adding an entry leaves every other byte identical and re-parses', (name) => {
    const doc = parseVoxelDoc(model(name));
    const glyph = ['Q', 'q', 'Z', 'z', 'V'].find((g) => paletteEntry(doc, g) === null) as string;
    const result = edit(
      addGlyph(doc, { glyph, color: [0.25, 0.5, 0.75, 1], emissive: 2, metallic: 0.5 }),
    );
    expect(inserted(doc.source, result)).toContain(`${glyph}:`);
    const reparsed = parseVoxelDoc(result.source);
    expect(reparsed.palette).toHaveLength(doc.palette.length + 1);
    expect(reparsed.layers.map((l) => l.rows.map((r) => r.text))).toEqual(
      doc.layers.map((l) => l.rows.map((r) => r.text)),
    );
    const added = paletteEntry(reparsed, glyph);
    expect(added?.color.map((c) => c.value)).toEqual([0.25, 0.5, 0.75, 1]);
    expect(added?.emissive?.value).toBe(2);
    expect(added?.metallic?.value).toBe(0.5);
    expect(added?.roughness).toBeNull();
  });

  it('matches the file line ending and non-standard indentation', () => {
    const doc = parseVoxelDoc(
      [
        'format: 1',
        'kind: voxel',
        'palette:',
        '    A:',
        '        color:',
        '            - 1.0',
        '            - 1.0',
        '            - 1.0',
        '            - 1.0',
        'slices:',
        '  -',
        '    - "A"',
        '',
      ].join('\r\n'),
    );
    const result = edit(addGlyph(doc, { glyph: 'B', color: [0, 0, 0, 1] }));
    expect(result.source).toContain(
      '    B:\r\n        color:\r\n            - 0.0\r\n            - 0.0\r\n            - 0.0\r\n            - 1.0\r\n',
    );
    expect(parseVoxelDoc(result.source).palette.map((e) => e.glyph)).toEqual(['A', 'B']);
  });

  it('goes above a comment block introducing the successor, never between them', () => {
    const doc = parseVoxelDoc(
      [
        'format: 1',
        'kind: voxel',
        'palette:',
        '  A:',
        '    color:',
        '      - 1.0',
        '      - 1.0',
        '      - 1.0',
        '      - 1.0',
        '',
        '  # C is the emissive accent — it must stay attached to C.',
        '  C:',
        '    color:',
        '      - 0.0',
        '      - 0.0',
        '      - 0.0',
        '      - 1.0',
        'slices:',
        '  -',
        '    - "A"',
        '',
      ].join('\n'),
    );
    const result = edit(addGlyph(doc, { glyph: 'B', color: [0.5, 0.5, 0.5, 1] }));
    expect(result.source).toContain(
      '      - 1.0\n  B:\n    color:\n      - 0.5\n      - 0.5\n      - 0.5\n      - 1.0\n\n  # C is the emissive accent',
    );
  });

  it('quotes a glyph that YAML would not read as a plain key', () => {
    const doc = parseVoxelDoc(BRAZIER);
    const result = edit(addGlyph(doc, { glyph: '#', color: [1, 0, 0, 1] }));
    expect(result.source).toContain('palette:\n  "#":\n    color:\n      - 1.0\n');
    expect(parseVoxelDoc(result.source).palette.map((e) => e.glyph)).toEqual([
      '#',
      'C',
      'E',
      'I',
    ]);
  });

  it('appends a line break first when the file does not end in one', () => {
    const doc = parseVoxelDoc(
      'format: 1\nkind: voxel\npalette:\n  A:\n    color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0',
    );
    const result = edit(addGlyph(doc, { glyph: 'B', color: [0, 0, 0, 1] }));
    expect(result.source).toBe(`${doc.source}\n  B:\n    color:\n      - 0.0\n      - 0.0\n      - 0.0\n      - 1.0\n`);
  });

  it('refuses a duplicate, a bad glyph, a non-finite value and an empty palette', () => {
    const doc = parseVoxelDoc(BRAZIER);
    expect(addGlyph(doc, { glyph: 'C', color: [0, 0, 0, 1] })).toMatchObject({
      error: { reason: 'duplicate-glyph' },
    });
    for (const glyph of ['.', '', 'AB', ' ']) {
      expect(addGlyph(doc, { glyph, color: [0, 0, 0, 1] })).toMatchObject({
        error: { reason: 'unknown-glyph' },
      });
    }
    expect(addGlyph(doc, { glyph: 'Q', color: [0, 0, Number.NaN, 1] })).toMatchObject({
      error: { reason: 'bad-number' },
    });
    expect(addGlyph(doc, { glyph: 'Q', color: [0, 0, 0, 1], emissive: Number.NaN })).toMatchObject({
      error: { reason: 'bad-number' },
    });
    const empty = parseVoxelDoc('format: 1\nkind: voxel\npalette: {}\nslices: []\n');
    expect(addGlyph(empty, { glyph: 'A', color: [0, 0, 0, 1] })).toMatchObject({
      error: { reason: 'no-anchor' },
    });
  });
});

describe('formatVoxelFloat', () => {
  it('is shortest round-trip, never fixed precision', () => {
    expect(formatVoxelFloat(0.1)).toBe('0.1');
    expect(formatVoxelFloat(0.2)).toBe('0.2');
    expect(formatVoxelFloat(0.15)).toBe('0.15');
    expect(formatVoxelFloat(0.95)).toBe('0.95');
    expect(formatVoxelFloat(5.5)).toBe('5.5');
    // A picker's 128/255 is genuinely that number; the caller rounds if it wants fewer digits.
    expect(formatVoxelFloat(128 / 255)).toBe('0.5019607843137255');
  });

  it('never writes an integral value as a YAML integer', () => {
    expect(formatVoxelFloat(1)).toBe('1.0');
    expect(formatVoxelFloat(0)).toBe('0.0');
    expect(formatVoxelFloat(-0)).toBe('0.0');
    expect(formatVoxelFloat(12)).toBe('12.0');
  });

  it('keeps exponent forms, which the engine reads as floats', () => {
    expect(formatVoxelFloat(1e-7)).toBe('1e-7');
    expect(formatVoxelFloat(1e21)).toBe('1e+21');
  });

  it('round-trips every value it formats', () => {
    for (const value of [0, 1, -1, 0.1, 0.3, 1 / 3, 128 / 255, 5.5, 1e-7, 1e21, -0.25]) {
      expect(Number(formatVoxelFloat(value))).toBe(value);
    }
  });
});
