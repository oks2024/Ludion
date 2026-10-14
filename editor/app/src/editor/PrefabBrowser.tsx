// The Level Designer's prefab list: every `*.prefab.yaml` under the content root, one drag from the
// viewport. Listing is host file access (the shared `AssetBrowser`); placing is `prefab.spawn`, run by
// App from `onPlace`. The drag is pointer events on window listeners, not HTML5 DnD — Tauri's webview
// intercepts native drag/drop on Windows, and the outliner drags the same way.

import { useCallback, useEffect, useRef, useState } from 'react';
import type { EditorMode } from '../App';
import { AssetBrowser } from './voxel/AssetBrowser';
import { instanceBaseName } from './prefab-place';

const PREFAB_EXT = '.prefab.yaml';
const DRAG_THRESHOLD_PX = 4;

interface PrefabBrowserProps {
  /** The content root (`prefab.spawn` resolves paths against it), or `''` before a project validates. */
  readonly root: string;
  readonly mode: EditorMode;
  /** A prefab row was dropped at a client position; the caller decides whether that is on the viewport. */
  readonly onPlace: (relPath: string, clientX: number, clientY: number) => void;
}

interface Ghost {
  readonly label: string;
  readonly x: number;
  readonly y: number;
}

export function PrefabBrowser({ root, mode, onPlace }: PrefabBrowserProps) {
  const [selected, setSelected] = useState<string | null>(null);
  const [ghost, setGhost] = useState<Ghost | null>(null);
  const cancelRef = useRef<(() => void) | null>(null);

  // Unmounting (a view switch) mid-drag must not leave listeners or the body cursor behind.
  useEffect(() => () => cancelRef.current?.(), []);

  const onFilePointerDown = useCallback(
    (relPath: string, e: React.PointerEvent): void => {
      if (mode !== 'edit' || e.button !== 0) {
        return;
      }
      const startX = e.clientX;
      const startY = e.clientY;
      let active = false;
      const label = instanceBaseName(relPath);
      const onMove = (ev: PointerEvent): void => {
        if (!active && Math.hypot(ev.clientX - startX, ev.clientY - startY) >= DRAG_THRESHOLD_PX) {
          active = true;
          document.body.style.cursor = 'copy';
        }
        if (active) {
          setGhost({ label, x: ev.clientX, y: ev.clientY });
        }
      };
      const cleanup = (): void => {
        window.removeEventListener('pointermove', onMove);
        window.removeEventListener('pointerup', onUp);
        window.removeEventListener('keydown', onKey);
        document.body.style.cursor = '';
        cancelRef.current = null;
        setGhost(null);
      };
      const onUp = (ev: PointerEvent): void => {
        const wasActive = active;
        cleanup();
        if (wasActive) {
          onPlace(relPath, ev.clientX, ev.clientY);
        }
      };
      const onKey = (ev: KeyboardEvent): void => {
        if (ev.key === 'Escape') {
          cleanup();
        }
      };
      cancelRef.current = cleanup;
      window.addEventListener('pointermove', onMove);
      window.addEventListener('pointerup', onUp);
      window.addEventListener('keydown', onKey);
    },
    [mode, onPlace],
  );

  return (
    <>
      <AssetBrowser
        root={root}
        ext={PREFAB_EXT}
        selected={selected}
        onSelect={setSelected}
        prune
        head="Prefabs"
        counted="prefabs"
        onFilePointerDown={onFilePointerDown}
      />
      {ghost !== null && (
        <div className="drag-ghost" style={{ left: ghost.x + 12, top: ghost.y + 12 }}>
          <span className="glyph">◆</span>
          {ghost.label}
        </div>
      )}
    </>
  );
}
