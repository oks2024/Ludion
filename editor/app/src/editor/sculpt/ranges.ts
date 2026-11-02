// The slider range for every numeric field `doc.ts` can produce — one `sliderRange` call per
// `NumberField`, so the inspector never special-cases a key itself.
//
// Ranges come from `Plan/mesh-modeling.md`'s authored table, not from the format's own
// validity rules (`docs/sculpt-format.md`): a slider is a drag surface, not a validator, so its
// bounds are chosen to keep a drag USEFUL — wide enough to reach a sensibly different model, narrow
// enough that a pixel of drag is a visible step. `check-scene` still has the final word on whether a
// value is legal; a slider may offer a value outside the format's own hard limits (`resolution`'s
// `4..128` happens to match both, `blend` does not need to).
//
// COORDINATES SCALE WITH THE MODEL. A fixed range like `-2..2` would be too coarse for a gnat-sized
// part and too fine for a giant's, so `at`/`from`/`to`/`step` range over the model's own extent — the
// largest position or size magnitude any part actually uses, floored at 1 so a model with no part
// yet placed still gets a workable range.
//
// Pure: no React, no Tauri, no DOM.

import type { NumberField, SculptDoc } from './doc';

/** A slider's bounds and granularity for one field. */
export interface SliderRange {
  readonly min: number;
  readonly max: number;
  readonly step: number;
  readonly integer: boolean;
}

/** Every key `doc.ts` can put on a `NumberField` — the switch in `rangeFor` is exhaustive over
 *  this set, checked at compile time by `assertNever` in its `default` arm and at test time by
 *  iterating this array directly, so the two lists cannot drift apart. */
export const SLIDER_KEYS = [
  'at',
  'from',
  'to',
  'step',
  'head',
  'tail',
  'radius',
  'radii',
  'size',
  'tube',
  'height',
  'blend',
  'round',
  'rotation',
  'degrees',
  'taper',
  'amplitude',
  'scale',
  'resolution',
  'triangles',
  'count',
  'color',
  'emissive',
  'roughness',
  'metallic',
] as const;
// A `noise`'s `seed` has no entry here: it never becomes a `NumberField` in the first place (see
// `doc.ts`'s `readNoise`) — a seed is a pick (which lattice), not a tunable (how much), so there is
// no smooth value for a slider to interpolate.

type SliderKey = (typeof SLIDER_KEYS)[number];

/** The range for `field`, sized against `doc` for the position/size keys that scale with the
 *  model. Total over every key `doc.ts` produces — see `rangeFor`. */
export function sliderRange(field: NumberField, doc: SculptDoc): SliderRange {
  return rangeFor(field.key as SliderKey, field, doc);
}

function rangeFor(key: SliderKey, field: NumberField, doc: SculptDoc): SliderRange {
  switch (key) {
    case 'at':
    case 'from':
    case 'to':
    case 'step':
    case 'head':
    case 'tail': {
      // A bone's `head`/`tail` is a position in the same document coordinates as a part's `at`, so
      // it reuses the same model-scaled range.
      const extent = modelExtent(doc) * 2;
      return range(-extent, extent, false);
    }
    case 'radius':
    case 'radii':
    case 'size':
    case 'tube':
    case 'height':
      return range(0, Math.max(field.value * 2, 0.1), false);
    case 'blend':
      return range(0, 0.5, false);
    case 'round':
      return range(0, 0.2, false);
    case 'rotation':
      return range(-180, 180, false);
    case 'degrees':
      // A `bend`/`twist`'s rotation-per-world-unit: the same travel as `rotation`, but a coarser
      // fixed step reads as a usable drag rather than 200 evenly-spaced stops over 360 degrees.
      return range(-180, 180, false, 1);
    case 'taper':
      return range(0.1, 3, false, 0.01);
    case 'amplitude':
      return range(0, 0.2, false, 0.001);
    case 'scale':
      return range(0.01, 1, false, 0.01);
    case 'resolution':
      return range(4, 128, true);
    case 'triangles':
      return range(500, 20000, true);
    case 'count':
      return range(2, 16, true);
    case 'color':
      return range(0, 1, false);
    case 'emissive':
      return range(0, 8, false);
    case 'roughness':
    case 'metallic':
      return range(0, 1, false);
    default:
      // `key` is `never` here iff the switch above covers every `SliderKey` — a key `doc.ts` gains
      // without a case here is a compile error, not a silent runtime fallback.
      assertNever(key);
      return range(field.value, field.value + 1, false);
  }
}

function assertNever(value: never): void {
  void value;
}

/** The largest absolute position or size magnitude any part or bone uses, floored at 1 so a
 *  coordinate range never collapses to a useless sliver around zero. */
function modelExtent(doc: SculptDoc): number {
  const keys = new Set([
    'at',
    'from',
    'to',
    'step',
    'head',
    'tail',
    'radius',
    'radii',
    'size',
    'tube',
    'height',
  ]);
  let extent = 0;
  for (const fields of [...doc.parts.map((part) => part.fields), ...doc.bones.map((bone) => bone.fields)]) {
    for (const field of fields) {
      if (keys.has(field.key)) {
        extent = Math.max(extent, Math.abs(field.value));
      }
    }
  }
  return Math.max(extent, 1);
}

function range(min: number, max: number, integer: boolean, step?: number): SliderRange {
  return { min, max, step: step ?? (integer ? 1 : (max - min) / 200), integer };
}
