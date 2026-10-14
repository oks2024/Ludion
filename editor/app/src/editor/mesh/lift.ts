// Lifting a mesh's authored placement out of the scene that references it.
//
// WHY A LIFT AND NOT JUST A SHAPE. `ludion import` writes a mesh document *and* the scene node that
// places it, and the material lives on the node: the generated idol's `scene.yaml` authors
// `metallic: 1.0` beside `shape: { asset: meshes/mesh-0.mesh.yaml }`. A stage that wrote the shape
// alone would light the asset with the component's defaults and read wrong, so the selection carries
// the whole `Mesh` component — and the node's `Transform.scale`, since the stage's subject stands at
// the voxel library's placeholder scale.
//
// WHICH SCENE. Nearest ancestor directory first, never the library root: an importer drops its scene
// beside the meshes it wrote, while the root holds every district scene in the project and reading
// those to answer "who places this mesh?" would cost megabytes per click.
//
// PREFABS ARE PLACEMENTS TOO. `ludion import` writes `model.prefab.yaml`, whose parts are authored the
// way a scene's entities are, so the same lift reads it. Instances are never expanded: every mesh that
// reaches a scene through a prefab instance is placed directly by some prefab's own part, and expanding
// would mean spawning the prefab into the stage's world — and an instance's override is not "the
// authored placement" anyway.
//
// Pure: no React, no Tauri, no DOM. Text arrives as an argument; the caller does the reading.

import { parse } from 'yaml';
import type { DirEntry } from '../../shell/host';
import { ancestorDirs } from '../voxel/tree';

/** The double extensions that mark a `.yaml` as a document kind rather than a scene (`ludion docs
 *  data-docs`).
 *
 *  THE LIST CANNOT BE COMPLETE, BY CONSTRUCTION: several kinds carry no suffix at all and are plain
 *  `.yaml` distinguished only by a `kind:` key — `kind: pattern` under `scenes/patterns/`,
 *  `kind: enemy` under `scenes/enemies/`. Those are read like a scene, which is why this list is an
 *  optimization and never the correctness argument: a document that is not a scene costs one wasted
 *  read and nothing else, since the match is on the resolved asset path and only a scene has
 *  `entities`. */
const DOCUMENT_SUFFIXES: readonly string[] = [
  '.anim.yaml',
  '.assembly.yaml',
  '.atlas-source.yaml',
  '.atlas.yaml',
  '.gen.yaml',
  '.kit.yaml',
  '.map.yaml',
  '.mesh-edit.yaml',
  '.mesh.yaml',
  '.ops.yaml',
  '.origin.yaml',
  '.palette.yaml',
  '.part.yaml',
  '.sculpt.yaml',
  '.sprite.yaml',
  '.voxel.yaml',
];

/** Whether a listed file could place a mesh: a scene or a `.prefab.yaml` (a `.yaml` carrying no other
 *  document-kind suffix). */
export function isPlacementCandidate(name: string): boolean {
  const lower = name.toLowerCase();
  return lower.endsWith('.yaml') && !DOCUMENT_SUFFIXES.some((suffix) => lower.endsWith(suffix));
}

/**
 * The scenes and prefabs that could place `meshRelPath`, nearest ancestor directory first and in listing order
 * within each — the order the caller reads them in, stopping at the first match.
 *
 * The library root is deliberately excluded (see the module doc). Within one directory the order
 * does not decide anything: two scenes in the same folder that place the same mesh differ in
 * material, and the first one listed is as good an answer as the human's click can ask for.
 */
export function candidatePlacements(meshRelPath: string, entries: readonly DirEntry[]): string[] {
  const nearestFirst = ancestorDirs(meshRelPath).reverse();
  const byDir = new Map<string, string[]>();
  for (const entry of entries) {
    if (entry.isDir || !isPlacementCandidate(entry.name)) {
      continue;
    }
    const cut = entry.relPath.lastIndexOf('/');
    if (cut < 0) {
      continue;
    }
    const dir = entry.relPath.slice(0, cut);
    const bucket = byDir.get(dir);
    if (bucket === undefined) {
      byDir.set(dir, [entry.relPath]);
    } else {
      bucket.push(entry.relPath);
    }
  }
  return nearestFirst.flatMap((dir) => byDir.get(dir) ?? []);
}

/** A mesh's authored placement, as one scene node writes it. */
export interface MeshLift {
  /** The node's whole `Mesh` component. Its `shape` is still named relative to the scene's own
   *  directory, so the caller replaces it with a content-root reference before writing. */
  readonly mesh: Record<string, unknown>;
  /** The node's `Transform.scale`, or `[1, 1, 1]` when it authors none. */
  readonly scale: readonly [number, number, number];
  /** How many entities in that scene carry a `Mesh` — a multi-node import places several, and only
   *  the first is put on the stage. */
  readonly meshNodes: number;
}

const UNIT_SCALE: readonly [number, number, number] = [1, 1, 1];

/**
 * The first node of `sceneText` whose `Mesh.shape.asset` resolves to `meshRelPath`, or `null` when
 * the scene places some other mesh (or is not a scene at all).
 *
 * Both paths are library-root-relative: the scene's reference is resolved against the scene file's
 * own directory, which is what the engine does with it.
 *
 * A scene whose YAML does not parse yields `null` rather than throwing — a broken district scene two
 * folders up is not this selection's error to report, and the caller's "no scene or prefab places this mesh"
 * answer is the honest one either way.
 */
export function liftMesh(sceneText: string, sceneRelPath: string, meshRelPath: string): MeshLift | null {
  let parsed: unknown;
  try {
    parsed = parse(sceneText);
  } catch {
    return null;
  }
  const entities = record(parsed)?.entities;
  if (!Array.isArray(entities)) {
    return null;
  }
  const sceneDir = dirOf(sceneRelPath);
  let node: Omit<MeshLift, 'meshNodes'> | null = null;
  let meshNodes = 0;
  for (const entity of entities) {
    const components = record(record(entity)?.components);
    const mesh = record(components?.Mesh);
    if (mesh === null) {
      continue;
    }
    meshNodes += 1;
    const asset = record(mesh.shape)?.asset;
    if (node !== null || typeof asset !== 'string' || resolveFrom(sceneDir, asset) !== meshRelPath) {
      continue;
    }
    node = { mesh, scale: scaleOf(record(components?.Transform)) };
  }
  return node === null ? null : { ...node, meshNodes };
}

/** `value` as a plain object, or `null` — arrays included, since neither is a component map. */
function record(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

/** A `Transform`'s scale as three numbers, or the unit scale when it authors none. */
function scaleOf(transform: Record<string, unknown> | null): readonly [number, number, number] {
  const scale = transform?.scale;
  if (!Array.isArray(scale) || scale.length !== 3 || scale.some((n) => typeof n !== 'number')) {
    return UNIT_SCALE;
  }
  return [scale[0] as number, scale[1] as number, scale[2] as number];
}

/** The directory part of a `/`-separated relative path, `''` at the root. */
function dirOf(relPath: string): string {
  const cut = relPath.lastIndexOf('/');
  return cut < 0 ? '' : relPath.slice(0, cut);
}

/** `ref` resolved against `dir`, both `/`-separated and relative, with `.`/`..` collapsed. */
function resolveFrom(dir: string, ref: string): string {
  const out: string[] = [];
  for (const segment of `${dir}/${ref}`.split('/')) {
    if (segment === '' || segment === '.') {
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
