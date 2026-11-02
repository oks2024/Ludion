// A `.sculpt.yaml` read for EDITING: every numeric field of a `kind: sculpt` document, with the
// exact source position of its scalar — the `voxel/doc.ts` precedent, applied to the implicit-
// surface format (`ludion docs sculpt-format`).
//
// WHY POSITIONS, NOT A PARSE TREE. The inspector this module feeds (`Plan/mesh-modeling.md`)
// drags a slider over a value that already sits in a hand-authored file — a header docstring, a
// comment above a part, an authored (not sorted) part order. A canonical re-emit would erase all of
// that and turn a one-number change into a whole-file diff, so `edit.ts` splices the source at the
// position this module measures instead of rebuilding it. This file only reads; it never writes.
//
// WHAT THIS DELIBERATELY DOES NOT DO (the voxel module's contract 2, unchanged here):
//   - It does not validate. A duplicate part name, a shape given a key that belongs to another
//     shape, an out-of-range `resolution`: all of that is `check-scene`'s answer. Nothing here
//     reports a problem.
//   - It does not throw. A file it cannot fully understand yields whatever it could read.
//   - It does not resolve `format`/`kind`, bones, palette blending, or the mesher's own semantics.
//   - It reads only BLOCK-style sequences (`- 0.9` on its own line), the canonical writer's form and
//     the only form the gallery idol uses. A flow sequence (`at: [0, 1, 0]`) is left unparsed — its
//     key's inline value is skipped rather than guessed at, so nothing here ever misreads a bracket
//     as a number.
//
// POSITIONS. `line`/`column` are 0-based: `line` counts newlines from the start of `source`, and
// `column` is the 0-based character index within that line — the same pair `splitLines` computes
// internally, just without the extra `offset`/1-based `line` fields `voxel/doc.ts`'s `SourceSpan`
// carries (an inspector renders line/column; nothing outside this module and `edit.ts` needs a raw
// offset). `edit.ts` recovers the offset it splices at from `line`/`column` via `fieldOffset` below,
// so the two files agree on exactly one definition of "where a field is."
//
// Pure: no React, no Tauri, no DOM.

/** A number as authored, with the position of exactly its token and the JSON-pointer path
 *  `edit.ts` and `ranges.ts` address it by. */
export interface NumberField {
  /** The engine's own pointer scheme for this document (`ludion docs sculpt-format`), e.g.
   *  `/parts/3/at/1` or `/palette/E/emissive`. */
  readonly pointer: string;
  /** The pointer's last non-index segment (`at`, `radius`, `color`, `blend`, …) — what
   *  `ranges.ts` looks a slider range up by, since `/parts/3/at/1` and `/parts/7/at/0` share a
   *  range shape even though their pointers differ. */
  readonly key: string;
  readonly value: number;
  /** 0-based line, counting `\n` from the start of `source`. */
  readonly line: number;
  /** 0-based character index of the scalar's first character within its line. */
  readonly column: number;
  /** Length in characters of the source token — `formatSculptFloat`'s replacement text may differ,
   *  which is exactly what makes an edit a splice rather than an in-place character swap. */
  readonly length: number;
}

/** One primitive in `parts:`, in file order. */
export interface SculptPart {
  /** Position in `parts:`, file order — stable across a re-parse of the same file, and what
   *  `/parts/<index>/...` addresses. */
  readonly index: number;
  /** `''` when the part declares no `name` — leniency, not validation (see the module doc). */
  readonly name: string;
  /** `''` when the part declares no `shape`. */
  readonly shape: string;
  /** Every numeric field this part carries, in file order — including a `deform`'s turns/taper and
   *  a `noise`'s amplitude/scale, addressed as `/parts/<index>/deform/<bend|twist>/degrees`,
   *  `/parts/<index>/deform/taper/<from|to>` and `/parts/<index>/noise/<amplitude|scale>`. */
  readonly fields: readonly NumberField[];
}

