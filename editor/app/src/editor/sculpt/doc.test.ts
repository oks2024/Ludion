import { describe, expect, it } from 'vitest';
import { fieldAt, fieldOffset, parseSculptDoc, type NumberField, type SculptDoc } from './doc';

// Read against the REAL gallery idol, not an inlined fixture — the whole point of this module is
// that a hand-authored file's comments and part order survive being read positionally, and a
// synthetic fixture would not exercise the mirrored parts, the subtract part, or the rotated ones.
const FILES = import.meta.glob('../../../../../examples/gallery/sculpts/idol.sculpt.yaml', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

function idolSource(): string {
  const keys = Object.keys(FILES);
  expect(keys, 'the gallery idol is missing from examples/gallery/sculpts/').toHaveLength(1);
  return FILES[keys[0]];
}

const IDOL = idolSource();

/** Reads back exactly the source characters `field`'s position covers. */
function at(doc: SculptDoc, field: NumberField): string {
  const offset = fieldOffset(doc, field);
  return doc.source.slice(offset, offset + field.length);
}

function field(value: NumberField | null): NumberField {
  expect(value).not.toBeNull();
  return value as NumberField;
}

describe('parseSculptDoc — the gallery idol', () => {
  const doc = parseSculptDoc(IDOL);

  it('reads the header scalars', () => {
    expect(doc.name).toBe('idol');
    expect(field(doc.resolution).value).toBe(48);
    expect(field(doc.triangles).value).toBe(4000);
  });

  it('reads the palette in authored order, with color and the optional material scalars', () => {
    expect(doc.palette.map((entry) => entry.glyph)).toEqual(['B', 'D', 'E', 'J', 'K', 'T']);
    const e = doc.palette.find((entry) => entry.glyph === 'E');
    expect(e?.fields.map((f) => f.pointer)).toEqual([
      '/palette/E/color/0',
      '/palette/E/color/1',
      '/palette/E/color/2',
      '/palette/E/color/3',
      '/palette/E/emissive',
      '/palette/E/roughness',
    ]);
    expect(e?.fields.map((f) => f.value)).toEqual([0.5, 0.95, 0.45, 1.0, 6.75, 0.19]);
  });

  it('reads all fourteen parts in file order, with name and shape', () => {
    expect(doc.parts).toHaveLength(14);
    expect(doc.parts.map((p) => p.index)).toEqual([...Array(14).keys()]);
    expect(doc.parts[0]).toMatchObject({ name: 'plinth', shape: 'box' });
    expect(doc.parts[6]).toMatchObject({ name: 'wing', shape: 'ellipsoid' });
    expect(doc.parts.at(-1)).toMatchObject({ name: 'eye', shape: 'sphere' });
  });

  it('reads a box part: size, at, and no material/op numeric leakage', () => {
    const plinth = doc.parts[0];
    expect(plinth.fields.map((f) => f.pointer)).toEqual([
      '/parts/0/size/0',
      '/parts/0/size/1',
      '/parts/0/size/2',
      '/parts/0/at/0',
      '/parts/0/at/1',
      '/parts/0/at/2',
    ]);
    expect(plinth.fields.map((f) => f.value)).toEqual([0.9, 0.3, 0.9, 0.0, 0.15, 0.0]);
  });

  it('reads a capsule part: radius, from, to, blend — no `at`', () => {
    const arm = doc.parts.find((p) => p.name === 'arm');
    expect(arm?.shape).toBe('capsule');
    expect(arm?.fields.map((f) => f.pointer)).toEqual([
      '/parts/5/radius',
      '/parts/5/from/0',
      '/parts/5/from/1',
      '/parts/5/from/2',
      '/parts/5/to/0',
      '/parts/5/to/1',
      '/parts/5/to/2',
      '/parts/5/blend',
    ]);
    expect(arm?.fields.every((f) => f.key !== 'at')).toBe(true);
  });

  it('reads rotation on the parts that declare it', () => {
    const wing = doc.parts.find((p) => p.name === 'wing');
    const rotation = wing?.fields.filter((f) => f.key === 'rotation') ?? [];
    expect(rotation.map((f) => f.pointer)).toEqual([
      '/parts/6/rotation/0',
      '/parts/6/rotation/1',
      '/parts/6/rotation/2',
    ]);
    expect(rotation.map((f) => f.value)).toEqual([25.0, 0.0, -20.0]);
  });

  it('`key` is the pointer\'s last non-index segment', () => {
    const socket = doc.parts.find((p) => p.name === 'socket');
    expect(socket?.fields.map((f) => f.key)).toEqual(['radius', 'at', 'at', 'at', 'blend']);
  });

  it('the flat `fields` list is the union of the header, palette, bones and part fields, in file order', () => {
    const total =
      1 + // resolution
      1 + // triangles
      doc.palette.reduce((sum, entry) => sum + entry.fields.length, 0) +
      doc.bones.reduce((sum, bone) => sum + bone.fields.length, 0) +
      doc.parts.reduce((sum, part) => sum + part.fields.length, 0);
    expect(doc.fields).toHaveLength(total);
    expect(doc.fields[0].pointer).toBe('/resolution');
    expect(doc.fields[1].pointer).toBe('/triangles');
  });

  it('the idol\'s one bone (`root`, ungrouped) has a name and no position fields', () => {
    expect(doc.bones.map((b) => b.name)).toEqual(['root']);
    expect(doc.bones[0].fields).toEqual([]);
  });

  it('fieldAt finds a field by pointer, or null for an unknown one', () => {
    expect(field(fieldAt(doc, '/parts/0/at/1')).value).toBe(0.15);
    expect(fieldAt(doc, '/parts/99/at/0')).toBeNull();
    expect(fieldAt(doc, '/parts/0/material')).toBeNull();
  });
});

describe('parseSculptDoc — every numeric field lands on its own token', () => {
  const doc = parseSculptDoc(IDOL);

  it('every field position slices back exactly the token that produced its value', () => {
    expect(doc.fields.length).toBeGreaterThan(50);
    for (const f of doc.fields) {
      const token = at(doc, f);
      expect(Number(token)).toBe(f.value);
      expect(IDOL.split('\n')[f.line].slice(f.column, f.column + f.length)).toBe(token);
    }
  });

  it('line/column are 0-based', () => {
    // The idol opens with a three-line comment, then `format`/`kind`/`name` — `resolution: 48` is
    // the seventh line of the file, index 6 in 0-based counting.
    const resolution = field(doc.resolution);
    expect(IDOL.split('\n')[resolution.line]).toBe('resolution: 48');
    expect(resolution.line).toBe(6);
    expect(resolution.column).toBe('resolution: '.length);
  });
});

describe('parseSculptDoc — leniency (it never judges, and never throws)', () => {
  it('an empty document is an empty doc', () => {
    const doc = parseSculptDoc('');
    expect(doc).toMatchObject({
      name: null,
      resolution: null,
      triangles: null,
      palette: [],
      parts: [],
      fields: [],
    });
  });

  it('a comment-only document is an empty doc', () => {
    expect(parseSculptDoc('# nothing here\n\n# still nothing\n').parts).toEqual([]);
  });

  it('a flow-style vector is left unparsed rather than misread', () => {
    const doc = parseSculptDoc(
      [
        'format: 1',
        'kind: sculpt',
        'parts:',
        '  - name: box',
        '    shape: box',
        '    size: [0.5, 0.5, 0.5]',
        '    at: [0, 0, 0]',
        '',
      ].join('\n'),
    );
    expect(doc.parts).toHaveLength(1);
    expect(doc.parts[0]).toMatchObject({ name: 'box', shape: 'box', fields: [] });
  });

  it('a part with a missing name/shape reads as empty strings, not a crash', () => {
    const doc = parseSculptDoc(
      ['format: 1', 'kind: sculpt', 'parts:', '  - radius: 0.5', ''].join('\n'),
    );
    expect(doc.parts).toHaveLength(1);
    expect(doc.parts[0]).toMatchObject({ name: '', shape: '', index: 0 });
    expect(doc.parts[0].fields[0]).toMatchObject({ pointer: '/parts/0/radius', value: 0.5 });
  });

  it('a truncated file yields what parsed', () => {
    const doc = parseSculptDoc('format: 1\nkind: sculpt\nresolution: 32\ntriangl');
    expect(field(doc.resolution).value).toBe(32);
    expect(doc.triangles).toBeNull();
  });

  it('CRLF line endings do not shift positions', () => {
    const doc = parseSculptDoc(IDOL.replace(/\n/g, '\r\n'));
    for (const f of doc.fields) {
      expect(Number(at(doc, f))).toBe(f.value);
    }
    expect(field(doc.resolution).value).toBe(48);
  });
});

// A synthetic fixture, not the idol: the idol has none of `bones:`' `head`/`tail`, a part's `deform`
// or `noise` — the keys Lore rev 742 added (`crates/ludion-scene/src/sculpt.rs`).
const RIG = [
  'format: 1',
  'kind: sculpt',
  'name: rig-test',
  'resolution: 32',
  'triangles: 4000',
  'palette:',
  '  S:',
  '    color:',
  '      - 1.0',
  '      - 1.0',
  '      - 1.0',
  '      - 1.0',
  'bones:',
  '  - name: root',
  '  - name: arm',
  '    parent: root',
  '    head:',
  '      - 0.0',
  '      - 1.0',
  '      - 0.0',
  '    tail:',
  '      - 0.0',
  '      - 1.6',
  '      - 0.0',
  'parts:',
  '  - name: limb',
  '    shape: capsule',
  '    radius: 0.1',
  '    from:',
  '      - 0.0',
  '      - 0.0',
  '      - 0.0',
  '    to:',
  '      - 0.0',
  '      - 0.5',
  '      - 0.0',
  '    blend: 0.05',
  '    material: S',
  '    bone: arm',
  '    deform:',
  '      bend:',
  '        axis: x',
  '        degrees: 30.0',
  '      twist:',
  '        axis: y',
  '        degrees: 90',
  '      taper:',
  '        axis: z',
  '        from: 1.0',
  '        to: 0.5',
  '    noise:',
  '      amplitude: 0.02',
  '      scale: 0.1',
  '      seed: 7',
  '',
].join('\n');

describe('parseSculptDoc — bones (`head`/`tail`) and a part\'s `deform`/`noise` (Lore rev 742)', () => {
  const doc = parseSculptDoc(RIG);

  it('reads both bones, the root ungrouped and `arm` with a head/tail segment', () => {
    expect(doc.bones.map((b) => b.name)).toEqual(['root', 'arm']);
    expect(doc.bones[0].fields).toEqual([]);
    expect(doc.bones[1].fields.map((f) => f.pointer)).toEqual([
      '/bones/1/head/0',
      '/bones/1/head/1',
      '/bones/1/head/2',
      '/bones/1/tail/0',
      '/bones/1/tail/1',
      '/bones/1/tail/2',
    ]);
    expect(doc.bones[1].fields.map((f) => f.value)).toEqual([0.0, 1.0, 0.0, 0.0, 1.6, 0.0]);
    expect(doc.bones[1].fields.every((f) => f.key === 'head' || f.key === 'tail')).toBe(true);
  });

  it('reads a part\'s `deform` (bend, twist, taper) in file order, `axis` never numeric', () => {
    const limb = doc.parts[0];
    expect(limb.fields.map((f) => f.pointer)).toEqual([
      '/parts/0/radius',
      '/parts/0/from/0',
      '/parts/0/from/1',
      '/parts/0/from/2',
      '/parts/0/to/0',
      '/parts/0/to/1',
      '/parts/0/to/2',
      '/parts/0/blend',
      '/parts/0/deform/bend/degrees',
      '/parts/0/deform/twist/degrees',
      '/parts/0/deform/taper/from',
      '/parts/0/deform/taper/to',
      '/parts/0/noise/amplitude',
      '/parts/0/noise/scale',
    ]);
    expect(field(fieldAt(doc, '/parts/0/deform/bend/degrees')).value).toBe(30.0);
    expect(field(fieldAt(doc, '/parts/0/deform/twist/degrees')).value).toBe(90);
    expect(field(fieldAt(doc, '/parts/0/deform/taper/from')).value).toBe(1.0);
    expect(field(fieldAt(doc, '/parts/0/deform/taper/to')).value).toBe(0.5);
  });

  it('`bend` and `twist` share the `degrees` key; `taper`\'s `from`/`to` are keyed `taper`, not `from`/`to`', () => {
    expect(fieldAt(doc, '/parts/0/deform/bend/degrees')?.key).toBe('degrees');
    expect(fieldAt(doc, '/parts/0/deform/twist/degrees')?.key).toBe('degrees');
    expect(fieldAt(doc, '/parts/0/deform/taper/from')?.key).toBe('taper');
    expect(fieldAt(doc, '/parts/0/deform/taper/to')?.key).toBe('taper');
  });

  it('reads a part\'s `noise` amplitude/scale, but never its `seed`', () => {
    expect(field(fieldAt(doc, '/parts/0/noise/amplitude')).value).toBe(0.02);
    expect(field(fieldAt(doc, '/parts/0/noise/scale')).value).toBe(0.1);
    expect(fieldAt(doc, '/parts/0/noise/seed')).toBeNull();
    expect(doc.fields.some((f) => f.key === 'seed')).toBe(false);
  });
});

// A `voxel`/`mesh` part's `size` is one inline scalar (world units per voxel/document unit); a
// `box`'s `size` is a three-component block sequence. The two never appear on the same part, so
// this fixture carries all three shapes to prove the reader tells them apart by the entry's own
// form, not by which shape declared it.
const SIZES = [
  'format: 1',
  'kind: sculpt',
  'palette:',
  '  S:',
  '    color:',
  '      - 1.0',
  '      - 1.0',
  '      - 1.0',
  '      - 1.0',
  'bones:',
  '  - name: root',
  'parts:',
  '  - name: hull',
  '    shape: voxel',
  '    path: hull.voxel.yaml',
  '    size: 0.125',
  '    material: S',
  '  - name: idol',
  '    shape: mesh',
  '    path: idol.mesh.yaml',
  '    size: 1.5',
  '    material: S',
  '  - name: idol-bare',
  '    shape: mesh',
  '    path: idol.mesh.yaml',
  '    material: S',
  '  - name: plinth',
  '    shape: box',
  '    size:',
  '      - 0.9',
  '      - 0.3',
  '      - 0.9',
  '    material: S',
  '',
].join('\n');

describe('parseSculptDoc — a voxel/mesh part\'s scalar `size` (Lore rev 743)', () => {
  const doc = parseSculptDoc(SIZES);

  it('reads a voxel part\'s `size` as one scalar field, addressed whole', () => {
    const hull = doc.parts.find((p) => p.name === 'hull');
    expect(hull?.fields.map((f) => f.pointer)).toEqual(['/parts/0/size']);
    expect(hull?.fields[0]).toMatchObject({ key: 'size', value: 0.125 });
  });

  it('reads an authored mesh part\'s `size` as one scalar field', () => {
    const idol = doc.parts.find((p) => p.name === 'idol');
    expect(idol?.fields.map((f) => f.pointer)).toEqual(['/parts/1/size']);
    expect(idol?.fields[0]).toMatchObject({ key: 'size', value: 1.5 });
  });

  it('an omitted mesh `size` yields no field at all — no insertion machinery', () => {
    const bare = doc.parts.find((p) => p.name === 'idol-bare');
    expect(bare?.fields).toEqual([]);
    expect(fieldAt(doc, '/parts/2/size')).toBeNull();
  });

  it('a box\'s `size` is still read as three components, unaffected by the scalar case', () => {
    const plinth = doc.parts.find((p) => p.name === 'plinth');
    expect(plinth?.fields.map((f) => f.pointer)).toEqual([
      '/parts/3/size/0',
      '/parts/3/size/1',
      '/parts/3/size/2',
    ]);
    expect(plinth?.fields.map((f) => f.value)).toEqual([0.9, 0.3, 0.9]);
  });
});
