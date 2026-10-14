// The editing session for one `.sculpt.yaml`: who holds the working text, when it reaches the disk,
// and what happens when the persona edits the same file at the same time.
//
// The load, the change poll, the throttled write, the stat-before-write conflict check and the flush on
// dispose are `systems/syncedFile.ts`'s (its header states them). This hook adds only what a scalar
// inspector needs: `setScalar` replaces one number token, so there is no stroke to coalesce, no cell
// patch to attempt and no undo stack — which is why `voxel/useVoxelEdit.ts` is not this hook.
//
// THREE PROPERTIES WORTH STATING:
//
//   1. THE WORKING DOCUMENT IS OPTIMISTIC, AND ALWAYS RE-PARSED. A slider updates it immediately, so
//      the readout never waits on a disk round trip — and because the new number's token can be a
//      different length than the old one (`0.9` → `0.85`), every field after it has moved. The whole
//      source is re-parsed on every edit rather than patched, so the next drag splices at the right
//      offset. A sculpt is a few hundred lines; the honest reparse costs nothing worth saving.
//   2. THE WRITE IS THE RELOAD. The host watches `<content-root>/sculpts/**` and turns each write
//      into an `asset.reload` on the stage session — this hook never calls it (the pattern editor's
//      own writes are the one exception, in its own module). So the rate the disk sees is the rate the
//      stage re-meshes, which is why a drag writes on `save.ts`'s throttle and always writes the
//      value the gesture ended on.
//   3. A CONFLICT HOLDS THE WRITE. While the banner is up, nothing is written: `keepMine` flushes it,
//      `reload` discards it. The persona editing this same file while a human drags a slider is the
//      expected case here, not an exotic one.

import { useCallback } from 'react';
import { isTauri } from '../../shell/tauri';
import { useSyncedFile } from '../systems/useSyncedFile';
import { parseSculptDoc, type SculptDoc } from './doc';
import { setScalar } from './edit';

export interface SculptEditSession {
  /** The working document — optimistic, and what the inspector draws. */
  readonly doc: SculptDoc | null;
  /** The file could not be read (or does not exist). */
  readonly loadError: string | null;
  /** The last failure from the host's writer, cleared by the next good write. */
  readonly writeError: string | null;
  /** True while the file changed on disk under unsaved local edits (the Reload / Keep mine banner). */
  readonly conflict: boolean;
  /** Bumped whenever the DISK content changed (a load, an adopted external change, our own save). */
  readonly savedRevision: number;
  /** False in the browser dev page (no host) or with nothing selected: every control disables. */
  readonly editable: boolean;
  /** Writes `value` at `pointer`. `commit` is a gesture's end (a pointer release, a typed value):
   *  it always reaches the disk, where a mid-drag change only does if the throttle allows. */
  readonly setScalarAt: (pointer: string, value: number, commit: boolean) => void;
  /** Banner "Reload": discard local edits, take the disk. */
  readonly reload: () => void;
  /** Banner "Keep mine": keep local edits and write them (last-writer-wins, the settled policy). */
  readonly keepMine: () => void;
}

const SYNC_OPTIONS = { parse: parseSculptDoc, source: (doc: SculptDoc) => doc.source };

export function useSculptEdit(modelPath: string | null): SculptEditSession {
  const file = useSyncedFile(modelPath, SYNC_OPTIONS);
  const { current, edit, request } = file;

  const setScalarAt = useCallback(
    (pointer: string, value: number, commit: boolean): void => {
      const doc = current();
      if (doc === null) {
        return;
      }
      const result = setScalar(doc, pointer, value);
      if (result.splice === null) {
        // A no-op edit is not a write (it would cost the watcher a reload for nothing) — but a
        // commit still flushes, because the gesture it ends may have deferred earlier changes.
        if (commit) {
          request('commit');
        }
        return;
      }
      edit(parseSculptDoc(result.source), commit ? 'commit' : 'throttle');
    },
    [current, edit, request],
  );

  return {
    doc: file.doc,
    loadError: file.loadError,
    writeError: file.writeError,
    conflict: file.phase === 'conflict',
    savedRevision: file.savedRevision,
    editable: isTauri() && modelPath !== null && file.doc !== null,
    setScalarAt,
    reload: file.reload,
    keepMine: file.keepMine,
  };
}
