// The editing session for one `.voxel.yaml`: who holds the working text, when it reaches the disk,
// and what happens when the persona edits the same file at the same time.
//
// The load, the change poll, the throttled write, the stat-before-write conflict check and the flush on
// dispose are `systems/syncedFile.ts`'s (its header states them). This hook adds what only a voxel
// editor has — the document reader/writer (`doc.ts` + `edit.ts`), the cell-patch fast path
// (`patch.ts`) and the undo stack (`paint.ts`).
//
// THREE PROPERTIES WORTH STATING, because they are what makes the two editors above it thin:
//
//   1. THE WORKING DOCUMENT IS OPTIMISTIC. An edit updates it immediately and the panels redraw from
//      it, so the grid and the swatches never wait on a disk round trip. The file catches up under
//      `save.ts`'s policy, and the host's watcher turns each write into the viewport update — this
//      hook NEVER calls `asset.reload` (contract 3: the host's watcher does the reloading).
//   2. THE EDIT IS RE-DERIVED, NOT REPLAYED. Every caller passes a function of the CURRENT document
//      rather than a precomputed result, because a palette edit moves offsets (`0.9` → `0.85` is a
//      character longer) and every span after it goes stale. How the result reaches the working
//      document is `patch.ts`'s decision, not this hook's: a cell edit patches the rows it touched,
//      an offset-moving edit re-reads the file.
//   3. A CONFLICT HOLDS THE WRITE. While the banner is up, nothing is written: `keepMine` flushes it,
//      `reload` discards it, and a reload also drops the undo history — the disk is the new root, and
//      undoing past it would resurrect text the human just chose to drop.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { isTauri } from '../../shell/tauri';
import { useSyncedFile } from '../systems/useSyncedFile';
import type { WriteMode } from '../systems/syncedFile';
import { parseVoxelDoc, type VoxelDoc } from './doc';
import type { VoxelEditResult } from './edit';
import { EMPTY_UNDO, opensUndoEntry, recordEdit, redoStep, undoStep, type UndoStack } from './paint';
import { applySpliceSteps, patchVoxelDoc } from './patch';

export type { WriteMode };

export interface ApplyOptions {
  readonly write: WriteMode;
  /** Edits sharing a key push ONE undo entry — a drag is one step back, not sixty. `null` (the
   *  default) always opens a new entry. */
  readonly coalesce?: string | null;
}

/** Builds the edit from the current working document. Returning a refusal is normal: it surfaces as
 *  `editError` and nothing is written. */
export type EditFn = (doc: VoxelDoc) => VoxelEditResult;

export interface VoxelEditSession {
  /** The working document — optimistic, and what every editing panel draws. */
  readonly doc: VoxelDoc | null;
  /** The file could not be read (or does not exist). */
  readonly loadError: string | null;
  /** The last refusal from `edit.ts` or failure from the host's writer, cleared by the next good
   *  edit. This is how a bad edit surfaces its diagnostic instead of looking like a hang. */
  readonly editError: string | null;
  /** True while the file changed on disk under unsaved local edits (the Reload / Keep mine banner). */
  readonly conflict: boolean;
  /** Local edits not yet on disk. */
  readonly dirty: boolean;
  readonly canUndo: boolean;
  readonly canRedo: boolean;
  /** Bumped whenever the DISK content changed (a load, an adopted external change, our own save) —
   *  the cue to re-run `check-scene`. */
  readonly savedRevision: number;
  /** False in the browser dev page (no host) or with nothing selected: every control disables. */
  readonly editable: boolean;
  readonly apply: (make: EditFn, options: ApplyOptions) => void;
  readonly undo: () => void;
  readonly redo: () => void;
  /** Banner "Reload": discard local edits, take the disk. */
  readonly reload: () => void;
  /** Banner "Keep mine": keep local edits and write them (last-writer-wins, the settled policy). */
  readonly keepMine: () => void;
}

export function useVoxelEdit(modelPath: string | null): VoxelEditSession {
  const [undo, setUndoState] = useState<UndoStack>(EMPTY_UNDO);
  const [refusal, setRefusal] = useState<string | null>(null);
  const undoRef = useRef<UndoStack>(EMPTY_UNDO);
  const coalesceRef = useRef<string | null>(null);

  const setUndo = useCallback((next: UndoStack): void => {
    undoRef.current = next;
    setUndoState(next);
  }, []);

  const syncOptions = useMemo(
    () => ({
      parse: parseVoxelDoc,
      source: (doc: VoxelDoc) => doc.source,
      onReplaced: () => {
        // The disk is the new history root.
        setUndo(EMPTY_UNDO);
        coalesceRef.current = null;
      },
    }),
    [setUndo],
  );
  const file = useSyncedFile(modelPath, syncOptions);
  const { current, edit, request } = file;

  // A new document starts a new history.
  useEffect(() => {
    setUndo(EMPTY_UNDO);
    coalesceRef.current = null;
    setRefusal(null);
  }, [modelPath, setUndo]);

  const apply = useCallback(
    (make: EditFn, options: ApplyOptions): void => {
      const doc = current();
      if (doc === null) {
        return;
      }
      const result = make(doc);
      if (!result.ok) {
        setRefusal(result.error.message);
        return;
      }
      setRefusal(null);
      if (result.edit.splices.length === 0) {
        // A no-op edit is not a write (it would cost the watcher a reload for nothing) — but a
        // commit still flushes, because the gesture it ends may have deferred earlier changes.
        request(options.write === 'commit' ? 'commit' : 'defer');
      } else {
        const coalesce = options.coalesce ?? null;
        const opens = opensUndoEntry(coalesceRef.current, coalesce);
        setUndo(recordEdit(undoRef.current, result.edit.splices, opens));
        coalesceRef.current = coalesce;
        // The fast path when the edit only rewrote characters inside slice rows (a paint stroke),
        // the full read when it moved offsets (a palette edit) — see `patch.ts`.
        edit(
          patchVoxelDoc(doc, result.edit.splices, result.edit.source) ?? parseVoxelDoc(result.edit.source),
          options.write,
        );
      }
      if (options.write === 'commit') {
        // The gesture is over: the next one opens its own undo entry even under the same key, so
        // two separate colour drags are two steps back rather than one.
        coalesceRef.current = null;
      }
    },
    [current, edit, request, setUndo],
  );

  const stepHistory = useCallback(
    (take: typeof undoStep): void => {
      const doc = current();
      if (doc === null) {
        return;
      }
      const stepped = take(undoRef.current);
      if (stepped === null) {
        return;
      }
      setUndo(stepped.stack);
      coalesceRef.current = null;
      setRefusal(null);
      edit(applySpliceSteps(doc, stepped.steps), 'commit');
    },
    [current, edit, setUndo],
  );

  const undoAction = useCallback(() => stepHistory(undoStep), [stepHistory]);
  const redoAction = useCallback(() => stepHistory(redoStep), [stepHistory]);

  return {
    doc: file.doc,
    loadError: file.loadError,
    editError: refusal ?? file.writeError,
    conflict: file.phase === 'conflict',
    dirty: file.phase !== 'clean',
    canUndo: undo.past.length > 0,
    canRedo: undo.future.length > 0,
    savedRevision: file.savedRevision,
    editable: isTauri() && modelPath !== null && file.doc !== null,
    apply,
    undo: undoAction,
    redo: redoAction,
    reload: file.reload,
    keepMine: file.keepMine,
  };
}
