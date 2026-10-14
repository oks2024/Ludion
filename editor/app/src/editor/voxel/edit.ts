// A `.voxel.yaml` written for EDITING: the surgical half of the contract `doc.ts` reads for.
//
// THE CONTRACT (`Plan/editor-voxel-artist.md`, contract 1 — the hardest constraint in the arc).
// Every write to a voxel document is a surgical in-place text edit, never a parse → `emit_voxel`
// round trip. vigil's models are hand-authored: a header docstring records the silhouette intent
// and the front-face convention, and a comment sits above nearly every slice (`# y0 — ground
// contact: hands planted FORWARD …`). Those comments are how an agent re-reads a model's intent on
// its next visit; a canonical round trip would erase all of them and produce a diff the size of the
// file. So every function here splices `doc.source` at spans `doc.ts` measured, and the file's
// bytes outside those spans come out identical — comments, blank lines, key order, quoting style,
// authored palette order, all of it.
//
// `doc.ts` deliberately exposes no splice helper so that mutation has exactly one owner. This is it.
//
// WHAT IT DOES NOT DO (contract 2 — one checker, the engine's): it does not validate the result.
// It refuses only the edits it cannot perform WITHOUT CORRUPTING THE FILE — an unaddressable cell,
// a row whose characters do not map 1:1 onto the source, a glyph that names no palette entry, a
// missing field, a non-finite number. Whether the edited model is a good model is `check-scene`'s
// answer, read through `readout.ts`. Nothing here throws.
//
// CALLING SHAPE. Every function takes the parsed `VoxelDoc` (the caller already holds one — it is
// what the painter draws) and returns the whole new source text plus the splices that produced it.
// The splices are the interesting half: `patch.ts` carries them into the working document without
// re-reading the file, `paint.ts` stores them (never whole sources) as history, and they are what
// makes the one-character property machine-checkable. No splices means the edit was a no-op and
// `source` is `doc.source` unchanged — the painter skips the write, which spares the host watcher a
// reload.
//
// A CELL EDIT NEVER MOVES AN OFFSET (one character replaces one character), so a whole mouse drag
// can be applied against ONE doc: `setCells` is the primitive and `setCell` its one-cell case.
// A PALETTE EDIT DOES move offsets (`0.9` → `0.85` is a character longer), so re-parse after each.
//
// Pure: no React, no Tauri, no DOM.

import { cellOffset, paletteEntry, sliceRow, type PaletteEntry, type VoxelDoc } from './doc';

/** One replacement in the source text: `[offset, offset + removed.length)` becomes `inserted`. */
export interface Splice {
  readonly offset: number;
  /** The exact source characters replaced — empty for a pure insertion. */
  readonly removed: string;
  /** The characters written in their place — empty for a pure deletion. */
  readonly inserted: string;
}

/** A successful edit: the file to write, and how it differs from `doc.source`. */
export interface VoxelEdit {
  /** The whole new source text. */
  readonly source: string;
  /** The splices applied to `doc.source`, in ascending offset order and non-overlapping. Empty
   *  when the edit was a no-op, in which case `source` is `doc.source` unchanged. */
  readonly splices: readonly Splice[];
}

/** Why an edit was refused. Each one names an edit that could not be made without mangling the
 *  file — never a judgment about the model (contract 2). */
export type VoxelEditReason =
  /** The cell has no slice row, or x is outside the row. */
  | 'out-of-bounds'
  /** The row carried an escape, so its characters do not map 1:1 onto the source. */
  | 'not-paintable'
  /** The paint glyph is not a single printable-ASCII character, or names no palette entry. */
  | 'unknown-glyph'
  /** The palette has no entry for that glyph. */
  | 'no-such-entry'
  /** The entry declares no such field, so there is no number to replace. */
  | 'no-such-field'
  /** `addGlyph` was given a glyph the palette already has. */
  | 'duplicate-glyph'
  /** A value was NaN or infinite — no YAML number can carry it. */
  | 'bad-number'
  /** `addGlyph` found no existing entry to take indentation and position from. */
  | 'no-anchor';

/** A refusal: a code to branch on and a sentence for the status bar. */
export interface VoxelEditError {
  readonly reason: VoxelEditReason;
  readonly message: string;
}

/** Every function in this module returns one of these. Nothing throws. */
export type VoxelEditResult =
  | { readonly ok: true; readonly edit: VoxelEdit }
  | { readonly ok: false; readonly error: VoxelEditError };

/** One cell to paint. `glyph` is a palette key, or `.` to erase. */
export interface CellPaint {
  readonly x: number;
  readonly y: number;
  readonly z: number;
  readonly glyph: string;
}

