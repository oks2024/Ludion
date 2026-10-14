// Entity-palette operations: spawn an archetype, duplicate the selection, destroy an entity and its
// parenting. All three are *transport-free* — they take an injected `CallFn`, so the
// whole flow is unit-testable against object literals with no socket (see entity-ops.test.ts), exactly
// like the undo executors and the picking geometry.
//
// Each op returns a plain {@link EntityOpResult}: the successful `{command, body}` records IN APPLIED
// ORDER (so the caller can feed them to `mutationToEntry` and record undo), the fresh entity id when a
// spawn happened, and the error when the flow stopped early. On a mid-flow failure the records already
// applied are still returned alongside the error — the caller pushes them as one undoable group so a
// partial cascade stays reversible (never-wedge > all-or-nothing, matching the undo executor's stance).
//
// The generated response bodies carry `[k: string]: unknown` index signatures that defeat TS's
// discriminant narrowing; the same cast-and-read pattern used across the client (protocol/client.ts,
// editor/undo.ts, viewer/picking.ts) applies here.

import type { Command, ResponseBody } from '@ludion/protocol';
import type { MutationRecord } from './mutations';
import type { CallFn, EntityId } from './undo';

/**
 * The outcome of one entity op.
 *
 * - `records` — every successful mutation (`{command, body}`), in the order it was applied. A single
 *   `component.set`/`entity.spawn` for spawn/duplicate; a cascade of `component.remove`s then an
 *   `entity.destroy` for destroy. Empty when the very first call failed.
 * - `newEntity` — the fresh id a spawn/duplicate handed back (absent for destroy or on failure).
 * - `error` — set when the flow stopped early; `records` still holds whatever landed before it.
 */
export interface EntityOpResult {
  readonly records: MutationRecord[];
  readonly newEntity?: EntityId;
  readonly error?: unknown;
}

/** A stable `index:generation` key for an entity handle — the same rule the inspector's list uses. */
function entityKey(entity: EntityId): string {
  return `${entity.index}:${entity.generation}`;
}

/** Reads a wire entity handle `{index, generation}` out of an unknown value, or `null`. */
function asEntityId(value: unknown): EntityId | null {
  if (typeof value !== 'object' || value === null) {
    return null;
  }
  const o = value as { index?: unknown; generation?: unknown };
  if (typeof o.index !== 'number' || typeof o.generation !== 'number') {
    return null;
  }
  return { index: o.index, generation: o.generation };
}

/** Reads the fresh id off a `spawned` response body, or `null` for any other shape. */
function spawnedEntity(body: ResponseBody): EntityId | null {
  const b = body as { kind?: unknown; entity?: unknown };
  return b.kind === 'spawned' ? asEntityId(b.entity) : null;
}

/** The registry's component names (`engine.describe` → `description.components` keys), or `[]`. This
 *  is the include-list a full-component sweep needs, since there's no include-everything wildcard. */
export async function componentNames(call: CallFn): Promise<string[]> {
  const body = await call({ method: 'engine.describe' });
  const b = body as { kind?: unknown; description?: unknown };
  if (b.kind !== 'engine_description' || typeof b.description !== 'object' || b.description === null) {
    return [];
  }
  const components = (b.description as { components?: unknown }).components;
  return typeof components === 'object' && components !== null ? Object.keys(components) : [];
}

/** Finds `target`'s component map in a `query_result` body, or `null` if it isn't present. */
export function findComponents(body: ResponseBody, target: EntityId): Record<string, unknown> | null {
  const b = body as { kind?: unknown; entities?: unknown };
  if (b.kind !== 'query_result' || !Array.isArray(b.entities)) {
    return null;
  }
  const key = entityKey(target);
  for (const raw of b.entities) {
    if (typeof raw !== 'object' || raw === null) {
      continue;
    }
    const match = raw as { entity?: unknown; components?: unknown };
    const id = asEntityId(match.entity);
    if (id !== null && entityKey(id) === key) {
      return typeof match.components === 'object' && match.components !== null
        ? (match.components as Record<string, unknown>)
        : {};
    }
  }
  return null;
}

/**
 * Spawns a prebuilt `entity.spawn` command (a palette archetype) and reports the record + fresh id.
 * Any rejection is captured into `error` so the UI surfaces it through the shared banner rather than
 * an unhandled promise.
 */
export async function spawnArchetype(command: Command, call: CallFn): Promise<EntityOpResult> {
  try {
    const body = await call(command);
    const newEntity = spawnedEntity(body);
    if (newEntity === null) {
      return { records: [], error: new Error('entity.spawn did not return a new entity') };
    }
    return { records: [{ command, body }], newEntity };
  } catch (error) {
    return { records: [], error };
  }
}

/**
 * Duplicates the entity `selection`: reads its full component set (one `query.entities` for its id including
 * every registry component), then `entity.spawn`s an exact copy. `Name` rides inside the components
 * map (never the `name` param — passing both is rejected, L3014) and `Parent` is copied as-is, so the
 * copy is a sibling of the original. Returns the spawn record + fresh id, or a structured failure.
 */
export async function duplicateEntity(selection: EntityId, call: CallFn): Promise<EntityOpResult> {
  try {
    const names = await componentNames(call);
    const queried = await call({ method: 'query.entities', params: { filter: [], include: names, entities: [selection] } });
    const components = findComponents(queried, selection);
    if (components === null) {
      return { records: [], error: new Error(`entity ${entityKey(selection)} not found`) };
    }
    const command: Command = { method: 'entity.spawn', params: { components } };
    return await spawnArchetype(command, call);
  } catch (error) {
    return { records: [], error };
  }
}

/**
 * Destroys `victim`. `entity.destroy` cascades to the whole subtree, and its response carries the
 * destroyed descendants, so one command is the whole operation; the caller derives the undo group
 * from that single record (`mutationToEntries`). A rejection is returned as `error` with no records.
 */
export async function destroyEntity(victim: EntityId, call: CallFn): Promise<EntityOpResult> {
  const command: Command = { method: 'entity.destroy', params: { entity: victim } };
  try {
    const body = await call(command);
    return { records: [{ command, body }] };
  } catch (error) {
    return { records: [], error };
  }
}