/** One entry in `bones:`, read positionally like a part. */
export interface SculptBone {
  /** Position in `bones:`, file order — stable across a re-parse of the same file, and what
   *  `/bones/<index>/...` addresses. */
  readonly index: number;
  /** `''` when the bone declares no `name` — leniency, not validation. */
  readonly name: string;
  /** `head`/`tail` components, in file order — `/bones/<index>/head/0..2` and
   *  `/bones/<index>/tail/0..2`. A grouping bone with neither carries no fields. */
  readonly fields: readonly NumberField[];
}

/** One `palette:` entry — the voxel palette shape, reused verbatim by this format. */
export interface SculptPaletteEntry {
  /** The glyph exactly as authored. Malformed glyphs are read as-is; validation is the engine's. */
  readonly glyph: string;
  readonly fields: readonly NumberField[];
}

/** A sculpt document read positionally. Every field is best-effort; see the module doc. */
export interface SculptDoc {
  /** The source text verbatim — `line`/`column` index into this, and `edit.ts` splices this. */
  readonly source: string;
  readonly name: string | null;
  readonly resolution: NumberField | null;
  readonly triangles: NumberField | null;
  /** Palette entries in AUTHORED order. */
  readonly palette: readonly SculptPaletteEntry[];
  /** Bones in AUTHORED (tree) order. */
  readonly bones: readonly SculptBone[];
  /** Parts in AUTHORED (evaluation) order. */
  readonly parts: readonly SculptPart[];
  /** Every numeric field in the document, in file order — what the inspector iterates to lay out
   *  its controls without knowing the document's shape in advance. */
  readonly fields: readonly NumberField[];
}

/** Reads `source`. Total: never throws, never validates. */
export function parseSculptDoc(source: string): SculptDoc {
  const raw = splitLines(source);
  let name: string | null = null;
  let resolution: NumberField | null = null;
  let triangles: NumberField | null = null;
  let palette: readonly SculptPaletteEntry[] = [];
  let bones: readonly SculptBone[] = [];
  let parts: readonly SculptPart[] = [];

  for (const entry of mapEntries(raw, 0, raw.length)) {
    switch (entry.key) {
      case 'name':
        name = entry.inline === null ? null : entry.inline.text;
        break;
      case 'resolution':
        resolution = readScalarField(entry, '/resolution', 'resolution');
        break;
      case 'triangles':
        triangles = readScalarField(entry, '/triangles', 'triangles');
        break;
      case 'palette':
        palette = readPalette(raw, entry);
        break;
      case 'bones':
        bones = readBones(raw, entry);
        break;
      case 'parts':
        parts = readParts(raw, entry);
        break;
      default:
        break;
    }
  }

  const fields: NumberField[] = [];
  if (resolution !== null) fields.push(resolution);
  if (triangles !== null) fields.push(triangles);
  for (const entry of palette) fields.push(...entry.fields);
  for (const bone of bones) fields.push(...bone.fields);
  for (const part of parts) fields.push(...part.fields);

  return { source, name, resolution, triangles, palette, bones, parts, fields };
}

/** The absolute character offset of `field`'s first character in `doc.source` — the position
 *  `edit.ts` splices at. Recovered from `line`/`column` (not carried on `NumberField` itself, which
 *  exposes only what an inspector renders) by counting newlines up to `field.line`. */
export function fieldOffset(doc: SculptDoc, field: NumberField): number {
  let offset = 0;
  for (let line = 0; line < field.line; line += 1) {
    const newline = doc.source.indexOf('\n', offset);
    offset = newline < 0 ? doc.source.length : newline + 1;
  }
  return offset + field.column;
}

/** The field at `pointer`, or `null`. */
export function fieldAt(doc: SculptDoc, pointer: string): NumberField | null {
  return doc.fields.find((field) => field.pointer === pointer) ?? null;
}

// --- Sections --------------------------------------------------------------------------------- //

