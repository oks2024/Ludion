// The Game Designer view's code pane: a CodeMirror 6 editor over a resolved system source, wired to
// the `ludion dev` edit loop. It owns the save/watch state machine (fileSync.ts) and the dev-status
// integration (devStatus.ts) around the pure editor (RustEditor.tsx). Editing, Ctrl+F search, Ctrl+S
// save, a dirty dot, and a Save button; a build chip and inline rustc diagnostics from `ludion dev`;
// a non-blocking conflict banner when the file changes on disk under unsaved edits. A maximize toggle
// collapses the viewport above to a thumbnail strip. Host-backed (Tauri only) — the plain browser dev
// page shows the source read-only-ish (no file watch/save, since there is no host to serve them).

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ResolvedSource } from './locate';
import { isAbsolutePath, joinPath } from './locate';
import { RustEditor, type EditorDiag, type RustEditorHandle } from './RustEditor';
import { initialSyncState, reduce, type SyncEvent, type SyncState } from './fileSync';
import { diagnosticsForFile, isNewer, parseDevStatus, type DevStatus } from './devStatus';
import { readEditorFile, statEditorFile, writeEditorFile } from '../../terminal/pty';
import { isTauri } from '../../shell/tauri';

export type CodePanelStatus = 'idle' | 'loading' | 'ready' | 'error';

interface CodePanelProps {
  /** The selected system's name (panel header), or null when nothing is selected. */
  readonly systemName: string | null;
  /** The resolved source (path + text + target line), or null while idle/loading/errored. */
  readonly source: ResolvedSource | null;
  readonly status: CodePanelStatus;
  /** A human-readable reason shown in the error state. */
  readonly errorText: string | null;
  readonly maximized: boolean;
  readonly onToggleMaximize: () => void;
  /** The game project dir (root of `.ludion/dev-status.json` and relative rustc paths). */
  readonly projectDir: string;
  /** Open an arbitrary file at a line (a diagnostics-list click for another file). */
  readonly onOpenPath: (path: string, line: number) => void;
}

