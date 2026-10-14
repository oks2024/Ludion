import { describe, expect, it } from 'vitest';
import { parseVoxelDoc, paletteEntry, type PaletteEntry } from './doc';
import { setPaletteColor, setPaletteMaterial } from './edit';
import { colorControl, hexToLinear, materialControl, materialRange, MATERIAL_FIELDS } from './palette';
import { swatchHex } from './swatch';

// The frozen stage library, like `doc.test.ts` and `edit.test.ts`: the palettes this editor exists to
// calibrate are the hand-authored ones, with their mixed optional material scalars.
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
const BRAZIER = parseVoxelDoc(model('brazier.voxel.yaml'));

function entry(glyph: string): PaletteEntry {
  const found = paletteEntry(BRAZIER, glyph);
  expect(found, `brazier has no \`${glyph}\` entry`).not.toBeNull();
  return found as PaletteEntry;
}

// `quantize` itself is tested where it lives (`viewer/slider.test.ts`). What this file still owns is
// the measurement that chose its digits: the round trip below is what makes 3 significant digits the
// answer and 3 decimals the wrong one.
describe('hexToLinear ↔ swatchHex', () => {
  it('round-trips every 8-bit level exactly (the measurement the digit choice rests on)', () => {
    for (let level = 0; level < 256; level += 1) {
      const hex = `#${level.toString(16).padStart(2, '0').repeat(3)}`;
      const linear = hexToLinear(hex);
      expect(linear, hex).not.toBeNull();
      expect(swatchHex(linear as readonly number[])).toBe(hex);
    }
  });

  it('round-trips saturated and mixed colors', () => {
    for (const hex of ['#000000', '#ffffff', '#6e7bff', '#4c1a08', '#0a0f03']) {
      expect(swatchHex(hexToLinear(hex) as readonly number[])).toBe(hex);
    }
  });

  it('accepts a hex with or without the leading #, any case', () => {
    expect(hexToLinear('6E7BFF')).toEqual(hexToLinear('#6e7bff'));
  });

  it('refuses anything that is not a six-digit hex color', () => {
    for (const bad of ['', '#fff', 'rgb(1,2,3)', '#12345g', '#1234567']) {
      expect(hexToLinear(bad)).toBeNull();
    }
  });

  it('reads the swatch of every authored entry back as its own hex', () => {
    for (const name of NAMES) {
      for (const authored of parseVoxelDoc(model(name)).palette) {
        const hex = swatchHex(authored.color);
        expect(hexToLinear(hex), `${name} ${authored.glyph}`).not.toBeNull();
        expect(swatchHex(hexToLinear(hex) as readonly number[])).toBe(hex);
      }
    }
  });
});

describe('materialRange', () => {
  it('is 0..1 for the two scalars the format bounds', () => {
    expect(materialRange('metallic', 0.4)).toEqual({ min: 0, max: 1, step: 0.01 });
    expect(materialRange('roughness', 0.95)).toEqual({ min: 0, max: 1, step: 0.01 });
  });

  it('widens rather than clipping an authored value out of the nominal range', () => {
    // an authored emissive may sit above the nominal range; nothing may rewrite that just because a slider was touched.
    expect(materialRange('emissive', 12).max).toBe(12);
    expect(materialRange('emissive', 5.5).max).toBe(8);
    expect(materialRange('roughness', -0.2).min).toBe(-0.2);
  });

  it('falls back to the nominal range for a value that is not a number', () => {
    expect(materialRange('emissive', Number.NaN)).toEqual({ min: 0, max: 8, step: 0.05 });
  });
});

describe('colorControl — surfacing edit.ts’s color refusal', () => {
  it('is editable for a well-formed entry, at the entry’s own hex', () => {
    const control = colorControl(entry('I'));
    expect(control.editable).toBe(true);
    if (control.editable) {
      expect(control.hex).toBe(swatchHex(entry('I').color));
    }
  });

  it('is disabled exactly when setPaletteColor would refuse, with a reason', () => {
    const doc = parseVoxelDoc(
      ['format: 1', 'kind: voxel', 'palette:', '  X:', '    color:', '      - 0.5', '      - 0.5', ''].join('\n'),
    );
    const control = colorControl(doc.palette[0]);
    expect(control.editable).toBe(false);
    if (!control.editable) {
      expect(control.reason).toContain('2 addressable color components');
    }
    // The refusal it is reporting is real.
    const refused = setPaletteColor(doc, 'X', [0.1, null, null, null]);
    expect(refused.ok).toBe(false);
  });
});

describe('materialControl — surfacing edit.ts’s missing-key refusal', () => {
  it('is editable for a declared field', () => {
    const control = materialControl(entry('C'), 'roughness');
    expect(control).toEqual({ editable: true, value: 0.95, range: { min: 0, max: 1, step: 0.01 } });
  });

  it('is disabled for an undeclared field, and says why', () => {
    const control = materialControl(entry('C'), 'emissive');
    expect(control.editable).toBe(false);
    if (!control.editable) {
      expect(control.reason).toContain('declares no `emissive`');
    }
  });

  it('agrees with setPaletteMaterial on every entry of every authored model', () => {
    for (const name of NAMES) {
      const doc = parseVoxelDoc(model(name));
      for (const authored of doc.palette) {
        for (const field of MATERIAL_FIELDS) {
          const control = materialControl(authored, field);
          const attempted = setPaletteMaterial(doc, authored.glyph, field, 0.5);
          expect(control.editable, `${name} ${authored.glyph} ${field}`).toBe(attempted.ok);
        }
      }
    }
  });
});