/** The optional material scalars a palette entry may declare. */
export type MaterialField = 'emissive' | 'metallic' | 'roughness';

/** A color change: components in `r, g, b, a` order, `null` (or a short array) leaving one alone.
 *  A picker sends `[r, g, b, null]`; a single-channel drag sends `[null, 0.3, null]`. */
export type ColorPatch = readonly (number | null)[];

/** A palette entry to add. The optional scalars are written only when given a number. */
export interface NewPaletteEntry {
  readonly glyph: string;
  readonly color: readonly [number, number, number, number];
  readonly emissive?: number | null;
  readonly metallic?: number | null;
  readonly roughness?: number | null;
}

/** The optional material scalars, in the canonical key order `emit_voxel` writes them. */
const MATERIAL_FIELDS: readonly MaterialField[] = ['emissive', 'metallic', 'roughness'];

/** `color` is r, g, b, a — the format admits no other arity. */
const COLOR_COMPONENTS = 4;

/**
 * A number as this format writes it: shortest round-trip, with `.0` appended to an integral value.
 *
 * Shortest round-trip is what `emit_voxel` produces (ryu, via `crates/ludion-scene/src/scalar.rs`),
 * and CLAUDE.md calls fixed precision a data-loss bug — so `0.1` stays `0.1`, never `0.100` and
 * never `0.10000000149011612`. JS `String` is already shortest-round-trip for a double; the one
 * thing it gets wrong for this format is `1` → `"1"`, which YAML resolves as an INTEGER, so an
 * integral value gains a `.0` (`1` → `1.0`, matching both ryu and every authored model).
 *
 * Authored trailing zeros (`0.20`, which several vigil palettes use) are NOT imitated: reproducing
 * them would mean guessing a per-file precision, and `0.2` is the same number written canonically.
 * A component whose value does not change is never rewritten at all, so the imitation question only
 * arises for digits the human actually moved.
 */
export function formatVoxelFloat(value: number): string {
  const text = String(value);
  return /[.e]/.test(text) ? text : `${text}.0`;
}

/** Paints one cell. See `setCells` — this is its one-cell case. */
export function setCell(doc: VoxelDoc, paint: CellPaint): VoxelEditResult {
  return setCells(doc, [paint]);
}

/**
 * Paints a batch of cells against one document — a whole drag stroke in a single write.
 *
 * Sound as a batch because a cell edit replaces one character with one character, so no earlier
 * paint moves a later paint's offset. Repeats of the same cell resolve last-write-wins, and cells
 * already carrying their target glyph drop out (a drag re-crossing painted cells is a no-op).
 *
 * All-or-nothing: one unaddressable cell refuses the whole batch rather than writing a partial
 * stroke. A painter that wants to clip at the grid edge filters before calling.
 */
export function setCells(doc: VoxelDoc, paints: readonly CellPaint[]): VoxelEditResult {
  const wanted = new Map<number, string>();
  for (const paint of paints) {
    const rejected = rejectGlyph(doc, paint.glyph);
    if (rejected !== null) {
      return rejected;
    }
    const offset = cellOffset(doc, paint.x, paint.y, paint.z);
    if (offset === null) {
      return rejectCell(doc, paint);
    }
    wanted.set(offset, paint.glyph);
  }
  const splices = [...wanted]
    .filter(([offset, glyph]) => doc.source[offset] !== glyph)
    .sort((a, b) => a[0] - b[0])
    .map(([offset, glyph]) => ({ offset, removed: doc.source[offset], inserted: glyph }));
  return applied(doc.source, splices);
}

/** Rewrites the color components the patch names, leaving every other byte — including the
 *  authored text of components whose value does not change — exactly as it was. */
export function setPaletteColor(doc: VoxelDoc, glyph: string, color: ColorPatch): VoxelEditResult {
  const entry = paletteEntry(doc, glyph);
  if (entry === null) {
    return fail('no-such-entry', `the palette has no entry for \`${glyph}\``);
  }
  // A color is exactly four numbers. Reading any other count means a component is unaddressable —
  // an absent one, or one carrying a trailing comment, which `doc.ts` deliberately does not follow.
  // Editing by index through a short list would write red into green, so refuse instead.
  if (entry.color.length !== COLOR_COMPONENTS) {
    return fail(
      'no-such-field',
      `\`${glyph}\` has ${entry.color.length} addressable color components, not ${COLOR_COMPONENTS}; edit its \`color\` by hand`,
    );
  }
  const splices: Splice[] = [];
  for (const [index, value] of color.entries()) {
    if (value === null) {
      continue;
    }
    if (!Number.isFinite(value)) {
      return fail('bad-number', `color component ${index} must be a finite number, got ${value}`);
    }
    const component = entry.color[index];
    if (component === undefined) {
      return fail('no-such-field', `a color has ${COLOR_COMPONENTS} components (r, g, b, a); there is no component ${index}`);
    }
    if (component.value !== value) {
      splices.push(replacement(doc.source, component.span, formatVoxelFloat(value)));
    }
  }
  return applied(doc.source, splices);
}

