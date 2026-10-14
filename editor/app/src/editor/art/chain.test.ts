import { describe, expect, it } from 'vitest';
import {
  bucketSchemaFields,
  deriveLiveEffects,
  effectFields,
  effectiveValue,
  effectiveValues,
  effectRow,
  EFFECT_ORDER,
  FIELDS,
  FIELD_NAMES,
  gateStateKey,
  isComparable,
  neutralPatch,
  PASS_GATING_EFFECTS,
  reconcileWithGraph,
  schemaFieldNames,
  type EffectId,
} from './chain';

/** The `PostProcess` fields the engine declares, in `components.rs`' declaration order. Mirrored here
 *  rather than imported from the table under test: this list is the assertion. */
const ENGINE_FIELDS = [
  'bloom_threshold',
  'bloom_strength',
  'exposure',
  'tonemap',
  'dither',
  'contrast',
  'saturation',
  'temperature',
  'tint',
  'lift',
  'gamma',
  'gain',
  'fog_density',
  'fog_color',
  'fog_start',
  'fog_height',
  'fog_height_falloff',
  'fog_albedo',
  'fog_sky',
  'fog_anisotropy',
  'vignette',
  'vignette_radius',
  'ssao',
  'ssao_radius',
  'ssao_bias',
  'ssao_direct',
  'shafts',
  'dof',
  'focus_distance',
  'chromatic_aberration',
  'halation',
  'grain',
  'grain_size',
];

/** The minimal thirteen-pass graph: no sun, no UI, none of the conditional effects. */
const MINIMAL_PASSES = [
  'scene',
  'bloom_threshold',
  'bloom_down_1',
  'bloom_down_2',
  'bloom_down_3',
  'bloom_down_4',
  'bloom_down_5',
  'bloom_up_4',
  'bloom_up_3',
  'bloom_up_2',
  'bloom_up_1',
  'bloom_up_0',
  'composite',
];

const state = (effects: ReturnType<typeof deriveLiveEffects>, id: EffectId) => {
  const found = effects.find((e) => e.id === id);
  expect(found, `no state for ${id}`).toBeDefined();
  return found as NonNullable<typeof found>;
};

describe('the pinned pipeline order', () => {
  it('is the order composite.wgsl executes, with ambient occlusion ahead of it', () => {
    expect(EFFECT_ORDER.map((row) => row.id)).toEqual([
      'ssao',
      'chromatic_aberration',
      'dof',
      'bloom',
      'halation',
      'fog',
      'shafts',
      'exposure',
      'tonemap',
      'grade',
      'vignette',
      'grain',
      'dither',
    ]);
  });

  it('places the three effects that are not composite steps by their real stage', () => {
    expect(effectRow('ssao').stage).toBe('scene');
    expect(effectRow('chromatic_aberration').stage).toBe('fetch');
    expect(effectRow('dof').stage).toBe('composite');
  });

  it('gates frame-graph passes only for ambient occlusion, light shafts and depth of field', () => {
    expect(PASS_GATING_EFFECTS).toEqual(['ssao', 'dof', 'shafts']);
    expect(effectRow('ssao').gatedPasses).toEqual(['prepass', 'ssao', 'ssao_blur']);
    expect(effectRow('shafts').gatedPasses).toEqual(['shafts', 'shafts_blur']);
    expect(effectRow('dof').gatedPasses).toEqual(['dof_coc', 'dof']);
  });

  it('leaves bloom without gated passes — the pyramid is built whatever its strength says', () => {
    expect(effectRow('bloom').gatedPasses).toEqual([]);
  });
});

