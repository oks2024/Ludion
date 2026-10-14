// A `.gen.yaml` written for EDITING: the surgical half of the contract `doc.ts` reads for —
// `sculpt/edit.ts`'s shape on a second document kind.
//
// THE CONTRACT. Every write is an in-place text splice at the offset `doc.ts` measured, never a
// parse → re-emit round trip: a recipe is hand-authored and its prompt is a 600-character line, so a
// canonical re-emit would erase comments and authored key order and turn one slider drag into a
// whole-file diff. Every byte outside the one token touched comes out identical.
//
// TWO WRITERS, because the format treats the two value kinds differently:
//
//   - `setNumber` preserves the token's SPELLING CLASS. A ComfyUI `INT` input refuses `1024.0`
//     (`ludion docs generation`), so a field authored as an integer stays an integer, and a field
//     authored with a decimal point keeps one even when a drag lands on a whole number.
//   - `setText` rewrites the WHOLE token, quotes included, always as one double-quoted line. The
//     strict subset refuses block scalars (`L1006`), so a newline cannot survive in a value —
//     `sanitizeText` folds every line break to a space before it is ever written, which is why the
//     textarea shows exactly what the file will hold.
//
// WHAT IT DOES NOT DO: it does not validate. A count of zero, a negative seed, a prompt that says
// nothing — `gen check` has the answer (`L1162`), and the view's status line shows it. It never
// throws: an unknown pointer, or a value that cannot be written, is a no-op returning `doc.source`
// unchanged.
//
// Pure: no React, no Tauri, no DOM.

import { numberAt, textAt, type RecipeDoc } from './doc';

/** One replacement in the source text: `[offset, offset + removed.length)` becomes `inserted`. */
export interface Splice {
  readonly offset: number;
  readonly removed: string;
  readonly inserted: string;
}

/** The result of an edit: the file to write, and the one splice that produced it — `null` when the
 *  pointer was unknown or the write would change nothing, in which case `source` is unchanged. */
export interface RecipeEdit {
  readonly source: string;
  readonly splice: Splice | null;
}

/** A number in this format's own style. `String` is already shortest round-trip for a double; the
 *  one addition is the spelling class the original token carried (see the module doc). */
export function formatRecipeNumber(value: number, integer: boolean): string {
  if (integer) {
    return String(Math.trunc(value));
  }
  const text = String(value);
  return /[.e]/.test(text) ? text : `${text}.0`;
}

/**
 * Every character that cannot survive in a one-line double-quoted scalar, folded to a single space.
 *
 * A paste out of a browser is the case this exists for: it arrives with newlines, and the strict
 * subset has nowhere to put one (`L1006`). Folding rather than refusing keeps the pasted words —
 * losing a line break is recoverable, losing the prompt is not. Runs collapse, and the result is
 * trimmed at the ends, so a trailing newline does not become a trailing space in the file.
 */
export function sanitizeText(value: string): string {
  let out = value;
  // `\r\n` first, so one Windows line break does not become two spaces. Split/join rather than a
  // character class: a regexp holding control characters is exactly what the linters flag.
  for (const brk of ['\r\n', '\r', '\n', '\t', '\f', '\v']) {
    out = out.split(brk).join(' ');
  }
  return out.replace(/ {2,}/g, ' ').trim();
}

/** `value` as one double-quoted scalar: the two characters the format's own escape rules name. */
export function quoteText(value: string): string {
  return `"${sanitizeText(value).replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

/**
 * Rewrites the number at `pointer` to `value`.
 *
 * A no-op when `pointer` names no number in `doc`, `value` is not finite, or the formatted text is
 * byte-identical to what is already there (a slider released where it started).
 */
export function setNumber(doc: RecipeDoc, pointer: string, value: number): RecipeEdit {
  const field = numberAt(doc, pointer);
  if (field === null || !Number.isFinite(value)) {
    return { source: doc.source, splice: null };
  }
  return apply(
    doc.source,
    field.offset,
    field.length,
    formatRecipeNumber(value, field.integer),
  );
}

/**
 * Rewrites the string at `pointer` to `value`, as one double-quoted scalar.
 *
 * A no-op when `pointer` names no string in `doc`, or when the quoted form is byte-identical to what
 * is there. An empty `value` is a legal write: a prompt cleared on purpose is still an edit.
 */
export function setText(doc: RecipeDoc, pointer: string, value: string): RecipeEdit {
  const field = textAt(doc, pointer);
  if (field === null) {
    return { source: doc.source, splice: null };
  }
  return apply(doc.source, field.offset, field.length, quoteText(value));
}

function apply(source: string, offset: number, length: number, inserted: string): RecipeEdit {
  const removed = source.slice(offset, offset + length);
  if (inserted === removed) {
    return { source, splice: null };
  }
  const splice: Splice = { offset, removed, inserted };
  return { source: spliceSource(source, splice), splice };
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

/** The splice that undoes `splice`: applied to the text it produced, it restores the text it was
 *  applied to, byte for byte. `null` in, `null` out. */
export function invertSplice(splice: Splice | null): Splice | null {
  if (splice === null) {
    return null;
  }
  return { offset: splice.offset, removed: splice.inserted, inserted: splice.removed };
}
