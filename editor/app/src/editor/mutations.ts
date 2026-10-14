// Turns a completed mutation (the command that was sent + the body it resolved with) into the
// {@link UndoEntry} that inverts it. Pure and transport-free, so the mapping is unit-testable without
// a socket (see mutations.test.ts) and can be reused by any client that records history — Wave 2's
// inspector edits and Wave 4's entity palette both funnel through here.
//
// The inverse data rides the mutation's own response (Plan/phase-6-editing.md decision #2), so this
// reads it back off the body: `component.set` → `{previous}` (absent = the set was a fresh insert),
// `component.remove` → `{previous}`, `entity.destroy` → `{components}` (full state at destruction),
// `entity.spawn` → `{entity}` (the fresh id the ECS handed back). Anything it doesn't recognize — an
// unknown method, a body whose `kind` doesn't match the command, a malformed entity handle — yields
// `null`, and the caller simply records no undo entry for it.
//
// The generated protocol types carry `[k: string]: unknown` index signatures on their union arms,
// which defeats TS's discriminant narrowing; the same cast-and-read pattern used across the client
// (protocol/client.ts, editor/undo.ts, viewer/picking.ts) applies here.

import type { Command, ResponseBody } from '@ludion/protocol';
import type { EntityId, UndoEntry } from './undo';

/** A completed mutation: the command sent and the body it resolved with (`useComponentCommit`'s
 *  `MutationReport`). */
export interface MutationRecord {
  readonly command: Command;
  readonly body: ResponseBody;
}

/** Reads a wire entity handle out of an unknown value, or `null` if it isn't one. */
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

/** Reads a plain object out of an unknown value, or `null` if it isn't one. */
function asRecord(value: unknown): Record<string, unknown> | null {
  if (typeof value !== 'object' || value === null) {
    return null;
  }
  return value as Record<string, unknown>;
}

/**
 * Every {@link UndoEntry} that inverts `record`, in applied order. Only `entity.destroy` can yield
 * more than one: its cascaded `descendants` (parents before children on the wire) come first,
 * deepest last-listed first, then the victim — so undo, which runs a group in reverse, respawns the
 * victim and then each descendant parent-first, and redo destroys leaves before their parents.
 */
export function mutationToEntries(record: MutationRecord): UndoEntry[] {
  const victim = mutationToEntry(record);
  if (victim === null) {
    return [];
  }
  const descendants = (record.body as { descendants?: unknown }).descendants;
  if (victim.kind !== 'destroy' || !Array.isArray(descendants)) {
    return [victim];
  }
  const entries: UndoEntry[] = [];
  for (const raw of descendants) {
    const row = asRecord(raw);
    const entity = row === null ? null : asEntityId(row.entity);
    if (row !== null && entity !== null) {
      entries.push({ kind: 'destroy', entity, components: asRecord(row.components) ?? {} });
    }
  }
  return [...entries.reverse(), victim];
}

/**
 * The undo group for a completed `entity.reparent` (applied order `[reparent, set Transform]`). The
 * reparent itself undoes with `keep: 'local'`; the entity's exact local `Transform` bits ride a `set`
 * entry — `before` is the response's `previous_transform`, `after` is `afterTransform`, the local value
 * the caller read back after the reparent. The `set` is omitted when the entity had no `Transform`
 * (`previous_transform` null). A malformed shape yields `[]`, so nothing is recorded.
 */
export function reparentEntries(record: MutationRecord, afterTransform: unknown | null): UndoEntry[] {
  const method = (record.command as { method?: unknown }).method;
  const paramsObj = asRecord((record.command as { params?: unknown }).params);
  const body = record.body as { kind?: unknown; previous_parent?: unknown; previous_transform?: unknown };
  if (method !== 'entity.reparent' || paramsObj === null || body.kind !== 'reparented') {
    return [];
  }
  const entity = asEntityId(paramsObj.entity);
  const after = paramsObj.parent === undefined || paramsObj.parent === null ? null : asEntityId(paramsObj.parent);
  const before = body.previous_parent === undefined || body.previous_parent === null ? null : asEntityId(body.previous_parent);
  if (entity === null || (paramsObj.parent != null && after === null) || (body.previous_parent != null && before === null)) {
    return [];
  }
  const entries: UndoEntry[] = [{ kind: 'reparent', entity, before, after }];
  if (body.previous_transform !== undefined && body.previous_transform !== null && afterTransform !== null) {
    entries.push({ kind: 'set', entity, component: 'Transform', before: body.previous_transform, after: afterTransform });
  }
  return entries;
}

/**
 * Derives the {@link UndoEntry} that inverts `record`, or `null` for a shape we don't record.
 *
 * The four mutating methods map as: `component.set` → `set` (its `before` is `body.previous`, `null`
 * for a fresh insert whose inverse is a `component.remove`); `component.remove` → `remove`;
 * `entity.destroy` → `destroy` (the captured component map, `Name` included); `entity.spawn` →
 * `spawn` (id from the response, components from the request). A body whose `kind` doesn't match the
 * command, a missing/malformed entity handle, or any other method returns `null`.
 */
export function mutationToEntry(record: MutationRecord): UndoEntry | null {
  const method = (record.command as { method?: unknown }).method;
  const params = (record.command as { params?: unknown }).params;
  const paramsObj = asRecord(params);
  const body = record.body as {
    kind?: unknown;
    previous?: unknown;
    components?: unknown;
    entity?: unknown;
  };

  switch (method) {
    case 'component.set': {
      if (paramsObj === null || body.kind !== 'component_set') {
        return null;
      }
      const entity = asEntityId(paramsObj.entity);
      const component = paramsObj.component;
      if (entity === null || typeof component !== 'string') {
        return null;
      }
      // An absent (or null) `previous` means the component was freshly inserted — undo removes it.
      const before = body.previous === undefined || body.previous === null ? null : body.previous;
      return { kind: 'set', entity, component, before, after: paramsObj.value };
    }
    case 'component.remove': {
      if (paramsObj === null || body.kind !== 'component_removed') {
        return null;
      }
      const entity = asEntityId(paramsObj.entity);
      const component = paramsObj.component;
      if (entity === null || typeof component !== 'string') {
        return null;
      }
      return { kind: 'remove', entity, component, before: body.previous };
    }
    case 'entity.destroy': {
      if (paramsObj === null || body.kind !== 'entity_destroyed') {
        return null;
      }
      const entity = asEntityId(paramsObj.entity);
      if (entity === null) {
        return null;
      }
      return { kind: 'destroy', entity, components: asRecord(body.components) ?? {} };
    }
    case 'entity.spawn': {
      if (body.kind !== 'spawned') {
        return null;
      }
      const entity = asEntityId(body.entity);
      if (entity === null) {
        return null;
      }
      const components = { ...((paramsObj === null ? null : asRecord(paramsObj.components)) ?? {}) };
      // The convenience `name` param becomes the reserved `Name` component engine-side; fold it into
      // the entry so a redo (which respawns from `components`) restores the name too.
      if (typeof paramsObj?.name === 'string') {
        components.Name = paramsObj.name;
      }
      return { kind: 'spawn', entity, components };
    }
    default:
      return null;
  }
}
