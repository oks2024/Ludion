// The slice painter: one layer of the model as the x×z character grid it literally is.
//
// WHY IT LOOKS LIKE THE FILE. `slices[y][z]` is a quoted fixed-width row and character `c` of that
// row is `x = c`, so rows run down the panel in FILE order (z = 0 first, which the format's own
// convention makes the front face, +Z) and the cell under the pointer is the character in the file.
// That is what makes the write obviously surgical rather than a round trip.
//
// WHAT A DRAG COSTS. One write, on release. A cell edit replaces one character with one character,
// so the whole stroke applies against one document (`edit.ts`) and the grid draws it live from the
// working text while the disk — and therefore the host watcher, and therefore the viewport — sees a
// single reload. There is no reload control here and there never will be one: the host is the only
// reloader (contract 3).
//
// WHAT A RENDER COSTS. A 128×112 layer is 14,336 spans, so the grid is memoized on the three things
// it is a function of — the drawn rows, the onion-skin rows, and the palette's derived views — and
// the component is wrapped in `memo` so the view's own camera/fps churn never reaches it. Everything
// those three are held by is stabilized by CONTENT (`cellview.ts`'s predicates), because a document
// is rebuilt on every edit and identity alone would rebuild the grid for a palette drag.
//
// Thin: the grid, the layer arithmetic, the stroke accumulation and the undo stack are all
// `paint.ts`; the writes are the session's (`useVoxelEdit.ts`). This file is pointers and markup.

import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from 'react';
import { cellView, glyphViews, sameGlyphViews, sameRows } from './cellview';
import { EMPTY_GLYPH, type VoxelDoc } from './doc';
import { setCells } from './edit';
import {
  gridBounds,
  layerCount,
  onionLayer,
  pivotLayers,
  sliceRows,
  stepLayer,
  strokeExtend,
  strokeStart,
  type GridBounds,
  type Stroke,
} from './paint';
import type { ApplyOptions, EditFn } from './useVoxelEdit';
import type { Vec3 } from '../gizmo/vecmath';

const NO_BOUNDS: GridBounds = { width: 0, depth: 0 };

/** A float cell coordinate as an offset inside the grid. The pitch is the cell plus the grid's own
 *  1px gap, so cell `n` starts at `n · (cell + gap)` and a fractional coordinate lands between. */
function cellOffset(cells: number): string {
  return `calc((var(--vx-cell) + 1px) * ${cells})`;
}

/** Keeps the previous value for as long as `same` says nothing visible moved, so a memo keyed on it
 *  survives a document object rebuilt around unchanged content. */
function useStable<T>(next: T, same: (a: T, b: T) => boolean): T {
  const held = useRef(next);
  if (!same(held.current, next)) {
    held.current = next;
  }
  return held.current;
}

interface SlicePainterPanelProps {
  /** The selected model's `voxels/`-relative path, or `null`. */
  readonly relPath: string | null;
  /** The working document — optimistic, so a stroke draws before it reaches the disk. */
  readonly doc: VoxelDoc | null;
  /** The glyph the swatch strip selected; `.` erases. */
  readonly glyph: string;
  readonly editable: boolean;
  readonly apply: (make: EditFn, options: ApplyOptions) => void;
  readonly canUndo: boolean;
  readonly canRedo: boolean;
  readonly onUndo: () => void;
  readonly onRedo: () => void;
  /** The file changed on disk under unsaved edits — writes are held until this is resolved. */
  readonly conflict: boolean;
  readonly onReload: () => void;
  readonly onKeepMine: () => void;
  /** The last refusal from the surgical editor, or a failed write. */
  readonly editError: string | null;
  readonly maximized: boolean;
  readonly onToggleMaximize: () => void;
  /** The panel head's own name. A grid nothing can paint must not call itself a painter. */
  readonly head?: string;
  /** Why this grid cannot be painted, said once, or `null` for the editable model. */
  readonly readOnlyNote?: string | null;
  /** A part's pivot in PART-LOCAL float voxel coordinates, drawn as a crosshair on the layers it
   *  crosses. Part-local because the grid draws the part as authored — the mirrored twin of a
   *  `mirror: x` part is a property of the assembled grid, not of this file. */
  readonly pivot?: Vec3 | null;
  /** Replaces the empty body's two lines where "pick a model" is not the right answer. */
  readonly emptyState?: { readonly title: string; readonly detail: string } | null;
}

