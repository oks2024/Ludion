// The Models view's session plumbing, as pure data: where its model files live and how a scene
// names one.
//
// IT IS THE SAME STAGE. A sculpt is the second `Mesh.shape` model kind beside a voxel grid, so the
// view puts one on the very stage the Voxel Artist uses — `model-stage.yaml`, the `model` host slot,
// `voxel/stage.ts`'s `stageScenePath` and `stageSlot` unchanged. Only the library differs, which is
// why this module is paths and one extension rather than a second copy of the session logic.
//
// The content root is the stage scene's directory (`<projectDir>/scenes`), so a model at
// `<projectDir>/scenes/sculpts/idol.sculpt.yaml` is referenced on the wire as exactly
// `sculpts/idol.sculpt.yaml` — byte-identical to how a real scene names it.
//
// Pure: no React, no Tauri, no DOM.

import { join } from '../voxel/stage';
import { displayName } from '../voxel/tree';

/** The sculpt format's double extension — what the asset browser lists and nothing else. */
export const SCULPT_EXT = '.sculpt.yaml';

/** The directory the asset browser lists (and the host watches). */
export function sculptsRoot(projectDir: string): string {
  return join(projectDir, 'scenes', 'sculpts');
}

/** The absolute path of a sculpt, from its `sculptsRoot`-relative path — what `readEditorFile` and
 *  `runCheckScene` take. */
export function sculptFilePath(projectDir: string, relPath: string): string {
  return join(sculptsRoot(projectDir), relPath);
}

/** The sculpt's asset reference as a scene names it: `sculpts/<relPath>`, resolved against the stage
 *  scene's own directory. This is the value written into `Mesh.shape.sculpt`. */
export function sculptAssetRef(relPath: string): string {
  return join('sculpts', relPath);
}

/** The sculpt's display name — the basename with the format's double extension stripped. */
export function sculptDisplayName(relPath: string): string {
  return displayName(relPath, SCULPT_EXT);
}
