import type { Command, ResponseBody } from '@ludion/protocol';
import { describe, expect, it } from 'vitest';
import type { EntityId } from './undo';
import { mutationToEntries, mutationToEntry, reparentEntries, type MutationRecord } from './mutations';

// --- helpers -----------------------------------------------------------------------------------

const E = (index: number, generation: number): EntityId => ({ index, generation });

/** Builds a `MutationRecord` from a raw command/body pair, past the generated types' index sigs. */
function record(command: unknown, body: unknown): MutationRecord {
  return { command: command as Command, body: body as ResponseBody };
}

// --- component.set -----------------------------------------------------------------------------

describe('mutationToEntry: component.set', () => {
  it('maps an overwrite (previous present) to a set carrying before/after', () => {
    const entry = mutationToEntry(
      record(
        { method: 'component.set', params: { entity: E(1, 0), component: 'Sprite', value: { tint: 2 } } },
        { kind: 'component_set', previous: { tint: 1 } },
      ),
    );
    expect(entry).toEqual({ kind: 'set', entity: E(1, 0), component: 'Sprite', before: { tint: 1 }, after: { tint: 2 } });
  });

  it('maps a fresh insert (previous absent) to a set with before === null', () => {
    const entry = mutationToEntry(
      record(
        { method: 'component.set', params: { entity: E(3, 1), component: 'Light', value: { lux: 9 } } },
        { kind: 'component_set' },
      ),
    );
    expect(entry).toEqual({ kind: 'set', entity: E(3, 1), component: 'Light', before: null, after: { lux: 9 } });
  });

  it('treats an explicit null previous as a fresh insert', () => {
    const entry = mutationToEntry(
      record(
        { method: 'component.set', params: { entity: E(3, 1), component: 'Light', value: { lux: 9 } } },
        { kind: 'component_set', previous: null },
      ),
    );
    expect(entry?.kind).toBe('set');
    expect((entry as { before: unknown }).before).toBeNull();
  });

  it('returns null when the body kind does not match the command', () => {
    const entry = mutationToEntry(
      record(
        { method: 'component.set', params: { entity: E(1, 0), component: 'Sprite', value: { tint: 2 } } },
        { kind: 'ack' },
      ),
    );
    expect(entry).toBeNull();
  });

  it('returns null when the entity handle is malformed', () => {
    const entry = mutationToEntry(
      record(
        { method: 'component.set', params: { entity: { index: 'x' }, component: 'Sprite', value: {} } },
        { kind: 'component_set', previous: {} },
      ),
    );
    expect(entry).toBeNull();
  });

  it('returns null when the component name is missing', () => {
    const entry = mutationToEntry(
      record(
        { method: 'component.set', params: { entity: E(1, 0), value: {} } },
        { kind: 'component_set', previous: {} },
      ),
    );
    expect(entry).toBeNull();
  });
});

// --- component.remove --------------------------------------------------------------------------

describe('mutationToEntry: component.remove', () => {
  it('maps to a remove carrying the previous value', () => {
    const entry = mutationToEntry(
      record(
        { method: 'component.remove', params: { entity: E(2, 4), component: 'Health' } },
        { kind: 'component_removed', previous: { hp: 100 } },
      ),
    );
    expect(entry).toEqual({ kind: 'remove', entity: E(2, 4), component: 'Health', before: { hp: 100 } });
  });

  it('returns null on a mismatched body kind', () => {
    const entry = mutationToEntry(
      record({ method: 'component.remove', params: { entity: E(2, 4), component: 'Health' } }, { kind: 'ack' }),
    );
    expect(entry).toBeNull();
  });
});

// --- entity.destroy ----------------------------------------------------------------------------

describe('mutationToEntry: entity.destroy', () => {
  it('maps to a destroy carrying the captured component map', () => {
    const components = { Name: { name: 'brick' }, Transform: { position: [1, 2, 3] } };
    const entry = mutationToEntry(
      record({ method: 'entity.destroy', params: { entity: E(5, 0) } }, { kind: 'entity_destroyed', components }),
    );
    expect(entry).toEqual({ kind: 'destroy', entity: E(5, 0), components });
  });

  it('defaults a missing components map to {}', () => {
    const entry = mutationToEntry(
      record({ method: 'entity.destroy', params: { entity: E(5, 0) } }, { kind: 'entity_destroyed' }),
    );
    expect(entry).toEqual({ kind: 'destroy', entity: E(5, 0), components: {} });
  });

  it('returns null when the entity handle is malformed', () => {
    const entry = mutationToEntry(
      record({ method: 'entity.destroy', params: { entity: null } }, { kind: 'entity_destroyed', components: {} }),
    );
    expect(entry).toBeNull();
  });
});

// --- entity.spawn ------------------------------------------------------------------------------

