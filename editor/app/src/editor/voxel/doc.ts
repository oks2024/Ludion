// A `.voxel.yaml` read for EDITING: the grid the painter draws, plus the exact source location of
// every slice row and every palette number.
//
// WHY OFFSETS ARE THE POINT (`Plan/editor-voxel-artist.md`, contract 1 — the hardest constraint in
// the arc). Every write to a voxel document is a surgical in-place text edit, never a parse →
// `emit_voxel` round trip. vigil's models are hand-authored: a header docstring records the
// silhouette intent and the front-face convention, and a comment sits above nearly every slice
// (`# y0 — ground contact: hands planted FORWARD …`). Those comments are how an agent re-reads a
// model's intent on its next visit; a canonical round trip would erase all of them and produce a
// diff the size of the file. The format makes the surgical edit easy — a slice row is a fixed-width
// scalar on its own line — so painting cell (x, z) in layer y is replacing ONE character at
// `cellOffset(doc, x, y, z)`, and re-tinting a glyph is replacing the numbers at
// `entry.color[i].span`. That is the whole contract, and this module is the half of it that reads.
//
// WHAT THIS DELIBERATELY DOES NOT DO (contract 2 — one checker, the engine's):
//   - It does not validate. Row widths, layer counts, glyphs that name no palette entry, duplicate
//     or non-ASCII palette keys, a non-positive `scale`: all of that is `check-scene`'s answer, read
//     through `readout.ts`. Nothing here reports a problem, because a second opinion that disagreed
//     with the engine would be worse than no opinion.
//   - It does not throw. A file it cannot fully understand yields whatever it could read — a doc
//     with `size: null`, a short palette, missing layers. The painter draws what parsed and the
//     status bar carries the engine's verdict.
//   - It does not resolve `format`/`kind`, connectivity, or the mesher's world placement.
//   - It does not follow trailing `#` comments on a value line. Nothing in the format needs one, no
//     authored model has one, and `#` is a legal palette glyph — guessing would corrupt a row.
//
// The format it reads is `crates/ludion-scene/src/voxel.rs` (7 keys: `format`, `kind`,
// `name`, `size`, `scale`, `palette`, `slices`). `slices[y][z]` is bottom-up, character `c` of a row
// is `x = c`, `.` is empty, and a palette entry carries `color` plus optional `emissive`/`metallic`/
// `roughness`. Both authored idioms are accepted: canonical bare scalars (`- A.`, what `emit_voxel`
// writes) and the quoted rows every vigil model uses (`- "....."`).
//
// The line scanner and the three section readers are exported because `assembly.ts` reads documents
// in the SAME strict-subset YAML (`kind: assembly`, `kind: part`, `kind: animation`), and a second
// scanner would be a second set of quoting and indentation bugs. Nothing about this module's own
// behaviour depends on those exports.
//
// Pure: no React, no Tauri, no DOM.

import type { Vec3 } from '../gizmo/vecmath';

/** The reserved glyph for an empty cell. It can never name a palette entry. */
export const EMPTY_GLYPH = '.';

/** Where something sits in the source text. */
export interface SourceSpan {
  /** 1-based line number — the engine's diagnostics and every editor gutter count this way. */
  readonly line: number;
  /** 0-based character index within the line. */
  readonly column: number;
  /** 0-based character index into the whole source. This is the one `edit.ts` splices at. */
  readonly offset: number;
  /** Length in characters of the source text this span covers. */
  readonly length: number;
}

/** A number as authored, with the span covering exactly its token — so a palette drag rewrites the
 *  number and nothing else on the line. */
export interface NumberField {
  readonly value: number;
  readonly span: SourceSpan;
}

/** One palette entry, with a span per editable field. */
export interface PaletteEntry {
  /** The palette key exactly as authored, quotes stripped. Valid keys are one printable-ASCII
   *  character, but a malformed one is reported as read — validation is the engine's job. */
  readonly glyph: string;
  /** The key token in the source (inside its quotes, if it was quoted). */
  readonly keySpan: SourceSpan;
  /** The `color` components in authored order — normally four (r, g, b, a). */
  readonly color: readonly NumberField[];
  readonly emissive: NumberField | null;
  readonly metallic: NumberField | null;
  readonly roughness: NumberField | null;
}

