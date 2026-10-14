// Carrying an edit into the working `VoxelDoc` WITHOUT re-reading the file.
//
// WHY THIS EXISTS. `parseVoxelDoc` reads the whole source; on a budget-exempt model that is ~3 MB
// and ~21k rows, and re-running it per painted cell is what made the painter collapse. But a paint
// stroke's splices are single characters INSIDE slice-row scalars, and one character replacing one
// character moves no offset — so the previous document is still correct everywhere except the rows
// the stroke touched. This module rebuilds exactly those rows and reuses the rest.
//
// WHEN THE FAST PATH REFUSES (and the caller falls back to a full parse — which is always correct,
// just slower). Every refusal is a case where the patched document could disagree with what
// `parseVoxelDoc` would read, and the three of them are the whole contract:
//
//   1. A splice that changes length. It moves every later offset, so every span after it is stale.
//      Palette numbers (`0.9` → `0.85`) and `addGlyph` are this, and both are rare.
//   2. A splice outside a paintable slice row. A palette edit that happens to preserve length
//      (`0.15` → `0.99`) lands in a `NumberField`, whose `value` this module does not recompute; a
//      row carrying an escape does not map 1:1 onto the source at all.
//   3. An inserted character that could re-tokenize the line — a quote, a backslash, or anything
//      outside printable ASCII. Painting `"` into a quoted row would close the quote early, and the
//      re-read would find a different row than the patch did.
//
// IDENTITIES ARE PART OF THE CONTRACT. The painter memoizes its span grid on the identity of the
// row array it draws, so a patch that returned fresh objects for untouched layers would defeat the
// very re-render isolation it exists to serve. The rule: the document is a new object, a touched
// layer and its row array are new objects, a touched row is a new object — and everything else
// (untouched layers, untouched rows, the palette, `size`, `scale`) keeps the identity it had. The
// previous document is never mutated; an undo of the edit must still find it intact.
//
// Pure: no React, no Tauri, no DOM.

import { parseVoxelDoc, type SliceRow, type VoxelDoc } from './doc';
import { spliceSource, type Splice } from './edit';

/**
 * `doc` with `splices` applied, or `null` when the fast path does not apply (see the module doc).
 *
 * `nextSource` is `spliceSource(doc.source, splices)`, which every caller already holds — passing it
 * spares a second rebuild of a multi-megabyte string.
 */
export function patchVoxelDoc(
  doc: VoxelDoc,
  splices: readonly Splice[],
  nextSource: string,
): VoxelDoc | null {
  if (splices.length === 0) {
    return doc;
  }
  const ranges = layerRanges(doc);
  // Layer index → row index → the row's patched text. Row indices, not `row.z`: `sliceRow` reads
  // `rows[z]` positionally and a malformed document can make the two disagree.
  const edits = new Map<number, Map<number, string>>();
  for (const splice of splices) {
    if (splice.inserted.length !== splice.removed.length || !isInert(splice.inserted)) {
      return null;
    }
    const layerIndex = rangeAt(ranges, splice.offset);
    if (layerIndex === null) {
      return null;
    }
    const rows = doc.layers[layerIndex].rows;
    const rowIndex = rowAt(rows, splice.offset);
    if (rowIndex === null) {
      return null;
    }
    const row = rows[rowIndex];
    const at = splice.offset - row.span.offset;
    if (!row.paintable || at + splice.removed.length > row.span.length) {
      return null;
    }
    // The splice's own record of what it replaced must be what is there, or the two paths diverge.
    if (!doc.source.startsWith(splice.removed, splice.offset)) {
      return null;
    }
    let byRow = edits.get(layerIndex);
    if (byRow === undefined) {
      byRow = new Map();
      edits.set(layerIndex, byRow);
    }
    const text = byRow.get(rowIndex) ?? row.text;
    byRow.set(rowIndex, text.slice(0, at) + splice.inserted + text.slice(at + splice.inserted.length));
  }
  const layers = doc.layers.map((layer, index) => {
    const byRow = edits.get(index);
    if (byRow === undefined) {
      return layer;
    }
    const rows = layer.rows.map((row, rowIndex) => {
      const text = byRow.get(rowIndex);
      // The span is unchanged (one character replaced one character), so it keeps its identity.
      return text === undefined ? row : { ...row, text };
    });
    return { y: layer.y, rows };
  });
  return { ...doc, source: nextSource, layers };
}