export const SlicePainterPanel = memo(function SlicePainterPanel(props: SlicePainterPanelProps) {
  const { doc, glyph, editable, apply, conflict } = props;
  const [layer, setLayer] = useState(0);
  const strokeRef = useRef<Stroke | null>(null);
  // A new id per pointer-down: edits sharing it collapse into ONE undo entry, so a stroke is one
  // step back rather than one per cell.
  const strokeIdRef = useRef(0);
  // Read at pointer-down rather than closed over, so picking a swatch does not rebuild the grid.
  const glyphRef = useRef(glyph);
  glyphRef.current = glyph;

  // Two steps: the inner memo keeps `gridBounds`' layer walk off every render, the outer one keeps
  // the OBJECT stable across document rebuilds that did not move the extent.
  const extent = useMemo(() => (doc === null ? NO_BOUNDS : gridBounds(doc)), [doc]);
  const bounds = useMemo(
    () => ({ width: extent.width, depth: extent.depth }),
    [extent.width, extent.depth],
  );
  const layers = doc === null ? 0 : layerCount(doc);
  // Selecting a smaller model must not leave the painter pointing past the top.
  const y = doc === null ? 0 : stepLayer(layer, 0, layers);
  const rows = useStable(
    useMemo(() => (doc === null ? [] : sliceRows(doc, y, bounds)), [doc, y, bounds]),
    sameRows,
  );
  const below = doc === null ? null : onionLayer(doc, y);
  const onion = useStable(
    useMemo(
      () => (doc === null || below === null ? null : sliceRows(doc, below, bounds)),
      [doc, below, bounds],
    ),
    sameRows,
  );
  const views = useStable(useMemo(() => glyphViews(doc?.palette ?? []), [doc?.palette]), sameGlyphViews);

  const readOnlyNote = props.readOnlyNote ?? null;
  const paintable = editable && !conflict && doc !== null && bounds.width > 0;
  const pivot = props.pivot ?? null;
  // The crosshair is drawn at the true float position: a `.5` sits on a cell boundary, and snapping
  // it to a cell centre would move the joint by half a voxel in the one panel that shows it.
  const pivotHere = pivot !== null && pivotLayers(pivot[1], layers).includes(y) ? pivot : null;

  // A new model starts at the ground. Carrying the layer over would land a 48-tall model's index in
  // the middle of a 9-tall one, which reads as a jump rather than a place.
  const { relPath } = props;
  useEffect(() => {
    setLayer(0);
  }, [relPath]);

  const endStroke = useCallback((): void => {
    const stroke = strokeRef.current;
    strokeRef.current = null;
    if (stroke === null) {
      return;
    }
    // The cells are already in the working document; this call exists to flush the stroke to disk
    // (a no-op edit under a `commit` still writes — see `useVoxelEdit`).
    apply((current) => setCells(current, stroke.cells), {
      write: 'commit',
      coalesce: String(strokeIdRef.current),
    });
  }, [apply]);

  // A drag that leaves the grid still ends where the pointer is released.
  useEffect(() => {
    window.addEventListener('pointerup', endStroke);
    window.addEventListener('pointercancel', endStroke);
    return () => {
      window.removeEventListener('pointerup', endStroke);
      window.removeEventListener('pointercancel', endStroke);
    };
  }, [endStroke]);

  const paint = useCallback(
    (stroke: Stroke): void => {
      strokeRef.current = stroke;
      apply((current) => setCells(current, stroke.cells), {
        write: 'defer',
        coalesce: String(strokeIdRef.current),
      });
    },
    [apply],
  );

  const onCellDown = useCallback(
    (event: ReactPointerEvent, x: number, z: number): void => {
      if (!paintable) {
        return;
      }
      event.preventDefault();
      strokeIdRef.current += 1;
      // Right-drag erases without leaving the glyph you are painting with.
      paint(strokeStart(bounds, y, event.button === 2 ? EMPTY_GLYPH : glyphRef.current, x, z));
    },
    [paintable, bounds, y, paint],
  );

  const onCellEnter = useCallback(
    (x: number, z: number): void => {
      const stroke = strokeRef.current;
      if (stroke === null) {
        return;
      }
      const next = strokeExtend(stroke, bounds, x, z);
      if (next !== stroke) {
        paint(next);
      }
    },
    [bounds, paint],
  );

  const { onRedo, onUndo } = props;
  const onKeyDown = useCallback(
    (event: ReactKeyboardEvent): void => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'z') {
        event.preventDefault();
        (event.shiftKey ? onRedo : onUndo)();
        return;
      }
      if (event.key === '[' || event.key === ']') {
        event.preventDefault();
        setLayer(stepLayer(y, event.key === ']' ? 1 : -1, layers));
      }
    },
    [onRedo, onUndo, y, layers],
  );

  // The 14k spans. Everything it reads is content-stable, so this survives every render the document
  // did not cause — an fps tick, a camera pose, a swatch pick, the maximize toggle.
  const cells = useMemo(
    () =>
      rows.flatMap((row, z) =>
        [...row].map((cell, x) => {
          const view = cellView(cell, onion?.[z]?.[x] ?? EMPTY_GLYPH, views);
          return (
            <span
              key={`${z}-${x}`}
              className={view.classes}
              style={view.style}
              title={`x ${x} · z ${z} · ${cell === EMPTY_GLYPH ? 'empty' : cell}`}
              onPointerDown={(event) => onCellDown(event, x, z)}
              onPointerEnter={() => onCellEnter(x, z)}
            >
              {view.text}
            </span>
          );
        }),
      ),
    [rows, onion, views, onCellDown, onCellEnter],
  );

  return (
    <section
      className="codepanel vx-painter"
      tabIndex={0}
      onKeyDown={onKeyDown}
      onContextMenu={(event) => event.preventDefault()}
    >
      <div className="panel-head">
        <span className="code-title">
          {props.head ?? 'Slice painter'}
          <span className="code-path">{props.relPath ?? 'no model selected'}</span>
        </span>
        <span className="head-actions">
          {doc !== null && layers > 0 && (
            <span className="vx-layer-nav">
              <button
                type="button"
                className="icon-btn"
                title="Layer down ( [ )"
                disabled={y === 0}
                onClick={() => setLayer(stepLayer(y, -1, layers))}
              >
                ◀
              </button>
              <span className="vx-layer-label">
                y = <span className="num">{y}</span>
                <span className="vx-layer-of"> / {layers - 1}</span>
              </span>
              <button
                type="button"
                className="icon-btn"
                title="Layer up ( ] )"
                disabled={y >= layers - 1}
                onClick={() => setLayer(stepLayer(y, 1, layers))}
              >
                ▶
              </button>
            </span>
          )}
          <button type="button" className="icon-btn" disabled={!props.canUndo} onClick={props.onUndo} title="Undo (Ctrl+Z)">
            Undo
          </button>
          <button type="button" className="icon-btn" disabled={!props.canRedo} onClick={props.onRedo} title="Redo (Ctrl+Shift+Z)">
            Redo
          </button>
          <button type="button" className="icon-btn" onClick={props.onToggleMaximize}>
            {props.maximized ? 'Restore viewport' : 'Maximize'}
          </button>
        </span>
      </div>

      {conflict && (
        <div className="code-banner" role="alert">
          <span className="code-banner-text">
            This model changed on disk while you have unsaved edits. Writes are held until you choose.
          </span>
          <button type="button" className="icon-btn" onClick={props.onReload}>
            Reload
          </button>
          <button type="button" className="icon-btn" onClick={props.onKeepMine}>
            Keep mine
          </button>
        </div>
      )}
      {props.editError !== null && <div className="code-banner danger">{props.editError}</div>}

      {doc === null || bounds.width === 0 ? (
        <div className="code-empty">
          <span className="code-empty-title">
            {props.emptyState?.title ?? (props.relPath === null ? 'No model selected' : 'Nothing to paint')}
          </span>
          <span className="code-empty-sub">
            {props.emptyState?.detail ??
              (props.relPath === null
                ? 'Pick a model in the library to put it on the stage and open its slices here.'
                : 'This document has no slice rows the painter can read. The status bar carries the engine’s verdict.')}
          </span>
        </div>
      ) : (
        <div className="vx-painter-body">
          <div className="vx-grid-wrap">
            <div
              className={readOnlyNote === null ? 'vx-grid' : 'vx-grid vx-grid-ro'}
              style={{ gridTemplateColumns: `repeat(${bounds.width}, var(--vx-cell))` }}
              aria-label={`Layer ${y} of ${props.relPath ?? 'the model'}`}
            >
              {cells}
            </div>
            {pivotHere !== null && (
              <span
                className="vx-pivot"
                style={{ left: cellOffset(pivotHere[0]), top: cellOffset(pivotHere[2]) }}
                title={`pivot ${pivotHere.join(' ')}`}
                aria-label="pivot"
              />
            )}
          </div>

          <div className="vx-painter-foot">
            <span className="vx-toolbar-note">
              {readOnlyNote === null ? (
                <>
                  painting <span className="num">{glyph === EMPTY_GLYPH ? '. (erase)' : glyph}</span> ·
                  right-drag erases ·{' '}
                </>
              ) : (
                <>{readOnlyNote} · </>
              )}
              rows are <span className="num">slices[{y}][z]</span>, z down the panel, z = 0 the front face
            </span>
            {below !== null && (
              <span className="vx-toolbar-note">
                onion-skin: y = <span className="num">{below}</span>
              </span>
            )}
            {pivot !== null && (
              <span className="vx-toolbar-note">
                pivot <span className="num">{pivot.join(' ')}</span>
                {pivotHere === null && ' — not on this layer'}
              </span>
            )}
            {readOnlyNote === null && !paintable && props.relPath !== null && (
              <span className="vx-toolbar-note">
                {conflict ? 'resolve the banner above to paint' : 'read-only — the desktop editor writes files'}
              </span>
            )}
          </div>
        </div>
      )}
    </section>
  );
});