describe('the field table', () => {
  it('covers every field PostProcess declares, and nothing it does not', () => {
    expect(FIELD_NAMES).toEqual(ENGINE_FIELDS);
  });

  it('assigns every field to exactly one effect, and every effect owns at least one', () => {
    const owned = EFFECT_ORDER.flatMap((row) => effectFields(row.id));
    expect([...owned].sort()).toEqual([...FIELD_NAMES].sort());
    expect(new Set(owned).size).toBe(owned.length);
    for (const row of EFFECT_ORDER) {
      expect(effectFields(row.id).length, `${row.id} owns no field`).toBeGreaterThan(0);
    }
  });

  it('names an effect for every entry, and only effects the chain renders', () => {
    const ids = new Set(EFFECT_ORDER.map((row) => row.id));
    for (const name of FIELD_NAMES) {
      expect(ids.has(FIELDS[name].effect), `${name} names an unknown effect`).toBe(true);
    }
  });

  it('keeps exposure neutral at 1.0 even though it ships at 1.3', () => {
    expect(FIELDS.exposure.neutral).toBe(1.0);
    expect(FIELDS.exposure.shipped).toBe(1.3);
  });

  it('gives the tonemap no neutral at all rather than a fake one', () => {
    expect(FIELDS.tonemap.neutral).toBeNull();
    expect(isComparable('tonemap')).toBe(false);
    expect(neutralPatch('tonemap')).toEqual({});
  });

  it('makes fog_density the gate and fog_height_falloff a neutral that is not one', () => {
    expect(FIELDS.fog_density.gate).toBe(true);
    expect(FIELDS.fog_height_falloff.neutral).toBe(0);
    expect(FIELDS.fog_height_falloff.gate).toBe(false);
  });

  it('reads the medium: a purely scattering albedo is neutral, the phase asymmetry has none', () => {
    // 1.0 is the engine's shipped albedo *and* its neutral: a purely scattering medium is the
    // analytic fog the composite drew before the albedo existed. It is not fog's gate — the
    // extinction is — so an absorbing medium with no density is still no medium.
    expect(FIELDS.fog_albedo.neutral).toBe(1.0);
    expect(FIELDS.fog_albedo.shipped).toBe(1.0);
    expect(FIELDS.fog_albedo.gate).toBe(false);
    // No value of `g` contributes nothing: 0 is isotropic scattering, a shape rather than an
    // absence, so it is a shaping field like `fog_start`.
    expect(FIELDS.fog_anisotropy.neutral).toBeNull();
    expect(FIELDS.fog_anisotropy.shipped).toBe(0.7);
    // The sky coupling ships *at* its neutral: 0 is the flat authored slab the haze has always been,
    // so an unauthored scene contributes nothing through it. Not a gate — a fully sky-coupled medium
    // with no extinction is still no medium.
    expect(FIELDS.fog_sky.neutral).toBe(0);
    expect(FIELDS.fog_sky.shipped).toBe(0);
    expect(FIELDS.fog_sky.gate).toBe(false);
  });

  it('gives every shaping field a null neutral — its gate is what switches the effect off', () => {
    for (const name of ['bloom_threshold', 'fog_color', 'fog_start', 'fog_anisotropy', 'vignette_radius', 'ssao_radius', 'focus_distance', 'grain_size']) {
      expect(FIELDS[name].neutral, `${name} should have no neutral`).toBeNull();
    }
  });
});

describe('neutralPatch', () => {
  it('collects every neutral an effect has, and never a shaping field', () => {
    expect(neutralPatch('fog')).toEqual({
      fog_density: 0,
      fog_height_falloff: 0,
      fog_albedo: 1.0,
      fog_sky: 0,
    });
    expect(neutralPatch('vignette')).toEqual({ vignette: 0 });
  });

  it('neutralizes the whole grade, triples included', () => {
    expect(neutralPatch('grade')).toEqual({
      contrast: 1.0,
      saturation: 1.0,
      temperature: 0,
      tint: 0,
      lift: [0, 0, 0],
      gamma: [1, 1, 1],
      gain: [1, 1, 1],
    });
  });
});

describe('effective values', () => {
  it('reads an unauthored field as the shipped default the engine renders', () => {
    expect(effectiveValue({}, 'exposure')).toBe(1.3);
    expect(effectiveValue({ exposure: 0.9 }, 'exposure')).toBe(0.9);
    expect(effectiveValue({}, 'not_a_field')).toBeUndefined();
  });

  it('fills every field so an unauthored scene still shows the look it renders', () => {
    const values = effectiveValues({ fog_density: 0.02 });
    expect(Object.keys(values)).toEqual(ENGINE_FIELDS);
    expect(values.fog_density).toBe(0.02);
    expect(values.dither).toBe(true);
  });
});

