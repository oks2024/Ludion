// A `.gen.yaml` read for EDITING: every value of a `kind: generation` recipe the view offers a
// control for, with the exact source span of its scalar.
//
// WHY POSITIONS, NOT A PARSE TREE. It is `sculpt/doc.ts`'s contract on a second document kind, for
// the same reason: a recipe is hand-authored (a comment above a node's overrides, an authored key
// order) and a prompt is a 600-character double-quoted line, so a canonical re-emit would turn a
// one-number change into a whole-file diff. `edit.ts` splices the source at the offsets this module
// measures; this file only reads.
//
// WHAT IT DELIBERATELY DOES NOT DO:
//   - It does not validate. An unknown key, a count of zero, a `workflow` that resolves nowhere: all
//     of that is `gen check`'s answer (`L1161`/`L1162`), read through the view's status line.
//   - It does not throw. A file it cannot fully understand yields whatever it could read.
//   - It does not read `format`, `kind`, or an image REFERENCE's target (`image: { image: subject }`)
//     as an editable value. A reference names an edge into the recipe's own `images:` map, which is
//     a file to pick, not a number to drag — it is recorded as a read-only row and nothing more.
//   - It reads only BLOCK style, the one form the strict YAML subset allows (`L1005`/`L1006`).
//
// OFFSETS, NOT LINE/COLUMN. `sculpt/doc.ts` carries line/column because its inspector renders them;
// nothing here does, so a field carries the absolute offset `edit.ts` splices at directly.
//
// Pure: no React, no Tauri, no DOM.

/** A number as authored, with the span of exactly its token. */
export interface RecipeNumber {
  /** The engine's pointer scheme for this document: `/seed`, `/count`, or
   *  `/inputs/<title>/<key>` with `~`/`/` escaped per JSON Pointer. */
  readonly pointer: string;
  /** The input's own name (`seed`, `count`, `width`, `steps`, …) — what `ranges.ts` looks a control
   *  up by, since two nodes' `width` share a control even though their pointers differ. */
  readonly key: string;
  readonly value: number;
  /** Absolute character offset of the token's first character in `source`. */
  readonly offset: number;
  /** Length of the source token. A replacement may be a different length, which is exactly what
   *  makes an edit a splice rather than a character swap. */
  readonly length: number;
  /** The token was spelled as an integer (no `.`, no exponent). A ComfyUI `INT` input refuses
   *  `1024.0`, so the spelling is load-bearing and a write preserves it (`ludion docs generation`). */
  readonly integer: boolean;
}

/** A string as authored, with the span of its whole token — the opening quote through the closing
 *  one, so a rewrite replaces the quoting too and can normalize a bare scalar into a quoted one. */
export interface RecipeText {
  readonly pointer: string;
  readonly key: string;
  /** The decoded text: escapes resolved, quotes removed. */
  readonly value: string;
  readonly offset: number;
  readonly length: number;
}

/** One `inputs:` entry — a node title and every value under it, in file order. */
export interface RecipeNode {
  /** The `_meta.title` the overrides key on, exactly as authored. */
  readonly title: string;
  readonly numbers: readonly RecipeNumber[];
  readonly texts: readonly RecipeText[];
  /** Input names bound to an `images:` entry (`{ image: subject }`), read-only here. */
  readonly refs: readonly { readonly key: string; readonly image: string }[];
}

/** One `images:` entry: the recipe's own name for a committed image, and its path as authored. */
export interface RecipeImage {
  readonly name: string;
  readonly path: string;
}

/** A recipe read positionally. Every field is best-effort; see the module doc. */
export interface RecipeDoc {
  /** The source verbatim — every `offset` indexes into this, and `edit.ts` splices it. */
  readonly source: string;
  readonly name: string | null;
  readonly workflow: string | null;
  readonly seed: RecipeNumber | null;
  readonly count: RecipeNumber | null;
  readonly images: readonly RecipeImage[];
  readonly nodes: readonly RecipeNode[];
  /** Every number in the document, in file order (`seed`, `count`, then the nodes') — what the
   *  inspector iterates without knowing the recipe's shape in advance. */
  readonly numbers: readonly RecipeNumber[];
  /** Every string under `inputs:`, in file order. Top-level `name`/`workflow` are deliberately not
   *  here: one names the run folder and the other the graph, and neither is a value to type over in
   *  a form — they are shown as the document's identity instead. */
  readonly texts: readonly RecipeText[];
}

