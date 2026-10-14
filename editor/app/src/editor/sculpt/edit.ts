// A `.sculpt.yaml` written for EDITING: the surgical half of the contract `doc.ts` reads for — the
// `voxel/edit.ts` precedent, cut down to the one shape this format's inspector needs.
//
// THE CONTRACT. Every write is a surgical in-place text edit at the position `doc.ts` measured,
// never a parse -> re-emit round trip: a sculpt model is hand-authored (a header docstring, a
// comment above a part), and a canonical re-emit would erase all of that and turn a one-number drag
// into a whole-file diff. So `setScalar` splices `doc.source` at exactly the field's own token, and
// every other byte of the file — comments, blank lines, key order, part order, quoting — comes out
// identical.
//
// ONE SPLICE, NOT A LIST. Unlike a voxel palette insertion, a sculpt scalar edit never adds or
// removes a line: it replaces one number token with another, so one splice is always the whole
// story. `Splice`/`spliceSource`/`invertSplices` still mirror `voxel/edit.ts`'s shape (an offset,
// the exact characters removed, the characters inserted) so a future undo stack can treat a sculpt
// edit and a voxel edit the same way.
//
// WHAT IT DOES NOT DO: it does not validate the result (a `blend` below zero, a `resolution` outside
// `4..128` — that is `check-scene`'s answer, read through `ranges.ts`'s slider bounds at most). It
// never throws: an unknown pointer, or a value that cannot be written as a finite number, is a no-op
// that returns `doc.source` unchanged.
//
// Pure: no React, no Tauri, no DOM.

import { fieldOffset, type SculptDoc } from './doc';

/** One replacement in the source text: `[offset, offset + removed.length)` becomes `inserted`. */
export interface Splice {
  readonly offset: number;
  readonly removed: string;
  readonly inserted: string;
}

/** The result of `setScalar`: the file to write, and the one splice that produced it — `null` when
 *  the pointer was unknown or the value could not be written, in which case `source` is
 *  `doc.source` unchanged. */
export interface SculptEdit {
  readonly source: string;
  readonly splice: Splice | null;
}

/** Keys written without a decimal point, however their value arrives — `resolution`/`triangles` are
 *  grid and count parameters in the format, `count` a `repeat:` iteration count; a `4.0` in any of
 *  them would be a different (and here, invalid) YAML type. */
const INTEGER_KEYS = new Set(['resolution', 'triangles', 'count']);

/**
 * A number in this format's own style: shortest round-trip, matching `formatVoxelFloat`'s rule
 * (JS `String` is already shortest round-trip for a double) with one addition — an integer-typed
 * key is written bare, and any other whole-number value keeps the `.0` suffix `original` already
 * had, so a slider does not silently turn `0.0` into `0` on a value nobody asked to shorten.
 */
export function formatSculptFloat(value: number, key: string, original: string): string {
  if (INTEGER_KEYS.has(key)) {
    return String(Math.trunc(value));
  }
  const text = String(value);
  if (/[.e]/.test(text)) {
    return text;
  }
  return original.includes('.') ? `${text}.0` : text;
}

/**
 * Rewrites the scalar at `pointer` to `value`.
 *
 * A no-op — `source` equal to `doc.source`, `splice` `null` — when `pointer` names no field in
 * `doc`, `value` is not finite, or the formatted text is byte-identical to what was already there
 * (a slider released back where it started).
 */
export function setScalar(doc: SculptDoc, pointer: string, value: number): SculptEdit {
  const field = doc.fields.find((candidate) => candidate.pointer === pointer);
  if (field === undefined || !Number.isFinite(value)) {
    return { source: doc.source, splice: null };
  }
  const offset = fieldOffset(doc, field);
  const removed = doc.source.slice(offset, offset + field.length);
  const inserted = formatSculptFloat(value, field.key, removed);
  if (inserted === removed) {
    return { source: doc.source, splice: null };
  }
  const splice: Splice = { offset, removed, inserted };
  return { source: spliceSource(doc.source, splice), splice };
}

/** The text `splice` produces from `source` — `source` unchanged when `splice` is `null`. */
export function spliceSource(source: string, splice: Splice | null): string {
  if (splice === null) {
    return source;
  }
  return (
    source.slice(0, splice.offset) +
    splice.inserted +
    source.slice(splice.offset + splice.removed.length)
  );
}

/**
 * The splice that undoes `splice`: applied to the text it produced, it restores the text it was
 * applied to, byte for byte. `null` in, `null` out, so an undo stack can carry a no-op edit without
 * a special case.
 */
export function invertSplices(splice: Splice | null): Splice | null {
  if (splice === null) {
    return null;
  }
  return { offset: splice.offset, removed: splice.inserted, inserted: splice.removed };
}