function readPalette(raw: readonly RawLine[], entry: MapEntryRead): SculptPaletteEntry[] {
  const out: SculptPaletteEntry[] = [];
  for (const glyph of mapEntries(raw, entry.body[0], entry.body[1])) {
    const prefix = `/palette/${glyph.key}`;
    const fields: NumberField[] = [];
    for (const field of mapEntries(raw, glyph.body[0], glyph.body[1])) {
      switch (field.key) {
        case 'color':
          fields.push(...readVectorFields(raw, field, `${prefix}/color`, 'color'));
          break;
        case 'emissive':
        case 'metallic':
        case 'roughness': {
          const value = readScalarField(field, `${prefix}/${field.key}`, field.key);
          if (value !== null) fields.push(value);
          break;
        }
        default:
          break;
      }
    }
    out.push({ glyph: glyph.key, fields });
  }
  return out;
}

/** Sequence items that are themselves mappings (`- key: value`, the rest of the item indented under
 *  it) — the shape both `parts:` and `bones:` share. Patches the dash line's first key onto the base
 *  indent so the ordinary map scanner reads the whole item, dash line included, as a single mapping —
 *  no separate "first key on a sequence item" code path. */
function readMappingItems<T>(
  raw: readonly RawLine[],
  entry: MapEntryRead,
  empty: (index: number) => T,
  read: (raw: readonly RawLine[], from: number, to: number, index: number) => T,
): T[] {
  const out: T[] = [];
  const start = nextSignificant(raw, entry.body[0], entry.body[1]);
  if (start >= entry.body[1]) {
    return out;
  }
  const base = raw[start].indent as number;
  let index = start;
  let itemIndex = 0;
  while (index < entry.body[1]) {
    const line = raw[index];
    if (line.indent === null) {
      index += 1;
      continue;
    }
    if (line.indent < base) {
      break;
    }
    if (line.indent > base || line.text[base] !== '-') {
      index += 1;
      continue;
    }
    const itemEnd = bodyRange(raw, index + 1, entry.body[1], base)[1];
    const valueColumn = firstNonSpace(line.text, base + 1);
    if (valueColumn >= 0) {
      const patched = raw.slice();
      patched[index] = { ...line, indent: valueColumn };
      out.push(read(patched, index, itemEnd, itemIndex));
    } else {
      out.push(empty(itemIndex));
    }
    index = itemEnd;
    itemIndex += 1;
  }
  return out;
}

function readParts(raw: readonly RawLine[], entry: MapEntryRead): SculptPart[] {
  return readMappingItems(raw, entry, (index) => ({ index, name: '', shape: '', fields: [] }), readPart);
}

function readBones(raw: readonly RawLine[], entry: MapEntryRead): SculptBone[] {
  return readMappingItems(raw, entry, (index) => ({ index, name: '', fields: [] }), readBone);
}

function readBone(raw: readonly RawLine[], from: number, to: number, index: number): SculptBone {
  let name = '';
  const fields: NumberField[] = [];
  const prefix = `/bones/${index}`;
  for (const field of mapEntries(raw, from, to)) {
    switch (field.key) {
      case 'name':
        name = field.inline === null ? '' : field.inline.text;
        break;
      case 'head':
      case 'tail':
        fields.push(...readVectorFields(raw, field, `${prefix}/${field.key}`, field.key));
        break;
      default:
        break;
    }
  }
  return { index, name, fields };
}

