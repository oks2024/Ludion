// The Sculpt inspector's layout, as pure data: which control belongs to which group, what each row
// is called, and what a dragged value is allowed to be.
//
// FILE ORDER IS THE LAYOUT. A sculpt is an ordered stack of primitives — a later part's `blend`
// reads against the parts before it — so the inspector shows the parts in the order the mesher
// evaluates them, never sorted or bucketed by shape. Same for a part's own fields and for the
// palette: `doc.ts` reads them in authored order and this module keeps that order intact, so the
// control a human reaches for is where the text they may also be reading puts it.
//
// WHY QUANTIZE HERE. A range input reports whatever the browser's own arithmetic produced, and
// `0.30000000000000004` written into a hand-authored file is a defect the human never asked for and
// cannot see on the slider. Snapping to the range's own step — the granularity the slider offered in
// the first place — is what makes a drag's result the value the handle claims.
//
// Pure: no React, no Tauri, no DOM.

import type { NumberField, SculptDoc } from './doc';
import type { SliderRange } from './ranges';

/** One slider row: the field it writes and the name it goes by on screen. */
export interface InspectorRow {
  readonly field: NumberField;
  /** The field's key, plus the component name where the pointer ends in an index (`at.y`,
   *  `color.r`). */
  readonly label: string;
}

/** A titled run of rows — one part, one palette entry, or the document's own two numbers. */
export interface InspectorGroup {
  /** Stable across a re-parse of the same file: React's key, and the test's handle. */
  readonly id: string;
  readonly head: string;
  readonly rows: readonly InspectorRow[];
}

/** Component names for an indexed field. A colour is read in channels and everything else in axes;
 *  an index past either list falls back to the number, which is always true if never evocative. */
const AXES = ['x', 'y', 'z', 'w'];
const CHANNELS = ['r', 'g', 'b', 'a'];

/** Nested-object parents whose scalar leaf reads "parent · leaf" rather than the leaf's key alone —
 *  a `deform`'s `bend`/`twist`/`taper` and a part's `noise` sit one level deeper than a part's own
 *  fields, so the field alone (`degrees`, shared by `bend` and `twist`) cannot name itself. */
const NESTED_PARENTS = new Set(['bend', 'twist', 'taper', 'noise']);

/** The row label for `field`: the key alone for a scalar, `key.component` for one element of a
 *  top-level vector, `parent · leaf` for a field nested inside `deform`/`noise` or a bone's
 *  `head`/`tail` (including that vector's own components, `head · x`). The index and parent come
 *  from the pointer's own segments, so nothing here has to know which keys are vectors. */
export function rowLabel(field: NumberField): string {
  const segments = field.pointer.split('/');
  const last = segments.at(-1) ?? '';
  const parent = segments.at(-2) ?? '';
  if (/^\d+$/.test(last)) {
    const index = Number(last);
    if (parent === 'head' || parent === 'tail') {
      return `${parent} · ${AXES[index] ?? String(index)}`;
    }
    const names = field.key === 'color' ? CHANNELS : AXES;
    return `${field.key}.${names[index] ?? String(index)}`;
  }
  if (NESTED_PARENTS.has(parent)) {
    return `${parent} · ${last}`;
  }
  return field.key;
}

/**
 * The inspector's groups, in the order it renders them: every part in file order, then every bone in
 * authored (tree) order, then every palette entry in authored order, then the document's own
 * `resolution`/`triangles` last.
 *
 * Bones sit right after parts — a part's `deform`/`bone` reference is shape, same as its own fields,
 * while a palette entry is material. The two document-wide numbers come last on purpose — they are
 * mesher settings rather than shape, so they must not sit between the human and the first part they
 * came to drag. A group with no numeric field at all is dropped rather than rendered empty (most
 * bones: an unrigged model's `root` has neither `head` nor `tail`).
 */
export function inspectorGroups(doc: SculptDoc): readonly InspectorGroup[] {
  const groups: InspectorGroup[] = [];

  for (const part of doc.parts) {
    const name = part.name === '' ? `part ${part.index}` : part.name;
    groups.push({
      id: `part:${part.index}`,
      head: part.shape === '' ? name : `${name} · ${part.shape}`,
      rows: part.fields.map((field) => ({ field, label: rowLabel(field) })),
    });
  }

  for (const bone of doc.bones) {
    groups.push({
      id: `bone:${bone.index}`,
      head: bone.name === '' ? `bone ${bone.index}` : bone.name,
      rows: bone.fields.map((field) => ({ field, label: rowLabel(field) })),
    });
  }

  for (const entry of doc.palette) {
    groups.push({
      id: `palette:${entry.glyph}`,
      head: `palette · ${entry.glyph}`,
      rows: entry.fields.map((field) => ({ field, label: rowLabel(field) })),
    });
  }

  const model: NumberField[] = [];
  if (doc.resolution !== null) model.push(doc.resolution);
  if (doc.triangles !== null) model.push(doc.triangles);
  if (model.length > 0) {
    groups.push({
      id: 'model',
      head: 'model',
      rows: model.map((field) => ({ field, label: rowLabel(field) })),
    });
  }

  return groups.filter((group) => group.rows.length > 0);
}

/**
 * `value` snapped to `range`'s own granularity and clamped to its bounds.
 *
 * The decimal rounding after the snap is not cosmetic: `min + k * step` is exact in neither binary
 * nor decimal, and the residue would reach the file verbatim. One digit past the step's own
 * magnitude is enough to erase it without ever moving the value to a neighbouring stop.
 */
export function quantize(value: number, range: SliderRange): number {
  if (!Number.isFinite(value)) {
    return range.min;
  }
  const clamped = Math.min(range.max, Math.max(range.min, value));
  if (range.integer) {
    return Math.round(clamped);
  }
  const step = range.step > 0 ? range.step : 1;
  const snapped = range.min + Math.round((clamped - range.min) / step) * step;
  const decimals = Math.min(12, Math.max(0, Math.ceil(-Math.log10(step)) + 1));
  return Number(Math.min(range.max, Math.max(range.min, snapped)).toFixed(decimals));
}
