// A `kind: assembly` document read for VIEWING: the one palette, the placed parts and their
// hierarchy, the bound clips — plus the arithmetic that turns a part entry into something a camera
// or a crosshair can be aimed at. The `kind: part` reader lives here too, because a part carries no
// palette and is only meaningful through the assembly that places it.
//
// READ-ONLY, AND THAT IS THE WHOLE SURFACE. `doc.ts` keeps a source span per editable token because
// every voxel edit is a surgical splice; nothing here does, because nothing here writes. A pivot
// gizmo or a dope sheet is separate, triggered work, and the day it arrives it will want spans —
// carrying them now would be a writer's contract with no writer behind it.
//
// Like `doc.ts` this reader is total: it never throws and never validates. A file it cannot fully
// understand yields whatever parsed (a `null` size, a part with a defaulted `place`), and the
// engine's verdict arrives separately through `readout.ts` — one checker, the engine's.
//
// The format is `crates/ludion-scene/src/assembly.rs` (`ludion docs voxel-assemblies`): `format`,
// `kind`, `name`, `size`, `palette`, `parts`, `clips`, with `parts` in authored order — the order
// the engine's own `/parts/<i>` diagnostic pointers index.
//
// Pure: no React, no Tauri, no DOM.

import type { Vec3 } from '../gizmo/vecmath';
import {
  itemEntries,
  mapEntries,
  readPalette,
  readSlices,
  readVec3,
  seqItems,
  splitLines,
  type MapEntryRead,
  type PaletteEntry,
  type RawLine,
  type SliceLayer,
  type VoxelDoc,
} from './doc';

/** The grid origin — a part's `place` and `pivot` default, and what an unreadable one falls back
 *  to. The canonical writer omits a `[0, 0, 0]` pivot entirely: one spelling per meaning. */
const ORIGIN: Vec3 = [0, 0, 0];

/** One placed part. Field for field the engine's `AssemblyPartEntry`, with `mirror: x` decoded into
 *  the one boolean the format admits. */
export interface AssemblyPart {
  /** Unique within the assembly; what a clip's tracks address. */
  readonly name: string;
  /** The `kind: part` document's path AS AUTHORED — relative to the assembly file, not to the
   *  content root. `partFilePath` turns it into a path the host can read. */
  readonly file: string;
  /** Another part's name, or `null` for the one root. */
  readonly parent: string | null;
  /** Integer cell coordinates: where this part's own cell `(0, 0, 0)` lands in the assembled grid.
   *  `ORIGIN` when the key is missing or malformed — a part drawn in the wrong place is visible,
   *  where a dropped part would be silent. */
  readonly place: Vec3;
  /** Part-local, UNMIRRORED float voxel coordinates. A mirrored part quotes the same numbers as its
   *  twin; `pivotGlobal` carries them into the assembly's frame. */
  readonly pivot: Vec3;
  /** `mirror: x` — the only value the format admits. */
  readonly mirrorX: boolean;
}

/** One clip binding: the short name `Animation.clip` and `render --clip` take, and the clip
 *  document's path relative to the assembly file. */
export interface ClipBinding {
  readonly name: string;
  readonly file: string;
  /** A mapping-form entry's overlay `speed` (clip ticks per sim tick); a scalar entry leaves it unset. */
  readonly speed?: number;
}

/** Reads one `clips:` entry in either canonical spelling: a scalar (`idle: ../clips/idle.anim.yaml`)
 *  or a mapping (`clip:` plus an optional `speed:`). `speed` is kept only when finite and positive.
 *  Total: a malformed entry reads as an empty `file`. */