function readPart(raw: readonly RawLine[], from: number, to: number, index: number): SculptPart {
  let name = '';
  let shape = '';
  const fields: NumberField[] = [];
  const prefix = `/parts/${index}`;
  for (const field of mapEntries(raw, from, to)) {
    switch (field.key) {
      case 'name':
        name = field.inline === null ? '' : field.inline.text;
        break;
      case 'shape':
        shape = field.inline === null ? '' : field.inline.text;
        break;
      case 'radius':
      case 'tube':
      case 'height':
      case 'round':
      case 'blend': {
        const value = readScalarField(field, `${prefix}/${field.key}`, field.key);
        if (value !== null) fields.push(value);
        break;
      }
      case 'radii':
      case 'from':
      case 'to':
      case 'at':
      case 'rotation':
        fields.push(...readVectorFields(raw, field, `${prefix}/${field.key}`, field.key));
        break;
      case 'size': {
        // A box's `size` is a vector (three extents); a voxel or mesh part's `size` is one
        // scalar (world units per voxel/document unit) — the entry's own form tells them apart
        // without knowing the shape: an inline value here is never a flow sequence (the module
        // doc), so it can only be the scalar form, and the two never share a line.
        if (field.inline !== null) {
          const value = readScalarField(field, `${prefix}/size`, 'size');
          if (value !== null) fields.push(value);
        } else {
          fields.push(...readVectorFields(raw, field, `${prefix}/size`, 'size'));
        }
        break;
      }
      case 'repeat':
        fields.push(...readRepeat(raw, field, prefix));
        break;
      case 'deform':
        fields.push(...readDeform(raw, field, prefix));
        break;
      case 'noise':
        fields.push(...readNoise(raw, field, prefix));
        break;
      default:
        break;
    }
  }
  return { index, name, shape, fields };
}

function readRepeat(raw: readonly RawLine[], entry: MapEntryRead, prefix: string): NumberField[] {
  const out: NumberField[] = [];
  for (const field of mapEntries(raw, entry.body[0], entry.body[1])) {
    switch (field.key) {
      case 'count': {
        const value = readScalarField(field, `${prefix}/repeat/count`, 'count');
        if (value !== null) out.push(value);
        break;
      }
      case 'step':
        out.push(...readVectorFields(raw, field, `${prefix}/repeat/step`, 'step'));
        break;
      default:
        break;
    }
  }
  return out;
}

/** A part's `deform:` sub-map: each of `bend`/`twist`'s own `degrees`, and `taper`'s `from`/`to`.
 *  `axis` is a keyword (`x`/`y`/`z`) in every one of the three, never a `NumberField`. */
function readDeform(raw: readonly RawLine[], entry: MapEntryRead, prefix: string): NumberField[] {
  const out: NumberField[] = [];
  for (const field of mapEntries(raw, entry.body[0], entry.body[1])) {
    switch (field.key) {
      case 'bend':
      case 'twist':
        out.push(...readTurn(raw, field, `${prefix}/deform/${field.key}`));
        break;
      case 'taper':
        out.push(...readTaper(raw, field, `${prefix}/deform/taper`));
        break;
      default:
        break;
    }
  }
  return out;
}

function readTurn(raw: readonly RawLine[], entry: MapEntryRead, pointerPrefix: string): NumberField[] {
  const out: NumberField[] = [];
  for (const field of mapEntries(raw, entry.body[0], entry.body[1])) {
    if (field.key === 'degrees') {
      const value = readScalarField(field, `${pointerPrefix}/degrees`, 'degrees');
      if (value !== null) out.push(value);
    }
  }
  return out;
}

function readTaper(raw: readonly RawLine[], entry: MapEntryRead, pointerPrefix: string): NumberField[] {
  const out: NumberField[] = [];
  for (const field of mapEntries(raw, entry.body[0], entry.body[1])) {
    switch (field.key) {
      case 'from':
      case 'to': {
        // Keyed `taper`, not `from`/`to` — those already name a capsule's position vector (a
        // different unit, a different `ranges.ts` case); the row label still reads the leaf segment
        // (`from`/`to`) off the pointer, in `rows.ts`.
        const value = readScalarField(field, `${pointerPrefix}/${field.key}`, 'taper');
        if (value !== null) out.push(value);
        break;
      }
      default:
        break;
    }
  }
  return out;
}

/** A part's `noise:` sub-map: `amplitude`/`scale`. `seed` is not read here — a seed is a pick (which
 *  lattice), not a tunable (how much), so it never becomes a `NumberField` and the inspector never
 *  offers it a slider. */
