// Place-by-drag: spawns a prefab file as a named instance at a world point and records the spawn as one
// undo group. Transport-free like `prefab-actions.ts` — the call is an injected `CallFn`.
//
// `prefab.spawn` returns one id per part and nothing else, so the group is built from a full component
// sweep after the spawn: a `spawn` entry per part, parents before children (undo runs the group in
// reverse, so children are destroyed first and a parent's cascade never leaves L0002 behind; redo runs
// it forward and `remapEntry` follows each fresh id through `Parent` and `PrefabInstance.root`).
//
// Known limit: a redone instance is plain respawned entities carrying `PrefabInstance`, without the
// engine library's placement record — the same as the unpack undo in `prefab-actions.ts`.

import type { Command, ResponseBody } from '@ludion/protocol';
import { componentNames as registryComponentNames, findComponents } from './entity-ops';
import type { CallFn, EntityId, UndoEntry } from './undo';

type Vec3 = readonly [number, number, number];

const PREFAB_SUFFIX = '.prefab.yaml';

function key(entity: EntityId): string {
  return `${entity.index}:${entity.generation}`;
}

function asEntityId(value: unknown): EntityId | null {
  if (typeof value !== 'object' || value === null) {
    return null;
  }
  const o = value as { index?: unknown; generation?: unknown };
  return typeof o.index === 'number' && typeof o.generation === 'number' ? { index: o.index, generation: o.generation } : null;
}

/** The file's basename minus `.prefab.yaml` — the instance's default name. */
export function instanceBaseName(relPath: string): string {
  const base = relPath.slice(relPath.lastIndexOf('/') + 1);
  return base.endsWith(PREFAB_SUFFIX) ? base.slice(0, -PREFAB_SUFFIX.length) : base;
}

/** `base`, or `base-2`, `base-3`… when a live `Name` equals it or starts with `<it>/` (a part of an
 *  instance of that name). */
export function freeInstanceName(base: string, liveNames: readonly string[]): string {
  const taken = (name: string): boolean => liveNames.some((live) => live === name || live.startsWith(`${name}/`));
  if (!taken(base)) {
    return base;
  }
  for (let n = 2; ; n += 1) {
    if (!taken(`${base}-${n}`)) {
      return `${base}-${n}`;
    }
  }
}

/** `prefab.spawn` for `relPath` (resolved against the content root) placed at `at`. */
export function spawnCommand(relPath: string, name: string, at: Vec3): Command {
  return {
    method: 'prefab.spawn',
    params: { path: relPath, name, transform: { translation: [...at], rotation: [0, 0, 0], scale: [1, 1, 1] } },
  };
}

/** One `spawn` entry per entity in `entities` that has a component map, parents first. A `Parent`
 *  pointing outside the set counts as a root; peers keep their input order. */
export function spawnEntries(byKey: Map<string, Record<string, unknown>>, entities: readonly EntityId[]): UndoEntry[] {
  const present = entities.filter((e) => byKey.has(key(e)));
  const inSet = new Set(present.map(key));
  const parentKey = (e: EntityId): string | null => {
    const parent = asEntityId(byKey.get(key(e))?.Parent);
    return parent !== null && inSet.has(key(parent)) ? key(parent) : null;
  };
  const placed = new Set<string>();
  const out: UndoEntry[] = [];
  let remaining = present;
  while (remaining.length > 0) {
    const ready = remaining.filter((e) => {
      const p = parentKey(e);
      return p === null || placed.has(p);
    });
    // A parent cycle cannot come off the engine; flush the rest rather than loop forever.
    const batch = ready.length > 0 ? ready : remaining;
    for (const e of batch) {
      placed.add(key(e));
      out.push({ kind: 'spawn', entity: e, components: byKey.get(key(e)) ?? {} });
    }
    const done = new Set(batch.map(key));
    remaining = remaining.filter((e) => !done.has(key(e)));
  }
  return out;
}

async function liveNames(call: CallFn): Promise<string[]> {
  const body = await call({ method: 'query.entities', params: { filter: ['Name'], include: ['Name'] } });
  const b = body as { kind?: unknown; entities?: unknown };
  if (b.kind !== 'query_result' || !Array.isArray(b.entities)) {
    return [];
  }
  return b.entities.flatMap((raw) => {
    const name = ((raw ?? {}) as { components?: { Name?: unknown } }).components?.Name;
    return typeof name === 'string' ? [name] : [];
  });
}

function spawnedEntities(body: ResponseBody): EntityId[] {
  const entities = (body as { entities?: unknown }).entities;
  return Array.isArray(entities) ? entities.flatMap((e) => asEntityId(e) ?? []) : [];
}

/**
 * Spawns `relPath` at `at` and returns the undo group for it plus the first part's id (for selection).
 * A rejected call rethrows with nothing recorded.
 */
export async function placePrefab(
  relPath: string,
  at: Vec3,
  call: CallFn,
  componentNames: (call: CallFn) => Promise<string[]> = registryComponentNames,
): Promise<{ entries: UndoEntry[]; first: EntityId | null }> {
  const name = freeInstanceName(instanceBaseName(relPath), await liveNames(call));
  const spawned = spawnedEntities(await call(spawnCommand(relPath, name, at)));
  if (spawned.length === 0) {
    return { entries: [], first: null };
  }
  const queried = await call({
    method: 'query.entities',
    params: { filter: [], include: await componentNames(call) },
  });
  const byKey = new Map<string, Record<string, unknown>>();
  for (const entity of spawned) {
    const components = findComponents(queried, entity);
    if (components !== null) {
      byKey.set(key(entity), components);
    }
  }
  return { entries: spawnEntries(byKey, spawned), first: spawned[0] ?? null };
}