/** One slice row: `slices[y][z]`, one character per x. */
export interface SliceRow {
  readonly y: number;
  readonly z: number;
  /** The row's decoded content — what the painter draws, one character per cell. */
  readonly text: string;
  /** The row content in the source: the first character INSIDE the quotes for a quoted row, the
   *  first character of the token for a bare one. `length` is the raw source length, which differs
   *  from `text.length` exactly when an escape was decoded (see `paintable`). */
  readonly span: SourceSpan;
  readonly quoted: boolean;
  /** True when source characters map 1:1 onto `text` characters — i.e. `span.offset + x` is the
   *  source offset of cell x. False only when the scalar carried an escape (`\"`, `\\`, `''`), which
   *  no authored voxel row does. A row where this is false may not be painted, and may not be
   *  patched incrementally (`patch.ts`). */
  readonly paintable: boolean;
}

/** One `slices` layer, bottom-up (`y = 0` first). */
export interface SliceLayer {
  readonly y: number;
  readonly rows: readonly SliceRow[];
}

/** A voxel document read positionally. Every field is best-effort; see the module doc. */
export interface VoxelDoc {
  /** The source text verbatim — offsets index into this, and `edit.ts` splices this. */
  readonly source: string;
  readonly name: string | null;
  /** `[x, y, z]`, or `null` when `size` is absent or not three numbers. */
  readonly size: Vec3 | null;
  /** The declared intended world scale, or `null` when the model declares none. */
  readonly scale: NumberField | null;
  /** Palette entries in AUTHORED order (not the sorted order `emit_voxel` writes) — the swatch strip
   *  should look like the file. */
  readonly palette: readonly PaletteEntry[];
  readonly layers: readonly SliceLayer[];
}

/** Reads `source`. Total: never throws, never validates. */
export function parseVoxelDoc(source: string): VoxelDoc {
  const raw = splitLines(source);
  let name: string | null = null;
  let size: Vec3 | null = null;
  let scale: NumberField | null = null;
  let palette: readonly PaletteEntry[] = [];
  let layers: readonly SliceLayer[] = [];

  for (const entry of mapEntries(raw, 0, raw.length)) {
    switch (entry.key) {
      case 'name':
        name = entry.inline === null ? null : entry.inline.text;
        break;
      case 'size':
        size = readVec3(raw, entry);
        break;
      case 'scale':
        scale = entry.inline === null ? null : numberField(entry.inline);
        break;
      case 'palette':
        palette = readPalette(raw, entry);
        break;
      case 'slices':
        layers = readSlices(raw, entry);
        break;
      default:
        break;
    }
  }
  return { source, name, size, scale, palette, layers };
}

/** `slices[y][z]`, or `null` when the document has no such row. */
export function sliceRow(doc: VoxelDoc, y: number, z: number): SliceRow | null {
  return doc.layers[y]?.rows[z] ?? null;
}

/** The authored glyph at `(x, y, z)`, or `EMPTY_GLYPH` for any cell the document does not cover —
 *  the same lenient read as `VoxelDocument::glyph_at` in `voxel.rs`. */
export function glyphAt(doc: VoxelDoc, x: number, y: number, z: number): string {
  return sliceRow(doc, y, z)?.text[x] ?? EMPTY_GLYPH;
}

/**
 * The source offset of the single character encoding cell `(x, y, z)` — the wave-4 paint seam.
 *
 * `null` when the cell has no row, when x is outside the row, or when the row is not `paintable`.
 * A caller replacing `[offset, offset + 1)` with one glyph changes exactly one character of the
 * file and nothing else: no comment, no blank line, no key order.
 */
export function cellOffset(doc: VoxelDoc, x: number, y: number, z: number): number | null {
  const row = sliceRow(doc, y, z);
  if (row === null || !row.paintable || x < 0 || x >= row.text.length) {
    return null;
  }
  return row.span.offset + x;
}

/** The palette entry for `glyph`, or `null`. */
export function paletteEntry(doc: VoxelDoc, glyph: string): PaletteEntry | null {
  return doc.palette.find((entry) => entry.glyph === glyph) ?? null;
}

// --- Sections --------------------------------------------------------------------------------- //