function readNoise(raw: readonly RawLine[], entry: MapEntryRead, prefix: string): NumberField[] {
  const out: NumberField[] = [];
  for (const field of mapEntries(raw, entry.body[0], entry.body[1])) {
    switch (field.key) {
      case 'amplitude':
      case 'scale': {
        const value = readScalarField(field, `${prefix}/noise/${field.key}`, field.key);
        if (value !== null) out.push(value);
        break;
      }
      default:
        break;
    }
  }
  return out;
}

/** A block-sequence numeric field (`color:`, `at:`, `radii:`, …). An inline value here (rather than
 *  indented `- ` items) is a flow sequence — left unparsed, per the module doc. */
function readVectorFields(
  raw: readonly RawLine[],
  entry: MapEntryRead,
  pointerPrefix: string,
  key: string,
): NumberField[] {
  const out: NumberField[] = [];
  seqItems(raw, entry.body[0], entry.body[1]).forEach((item, componentIndex) => {
    if (item.inline === null) {
      return;
    }
    const field = numberField(item.inline, `${pointerPrefix}/${componentIndex}`, key);
    if (field !== null) out.push(field);
  });
  return out;
}

function readScalarField(entry: MapEntryRead, pointer: string, key: string): NumberField | null {
  return entry.inline === null ? null : numberField(entry.inline, pointer, key);
}

// --- The line scanner ------------------------------------------------------------------------- //
//
// A targeted, indentation-driven scan rather than a YAML library — the same shape `voxel/doc.ts`
// uses, since the strict-subset block YAML this format allows is likewise a handful of shapes.
// Comment-only and blank lines are skipped everywhere, so the header docstring and the `# see
// sculpt-authoring` comment above a model cost nothing to read.

interface RawLine {
  readonly text: string;
  /** 0-based line number. */
  readonly line: number;
  /** Leading-space count, or `null` for a blank or comment-only line. */
  readonly indent: number | null;
}

function splitLines(source: string): RawLine[] {
  const lines = source.split('\n');
  const out: RawLine[] = new Array<RawLine>(lines.length);
  for (let index = 0; index < lines.length; index += 1) {
    const text = lines[index];
    const body = text.endsWith('\r') ? text.slice(0, -1) : text;
    const first = firstNonSpace(body, 0);
    out[index] = {
      text: body,
      line: index,
      // 0x23 is `#`: a comment-only line carries no indentation, like a blank one.
      indent: first < 0 || body.charCodeAt(first) === 0x23 ? null : first,
    };
  }
  return out;
}

/** ASCII whitespace — the strict-subset YAML this reads admits no other kind. */
function isSpace(code: number): boolean {
  return code === 0x20 || (code >= 0x09 && code <= 0x0d);
}

/** Index of the first non-whitespace character at or after `from`, or `-1` when the rest is blank. */
function firstNonSpace(text: string, from: number): number {
  let index = from;
  while (index < text.length && isSpace(text.charCodeAt(index))) {
    index += 1;
  }
  return index < text.length ? index : -1;
}

/** A `key:` entry: the key token, its inline value if any, and the range of its indented body. */
interface MapEntryRead {
  readonly key: string;
  readonly inline: ScalarRead | null;
  readonly body: readonly [number, number];
}

/** A `- ` sequence item: its inline value if any, and the range of its indented body. */
interface SeqItemRead {
  readonly inline: ScalarRead | null;
  readonly body: readonly [number, number];
}

/** Index of the next significant (non-blank, non-comment) line in `[from, to)`, or `to`. */
function nextSignificant(raw: readonly RawLine[], from: number, to: number): number {
  let index = from;
  while (index < to && raw[index].indent === null) {
    index += 1;
  }
  return index;
}

/** The half-open range of lines indented deeper than `indent`, starting at `from`. */
function bodyRange(
  raw: readonly RawLine[],
  from: number,
  to: number,
  indent: number,
): readonly [number, number] {
  let end = from;
  while (end < to) {
    const line = raw[end];
    if (line.indent !== null && line.indent <= indent) {
      break;
    }
    end += 1;
  }
  return [from, end];
}

