import type { Command, ResponseBody } from '@ludion/protocol';
import { describe, expect, it } from 'vitest';
import {
  type ComponentSnapshot,
  instanceCommand,
  revertEntries,
  revertPartPointer,
  runInstanceAction,
  unpackEntries,
} from './prefab-actions';
import { type CallFn, type EntityId, UndoHistory, executeRedo, executeUndo } from './undo';

const CORE_REFS = new Map([
  ['Parent', ['']],
  ['PrefabInstance', ['/root']],
]);

const E = (index: number, generation = 0): EntityId => ({ index, generation });
const snap = (...pairs: [EntityId, Record<string, unknown>][]): ComponentSnapshot =>
  new Map(pairs.map(([e, c]) => [`${e.index}:${e.generation}`, c]));
const methodOf = (c: Command): string => (c as { method: string }).method;
const paramsOf = (c: Command): Record<string, unknown> => (c as { params: Record<string, unknown> }).params;
const link = (part: string) => ({ source: 'a.prefab.yaml', part, root: E(1) });

describe('instanceCommand', () => {
  it('builds apply/revert/unpack, with an optional pointer', () => {
    expect(instanceCommand('apply', E(2))).toEqual({ method: 'prefab.apply', params: { instance: E(2) } });
    expect(instanceCommand('revert', E(2), '/body')).toEqual({
      method: 'prefab.revert',
      params: { instance: E(2), pointer: '/body' },
    });
    expect(instanceCommand('unpack', E(2)).method).toBe('prefab.unpack');
  });

  it('revertPartPointer is /<part path>', () => {
    expect(revertPartPointer({ source: 's', part: 'turret/barrel', root: E(1) })).toBe('/turret/barrel');
  });
});

describe('revertEntries', () => {
  const e = E(3);
  it('diffs changed, dropped, restored; skips unchanged and PrefabInstance', () => {
    const before = snap([e, { A: { x: 1 }, B: { y: 1 }, Same: { z: 1 }, PrefabInstance: link('a') }]);
    const after = snap([e, { A: { x: 2 }, C: { w: 1 }, Same: { z: 1 }, PrefabInstance: { ...link('a'), part: 'zzz' } }]);
    expect(revertEntries(before, after, [e])).toEqual([
      { kind: 'set', entity: e, component: 'A', before: { x: 1 }, after: { x: 2 } },
      { kind: 'remove', entity: e, component: 'B', before: { y: 1 } },
      { kind: 'set', entity: e, component: 'C', before: null, after: { w: 1 } },
    ]);
  });

  it('skips an entity absent from either snapshot', () => {
    expect(revertEntries(snap(), snap([e, { A: 1 }]), [e])).toEqual([]);
    expect(revertEntries(snap([e, { A: 1 }]), snap(), [e])).toEqual([]);
  });
});

describe('unpackEntries', () => {
  it('removes PrefabInstance per part with the captured value', () => {
    const before = snap([E(1), { PrefabInstance: link('a') }], [E(2), { PrefabInstance: link('b') }]);
    expect(unpackEntries(before, [E(1), E(2), E(9)])).toEqual([
      { kind: 'remove', entity: E(1), component: 'PrefabInstance', before: link('a') },
      { kind: 'remove', entity: E(2), component: 'PrefabInstance', before: link('b') },
    ]);
  });

  it('undo re-sets PrefabInstance per part and redo removes it', async () => {
    const h = new UndoHistory();
    h.setEntityRefs(CORE_REFS);
    h.pushGroup(unpackEntries(snap([E(1), { PrefabInstance: link('a') }]), [E(1)]));
    const calls: Command[] = [];
    const call: CallFn = async (c) => {
      calls.push(c);
      return { kind: 'ack' } as ResponseBody;
    };
    await executeUndo(h, call);
    expect(methodOf(calls[0]!)).toBe('component.set');
    expect(paramsOf(calls[0]!)).toEqual({ entity: E(1), component: 'PrefabInstance', value: link('a') });
    await executeRedo(h, call);
    expect(methodOf(calls[1]!)).toBe('component.remove');
  });
});

describe('runInstanceAction', () => {
  const names = async (): Promise<string[]> => ['Transform', 'PrefabInstance'];
  const queryResult = (transformX: number, root = E(1)): ResponseBody =>
    ({
      kind: 'query_result',
      entities: [
        { entity: E(1), components: { Transform: { x: transformX }, PrefabInstance: { source: 's', part: 'a', root } } },
        { entity: E(2), components: { Transform: { x: 5 }, PrefabInstance: { source: 's', part: 'b', root } } },
        { entity: E(7), components: { Transform: { x: 9 } } },
      ],
    }) as unknown as ResponseBody;

  it('revert sweeps before and after and records the changed component', async () => {
    let swept = 0;
    const calls: Command[] = [];
    const call: CallFn = async (c) => {
      calls.push(c);
      if (methodOf(c) === 'query.entities') {
        return queryResult(swept++ === 0 ? 1 : 2);
      }
      return { kind: 'prefab_reverted', entities: [E(1)], not_restored: [] } as unknown as ResponseBody;
    };
    const { entries } = await runInstanceAction('revert', E(2), call, undefined, names);
    expect(calls.map(methodOf)).toEqual(['query.entities', 'prefab.revert', 'query.entities']);
    expect(entries).toEqual([{ kind: 'set', entity: E(1), component: 'Transform', before: { x: 1 }, after: { x: 2 } }]);
  });

  it('unpack records one remove per response entity', async () => {
    const call: CallFn = async (c) =>
      methodOf(c) === 'query.entities'
        ? queryResult(1)
        : ({ kind: 'prefab_unpacked', entities: [E(1), E(2)] } as unknown as ResponseBody);
    const { entries } = await runInstanceAction('unpack', E(1), call, undefined, names);
    expect(entries.map((x) => [x.kind, x.entity])).toEqual([
      ['remove', E(1)],
      ['remove', E(2)],
    ]);
  });

  it('apply just calls and records nothing', async () => {
    const calls: Command[] = [];
    const call: CallFn = async (c) => {
      calls.push(c);
      return { kind: 'prefab_applied', path: 'a', parts: [], removed: [] } as unknown as ResponseBody;
    };
    const { entries } = await runInstanceAction('apply', E(1), call, undefined, names);
    expect(entries).toEqual([]);
    expect(calls.map(methodOf)).toEqual(['prefab.apply']);
  });

  it('a rejected call rethrows', async () => {
    const boom = new Error('L1235');
    const call: CallFn = async (c) => {
      if (methodOf(c) === 'query.entities') {
        return queryResult(1);
      }
      throw boom;
    };
    await expect(runInstanceAction('revert', E(1), call, undefined, names)).rejects.toBe(boom);
  });
});