/** Reads `source`. Total: never throws, never validates. */
export function parseRecipeDoc(source: string): RecipeDoc {
  const lines = splitLines(source);
  let name: string | null = null;
  let workflow: string | null = null;
  let seed: RecipeNumber | null = null;
  let count: RecipeNumber | null = null;
  let images: readonly RecipeImage[] = [];
  let nodes: readonly RecipeNode[] = [];

  for (const entry of mapEntries(lines, 0, lines.length)) {
    switch (entry.key) {
      case 'name':
        name = entry.value === null ? null : entry.value.text;
        break;
      case 'workflow':
        workflow = entry.value === null ? null : entry.value.text;
        break;
      case 'seed':
        seed = numberOf(entry, '/seed', 'seed');
        break;
      case 'count':
        count = numberOf(entry, '/count', 'count');
        break;
      case 'images':
        images = readImages(lines, entry);
        break;
      case 'inputs':
        nodes = readNodes(lines, entry);
        break;
      default:
        break;
    }
  }

  const numbers: RecipeNumber[] = [];
  if (seed !== null) numbers.push(seed);
  if (count !== null) numbers.push(count);
  const texts: RecipeText[] = [];
  for (const node of nodes) {
    numbers.push(...node.numbers);
    texts.push(...node.texts);
  }

  return { source, name, workflow, seed, count, images, nodes, numbers, texts };
}

/** The number at `pointer`, or `null`. */
export function numberAt(doc: RecipeDoc, pointer: string): RecipeNumber | null {
  return doc.numbers.find((field) => field.pointer === pointer) ?? null;
}

/** The string at `pointer`, or `null`. */
export function textAt(doc: RecipeDoc, pointer: string): RecipeText | null {
  return doc.texts.find((field) => field.pointer === pointer) ?? null;
}

