import { describe, expect, it } from 'vitest';
import { parseSculptDoc, type SculptDoc } from './doc';
import {
  formatSculptFloat,
  invertSplices,
  setScalar,
  spliceSource,
  type Splice,
  type SculptEdit,
} from './edit';

// Edited against the REAL gallery idol, like `doc.test.ts` — the property under test is that a
// hand-authored file survives an edit byte-for-byte outside the one token touched, and only a real
// file has comments and non-canonical part order to lose.
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

/** Asserts `after` differs from `before` only inside the region `splice` describes. */
function assertOnlySpliceChanged(before: string, after: string, splice: Splice): void {
  expect(after.slice(0, splice.offset)).toBe(before.slice(0, splice.offset));
  expect(after.slice(splice.offset, splice.offset + splice.inserted.length)).toBe(splice.inserted);
  expect(after.slice(splice.offset + splice.inserted.length)).toBe(
    before.slice(splice.offset + splice.removed.length),
  );
}

function ok(result: SculptEdit): Splice {
  expect(result.splice, 'expected an edit, got a no-op').not.toBeNull();
  return result.splice as Splice;
}

describe('setScalar — the idol, every field', () => {
  const doc = parseSculptDoc(IDOL);

  it.each(doc.fields.map((f) => f.pointer))('%s: writes a new value and nothing else', (pointer) => {
    const before = doc.fields.find((f) => f.pointer === pointer)!;
    const target = before.value + 1;
    const result = setScalar(doc, pointer, target);
    const splice = ok(result);

    assertOnlySpliceChanged(doc.source, result.source, splice);

    const reparsed = parseSculptDoc(result.source);
    const after = reparsed.fields.find((f) => f.pointer === pointer);
    expect(after?.value).toBe(target);
    // Every other field's value is untouched.
    for (const field of doc.fields) {
      if (field.pointer === pointer) continue;
      const same = reparsed.fields.find((f) => f.pointer === field.pointer);
      expect(same?.value).toBe(field.value);
    }
  });

  it.each(doc.fields.map((f) => f.pointer))('%s: undoing the edit restores the file byte-identically', (pointer) => {
    const before = doc.fields.find((f) => f.pointer === pointer)!;
    const result = setScalar(doc, pointer, before.value + 1);
    const splice = ok(result);
    const restored = spliceSource(result.source, invertSplices(splice));
    expect(restored).toBe(doc.source);
  });
});

describe('setScalar — integer keys write no decimal point', () => {
  const doc = parseSculptDoc(IDOL);

  it('resolution', () => {
    const splice = ok(setScalar(doc, '/resolution', 64));
    expect(splice.inserted).toBe('64');
  });

  it('triangles', () => {
    const splice = ok(setScalar(doc, '/triangles', 8192));
    expect(splice.inserted).toBe('8192');
  });

  it('a `repeat.count` written on a synthetic document also stays bare', () => {
    const source = [
      'format: 1',
      'kind: sculpt',
      'parts:',
      '  - name: ring',
      '    shape: sphere',
      '    radius: 0.1',
      '    repeat:',
      '      count: 4',
      '      step:',
      '        - 0.2',
      '        - 0.0',
      '        - 0.0',
      '',
    ].join('\n');
    const ringDoc: SculptDoc = parseSculptDoc(source);
    const splice = ok(setScalar(ringDoc, '/parts/0/repeat/count', 8));
    expect(splice.inserted).toBe('8');
  });
});

describe('setScalar — whole-number floats keep the file\'s own `.0` style', () => {
  it('a component authored as `1.0` stays in `.0` style when set to another whole number', () => {
    const doc = parseSculptDoc(IDOL);
    // `/palette/B/color/3` is authored `1.0`.
    const splice = ok(setScalar(doc, '/palette/B/color/3', 2));
    expect(splice.removed).toBe('1.0');
    expect(splice.inserted).toBe('2.0');
  });

  it('a scalar authored without a decimal point is not given one it never had', () => {
    const source = [
      'format: 1',
      'kind: sculpt',
      'parts:',
      '  - name: box',
      '    shape: box',
      '    blend: 2',
      '    size:',
      '      - 1.0',
      '      - 1.0',
      '      - 1.0',
      '',
    ].join('\n');
    const doc = parseSculptDoc(source);
    const splice = ok(setScalar(doc, '/parts/0/blend', 3));
    expect(splice.removed).toBe('2');
    expect(splice.inserted).toBe('3');
  });

  it('a non-whole value never gains or loses digits', () => {
    const doc = parseSculptDoc(IDOL);
    const splice = ok(setScalar(doc, '/parts/0/at/1', 0.275));
    expect(splice.inserted).toBe('0.275');
  });
});