export function readClipBinding(raw: readonly RawLine[], clip: MapEntryRead): ClipBinding {
  if (clip.inline !== null) {
    return { name: clip.key, file: clip.inline.text };
  }
  const fields = mapEntries(raw, clip.body[0], clip.body[1]);
  const file = fields.find((field) => field.key === 'clip')?.inline?.text ?? '';
  const text = fields.find((field) => field.key === 'speed')?.inline?.text;
  const speed = text === undefined || text === '' ? Number.NaN : Number(text);
  return Number.isFinite(speed) && speed > 0
    ? { name: clip.key, file, speed }
    : { name: clip.key, file };
}

/** An assembly document read positionally. Every field is best-effort; see the module doc. */
export interface AssemblyDoc {
  readonly name: string | null;
  /** The assembled grid's extent in cells, or `null` when `size` is absent or not three numbers. */
  readonly size: Vec3 | null;
  /** The one palette, in AUTHORED order and in `doc.ts`'s shape — so `glyphViews` and the swatch
   *  strip colour a part's cells through it unchanged. */
  readonly palette: readonly PaletteEntry[];
  /** Parts in authored order. */
  readonly parts: readonly AssemblyPart[];
  /** Clip bindings in authored order. */
  readonly clips: readonly ClipBinding[];
}

/** A `kind: part` document: a lean grid with no palette of its own. */
export interface PartDoc {
  readonly name: string | null;
  readonly size: Vec3 | null;
  /** `slices[y][z]` bottom-up — exactly `VoxelDoc.layers`. */
  readonly layers: readonly SliceLayer[];
}