/** Mapping entries at the base indent of the first significant line in `[from, to)`. */
function mapEntries(raw: readonly RawLine[], from: number, to: number): MapEntryRead[] {
  const out: MapEntryRead[] = [];
  const start = nextSignificant(raw, from, to);
  if (start >= to) {
    return out;
  }
  const base = raw[start].indent as number;
  let index = start;
  while (index < to) {
    const line = raw[index];
    if (line.indent === null) {
      index += 1;
      continue;
    }
    if (line.indent < base) {
      break;
    }
    if (line.indent > base) {
      index += 1;
      continue;
    }
    const parsed = splitKey(line, base);
    if (parsed === null) {
      index += 1;
      continue;
    }
    const body = bodyRange(raw, index + 1, to, base);
    out.push({ ...parsed, body });
    index = body[1];
  }
  return out;
}

/** Sequence items at the base indent of the first significant line in `[from, to)`. */
function seqItems(raw: readonly RawLine[], from: number, to: number): SeqItemRead[] {
  const out: SeqItemRead[] = [];
  const start = nextSignificant(raw, from, to);
  if (start >= to) {
    return out;
  }
  const base = raw[start].indent as number;
  let index = start;
  while (index < to) {
    const line = raw[index];
    if (line.indent === null) {
      index += 1;
      continue;
    }
    if (line.indent < base) {
      break;
    }
    if (line.indent > base || line.text[base] !== '-') {
      index += 1;
      continue;
    }
    const value = firstNonSpace(line.text, base + 1);
    const inline = value < 0 ? null : readScalar(line, value);
    const body = bodyRange(raw, index + 1, to, base);
    out.push({ inline, body });
    index = body[1];
  }
  return out;
}

/** Splits `key:` / `key: value` at `indent`, or `null` when the line is not a mapping entry. */
function splitKey(line: RawLine, indent: number): Omit<MapEntryRead, 'body'> | null {
  const key = readScalar(line, indent, true);
  const after = key.span.column + key.span.length + (key.quoted ? 1 : 0);
  if (line.text[after] !== ':') {
    return null;
  }
  const value = firstNonSpace(line.text, after + 1);
  return {
    key: key.text,
    inline: value < 0 ? null : readScalar(line, value),
  };
}

/** Where something sits in the source text, internal to the scanner — 0-based throughout, matching
 *  `RawLine.line` and the public `NumberField.line`/`column` directly. */
interface InternalSpan {
  readonly line: number;
  readonly column: number;
  readonly length: number;
}

/** A scalar read at a known column: its decoded text and the span of its source characters. */
interface ScalarRead {
  readonly text: string;
  readonly quoted: boolean;
  readonly span: InternalSpan;
}

/**
 * Reads the scalar starting at `column`.
 *
 * A double-quoted scalar decodes `\\` and `\"`; a single-quoted one decodes `''`. A bare scalar runs
 * to the end of the line — not up to a `#`, because this format never puts a trailing comment on a
 * value. `stopAtColon` is for the KEY position only, where a bare scalar ends at its `:`.
 */
function readScalar(line: RawLine, column: number, stopAtColon = false): ScalarRead {
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
    return {
      text: text + source.slice(chunk, index),
      quoted: true,
      span: span(line, start, index - start),
    };
  }
  const colon = stopAtColon ? source.indexOf(':', column) : -1;
  const end = colon < 0 ? source.length : colon;
  const text = source.slice(column, end).trimEnd();
  return { text, quoted: false, span: span(line, column, text.length) };
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

function span(line: RawLine, column: number, length: number): InternalSpan {
  return { line: line.line, column, length };
}

const NUMBER = /^[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?$/;

/** A scalar as a number field addressed by `pointer`, or `null` when it is not a plain number
 *  token — including a flow-sequence bracket, which never matches. */
function numberField(scalar: ScalarRead, pointer: string, key: string): NumberField | null {
  if (!NUMBER.test(scalar.text)) {
    return null;
  }
  return {
    pointer,
    key,
    value: Number(scalar.text),
    line: scalar.span.line,
    column: scalar.span.column,
    length: scalar.span.length,
  };
}
