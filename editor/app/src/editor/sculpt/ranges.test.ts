import { describe, expect, it } from 'vitest';
import { parseSculptDoc, type NumberField } from './doc';
import { SLIDER_KEYS, sliderRange } from './ranges';

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

describe('sliderRange — total over the key set', () => {
  it('every key `doc.ts` can produce gets a range with min < max, no matter the value', () => {
    for (const key of SLIDER_KEYS) {
      for (const value of [-5, 0, 0.001, 1, 5, 4000]) {
        const field: NumberField = { pointer: `/${key}`, key, value, line: 0, column: 0, length: 1 };
        const doc = parseSculptDoc('format: 1\nkind: sculpt\n');
        const range = sliderRange(field, doc);
        expect(range.min).toBeLessThan(range.max);
        expect(range.step).toBeGreaterThan(0);
      }
    }
  });

  it('an integer key reports `integer: true` and a unit step', () => {
    const doc = parseSculptDoc('format: 1\nkind: sculpt\n');
    for (const key of ['resolution', 'triangles', 'count'] as const) {
      const field: NumberField = { pointer: `/${key}`, key, value: 10, line: 0, column: 0, length: 2 };
      const range = sliderRange(field, doc);
      expect(range.integer).toBe(true);
      expect(range.step).toBe(1);
    }
  });

  it('a non-integer key reports `integer: false`', () => {
    const doc = parseSculptDoc('format: 1\nkind: sculpt\n');
    for (const key of SLIDER_KEYS.filter((k) => k !== 'resolution' && k !== 'triangles' && k !== 'count')) {
      const field: NumberField = { pointer: `/${key}`, key, value: 0.5, line: 0, column: 0, length: 3 };
      expect(sliderRange(field, doc).integer).toBe(false);
    }
  });
});

describe('sliderRange — every field of the gallery idol', () => {
  const doc = parseSculptDoc(IDOL);

  it('finds a real field of every key the format doc lists (proves the fixture is not stale)', () => {
    const present = new Set(doc.fields.map((f) => f.key));
    for (const key of ['at', 'from', 'to', 'radius', 'radii', 'size', 'blend', 'rotation', 'color', 'emissive', 'roughness', 'resolution', 'triangles']) {
      expect(present.has(key), `no idol field uses key \`${key}\``).toBe(true);
    }
  });

  it.each(doc.fields.map((f) => f.pointer))('%s: the field\'s own value sits inside its range', (pointer) => {
    const field = doc.fields.find((f) => f.pointer === pointer)!;
    const range = sliderRange(field, doc);
    expect(range.min).toBeLessThan(range.max);
    expect(field.value).toBeGreaterThanOrEqual(range.min);
    expect(field.value).toBeLessThanOrEqual(range.max);
  });

  it('coordinate ranges scale with the model, not a fixed window', () => {
    const at = doc.fields.find((f) => f.pointer === '/parts/8/at/1')!; // cranium, y = 1.3
    const range = sliderRange(at, doc);
    // The model's tallest position/size coordinate is around y ~= 1.3 (cranium), so the range must
    // reach at least that far in both directions — `rotation`/`blend` do not count towards it, so
    // this excludes them exactly as `ranges.ts`'s own `modelExtent` does.
    const positionKeys = new Set(['at', 'from', 'to', 'step', 'radius', 'radii', 'size', 'tube', 'height']);
    const extent = Math.max(
      ...doc.parts.flatMap((p) => p.fields).filter((f) => positionKeys.has(f.key)).map((f) => Math.abs(f.value)),
    );
    expect(range.max).toBeGreaterThanOrEqual(extent);
    expect(range.min).toBeLessThanOrEqual(-extent);
  });

  it('a size/radius range is centered at zero and doubles the authored value', () => {
    const radius = doc.fields.find((f) => f.pointer === '/parts/10/radius')!; // socket, 0.07
    const range = sliderRange(radius, doc);
    expect(range.min).toBe(0);
    expect(range.max).toBeCloseTo(radius.value * 2);
  });
});

// A synthetic fixture for the keys Lore rev 742 added — the idol has none of them. Matches
// `doc.test.ts`'s `RIG`.
const RIG = [
  'format: 1',
  'kind: sculpt',
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

describe('sliderRange — deform/noise/bone-position fields (Lore rev 742)', () => {
  const doc = parseSculptDoc(RIG);
  const at = (pointer: string): NumberField => {
    const found = doc.fields.find((f) => f.pointer === pointer);
    expect(found, pointer).toBeDefined();
    return found!;
  };

  it('`degrees` (bend/twist) is -180..180 with a fixed step of 1, not the 200-step default', () => {
    const range = sliderRange(at('/parts/0/deform/bend/degrees'), doc);
    expect(range).toEqual({ min: -180, max: 180, step: 1, integer: false });
  });

  it('a taper\'s `from`/`to` is a 0.1..3 cross-section scale, not a position', () => {
    const range = sliderRange(at('/parts/0/deform/taper/from'), doc);
    expect(range).toEqual({ min: 0.1, max: 3, step: 0.01, integer: false });
    expect(sliderRange(at('/parts/0/deform/taper/to'), doc)).toEqual(range);
  });

  it('noise `amplitude` and `scale` get their own narrow ranges', () => {
    expect(sliderRange(at('/parts/0/noise/amplitude'), doc)).toEqual({
      min: 0,
      max: 0.2,
      step: 0.001,
      integer: false,
    });
    expect(sliderRange(at('/parts/0/noise/scale'), doc)).toEqual({
      min: 0.01,
      max: 1,
      step: 0.01,
      integer: false,
    });
  });

  it('a bone\'s `head`/`tail` reuses a part\'s `at` range shape: model-scaled, symmetric about zero', () => {
    const head = sliderRange(at('/bones/1/head/1'), doc); // arm head.y = 1.0
    const partAt = sliderRange({ pointer: '/x', key: 'at', value: 0, line: 0, column: 0, length: 1 }, doc);
    expect(head).toEqual(partAt);
    expect(head.min).toBeLessThan(0);
    expect(head.max).toBeGreaterThan(0);
    // The extent scales with the bone's own position (1.6, the tail), not just the parts.
    expect(head.max).toBeGreaterThanOrEqual(1.6);
  });
});