/** Reads a `kind: assembly` document. Total: never throws, never validates. */
export function parseAssemblyDoc(source: string): AssemblyDoc {
  const raw = splitLines(source);
  let name: string | null = null;
  let size: Vec3 | null = null;
  let palette: readonly PaletteEntry[] = [];
  let parts: readonly AssemblyPart[] = [];
  let clips: readonly ClipBinding[] = [];

  for (const entry of mapEntries(raw, 0, raw.length)) {
    switch (entry.key) {
      case 'name':
        name = entry.inline === null ? null : entry.inline.text;
        break;
      case 'size':
        size = readVec3(raw, entry);
        break;
      case 'palette':
        palette = readPalette(raw, entry);
        break;
      case 'parts':
        parts = readParts(raw, entry);
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
  return { name, size, palette, parts, clips };
}

/** Reads a `kind: part` document. Total: never throws, never validates. */
export function parsePartDoc(source: string): PartDoc {
  const raw = splitLines(source);
  let name: string | null = null;
  let size: Vec3 | null = null;
  let layers: readonly SliceLayer[] = [];

  for (const entry of mapEntries(raw, 0, raw.length)) {
    switch (entry.key) {
      case 'name':
        name = entry.inline === null ? null : entry.inline.text;
        break;
      case 'size':
        size = readVec3(raw, entry);
        break;
      case 'slices':
        layers = readSlices(raw, entry);
        break;
      default:
        break;
    }
  }
  return { name, size, layers };
}

/** A part's grid as the slice painter's own input: the part's layers under the assembly's palette,
 *  which is the only palette its glyphs ever resolve against. `scale` is `null` because a part
 *  document declares none, and `source` is empty because nothing here may be spliced. */
export function partAsVoxelDoc(part: PartDoc, palette: readonly PaletteEntry[]): VoxelDoc {
  return { source: '', name: part.name, size: part.size, scale: null, palette, layers: part.layers };
}

function readParts(raw: readonly RawLine[], entry: MapEntryRead): AssemblyPart[] {
  const out: AssemblyPart[] = [];
  for (const item of seqItems(raw, entry.body[0], entry.body[1])) {
    let name = '';
    let file = '';
    let parent: string | null = null;
    let place: Vec3 | null = null;
    let pivot: Vec3 | null = null;
    let mirrorX = false;
    for (const field of itemEntries(raw, item)) {
      switch (field.key) {
        case 'name':
          name = field.inline?.text ?? '';
          break;
        case 'file':
          file = field.inline?.text ?? '';
          break;
        case 'parent':
          parent = field.inline?.text ?? null;
          break;
        case 'place':
          place = readVec3(raw, field);
          break;
        case 'pivot':
          pivot = readVec3(raw, field);
          break;
        case 'mirror':
          mirrorX = field.inline?.text === 'x';
          break;
        default:
          break;
      }
    }
    out.push({ name, file, parent, place: place ?? ORIGIN, pivot: pivot ?? ORIGIN, mirrorX });
  }
  return out;
}

// --- The part hierarchy ------------------------------------------------------------------------ //

/** One node of the part tree. `index` is the part's position in `AssemblyDoc.parts` — the index an
 *  engine diagnostic's `/parts/<i>` pointer names. */
export interface PartNode {
  readonly part: AssemblyPart;
  readonly index: number;
  /** 0 for a root, one more than its parent's otherwise. */
  readonly depth: number;
  readonly children: readonly PartNode[];
}

/**
 * The parent hierarchy: roots first, every list in authored order.
 *
 * The engine refuses anything but one root and one acyclic tree (`L1138`), so this reader's whole
 * job on the way there is to keep drawing something while the file is mid-edit. A part whose
 * `parent` names nothing, names itself, or sits on a cycle DEGRADES TO A ROOT — a panel that hangs
 * on a half-typed parent name is worse than one briefly showing a limb at the top level, and the
 * engine's verdict says what is really wrong.
 *
 * A duplicated part name resolves to the FIRST entry carrying it, matching `paletteEntry`'s read of
 * a duplicated glyph.
 */
export function partTree(parts: readonly AssemblyPart[]): readonly PartNode[] {
  const parent = resolveParents(parts);
  const depth = parts.map((_, index) => depthOf(parent, index));
  const children: PartNode[][] = parts.map(() => []);
  const roots: PartNode[] = [];

  // Shallowest first, authored order within a depth: a child authored above its parent still lands
  // in a children list that already exists, without a second pass to stitch it in.
  const order = parts.map((_, index) => index).sort((a, b) => depth[a] - depth[b] || a - b);
  for (const index of order) {
    const node: PartNode = {
      part: parts[index],
      index,
      depth: depth[index],
      children: children[index],
    };
    (parent[index] < 0 ? roots : children[parent[index]]).push(node);
  }
  return roots;
}

/** Each part's parent index, or `-1` for a root — unknown, self- and cycle-parented entries
 *  included. */
function resolveParents(parts: readonly AssemblyPart[]): number[] {
  const byName = new Map<string, number>();
  for (const [index, part] of parts.entries()) {
    if (!byName.has(part.name)) {
      byName.set(part.name, index);
    }
  }
  const direct = parts.map((part) => (part.parent === null ? -1 : (byName.get(part.parent) ?? -1)));
  return direct.map((link, index) => (onCycle(direct, link, index) ? -1 : link));
}

/** Whether walking up from `start` ever reaches `index` (a self-link or a cycle through it) or runs
 *  past the part count (a cycle above it). Either way `index` cannot be given a parent. */
function onCycle(direct: readonly number[], start: number, index: number): boolean {
  let at = start;
  for (let steps = 0; at >= 0; steps += 1) {
    if (at === index || steps > direct.length) {
      return true;
    }
    at = direct[at];
  }
  return false;
}

function depthOf(parent: readonly number[], index: number): number {
  let depth = 0;
  let at = parent[index];
  while (at >= 0 && depth <= parent.length) {
    depth += 1;
    at = parent[at];
  }
  return depth;
}

// --- Paths ------------------------------------------------------------------------------------- //

/**
 * A part file's path, relative to the same root `assemblyRelPath` is relative to.
 *
 * Part and clip paths resolve against THE ASSEMBLY FILE'S OWN DIRECTORY (`run_assembly` roots the
 * load at the document's `path.parent()`), which is what lets a model move as a directory. So
 * `voxels/player.assembly.yaml` + `parts/x.part.yaml` is `voxels/parts/x.part.yaml` — a
 * content-root-relative path exactly as the assembly reference itself is.
 *
 * `.` segments drop and `..` pops; a `..` that would climb above the root clamps there, since a path
 * outside the content root is not one any client could load anyway.
 */
export function partFilePath(assemblyRelPath: string, file: string): string {
  const out: string[] = [];
  for (const segment of [...segments(assemblyRelPath).slice(0, -1), ...segments(file)]) {
    if (segment === '.') {
      continue;
    }
    if (segment === '..') {
      out.pop();
      continue;
    }
    out.push(segment);
  }
  return out.join('/');
}

/** A clip file's path. The engine resolves a binding exactly as it resolves a part — same directory,
 *  same rule — so this is that one function under the name a call site reads better with. */
export const clipFilePath = partFilePath;

function segments(path: string): string[] {
  return path.split(/[/\\]+/).filter((segment) => segment !== '');
}

// --- Boxes and pivots -------------------------------------------------------------------------- //

/** An axis-aligned box, in whichever space the producing function names. */
export interface Box3 {
  readonly min: Vec3;
  readonly max: Vec3;
}

/** A part's rest-pose box in ASSEMBLY-GRID CELLS: `place` to `place + partSize`. Mirroring reverses
 *  rows inside the same extent, so a `mirror: x` part occupies exactly this box too. */
export function partRestBox(part: AssemblyPart, partSize: Vec3): Box3 {
  return {
    min: part.place,
    max: [part.place[0] + partSize[0], part.place[1] + partSize[1], part.place[2] + partSize[2]],
  };
}

/**
 * A cell box in the staged subject's model space.
 *
 * Mirrors the mesher's grid→model convention, `crates/ludion-engine/src/voxel_mesh.rs` — and
 * `assembly_mesh.rs`, which applies that same convention at the ASSEMBLY's extent, the frame the
 * entity's `Transform` acts in. One voxel is one unit and the origin is BOTTOM-CENTRE: cell
 * `(x, y, z)` of a grid `[SX, SY, SZ]` spans `[x − SX/2, x+1 − SX/2] × [y, y+1] × [z − SZ/2,
 * z+1 − SZ/2]`. So X and Z are centred on the assembly, Y sits on the ground, and authored slice row
 * `z = 0` is the most negative Z — the model's own forward face.
 *
 * `scale` is the entity's uniform `Transform.scale` (vigil's player stands at `0.125`).
 */
export function partWorldBox(box: Box3, assemblySize: Vec3, scale: number): Box3 {
  const halfX = assemblySize[0] * 0.5;
  const halfZ = assemblySize[2] * 0.5;
  const at = (cell: Vec3): Vec3 => [
    (cell[0] - halfX) * scale,
    cell[1] * scale,
    (cell[2] - halfZ) * scale,
  ];
  return { min: at(box.min), max: at(box.max) };
}

/**
 * A part's pivot in assembly-grid float coordinates — `place + pivot`, with a `mirror: x` part's
 * pivot carried into the mirrored frame first.
 *
 * That mirroring is the trap this function exists to close. The authored pivot is part-local and
 * UNMIRRORED, so both twins quote the same numbers, while the composed grid holds mirrored cells —
 * and the two have to be measured in one frame. `assembly_part_voxel` reverses each row, so a
 * continuous coordinate `px` becomes `partSize.x − px`: `AssemblyPartEntry::pivot_in_mesh_frame` and
 * `pivot_in_assembly` in `crates/ludion-scene/src/assembly.rs`, which this mirrors. Skipping it puts
 * a crosshair a whole limb width off on every `-r` part of a humanoid.
 */
export function pivotGlobal(part: AssemblyPart, partSize: Vec3): Vec3 {
  const x = part.mirrorX ? partSize[0] - part.pivot[0] : part.pivot[0];
  return [part.place[0] + x, part.place[1] + part.pivot[1], part.place[2] + part.pivot[2]];
}
