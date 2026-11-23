// The `scene.changed` event's decision, extracted from `App.tsx`'s handler so the own-vs-foreign
// distinction the scene picker depends on (`Plan/editor-open-project.md` decision 7) is testable
// without a client or a subscription. Pure.

import type { SceneChangedData } from '@ludion/protocol';

export interface SceneChangeOutcome {
  /** Drop the undo stack, selection and pointer: the world was replaced, so entity ids may be stale. */
  readonly resetEditState: boolean;
  /** The served world now equals what is on disk (or was replaced), so the dirty mark goes. */
  readonly clearDirty: boolean;
  /** The new Save path; null leaves it unchanged, `''` clears it. */
  readonly savePath: string | null;
  /** The matched id in `ownSaves`, to delete from it. */
  readonly ownSaveId: number | null;
}

/** A save the world survives: only a `save` event leaves entity ids valid. Our own save (its
 *  request id was registered with the path it sent, before it was sent; `scene.save` echoes that
 *  path verbatim) clears dirty; a foreign save of the served path does too, since the file now holds
 *  the live world. A foreign save elsewhere changes nothing here, even if it reuses one of our ids. */
export function sceneChangeOutcome(
  data: SceneChangedData,
  ownSaves: ReadonlyMap<number, string>,
  currentSavePath: string,
): SceneChangeOutcome {
  const path = data.path ?? null;
  if (data.kind === 'save') {
    const id = data.request_id ?? null;
    if (path !== null && id !== null && ownSaves.get(id) === path) {
      return { resetEditState: false, clearDirty: true, savePath: path, ownSaveId: id };
    }
    if (path !== null && path === currentSavePath) {
      return { resetEditState: false, clearDirty: true, savePath: null, ownSaveId: null };
    }
    if (currentSavePath === '' && path !== null) {
      return { resetEditState: false, clearDirty: false, savePath: path, ownSaveId: null };
    }
    return { resetEditState: false, clearDirty: false, savePath: null, ownSaveId: null };
  }
  // The save path follows the world; a switch, restore or checkpoint restore with no path clears it so Ctrl+S cannot
  // write the old file over a world that no longer comes from it. Only a pathless load keeps it.
  const savePath = path ?? (data.kind === 'load' ? null : '');
  return { resetEditState: true, clearDirty: true, savePath, ownSaveId: null };
}
