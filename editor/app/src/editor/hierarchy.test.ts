import type { ResponseBody } from '@ludion/protocol';
import { describe, expect, it } from 'vitest';
import {
  buildTree,
  displayName,
  dropCommand,
  parentOptions,
  parseHierarchy,
  reparentCommand,
  type HierarchyRow,
  type InstanceStatus,
  type OutlineRow,
  type PrefabLink,
} from './hierarchy';
import type { EntityId } from './undo';

const E = (index: number, generation = 0): EntityId => ({ index, generation });
const row = (index: number, name: string | null, parent: EntityId | null, prefab: PrefabLink | null = null): HierarchyRow => ({
  entity: E(index),
  name,
  parent,
  prefab,
});
const part = (index: number, name: string | null, parent: EntityId | null, root: number, partPath: string, source = 'a.prefab.yaml'): HierarchyRow =>
  row(index, name, parent, { source, part: partPath, root: E(root) });

// --- parseHierarchy ------------------------------------------------------------------------------

describe('parseHierarchy', () => {
  it('reads entity, Name, and Parent out of a query_result', () => {
    const body = {
      kind: 'query_result',
      entities: [
        { entity: { index: 1, generation: 0 }, components: { Name: 'root' } },
        { entity: { index: 2, generation: 0 }, components: { Name: 'child', Parent: { index: 1, generation: 0 } } },
      ],
    } as unknown as ResponseBody;
    expect(parseHierarchy(body)).toEqual([
      { entity: E(1), name: 'root', parent: null, prefab: null },
      { entity: E(2), name: 'child', parent: E(1), prefab: null },
    ]);
  });

  it('reads PrefabInstance provenance, null when malformed', () => {
    const body = {
      kind: 'query_result',
      entities: [
        {
          entity: { index: 1, generation: 0 },
          components: { PrefabInstance: { source: 'a.prefab.yaml', part: 'body', root: { index: 1, generation: 0 } } },
        },
        { entity: { index: 2, generation: 0 }, components: { PrefabInstance: { source: 'a', part: 3 } } },
      ],
    } as unknown as ResponseBody;
    expect(parseHierarchy(body).map((r) => r.prefab)).toEqual([{ source: 'a.prefab.yaml', part: 'body', root: E(1) }, null]);
  });

  it('is null-safe for absent Name/Parent and tolerant of malformed entries', () => {
    const body = {
      kind: 'query_result',
      entities: [
        { entity: { index: 1, generation: 0 }, components: {} },
        { entity: { index: 2, generation: 0 } }, // no components key
        null,
        { entity: 'not-an-id', components: { Name: 'x' } },
        { entity: { index: 3, generation: 0 }, components: { Name: 42, Parent: 'garbage' } }, // wrong types
      ],
    } as unknown as ResponseBody;
    expect(parseHierarchy(body)).toEqual([
      { entity: E(1), name: null, parent: null, prefab: null },
      { entity: E(2), name: null, parent: null, prefab: null },
      { entity: E(3), name: null, parent: null, prefab: null },
    ]);
  });

  it('returns empty for a body that is not a query_result', () => {
    expect(parseHierarchy({ kind: 'ok' } as unknown as ResponseBody)).toEqual([]);
    expect(parseHierarchy({ kind: 'query_result', entities: 'nope' } as unknown as ResponseBody)).toEqual([]);
  });
});

// --- displayName -----------------------------------------------------------------------------------

describe('displayName', () => {
  it('uses Name when present', () => {
    expect(displayName(row(3, 'brick', null))).toBe('brick');
  });

  it('falls back to "entity <index>" when unnamed', () => {
    expect(displayName(row(7, null, null))).toBe('entity 7');
  });
});

// --- buildTree -------------------------------------------------------------------------------------