/** Three numbers in a block sequence — a voxel `size`, and an assembly part's `place`/`pivot`.
 *  `null` unless the sequence holds exactly three plain number tokens. */
export function readVec3(raw: readonly RawLine[], entry: MapEntryRead): Vec3 | null {
  const items = seqItems(raw, entry.body[0], entry.body[1]);
  const values = items.map((item) => (item.inline === null ? null : numberField(item.inline)));
  if (values.length !== 3 || values.some((value) => value === null)) {
    return null;
  }
  const [x, y, z] = values as NumberField[];
  return [x.value, y.value, z.value];
}

/** A `palette:` block. Identical in a `kind: voxel` and a `kind: assembly` document — the assembly
 *  owns the one palette its parts' glyphs resolve against. */
export function readPalette(raw: readonly RawLine[], entry: MapEntryRead): PaletteEntry[] {
  const out: PaletteEntry[] = [];
  for (const glyph of mapEntries(raw, entry.body[0], entry.body[1])) {
    let color: readonly NumberField[] = [];
    let emissive: NumberField | null = null;
    let metallic: NumberField | null = null;
    let roughness: NumberField | null = null;
    for (const field of mapEntries(raw, glyph.body[0], glyph.body[1])) {
      switch (field.key) {
        case 'color':
          color = seqItems(raw, field.body[0], field.body[1])
            .map((item) => (item.inline === null ? null : numberField(item.inline)))
            .filter((value): value is NumberField => value !== null);
          break;
        case 'emissive':
          emissive = field.inline === null ? null : numberField(field.inline);
          break;
        case 'metallic':
          metallic = field.inline === null ? null : numberField(field.inline);
          break;
        case 'roughness':
          roughness = field.inline === null ? null : numberField(field.inline);
          break;
        default:
          break;
      }
    }
    out.push({ glyph: glyph.key, keySpan: glyph.keySpan, color, emissive, metallic, roughness });
  }
  return out;
}

/** A `slices:` block. A `kind: part` document's grid is exactly a voxel document's, minus the
 *  palette, so one reader serves both. */
export function readSlices(raw: readonly RawLine[], entry: MapEntryRead): SliceLayer[] {
  const out: SliceLayer[] = [];
  const layers = seqItems(raw, entry.body[0], entry.body[1]);
  for (const [y, layer] of layers.entries()) {
    // A layer is `-` with its rows indented beneath; the rows sit in the item's body.
    const rows = seqItems(raw, layer.body[0], layer.body[1]).flatMap((item, z) =>
      item.inline === null ? [] : [sliceRowOf(y, z, item.inline)],
    );
    out.push({ y, rows });
  }
  return out;
}

function sliceRowOf(y: number, z: number, scalar: ScalarRead): SliceRow {
  return {
    y,
    z,
    text: scalar.text,
    span: scalar.span,
    quoted: scalar.quoted,
    paintable: scalar.span.length === scalar.text.length,
  };
}

// --- The line scanner ------------------------------------------------------------------------- //
//
// A targeted, indentation-driven scan rather than a YAML library: this file has no dependencies, and
// the strict-subset block YAML the voxel format allows (`crates/ludion-scene/src/yaml.rs`) is a
// handful of shapes. Comment-only and blank lines are skipped everywhere, which is what makes the
// authored idiom — a `# y4 — muzzle top …` line above nearly every layer — cost nothing to read.

export interface RawLine {
  readonly text: string;
  /** Source offset of the line's first character. */
  readonly offset: number;
  /** 1-based line number. */
  readonly line: number;
  /** Leading-space count, or `null` for a blank or comment-only line. */
  readonly indent: number | null;
}

/** Splits `source` into the indentation-annotated lines every reader here scans. */
export function splitLines(source: string): RawLine[] {
  const lines = source.split('\n');
  const out: RawLine[] = new Array<RawLine>(lines.length);
  let offset = 0;
  for (let index = 0; index < lines.length; index += 1) {
    const text = lines[index];
    const body = text.endsWith('\r') ? text.slice(0, -1) : text;
    const first = firstNonSpace(body, 0);
    out[index] = {
      text: body,
      offset,
      line: index + 1,
      // 0x23 is `#`: a comment-only line carries no indentation, like a blank one.
      indent: first < 0 || body.charCodeAt(first) === 0x23 ? null : first,
    };
    offset += text.length + 1;
  }
  return out;
}

