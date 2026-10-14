// The React wrapper over `syncedFile.ts`: one session per path, the host's file calls, a 1 s change
// poll, and a dispose that flushes a pending edit when the path changes or the view unmounts. All
// policy lives in the session; this adds only the lifecycle. The one place an open document is written
// (`fileWrite-sites.test.ts`).

import { useCallback, useEffect, useRef, useState } from 'react';
import { isTauri } from '../../shell/tauri';
import { readEditorFile, statEditorFile, writeEditorFile } from '../../terminal/pty';
import { MODEL_POLL_MS } from '../voxel/stage';
import {
  EMPTY_SNAPSHOT,
  SyncedFileSession,
  type FileIo,
  type SyncedFileOptions,
  type SyncedFileSnapshot,
  type Timers,
  type WriteMode,
} from './syncedFile';

const HOST_IO: FileIo = {
  read: readEditorFile,
  stat: statEditorFile,
  write: (path, text) => writeEditorFile(path, text),
};

const WINDOW_TIMERS: Timers = {
  now: () => performance.now(),
  set: (fn, ms) => window.setTimeout(fn, ms),
  clear: (id) => window.clearTimeout(id),
};

export interface SyncedFile<D> extends SyncedFileSnapshot<D> {
  /** The working document, read synchronously — build every edit from this, not from React state. */
  readonly current: () => D | null;
  readonly edit: (next: D, mode: WriteMode) => void;
  readonly request: (mode: WriteMode) => void;
  readonly flush: () => Promise<void>;
  readonly reload: () => void;
  readonly keepMine: () => void;
}

/** `path` of `null` (nothing selected) or no host (the browser dev page) opens nothing. The latest
 *  `options` closure wins, so callers may pass fresh functions every render. */
export function useSyncedFile<D>(path: string | null, options: SyncedFileOptions<D>): SyncedFile<D> {
  const [snapshot, setSnapshot] = useState<SyncedFileSnapshot<D>>(EMPTY_SNAPSHOT);
  const sessionRef = useRef<SyncedFileSession<D> | null>(null);
  const optionsRef = useRef(options);
  optionsRef.current = options;
  // `savedRevision` stays monotonic across documents: a consumer keyed on it must see a change on
  // every switch, and a new session counts from zero.
  const revisionBase = useRef(0);
  const lastRevision = useRef(0);

  useEffect(() => {
    revisionBase.current += lastRevision.current;
    lastRevision.current = 0;
    setSnapshot(EMPTY_SNAPSHOT);
    if (path === null || !isTauri()) {
      return;
    }
    const session = new SyncedFileSession<D>(
      path,
      HOST_IO,
      WINDOW_TIMERS,
      {
        parse: (text) => optionsRef.current.parse(text),
        source: (doc) => optionsRef.current.source(doc),
        afterWrite: (written) => optionsRef.current.afterWrite?.(written),
        onReplaced: () => optionsRef.current.onReplaced?.(),
      },
      (next) => {
        lastRevision.current = next.savedRevision;
        setSnapshot(next);
      },
    );
    sessionRef.current = session;
    void session.load();
    const timer = window.setInterval(() => void session.poll(), MODEL_POLL_MS);
    return () => {
      window.clearInterval(timer);
      void session.dispose();
      sessionRef.current = null;
    };
  }, [path]);

  const current = useCallback(() => sessionRef.current?.current() ?? null, []);
  const edit = useCallback((next: D, mode: WriteMode) => sessionRef.current?.edit(next, mode), []);
  const request = useCallback((mode: WriteMode) => sessionRef.current?.request(mode), []);
  const flush = useCallback(() => sessionRef.current?.flush() ?? Promise.resolve(), []);
  const reload = useCallback(() => sessionRef.current?.reload(), []);
  const keepMine = useCallback(() => sessionRef.current?.keepMine(), []);

  return {
    ...snapshot,
    savedRevision: revisionBase.current + snapshot.savedRevision,
    current,
    edit,
    request,
    flush,
    reload,
    keepMine,
  };
}
