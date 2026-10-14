import type { ResponseBody } from '@ludion/protocol';
import { describe, expect, it } from 'vitest';
import { EMPTY_OVERRIDES, type HierarchyRow } from './hierarchy';
import { OVERRIDES_INTERVAL_MS, canRevertPointer, parseOverrides, shouldRefresh } from './prefab-overrides';

const E = (index: number) => ({ index, generation: 0 });
const part = (index: number, root: number, partPath: string): HierarchyRow => ({
  entity: E(index),
  name: null,
  parent: null,
  prefab: { source: 'a.prefab.yaml', part: partPath, root: E(root) },
});
const body = (instances: unknown[]): ResponseBody => ({ kind: 'prefab_overrides', instances }) as unknown as ResponseBody;
const inst = (root: number, extra: Record<string, unknown> = {}) => ({
  root: E(root),
  prefab: 'a.prefab.yaml',
  name: 'i',
  placement_differs: false,
  overrides: {},
  removed: [],
  root_parts: [],
  ...extra,
});

describe('parseOverrides', () => {
  it('keys pointers by part entity and drops empty lists', () => {
    const rows = [part(1, 1, 'a'), part(2, 1, 'b')];
    const view = parseOverrides(body([inst(1, { overrides: { b: ['/Transform/translation', '/Name'], a: [] } })]), rows);
    expect([...view.parts]).toEqual([['2:0', ['/Transform/translation', '/Name']]]);
    expect(view.instances.get('1:0')).toEqual({ placementDiffers: false, removed: [], rootParts: [], diagnostic: null });
  });

  it('reads the prefab’s root parts', () => {
    const view = parseOverrides(body([inst(1, { root_parts: ['base', 'barrel/tube'] })]), [part(1, 1, 'base')]);
    expect(view.instances.get('1:0')?.rootParts).toEqual(['base', 'barrel/tube']);
  });

  it('matches nested part paths', () => {
    const rows = [part(1, 1, 'turret'), part(2, 1, 'turret/barrel')];
    const view = parseOverrides(body([inst(1, { overrides: { 'turret/barrel': ['/Transform'] } })]), rows);
    expect([...view.parts.keys()]).toEqual(['2:0']);
  });

  it('keeps two instances of one prefab apart by root', () => {
    const rows = [part(1, 1, 'a'), part(2, 2, 'a')];
    const view = parseOverrides(body([inst(1), inst(2, { overrides: { a: ['/Name'] } })]), rows);
    expect([...view.parts.keys()]).toEqual(['2:0']);
    expect(view.instances.size).toBe(2);
  });

  it('ignores a part path with no live row', () => {
    const view = parseOverrides(body([inst(1, { overrides: { gone: ['/Name'] } })]), [part(1, 1, 'a')]);
    expect(view.parts.size).toBe(0);
  });

  it('reads status: placement, removed and a diagnostic', () => {
    const rows = [part(1, 1, 'a')];
    const view = parseOverrides(
      body([inst(1, { placement_differs: true, removed: ['b'], diagnostic: { code: 'L1234', message: 'override on a vanished part', related: [] } })]),
      rows,
    );
    expect(view.instances.get('1:0')).toEqual({
      placementDiffers: true,
      removed: ['b'],
      rootParts: [],
      diagnostic: { code: 'L1234', message: 'override on a vanished part' },
    });
  });

  it('reads a malformed body as empty and skips malformed instances', () => {
    expect(parseOverrides({ kind: 'query_result' } as unknown as ResponseBody, [])).toBe(EMPTY_OVERRIDES);
    expect(parseOverrides({ kind: 'prefab_overrides', instances: 7 } as unknown as ResponseBody, [])).toBe(EMPTY_OVERRIDES);
    const view = parseOverrides(body([null, 'x', { root: 'nope' }, inst(1)]), [part(1, 1, 'a')]);
    expect([...view.instances.keys()]).toEqual(['1:0']);
  });
});

describe('canRevertPointer', () => {
  const root = part(1, 1, 'body');
  const lid = part(2, 1, 'body/lid');
  const second = part(3, 1, 'wheel'); // a second root part of a multi-root prefab
  const rows = [root, lid, second];
  const view = parseOverrides(body([inst(1, { root_parts: ['body', 'wheel'] })]), rows);

  it('refuses a root part’s Transform and Parent pointers', () => {
    expect(canRevertPointer('/Transform/translation', root, view)).toBe(false);
    expect(canRevertPointer('/Transform', second, view)).toBe(false);
    expect(canRevertPointer('/Parent', root, view)).toBe(false);
  });

  it('still refuses a root part reparented under a sibling', () => {
    expect(canRevertPointer('/Transform', { ...second, parent: E(1) }, view)).toBe(false);
  });

  it('offers a non-root part dragged to the top level', () => {
    expect(canRevertPointer('/Transform/translation', { ...lid, parent: null }, view)).toBe(true);
  });

  it('offers other root fields and every nested-part field', () => {
    expect(canRevertPointer('/Sprite/tint', root, view)).toBe(true);
    expect(canRevertPointer('/TransformX', root, view)).toBe(true);
    expect(canRevertPointer('/Transform/translation', { ...lid, parent: E(1) }, view)).toBe(true);
  });
});

describe('shouldRefresh', () => {
  it('runs the first time and after the interval', () => {
    expect(shouldRefresh(0, null, false, false)).toBe(true);
    expect(shouldRefresh(OVERRIDES_INTERVAL_MS - 1, 0, false, false)).toBe(false);
    expect(shouldRefresh(OVERRIDES_INTERVAL_MS, 0, false, false)).toBe(true);
  });

  it('a forced refresh skips the throttle', () => {
    expect(shouldRefresh(5, 0, false, true)).toBe(true);
  });

  it('never overlaps an in-flight refresh, forced or not', () => {
    expect(shouldRefresh(10_000, 0, true, false)).toBe(false);
    expect(shouldRefresh(10_000, 0, true, true)).toBe(false);
  });
});