export function CodePanel({
  systemName,
  source,
  status,
  errorText,
  maximized,
  onToggleMaximize,
  projectDir,
  onOpenPath,
}: CodePanelProps) {
  const path = source?.path ?? null;
  const editorRef = useRef<RustEditorHandle>(null);

  // --- save/watch state machine (pure reduce in fileSync.ts) ---
  const [sync, setSyncState] = useState<SyncState>(initialSyncState);
  const syncRef = useRef<SyncState>(initialSyncState);
  const [saveError, setSaveError] = useState<string | null>(null);

  const setSync = useCallback((next: SyncState) => {
    syncRef.current = next;
    setSyncState(next);
  }, []);
  const applyEvent = useCallback(
    (event: SyncEvent) => {
      const { state, effect } = reduce(syncRef.current, event);
      if (effect.type === 'setContent') {
        editorRef.current?.replaceDoc(effect.content);
      }
      setSync(state);
    },
    [setSync],
  );

  // A new file resets the machine, then adopts its on-disk mtime as the baseline.
  useEffect(() => {
    setSync(initialSyncState);
    setSaveError(null);
    if (!isTauri() || path === null) {
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const st = await statEditorFile(path);
        if (!cancelled) {
          applyEvent({ type: 'loaded', mtime: st.mtimeMs });
        }
      } catch {
        // No baseline mtime — the first external change will reload rather than false-conflict.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [path, applyEvent, setSync]);

  // Watch the open file for external mtime changes (~1s). The reducer decides: silent reload when
  // clean, conflict banner when dirty. Our own saves adopt the returned mtime, so they don't re-fire.
  useEffect(() => {
    if (!isTauri() || path === null) {
      return;
    }
    const id = window.setInterval(() => {
      void (async () => {
        try {
          const st = await statEditorFile(path);
          if (st.mtimeMs === syncRef.current.baselineMtime) {
            return;
          }
          const content = await readEditorFile(path);
          applyEvent({ type: 'externalChange', mtime: st.mtimeMs, content });
        } catch {
          // File vanished mid-edit (e.g. a checkout) — ignore; a later poll recovers.
        }
      })();
    }, 1000);
    return () => window.clearInterval(id);
  }, [path, applyEvent]);

  const save = useCallback(() => {
    if (!isTauri() || path === null) {
      return;
    }
    const text = editorRef.current?.currentDoc() ?? '';
    void (async () => {
      try {
        const st = await writeEditorFile(path, text);
        applyEvent({ type: 'save', mtime: st.mtimeMs });
        setSaveError(null);
      } catch (e) {
        setSaveError(String(e));
      }
    })();
  }, [path, applyEvent]);

  const onEdit = useCallback(() => applyEvent({ type: 'edit' }), [applyEvent]);

  // --- dev-status integration (parse/seq-gate in devStatus.ts) ---
  const [devStatus, setDevStatus] = useState<DevStatus | null>(null);
  const lastSeqRef = useRef<number | null>(null);
  useEffect(() => {
    lastSeqRef.current = null;
    setDevStatus(null);
    if (!isTauri() || projectDir === '') {
      return;
    }
    const statusPath = `${projectDir.replace(/[/\\]+$/, '')}/.ludion/dev-status.json`;
    let cancelled = false;
    const poll = async () => {
      try {
        const text = await readEditorFile(statusPath);
        const parsed = parseDevStatus(text);
        if (!cancelled && parsed !== null && isNewer(lastSeqRef.current, parsed)) {
          lastSeqRef.current = parsed.seq;
          setDevStatus(parsed);
        }
      } catch {
        // No dev running / no status file — show nothing.
      }
    };
    void poll();
    const id = window.setInterval(() => void poll(), 1000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [projectDir]);

  const failedDiags = devStatus?.state === 'failed' ? devStatus.diagnostics : [];

  // Diagnostics for the OPEN file → in-editor decorations (gutter + underline + hover).
  const fileDiags: EditorDiag[] = useMemo(() => {
    if (path === null) {
      return [];
    }
    return diagnosticsForFile(failedDiags, path)
      .filter((d) => d.line !== null)
      .map((d) => ({
        line: d.line as number,
        column: d.column,
        severity: d.level === 'warning' ? 'warning' : 'error',
        message: d.message,
      }));
  }, [failedDiags, path]);

  const dirty = sync.phase !== 'clean';
  const ready = status === 'ready' && source !== null;

  return (
    <section className="codepanel">
      <div className="panel-head">
        <span className="code-title">
          {dirty && <span className="code-dirty" title="Unsaved changes" />}
          {systemName === null ? 'Code' : <span className="code-sysname">{systemName}</span>}
          {source !== null && <span className="code-path num">{source.path}</span>}
        </span>
        <div className="head-actions">
          {devStatus !== null && <BuildChip status={devStatus} />}
          <button
            type="button"
            className="icon-btn"
            onClick={save}
            disabled={!ready || !dirty || !isTauri()}
            title="Save (Ctrl+S) — ludion dev rebuilds"
          >
            Save
          </button>
          <button
            type="button"
            className="icon-btn"
            onClick={onToggleMaximize}
            title={maximized ? 'Restore the viewport' : 'Maximize the code panel'}
          >
            {maximized ? 'Restore' : 'Maximize'}
          </button>
        </div>
      </div>

      {sync.phase === 'conflict' && (
        <div className="code-banner" role="alert">
          <span className="code-banner-text">This file changed on disk while you have unsaved edits.</span>
          <button type="button" className="icon-btn" onClick={() => applyEvent({ type: 'reload' })}>
            Reload
          </button>
          <button type="button" className="icon-btn" onClick={() => applyEvent({ type: 'keepMine' })}>
            Keep mine
          </button>
        </div>
      )}
      {saveError !== null && <div className="code-banner danger">Save failed: {saveError}</div>}

      {ready ? (
        <>
          <div className="code-editor">
            <RustEditor
              key={source.path}
              initialDoc={source.text}
              targetLine={source.line}
              diagnostics={fileDiags}
              onChange={onEdit}
              onSave={save}
              handleRef={editorRef}
            />
          </div>
          {failedDiags.length > 0 && (
            <div className="code-diags">
              <div className="code-diags-head">
                <span className="num">{failedDiags.length}</span> build{' '}
                {failedDiags.length === 1 ? 'diagnostic' : 'diagnostics'}
              </div>
              <div className="code-diags-list">
                {failedDiags.map((d, i) => {
                  const isHere = d.file !== null && path !== null && diagnosticsForFile([d], path).length > 0;
                  const target = d.file === null ? null : isAbsolutePath(d.file) ? d.file : joinPath(projectDir, d.file);
                  return (
                    <button
                      type="button"
                      key={`${d.file ?? '?'}:${d.line ?? 0}:${i}`}
                      className={`code-diag-row${isHere ? ' here' : ''}`}
                      disabled={target === null}
                      onClick={() => target !== null && onOpenPath(target, d.line ?? 0)}
                      title={d.rendered ?? d.message}
                    >
                      <span className={`code-diag-sev ${d.level === 'warning' ? 'warning' : 'error'}`}>
                        {d.level === 'warning' ? 'warn' : 'error'}
                      </span>
                      <span className="code-diag-msg">{d.message}</span>
                      <span className="code-diag-loc num">
                        {d.file === null ? '' : `${shortFile(d.file)}${d.line !== null ? `:${d.line}` : ''}`}
                      </span>
                    </button>
                  );
                })}
              </div>
            </div>
          )}
        </>
      ) : (
        <div className="code-empty">
          {status === 'loading' && <span className="code-empty-sub">Resolving source…</span>}
          {status === 'idle' && (
            <span className="code-empty-sub">Select a system to read the Rust that drives it.</span>
          )}
          {status === 'error' && (
            <>
              <span className="code-empty-title">Source unavailable</span>
              <span className="code-empty-sub">{errorText ?? 'Could not locate the system source.'}</span>
            </>
          )}
        </div>
      )}
    </section>
  );
}

/** The last two path segments — a compact location for a diagnostics row. */
function shortFile(file: string): string {
  const parts = file.replace(/\\/g, '/').split('/').filter((s) => s.length > 0);
  return parts.slice(-2).join('/');
}

/** The `ludion dev` build state chip in the panel header. */
function BuildChip({ status }: { readonly status: DevStatus }) {
  const dur = status.durationMs !== null ? ` ${(status.durationMs / 1000).toFixed(1)}s` : '';
  switch (status.state) {
    case 'building':
      return (
        <span className="build-chip building" title="ludion dev is rebuilding">
          <span className="build-spinner" /> building
        </span>
      );
    case 'ok':
      return (
        <span className="build-chip ok" title="Last build succeeded">
          built{dur}
        </span>
      );
    case 'failed':
      return (
        <span className="build-chip failed" title="Last build failed">
          failed · <span className="num">{status.diagnostics.length}</span>
        </span>
      );
    case 'crashed':
      return (
        <span className="build-chip failed" title={status.message ?? 'The game crashed after reload'}>
          crashed
        </span>
      );
    case 'restore_failed':
      return (
        <span className="build-chip failed" title={status.message ?? 'The reload could not restore the world'}>
          not restored
        </span>
      );
  }
}
