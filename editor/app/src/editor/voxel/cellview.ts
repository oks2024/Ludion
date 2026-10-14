// The slice painter's per-cell display data, derived once per PALETTE rather than once per cell.
//
// The painter draws one `<span>` per cell, and a 128×112 layer is 14,336 of them. Reading each one's
// colour off its palette entry cost a linear palette scan plus up to six `Math.pow` calls (the sRGB
// encode in `swatch.ts`) — per cell, per render. A palette is a dozen entries, so the encode belongs
// here: once per entry, shared by every cell that names it. The style objects are shared too, which
// is what lets the grid memoize on the view map's identity.
//
// The equality predicates exist for the same reason. The painter's output is a function of the drawn
// rows, the onion-skin rows, and these views — nothing else — so a document rebuilt without any of
// the three moving must not rebuild 14k spans.
//
// This module renders nothing and validates nothing: a glyph the palette does not name still draws
// (contract 2 — one checker, the engine's).
//
// Pure: no React (the CSS type is erased), no Tauri, no DOM.

import type { CSSProperties } from 'react';
import { EMPTY_GLYPH, type PaletteEntry } from './doc';
import { swatchCss, swatchIsDark } from './swatch';

/** One palette glyph's look, in the two roles a cell can draw it in. */
export interface GlyphView {
  /** A filled cell's classes, with `dark` folded in when the swatch needs a light glyph label. */
  readonly classes: string;
  /** A filled cell's background. */
  readonly fill: CSSProperties;
  /** An onion-skin cell's ink. */
  readonly ink: CSSProperties;
}

export type GlyphViews = ReadonlyMap<string, GlyphView>;

/** How one cell draws: the class list, the inline style (shared, never per-cell), and its label. */
export interface CellView {
  readonly classes: string;
  readonly style: CSSProperties | undefined;
  readonly text: string;
}

const EMPTY_CELL: CellView = { classes: 'vx-cell', style: undefined, text: '' };

/** One view per palette entry. The FIRST entry wins a duplicated glyph, matching `paletteEntry`'s
 *  own read of the same palette. */
export function glyphViews(palette: readonly PaletteEntry[]): GlyphViews {
  const views = new Map<string, GlyphView>();
  for (const entry of palette) {
    if (views.has(entry.glyph)) {
      continue;
    }
    const css = swatchCss(entry.color);
    views.set(entry.glyph, {
      classes: swatchIsDark(entry.color) ? 'vx-cell filled dark' : 'vx-cell filled',
      fill: { background: css },
      ink: { color: css },
    });
  }
  return views;
}

/** One cell's look: its own glyph when filled, else the onion-skin glyph from the layer below. */
export function cellView(cell: string, ghost: string, views: GlyphViews): CellView {
  if (cell !== EMPTY_GLYPH) {
    const view = views.get(cell);
    return { classes: view?.classes ?? 'vx-cell filled', style: view?.fill, text: cell };
  }
  if (ghost === EMPTY_GLYPH) {
    return EMPTY_CELL;
  }
  return { classes: 'vx-cell onion', style: views.get(ghost)?.ink, text: ghost };
}

/** True when two slice reads carry the same characters. */
export function sameRows(a: readonly string[] | null, b: readonly string[] | null): boolean {
  if (a === b) {
    return true;
  }
  if (a === null || b === null || a.length !== b.length) {
    return false;
  }
  return a.every((row, index) => row === b[index]);
}

/** True when two view maps would paint identically. `classes` carries the dark flag and the fill
 *  carries the colour, so those two are the whole visible difference. */
export function sameGlyphViews(a: GlyphViews, b: GlyphViews): boolean {
  if (a === b) {
    return true;
  }
  if (a.size !== b.size) {
    return false;
  }
  for (const [glyph, view] of a) {
    const other = b.get(glyph);
    if (other === undefined || other.classes !== view.classes || other.fill.background !== view.fill.background) {
      return false;
    }
  }
  return true;
}