/**
 * Rewrites one of the optional material scalars.
 *
 * Refuses when the entry does not declare the field: there is no number to replace, and inserting
 * the key is the `addGlyph` problem (where does the line go, at what indentation) for a case no
 * authored model needs. The palette editor offers a material control only for a declared field.
 */
export function setPaletteMaterial(
  doc: VoxelDoc,
  glyph: string,
  field: MaterialField,
  value: number,
): VoxelEditResult {
  const entry = paletteEntry(doc, glyph);
  if (entry === null) {
    return fail('no-such-entry', `the palette has no entry for \`${glyph}\``);
  }
  if (!Number.isFinite(value)) {
    return fail('bad-number', `\`${field}\` must be a finite number, got ${value}`);
  }
  const current = entry[field];
  if (current === null) {
    return fail(
      'no-such-field',
      `\`${glyph}\` declares no addressable \`${field}\`, so there is no number to replace`,
    );
  }
  if (current.value === value) {
    return applied(doc.source, []);
  }
  return applied(doc.source, [replacement(doc.source, current.span, formatVoxelFloat(value))]);
}

/**
 * Inserts a new palette entry — the one edit here that adds lines rather than replacing characters.
 *
 * WHERE IT GOES. `emit_voxel`'s canonical order is sorted by glyph, but `doc.ts` preserves the
 * AUTHORED order and 7 of vigil's 19 models are not sorted (`house-a` is `N D R r G`, grouped by
 * material, not alphabet). So the rule is: **a sorted palette stays sorted — the entry lands in its
 * sorted position; an unsorted palette is appended to.** A sorted file stays canonical-adjacent and
 * the diff lands where a reader looks for it, while a file whose order is a deliberate grouping
 * keeps that grouping and gets the new entry somewhere obvious rather than somewhere arbitrary.
 * A comment block directly above the successor entry describes that entry, so the insertion goes
 * above the comment, never between it and its subject.
 *
 * HOW IT IS FORMATTED. Indentation is measured from the neighbouring entry, not assumed: the glyph
 * key line's indent, the `color:` key line's indent, and the `- ` item indent are read out of the
 * source, and the file's own line ending is reused. Fields are written in the canonical key order
 * (`color`, `emissive`, `metallic`, `roughness`) and floats through `formatVoxelFloat`. A glyph
 * needing YAML quoting (`#`, `:`, `-`, …) is written double-quoted; a plain one stays bare, as
 * every authored model writes it.
 *
 * Refuses on an empty palette: with no entry to measure, there is no honest indentation or position
 * to choose, and `doc.ts` does not span the `palette:` key itself.
 */
export function addGlyph(doc: VoxelDoc, entry: NewPaletteEntry): VoxelEditResult {
  const rejected = rejectNewEntry(doc, entry);
  if (rejected !== null) {
    return rejected;
  }
  const successor = doc.palette.findIndex((existing) => existing.glyph > entry.glyph);
  const sorted = doc.palette.every(
    (existing, index) => index === 0 || doc.palette[index - 1].glyph < existing.glyph,
  );
  const insertBefore = sorted && successor >= 0 ? doc.palette[successor] : null;
  const neighbour = insertBefore ?? doc.palette[doc.palette.length - 1];

  const layout = readLayout(doc, neighbour);
  if (layout === null) {
    return fail(
      'no-anchor',
      'the palette has no entry with a color to take indentation from; add the first entry by hand',
    );
  }
  const offset =
    insertBefore === null
      ? endOfEntry(doc.source, neighbour, layout.keyIndent)
      : startOfEntry(doc.source, insertBefore);
  const block = entryBlock(entry, layout);
  // A file that does not end in a newline (a truncated save) needs one before the new block.
  const lead = offset > 0 && doc.source[offset - 1] !== '\n' ? layout.eol : '';
  return applied(doc.source, [{ offset, removed: '', inserted: lead + block }]);
}

// --- Refusals --------------------------------------------------------------------------------- //

function fail(reason: VoxelEditReason, message: string): VoxelEditResult {
  return { ok: false, error: { reason, message } };
}

