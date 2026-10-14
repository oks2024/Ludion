// Reading one file and keeping it current — the read-only half of `useVoxelEdit`.
//
// An assembly, its parts and its clips are viewed, never written here, so the editing session's
// working document, undo stack, write policy and conflict reducer would all be machinery with no
// writer behind them. What is still needed is the poll: the persona and a text editor edit these
// files while they are on screen, and the panels must follow without a click. The stage follows
// separately through the host's watcher — nothing here ever calls `asset.reload` (contract 3).
//
// One file per hook, on the same cadence the editing session polls at. `revision` bumps only when
// the bytes changed, so a memo keyed on `text` does not rebuild on a poll that found nothing.

import { useEffect, useState } from 'react';
import { isTauri } from '../../shell/tauri';
import { readEditorFile, statEditorFile } from '../../terminal/pty';
import { MODEL_POLL_MS } from './stage';

export interface WatchedFile {
  /** The file's text, or `null` before the first read (and after a failure). */
  readonly text: string | null;
  readonly error: string | null;
  /** Bumped on every change the poll adopted — the cue for anything derived from a read of the
   *  file rather than from `text` itself. */
  readonly revision: number;
}

const NOTHING: WatchedFile = { text: null, error: null, revision: 0 };

export function useWatchedFile(path: string | null): WatchedFile {
  const [state, setState] = useState<WatchedFile>(NOTHING);

  useEffect(() => {
    setState(NOTHING);
    if (path === null || !isTauri()) {
      return;
    }
    let cancelled = false;
    let baseline: number | null = null;

    const adopt = (text: string, mtime: number): void => {
      baseline = mtime;
      if (!cancelled) {
        setState((current) => ({ text, error: null, revision: current.revision + 1 }));
      }
    };

    void (async () => {
      try {
        const [text, stat] = await Promise.all([readEditorFile(path), statEditorFile(path)]);
        adopt(text, stat.mtimeMs);
      } catch (err) {
        if (!cancelled) {
          setState({ text: null, error: String(err), revision: 0 });
        }
      }
    })();

    const timer = window.setInterval(() => {
      void (async () => {
        try {
          const stat = await statEditorFile(path);
          if (stat.mtimeMs === baseline) {
            return;
          }
          adopt(await readEditorFile(path), stat.mtimeMs);
        } catch {
          // A file that vanished under a half-typed reference is the readout's story to tell, not
          // the poll's — and the next good save resolves it without a click.
        }
      })();
    }, MODEL_POLL_MS);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [path]);

  return state;
}