describe('deriveLiveEffects', () => {
  it('reports shafts dead without a sun even when authored non-zero', () => {
    // A medium is authored, so the only thing missing is the sun.
    const effects = deriveLiveEffects({ shafts: 0.02, fog_density: 0.02 }, { hasSun: false });
    const shafts = state(effects, 'shafts');
    expect(shafts.live).toBe(false);
    expect(shafts.reason).toBe('missing-sun');
    // The distinction the whole module exists for: the scene asks for it, and gets nothing.
    expect(shafts.engaged).toBe(true);
    expect(shafts.authored).toBe(true);
  });

  // The second silent-nothing case, and the one the merged medium introduced: the march is the
  // *directional* in-scatter of the fog's medium, so with no extinction its estimate is exactly zero
  // and `PostChain::for_scene` schedules neither shaft pass. Authored, validated, and contributing
  // nothing — precisely the trap this module exists to name.
  it('reports shafts dead without a medium, sun or no sun', () => {
    const shafts = state(deriveLiveEffects({ shafts: 0.19 }, { hasSun: true }), 'shafts');
    expect(shafts.live).toBe(false);
    expect(shafts.reason).toBe('missing-medium');
    expect(shafts.engaged).toBe(true);
    expect(shafts.authored).toBe(true);
  });

  // Both absent: the sun wins the report, because it is the blocker the Art Director panel cannot fix
  // — `fog_density` is a field on the very row below.
  it('blames the sun first when both the sun and the medium are missing', () => {
    expect(state(deriveLiveEffects({ shafts: 0.19 }, { hasSun: false }), 'shafts').reason).toBe('missing-sun');
  });

  it('brings the same shafts value alive once the scene carries both a sun and a medium', () => {
    const shafts = state(deriveLiveEffects({ shafts: 0.02, fog_density: 0.02 }, { hasSun: true }), 'shafts');
    expect(shafts.live).toBe(true);
    expect(shafts.reason).toBe('live');
  });

  // `requiresEffect` reads the fog row's *engagement*, so it moves with `fog_density` — fog's own gate
  // — and not with a shaping field that leaves the medium empty.
  it('does not accept a shaping fog field as a medium', () => {
    const effects = deriveLiveEffects(
      { shafts: 0.02, fog_start: 8, fog_height_falloff: 0.1, fog_albedo: 0.5 },
      { hasSun: true },
    );
    expect(state(effects, 'shafts').reason).toBe('missing-medium');
  });

  it('does not blame the sun or the medium for shafts nobody asked for', () => {
    const shafts = state(deriveLiveEffects({ shafts: 0, fog_density: 0.02 }, { hasSun: false }), 'shafts');
    expect(shafts.reason).toBe('neutral');
    expect(shafts.engaged).toBe(false);
  });

  it('reads a scene with no PostProcess as the shipped chain: bloom, exposure, tonemap and dither', () => {
    const effects = deriveLiveEffects({}, { hasSun: true });
    const live = effects.filter((e) => e.live).map((e) => e.id);
    expect(live).toEqual(['bloom', 'exposure', 'tonemap', 'dither']);
    expect(effects.every((e) => !e.authored)).toBe(true);
  });

  it('keeps the tonemap live with no gate of its own', () => {
    const tonemap = state(deriveLiveEffects({ tonemap: 'none' }, { hasSun: false }), 'tonemap');
    expect(tonemap.live).toBe(true);
    expect(tonemap.reason).toBe('live');
  });

  it('switches an effect off at its neutral and on one step away from it', () => {
    expect(state(deriveLiveEffects({ exposure: 1.0 }, { hasSun: false }), 'exposure').reason).toBe('neutral');
    expect(state(deriveLiveEffects({ dither: false }, { hasSun: false }), 'dither').reason).toBe('neutral');
    expect(state(deriveLiveEffects({ bloom_strength: 0 }, { hasSun: false }), 'bloom').reason).toBe('neutral');
    expect(state(deriveLiveEffects({ vignette: 0.4 }, { hasSun: false }), 'vignette').live).toBe(true);
  });

  it('lets any one grade knob light the grade row, triples included', () => {
    expect(state(deriveLiveEffects({ gamma: [1, 1, 1] }, { hasSun: false }), 'grade').live).toBe(false);
    expect(state(deriveLiveEffects({ gamma: [1, 1, 0.9] }, { hasSun: false }), 'grade').live).toBe(true);
    expect(state(deriveLiveEffects({ temperature: -0.1 }, { hasSun: false }), 'grade').live).toBe(true);
  });

  it('never reads a shaping field as engaging its effect', () => {
    const effects = deriveLiveEffects(
      { fog_start: 12, fog_height_falloff: 0.1, focus_distance: 4, vignette_radius: 0.2, ssao_radius: 2 },
      { hasSun: true },
    );
    expect(state(effects, 'fog').live).toBe(false);
    expect(state(effects, 'dof').live).toBe(false);
    expect(state(effects, 'vignette').live).toBe(false);
    expect(state(effects, 'ssao').live).toBe(false);
    // …but they are authored, so the UI shows them undimmed on a dead row.
    expect(state(effects, 'fog').authored).toBe(true);
  });

  it('is defensive about a value that is not a component', () => {
    expect(deriveLiveEffects(null as unknown as Record<string, unknown>, { hasSun: false })).toHaveLength(
      EFFECT_ORDER.length,
    );
  });

  it('returns one state per row, in pinned order', () => {
    expect(deriveLiveEffects({}, { hasSun: false }).map((e) => e.id)).toEqual(EFFECT_ORDER.map((r) => r.id));
  });
});

