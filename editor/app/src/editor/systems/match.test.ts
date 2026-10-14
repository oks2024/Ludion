import { describe, expect, it } from 'vitest';
import {
  fnNameOf,
  isUndeclared,
  moduleOf,
  parseSystems,
  partitionBySelection,
  systemMatchesEntity,
  type SystemDesc,
} from './match';

const sys = (name: string, fnPath: string, queries: SystemDesc['queries'], order = 0): SystemDesc => ({
  name,
  order,
  fnPath,
  registeredAt: null,
  queries,
});

const q = (...names: string[]): SystemDesc['queries'][number] => ({
  components: names.map((name) => ({ name, access: 'read' })),
});

describe('parseSystems', () => {
  it('reads the systems array off a describe body, mapping wire field names', () => {
    const body = {
      kind: 'engine_description',
      description: {
        components: { Transform: {} },
        systems: [
          {
            name: 'move_player',
            order: 0,
            fn_path: 'vigil::movement::move_player',
            registered_at: { file: 'games/vigil/src/lib.rs', line: 42 },
            queries: [{ components: [{ name: 'Transform', access: 'write' }, { name: 'Player', access: 'read' }] }],
          },
          { name: 'tick_clock', order: 1, fn_path: 'vigil::tick_clock', registered_at: { file: 'x', line: 3 }, queries: [] },
        ],
      },
    };
    const systems = parseSystems(body);
    expect(systems).toHaveLength(2);
    expect(systems[0]).toEqual({
      name: 'move_player',
      order: 0,
      fnPath: 'vigil::movement::move_player',
      registeredAt: { file: 'games/vigil/src/lib.rs', line: 42 },
      queries: [{ components: [{ name: 'Transform', access: 'write' }, { name: 'Player', access: 'read' }] }],
    });
    expect(systems[1].queries).toEqual([]);
    expect(systems[1].registeredAt).toEqual({ file: 'x', line: 3 });
  });

  it('is defensive against missing/garbage shapes', () => {
    expect(parseSystems(null)).toEqual([]);
    expect(parseSystems({})).toEqual([]);
    expect(parseSystems({ description: {} })).toEqual([]);
    expect(parseSystems({ description: { systems: 'nope' } })).toEqual([]);
    // A system missing name or fn_path is dropped; a query entry missing a name is skipped.
    const systems = parseSystems({
      description: {
        systems: [
          { name: 'ok', fn_path: 'c::ok', queries: [{ components: [{ name: 'A' }, { access: 'read' }] }] },
          { name: 'no_fn' },
          'garbage',
        ],
      },
    });
    expect(systems).toHaveLength(1);
    expect(systems[0].queries[0].components).toEqual([{ name: 'A', access: 'read' }]);
  });
});

describe('systemMatchesEntity', () => {
  it('matches when a query has all its components on the entity (access ignored)', () => {
    const s = sys('move', 'c::move', [q('Transform', 'Player')]);
    expect(systemMatchesEntity(s, ['Transform', 'Player', 'Sprite'])).toBe(true);
  });

  it('does not match when any component of every query is missing', () => {
    const s = sys('move', 'c::move', [q('Transform', 'Player')]);
    expect(systemMatchesEntity(s, ['Transform', 'Sprite'])).toBe(false);
  });

  it('matches if ANY one of several queries is fully satisfied', () => {
    const s = sys('multi', 'c::multi', [q('Enemy', 'Health'), q('Transform')]);
    expect(systemMatchesEntity(s, ['Transform'])).toBe(true);
  });

  it('never matches an undeclared system (queries: [])', () => {
    const s = sys('undeclared', 'c::undeclared', []);
    expect(systemMatchesEntity(s, ['Transform', 'Player'])).toBe(false);
    expect(isUndeclared(s)).toBe(true);
  });

  it('never matches an empty-component query', () => {
    const s = sys('empty', 'c::empty', [{ components: [] }]);
    expect(systemMatchesEntity(s, ['Transform'])).toBe(false);
  });
});

describe('partitionBySelection', () => {
  const schedule = [
    sys('a', 'c::a', [q('Transform')], 0),
    sys('b', 'c::b', [q('Enemy')], 1),
    sys('c', 'c::c', [], 2),
  ];

  it('puts everything in rest (execution order) with no selection', () => {
    const { matching, rest } = partitionBySelection(schedule, null);
    expect(matching).toEqual([]);
    expect(rest.map((s) => s.name)).toEqual(['a', 'b', 'c']);
  });

  it('floats matching systems out, keeping order in each group', () => {
    const { matching, rest } = partitionBySelection(schedule, ['Transform', 'Sprite']);
    expect(matching.map((s) => s.name)).toEqual(['a']);
    expect(rest.map((s) => s.name)).toEqual(['b', 'c']);
  });
});

describe('fn path helpers', () => {
  it('fnNameOf strips generics and takes the tail', () => {
    expect(fnNameOf('vigil::movement::move_player')).toBe('move_player');
    expect(fnNameOf('vigil::movement::run<T>')).toBe('run');
    expect(fnNameOf('vigil::tick')).toBe('tick');
  });

  it('moduleOf returns the fn owning module, or empty at crate root', () => {
    expect(moduleOf('vigil::movement::move_player')).toBe('movement');
    expect(moduleOf('vigil::tick')).toBe('');
  });
});
