// The pattern tab's YAML layer: read a file into an editable document, set or remove one path, and
// render it back as text a human would have written.
//
// WHY `parseDocument` AND NOT `parse`. A pattern file is hand-authored and the tab writes it on
// every edit, so a round trip that dropped the designer's header comments or re-ordered their keys
// would destroy work nobody asked us to touch. `yaml`'s document API edits the parsed tree in
// place: untouched nodes keep their comments, their order and their own formatting, and only the
// path being set is rebuilt.
//
// BLOCK STYLE, ALWAYS. The engine's content walk reports a data document written in flow style as
// `L1005` and does not load it (`ludion docs patterns`), so nothing here may emit `{ }` or `[ ]`. Created nodes are block
// by default; the two ways flow could sneak back in are an EMPTY collection (`motion: []`) and a
// folded long scalar, so `removeAtPath` prunes a collection it just emptied and `renderDoc` writes
// with folding off.
//
// FLOATS KEEP THEIR POINT. A schema `number` holding `1` must be written `1.0`, or a doc a human
// edits here and a doc written from Rust disagree over a value that is the same. `float()` marks
// such a number; `setAtPath` turns each mark into a scalar with a minimum fraction digit.
//
// Pure: no React, no Tauri, no DOM.

import { Document, parseDocument, Scalar, isCollection } from 'yaml';
import type { FieldPath } from '../../viewer/inspect-fields';

/** An editable pattern file. Mutated in place by `setAtPath`/`removeAtPath`, the `yaml` document
 *  API's own contract. */
export type PatternFile = Document;

/** Marks a number the writer must give a fraction digit — every schema `number` (`f32`) field. */
export class Float {
  readonly value: number;

  constructor(value: number) {
    this.value = value;
  }
}

/** {@link Float}'s constructor, as an expression. */
export function float(value: number): Float {
  return new Float(value);
}

/** Parses `text`. A file whose YAML is broken throws with the parser's own message — the tab shows
 *  it rather than opening an empty form over a file it could not read. */
export function parseDoc(text: string): PatternFile {
  const doc = parseDocument(text);
  if (doc.errors.length > 0) {
    throw new Error(doc.errors[0].message);
  }
  return doc;
}

/** The document's plain-JS value: what the form derives its fields from. */
export function docValue(doc: PatternFile): Record<string, unknown> {
  const value: unknown = doc.toJS();
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

/**
 * The shortest decimal that still round-trips to the same `f32` — `0.10000000149011612` back to
 * `0.1`.
 *
 * A schema default is an `f32` widened to a double, and writing the widened digits into a file a
 * human reads is a value nobody typed. This agrees with the Rust writer's f32 rule (`ludion-scene`
 * `format_f32_float`: an `f32` field is written as its shortest f32 decimal), so an editor edit and
 * `scene.save` spell the value alike. The search is the rule, not a table: raise the precision until
 * the candidate rounds to the same float.
 */
export function shortestFloat(value: number): number {
  const target = Math.fround(value);
  for (let digits = 1; digits <= 9; digits += 1) {
    const candidate = Number(target.toPrecision(digits));
    if (Math.fround(candidate) === target) {
      return candidate;
    }
  }
  return target;
}

/** Turns the `Float` marks in a value into scalars the writer gives a fraction digit to, leaving
 *  every other value for `yaml`'s own `createNode`. */
function toNode(value: unknown): unknown {
  if (value instanceof Float) {
    const scalar = new Scalar(shortestFloat(value.value));
    scalar.minFractionDigits = 1;
    return scalar;
  }
  if (Array.isArray(value)) {
    return value.map(toNode);
  }
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([key, inner]) => [key, toNode(inner)]));
  }
  return value;
}

/** Writes `value` at `path`, creating the collections along the way (a numeric step makes a
 *  sequence, a named one a mapping — `yaml`'s own rule). An empty path is refused: the document's
 *  envelope (`format:`, `kind:`) is not the form's to replace. */
export function setAtPath(doc: PatternFile, path: FieldPath, value: unknown): void {
  if (path.length === 0) {
    return;
  }
  doc.setIn([...path], toNode(value));
}

/**
 * Removes whatever is at `path`, then prunes every ancestor the removal left empty.
 *
 * The pruning is the block-style rule, not tidiness: an emptied sequence renders `motion: []` —
 * flow style, which the content walk reports as `L1005` — and every field this form removes is `#[serde(default)]`
 * in Rust, so an absent key and an empty collection mean the same thing to the engine.
 */
export function removeAtPath(doc: PatternFile, path: FieldPath): void {
  if (path.length === 0) {
    return;
  }
  doc.deleteIn([...path]);
  for (let end = path.length - 1; end > 0; end -= 1) {
    const parentPath = path.slice(0, end);
    const parent: unknown = doc.getIn([...parentPath], true);
    if (!isCollection(parent) || parent.items.length > 0) {
      return;
    }
    doc.deleteIn([...parentPath]);
  }
}

/** Reads whatever is at `path` as plain JS (`undefined` when a step is missing). Read off the
 *  converted document rather than `getIn`, which hands back a `yaml` node for a collection. */
export function valueAt(doc: PatternFile, path: FieldPath): unknown {
  let cur: unknown = docValue(doc);
  for (const step of path) {
    if (Array.isArray(cur)) {
      cur = cur[Number(step)];
    } else if (cur !== null && typeof cur === 'object') {
      cur = (cur as Record<string, unknown>)[String(step)];
    } else {
      return undefined;
    }
  }
  return cur;
}

/** The file's text. Folding is off: a wrapped long scalar is a shape the engine's strict YAML
 *  subset has no reason to accept, and a pattern's longest strings are asset paths. */
export function renderDoc(doc: PatternFile): string {
  return doc.toString({ lineWidth: 0 });
}