describe('reconcileWithGraph', () => {
  it('agrees with the minimal graph on a scene that enables nothing', () => {
    expect(reconcileWithGraph(deriveLiveEffects({}, { hasSun: false }), MINIMAL_PASSES)).toEqual([]);
  });

  it('agrees when every pass an enabled effect needs is present', () => {
    const effects = deriveLiveEffects({ ssao: 0.6, dof: 0.01 }, { hasSun: false });
    const passes = [...MINIMAL_PASSES, 'prepass', 'ssao', 'ssao_blur', 'dof_coc', 'dof'];
    expect(reconcileWithGraph(effects, passes)).toEqual([]);
  });

  it('agrees that a sunless scene asking for shafts gets none', () => {
    const sunless = deriveLiveEffects({ shafts: 0.5, fog_density: 0.02 }, { hasSun: false });
    expect(reconcileWithGraph(sunless, MINIMAL_PASSES)).toEqual([]);
  });

  it('agrees that a mediumless scene asking for shafts gets none', () => {
    expect(reconcileWithGraph(deriveLiveEffects({ shafts: 0.5 }, { hasSun: true }), MINIMAL_PASSES)).toEqual([]);
  });

  it('agrees once the medium arrives under a sun', () => {
    const live = deriveLiveEffects({ shafts: 0.5, fog_density: 0.02 }, { hasSun: true });
    expect(reconcileWithGraph(live, [...MINIMAL_PASSES, 'shafts', 'shafts_blur'])).toEqual([]);
  });

  it('reports a pass the model expects and the graph does not carry', () => {
    const effects = deriveLiveEffects({ ssao: 0.6 }, { hasSun: false });
    const passes = [...MINIMAL_PASSES, 'prepass', 'ssao'];
    expect(reconcileWithGraph(effects, passes)).toEqual([
      { effect: 'ssao', pass: 'ssao_blur', expected: true, present: false },
    ]);
  });

  it('reports the other direction too — a pass the graph carries and the model does not expect', () => {
    const effects = deriveLiveEffects({ shafts: 0.5 }, { hasSun: false });
    const passes = [...MINIMAL_PASSES, 'shafts', 'shafts_blur'];
    expect(reconcileWithGraph(effects, passes)).toEqual([
      { effect: 'shafts', pass: 'shafts', expected: false, present: true },
      { effect: 'shafts', pass: 'shafts_blur', expected: false, present: true },
    ]);
  });

  it('says nothing about a composite-resident effect, whichever way it is set', () => {
    const effects = deriveLiveEffects({ vignette: 0.5, grain: 0.02 }, { hasSun: false });
    expect(reconcileWithGraph(effects, MINIMAL_PASSES)).toEqual([]);
  });
});

