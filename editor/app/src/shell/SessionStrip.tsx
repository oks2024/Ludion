// The session strip (Meridian): the edit/play toggle, save, the scene picker, undo/redo, and the
// connection chip. It replaces the old SessionPanel's visual layout while keeping its behavior —
// every control is a public protocol command (the parity pillar), state updates only on success, and
// failures surface as a diagnostic (routed up to App's banner row via `onError`, since the 42px strip
// has no room inline).
//
// Edit mode pauses the sim (sim.pause) and play resumes it (sim.resume) — an editor convention, not an
// engine one (Plan/phase-6-editing.md decision #6). Save writes the served scene to `savePath` (the
// path the `scene.changed` event prefilled; disabled for a `generated_from:` scene, naming its
// generator); it also binds Ctrl+S. The run controls (step/speed/hash)
// the old panel exposed have no home in this design and were retired — the Edit/Play toggle preserves
// the pause/resume semantics that mattered for editing.

import { useCallback, useEffect, useState } from 'react';
import type { ServerHello } from '@ludion/protocol';
import type { ProtocolClient } from '../protocol/client';
import type { EditorMode } from '../App';
import { toDiagnostic, type DiagnosticView } from '../viewer/diagnostic';
import { isTextEditingTarget } from '../viewer/keymap';
import { readEditorFile } from '../terminal/pty';
import { ScenePicker } from './ScenePicker';
import { generatorOf, saveGate, type GeneratorState } from './sceneList';
import { isSaveConflict, saveFindings } from './saveResult';
import { isTauri } from './tauri';

interface SessionStripProps {
  readonly client: ProtocolClient;
  readonly hello: ServerHello;
  readonly url: string;
  /** The booted scene's directory — the root `scene.load` resolves paths against. `''` before a
   *  validated project answers. */
  readonly sceneRoot: string;
  readonly onDisconnect: () => void;
  readonly mode: EditorMode;
  readonly onModeChange: (mode: EditorMode) => void;
  readonly dirty: boolean;
  readonly savePath: string;
  readonly onSaved: () => void;
  /** Fired synchronously with the `scene.save` request id, before it is sent, so App recognizes the
   *  resulting `scene.changed` as ours; returns a cancel fn to call if the save is rejected. */
  readonly onSaveStarted: (requestId: number) => () => void;
  readonly canUndo: boolean;
  readonly canRedo: boolean;
  readonly onUndo: () => void;
  readonly onRedo: () => void;
  /** Surfaces a mode/save command failure in App's banner row (latest-wins; null clears). */
  readonly onError: (diagnostic: DiagnosticView | null) => void;
}

