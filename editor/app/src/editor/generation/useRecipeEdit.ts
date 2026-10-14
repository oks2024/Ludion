// The editing session for one `.gen.yaml`: who holds the working text, when it reaches the disk,
// and what happens when the persona edits the same file at the same time.
//
// The load, the change poll, the throttled write, the stat-before-write conflict check and the flush on
// dispose are `systems/syncedFile.ts`'s (its header states them). This hook adds the two writers a
// recipe needs — a number and a text field — over its own document type.
//
// THREE PROPERTIES WORTH STATING:
//
//   1. THE WORKING DOCUMENT IS OPTIMISTIC, AND ALWAYS RE-PARSED. A control updates it immediately,
//      and because the new token can be a different length than the old one (`8` → `12`, a prompt
//      rewritten), every field after it has moved — so the whole source is re-parsed on every edit
//      rather than patched. A recipe is a dozen lines; the honest reparse costs nothing.
//   2. THE FILE IS WHAT `ludion gen` READS. Unlike the model stage there is no watcher and no live
//      reload: the next `gen check` or `gen run` is a subprocess that opens this file off the disk.
//      So a write must land before a verb runs, which is why `generate` flushes first.
//   3. A CONFLICT HOLDS THE WRITE. While the banner is up nothing is written: `keepMine` flushes it,
//      `reload` discards it. The persona editing this same recipe while a human drags a control is
//      the expected case here, not an exotic one.

import { useCallback } from 'react';
import { isTauri } from '../../shell/tauri';
import { useSyncedFile } from '../systems/useSyncedFile';
import { parseRecipeDoc, type RecipeDoc } from './doc';
import { setNumber, setText } from './edit';

export interface RecipeEditSession {
  /** The working document — optimistic, and what the inspector draws. */
  readonly doc: RecipeDoc | null;
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
  /** True while edits have not reached the disk — what `flush` exists to clear. */
  readonly dirty: boolean;
  /** Writes `value` at `pointer`. `commit` is a gesture's end (a pointer release, a typed value): it
   *  always reaches the disk, where a mid-drag change only does if the throttle allows. */
  readonly setNumberAt: (pointer: string, value: number, commit: boolean) => void;
  /** Writes a string at `pointer`, always as one double-quoted line (see `edit.ts`). */
  readonly setTextAt: (pointer: string, value: string, commit: boolean) => void;
  /** Write the working text now and resolve once it has landed — what a verb run awaits, so the
   *  subprocess reads the file the human is looking at. */
  readonly flush: () => Promise<void>;
  /** Banner "Reload": discard local edits, take the disk. */
  readonly reload: () => void;
  /** Banner "Keep mine": keep local edits and write them (last-writer-wins, the settled policy). */
  readonly keepMine: () => void;
}

const SYNC_OPTIONS = { parse: parseRecipeDoc, source: (doc: RecipeDoc) => doc.source };

export function useRecipeEdit(recipePath: string | null): RecipeEditSession {
  const file = useSyncedFile(recipePath, SYNC_OPTIONS);
  const { current, edit, request } = file;

  /** Shared tail for both writers: adopt the edited source and schedule the write. A no-op edit is
   *  not a write, but a commit still flushes — the gesture it ends may have deferred an earlier
   *  change. */
  const applyEdit = useCallback(
    (source: string, changed: boolean, commit: boolean): void => {
      if (!changed) {
        if (commit) {
          request('commit');
        }
        return;
      }
      edit(parseRecipeDoc(source), commit ? 'commit' : 'throttle');
    },
    [edit, request],
  );

  const setNumberAt = useCallback(
    (pointer: string, value: number, commit: boolean): void => {
      const doc = current();
      if (doc === null) {
        return;
      }
      const result = setNumber(doc, pointer, value);
      applyEdit(result.source, result.splice !== null, commit);
    },
    [current, applyEdit],
  );

  const setTextAt = useCallback(
    (pointer: string, value: string, commit: boolean): void => {
      const doc = current();
      if (doc === null) {
        return;
      }
      const result = setText(doc, pointer, value);
      applyEdit(result.source, result.splice !== null, commit);
    },
    [current, applyEdit],
  );

  return {
    doc: file.doc,
    loadError: file.loadError,
    writeError: file.writeError,
    conflict: file.phase === 'conflict',
    savedRevision: file.savedRevision,
    editable: isTauri() && recipePath !== null && file.doc !== null,
    dirty: file.phase !== 'clean',
    setNumberAt,
    setTextAt,
    flush: file.flush,
    reload: file.reload,
    keepMine: file.keepMine,
  };
}