type EntityOutline = Extract<OutlineRow, { kind: 'entity' }>;
const ents = (tree: readonly OutlineRow[]): EntityOutline[] => tree.filter((r): r is EntityOutline => r.kind === 'entity');
/** `[kind-tag, depth]` per outline row: the entity index, or `inst` for a header. */
const shape = (tree: readonly OutlineRow[]): [number | string, number][] =>
  tree.map((r) => [r.kind === 'entity' ? r.row.entity.index : 'inst', r.depth]);

describe('buildTree', () => {
  it('nests children under their parent, ordered by index at every level', () => {
    const rows = [row(3, 'c', E(1)), row(1, 'root', null), row(2, 'b', E(1)), row(0, 'a', E(1))];
    const flat = ents(buildTree(rows));
    expect(flat.map((f) => [f.row.entity.index, f.depth])).toEqual([
      [1, 0],
      [0, 1],
      [2, 1],
      [3, 1],
    ]);
    expect(flat.every((f) => f.dangling === undefined && f.cyclic === undefined)).toBe(true);
  });

  it('orders multiple roots by index', () => {
    const rows = [row(5, 'e', null), row(1, 'a', null)];
    expect(ents(buildTree(rows)).map((f) => f.row.entity.index)).toEqual([1, 5]);
  });

  it('supports multi-level nesting', () => {
    const rows = [row(0, 'root', null), row(1, 'mid', E(0)), row(2, 'leaf', E(1))];
    const flat = ents(buildTree(rows));
    expect(flat.map((f) => f.depth)).toEqual([0, 1, 2]);
  });

  it('surfaces a dangling Parent (target destroyed/absent) at the root, marked dangling', () => {
    const rows = [row(0, 'orphan', E(99))]; // 99 does not exist
    const flat = ents(buildTree(rows));
    expect(flat).toEqual([{ kind: 'entity', row: rows[0], depth: 0, dangling: true }]);
  });

  it('does not mark a normal root (no parent at all) as dangling', () => {
    const rows = [row(0, 'root', null)];
    expect(ents(buildTree(rows))[0]!.dangling).toBeUndefined();
  });

  it('surfaces a two-entity parent cycle at the root, both marked cyclic, and terminates', () => {
    const rows = [row(0, 'a', E(1)), row(1, 'b', E(0))];
    const flat = ents(buildTree(rows));
    expect(flat).toHaveLength(2);
    expect(flat.every((f) => f.cyclic === true && f.depth === 0)).toBe(true);
  });

  it('surfaces a self-parented entity at the root, marked cyclic', () => {
    const rows = [row(0, 'self', E(0))];
    const flat = ents(buildTree(rows));
    expect(flat).toEqual([{ kind: 'entity', row: rows[0], depth: 0, cyclic: true }]);
  });

  it('nests a non-cyclic child under a cyclic root without looping', () => {
    // 0 <-> 1 cycle; 2's parent is 0 (a valid, existing, non-self reference) so 2 nests under it.
    const rows = [row(0, 'a', E(1)), row(1, 'b', E(0)), row(2, 'c', E(0))];
    const flat = ents(buildTree(rows));
    expect(flat).toHaveLength(3);
    const leaf = flat.find((f) => f.row.entity.index === 2)!;
    expect(leaf.cyclic).toBeUndefined();
    expect(leaf.dangling).toBeUndefined();
    expect(leaf.depth).toBe(1);
  });

  it('terminates on a longer cycle (3-entity ring) instead of hanging', () => {
    const rows = [row(0, 'a', E(1)), row(1, 'b', E(2)), row(2, 'c', E(0))];
    const flat = ents(buildTree(rows));
    expect(flat).toHaveLength(3);
    expect(flat.every((f) => f.cyclic === true)).toBe(true);
  });

  it('handles the empty list', () => {
    expect(buildTree([])).toEqual([]);
  });

  describe('prefab instances', () => {
    it('groups two root parts under one header', () => {
      const rows = [row(0, 'ground', null), part(1, 'tree/a', null, 1, 'a'), part(2, 'tree/b', null, 1, 'b')];
      const tree = buildTree(rows);
      expect(shape(tree)).toEqual([[0, 0], ['inst', 0], [1, 1], [2, 1]]);
      expect(tree[1]).toEqual({ kind: 'instance', key: 'inst:1:0', root: E(1), name: 'tree', source: 'a.prefab.yaml', depth: 0, parts: 2 });
    });

    it('keeps part nesting beneath the header and counts every part', () => {
      const rows = [part(1, 'i/a', null, 1, 'a'), part(2, 'i/a/b', E(1), 1, 'a/b'), part(3, 'i/a/b/c', E(2), 1, 'a/b/c')];
      const tree = buildTree(rows);
      expect(shape(tree)).toEqual([['inst', 0], [1, 1], [2, 2], [3, 3]]);
      expect(tree[0]).toMatchObject({ parts: 3 });
    });

    it('nests a header under a plain entity parent', () => {
      const rows = [row(0, 'holder', null), part(1, 'i/a', E(0), 1, 'a'), part(2, 'i/b', E(0), 1, 'b')];
      expect(shape(buildTree(rows))).toEqual([[0, 0], ['inst', 1], [1, 2], [2, 2]]);
    });

    it('makes two instances of one prefab two headers', () => {
      const rows = [part(1, 'x/a', null, 1, 'a'), part(2, 'y/a', null, 2, 'a')];
      const heads = buildTree(rows).filter((r) => r.kind === 'instance');
      expect(heads.map((h) => (h.kind === 'instance' ? h.name : ''))).toEqual(['x', 'y']);
    });

    it('strips the part suffix from the root name, else falls back to the source basename', () => {
      const named = buildTree([part(1, 'castle/walls/gate', null, 1, 'walls/gate')])[0]!;
      expect(named).toMatchObject({ name: 'castle' });
      const unnamed = buildTree([part(1, 'gate', null, 1, 'gate', 'prefabs/keep.prefab.yaml')])[0]!;
      expect(unnamed).toMatchObject({ name: 'keep.prefab.yaml' });
    });

    it('a collapsed instance emits its header but no parts', () => {
      const rows = [part(1, 'i/a', null, 1, 'a'), part(2, 'i/b', null, 1, 'b')];
      expect(shape(buildTree(rows, undefined, new Set(['inst:1:0'])))).toEqual([['inst', 0]]);
    });

    it('marks entities from a non-empty override map', () => {
      const rows = [part(1, 'i/a', null, 1, 'a'), part(2, 'i/b', null, 1, 'b')];
      const parts = new Map([['2:0', ['/Transform']], ['1:0', []]]);
      const tree = ents(buildTree(rows, { parts, instances: new Map() }));
      expect(tree.map((r) => r.overridden)).toEqual([undefined, true]);
    });

    it('sets a header status only when it is non-trivial', () => {
      const rows = [part(1, 'i/a', null, 1, 'a'), part(2, 'j/a', null, 2, 'a')];
      const trivial: InstanceStatus = { placementDiffers: false, removed: [], rootParts: [], diagnostic: null };
      const broken: InstanceStatus = { placementDiffers: false, removed: [], rootParts: [], diagnostic: { code: 'L1234', message: 'gone' } };
      const tree = buildTree(rows, {
        parts: new Map(),
        instances: new Map([['1:0', trivial], ['2:0', broken]]),
      });
      const headers = tree.filter((r) => r.kind === 'instance');
      expect(headers.map((h) => (h.kind === 'instance' ? h.status : 'x'))).toEqual([undefined, broken]);
    });
  });
});