/** One pointer segment, escaped per RFC 6901 — a node title is human-typed and may hold a `/`. */
export function pointerSegment(segment: string): string {
  return segment.replace(/~/g, '~0').replace(/\//g, '~1');
}

// --- sections --------------------------------------------------------------------------------- //

function readImages(lines: readonly Line[], entry: MapEntry): RecipeImage[] {
  const out: RecipeImage[] = [];
  for (const image of mapEntries(lines, entry.body[0], entry.body[1])) {
    if (image.value !== null) {
      out.push({ name: image.key, path: image.value.text });
    }
  }
  return out;
}

function readNodes(lines: readonly Line[], entry: MapEntry): RecipeNode[] {
  const out: RecipeNode[] = [];
  for (const node of mapEntries(lines, entry.body[0], entry.body[1])) {
    const prefix = `/inputs/${pointerSegment(node.key)}`;
    const numbers: RecipeNumber[] = [];
    const texts: RecipeText[] = [];
    const refs: { key: string; image: string }[] = [];
    for (const input of mapEntries(lines, node.body[0], node.body[1])) {
      const pointer = `${prefix}/${pointerSegment(input.key)}`;
      if (input.value === null) {
        // A nested map: the one structured form is a reference into `images:`.
        const image = mapEntries(lines, input.body[0], input.body[1]).find(
          (nested) => nested.key === 'image' && nested.value !== null,
        );
        if (image?.value !== undefined && image.value !== null) {
          refs.push({ key: input.key, image: image.value.text });
        }
        continue;
      }
      const numeric = numberOf(input, pointer, input.key);
      if (numeric !== null) {
        numbers.push(numeric);
        continue;
      }
      // Not a number, so it is a string the graph is fed verbatim — a prompt, a sampler name.
      texts.push({
        pointer,
        key: input.key,
        value: input.value.text,
        offset: input.value.offset,
        length: input.value.length,
      });
    }
    out.push({ title: node.key, numbers, texts, refs });
  }
  return out;
}

const NUMBER = /^[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?$/;

/** An entry's inline value as a number field, or `null` when it is not a plain number token. A
 *  quoted scalar is never one: `"1024"` is a string to the graph, and so it stays here. */
function numberOf(entry: MapEntry, pointer: string, key: string): RecipeNumber | null {
  const value = entry.value;
  if (value === null || value.quoted || !NUMBER.test(value.text)) {
    return null;
  }
  return {
    pointer,
    key,
    value: Number(value.text),
    offset: value.offset,
    length: value.length,
    integer: !/[.eE]/.test(value.text),
  };
}

// --- the line scanner ------------------------------------------------------------------------- //
//
// An indentation-driven scan rather than a YAML library — `sculpt/doc.ts`'s shape, since the strict
// subset a recipe allows is a handful of forms. Blank and comment-only lines are skipped
// everywhere, so a header comment costs nothing to read.

interface Line {
  readonly text: string;
  /** Absolute offset of the line's first character in the source. */
  readonly start: number;
  /** Leading-space count, or `null` for a blank or comment-only line. */
  readonly indent: number | null;
}

function splitLines(source: string): Line[] {
  const out: Line[] = [];
  let start = 0;
  for (;;) {
    const newline = source.indexOf('\n', start);
    const end = newline < 0 ? source.length : newline;
    const raw = source.slice(start, end);
    const text = raw.endsWith('\r') ? raw.slice(0, -1) : raw;
    const first = firstNonSpace(text, 0);
    out.push({
      text,
      start,
      // 0x23 is `#`: a comment-only line carries no indentation, like a blank one.
      indent: first < 0 || text.charCodeAt(first) === 0x23 ? null : first,
    });
    if (newline < 0) {
      return out;
    }
    start = newline + 1;
  }
}

/** ASCII whitespace — the strict subset admits no other kind. */
function isSpace(code: number): boolean {
  return code === 0x20 || (code >= 0x09 && code <= 0x0d);
}

/** Index of the first non-whitespace character at or after `from`, or `-1`. */
function firstNonSpace(text: string, from: number): number {
  let index = from;
  while (index < text.length && isSpace(text.charCodeAt(index))) {
    index += 1;
  }
  return index < text.length ? index : -1;
}

/** A scalar's decoded text and the absolute span of its source characters. */
interface Scalar {
  readonly text: string;
  readonly quoted: boolean;
  /** Absolute offset of the token's first source character — the opening quote, when quoted. */
  readonly offset: number;
  /** Length of the whole token, quotes included. */
  readonly length: number;
}

/** A `key:` entry: the key, its inline value if any, and the line range of its indented body. */
interface MapEntry {
  readonly key: string;
  readonly value: Scalar | null;
  readonly body: readonly [number, number];
}

/** Index of the next significant (non-blank, non-comment) line in `[from, to)`, or `to`. */
function nextSignificant(lines: readonly Line[], from: number, to: number): number {
  let index = from;
  while (index < to && lines[index].indent === null) {
    index += 1;
  }
  return index;
}

/** The half-open line range indented deeper than `indent`, starting at `from`. */
function bodyRange(
  lines: readonly Line[],
  from: number,
  to: number,
  indent: number,
): readonly [number, number] {
  let end = from;
  while (end < to) {
    const line = lines[end];
    if (line.indent !== null && line.indent <= indent) {
      break;
    }
    end += 1;
  }
  return [from, end];
}

/** Mapping entries at the base indent of the first significant line in `[from, to)`. */
function mapEntries(lines: readonly Line[], from: number, to: number): MapEntry[] {
  const out: MapEntry[] = [];
  const start = nextSignificant(lines, from, to);
  if (start >= to) {
    return out;
  }
  const base = lines[start].indent as number;
  let index = start;
  while (index < to) {
    const line = lines[index];
    if (line.indent === null || line.indent > base) {
      index += 1;
      continue;
    }
    if (line.indent < base) {
      break;
    }
    const parsed = splitKey(line, base);
    if (parsed === null) {
      index += 1;
      continue;
    }
    const body = bodyRange(lines, index + 1, to, base);
    out.push({ ...parsed, body });
    index = body[1];
  }
  return out;
}

/** Splits `key:` / `key: value` at `indent`, or `null` when the line is not a mapping entry. */
function splitKey(line: Line, indent: number): Omit<MapEntry, 'body'> | null {
  const key = readScalar(line, indent, true);
  const after = indent + key.length;
  if (line.text[after] !== ':') {
    return null;
  }
  const value = firstNonSpace(line.text, after + 1);
  return {
    key: key.text,
    // A trailing `#` is not a comment here: this format never puts one on a value, and a prompt may
    // legitimately contain one.
    value: value < 0 ? null : readScalar(line, value),
  };
}

/**
 * Reads the scalar starting at column `column` of `line`.
 *
 * A double-quoted scalar decodes `\\` and `\"`; a single-quoted one decodes `''`. A bare scalar runs
 * to the end of the line. `stopAtColon` is for the KEY position only, where a bare scalar ends at
 * its `:` — a quoted key ends at its closing quote, so its own `:` is safe.
 */
function readScalar(line: Line, column: number, stopAtColon = false): Scalar {
  const source = line.text;
  const quote = source[column];
  if (quote === '"' || quote === "'") {
    const escape = quote === '"' ? '\\' : "'";
    const start = column + 1;
    let text = '';
    let chunk = start;
    let index = start;
    for (;;) {
      index = nextBoundary(source, index, quote, escape);
      if (index >= source.length) {
        break;
      }
      if (source[index] === quote) {
        if (escape !== "'" || source[index + 1] !== "'") {
          break;
        }
        text += source.slice(chunk, index + 1);
        index += 2;
        chunk = index;
        continue;
      }
      if (index + 1 >= source.length) {
        index = source.length;
        break;
      }
      text += source.slice(chunk, index);
      chunk = index + 1;
      index += 2;
    }
    const closed = source[index] === quote;
    return {
      text: text + source.slice(chunk, index),
      quoted: true,
      offset: line.start + column,
      // Both quotes, when the closing one is there; an unterminated scalar stops at end of line.
      length: index - column + (closed ? 1 : 0),
    };
  }
  const colon = stopAtColon ? source.indexOf(':', column) : -1;
  const end = colon < 0 ? source.length : colon;
  const text = source.slice(column, end).trimEnd();
  return { text, quoted: false, offset: line.start + column, length: text.length };
}

/** The next `quote` or `escape` at or after `from`, or `text.length` when neither occurs again. */
function nextBoundary(text: string, from: number, quote: string, escape: string): number {
  const closing = text.indexOf(quote, from);
  if (escape === quote) {
    return closing < 0 ? text.length : closing;
  }
  const escaped = text.indexOf(escape, from);
  if (closing < 0) {
    return escaped < 0 ? text.length : escaped;
  }
  return escaped < 0 ? closing : Math.min(closing, escaped);
}
