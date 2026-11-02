// The session strip's scene picker (`Plan/editor-open-project.md` decision 7): lists
// `<sceneRoot>/**/*.yaml` documents that are scenes, marks stage and generated ones, marks the
// current scene from `savePath`, and loads the chosen one with `scene.load` — confirming discard
// first when the editor is dirty. It never restarts the game; the load takes the foreign
// `scene.changed` path (`sceneChange.ts`), same as any other client's load or save.
//
// `sceneRoot` is the booted scene's own directory, not a hardcoded `<project>/scenes`: the engine
// resolves `scene.load`'s `path` against the parent of the scene it started on
// (`ludion-engine`'s `load_scene_file`), so a `default_scene` in a subfolder moves the root with it.
// A manually attached server booted on some other directory is not detected — this always lists the
// GAME SLOT's own content root, whatever the attached session actually is.

import { useEffect, useRef, useState } from 'react';
import { ask } from '@tauri-apps/plugin-dialog';
import type { ProtocolClient } from '../protocol/client';
import { toDiagnostic, type DiagnosticView } from '../viewer/diagnostic';
import { listEditorDir, type DirEntry } from './host';
import { readEditorFile } from '../terminal/pty';
import { docParts } from './docpath';
import { isConfinementRefusal } from './hostRefusal';
import { isTauri } from './tauri';
import { listScenes, pickScene, sameScenePath, type SceneEntry } from './sceneList';

export interface ScenePickerProps {
  readonly client: ProtocolClient;
  /** The booted scene's directory — the root `scene.load` resolves paths against. `''` renders
   *  nothing: no validated project has answered yet. */
  readonly sceneRoot: string;
  readonly dirty: boolean;
  readonly savePath: string;
  /** Surfaces a load failure in App's banner row (latest-wins; null clears). */
  readonly onError: (diagnostic: DiagnosticView | null) => void;
}

export function ScenePicker({ client, sceneRoot, dirty, savePath, onError }: ScenePickerProps) {
  const [open, setOpen] = useState(false);
  const [scenes, setScenes] = useState<readonly SceneEntry[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);

  // List (and classify) every scene under the project whenever the menu opens; a stale result from a
  // since-closed or reopened menu is dropped.
  useEffect(() => {
    if (!open) {
      return;
    }
    setScenes(null);
    setListError(null);
    let cancelled = false;
    listEditorDir(sceneRoot, '.yaml')
      .then((entries: DirEntry[]) => listScenes(entries, (rel) => readEditorFile(`${sceneRoot}/${rel}`)))
      .then((result) => {
        if (!cancelled) {
          setScenes(result);
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setListError(isConfinementRefusal(err) ? String(err) : 'No scenes folder in this project');
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open, sceneRoot]);

  // Escape and an outside pointerdown both close the popover (mirrors ProjectSwitcher).
  useEffect(() => {
    if (!open) {
      return;
    }
    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        setOpen(false);
      }
    };
    const onPointerDown = (e: PointerEvent): void => {
      if (rootRef.current !== null && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('pointerdown', onPointerDown);
    return () => {
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('pointerdown', onPointerDown);
    };
  }, [open]);

  const choose = async (relPath: string): Promise<void> => {
    try {
      const outcome = await pickScene(relPath, {
        dirty,
        savePath,
        confirm: (message) => ask(message, { title: 'Discard changes?', kind: 'warning' }),
        load: (path) => client.call({ method: 'scene.load', params: { path } }),
      });
      if (outcome === 'loaded') {
        onError(null);
      }
    } catch (err) {
      onError(toDiagnostic(err));
    } finally {
      setOpen(false);
    }
  };

  if (!isTauri() || sceneRoot === '') {
    return null;
  }

  const label = docParts(savePath).scene ?? 'Scenes';

  return (
    <div className="scene-picker" ref={rootRef}>
      <button type="button" className="btn" title={savePath === '' ? 'Pick a scene' : savePath} onClick={() => setOpen(!open)}>
        <span className="label">{label}</span>
        <span className="scene-picker-caret" aria-hidden="true">
          ▾
        </span>
      </button>
      {open && (
        <div className="scene-picker-popover">
          {listError !== null ? (
            <div className="scene-picker-empty">{listError}</div>
          ) : scenes === null ? (
            <div className="scene-picker-empty">Loading…</div>
          ) : scenes.length === 0 ? (
            <div className="scene-picker-empty">No scenes found</div>
          ) : (
            <div className="scene-picker-list">
              {scenes.map((entry) => {
                const current = sameScenePath(entry.relPath, savePath);
                return (
                  <button
                    key={entry.relPath}
                    type="button"
                    className={`scene-picker-row${current ? ' current' : ''}`}
                    title={entry.relPath}
                    onClick={() => void choose(entry.relPath)}
                  >
                    <span className="scene-picker-name">{entry.relPath}</span>
                    {entry.marks.stage && <span className="scene-picker-tag">stage</span>}
                    {entry.marks.generated && <span className="scene-picker-tag">generated</span>}
                  </button>
                );
              })}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