// --- parentOptions -----------------------------------------------------------------------------------

describe('parentOptions', () => {
  it('leads with a "none" option', () => {
    const options = parentOptions([row(0, 'a', null)], E(0));
    expect(options[0]).toEqual({ entity: null, label: '(none)' });
  });

  it('excludes the subject itself', () => {
    const rows = [row(0, 'a', null), row(1, 'b', null)];
    const options = parentOptions(rows, E(0));
    expect(options.some((o) => o.entity !== null && o.entity.index === 0)).toBe(false);
  });

  it('excludes direct and transitive descendants (parenting to one would create a cycle)', () => {
    const rows = [row(0, 'root', null), row(1, 'mid', E(0)), row(2, 'leaf', E(1)), row(3, 'cousin', null)];
    const options = parentOptions(rows, E(0));
    const indices = options.map((o) => o.entity?.index).filter((i) => i !== undefined);
    expect(indices).toEqual([3]); // 1 and 2 are descendants of 0; 3 is unrelated
  });

  it('is cycle-safe: an existing cycle elsewhere does not hang the descendant walk', () => {
    const rows = [row(0, 'subject', null), row(1, 'a', E(2)), row(2, 'b', E(1)), row(3, 'other', null)];
    const options = parentOptions(rows, E(0));
    const indices = options.map((o) => o.entity?.index).filter((i) => i !== undefined);
    expect(indices).toEqual([1, 2, 3]); // the 1<->2 cycle is unrelated to subject 0, all offered
  });

  it('orders real targets by index, labeled via displayName', () => {
    const rows = [row(5, null, null), row(2, 'bee', null), row(0, 'subject', null)];
    const options = parentOptions(rows, E(0));
    expect(options.slice(1)).toEqual([
      { entity: E(2), label: 'bee' },
      { entity: E(5), label: 'entity 5' },
    ]);
  });
});

