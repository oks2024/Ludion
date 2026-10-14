// Palette swatches: a voxel palette entry's authored color as something a screen can show.
//
// The numbers in a `.voxel.yaml` palette are LINEAR albedo — what the shader multiplies — and a CSS
// color is sRGB-encoded. Handing 0.06 straight to `rgb()` would paint a near-black chip for a stone
// that reads as mid-gray in the viewport, which is exactly the misjudgment this whole view exists to
// remove. So the swatch encodes to sRGB, the same transfer function the renderer's output writes.
//
// This is a DISPLAY transform and nothing else: it never rounds the authored value, never writes
// back, and never judges the entry (contract 2 — one checker, the engine's).
//
// Pure: no React, no Tauri, no DOM.

import type { PaletteEntry } from './doc';

/** Linear → sRGB, the IEC 61966-2-1 encoding curve, on one channel of a 0..1 value. */
function encodeSrgb(linear: number): number {
  const c = Math.min(1, Math.max(0, linear));
  return c <= 0.0031308 ? 12.92 * c : 1.055 * Math.pow(c, 1 / 2.4) - 0.055;
}

/**
 * The CSS color for a palette entry's `color`, as `rgb(r, g, b)` in 0–255.
 *
 * Alpha is deliberately dropped: a swatch drawn over the panel background would read a translucent
 * glyph as a lighter one, and the judgment the swatch supports is about value, not coverage. Missing
 * components read as 0 — a half-typed palette still paints something rather than throwing.
 */
export function swatchCss(color: readonly { readonly value: number }[] | readonly number[]): string {
  const channel = (index: number): number => {
    const entry = color[index] as number | { readonly value: number } | undefined;
    const linear = typeof entry === 'number' ? entry : (entry?.value ?? 0);
    return Math.round(encodeSrgb(Number.isFinite(linear) ? linear : 0) * 255);
  };
  return `rgb(${channel(0)}, ${channel(1)}, ${channel(2)})`;
}

/**
 * The same encoding as `swatchCss`, written as `#rrggbb` — what an `<input type="color">` takes.
 *
 * The picker's inverse lives in `palette.ts` (the write side); this direction is display, like every
 * other function here. Alpha is dropped for the same reason `swatchCss` drops it: the picker has no
 * alpha channel, and a palette edit patches `[r, g, b, null]` so the authored alpha is never touched.
 */
export function swatchHex(color: readonly { readonly value: number }[] | readonly number[]): string {
  const hex = (index: number): string => {
    const entry = color[index] as number | { readonly value: number } | undefined;
    const linear = typeof entry === 'number' ? entry : (entry?.value ?? 0);
    return Math.round(encodeSrgb(Number.isFinite(linear) ? linear : 0) * 255)
      .toString(16)
      .padStart(2, '0');
  };
  return `#${hex(0)}${hex(1)}${hex(2)}`;
}

/** True when the swatch is dark enough that its glyph label needs a light foreground. Uses the
 *  encoded (perceptual) luminance, since that is what the eye sees on the chip. */
export function swatchIsDark(color: readonly { readonly value: number }[] | readonly number[]): boolean {
  const value = (index: number): number => {
    const entry = color[index] as number | { readonly value: number } | undefined;
    const linear = typeof entry === 'number' ? entry : (entry?.value ?? 0);
    return encodeSrgb(Number.isFinite(linear) ? linear : 0);
  };
  return 0.2126 * value(0) + 0.7152 * value(1) + 0.0722 * value(2) < 0.5;
}

/**
 * The swatch's hover title: the glyph, its authored linear color, and whichever material fields the
 * entry declares. Authored numbers verbatim — the tooltip is how a value judgment turns into a
 * sentence for the persona ("`E` is 0.9/0.75/0.35, emissive 1.4"), so it must not reformat them.
 */
export function swatchTitle(entry: PaletteEntry): string {
  const parts = [`${entry.glyph} · ${entry.color.map((c) => c.value).join(' ')}`];
  if (entry.emissive !== null) {
    parts.push(`emissive ${entry.emissive.value}`);
  }
  if (entry.metallic !== null) {
    parts.push(`metallic ${entry.metallic.value}`);
  }
  if (entry.roughness !== null) {
    parts.push(`roughness ${entry.roughness.value}`);
  }
  return parts.join(' · ');
}
