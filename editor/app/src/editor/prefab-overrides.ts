// Reading `prefab.overrides` into the outliner's `OverrideView`, and the refresh policy the panel
// polls it with. The call runs a full collapse engine-side, so it is throttled and never overlaps
// (`shouldRefresh`). Pure and transport-free like the other `editor/*` modules.

import type { ResponseBody } from '@ludion/protocol';
import { EMPTY_OVERRIDES, entityKey, type HierarchyRow, type InstanceStatus, type OverrideView } from './hierarchy';
import type { EntityId } from './undo';

/** The slowest cadence `prefab.overrides` is polled at, in milliseconds. */
export const OVERRIDES_INTERVAL_MS = 1000;

/** Whether a `prefab.overrides` refresh should start now. `forced` (after a panel action) skips the
 *  throttle; a refresh already `inFlight` never overlaps — the caller records a pending request and
 *  runs it once when the in-flight one ends. */
export function shouldRefresh(now: number, lastAt: number | null, inFlight: boolean, forced: boolean): boolean {
  if (inFlight) {
    return false;
  }
  return forced || lastAt === null || now - lastAt >= OVERRIDES_INTERVAL_MS;
}

/** Whether the ↺ button is offered for an override `pointer` on `row`: not for a prefab root part's
 *  placement (`/Transform…`, `/Parent…`), which the engine leaves to the user (edit it, or apply).
 *  Root parts come from `prefab.overrides`' `root_parts`, so this follows the engine's revert rule
 *  whatever the live tree looks like. */
export function canRevertPointer(pointer: string, row: HierarchyRow, overrides: OverrideView): boolean {
  const placement = /^\/(Transform|Parent)(\/|$)/.test(pointer);
  if (!placement || row.prefab === null) {
    return true;
  }
  const status = overrides.instances.get(entityKey(row.prefab.root));
  return !status?.rootParts.includes(row.prefab.part);
}

function asEntity(value: unknown): EntityId | null {
  if (typeof value !== 'object' || value === null) {
    return null;
  }
  const o = value as { index?: unknown; generation?: unknown };
  return typeof o.index === 'number' && typeof o.generation === 'number' ? { index: o.index, generation: o.generation } : null;
}

function asStrings(value: unknown): string[] | null {
  return Array.isArray(value) && value.every((v) => typeof v === 'string') ? (value as string[]) : null;
}

/**
 * Reads a `prefab_overrides` body against the latest `rows`: pointers are keyed by the part's entity
 * (found by `root|part path`, the same string as `PrefabInstance.part`), statuses by instance root.
 * Tolerant — a wrong `kind` reads as `EMPTY_OVERRIDES`; a malformed instance is skipped; a part path
 * with no live row is ignored; empty pointer lists are dropped.
 */
export function parseOverrides(body: ResponseBody, rows: readonly HierarchyRow[]): OverrideView {
  const b = body as { kind?: unknown; instances?: unknown };
  if (b.kind !== 'prefab_overrides' || !Array.isArray(b.instances)) {
    return EMPTY_OVERRIDES;
  }
  const byPart = new Map<string, string>();
  for (const row of rows) {
    if (row.prefab !== null) {
      byPart.set(`${entityKey(row.prefab.root)}|${row.prefab.part}`, entityKey(row.entity));
    }
  }
  const parts = new Map<string, readonly string[]>();
  const instances = new Map<string, InstanceStatus>();
  for (const raw of b.instances) {
    if (typeof raw !== 'object' || raw === null) {
      continue;
    }
    const inst = raw as {
      root?: unknown;
      placement_differs?: unknown;
      overrides?: unknown;
      removed?: unknown;
      root_parts?: unknown;
      diagnostic?: unknown;
    };
    const root = asEntity(inst.root);
    if (root === null) {
      continue;
    }
    const rootKey = entityKey(root);
    const diag = inst.diagnostic as { code?: unknown; message?: unknown } | null | undefined;
    const diagnostic =
      typeof diag === 'object' && diag !== null && typeof diag.code === 'string' && typeof diag.message === 'string'
        ? { code: diag.code, message: diag.message }
        : null;
    instances.set(rootKey, {
      placementDiffers: inst.placement_differs === true,
      removed: asStrings(inst.removed) ?? [],
      rootParts: asStrings(inst.root_parts) ?? [],
      diagnostic,
    });
    if (typeof inst.overrides !== 'object' || inst.overrides === null) {
      continue;
    }
    for (const [path, pointers] of Object.entries(inst.overrides)) {
      const list = asStrings(pointers);
      const entity = byPart.get(`${rootKey}|${path}`);
      if (list !== null && list.length > 0 && entity !== undefined) {
        parts.set(entity, list);
      }
    }
  }
  return { parts, instances };
}
