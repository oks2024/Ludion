// Which control a recipe number gets, and what range it spans.
//
// A RECIPE'S KEYS ARE THE GRAPH'S, NOT THE ENGINE'S. `sculpt/ranges.ts` can be exhaustive over its
// format's key set; this cannot — an input name comes from whatever node class the workflow happens
// to use, so the set is open by construction. The answer is not a guessed slider: a key this module
// knows gets a drag surface with a range chosen to keep the drag USEFUL, and everything else gets a
// typed number box, which is honest about there being no sensible travel to offer.
//
// A SEED IS NEVER A SLIDER. `seed`/`noise_seed` span the whole 64-bit space and a candidate's index
// IS its seed offset (`ludion docs generation`), so the value is a pick, not a tunable — the same
// reasoning that keeps a sculpt `noise.seed` off a slider.
//
// `gen check` still has the final word on whether a value is legal (`L1165` is the input's declared
// type or enum refusing it); a control is a drag surface, not a validator.
//
// Pure: no React, no Tauri, no DOM.

import type { RecipeNumber } from './doc';

/** A slider's bounds and granularity. */
export interface SliderRange {
  readonly min: number;
  readonly max: number;
  readonly step: number;
}

/** What the inspector renders for one number. */
export type NumberControl =
  | { readonly kind: 'slider'; readonly range: SliderRange }
  /** A typed box: `step` is the arrow-key increment, `min` a floor where one is meaningful. */
  | { readonly kind: 'number'; readonly step: number; readonly min: number | null };

/** Image dimensions and bake resolutions: the generators want multiples of 64, and a 64-step drag
 *  from 256 to 2048 is 28 stops — coarse enough that every stop is a value worth submitting. */
const PIXELS: SliderRange = { min: 256, max: 2048, step: 64 };

/** The known keys, each with the travel that makes its drag useful. Anything absent here falls to a
 *  number box; adding a row is how a new workflow's knob earns a slider. */
const SLIDERS: Readonly<Record<string, SliderRange>> = {
  // `count` is candidates per run, and each one is a separate job on the server.
  count: { min: 1, max: 16, step: 1 },
  width: PIXELS,
  height: PIXELS,
  resolution: PIXELS,
  texture_size: PIXELS,
  batch_size: { min: 1, max: 8, step: 1 },
  steps: { min: 1, max: 60, step: 1 },
  cfg: { min: 0, max: 20, step: 0.1 },
  guidance: { min: 0, max: 20, step: 0.1 },
  denoise: { min: 0, max: 1, step: 0.01 },
  strength: { min: 0, max: 2, step: 0.01 },
};

/** Keys whose value is a pick rather than a tunable — see the module doc. */
const PICKS = new Set(['seed', 'noise_seed']);

/** The control for `field`. Total over every key `doc.ts` can produce. */
export function controlFor(field: RecipeNumber): NumberControl {
  if (PICKS.has(field.key)) {
    return { kind: 'number', step: 1, min: 0 };
  }
  const range = SLIDERS[field.key];
  if (range !== undefined) {
    return { kind: 'slider', range };
  }
  return { kind: 'number', step: field.integer ? 1 : 0.01, min: null };
}

/** `value` snapped to `range`'s step and clamped into it — the value a drag is allowed to write.
 *
 *  The step grid is anchored at `min`, not at zero, so a `cfg` slider from 0 in steps of 0.1 cannot
 *  land on 7.000000000000001 and write that into the file. */
export function quantize(value: number, range: SliderRange): number {
  if (!Number.isFinite(value)) {
    return range.min;
  }
  const clamped = Math.min(range.max, Math.max(range.min, value));
  const steps = Math.round((clamped - range.min) / range.step);
  const snapped = range.min + steps * range.step;
  // The multiply reintroduces float error for a fractional step; round it off at the step's own
  // precision so the written token is as short as the step is.
  const decimals = decimalsOf(range.step);
  return Number(snapped.toFixed(decimals));
}

function decimalsOf(step: number): number {
  const text = String(step);
  const dot = text.indexOf('.');
  return dot < 0 ? 0 : text.length - dot - 1;
}
