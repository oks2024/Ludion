import { describe, expect, it } from 'vitest';
import { parseSculptDoc } from './doc';
import { sliderRange } from './ranges';
import { inspectorGroups, quantize, rowLabel } from './rows';

// The real gallery idol, for the same reason `doc.test.ts` uses it: an authored part order, mirrored
// and rotated parts, and a six-entry palette are what the layout has to survive.
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

const doc = parseSculptDoc(idolSource());

describe('rowLabel', () => {
  const byPointer = (pointer: string) => {
    const field = doc.fields.find((candidate) => candidate.pointer === pointer);
    expect(field, pointer).toBeDefined();
    return rowLabel(field!);
  };

  it('names a scalar by its key alone', () => {
    expect(byPointer('/resolution')).toBe('resolution');
    expect(byPointer('/palette/E/emissive')).toBe('emissive');
  });

  it('names a vector component by axis', () => {
    expect(byPointer('/parts/0/at/0')).toBe('at.x');
    expect(byPointer('/parts/0/at/1')).toBe('at.y');
    expect(byPointer('/parts/0/size/2')).toBe('size.z');
  });

  it('names a colour component by channel', () => {
    expect(byPointer('/palette/E/color/0')).toBe('color.r');
    expect(byPointer('/palette/E/color/3')).toBe('color.a');
  });
});

describe('inspectorGroups', () => {
  const groups = inspectorGroups(doc);

  it('lays out parts first, then the palette, then the document numbers', () => {
    const ids = groups.map((group) => group.id);
    expect(ids.slice(0, 14)).toEqual([...Array(14).keys()].map((i) => `part:${i}`));
    expect(ids.slice(14, 20)).toEqual(['B', 'D', 'E', 'J', 'K', 'T'].map((g) => `palette:${g}`));
    expect(ids.at(-1)).toBe('model');
  });

  it('heads a part with its name and shape', () => {
    expect(groups[0].head).toBe('plinth · box');
    expect(groups.find((group) => group.id === 'palette:E')?.head).toBe('palette · E');
  });

  // File order is evaluation order: a later part blends against the ones before it, so a sorted
  // inspector would disagree with the file the human is also reading.
  it('keeps a part’s fields in file order', () => {
    expect(groups[0].rows.map((row) => row.field.pointer)).toEqual(
      doc.parts[0].fields.map((field) => field.pointer),
    );
  });

  it('puts resolution and triangles in the last group', () => {
    expect(groups.at(-1)?.rows.map((row) => row.label)).toEqual(['resolution', 'triangles']);
  });

  it('covers every numeric field exactly once', () => {
    const laid = groups.flatMap((group) => group.rows.map((row) => row.field.pointer));
    expect(new Set(laid).size).toBe(laid.length);
    expect(new Set(laid)).toEqual(new Set(doc.fields.map((field) => field.pointer)));
  });

  it('drops a group with nothing to drag rather than rendering it empty', () => {
    const bare = parseSculptDoc('kind: sculpt\nname: bare\nparts:\n  - name: a\n    shape: sphere\n');
    expect(inspectorGroups(bare).every((group) => group.rows.length > 0)).toBe(true);
  });
});

describe('quantize', () => {
  const field = (pointer: string) => {
    const found = doc.fields.find((candidate) => candidate.pointer === pointer);
    expect(found, pointer).toBeDefined();
    return found!;
  };

  // The defect this exists to prevent: a range input's own arithmetic writing float residue into a
  // hand-authored file.
  it('lands a dragged value on a clean decimal', () => {
    const range = sliderRange(field('/palette/E/roughness'), doc);
    expect(String(quantize(0.1 + 0.2, range))).toBe('0.3');
  });

  // The coordinate ranges scale with the model, so their step is not a round number — the value
  // still has to come out readable rather than carrying the snap's own residue.
  it('leaves no residue on a range whose step is not round', () => {
    const range = sliderRange(field('/parts/0/at/1'), doc);
    expect(String(quantize(0.1 + 0.2, range))).not.toMatch(/\d{8}/);
  });

  it('rounds an integer key to a whole number', () => {
    const range = sliderRange(field('/resolution'), doc);
    expect(quantize(47.6, range)).toBe(48);
    expect(Number.isInteger(quantize(47.6, range))).toBe(true);
  });

  it('clamps to the range on both ends', () => {
    const range = sliderRange(field('/palette/E/roughness'), doc);
    expect(quantize(-5, range)).toBe(0);
    expect(quantize(9, range)).toBe(1);
  });

  it('answers a non-finite value with the range floor rather than NaN', () => {
    const range = sliderRange(field('/palette/E/roughness'), doc);
    expect(quantize(Number.NaN, range)).toBe(0);
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

describe('rowLabel — nested deform/noise fields and a bone\'s head/tail (Lore rev 742)', () => {
  const rig = parseSculptDoc(RIG);
  const byPointer = (pointer: string) => {
    const field = rig.fields.find((candidate) => candidate.pointer === pointer);
    expect(field, pointer).toBeDefined();
    return rowLabel(field!);
  };

  it('names a `deform` turn/taper leaf "kind · leaf", not the bare key', () => {
    expect(byPointer('/parts/0/deform/bend/degrees')).toBe('bend · degrees');
    expect(byPointer('/parts/0/deform/twist/degrees')).toBe('twist · degrees');
    expect(byPointer('/parts/0/deform/taper/from')).toBe('taper · from');
    expect(byPointer('/parts/0/deform/taper/to')).toBe('taper · to');
  });

  it('names a `noise` field "noise · leaf"', () => {
    expect(byPointer('/parts/0/noise/amplitude')).toBe('noise · amplitude');
    expect(byPointer('/parts/0/noise/scale')).toBe('noise · scale');
  });

  it('names a bone\'s head/tail component "head · x", not "head.x"', () => {
    expect(byPointer('/bones/1/head/0')).toBe('head · x');
    expect(byPointer('/bones/1/head/1')).toBe('head · y');
    expect(byPointer('/bones/1/tail/2')).toBe('tail · z');
  });
});

describe('inspectorGroups — bones (Lore rev 742)', () => {
  const rig = parseSculptDoc(RIG);
  const groups = inspectorGroups(rig);

  it('groups a bone right after the parts (before the model group), dropping an empty bone (`root`)', () => {
    expect(groups.map((g) => g.id)).toEqual(['part:0', 'bone:1', 'model']);
  });

  it('heads the bone group with its name, and keeps head-then-tail file order', () => {
    const bone = groups.find((g) => g.id === 'bone:1')!;
    expect(bone.head).toBe('arm');
    expect(bone.rows.map((r) => r.label)).toEqual([
      'head · x',
      'head · y',
      'head · z',
      'tail · x',
      'tail · y',
      'tail · z',
    ]);
  });
});
