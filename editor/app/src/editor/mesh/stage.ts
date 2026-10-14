// The Meshes library's paths, as pure data: where a `kind: mesh` document lives and how a scene
// names one.
//
// IT IS THE SAME STAGE the Voxel Artist and the sculpt library use — `model-stage.yaml`, the `model`
// host slot, `voxel/stage.ts`'s `stageScenePath` and `stageSlot` unchanged. Only the library differs.
//
// THE LIBRARY ROOT *IS* THE CONTENT ROOT. `Engine::load_scene_file` takes the scene's parent
// directory as the project root (`crates/ludion-engine/src/builder.rs`) and `ludion_assets`' resolve
// is a plain `root.join` (`crates/ludion-assets/src/lib.rs`), so the stage scene at
// `<projectDir>/scenes/model-stage.yaml` resolves every `Mesh.shape.asset` against
// `<projectDir>/scenes`. That is the directory this library lists, which is why a mesh's asset
// reference is its listing path verbatim rather than a prefixed one — unlike the voxel and sculpt
// libraries, which list a subdirectory and have to prepend it.
//
// Pure: no React, no Tauri, no DOM.

import { join } from '../voxel/stage';
import { displayName } from '../voxel/tree';

/** The mesh format's double extension — what the asset browser lists and nothing else. Note it does
 *  NOT match `.mesh-edit.yaml`: an op list is not a model. */
export const MESH_EXT = '.mesh.yaml';

/** The directory the asset browser lists — the stage session's content root, so every mesh document
 *  anywhere under it is reachable. */
export function meshesRoot(projectDir: string): string {
  return join(projectDir, 'scenes');
}

/** The absolute path of a mesh document, from its `meshesRoot`-relative path — what `readEditorFile`
 *  takes. */
export function meshFilePath(projectDir: string, relPath: string): string {
  return join(meshesRoot(projectDir), relPath);
}

/** The mesh's asset reference as a scene names it. The library root is the content root, so this is
 *  the listing path itself, normalized to `/`. This is the value written into `Mesh.shape.asset`. */
export function meshAssetRef(relPath: string): string {
  return join(relPath);
}

/** The mesh's display name — the basename with the format's double extension stripped. */
export function meshDisplayName(relPath: string): string {
  return displayName(relPath, MESH_EXT);
}
