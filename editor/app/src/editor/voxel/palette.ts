// The palette editor's pure half: what a color picker and three material sliders may write, and
// what they must refuse.
//
// WHY THIS EXISTS AT ALL. G7's voxel-authoring report ends at "a competent model lands in 1–2
// renders; color taste is the only loop left" — every extra render across the whole six-model series
// was palette-value calibration. The shipped fix was a guide paragraph telling the author to write
// albedo 2–3× darker than the target read, because the sheet rig is a bright neutral key+fill while
// vigil consumes these models at night. This module is the other half of retiring that arithmetic:
// drag the color, watch the model under `env-night.yaml`.
//
// QUANTIZATION IS THE CALLER'S JOB, AND THIS IS THE CALLER. `edit.ts` writes shortest-round-trip
// floats (CLAUDE.md calls fixed precision a data-loss bug), so a picker handing it `128/255` decoded
// to linear would write `0.21586050011389923` into a file whose authored neighbours read `0.15`,
// `0.20`, `0.95`. Every value this module produces goes through `quantize` first — the shared one in
// `viewer/slider.ts`, which carries the measurement behind its significant-digit rounding (this
// palette's near-black range is what measured it).
//
// REFUSALS ARE SURFACED, NOT WORKED AROUND. `edit.ts` deliberately refuses two things, and this
// module turns each into a disabled control carrying the reason rather than a control that fails
// when clicked: a material key the entry does not declare (inserting it is the `addGlyph` problem —
// where does the line go, at what indent — for a case no authored model needs), and a `color` whose
// components are not four addressable numbers (editing by index through a short list would write red
// into green).
//
// Pure: no React, no Tauri, no DOM.

import { quantize } from '../../viewer/slider';
import type { PaletteEntry } from './doc';
import type { MaterialField } from './edit';
import { swatchHex } from './swatch';

/** The optional material scalars, in the canonical key order `emit_voxel` writes them. */
export const MATERIAL_FIELDS: readonly MaterialField[] = ['emissive', 'metallic', 'roughness'];

/** sRGB → linear, the inverse of `swatch.ts`'s display encoding: what the picker's 8-bit channel
 *  means as the albedo the shader multiplies. */
function decodeSrgb(encoded: number): number {
  const c = Math.min(1, Math.max(0, encoded));
  return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}

const HEX = /^#?([0-9a-f]{6})$/i;

/** An `<input type="color">` value as quantized linear `r, g, b`, or `null` when it is not a
 *  six-digit hex color. Alpha is not a picker channel and is never derived here. */
export function hexToLinear(hex: string): readonly [number, number, number] | null {
  const match = HEX.exec(hex.trim());
  if (match === null) {
    return null;
  }
  const packed = Number.parseInt(match[1], 16);
  const channel = (shift: number): number => quantize(decodeSrgb(((packed >> shift) & 0xff) / 255));
  return [channel(16), channel(8), channel(0)];
}

/** The slider bounds for a material field. */
export interface MaterialRange {
  readonly min: number;
  readonly max: number;
  readonly step: number;
}

/** `emissive` is unbounded in the format and models author it well past 1 (the gallery beacon is 4.5), so
 *  its nominal ceiling is generous and `materialRange` widens it further rather than clipping an
 *  authored value the moment its slider is touched. `metallic`/`roughness` are 0..1 by definition. */
const NOMINAL: Readonly<Record<MaterialField, MaterialRange>> = {
  emissive: { min: 0, max: 8, step: 0.05 },
  metallic: { min: 0, max: 1, step: 0.01 },
  roughness: { min: 0, max: 1, step: 0.01 },
};

/** The field's range, widened to contain `value` — a slider must never silently rewrite the number
 *  it was opened on. */
export function materialRange(field: MaterialField, value: number): MaterialRange {
  const nominal = NOMINAL[field];
  if (!Number.isFinite(value)) {
    return nominal;
  }
  return {
    ...nominal,
    min: Math.min(nominal.min, value),
    max: Math.max(nominal.max, value),
  };
}

/** A control that can be rendered live, or one that must be disabled with the reason shown. */
export type Control<T> = ({ readonly editable: true } & T) | { readonly editable: false; readonly reason: string };

/**
 * The color picker's state for an entry.
 *
 * Disabled exactly when `setPaletteColor` would refuse: the entry's `color` is not four addressable
 * numbers (an absent component, or one `doc.ts` could not span). The reason says so in the same
 * words the edit would have, so the panel and the status bar never tell two stories.
 */
export function colorControl(entry: PaletteEntry): Control<{ readonly hex: string }> {
  if (entry.color.length !== 4) {
    return {
      editable: false,
      reason: `\`${entry.glyph}\` has ${entry.color.length} addressable color components, not 4 — edit its \`color\` in the file.`,
    };
  }
  return { editable: true, hex: swatchHex(entry.color) };
}

/**
 * A material slider's state for an entry.
 *
 * Disabled exactly when `setPaletteMaterial` would refuse: the entry declares no such key, so there
 * is no number to replace and inserting one is out of the surgical editor's contract. The reason
 * names the way forward (the file, or the persona), because a greyed control with no explanation is
 * the failure mode this arc exists to remove.
 */
export function materialControl(
  entry: PaletteEntry,
  field: MaterialField,
): Control<{ readonly value: number; readonly range: MaterialRange }> {
  const declared = entry[field];
  if (declared === null) {
    return {
      editable: false,
      reason: `\`${entry.glyph}\` declares no \`${field}\`. The surgical editor only rewrites numbers that are already there — add the key in the file, or ask the persona.`,
    };
  }
  return { editable: true, value: declared.value, range: materialRange(field, declared.value) };
}
