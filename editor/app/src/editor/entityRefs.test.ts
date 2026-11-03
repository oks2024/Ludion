import { describe, expect, it } from 'vitest';
import { entityRefIndex, entityRefSuffixes, loadEntityRefs, remapRefsAt } from './entityRefs';
import type { CallFn, EntityId } from './undo';

const MARK = { 'x-entity-ref': {} };
const E = (index: number, generation: number): EntityId => ({ index, generation });
const ref = (e: EntityId) => ({ index: e.index, generation: e.generation });

describe('entityRefSuffixes', () => {
  it('finds a marked root as the empty suffix', () => {
    expect(entityRefSuffixes({ type: 'object', ...MARK })).toEqual(['']);
  });

  it('finds nested marked fields, sorted, whether or not they are required', () => {
    const schema = {
      type: 'object',
      properties: { b: { ...MARK }, a: { ...MARK }, n: { type: 'number' } },
      required: ['a'],
    };
    expect(entityRefSuffixes(schema)).toEqual(['/a', '/b']);
  });

  it('finds a ref behind a null branch and behind a type array', () => {
    const anyOf = { properties: { target: { anyOf: [{ ...MARK }, { type: 'null' }] } } };
    const typed = { properties: { target: { type: ['object', 'null'], ...MARK } } };
    expect(entityRefSuffixes(anyOf)).toEqual(['/target']);
    expect(entityRefSuffixes(typed)).toEqual(['/target']);
  });

  it('follows $ref into $defs, decoding pointer escapes', () => {
    const schema = {
      properties: { root: { $ref: '#/$defs/Ref' }, odd: { $ref: '#/$defs/a~1b~0c' } },
      $defs: { Ref: { ...MARK }, 'a/b~c': { ...MARK } },
    };
    expect(entityRefSuffixes(schema)).toEqual(['/odd', '/root']);
  });

  it('dedupes the same field in several oneOf variants', () => {
    const schema = {
      oneOf: [
        { properties: { who: { ...MARK } } },
        { properties: { who: { ...MARK }, other: { type: 'string' } } },
      ],
    };
    expect(entityRefSuffixes(schema)).toEqual(['/who']);
  });

  it('does not walk arrays', () => {
    expect(entityRefSuffixes({ properties: { list: { type: 'array', items: { ...MARK } } } })).toEqual([]);
  });

  it('stops at the depth cap', () => {
    let schema: unknown = { ...MARK };
    for (let i = 0; i < 14; i++) schema = { properties: { x: schema } };
    expect(entityRefSuffixes(schema)).toEqual([]);
  });

  it('matches the engine guard list for the built-in shapes', () => {
    const parent = { $ref: '#/$defs/EntityRef', $defs: { EntityRef: { ...MARK } } };
    const prefab = { properties: { root: { $ref: '#/$defs/EntityRef' } }, $defs: { EntityRef: { ...MARK } } };
    const joint = { properties: { a: { ...MARK }, b: { ...MARK }, stiffness: { type: 'number' } } };
    expect(entityRefSuffixes(parent)).toEqual(['']);
    expect(entityRefSuffixes(prefab)).toEqual(['/root']);
    expect(entityRefSuffixes(joint)).toEqual(['/a', '/b']);
    const player = { properties: { target: { type: ['object', 'null'], ...MARK } } };
    const volley = { properties: { source: { anyOf: [{ ...MARK }, { type: 'null' }] } } };
    expect(entityRefSuffixes(player)).toEqual(['/target']);
    expect(entityRefSuffixes(volley)).toEqual(['/source']);
  });
});

describe('entityRefIndex', () => {
  it('keeps only components with a ref field and tolerates odd shapes', () => {
    const index = entityRefIndex({
      components: {
        Volley: { properties: { source: { anyOf: [{ ...MARK }, { type: 'null' }] } } },
        Sprite: { properties: { w: { type: 'number' } } },
      },
    });
    expect([...index]).toEqual([['Volley', ['/source']]]);
    expect(entityRefIndex(null).size).toBe(0);
    expect(entityRefIndex({ components: 3 }).size).toBe(0);
  });
});

describe('remapRefsAt', () => {
  const from = E(1, 0);
  const to = E(5, 2);

  it('rewrites a matching ref and copies only the changed path', () => {
    const keep = { x: 1 };
    const value = { target: ref(from), keep };
    const out = remapRefsAt(value, ['/target'], from, to) as typeof value;
    expect(out.target).toEqual(ref(to));
    expect(out.keep).toBe(keep);
    expect(value.target).toEqual(ref(from));
  });

  it('rewrites a root-suffix value', () => {
    expect(remapRefsAt(ref(from), [''], from, to)).toEqual(ref(to));
  });

  it('returns the same object when nothing matches, on index or generation', () => {
    const value = { target: ref(E(1, 1)), other: null };
    expect(remapRefsAt(value, ['/target', '/other', '/missing'], from, to)).toBe(value);
  });

  it('escapes keys holding / or ~ and remaps through them', () => {
    const schema = { properties: { 'a/b': { ...MARK }, 'c~d': { ...MARK } } };
    const suffixes = entityRefSuffixes(schema);
    expect(suffixes).toEqual(['/a~1b', '/c~0d']);
    const value = { 'a/b': ref(from), 'c~d': ref(from) };
    expect(remapRefsAt(value, suffixes, from, to)).toEqual({ 'a/b': ref(to), 'c~d': ref(to) });
  });

  it('leaves arrays and non-objects alone', () => {
    const value = { list: [ref(from)] };
    expect(remapRefsAt(value, ['/list'], from, to)).toBe(value);
    expect(remapRefsAt(7, ['/a'], from, to)).toBe(7);
  });
});

describe('loadEntityRefs', () => {
  it('builds the index from engine.describe', async () => {
    const call: CallFn = async () =>
      ({ kind: 'engine_description', description: { components: { Parent: { ...MARK } } } }) as never;
    expect([...(await loadEntityRefs(call))]).toEqual([['Parent', ['']]]);
  });

  it('names an unexpected reply', async () => {
    const call: CallFn = async () => ({ kind: 'ack' }) as never;
    await expect(loadEntityRefs(call)).rejects.toThrow(/ack/);
  });
});