describe('setScalar — leniency (it never throws, and refuses only by no-op)', () => {
  const doc = parseSculptDoc(IDOL);

  it('an unknown pointer is a no-op', () => {
    const result = setScalar(doc, '/parts/999/at/0', 5);
    expect(result.splice).toBeNull();
    expect(result.source).toBe(doc.source);
  });

  it('a non-numeric pointer (there is no field for it) is a no-op', () => {
    const result = setScalar(doc, '/parts/0/material', 1);
    expect(result.splice).toBeNull();
    expect(result.source).toBe(doc.source);
  });

  it('NaN and Infinity are refused as a no-op, never written', () => {
    expect(setScalar(doc, '/resolution', Number.NaN).splice).toBeNull();
    expect(setScalar(doc, '/resolution', Number.POSITIVE_INFINITY).splice).toBeNull();
  });

  it('setting a field to its own value is a no-op', () => {
    const field = doc.resolution!;
    const result = setScalar(doc, '/resolution', field.value);
    expect(result.splice).toBeNull();
    expect(result.source).toBe(doc.source);
  });
});

// A synthetic fixture for the keys Lore rev 742 added — the idol has none of them. Matches
// `doc.test.ts`'s `RIG`.
const RIG = [
  'format: 1',
  'kind: sculpt',
  'name: rig-test',
  'resolution: 32',
  'triangles: 4000',
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

describe('setScalar — nested deform/noise and a bone\'s head/tail (Lore rev 742)', () => {
  const doc = parseSculptDoc(RIG);

  it('a `degrees` authored without a decimal point (twist: 90) stays bare on a new whole value', () => {
    const splice = ok(setScalar(doc, '/parts/0/deform/twist/degrees', 45));
    expect(splice.removed).toBe('90');
    expect(splice.inserted).toBe('45');
    assertOnlySpliceChanged(doc.source, spliceSource(doc.source, splice), splice);
  });

  it('a `degrees` authored with a decimal point (bend: 30.0) keeps it on a new whole value', () => {
    const splice = ok(setScalar(doc, '/parts/0/deform/bend/degrees', 45));
    expect(splice.removed).toBe('30.0');
    expect(splice.inserted).toBe('45.0');
  });

  it('splices a bone\'s `head/1` at its own (deeper-indented) position, nothing else moves', () => {
    const splice = ok(setScalar(doc, '/bones/1/head/1', 2.5));
    expect(splice.removed).toBe('1.0');
    expect(splice.inserted).toBe('2.5');
    const after = spliceSource(doc.source, splice);
    assertOnlySpliceChanged(doc.source, after, splice);

    const reparsed = parseSculptDoc(after);
    expect(reparsed.bones[1].fields.map((f) => f.value)).toEqual([0.0, 2.5, 0.0, 0.0, 1.6, 0.0]);
  });

  it('a taper\'s `from`/`to` and a noise\'s `amplitude`/`scale` are ordinary decimal splices', () => {
    expect(ok(setScalar(doc, '/parts/0/deform/taper/from', 0.8)).inserted).toBe('0.8');
    expect(ok(setScalar(doc, '/parts/0/noise/amplitude', 0.05)).inserted).toBe('0.05');
  });

  it('`noise`\'s `seed` has no field to splice: the pointer is a no-op', () => {
    const result = setScalar(doc, '/parts/0/noise/seed', 99);
    expect(result.splice).toBeNull();
    expect(result.source).toBe(doc.source);
  });
});

describe('formatSculptFloat', () => {
  it('integer keys truncate and never carry a decimal point', () => {
    expect(formatSculptFloat(64, 'resolution', '48')).toBe('64');
    expect(formatSculptFloat(64.9, 'resolution', '48')).toBe('64');
    expect(formatSculptFloat(8, 'count', '4')).toBe('8');
  });

  it('a whole float keeps `.0` only when the original scalar had a decimal point', () => {
    expect(formatSculptFloat(2, 'color', '1.0')).toBe('2.0');
    expect(formatSculptFloat(2, 'blend', '2')).toBe('2');
  });

  it('shortest round trip for a non-whole value', () => {
    expect(formatSculptFloat(0.1, 'blend', '0.5')).toBe('0.1');
    expect(formatSculptFloat(-20, 'rotation', '25.0')).toBe('-20.0');
  });
});
