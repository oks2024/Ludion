// The hierarchy tree: reading `query.entities` into a parent/child forest, and the reparent-target
// and reparent-command logic the panel's `<select>` and drag-drop need.
//
// The engine composes world poses through `Parent`, cascades a despawn to descendants, and
// `entity.reparent` refuses a cycle. A scene on disk can still hold a `Parent` pointing at a
// destroyed entity or a cycle of `Parent`s, so every reader in this module tolerates both without
// treating them as errors: a dangling ref surfaces its owner at the tree root marked `dangling`, and
// a cycle surfaces every member at the root marked `cyclic` (breaking the mutual nesting that would
// otherwise loop forever) rather than crashing or hanging. `scene.save` refuses a dangling `Parent`,
// so surfacing it here is how a human finds and fixes it.
//
// Prefab instances (`PrefabInstance` provenance on each part) are grouped under one synthetic header
// row per instance; parts keep their own tree beneath it.
//
// Transport-free like the other `editor/*` modules: `parseHierarchy` reads a plain `ResponseBody`,
// everything else takes/returns plain data, so the whole module is unit-testable with object literals
// (see hierarchy.test.ts). `HierarchyPanel.tsx` is thin wiring on top.

import type { Command, ResponseBody } from '@ludion/protocol';
import type { EntityId } from './undo';

/** A prefab instance part's provenance (`PrefabInstance` on the wire): the outermost prefab file, the
 *  composed part path (`turret/barrel`), and the instance's first part. `root` is the instance identity. */
export interface PrefabLink {
  readonly source: string;
  readonly part: string;
  readonly root: EntityId;
}

/** One entity as the hierarchy panel cares about it: its name (if any), its `Parent` (if any), and its
 *  prefab provenance (`null` for a plain entity). */
export interface HierarchyRow {
  readonly entity: EntityId;
  readonly name: string | null;
  readonly parent: EntityId | null;
  readonly prefab: PrefabLink | null;
}

/** One row of the flattened outline: an entity row — its nesting depth (0 = root) and whether it's a
 *  broken reference (a dangling `Parent`, or a member of a parent cycle; both are root-level by
 *  construction, see module comment) or carries prefab overrides — or a synthetic instance header
 *  grouping the top-level parts of one prefab instance. */
export type OutlineRow =
  | {
      readonly kind: 'entity';
      readonly row: HierarchyRow;
      readonly depth: number;
      readonly dangling?: boolean;
      readonly cyclic?: boolean;
      readonly overridden?: true;
    }
  | {
      readonly kind: 'instance';
      /** `'inst:'` + the root's `entityKey` — the collapse-set key. */
      readonly key: string;
      readonly root: EntityId;
      readonly name: string;
      readonly source: string;
      readonly depth: number;
      /** Every part of the instance, nested ones included. */
      readonly parts: number;
      /** Set only when non-trivial: a diagnostic, a moved placement, or removed parts. */
      readonly status?: InstanceStatus;
    };

/** One prefab instance's state beyond its parts, from `prefab.overrides`. */
export interface InstanceStatus {
  readonly placementDiffers: boolean;
  /** Part paths the instance dropped from its prefab. */
  readonly removed: readonly string[];
  /** The prefab's root part paths: their `Transform`/`Parent` are the placement `prefab.revert` skips. */
  readonly rootParts: readonly string[];
  /** The L1234/L1230 a save would raise for this instance. */
  readonly diagnostic: { readonly code: string; readonly message: string } | null;
}

/** What `prefab.overrides` reports, keyed for `buildTree`: part entity key → override pointers, and
 *  instance root key → status. `buildTree` marks any part with a non-empty list. */
export interface OverrideView {
  readonly parts: ReadonlyMap<string, readonly string[]>;
  readonly instances: ReadonlyMap<string, InstanceStatus>;
}

export const EMPTY_OVERRIDES: OverrideView = { parts: new Map(), instances: new Map() };

const EMPTY_COLLAPSED: ReadonlySet<string> = new Set();