export function SessionStrip({
  client,
  hello,
  url,
  sceneRoot,
  onDisconnect,
  mode,
  onModeChange,
  dirty,
  savePath,
  onSaved,
  onSaveStarted,
  canUndo,
  canRedo,
  onUndo,
  onRedo,
  onError,
}: SessionStripProps) {
  const handleModeChange = useCallback(
    async (next: EditorMode): Promise<void> => {
      if (next === mode) {
        return;
      }
      try {
        await client.call({ method: next === 'edit' ? 'sim.pause' : 'sim.resume' });
        onError(null);
        onModeChange(next);
      } catch (err) {
        onError(toDiagnostic(err));
      }
    },
    [client, mode, onModeChange, onError],
  );

  // The loaded scene's generator, read from its file through the host (no protocol call): a scene
  // bearing `generated_from:` is never saved. Re-derived whenever the served scene changes, so loading
  // a hand-authored scene re-enables Save.
  // Save stays off while the read is pending or failed (`GeneratorState`); off the host there is no
  // file to read, so the scene counts as hand-authored.
  const [generator, setGenerator] = useState<GeneratorState>({ kind: 'unknown' });
  useEffect(() => {
    if (!isTauri()) {
      setGenerator({ kind: 'none' });
      return;
    }
    setGenerator({ kind: 'unknown' });
    if (sceneRoot === '' || savePath.trim() === '') {
      return;
    }
    let cancelled = false;
    readEditorFile(`${sceneRoot}/${savePath}`)
      .then((text) => {
        if (!cancelled) {
          const script = generatorOf(text);
          setGenerator(script === null ? { kind: 'none' } : { kind: 'generated', script });
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setGenerator({ kind: 'error', message: err instanceof Error ? err.message : String(err) });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [sceneRoot, savePath]);

  const { canSave, title: saveTitle } = saveGate(savePath, generator);

  // A refused save (the file changed on disk since this session loaded or saved it) waits here for
  // the human's choice; nothing was written.
  const [conflict, setConflict] = useState(false);
  useEffect(() => setConflict(false), [savePath]);

  const handleSave = useCallback(
    async (force = false): Promise<void> => {
      if (!canSave) {
        return;
      }
      let cancel = (): void => {};
      try {
        const { body } = await client.callFull(
          { method: 'scene.save', params: { path: savePath, ...(force ? { force: true } : {}) } },
          (id) => {
            cancel = onSaveStarted(id);
          },
        );
        setConflict(false);
        onError(saveFindings(body));
        onSaved();
      } catch (err) {
        cancel();
        const view = toDiagnostic(err);
        if (isSaveConflict(view)) {
          onError(null);
          setConflict(true);
        } else {
          onError(view);
        }
      }
    },
    [client, canSave, savePath, onSaved, onSaveStarted, onError],
  );

  const handleReload = useCallback(async (): Promise<void> => {
    try {
      await client.call({ method: 'scene.load', params: { path: savePath } });
      setConflict(false);
      onError(null);
    } catch (err) {
      onError(toDiagnostic(err));
    }
  }, [client, savePath, onError]);

  // Ctrl+S saves (any mode, whenever a path is known) — never while a text field is focused, so the
  // browser's own save-page shortcut is only overridden when the editor is the intended target.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey || event.key.toLowerCase() !== 's') {
        return;
      }
      if (isTextEditingTarget(event.target)) {
        return;
      }
      event.preventDefault();
      void handleSave();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [handleSave]);

  const schemaShort = hello.schema_hash.slice(0, 7);

  // A fragment: the banner is a sibling row of the 42px bar in `.app`'s column, not inside it.
  return (
    <>
      <div className="sessionbar">
        <div className="segmented" role="group" aria-label="Editor mode">
          <button
            type="button"
            className={mode === 'edit' ? 'active' : undefined}
            onClick={() => void handleModeChange('edit')}
          >
            Edit
          </button>
          <button
            type="button"
            className={mode === 'play' ? 'active' : undefined}
            onClick={() => void handleModeChange('play')}
          >
            Play
          </button>
        </div>

        <button
          type="button"
          className="btn"
          disabled={!canSave}
          title={saveTitle}
          onClick={() => void handleSave()}
        >
          {dirty && <span className="dirty-dot" title="unsaved changes" />}
          <span className="label">Save</span>
          <span className="kc">Ctrl+S</span>
        </button>

        <ScenePicker client={client} sceneRoot={sceneRoot} dirty={dirty} savePath={savePath} onError={onError} />

        <div className="btn-group" role="group" aria-label="History">
          <button type="button" className="btn" title="Undo" disabled={mode !== 'edit' || !canUndo} onClick={onUndo}>
            <span className="label">Undo</span>
            <span className="kc">Ctrl+Z</span>
          </button>
          <button type="button" className="btn" title="Redo" disabled={mode !== 'edit' || !canRedo} onClick={onRedo}>
            <span className="label">Redo</span>
            <span className="kc">Ctrl+Y</span>
          </button>
        </div>

        <button type="button" className="conn-chip" title="Disconnect from the engine" onClick={onDisconnect}>
          <span className="conn-dot" />
          <span className="num">{url}</span>
          <span className="sep">·</span>schema <span className="num">{schemaShort}</span>
          <span className="sep">·</span>connected
        </button>
      </div>
      {conflict && (
        <div className="code-banner" role="alert">
          <span className="code-banner-text">
            This scene changed on disk since this session loaded or saved it. Nothing was written. Choose.
          </span>
          <button type="button" className="icon-btn" onClick={() => void handleReload()}>
            Reload
          </button>
          <button type="button" className="icon-btn" onClick={() => void handleSave(true)}>
            Keep mine
          </button>
        </div>
      )}
    </>
  );
}