/** ASCII whitespace. The strict-subset YAML this reads admits no other kind, and scanning code
 *  points is what keeps `splitLines` from allocating a `trimStart()` copy of all 21k rows of a
 *  budget-exempt model. */
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
export interface MapEntryRead {
  readonly key: string;
  readonly keySpan: SourceSpan;
  readonly inline: ScalarRead | null;
  readonly body: readonly [number, number];
}

/** A `- ` sequence item: its inline value if any, and the range of its indented body. */
export interface SeqItemRead {
  readonly inline: ScalarRead | null;
  readonly body: readonly [number, number];
  /** Index of the dash line itself in the `RawLine[]` the item was read from — what `itemEntries`
   *  needs to reach a key authored on that line (`- name: torso`). */
  readonly at: number;
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
export function mapEntries(raw: readonly RawLine[], from: number, to: number): MapEntryRead[] {
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
export function seqItems(raw: readonly RawLine[], from: number, to: number): SeqItemRead[] {
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
    out.push({ inline, body, at: index });
    index = body[1];
  }
  return out;
}

/**
 * The mapping entries of a sequence item whose first key sits on the dash line itself
 * (`- name: torso`, `- t: 0.0`) — that key, then every key in the item's indented body.
 *
 * Both authored idioms yield the same entries, because the dash-line key is simply absent from the
 * `-`-alone form. A `slices` layer (`-` with rows beneath) has no inline key at all and comes back
 * with its body entries only.
 */
export function itemEntries(raw: readonly RawLine[], item: SeqItemRead): MapEntryRead[] {
  const body = mapEntries(raw, item.body[0], item.body[1]);
  if (item.inline === null) {
    return body;
  }
  const head = splitKey(raw[item.at], item.inline.span.column);
  return head === null ? body : [{ ...head, body: [item.at + 1, item.at + 1] }, ...body];
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
    keySpan: key.span,
    inline: value < 0 ? null : readScalar(line, value),
  };
}

/** A scalar read at a known column: its decoded text and the span of its source characters. */
export interface ScalarRead {
  readonly text: string;
  readonly quoted: boolean;
  /** The scalar's CONTENT — inside the quotes when quoted. `length` is the raw source length, which
   *  exceeds `text.length` exactly when an escape was decoded. */
  readonly span: SourceSpan;
}

/**
 * Reads the scalar starting at `column`.
 *
 * A double-quoted scalar decodes `\\` and `\"`; a single-quoted one decodes `''`. A bare scalar runs
 * to the end of the line — NOT up to a `#`, because this format never puts a trailing comment on a
 * value and `#` is a legal palette glyph. `stopAtColon` is for the KEY position only, where a bare
 * scalar ends at its `:`; a bare *value* keeps its colons, so a bare row of `:` glyphs survives.
 * An unterminated quote reads to end of line, so a half-typed file still yields rows.
 */
function readScalar(line: RawLine, column: number, stopAtColon = false): ScalarRead {
  const source = line.text;
  const quote = source[column];
  if (quote === '"' || quote === "'") {
    // Between two boundaries the scalar is verbatim, so it is appended as ONE slice — never a
    // concatenation per character. A row with no escape (every authored one) is a single slice.
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
        // A doubled `''` inside a single-quoted scalar: one literal quote.
        text += source.slice(chunk, index + 1);
        index += 2;
        chunk = index;
        continue;
      }
      if (index + 1 >= source.length) {
        // A trailing `\` escapes nothing, so it stands for itself and the scalar ends.
        index = source.length;
        break;
      }
      // A `\` in a double-quoted scalar: the character after it stands for itself.
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

function span(line: RawLine, column: number, length: number): SourceSpan {
  return { line: line.line, column, offset: line.offset + column, length };
}

const NUMBER = /^[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?$/;

/** A scalar as a number field, or `null` when it is not a plain number token. */
export function numberField(scalar: ScalarRead): NumberField | null {
  if (!NUMBER.test(scalar.text)) {
    return null;
  }
  return { value: Number(scalar.text), span: scalar.span };
}
