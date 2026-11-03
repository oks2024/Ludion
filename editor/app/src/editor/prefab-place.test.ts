import type { Command, ResponseBody } from '@ludion/protocol';
import { describe, expect, it } from 'vitest';
import { freeInstanceName, instanceBaseName, placePrefab, spawnCommand, spawnEntries } from './prefab-place';
import { type CallFn, type EntityId, UndoHistory, executeRedo, executeUndo } from './undo';

const CORE_REFS = new Map([
  ['Parent', ['']],
  ['PrefabInstance', ['/root']],
]);

const E = (index: number, generation = 0): EntityId => ({ index, generation });
const k = (e: EntityId): string => `${e.index}:${e.generation}`;
const methodOf = (c: Command): string => (c as { method: string }).method;
const paramsOf = (c: Command): Record<string, unknown> => (c as unknown as { params: Record<string, unknown> }).params;

describe('names', () => {
  it('strips folders and the double extension', () => {
    expect(instanceBaseName('props/crate.prefab.yaml')).toBe('crate');
    expect(instanceBaseName('crate.prefab.yaml')).toBe('crate');
  });

  it('dedupes against a live name or a live part path', () => {
    expect(freeInstanceName('crate', [])).toBe('crate');
    expect(freeInstanceName('crate', ['crate'])).toBe('crate-2');
    expect(freeInstanceName('crate', ['crate/lid'])).toBe('crate-2');
    expect(freeInstanceName('crate', ['crate', 'crate-2/lid'])).toBe('crate-3');
    expect(freeInstanceName('crate', ['crates', 'box'])).toBe('crate');
  });
});

describe('spawnCommand', () => {
  it('sends the path, name and an identity-rotation placement', () => {
    expect(spawnCommand('p/crate.prefab.yaml', 'crate', [1, 2, 3])).toEqual({
      method: 'prefab.spawn',
      params: {
        path: 'p/crate.prefab.yaml',
        name: 'crate',
        transform: { translation: [1, 2, 3], rotation: [0, 0, 0], scale: [1, 1, 1] },
      },
    });
  });
});

describe('spawnEntries', () => {
  const root = E(1);
  const child = E(2);
  const byKey = new Map<string, Record<string, unknown>>([
    [k(child), { Name: 'crate/lid', Parent: root }],
    [k(root), { Name: 'crate' }],
  ]);

  it('orders parents before children even when the child is listed first', () => {
    expect(spawnEntries(byKey, [child, root]).map((e) => e.entity)).toEqual([root, child]);
  });

  it('treats a parent outside the set as a root and keeps peer order', () => {
    const outside = new Map<string, Record<string, unknown>>([
      [k(E(5)), { Parent: E(99) }],
      [k(E(4)), {}],
    ]);
    expect(spawnEntries(outside, [E(5), E(4)]).map((e) => e.entity)).toEqual([E(5), E(4)]);
  });

  it('skips an entity with no component map', () => {
    expect(spawnEntries(byKey, [E(7), root]).map((e) => e.entity)).toEqual([root]);
  });
});

/** A fake engine holding a two-part instance (the child listed first by the prefab). */
function fakeEngine() {
  let next = 10;
  const log: string[] = [];
  const world = new Map<string, Record<string, unknown>>();
  const ids = new Map<string, EntityId>();
  const call: CallFn = async (cmd) => {
    const method = methodOf(cmd);
    const params = paramsOf(cmd);
    if (method === 'engine.describe') {
      return { kind: 'engine_description', description: { components: { Name: {}, Parent: {}, PrefabInstance: {} } } } as unknown as ResponseBody;
    }
    if (method === 'query.entities') {
      const entities = [...world.entries()].map(([key, components]) => ({ entity: ids.get(key)!, components }));
      return { kind: 'query_result', entities } as unknown as ResponseBody;
    }
    if (method === 'prefab.spawn') {
      const child = E(next++);
      const root = E(next++);
      world.set(k(child), { Name: 'crate/lid', Parent: root, PrefabInstance: { source: 'c', part: 'lid', root } });
      world.set(k(root), { Name: 'crate', PrefabInstance: { source: 'c', part: '', root } });
      ids.set(k(child), child);
      ids.set(k(root), root);
      return { kind: 'prefab_spawned', entities: [child, root] } as unknown as ResponseBody;
    }
    if (method === 'entity.destroy') {
      const e = params.entity as EntityId;
      log.push(`destroy ${world.get(k(e))?.Name as string}`);
      world.delete(k(e));
      return { kind: 'destroyed', components: {} } as unknown as ResponseBody;
    }
    if (method === 'entity.spawn') {
      const e = E(next++);
      const components = params.components as Record<string, unknown>;
      log.push(`spawn ${components.Name as string}`);
      world.set(k(e), components);
      ids.set(k(e), e);
      return { kind: 'spawned', entity: e } as unknown as ResponseBody;
    }
    if (method === 'component.set') {
      const live = world.get(k(params.entity as EntityId))!;
      live[params.component as string] = params.value;
      log.push(`set ${live.Name as string}`);
      return { kind: 'ack' } as unknown as ResponseBody;
    }
    throw new Error(`unexpected ${method}`);
  };
  return { call, world, log };
}

describe('placePrefab', () => {
  it('records parents first and names the instance past a taken name', async () => {
    const { call, world } = fakeEngine();
    await placePrefab('crate.prefab.yaml', [0, 0, 0], call);
    const second = await placePrefab('crate.prefab.yaml', [1, 0, 0], call);
    expect(second.entries.map((e) => (e as unknown as { components: { Name: string } }).components.Name)).toEqual(['crate', 'crate/lid']);
    expect(world.size).toBe(4);
  });

  it('rethrows a rejected spawn', async () => {
    const call: CallFn = async (cmd) => {
      if (methodOf(cmd) === 'prefab.spawn') {
        throw new Error('L3040');
      }
      return { kind: 'query_result', entities: [] } as unknown as ResponseBody;
    };
    await expect(placePrefab('x.prefab.yaml', [0, 0, 0], call)).rejects.toThrow('L3040');
  });

  it('undo removes children first, redo respawns parents first with refs remapped', async () => {
    const { call, world, log } = fakeEngine();
    const { entries } = await placePrefab('crate.prefab.yaml', [0, 0, 0], call);
    const history = new UndoHistory();
    history.setEntityRefs(CORE_REFS);
    history.pushGroup(entries);

    const undone = await executeUndo(history, call);
    expect(undone?.ok).toBe(true);
    expect(log).toEqual(['destroy crate/lid', 'destroy crate']);
    expect(world.size).toBe(0);

    log.length = 0;
    const redone = await executeRedo(history, call);
    expect(redone?.ok).toBe(true);
    expect(log).toEqual(['spawn crate', 'spawn crate/lid', 'set crate']);
    const live = [...world.values()];
    const root = [...world.entries()].find(([, c]) => c.Name === 'crate')![0];
    const lid = live.find((c) => c.Name === 'crate/lid')!;
    expect(k(lid.Parent as EntityId)).toBe(root);
    expect(k((lid.PrefabInstance as { root: EntityId }).root)).toBe(root);
    const rootPart = world.get(root)!;
    expect(k((rootPart.PrefabInstance as { root: EntityId }).root)).toBe(root);
  });
});
