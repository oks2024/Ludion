import { describe, expect, it } from 'vitest';
import { parseVoxelDoc } from './doc';
import { swatchCss, swatchIsDark, swatchTitle } from './swatch';

const n = (value: number) => ({ value });

describe('swatchCss', () => {
  it('encodes linear albedo to sRGB — the whole reason the chip is legible', () => {
    // 0.06 linear is a mid-dark stone in the viewport, not the near-black `rgb(15,…)` a raw
    // ×255 would paint.
    expect(swatchCss([n(0.06), n(0.06), n(0.06)])).toBe('rgb(69, 69, 69)');
    expect(swatchCss([n(0), n(0), n(0)])).toBe('rgb(0, 0, 0)');
    expect(swatchCss([n(1), n(1), n(1)])).toBe('rgb(255, 255, 255)');
    // The linear-segment end of the curve.
    expect(swatchCss([n(0.0031308), n(0), n(0)])).toBe('rgb(10, 0, 0)');
  });

  it('clamps out-of-range values and reads a short or non-finite palette as black', () => {
    expect(swatchCss([n(2), n(-1), n(Number.NaN)])).toBe('rgb(255, 0, 0)');
    expect(swatchCss([])).toBe('rgb(0, 0, 0)');
  });

  it('accepts plain numbers as well as parsed number fields', () => {
    expect(swatchCss([0.5, 0.5, 0.5])).toBe(swatchCss([n(0.5), n(0.5), n(0.5)]));
  });

  it('ignores alpha — a translucent chip would misreport the value it exists to show', () => {
    expect(swatchCss([n(0.2), n(0.4), n(0.6), n(0.1)])).toBe(swatchCss([n(0.2), n(0.4), n(0.6)]));
  });
});

describe('swatchIsDark', () => {
  it('picks the glyph foreground off perceptual luminance', () => {
    expect(swatchIsDark([n(0.02), n(0.02), n(0.02)])).toBe(true);
    expect(swatchIsDark([n(0.9), n(0.9), n(0.9)])).toBe(false);
    // Green dominates luminance: the same linear value reads light in green, dark in blue.
    expect(swatchIsDark([n(0), n(0.5), n(0)])).toBe(false);
    expect(swatchIsDark([n(0), n(0), n(0.5)])).toBe(true);
  });
});

describe('swatchTitle', () => {
  const SOURCE = [
    'format: 1',
    'kind: voxel',
    'name: eyes',
    'size:',
    '  - 1',
    '  - 1',
    '  - 1',
    'palette:',
    '  "E":',
    '    color:',
    '      - 0.9',
    '      - 0.75',
    '      - 0.35',
    '      - 1.0',
    '    emissive: 1.4',
    '    roughness: 0.35',
    '  ".x":',
    '    color:',
    '      - 0.1',
    '      - 0.1',
    '      - 0.1',
    '      - 1.0',
    'slices:',
    '  - - "E"',
    '',
  ].join('\n');

  it('reports the authored numbers verbatim, with only the fields the entry declares', () => {
    const doc = parseVoxelDoc(SOURCE);
    expect(swatchTitle(doc.palette[0])).toBe('E · 0.9 0.75 0.35 1 · emissive 1.4 · roughness 0.35');
    expect(swatchTitle(doc.palette[1])).toBe('.x · 0.1 0.1 0.1 1');
  });
});
