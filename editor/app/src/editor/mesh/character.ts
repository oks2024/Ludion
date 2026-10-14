// The Characters library's paths and its document reader, as pure data: where a `kind: character`
// document lives, how a scene names one, and which clips it binds.
//
// THE SAME CONTENT ROOT AS THE MESHES. A character's `Mesh.shape` is `{ character: <ref> }`, resolved
// against the stage session's content root exactly like a mesh asset, so its reference is its listing
// path verbatim and the library lists `meshesRoot` (`./stage.ts`). Its clip files resolve relative to
// the character file — `clipFilePath` (`../voxel/assembly.ts`) — not to the root.
//
// Total, like every reader in these folders: `parseCharacterDoc` never throws and never validates;
// `check-scene` is the one checker.
//
// Pure: no React, no Tauri, no DOM.

import type { ClipBinding } from '../voxel/assembly';
import { mapEntries, splitLines } from '../voxel/doc';
import { join } from '../voxel/stage';
import { displayName } from '../voxel/tree';

/** The character format's double extension — what the asset browser lists and nothing else. */
export const CHARACTER_EXT = '.character.yaml';

/** The character's asset reference as a scene names it: the listing path itself, normalized to `/`.
 *  This is the value written into `Mesh.shape.character`. */
export function characterAssetRef(relPath: string): string {
  return join(relPath);
}

/** The character's display name — the basename with the format's double extension stripped. */
export function characterDisplayName(relPath: string): string {
  return displayName(relPath, CHARACTER_EXT);
}

/** A `kind: character` document read positionally. */
export interface CharacterDoc {
  readonly name: string | null;
  /** The skinned mesh's path as authored, relative to the character file. */
  readonly mesh: string | null;
  /** Clip bindings in authored order; `speed` is the per-clip overlay when finite and positive. */
  readonly clips: readonly ClipBinding[];
}

/** Reads a `kind: character` document. Total: never throws, never validates. */
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
        clips = mapEntries(raw, entry.body[0], entry.body[1]).map((clip) => {
          const fields = mapEntries(raw, clip.body[0], clip.body[1]);
          const file = fields.find((field) => field.key === 'clip')?.inline?.text ?? '';
          const text = fields.find((field) => field.key === 'speed')?.inline?.text;
          const speed = text === undefined || text === '' ? Number.NaN : Number(text);
          return Number.isFinite(speed) && speed > 0
            ? { name: clip.key, file, speed }
            : { name: clip.key, file };
        });
        break;
      default:
        break;
    }
  }
  return { name, mesh, clips };
}