describe('mutationToEntry: entity.spawn', () => {
  it('takes the fresh id from the response and components from the request', () => {
    const components = { Sprite: { tint: 1 } };
    const entry = mutationToEntry(
      record({ method: 'entity.spawn', params: { components } }, { kind: 'spawned', entity: E(9, 2) }),
    );
    expect(entry).toEqual({ kind: 'spawn', entity: E(9, 2), components });
  });

  it('defaults missing request components to {}', () => {
    const entry = mutationToEntry(record({ method: 'entity.spawn', params: {} }, { kind: 'spawned', entity: E(9, 2) }));
    expect(entry).toEqual({ kind: 'spawn', entity: E(9, 2), components: {} });
  });

  it('returns null when the response carries no entity', () => {
    const entry = mutationToEntry(record({ method: 'entity.spawn', params: {} }, { kind: 'spawned' }));
    expect(entry).toBeNull();
  });

  it('folds the name param into the components so a redo respawns it named', () => {
    const entry = mutationToEntry(
      record(
        { method: 'entity.spawn', params: { name: 'Sprite', components: { Sprite: { tint: 1 } } } },
        { kind: 'spawned', entity: E(9, 2) },
      ),
    );
    expect(entry).toEqual({ kind: 'spawn', entity: E(9, 2), components: { Sprite: { tint: 1 }, Name: 'Sprite' } });
  });
});

// --- unrecognized shapes -----------------------------------------------------------------------

describe('mutationToEntry: unrecognized shapes', () => {
  it('returns null for a non-mutating method', () => {
    expect(mutationToEntry(record({ method: 'component.get', params: { entity: E(1, 0), component: 'x' } }, { kind: 'component_value', value: {} }))).toBeNull();
  });

  it('returns null for a missing method', () => {
    expect(mutationToEntry(record({ params: {} }, { kind: 'ack' }))).toBeNull();
  });

  it('returns null when params are absent on a set', () => {
    expect(mutationToEntry(record({ method: 'component.set' }, { kind: 'component_set' }))).toBeNull();
  });
});

describe('mutationToEntries: entity.destroy cascade', () => {
  const destroyed = (descendants?: unknown) =>
    record(
      { method: 'entity.destroy', params: { entity: E(5, 0) } },
      { kind: 'entity_destroyed', components: { Name: 'p' }, descendants },
    );

  it('lists descendants leaves-first, the victim last, so undo respawns parent-first', () => {
    const entries = mutationToEntries(
      destroyed([
        { entity: E(6, 0), components: { Parent: E(5, 0) } },
        { entity: E(7, 0), components: { Parent: E(6, 0) } },
      ]),
    );
    expect(entries.map((e) => e.entity)).toEqual([E(7, 0), E(6, 0), E(5, 0)]);
    expect(entries[0]).toEqual({ kind: 'destroy', entity: E(7, 0), components: { Parent: E(6, 0) } });
  });

  it('is the lone victim entry when there are no descendants', () => {
    expect(mutationToEntries(destroyed([])).map((e) => e.entity)).toEqual([E(5, 0)]);
    expect(mutationToEntries(destroyed()).map((e) => e.entity)).toEqual([E(5, 0)]);
  });

  it('passes other mutations through as one entry and unknown ones as none', () => {
    const set = record(
      { method: 'component.set', params: { entity: E(1, 0), component: 'Health', value: 3 } },
      { kind: 'component_set', previous: 2 },
    );
    expect(mutationToEntries(set)).toHaveLength(1);
    expect(mutationToEntries(record({ method: 'scene.save', params: {} }, { kind: 'ack' }))).toEqual([]);
  });
});

// --- entity.reparent -----------------------------------------------------------------------------

describe('reparentEntries', () => {
  const cmd = (parent: unknown) => ({ method: 'entity.reparent', params: { entity: E(1, 0), parent, keep: 'world' } });
  const body = (previousParent: unknown, previousTransform: unknown) => ({
    kind: 'reparented',
    previous_parent: previousParent,
    previous_transform: previousTransform,
  });

  it('yields [reparent, set Transform] carrying the old and the read-back local bits', () => {
    const entries = reparentEntries(record(cmd(E(2, 0)), body(null, { t: 'old' })), { t: 'new' });
    expect(entries).toEqual([
      { kind: 'reparent', entity: E(1, 0), before: null, after: E(2, 0) },
      { kind: 'set', entity: E(1, 0), component: 'Transform', before: { t: 'old' }, after: { t: 'new' } },
    ]);
  });

  it('reads an absent or null parent param as the root, and the previous parent off the body', () => {
    const entries = reparentEntries(record(cmd(null), body(E(3, 1), { t: 'old' })), { t: 'new' });
    expect(entries[0]).toEqual({ kind: 'reparent', entity: E(1, 0), before: E(3, 1), after: null });
    const absent = reparentEntries(
      record({ method: 'entity.reparent', params: { entity: E(1, 0), keep: 'world' } }, body(null, { t: 1 })),
      { t: 2 },
    );
    expect(absent[0]).toEqual({ kind: 'reparent', entity: E(1, 0), before: null, after: null });
  });

  it('omits the Transform set when the entity had no Transform', () => {
    expect(reparentEntries(record(cmd(E(2, 0)), body(null, null)), null)).toEqual([
      { kind: 'reparent', entity: E(1, 0), before: null, after: E(2, 0) },
    ]);
  });

  it('yields nothing for a malformed shape', () => {
    expect(reparentEntries(record(cmd(E(2, 0)), { kind: 'ack' }), null)).toEqual([]);
    expect(reparentEntries(record({ method: 'component.set', params: {} }, body(null, null)), null)).toEqual([]);
    expect(reparentEntries(record(cmd('garbage'), body(null, null)), null)).toEqual([]);
    expect(reparentEntries(record(cmd(E(2, 0)), body('garbage', null)), null)).toEqual([]);
    expect(reparentEntries(record({ method: 'entity.reparent', params: { entity: 'x' } }, body(null, null)), null)).toEqual([]);
  });
});
