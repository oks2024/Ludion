import { describe, expect, it } from 'vitest';
import type { Vec3 } from '../gizmo/vecmath';
import {
  cellOffset,
  glyphAt,
  paletteEntry,
  parseVoxelDoc,
  sliceRow,
  type NumberField,
  type VoxelDoc,
} from './doc';

// The tests run against a frozen copy of real authored models, pulled in as raw text rather than inlined:
// `brazier.voxel.yaml` carries a five-line header docstring, a comment above most of its sixteen
// layers, three palette entries with mixed optional material scalars, and quoted rows. Those
// are exactly the features contract 1 forbids destroying, so they are what the offsets have to
// survive — and a fixture copy would stop being real the first time a model is re-authored.
const MODELS = import.meta.glob('../../../fixtures/stage/scenes/voxels/*.voxel.yaml', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/** One model's source, by file name. */
function model(name: string): string {
  const key = Object.keys(MODELS).find((path) => path.endsWith(`/${name}`));
  expect(key, `${name} is missing from the frozen stage fixture (editor/app/fixtures/stage)`).toBeDefined();
  return MODELS[key as string];
}

const BRAZIER = model('brazier.voxel.yaml');

/** Reads back exactly the source characters a span covers. */
function at(doc: VoxelDoc, span: { offset: number; length: number }): string {
  return doc.source.slice(span.offset, span.offset + span.length);
}

function field(value: NumberField | null): NumberField {
  expect(value).not.toBeNull();
  return value as NumberField;
}

describe('parseVoxelDoc — a real authored model', () => {
  const doc = parseVoxelDoc(BRAZIER);

  it('reads the header keys, skipping the docstring comments', () => {
    expect(doc.name).toBe('brazier');
    expect(doc.size).toEqual([20, 16, 20]);
    expect(field(doc.scale).value).toBe(0.1);
  });

  it('reads the palette in AUTHORED order with its optional material scalars', () => {
    expect(doc.palette.map((entry) => entry.glyph)).toEqual(['C', 'E', 'I']);
    const d = paletteEntry(doc, 'C');
    expect(d?.color.map((c) => c.value)).toEqual([0.09, 0.07, 0.07, 1.0]);
    expect(field(d?.roughness ?? null).value).toBe(0.95);
    expect(d?.emissive).toBeNull();
    expect(d?.metallic).toBeNull();
    expect(field(paletteEntry(doc, 'E')?.emissive ?? null).value).toBe(4.0);
  });

  it('reads sixteen layers of twenty rows, bottom-up, with the comments in between ignored', () => {
    expect(doc.layers).toHaveLength(16);
    for (const layer of doc.layers) {
      expect(layer.rows).toHaveLength(20);
      for (const row of layer.rows) {
        expect(row.text).toHaveLength(20);
        expect(row.quoted).toBe(true);
      }
    }
    // y0 is the crossed foot bar, y11 the bowl wall with its lugs — the file's own comments say so.
    expect(sliceRow(doc, 0, 4)?.text).toBe('.........II.........');
    expect(sliceRow(doc, 11, 9)?.text).toBe('..IIIICEECCCECIIII..');
  });
});

describe('parseVoxelDoc — source offsets', () => {
  const doc = parseVoxelDoc(BRAZIER);

  it('every slice row span points at its own characters in the source', () => {
    for (const layer of doc.layers) {
      for (const row of layer.rows) {
        expect(at(doc, row.span)).toBe(row.text);
        expect(row.paintable).toBe(true);
      }
    }
  });

  it('cellOffset addresses the ONE character a paint replaces', () => {
    // y8 row z=9 is `......IICECEII......` — the embers among the coals.
    expect(sliceRow(doc, 8, 9)?.text).toBe('......IICECEII......');
    for (const [x, glyph] of [...'......IICECEII......'].entries()) {
      const offset = cellOffset(doc, x, 8, 9);
      expect(offset).not.toBeNull();
      expect(doc.source[offset as number]).toBe(glyph);
      expect(glyphAt(doc, x, 8, 9)).toBe(glyph);
    }
  });

  it('painting one cell rewrites exactly one character and nothing else', () => {
    const offset = cellOffset(doc, 9, 8, 9) as number;
    const painted = doc.source.slice(0, offset) + 'I' + doc.source.slice(offset + 1);
    expect(painted).toHaveLength(doc.source.length);
    expect(painted.split('\n').length).toBe(doc.source.split('\n').length);
    // The header docstring and every comment between layers survive byte-identical.
    expect(painted).toContain('# Brazier — a hollow iron fire bowl');
    expect(painted).toContain('# bowl wall; carrying lugs to the sides');
    const reparsed = parseVoxelDoc(painted);
    expect(glyphAt(reparsed, 9, 8, 9)).toBe('I');
    expect(sliceRow(reparsed, 8, 9)?.text).toBe('......IICICEII......');
  });

  it('cellOffset refuses coordinates it cannot address', () => {
    expect(cellOffset(doc, 20, 8, 9)).toBeNull();
    expect(cellOffset(doc, -1, 8, 9)).toBeNull();
    expect(cellOffset(doc, 0, 99, 1)).toBeNull();
  });

  it('palette number spans cover the number token alone', () => {
    const d = paletteEntry(doc, 'C');
    expect(at(doc, d?.color[0].span as NumberField['span'])).toBe('0.09');
    expect(at(doc, d?.color[3].span as NumberField['span'])).toBe('1.0');
    expect(at(doc, field(d?.roughness ?? null).span)).toBe('0.95');
    expect(at(doc, field(paletteEntry(doc, 'E')?.emissive ?? null).span)).toBe('4.0');
    expect(at(doc, field(doc.scale).span)).toBe('0.1');
  });

  it('the palette key span covers the glyph', () => {
    const e = paletteEntry(doc, 'E');
    expect(at(doc, e?.keySpan as NumberField['span'])).toBe('E');
  });

  it('spans carry 1-based lines and 0-based columns', () => {
    const first = sliceRow(doc, 0, 0);
    // `    - "...................."` — the content starts inside the quote, at column 7.
    expect(first?.span.column).toBe(7);
    expect(BRAZIER.split('\n')[(first?.span.line as number) - 1]).toBe('    - "...................."');
  });
});

describe('parseVoxelDoc — every committed stage model', () => {
  // Parser fidelity, not validation: these 5 files are known-good (CI runs `check-scene` on them),
  // so reading them back with the shape their own `size` declares proves this module agrees with
  // `crates/ludion-scene/src/voxel.rs` across every authored idiom in the repo — emissive accents,
  // metallic props, mirrored creatures, one-glyph palettes.
  const names = Object.keys(MODELS).map((path) => path.split('/').at(-1) as string);

  it('finds the model library', () => {
    expect(names.length).toBeGreaterThan(4);
  });

  it.each(names)('%s parses to its declared shape with round-tripping offsets', (name) => {
    const doc = parseVoxelDoc(model(name));
    const size = doc.size as Vec3;
    expect(size).not.toBeNull();
    expect(doc.layers).toHaveLength(size[1]);
    for (const layer of doc.layers) {
      expect(layer.rows).toHaveLength(size[2]);
      for (const row of layer.rows) {
        expect(row.text).toHaveLength(size[0]);
        expect(at(doc, row.span)).toBe(row.text);
        expect(row.paintable).toBe(true);
      }
    }
    expect(doc.palette.length).toBeGreaterThan(0);
    for (const entry of doc.palette) {
      expect(entry.glyph).toHaveLength(1);
      expect(entry.color.map((c) => at(doc, c.span))).toHaveLength(4);
      expect(entry.color.every((c) => Number.isFinite(c.value))).toBe(true);
    }
    // Wave 0 backfilled a declared scale into every one of them.
    expect(field(doc.scale).value).toBeGreaterThan(0);
  });
});

describe('parseVoxelDoc — the other authored idiom', () => {
  // What `emit_voxel` writes: bare scalars where quoting is unnecessary, and `- 2` size items.
  const CANONICAL = [
    'format: 1',
    'kind: voxel',
    'size:',
    '  - 2',
    '  - 1',
    '  - 2',
    'scale: 0.5',
    'palette:',
    '  A:',
    '    color:',
    '      - 1.0',
    '      - 0.0',
    '      - 0.0',
    '      - 1.0',
    'slices:',
    '  -',
    '    - A.',
    '    - ".A"',
    '',
  ].join('\n');

  it('reads bare and quoted rows alike, with correct offsets for both', () => {
    const doc = parseVoxelDoc(CANONICAL);
    expect(doc.size).toEqual([2, 1, 2]);
    expect(sliceRow(doc, 0, 0)).toMatchObject({ text: 'A.', quoted: false, paintable: true });
    expect(sliceRow(doc, 0, 1)).toMatchObject({ text: '.A', quoted: true, paintable: true });
    expect(at(doc, sliceRow(doc, 0, 0)?.span as NumberField['span'])).toBe('A.');
    expect(doc.source[cellOffset(doc, 1, 0, 1) as number]).toBe('A');
  });

  it('reads an empty palette and empty slices without inventing entries', () => {
    const doc = parseVoxelDoc('format: 1\nkind: voxel\npalette: {}\nslices: []\n');
    expect(doc.palette).toEqual([]);
    expect(doc.layers).toEqual([]);
    expect(doc.size).toBeNull();
  });
});

describe('parseVoxelDoc — leniency (it never judges, and never throws)', () => {
  it('a truncated file yields what parsed', () => {
    const doc = parseVoxelDoc('format: 1\nkind: voxel\nsize:\n  - 4\n  - 2\nslices:\n  -\n    - "AB');
    expect(doc.size).toBeNull(); // only two of three dimensions
    expect(doc.layers[0].rows[0].text).toBe('AB'); // unterminated quote still reads
  });

  it('rows that disagree with `size` are reported as authored — the engine judges, not this', () => {
    const doc = parseVoxelDoc(
      'format: 1\nkind: voxel\nsize:\n  - 9\n  - 1\n  - 1\nslices:\n  -\n    - "AB"\n',
    );
    expect(doc.size).toEqual([9, 1, 1]);
    expect(doc.layers[0].rows[0].text).toBe('AB');
  });

  it('an escaped row parses but is not paintable', () => {
    const doc = parseVoxelDoc('format: 1\nkind: voxel\nslices:\n  -\n    - "A\\\\B"\n');
    const row = sliceRow(doc, 0, 0);
    expect(row?.text).toBe('A\\B');
    expect(row?.paintable).toBe(false);
    expect(cellOffset(doc, 0, 0, 0)).toBeNull();
  });

  it('an empty document is an empty doc', () => {
    const doc = parseVoxelDoc('');
    expect(doc).toMatchObject({ name: null, size: null, scale: null, palette: [], layers: [] });
  });

  it('a comment-only document is an empty doc', () => {
    expect(parseVoxelDoc('# nothing here\n\n# still nothing\n').layers).toEqual([]);
  });

  it('CRLF line endings do not shift offsets', () => {
    const doc = parseVoxelDoc(BRAZIER.replace(/\n/g, '\r\n'));
    for (const layer of doc.layers) {
      for (const row of layer.rows) {
        expect(at(doc, row.span)).toBe(row.text);
      }
    }
  });
});
