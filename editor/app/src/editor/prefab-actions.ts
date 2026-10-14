// Prefab-instance actions for the hierarchy panel: `prefab.apply` / `prefab.revert` / `prefab.unpack`
// and the undo groups they leave behind. Transport-free like the other `editor/*` modules — the call
// is an injected `CallFn` (see prefab-actions.test.ts).
//
// Neither revert nor unpack returns the prior values, so the action captures the instance's components
// first and expresses what changed as plain `set`/`remove` entries — no new `UndoEntry` kind, every
// inverse a public command. Redo replays the captured `after` values (the same world state the revert
// produced), it is not re-sent as `prefab.revert`. Apply writes a file, so it records nothing.

import type { Command, ResponseBody } from '@ludion/protocol';
import { entityRef } from '../protocol/commands';
import { componentNames as registryComponentNames } from './entity-ops';
import type { PrefabLink } from './hierarchy';
import type { CallFn, EntityId, UndoEntry } from './undo';

export type InstanceOp = 'apply' | 'revert' | 'unpack';

/** Per-entity component maps, keyed by `index:generation`. */
export type ComponentSnapshot = Map<string, Record<string, unknown>>;

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

/** The command for `op` on `instance` (any part of it); `pointer` narrows a revert. */
export function instanceCommand(op: InstanceOp, instance: EntityId, pointer?: string): Command {
  const ref = entityRef(instance);
  switch (op) {
    case 'apply':
      return { method: 'prefab.apply', params: { instance: ref } };
    case 'revert':
      return { method: 'prefab.revert', params: pointer === undefined ? { instance: ref } : { instance: ref, pointer } };
    case 'unpack':
      return { method: 'prefab.unpack', params: { instance: ref } };
  }
}

/** The revert pointer that restores just `link`'s part (`/<part path>`). */
export function revertPartPointer(link: PrefabLink): string {
  return `/${link.part}`;
}

/** Diffs the captured `before` against `after` for each of `entities`, one entry per changed component
 *  (`PrefabInstance` is never compared): changed → `set`, a dropped gained component → `remove`, a
 *  restored one → a fresh-insert `set`. An entity missing from either snapshot is skipped. */
export function revertEntries(before: ComponentSnapshot, after: ComponentSnapshot, entities: readonly EntityId[]): UndoEntry[] {
  const entries: UndoEntry[] = [];
  for (const entity of entities) {
    const was = before.get(key(entity));
    const now = after.get(key(entity));
    if (was === undefined || now === undefined) {
      continue;
    }
    for (const name of new Set([...Object.keys(was), ...Object.keys(now)])) {
      if (name === 'PrefabInstance') {
        continue;
      }
      const b = was[name];
      const a = now[name];
      if (b === undefined && a === undefined) {
        continue;
      }
      if (b === undefined) {
        entries.push({ kind: 'set', entity, component: name, before: null, after: a });
      } else if (a === undefined) {
        entries.push({ kind: 'remove', entity, component: name, before: b });
      } else if (JSON.stringify(b) !== JSON.stringify(a)) {
        entries.push({ kind: 'set', entity, component: name, before: b, after: a });
      }
    }
  }
  return entries;
}

/** Unpack removes `PrefabInstance` from each part: undo re-sets the captured value, redo removes it. */
export function unpackEntries(before: ComponentSnapshot, entities: readonly EntityId[]): UndoEntry[] {
  const entries: UndoEntry[] = [];
  for (const entity of entities) {
    const link = before.get(key(entity))?.PrefabInstance;
    if (link !== undefined) {
      entries.push({ kind: 'remove', entity, component: 'PrefabInstance', before: link });
    }
  }
  return entries;
}

/** Every entity of `instance`'s prefab instance (those whose `PrefabInstance.root` matches), with the
 *  components named in `names`. The instance is any part; none found → an empty snapshot. */
async function sweep(call: CallFn, names: readonly string[], instance: EntityId): Promise<ComponentSnapshot> {
  const include = names.includes('PrefabInstance') ? [...names] : [...names, 'PrefabInstance'];
  const body = await call({ method: 'query.entities', params: { filter: [], include } });
  const b = body as { kind?: unknown; entities?: unknown };
  const all = new Map<string, Record<string, unknown>>();
  if (b.kind === 'query_result' && Array.isArray(b.entities)) {
    for (const raw of b.entities) {
      const match = (raw ?? {}) as { entity?: unknown; components?: unknown };
      const entity = asEntityId(match.entity);
      if (entity !== null && typeof match.components === 'object' && match.components !== null) {
        all.set(key(entity), match.components as Record<string, unknown>);
      }
    }
  }
  const rootKeyOf = (components: Record<string, unknown> | undefined): string | null => {
    const root = asEntityId((components?.PrefabInstance as { root?: unknown } | undefined)?.root);
    return root === null ? null : key(root);
  };
  const rootKey = rootKeyOf(all.get(key(instance)));
  const snapshot: ComponentSnapshot = new Map();
  if (rootKey === null) {
    return snapshot;
  }
  for (const [k, components] of all) {
    if (rootKeyOf(components) === rootKey) {
      snapshot.set(k, components);
    }
  }
  return snapshot;
}

function responseEntities(body: ResponseBody): EntityId[] {
  const entities = (body as { entities?: unknown }).entities;
  return Array.isArray(entities) ? entities.flatMap((e) => asEntityId(e) ?? []) : [];
}

/**
 * Runs `op` on `instance` and returns the undo entries it earned alongside the response. Unpack and
 * revert sweep the instance's components first (revert sweeps again after); apply just calls and
 * returns no entries. A rejected call rethrows, with nothing recorded.
 */
export async function runInstanceAction(
  op: InstanceOp,
  instance: EntityId,
  call: CallFn,
  pointer?: string,
  componentNames: (call: CallFn) => Promise<string[]> = registryComponentNames,
): Promise<{ entries: UndoEntry[]; body: ResponseBody }> {
  if (op === 'apply') {
    return { entries: [], body: await call(instanceCommand(op, instance, pointer)) };
  }
  const names = await componentNames(call);
  const before = await sweep(call, names, instance);
  const body = await call(instanceCommand(op, instance, pointer));
  const entities = responseEntities(body);
  if (op === 'unpack') {
    return { entries: unpackEntries(before, entities), body };
  }
  const after = await sweep(call, names, instance);
  return { entries: revertEntries(before, after, entities), body };
}