describe('bucketSchemaFields', () => {
  it('lands an unrecognised schema field in Other rather than dropping it', () => {
    const buckets = bucketSchemaFields([...ENGINE_FIELDS, 'lens_flare']);
    expect(buckets.other).toEqual(['lens_flare']);
    expect(buckets.missing).toEqual([]);
  });

  it('splits the real schema across the effects with nothing left over', () => {
    const buckets = bucketSchemaFields(ENGINE_FIELDS);
    expect(buckets.other).toEqual([]);
    expect(buckets.missing).toEqual([]);
    expect(buckets.byEffect.fog).toEqual([
      'fog_density',
      'fog_color',
      'fog_start',
      'fog_height',
      'fog_height_falloff',
      'fog_albedo',
      'fog_sky',
      'fog_anisotropy',
    ]);
    expect(buckets.byEffect.dither).toEqual(['dither']);
  });

  it('reports the other drift direction: a field this table claims and the engine dropped', () => {
    const buckets = bucketSchemaFields(ENGINE_FIELDS.filter((name) => name !== 'halation'));
    expect(buckets.missing).toEqual(['halation']);
    expect(buckets.byEffect.halation).toEqual([]);
  });

  it('keys every effect even when the schema is empty', () => {
    const buckets = bucketSchemaFields([]);
    expect(Object.keys(buckets.byEffect).sort()).toEqual(EFFECT_ORDER.map((r) => r.id).sort());
    expect(buckets.missing).toEqual(FIELD_NAMES);
  });
});

describe('schemaFieldNames', () => {
  it('reads the property keys of a struct schema, in schema order', () => {
    expect(schemaFieldNames({ type: 'object', properties: { exposure: {}, dither: {} } })).toEqual([
      'exposure',
      'dither',
    ]);
  });

  it('returns nothing for a shape that is not a struct schema', () => {
    expect(schemaFieldNames(null)).toEqual([]);
    expect(schemaFieldNames({ type: 'string' })).toEqual([]);
    expect(schemaFieldNames('nope')).toEqual([]);
  });
});

describe('effectRow', () => {
  it('throws on an effect the chain does not render, rather than returning undefined', () => {
    expect(() => effectRow('bokeh' as EffectId)).toThrow(/bokeh/);
  });
});

describe('gateStateKey', () => {
  const key = (values: Record<string, unknown>, hasSun = true): string =>
    gateStateKey(deriveLiveEffects(values, { hasSun }));

  // The whole point of the key: a caller refetches the frame graph exactly when this moves, so an
  // effect that cannot change the pass list must not cost a round trip.
  it('does not move when an effect that schedules no pass changes', () => {
    const base = key({});
    expect(key({ fog_density: 0.02 })).toBe(base);
    expect(key({ vignette: 0.4 })).toBe(base);
    expect(key({ grain: 0.1, saturation: 0.5, exposure: 2 })).toBe(base);
  });

  it('moves for each of the three effects that do', () => {
    const base = key({});
    expect(key({ ssao: 0.5 })).not.toBe(base);
    expect(key({ dof: 0.5 })).not.toBe(base);
    expect(key({ shafts: 0.02, fog_density: 0.02 })).not.toBe(base);
  });

  // `fog_density` schedules no pass of its own, but it is half of what the shaft chain is gated on, so
  // raising it under authored shafts *does* have to move the key — otherwise the panel would keep a
  // graph dump that no longer describes the frame.
  it('moves when a medium arrives under authored shafts', () => {
    expect(key({ shafts: 0.02, fog_density: 0.02 })).not.toBe(key({ shafts: 0.02 }));
  });

  // The shafts row is live only with a sun, and the graph agrees — so the key has to read the derived
  // state and not the authored field, or a sunless scene would refetch a graph that never changed.
  it('does not move when shafts is authored into a sunless scene', () => {
    expect(key({ shafts: 0.02, fog_density: 0.02 }, false)).toBe(key({ fog_density: 0.02 }, false));
  });

  it('does not move when shafts is authored into a scene with no medium', () => {
    expect(key({ shafts: 0.02 }, true)).toBe(key({}, true));
  });

  it('moves when a sun arrives under authored shafts and a medium, at an unchanged component', () => {
    const authored = { shafts: 0.02, fog_density: 0.02 };
    expect(key(authored, true)).not.toBe(key(authored, false));
  });
});
