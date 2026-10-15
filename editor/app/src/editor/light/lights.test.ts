import { describe, expect, it } from 'vitest';
import type { ResponseBody } from '@ludion/protocol';
import {
  DIRECTIONAL_LIGHT,
  GI_DYNAMIC,
  lightInclude,
  lightLabel,
  offersGiDynamicToggle,
  parseLightEntries,
  pickSun,
  POINT_LIGHT,
  SPOT_FEEDS_NO_GI,
  SPOT_LIGHT,
  SUN_ALWAYS_BAKES,
} from './lights';

/** A `query.entities` reply as the engine shapes it: matches carrying only the included components,
 *  with absent ones omitted from the map entirely. */
const reply = (
  entities: readonly { index: number; generation: number; components?: Record<string, unknown> }[],
): ResponseBody =>
  ({
    kind: 'query_result',
    entities: entities.map((e) => ({
      entity: { index: e.index, generation: e.generation },
      components: e.components ?? {},
    })),
  }) as unknown as ResponseBody;

describe('parseLightEntries', () => {
  it('reads the handle and the name of every match', () => {
    const entries = parseLightEntries(
      reply([
        { index: 4, generation: 1, components: { Name: 'torch-north' } },
        { index: 9, generation: 2, components: { PointLight: { intensity: 2 } } },
      ]),
    );
    expect(entries).toEqual([
      { entity: { index: 4, generation: 1 }, name: 'torch-north', dynamic: false, spot: false },
      { entity: { index: 9, generation: 2 }, name: null, dynamic: false, spot: false },
    ]);
  });

  it('merges several replies into one id-ordered list and reads a spot by presence', () => {
    const points = reply([{ index: 8, generation: 0, components: { Name: 'lamp' } }]);
    const cones = reply([{ index: 2, generation: 0, components: { Name: 'beam', SpotLight: {} } }]);
    const entries = parseLightEntries(points, cones);
    expect(entries.map((entry) => [entry.entity.index, entry.spot])).toEqual([
      [2, true],
      [8, false],
    ]);
  });

  it('orders ascending by entity id whatever order the reply arrived in', () => {
    const entries = parseLightEntries(
      reply([
        { index: 12, generation: 1 },
        { index: 3, generation: 4 },
        { index: 3, generation: 1 },
        { index: 7, generation: 1 },
      ]),
    );
    expect(entries.map((entry) => [entry.entity.index, entry.entity.generation])).toEqual([
      [3, 1],
      [3, 4],
      [7, 1],
      [12, 1],
    ]);
  });

  it('reads an empty name as unnamed rather than as a name of no characters', () => {
    const entries = parseLightEntries(reply([{ index: 1, generation: 1, components: { Name: '' } }]));
    expect(entries[0].name).toBeNull();
  });

  it('skips malformed entries and non-query bodies instead of throwing', () => {
    expect(parseLightEntries({ kind: 'component_value', value: {} } as unknown as ResponseBody)).toEqual([]);
    const entries = parseLightEntries({
      kind: 'query_result',
      entities: [null, { entity: { index: 'x' } }, { entity: { index: 2, generation: 0 } }],
    } as unknown as ResponseBody);
    expect(entries).toEqual([{ entity: { index: 2, generation: 0 }, name: null, dynamic: false, spot: false }]);
  });

  it('reads the GiDynamic marker by presence, since the component has no fields', () => {
    const entries = parseLightEntries(
      reply([
        { index: 1, generation: 0, components: { Name: 'lantern', GiDynamic: {} } },
        { index: 2, generation: 0, components: { Name: 'brazier' } },
      ]),
    );
    expect(entries.map((entry) => entry.dynamic)).toEqual([true, false]);
  });

  it('reads a present key as marked whatever its value — the engine omits absent components', () => {
    // `GiDynamic` is field-less, so its value is `{}`; nothing about the marker is readable from the
    // value, and treating anything but presence as the test would invent a third state.
    const entries = parseLightEntries(reply([{ index: 1, generation: 0, components: { GiDynamic: {} } }]));
    expect(entries[0].dynamic).toBe(true);
  });
});

describe('lightInclude', () => {
  it('asks for the name, the marker and the spot component, and never filters on them', () => {
    // `GiDynamic` in the FILTER would drop every unmarked light out of the list — which is most of
    // them, and exactly the ones a human opens this tab to mark.
    expect([...lightInclude()]).toEqual(['Name', GI_DYNAMIC, SPOT_LIGHT]);
  });
});

describe('offersGiDynamicToggle', () => {
  it('offers the toggle on a placed light and never on the sun', () => {
    expect(offersGiDynamicToggle(POINT_LIGHT)).toBe(true);
    expect(offersGiDynamicToggle(DIRECTIONAL_LIGHT)).toBe(false);
  });

  it('never offers the toggle on a spot light, which feeds no GI', () => {
    expect(offersGiDynamicToggle(SPOT_LIGHT)).toBe(false);
    expect(SPOT_FEEDS_NO_GI).toMatch(/no GI/);
  });

  it('carries the reason that takes the toggle\'s place on the sun\'s card', () => {
    expect(SUN_ALWAYS_BAKES).toMatch(/always baked/);
    expect(SUN_ALWAYS_BAKES).toMatch(/PointLight/);
  });
});

describe('pickSun', () => {
  it('takes the lowest entity id, which is the one the renderer lights the scene with', () => {
    const entries = parseLightEntries(
      reply([
        { index: 30, generation: 1, components: { Name: 'moon' } },
        { index: 6, generation: 1, components: { Name: 'sun' } },
      ]),
    );
    expect(pickSun(entries)).toEqual({ index: 6, generation: 1 });
  });

  it('is null for a scene with no directional light — never a spawn', () => {
    expect(pickSun([])).toBeNull();
  });
});

describe('lightLabel', () => {
  it('names a light by its Name, and says so when it has none', () => {
    expect(lightLabel({ entity: { index: 1, generation: 0 }, name: 'hearth', dynamic: false, spot: false })).toBe('hearth');
    expect(lightLabel({ entity: { index: 1, generation: 0 }, name: null, dynamic: true, spot: false })).toBe('unnamed');
  });
});