/** One reparent target: a real entity, or the `entity: null` "none" (un-parent) option. */
export interface ParentOption {
  readonly entity: EntityId | null;
  readonly label: string;
}

/** A stable `index:generation` key for an entity handle — the same rule the inspector's list uses
 *  (`viewer/Inspector.tsx`'s `entityKey`), duplicated here so this module stays transport- and
 *  React-free (no import from a `.tsx` file). */
export function entityKey(entity: EntityId): string {
  return `${entity.index}:${entity.generation}`;
}

function sameEntity(a: EntityId | null, b: EntityId | null): boolean {
  if (a === null || b === null) {
    return a === b;
  }
  return a.index === b.index && a.generation === b.generation;
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

/**
 * Reads a `query.entities` `query_result` body (requested with `include: ['Name', 'Parent']`, plus
 * `'PrefabInstance'` for the panel) into rows; a caller that doesn't ask for it reads `prefab: null`. Tolerant of malformed entries — anything that isn't the expected shape is skipped rather than
 * thrown on, matching the other wire readers (`entity-ops.ts`, `mutations.ts`). `Name` is a bare-string
 * newtype on the wire; anything else (missing, wrong type) reads as `null`.
 */
export function parseHierarchy(body: ResponseBody): HierarchyRow[] {
  const b = body as { kind?: unknown; entities?: unknown };
  if (b.kind !== 'query_result' || !Array.isArray(b.entities)) {
    return [];
  }
  const rows: HierarchyRow[] = [];
  for (const raw of b.entities) {
    if (typeof raw !== 'object' || raw === null) {
      continue;
    }
    const match = raw as { entity?: unknown; components?: unknown };
    const entity = asEntityId(match.entity);
    if (entity === null) {
      continue;
    }
    const components =
      typeof match.components === 'object' && match.components !== null
        ? (match.components as Record<string, unknown>)
        : {};
    const name = typeof components.Name === 'string' ? components.Name : null;
    rows.push({ entity, name, parent: asEntityId(components.Parent), prefab: asPrefabLink(components.PrefabInstance) });
  }
  return rows;
}

/** Reads a `PrefabInstance` value, or `null` when absent or malformed. */
function asPrefabLink(value: unknown): PrefabLink | null {
  if (typeof value !== 'object' || value === null) {
    return null;
  }
  const o = value as { source?: unknown; part?: unknown; root?: unknown };
  const root = asEntityId(o.root);
  if (typeof o.source !== 'string' || typeof o.part !== 'string' || root === null) {
    return null;
  }
  return { source: o.source, part: o.part, root };
}

/** `Name` if present, else `entity <index>`. */
export function displayName(row: HierarchyRow): string {
  return row.name ?? `entity ${row.entity.index}`;
}

/**
 * Whether `row` is part of a parent cycle: walking up its `parent` chain eventually returns to `row`
 * itself. Bounded by a visited set (never loops forever) — if the walk instead lands on a *different*
 * already-seen entity, that's some other cycle further up the chain, not one `row` belongs to, so this
 * returns `false` (row nests normally under its immediate parent, which is itself surfaced as a cyclic
 * root by its own call to this function).
 */
function isCyclic(row: HierarchyRow, byKey: ReadonlyMap<string, HierarchyRow>): boolean {
  const startKey = entityKey(row.entity);
  const seen = new Set<string>();
  let current = row.parent;
  while (current !== null) {
    const key = entityKey(current);
    if (key === startKey) {
      return true;
    }
    if (seen.has(key)) {
      return false;
    }
    seen.add(key);
    const parentRow = byKey.get(key);
    if (parentRow === undefined) {
      return false; // dangling somewhere up the chain: not a cycle back to `row`
    }
    current = parentRow.parent;
  }
  return false;
}

interface RowStatus {
  readonly dangling: boolean;
  readonly cyclic: boolean;
  readonly isRoot: boolean;
}

/** Per-row dangling/cyclic/root classification, computed once and shared by tree-building. */
function classify(rows: readonly HierarchyRow[], byKey: ReadonlyMap<string, HierarchyRow>): Map<string, RowStatus> {
  const status = new Map<string, RowStatus>();
  for (const row of rows) {
    const key = entityKey(row.entity);
    if (row.parent === null) {
      status.set(key, { dangling: false, cyclic: false, isRoot: true });
      continue;
    }
    const cyclic = isCyclic(row, byKey);
    const parentExists = byKey.has(entityKey(row.parent));
    status.set(key, {
      dangling: !cyclic && !parentExists,
      cyclic,
      isRoot: cyclic || !parentExists,
    });
  }
  return status;
}

const byIndex = (a: HierarchyRow, b: HierarchyRow): number => a.entity.index - b.entity.index;

/**
 * Builds a forest ordered by entity index and flattens it into a render list: roots first (entities
 * with no parent, a dangling parent, or membership in a parent cycle — see module comment), each
 * followed depth-first by its children, also ordered by index. Cycle-safe and dangling-safe by
 * construction: a row is only ever nested under a parent that (a) exists and (b) the row is not itself
 * cyclic with, so the DFS can't revisit a row and terminates in at most `rows.length` steps.
 */
export function buildTree(
  rows: readonly HierarchyRow[],
  overrides: OverrideView = EMPTY_OVERRIDES,
  collapsed: ReadonlySet<string> = EMPTY_COLLAPSED,
): OutlineRow[] {
  const byKey = new Map<string, HierarchyRow>();
  for (const row of rows) {
    byKey.set(entityKey(row.entity), row);
  }
  const status = classify(rows, byKey);

  const childrenOf = new Map<string, HierarchyRow[]>();
  const roots: HierarchyRow[] = [];
  for (const row of rows) {
    const key = entityKey(row.entity);
    const rowStatus = status.get(key)!;
    if (rowStatus.isRoot) {
      roots.push(row);
      continue;
    }
    const parentKey = entityKey(row.parent!);
    const siblings = childrenOf.get(parentKey);
    if (siblings === undefined) {
      childrenOf.set(parentKey, [row]);
    } else {
      siblings.push(row);
    }
  }
  roots.sort(byIndex);
  for (const siblings of childrenOf.values()) {
    siblings.sort(byIndex);
  }

  const partCount = new Map<string, number>();
  for (const row of rows) {
    if (row.prefab !== null) {
      const rootKey = entityKey(row.prefab.root);
      partCount.set(rootKey, (partCount.get(rootKey) ?? 0) + 1);
    }
  }

  const out: OutlineRow[] = [];
  const visit = (row: HierarchyRow, depth: number): void => {
    const key = entityKey(row.entity);
    const rowStatus = status.get(key)!;
    out.push({
      kind: 'entity',
      row,
      depth,
      ...(rowStatus.dangling ? { dangling: true } : {}),
      ...(rowStatus.cyclic ? { cyclic: true } : {}),
      ...((overrides.parts.get(key)?.length ?? 0) > 0 ? { overridden: true as const } : {}),
    });
    visitList(childrenOf.get(key) ?? [], depth + 1, byKey.get(key) ?? null);
  };

  // One sibling list: the top-level parts of an instance (its parts whose tree parent is not a part
  // of the same instance) collapse into one header at the position of the first of them.
  const visitList = (list: readonly HierarchyRow[], depth: number, parent: HierarchyRow | null): void => {
    const emitted = new Set<string>();
    for (const row of list) {
      const link = row.prefab;
      if (link === null || (parent?.prefab != null && sameEntity(parent.prefab.root, link.root))) {
        visit(row, depth);
        continue;
      }
      const rootKey = entityKey(link.root);
      if (emitted.has(rootKey)) {
        continue;
      }
      emitted.add(rootKey);
      const key = `inst:${rootKey}`;
      const status = overrides.instances.get(rootKey);
      const notable = status !== undefined && (status.diagnostic !== null || status.placementDiffers || status.removed.length > 0) ? status : null;
      out.push({
        kind: 'instance',
        key,
        root: link.root,
        name: instanceName(byKey.get(rootKey), link),
        source: link.source,
        depth,
        parts: partCount.get(rootKey) ?? 0,
        ...(notable !== null ? { status: notable } : {}),
      });
      if (collapsed.has(key)) {
        continue;
      }
      for (const part of list) {
        if (part.prefab !== null && sameEntity(part.prefab.root, link.root)) {
          visit(part, depth + 1);
        }
      }
    }
  };
  visitList(roots, 0, null);
  return out;
}

/** The instance's display name: the root part's `Name` minus its `/<part>` suffix (a named spawn names
 *  parts `<instance>/<part>`), else the prefab file's basename. */
function instanceName(rootRow: HierarchyRow | undefined, link: PrefabLink): string {
  const suffix = `/${rootRow?.prefab?.part ?? link.part}`;
  if (rootRow?.name != null && rootRow.name.endsWith(suffix) && rootRow.name.length > suffix.length) {
    return rootRow.name.slice(0, -suffix.length);
  }
  return link.source.split('/').pop() ?? link.source;
}

/**
 * The legal reparent targets for `subject`: every entity except itself and its own descendants
 * (parenting to a descendant would create a cycle; the editor simply doesn't offer it), plus a leading
 * "none" option (un-parent). Cycle-safe: descendant collection walks the raw `Parent` graph with a
 * visited set, so an existing cycle elsewhere can't make it loop forever.
 */
export function parentOptions(rows: readonly HierarchyRow[], subject: EntityId): ParentOption[] {
  const subjectKey = entityKey(subject);
  const childrenByParent = new Map<string, string[]>();
  for (const row of rows) {
    if (row.parent === null) {
      continue;
    }
    const parentKey = entityKey(row.parent);
    const childKey = entityKey(row.entity);
    const siblings = childrenByParent.get(parentKey);
    if (siblings === undefined) {
      childrenByParent.set(parentKey, [childKey]);
    } else {
      siblings.push(childKey);
    }
  }

  const excluded = new Set<string>([subjectKey]);
  const queue: string[] = [subjectKey];
  while (queue.length > 0) {
    const current = queue.shift()!;
    for (const childKey of childrenByParent.get(current) ?? []) {
      if (excluded.has(childKey)) {
        continue; // cycle guard: already queued/visited
      }
      excluded.add(childKey);
      queue.push(childKey);
    }
  }

  const options: ParentOption[] = [{ entity: null, label: '(none)' }];
  for (const row of [...rows].sort(byIndex)) {
    if (excluded.has(entityKey(row.entity))) {
      continue;
    }
    options.push({ entity: row.entity, label: displayName(row) });
  }
  return options;
}

/**
 * The command to send for a reparent choice, or `null` for a no-op (`chosen` names the same parent
 * `subject` already has — including both being "none"). It is `entity.reparent` with `keep: 'world'`
 * (`chosen: null` un-parents), so the entity keeps its world pose and its subtree follows. Guards
 * against ever targeting `subject` itself, even though `parentOptions` never offers it — defensive,
 * not load-bearing.
 */
export function reparentCommand(subject: EntityId, current: EntityId | null, chosen: EntityId | null): Command | null {
  if (sameEntity(current, chosen)) {
    return null;
  }
  if (chosen !== null && sameEntity(chosen, subject)) {
    return null;
  }
  return { method: 'entity.reparent', params: { entity: subject, parent: chosen, keep: 'world' } };
}

/**
 * The command for dropping `subject` onto `target` (`null` = the scene-root row, un-parent), or `null`
 * when the drop is refused or a no-op: `target` must be a legal reparent option (never `subject` or one
 * of its descendants) and differ from the parent `subject` already has.
 */
export function dropCommand(rows: readonly HierarchyRow[], subject: EntityId, target: EntityId | null): Command | null {
  const legal = parentOptions(rows, subject).some((o) => sameEntity(o.entity, target));
  if (!legal) {
    return null;
  }
  const current = rows.find((r) => sameEntity(r.entity, subject))?.parent ?? null;
  return reparentCommand(subject, current, target);
}