/**
 * `doc` with each of `steps` applied in order — the shape an undo or a redo arrives in, because one
 * gesture is many edits under one history entry.
 *
 * The document is derived ONCE, at the end: the intermediate sources are never parsed. When every
 * splice of every step preserves length the steps compose into one splice list and the fast path
 * takes it; otherwise the final source is parsed whole.
 */
export function applySpliceSteps(doc: VoxelDoc, steps: readonly (readonly Splice[])[]): VoxelDoc {
  let source = doc.source;
  for (const splices of steps) {
    source = spliceSource(source, splices);
  }
  if (source === doc.source) {
    return doc;
  }
  const composed = composeSplices(steps);
  const patched = composed === null ? null : patchVoxelDoc(doc, composed, source);
  return patched ?? parseVoxelDoc(source);
}

/**
 * The single splice list equivalent to applying `steps` in order, or `null` when any splice changes
 * length — offsets then shift between steps and are no longer comparable.
 *
 * Length-preserving steps compose per character: the first step to touch an offset supplies the
 * `removed` byte (what the composed list must restore), the last supplies the `inserted` one. A
 * character written and then written back drops out, which is what makes a stroke that re-crosses
 * its own start cost nothing.
 */
export function composeSplices(steps: readonly (readonly Splice[])[]): Splice[] | null {
  const byOffset = new Map<number, readonly [string, string]>();
  for (const splices of steps) {
    for (const splice of splices) {
      if (splice.inserted.length !== splice.removed.length) {
        return null;
      }
      for (let index = 0; index < splice.removed.length; index += 1) {
        const offset = splice.offset + index;
        const first = byOffset.get(offset);
        byOffset.set(offset, [first?.[0] ?? splice.removed[index], splice.inserted[index]]);
      }
    }
  }
  const offsets = [...byOffset.keys()].sort((a, b) => a - b);
  const out: Splice[] = [];
  for (const offset of offsets) {
    const [removed, inserted] = byOffset.get(offset) as readonly [string, string];
    if (removed === inserted) {
      continue;
    }
    // Adjacent offsets are adjacent source characters, so a run of painted cells becomes one splice.
    const last = out[out.length - 1];
    if (last !== undefined && last.offset + last.removed.length === offset) {
      out[out.length - 1] = {
        offset: last.offset,
        removed: last.removed + removed,
        inserted: last.inserted + inserted,
      };
      continue;
    }
    out.push({ offset, removed, inserted });
  }
  return out;
}

// --- Locating a splice ---------------------------------------------------------------------- //

/** The source extent of one layer's rows, for the layers that have any. */
interface LayerRange {
  readonly index: number;
  readonly start: number;
  readonly end: number;
}

function layerRanges(doc: VoxelDoc): LayerRange[] {
  const out: LayerRange[] = [];
  for (const [index, layer] of doc.layers.entries()) {
    const first = layer.rows[0];
    const last = layer.rows[layer.rows.length - 1];
    if (first === undefined || last === undefined) {
      continue;
    }
    out.push({ index, start: first.span.offset, end: last.span.offset + last.span.length });
  }
  return out;
}

/** The layer whose rows contain `offset`, or `null`. Ranges are ascending and disjoint in a document
 *  that parsed; a search that misses simply refuses the fast path. */
function rangeAt(ranges: readonly LayerRange[], offset: number): number | null {
  let low = 0;
  let high = ranges.length - 1;
  while (low <= high) {
    const mid = (low + high) >> 1;
    const range = ranges[mid];
    if (offset < range.start) {
      high = mid - 1;
    } else if (offset >= range.end) {
      low = mid + 1;
    } else {
      return range.index;
    }
  }
  return null;
}

/** The index of the row containing `offset`, or `null`. */
function rowAt(rows: readonly SliceRow[], offset: number): number | null {
  let low = 0;
  let high = rows.length - 1;
  while (low <= high) {
    const mid = (low + high) >> 1;
    const span = rows[mid].span;
    if (offset < span.offset) {
      high = mid - 1;
    } else if (offset >= span.offset + span.length) {
      low = mid + 1;
    } else {
      return mid;
    }
  }
  return null;
}

/** Printable ASCII other than the three characters that steer the scalar reader: `"` and `'` open or
 *  close a quoted scalar, `\` escapes inside one. Every palette glyph but those three qualifies. */
function isInert(text: string): boolean {
  for (let index = 0; index < text.length; index += 1) {
    const code = text.charCodeAt(index);
    if (code < 0x21 || code > 0x7e || code === 0x22 || code === 0x27 || code === 0x5c) {
      return false;
    }
  }
  return true;
}
