import type { Command, ResponseBody } from '@ludion/protocol';
import { describe, expect, it } from 'vitest';
import type { CallFn, EntityId } from './undo';
import { destroyEntity, duplicateEntity, spawnArchetype } from './entity-ops';

// --- helpers -----------------------------------------------------------------------------------

const E = (index: number, generation: number): EntityId => ({ index, generation });

/** The `method` off a recorded command, past the generated union's index signature. */
function methodOf(command: Command): string {
  return (command as { method: string }).method;
}

/** The `params` off a recorded command. */
function paramsOf(command: Command): Record<string, unknown> {
  return (command as { params?: Record<string, unknown> }).params ?? {};
}

/** A `CallFn` that records every command and answers via a per-method dispatch table. A handler that
 *  is a function is invoked (and may throw to simulate a rejection); anything else is returned as-is. */
function mockCall(
  calls: Command[],
  table: Record<string, ((command: Command) => ResponseBody) | ResponseBody>,
): CallFn {
  return async (command) => {
    calls.push(command);
    const entry = table[methodOf(command)];
    if (entry === undefined) {
      throw new Error(`unexpected method ${methodOf(command)}`);
    }
    return typeof entry === 'function' ? entry(command) : entry;
  };
}

const describeBody = {
  kind: 'engine_description',
  description: { components: { Transform: {}, Sprite: {}, Parent: {}, Name: {} } },
} as unknown as ResponseBody;

// --- spawnArchetype ----------------------------------------------------------------------------

describe('spawnArchetype', () => {
  const command = { method: 'entity.spawn', params: { name: 'Sprite', components: {} } } as Command;

  it('returns the spawn record and the fresh entity id', async () => {
    const calls: Command[] = [];
    const call = mockCall(calls, { 'entity.spawn': { kind: 'spawned', entity: E(5, 0) } as ResponseBody });
    const result = await spawnArchetype(command, call);
    expect(result.error).toBeUndefined();
    expect(result.newEntity).toEqual(E(5, 0));
    expect(result.records).toEqual([{ command, body: { kind: 'spawned', entity: E(5, 0) } }]);
  });

  it('fails when the response carries no entity', async () => {
    const calls: Command[] = [];
    const call = mockCall(calls, { 'entity.spawn': { kind: 'spawned' } as ResponseBody });
    const result = await spawnArchetype(command, call);
    expect(result.newEntity).toBeUndefined();
    expect(result.records).toEqual([]);
    expect(result.error).toBeInstanceOf(Error);
  });

  it('captures a rejection into error', async () => {
    const boom = new Error('rejected');
    const call: CallFn = () => Promise.reject(boom);
    const result = await spawnArchetype(command, call);
    expect(result.error).toBe(boom);
    expect(result.records).toEqual([]);
  });
});

// --- duplicateEntity ---------------------------------------------------------------------------

describe('duplicateEntity', () => {
  it('sweeps the selection and spawns an exact copy (Name in components, Parent copied as-is)', async () => {
    const selected = {
      Transform: { translation: [1, 2, 3] },
      Sprite: { size: [1, 1] },
      Name: 'brick',
      Parent: E(9, 0),
    };
    const calls: Command[] = [];
    const call = mockCall(calls, {
      'engine.describe': describeBody,
      'query.entities': {
        kind: 'query_result',
        entities: [
          { entity: E(1, 0), components: selected },
          { entity: E(2, 0), components: {} },
        ],
      } as unknown as ResponseBody,
      'entity.spawn': { kind: 'spawned', entity: E(7, 0) } as ResponseBody,
    });

    const result = await duplicateEntity(E(1, 0), call);

    expect(result.error).toBeUndefined();
    expect(result.newEntity).toEqual(E(7, 0));
    expect(result.records).toHaveLength(1);
    const spawn = result.records[0]!.command;
    expect(methodOf(spawn)).toBe('entity.spawn');
    // Name rides inside components; no `name` param (passing both is L3014). Parent copied verbatim.
    expect(paramsOf(spawn).name).toBeUndefined();
    expect(paramsOf(spawn).components).toEqual(selected);
    // The include list is the full registry component set from engine.describe.
    const query = calls.find((c) => methodOf(c) === 'query.entities')!;
    expect(paramsOf(query).include).toEqual(['Transform', 'Sprite', 'Parent', 'Name']);
    expect(paramsOf(query).entities).toEqual([E(1, 0)]);
  });

  it('fails when the selection is absent from the sweep', async () => {
    const calls: Command[] = [];
    const call = mockCall(calls, {
      'engine.describe': describeBody,
      'query.entities': { kind: 'query_result', entities: [{ entity: E(2, 0), components: {} }] } as unknown as ResponseBody,
    });
    const result = await duplicateEntity(E(1, 0), call);
    expect(result.newEntity).toBeUndefined();
    expect(result.records).toEqual([]);
    expect(result.error).toBeInstanceOf(Error);
    // Never reached the spawn.
    expect(calls.some((c) => methodOf(c) === 'entity.spawn')).toBe(false);
  });

  it('captures a rejection (e.g. describe fails) into error', async () => {
    const boom = new Error('describe rejected');
    const call = mockCall([], { 'engine.describe': () => { throw boom; } });
    const result = await duplicateEntity(E(1, 0), call);
    expect(result.error).toBe(boom);
    expect(result.records).toEqual([]);
  });
});

// --- destroyEntity -----------------------------------------------------------------------------

describe('destroyEntity', () => {
  it('sends one cascading entity.destroy and returns its record', async () => {
    const calls: Command[] = [];
    const call = mockCall(calls, {
      'entity.destroy': {
        kind: 'entity_destroyed',
        components: { Name: 'x' },
        descendants: [{ entity: E(10, 0), components: { Parent: E(4, 0) } }],
      } as ResponseBody,
    });
    const result = await destroyEntity(E(4, 0), call);
    expect(result.error).toBeUndefined();
    expect(calls.map(methodOf)).toEqual(['entity.destroy']);
    expect(result.records).toHaveLength(1);
    expect(paramsOf(result.records[0]!.command).entity).toEqual(E(4, 0));
  });

  it('captures a rejection into error with no records', async () => {
    const boom = new Error('destroy rejected');
    const call = mockCall([], { 'entity.destroy': () => { throw boom; } });
    const result = await destroyEntity(E(4, 0), call);
    expect(result.error).toBe(boom);
    expect(result.records).toEqual([]);
  });
});
