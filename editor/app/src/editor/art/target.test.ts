import { describe, expect, it } from 'vitest';
import type { ResponseBody } from '@ludion/protocol';
import { GI_VOLUME } from '../light/giFields';
import {
  ENVIRONMENT,
  parseTargetCandidates,
  pickTarget,
  POST_PROCESS,
  targetInclude,
  targetValue,
  type TargetCandidate,
} from './target';

const at = (index: number, components: Record<string, unknown> = {}, generation = 0): TargetCandidate => ({
  entity: { index, generation },
  components,
});

describe('pickTarget', () => {
  it('prefers the Environment carrier over a lower EntityId', () => {
    const target = pickTarget([at(0), at(3, { [ENVIRONMENT]: { ambient: [0, 0, 0] } }), at(7)], POST_PROCESS);
    expect(target).toEqual({
      entity: { index: 3, generation: 0 },
      reason: 'environment',
      authored: false,
      hasEnvironment: true,
    });
  });

  it('falls back to the lowest EntityId when no entity carries Environment', () => {
    const target = pickTarget([at(9), at(2), at(5)], POST_PROCESS);
    expect(target?.entity).toEqual({ index: 2, generation: 0 });
    expect(target?.reason).toBe('lowest-entity-id');
    expect(target?.hasEnvironment).toBe(false);
  });

  it('takes the lowest Environment carrier when several exist — the entity extract reads', () => {
    const target = pickTarget([at(8, { [ENVIRONMENT]: {} }), at(4, { [ENVIRONMENT]: {} }), at(1)], POST_PROCESS);
    expect(target?.entity).toEqual({ index: 4, generation: 0 });
    expect(target?.reason).toBe('environment');
  });

  it('reports whether the component is already authored on the chosen entity', () => {
    expect(pickTarget([at(0, { [POST_PROCESS]: { exposure: 1.3 } })], POST_PROCESS)?.authored).toBe(true);
    expect(pickTarget([at(0)], POST_PROCESS)?.authored).toBe(false);
    // The component is on a different entity: the target still has none, so the first edit inserts.
    const split = pickTarget([at(2, { [ENVIRONMENT]: {} }), at(0, { [POST_PROCESS]: {} })], POST_PROCESS);
    expect(split?.entity).toEqual({ index: 2, generation: 0 });
    expect(split?.authored).toBe(false);
  });

  it('answers per component: the same world is authored for one tab and not the other', () => {
    // `gi-interior.yaml`'s shape — `GiVolume` beside `Environment` on the environment entity, and no
    // `PostProcess` anywhere.
    const world = [at(0), at(5, { [ENVIRONMENT]: {}, [GI_VOLUME]: { bounce: 1.0 } })];
    expect(pickTarget(world, GI_VOLUME)).toEqual({
      entity: { index: 5, generation: 0 },
      reason: 'environment',
      authored: true,
      hasEnvironment: true,
    });
    expect(pickTarget(world, POST_PROCESS)?.authored).toBe(false);
  });

  it('returns null for an empty world rather than inventing an entity', () => {
    expect(pickTarget([], POST_PROCESS)).toBeNull();
    expect(pickTarget([], GI_VOLUME)).toBeNull();
  });

  it('orders by slot first and only then by generation', () => {
    expect(pickTarget([at(1, {}, 0), at(0, {}, 9)], POST_PROCESS)?.entity).toEqual({ index: 0, generation: 9 });
    expect(pickTarget([at(4, {}, 2), at(4, {}, 1)], POST_PROCESS)?.entity).toEqual({ index: 4, generation: 1 });
  });
});

describe('targetInclude', () => {
  it('asks for the host singleton beside the edited component, so one round trip decides the target', () => {
    expect(targetInclude(POST_PROCESS)).toEqual(['Environment', 'PostProcess']);
    expect(targetInclude(GI_VOLUME)).toEqual(['Environment', 'GiVolume']);
  });
});

describe('parseTargetCandidates', () => {
  it('reads a query_result body into candidates', () => {
    const body = {
      kind: 'query_result',
      entities: [
        { entity: { index: 0, generation: 0 }, components: {} },
        { entity: { index: 1, generation: 2 }, components: { Environment: { sky_horizon: [0.1, 0.1, 0.2] } } },
      ],
    } as unknown as ResponseBody;
    const candidates = parseTargetCandidates(body);
    expect(candidates).toHaveLength(2);
    expect(pickTarget(candidates, POST_PROCESS)?.entity).toEqual({ index: 1, generation: 2 });
  });

  it('is defensive against missing and garbage shapes', () => {
    expect(parseTargetCandidates({ kind: 'ok' } as unknown as ResponseBody)).toEqual([]);
    expect(parseTargetCandidates({ kind: 'query_result' } as unknown as ResponseBody)).toEqual([]);
    const mixed = parseTargetCandidates({
      kind: 'query_result',
      entities: ['garbage', { entity: { index: 'x', generation: 0 } }, { entity: { index: 3, generation: 0 } }],
    } as unknown as ResponseBody);
    expect(mixed).toEqual([{ entity: { index: 3, generation: 0 }, components: {} }]);
  });
});

describe('targetValue', () => {
  it('returns the authored component, or an empty value when the entity carries none', () => {
    expect(targetValue(at(0, { [POST_PROCESS]: { exposure: 0.9 } }), POST_PROCESS)).toEqual({ exposure: 0.9 });
    expect(targetValue(at(0), POST_PROCESS)).toEqual({});
    expect(targetValue(at(0, { [POST_PROCESS]: 'not a component' }), POST_PROCESS)).toEqual({});
    expect(targetValue(at(0, { [GI_VOLUME]: { cell_size: 0.25 } }), GI_VOLUME)).toEqual({ cell_size: 0.25 });
    // Reading the other tab's component off the same entity finds nothing rather than the wrong value.
    expect(targetValue(at(0, { [GI_VOLUME]: { cell_size: 0.25 } }), POST_PROCESS)).toEqual({});
  });
});