// --- reparentCommand -----------------------------------------------------------------------------------

describe('reparentCommand', () => {
  const subject = E(10);

  it('none -> none is a no-op', () => {
    expect(reparentCommand(subject, null, null)).toBeNull();
  });

  it('same parent chosen again is a no-op', () => {
    expect(reparentCommand(subject, E(1), E(1))).toBeNull();
  });

  it('none -> a parent sends entity.reparent keeping the world pose', () => {
    expect(reparentCommand(subject, null, E(1))).toEqual({
      method: 'entity.reparent',
      params: { entity: subject, parent: E(1), keep: 'world' },
    });
  });

  it('a parent -> none un-parents with a null parent', () => {
    expect(reparentCommand(subject, E(1), null)).toEqual({
      method: 'entity.reparent',
      params: { entity: subject, parent: null, keep: 'world' },
    });
  });

  it('parent A -> parent B sends entity.reparent onto B', () => {
    expect(reparentCommand(subject, E(1), E(2))).toEqual({
      method: 'entity.reparent',
      params: { entity: subject, parent: E(2), keep: 'world' },
    });
  });

  it('refuses to target the subject itself even if asked (defensive)', () => {
    expect(reparentCommand(subject, E(1), subject)).toBeNull();
  });
});

// --- dropCommand -----------------------------------------------------------------------------------

describe('dropCommand', () => {
  const rows = [row(0, 'root', null), row(1, 'mid', E(0)), row(2, 'leaf', E(1)), row(3, 'other', null)];

  it('drops onto a legal target as a world-keeping reparent', () => {
    expect(dropCommand(rows, E(2), E(3))).toEqual({
      method: 'entity.reparent',
      params: { entity: E(2), parent: E(3), keep: 'world' },
    });
  });

  it('refuses the subject itself and its descendants', () => {
    expect(dropCommand(rows, E(0), E(0))).toBeNull();
    expect(dropCommand(rows, E(0), E(1))).toBeNull();
    expect(dropCommand(rows, E(0), E(2))).toBeNull();
  });

  it('a drop on the scene root un-parents', () => {
    expect(dropCommand(rows, E(2), null)).toEqual({
      method: 'entity.reparent',
      params: { entity: E(2), parent: null, keep: 'world' },
    });
  });

  it('is a no-op onto the current parent or the root for a root entity', () => {
    expect(dropCommand(rows, E(2), E(1))).toBeNull();
    expect(dropCommand(rows, E(3), null)).toBeNull();
  });

  it('refuses a target that is not a live row', () => {
    expect(dropCommand(rows, E(2), E(99))).toBeNull();
  });
});