/** `null` when the glyph may be painted. */
function rejectGlyph(doc: VoxelDoc, glyph: string): VoxelEditResult | null {
  if (!isGlyphChar(glyph)) {
    return fail(
      'unknown-glyph',
      `\`${glyph}\` is not a palette glyph; a glyph is one printable-ASCII character`,
    );
  }
  if (glyph !== '.' && paletteEntry(doc, glyph) === null) {
    return fail('unknown-glyph', `\`${glyph}\` names no palette entry; paint with \`.\` to erase`);
  }
  return null;
}

/** The reason `cellOffset` returned `null`, as a refusal. */
function rejectCell(doc: VoxelDoc, paint: CellPaint): VoxelEditResult {
  const row = sliceRow(doc, paint.y, paint.z);
  if (row === null) {
    return fail(
      'out-of-bounds',
      `the model has no slice row at y=${paint.y}, z=${paint.z}`,
    );
  }
  if (!row.paintable) {
    return fail(
      'not-paintable',
      `slice row y=${paint.y}, z=${paint.z} carries an escape, so its characters do not map onto the source one for one; edit it by hand`,
    );
  }
  return fail(
    'out-of-bounds',
    `x=${paint.x} is outside slice row y=${paint.y}, z=${paint.z} (${row.text.length} cells wide)`,
  );
}

/** `null` when the new entry may be inserted. */
function rejectNewEntry(doc: VoxelDoc, entry: NewPaletteEntry): VoxelEditResult | null {
  if (!isGlyphChar(entry.glyph) || entry.glyph === '.') {
    return fail(
      'unknown-glyph',
      `\`${entry.glyph}\` cannot be a palette glyph; use one printable-ASCII character other than \`.\``,
    );
  }
  if (paletteEntry(doc, entry.glyph) !== null) {
    return fail('duplicate-glyph', `the palette already has an entry for \`${entry.glyph}\``);
  }
  for (const [index, value] of entry.color.entries()) {
    if (!Number.isFinite(value)) {
      return fail('bad-number', `color component ${index} must be a finite number, got ${value}`);
    }
  }
  for (const field of MATERIAL_FIELDS) {
    const value = entry[field];
    if (value !== undefined && value !== null && !Number.isFinite(value)) {
      return fail('bad-number', `\`${field}\` must be a finite number, got ${value}`);
    }
  }
  if (doc.palette.length === 0) {
    return fail(
      'no-anchor',
      'the palette is empty, so there is no entry to take indentation and position from; add the first entry by hand',
    );
  }
  return null;
}

/** One printable-ASCII character (`0x21..=0x7E`) — the format's own glyph rule. */
function isGlyphChar(glyph: string): boolean {
  const code = glyph.length === 1 ? glyph.charCodeAt(0) : 0;
  return code >= 0x21 && code <= 0x7e;
}

// --- Splicing --------------------------------------------------------------------------------- //

/** Applies ascending, non-overlapping splices. */
function applied(source: string, splices: readonly Splice[]): VoxelEditResult {
  return { ok: true, edit: { source: spliceSource(source, splices), splices } };
}

/** The text `splices` produce from `source`. They must be ascending and non-overlapping — every
 *  producer in this module emits them that way. */
export function spliceSource(source: string, splices: readonly Splice[]): string {
  if (splices.length === 0) {
    return source;
  }
  let out = '';
  let cursor = 0;
  for (const splice of splices) {
    out += source.slice(cursor, splice.offset) + splice.inserted;
    cursor = splice.offset + splice.removed.length;
  }
  return out + source.slice(cursor);
}

/**
 * The splices that undo `splices`: applied to the text they produced, they restore the text they
 * were applied to, byte for byte — `removed` records the exact characters that were there.
 *
 * An inverse is NOT just `removed` and `inserted` swapped: a splice that changes length moves every
 * later offset, so each inverse offset carries the net shift of the splices before it. Inverting
 * twice is the identity, which is what lets a redo be derived from the entry an undo consumed.
 */
export function invertSplices(splices: readonly Splice[]): Splice[] {
  const out: Splice[] = [];
  let shift = 0;
  for (const splice of splices) {
    out.push({ offset: splice.offset + shift, removed: splice.inserted, inserted: splice.removed });
    shift += splice.inserted.length - splice.removed.length;
  }
  return out;
}

/** A splice replacing exactly the characters a span covers. */
function replacement(
  source: string,
  span: { readonly offset: number; readonly length: number },
  inserted: string,
): Splice {
  return {
    offset: span.offset,
    removed: source.slice(span.offset, span.offset + span.length),
    inserted,
  };
}

// --- Insertion geometry ----------------------------------------------------------------------- //


