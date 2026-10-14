import { describe, expect, it } from 'vitest';
import { EMPTY_GLYPH, paletteEntry, parseVoxelDoc, type PaletteEntry } from './doc';
import { swatchCss, swatchIsDark } from './swatch';
import { cellView, glyphViews, sameGlyphViews, sameRows, type GlyphViews } from './cellview';

// The frozen stage library, like every other test in this directory: the palettes these views are
// derived from are the authored ones, mixed material scalars and all.
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

const BRAZIER = parseVoxelDoc(model('brazier.voxel.yaml'));

/** The look the painter derived per cell before the palette was precomputed — the pin the fast path
 *  has to reproduce exactly. */
function reference(cell: string, ghost: string) {
  const filled = cell !== EMPTY_GLYPH;
  const shown = filled ? cell : ghost;
  const entry = shown === EMPTY_GLYPH ? null : paletteEntry(BRAZIER, shown);
  if (filled) {
    const dark = entry !== null && swatchIsDark(entry.color) ? ' dark' : '';
    return {
      classes: `vx-cell filled${dark}`,
      style: entry === null ? undefined : { background: swatchCss(entry.color) },
      text: cell,
    };
  }
  if (ghost === EMPTY_GLYPH) {
    return { classes: 'vx-cell', style: undefined, text: '' };
  }
  return {
    classes: 'vx-cell onion',
    style: entry === null ? undefined : { color: swatchCss(entry.color) },
    text: ghost,
  };
}

describe('glyphViews', () => {
  it('carries one view per palette glyph', () => {
    const views = glyphViews(BRAZIER.palette);
    expect(views.size).toBe(BRAZIER.palette.length);
    for (const entry of BRAZIER.palette) {
      expect(views.get(entry.glyph)?.fill).toEqual({ background: swatchCss(entry.color) });
      expect(views.get(entry.glyph)?.ink).toEqual({ color: swatchCss(entry.color) });
    }
  });

  it('marks a dark swatch so its glyph label flips', () => {
    const views = glyphViews(BRAZIER.palette);
    for (const entry of BRAZIER.palette) {
      const expected = swatchIsDark(entry.color) ? 'vx-cell filled dark' : 'vx-cell filled';
      expect(views.get(entry.glyph)?.classes).toBe(expected);
    }
  });

  it('shares one style object across every cell drawing the glyph', () => {
    const views = glyphViews(BRAZIER.palette);
    const glyph = BRAZIER.palette[0].glyph;
    expect(cellView(glyph, EMPTY_GLYPH, views).style).toBe(cellView(glyph, EMPTY_GLYPH, views).style);
  });

  it('keeps the FIRST of a duplicated glyph, like paletteEntry', () => {
    const entry = (glyph: string, red: number): PaletteEntry => ({
      ...BRAZIER.palette[0],
      glyph,
      color: [{ value: red, span: BRAZIER.palette[0].color[0].span }, ...BRAZIER.palette[0].color.slice(1)],
    });
    const views = glyphViews([entry('#', 1), entry('#', 0)]);
    expect(views.size).toBe(1);
    expect(views.get('#')?.fill).toEqual({ background: swatchCss(entry('#', 1).color) });
  });
});

describe('cellView', () => {
  const views = glyphViews(BRAZIER.palette);
  const glyphs = [EMPTY_GLYPH, ...BRAZIER.palette.map((entry) => entry.glyph), '?'];

  it('reproduces the per-cell derivation for every glyph pair', () => {
    for (const cell of glyphs) {
      for (const ghost of glyphs) {
        expect(cellView(cell, ghost, views), `${cell} over ${ghost}`).toEqual(reference(cell, ghost));
      }
    }
  });

  it('draws a glyph the palette does not name rather than dropping it', () => {
    expect(cellView('?', EMPTY_GLYPH, views)).toEqual({
      classes: 'vx-cell filled',
      style: undefined,
      text: '?',
    });
  });

  it('ignores the onion-skin glyph under a filled cell', () => {
    const glyph = BRAZIER.palette[0].glyph;
    expect(cellView(glyph, BRAZIER.palette[1].glyph, views)).toEqual(cellView(glyph, EMPTY_GLYPH, views));
  });
});

describe('sameRows', () => {
  it('is true for the same array', () => {
    const rows = ['..A', 'A..'];
    expect(sameRows(rows, rows)).toBe(true);
  });

  it('is true for equal content in fresh arrays — a rebuilt document that did not move the layer', () => {
    expect(sameRows(['..A', 'A..'], ['..A', 'A..'])).toBe(true);
  });

  it('is false when a single cell moved', () => {
    expect(sameRows(['..A', 'A..'], ['..A', 'A.A'])).toBe(false);
  });

  it('is false on a depth change', () => {
    expect(sameRows(['..A'], ['..A', 'A..'])).toBe(false);
  });

  it('treats null (no onion layer) as its own value', () => {
    expect(sameRows(null, null)).toBe(true);
    expect(sameRows(null, ['..A'])).toBe(false);
    expect(sameRows(['..A'], null)).toBe(false);
  });
});

describe('sameGlyphViews', () => {
  const views: GlyphViews = glyphViews(BRAZIER.palette);

  it('is true for a map rebuilt from the same palette', () => {
    expect(sameGlyphViews(views, glyphViews(BRAZIER.palette))).toBe(true);
  });

  it('is false when a colour moved', () => {
    const [first, ...rest] = BRAZIER.palette;
    const retinted: PaletteEntry = {
      ...first,
      color: [{ value: first.color[0].value + 0.5, span: first.color[0].span }, ...first.color.slice(1)],
    };
    expect(sameGlyphViews(views, glyphViews([retinted, ...rest]))).toBe(false);
  });

  it('is false when an entry is added or removed', () => {
    expect(sameGlyphViews(views, glyphViews(BRAZIER.palette.slice(1)))).toBe(false);
  });

  it('is false when a glyph was renamed', () => {
    const [first, ...rest] = BRAZIER.palette;
    expect(sameGlyphViews(views, glyphViews([{ ...first, glyph: '~' }, ...rest]))).toBe(false);
  });
});
