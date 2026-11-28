// The Characters library's paths and its document reader, as pure data: where a skinned `kind: assembly`
// (`mesh:`) document lives, how a scene names one, and which clips it binds.
//
// THE SAME CONTENT ROOT AS THE MESHES. A character's `Mesh.shape` is `{ assembly: <ref> }`, resolved
// against the stage session's content root exactly like a mesh asset, so its reference is its listing
// path verbatim and the library lists `meshesRoot` (`./stage.ts`). Its clip files resolve relative to
// the character file — `clipFilePath` (`../voxel/assembly.ts`) — not to the root.
//
// Total, like every reader in these folders: `parseCharacterDoc` never throws and never validates;
// `check-scene` is the one checker.
//
// Pure: no React, no Tauri, no DOM.

import { readClipBinding, type ClipBinding } from '../voxel/assembly';
import { mapEntries, splitLines } from '../voxel/doc';
import { ASSEMBLY_EXT, join } from '../voxel/stage';
import { displayName } from '../voxel/tree';

/** The character's asset reference as a scene names it: the listing path itself, normalized to `/`.
 *  This is the value written into `Mesh.shape.assembly`. */
export function characterAssetRef(relPath: string): string {
  return join(relPath);
}

/** The character's display name — the basename with the format's double extension stripped. */
export function characterDisplayName(relPath: string): string {
  return displayName(relPath, ASSEMBLY_EXT);
}

/** Whether the document's body is a skinned mesh (a non-empty `mesh:`) rather than voxel parts. */
export function isSkinned(doc: CharacterDoc): boolean {
  return doc.mesh !== null && doc.mesh !== '';
}

/** A skinned `kind: assembly` (`mesh:`) document read positionally. */
export interface CharacterDoc {
  readonly name: string | null;
  /** The skinned mesh's path as authored, relative to the character file. */
  readonly mesh: string | null;
  /** Clip bindings in authored order; `speed` is the per-clip overlay when finite and positive. */
  readonly clips: readonly ClipBinding[];
}

/** Reads a skinned `kind: assembly` (`mesh:`) document. Total: never throws, never validates. */
export function parseCharacterDoc(source: string): CharacterDoc {
  const raw = splitLines(source);
  let name: string | null = null;
  let mesh: string | null = null;
  let clips: readonly ClipBinding[] = [];

  for (const entry of mapEntries(raw, 0, raw.length)) {
    switch (entry.key) {
      case 'name':
        name = entry.inline === null ? null : entry.inline.text;
        break;
      case 'mesh':
        mesh = entry.inline === null ? null : entry.inline.text;
        break;
      case 'clips':
        clips = mapEntries(raw, entry.body[0], entry.body[1]).map((clip) =>
          readClipBinding(raw, clip),
        );
        break;
      default:
        break;
    }
  }
  return { name, mesh, clips };
}