/** The indentation and line ending a new entry must match, measured from an existing one. */
interface PaletteLayout {
  /** Indent of a glyph key line (`  D:`). */
  readonly keyIndent: number;
  /** Indent of a field key line (`    color:`). */
  readonly fieldIndent: number;
  /** Indent of a color sequence item (`      - 0.15`). */
  readonly itemIndent: number;
  readonly eol: string;
}

/** Reads the layout from `entry`, falling back to any entry that declares a color. */
function readLayout(doc: VoxelDoc, entry: PaletteEntry): PaletteLayout | null {
  const anchor = entry.color.length > 0 ? entry : doc.palette.find((e) => e.color.length > 0);
  if (anchor === undefined) {
    return null;
  }
  const source = doc.source;
  const keyIndent = indentAt(source, anchor.keySpan.offset);
  const itemIndent = indentAt(source, anchor.color[0].span.offset);
  return {
    keyIndent,
    itemIndent,
    fieldIndent: enclosingIndent(source, anchor.color[0].span.offset, itemIndent) ?? keyIndent + 2,
    eol: source.includes('\r\n') ? '\r\n' : '\n',
  };
}

/** The block of lines a new entry adds, ending in a line break. */
function entryBlock(entry: NewPaletteEntry, layout: PaletteLayout): string {
  const key = ' '.repeat(layout.keyIndent);
  const field = ' '.repeat(layout.fieldIndent);
  const item = ' '.repeat(layout.itemIndent);
  let out = `${key}${glyphKey(entry.glyph)}:${layout.eol}${field}color:${layout.eol}`;
  for (const value of entry.color) {
    out += `${item}- ${formatVoxelFloat(value)}${layout.eol}`;
  }
  for (const name of MATERIAL_FIELDS) {
    const value = entry[name];
    if (value !== undefined && value !== null) {
      out += `${field}${name}: ${formatVoxelFloat(value)}${layout.eol}`;
    }
  }
  return out;
}

/** A glyph as a mapping key: bare when it is plainly safe, double-quoted otherwise. */
function glyphKey(glyph: string): string {
  return /^[A-Za-z0-9_]$/.test(glyph) ? glyph : JSON.stringify(glyph);
}

/** The offset a new entry is inserted at to land BEFORE `entry` — above any comment block
 *  introducing it, because such a comment describes the entry it sits on. */
function startOfEntry(source: string, entry: PaletteEntry): number {
  let start = lineStart(source, entry.keySpan.offset);
  while (start > 0) {
    const previous = lineStart(source, start - 1);
    if (!isBlankOrComment(source.slice(previous, start))) {
      break;
    }
    start = previous;
  }
  return start;
}

/** The offset just past `entry`'s last line — where a new entry is appended. */
function endOfEntry(source: string, entry: PaletteEntry, keyIndent: number): number {
  const last = [...entry.color, entry.emissive, entry.metallic, entry.roughness].reduce(
    (end, field) => (field === null ? end : Math.max(end, field.span.offset + field.span.length)),
    entry.keySpan.offset,
  );
  let offset = lineEnd(source, last);
  while (offset < source.length) {
    const next = lineEnd(source, offset);
    const line = source.slice(offset, next);
    if (isBlankOrComment(line) || indentAt(source, offset) <= keyIndent) {
      break;
    }
    offset = next;
  }
  return offset;
}

/** The indent of the nearest line above `offset` that is shallower than `indent` — the key line a
 *  sequence item hangs under. */
function enclosingIndent(source: string, offset: number, indent: number): number | null {
  let start = lineStart(source, offset);
  while (start > 0) {
    const previous = lineStart(source, start - 1);
    const line = source.slice(previous, start);
    if (!isBlankOrComment(line)) {
      const outer = indentAt(source, previous);
      if (outer < indent) {
        return outer;
      }
    }
    start = previous;
  }
  return null;
}

/** Offset of the first character of the line containing `offset`. */
function lineStart(source: string, offset: number): number {
  return source.lastIndexOf('\n', offset - 1) + 1;
}

/** Offset just past the line break ending the line containing `offset`, or the end of the source. */
function lineEnd(source: string, offset: number): number {
  const nl = source.indexOf('\n', offset);
  return nl < 0 ? source.length : nl + 1;
}

/** Leading-space count of the line containing `offset`. */
function indentAt(source: string, offset: number): number {
  const start = lineStart(source, offset);
  let index = start;
  while (source[index] === ' ') {
    index += 1;
  }
  return index - start;
}

function isBlankOrComment(line: string): boolean {
  const text = line.trim();
  return text === '' || text.startsWith('#');
}
