import { describe, expect, it } from 'vitest';
import { beginCompare, endCompare, restoredValue, type CompareSession, type ComparePatch, type CompareStart } from './compare';
import { effectiveValues, neutralPatch, type EffectId } from './art/chain';
import { POST_PROCESS } from './art/target';
import { BOUNCE_AB, GI_AB, GI_VOLUME } from './light/giFields';

const ENTITY = { index: 3, generation: 1 };

/** A fully-authored value, the shape `component.get` returns for a real `PostProcess` (serde writes
 *  every field), carrying a look worth comparing against. */
const AUTHORED = (): Record<string, unknown> => ({
  ...effectiveValues({}),
  fog_density: 0.02,
  fog_color: [0.06, 0.07, 0.12],
  fog_start: 8,
  fog_height_falloff: 0.1,
  vignette: 0.35,
  gamma: [0.9, 0.95, 1.05],
  exposure: 1.1,
});

/** The Art Director's press, verbatim: it derives the effect's neutral and hands it in. */
function press(current: Record<string, unknown> | null, effect: EffectId): CompareStart {
  const start = beginCompare(POST_PROCESS, ENTITY, current, neutralPatch(effect), effect);
  expect(start, `${effect} should be comparable`).not.toBeNull();
  return start as CompareStart;
}

const params = (patch: ComparePatch): Record<string, unknown> =>
  (patch.command as unknown as { params: Record<string, unknown> }).params;

const method = (patch: ComparePatch): string => (patch.command as unknown as { method: string }).method;

const written = (patch: ComparePatch): Record<string, unknown> =>
  params(patch).value as Record<string, unknown>;

describe('hold to compare', () => {
  it('restores the captured object exactly, arrays included', () => {
    const before = AUTHORED();
    const start = press(before, 'fog');
    const restored = written(endCompare(start.session));
    expect(restored).toEqual(before);
    // Byte-equal through JSON, which is what actually goes on the wire.
    expect(JSON.stringify(restored)).toBe(JSON.stringify(before));
  });

  it('writes the effect at its neutral and leaves every other field alone', () => {
    const before = AUTHORED();
    const value = written(press(before, 'fog').patch);
    expect(value.fog_density).toBe(0);
    expect(value.fog_height_falloff).toBe(0);
    // Shaping fields are untouched: they are already inert once the gate is neutral, and rewriting
    // them would give the release something more to get wrong.
    expect(value.fog_color).toEqual(before.fog_color);
    expect(value.fog_start).toBe(8);
    expect(value.vignette).toBe(0.35);
    expect(value.exposure).toBe(1.1);
  });

  it('addresses the whole component, because component.set takes no patch', () => {
    const start = press(AUTHORED(), 'vignette');
    expect(method(start.patch)).toBe('component.set');
    expect(params(start.patch).component).toBe('PostProcess');
    expect(params(start.patch).entity).toEqual(ENTITY);
    expect(Object.keys(written(start.patch))).toEqual(Object.keys(AUTHORED()));
  });

  it('is reported as neither a mutation nor an undo entry, on both ends', () => {
    const start = press(AUTHORED(), 'grade');
    expect(start.patch.undoable).toBe(false);
    expect(start.patch.dirties).toBe(false);
    const end = endCompare(start.session);
    expect(end.undoable).toBe(false);
    expect(end.dirties).toBe(false);
  });

  it('refuses a row with no off, rather than writing a fake neutral', () => {
    // The tonemap: `neutralPatch` is empty for it, which is how the caller says "nothing to hold".
    expect(neutralPatch('tonemap')).toEqual({});
    expect(beginCompare(POST_PROCESS, ENTITY, AUTHORED(), {}, 'tonemap')).toBeNull();
  });
});

describe('the capture is isolated in both directions', () => {
  it('does not alias the value it was handed — later edits cannot rewrite history', () => {
    const before = AUTHORED();
    const start = press(before, 'grade');
    (before.gamma as number[])[0] = 42;
    before.exposure = 99;
    expect(restoredValue(start.session)).toEqual(AUTHORED());
  });

  it('does not alias the value it hands back — a mutated restore cannot corrupt the session', () => {
    const start = press(AUTHORED(), 'grade');
    const first = written(endCompare(start.session));
    (first.gamma as number[])[0] = 42;
    first.exposure = 99;
    expect(written(endCompare(start.session))).toEqual(AUTHORED());
  });

  it('does not alias the neutralized value into the capture', () => {
    const start = press(AUTHORED(), 'grade');
    const neutralized = written(start.patch);
    (neutralized.gamma as number[])[2] = 42;
    expect(restoredValue(start.session)).toEqual(AUTHORED());
  });

  it('does not alias the neutral patch it was handed', () => {
    const neutral = { sky_visibility: 0 };
    const start = beginCompare(GI_VOLUME, ENTITY, { sky_visibility: 1, bounce: 1 }, neutral, 'gi');
    written(start!.patch).sky_visibility = 42;
    expect(neutral).toEqual({ sky_visibility: 0 });
  });

  it('releases the same value however many times it is asked', () => {
    const start = press(AUTHORED(), 'fog');
    expect(written(endCompare(start.session))).toEqual(written(endCompare(start.session)));
  });
});

describe('comparing on a scene that authors no PostProcess', () => {
  it('materializes only the neutralized fields, so nothing else moves', () => {
    const start = press(null, 'dither');
    expect(written(start.patch)).toEqual({ dither: false });
    expect(start.session.captured).toBeNull();
  });

  it('releases by removing the component again — the inverse of a fresh insert', () => {
    const start = press(null, 'exposure');
    expect(written(start.patch)).toEqual({ exposure: 1.0 });
    const end = endCompare(start.session);
    expect(method(end)).toBe('component.remove');
    expect(params(end)).toEqual({ entity: ENTITY, component: 'PostProcess' });
    expect(restoredValue(start.session)).toBeNull();
  });

  it('treats a non-component value as absence rather than trusting it', () => {
    const start = press('nope' as unknown as Record<string, unknown>, 'vignette');
    expect(start.session.captured).toBeNull();
  });
});

describe('a session is plain data', () => {
  it('carries only what a release needs, so a caller can hold it across a re-render', () => {
    const session: CompareSession = press(AUTHORED(), 'vignette').session;
    expect(session.component).toBe('PostProcess');
    expect(session.label).toBe('vignette');
    expect(session.entity).toEqual(ENTITY);
    expect(Object.keys(session).sort()).toEqual(['captured', 'component', 'entity', 'label']);
  });
});

describe('the same reducer over GiVolume', () => {
  /** What `component.get` returns for an authored `GiVolume`: serde writes every field. */
  const GI = (over: Record<string, unknown> = {}): Record<string, unknown> => ({
    cell_size: 0.5,
    probe_spacing: 1.0,
    normal_bias: 0.25,
    sky_visibility: 1.0,
    bounce: 1.0,
    ...over,
  });

  it('holds the GI A/B on one field and restores byte-exactly', () => {
    const before = GI({ cell_size: 0.3 });
    const start = beginCompare(GI_VOLUME, ENTITY, before, GI_AB, 'gi')!;
    expect(params(start.patch).component).toBe('GiVolume');
    expect(written(start.patch).sky_visibility).toBe(0);
    // The expensive fields are untouched, so the hold pays no re-voxelize.
    expect(written(start.patch).cell_size).toBe(0.3);
    expect(written(start.patch).probe_spacing).toBe(1.0);
    const restored = written(endCompare(start.session));
    expect(JSON.stringify(restored)).toBe(JSON.stringify(before));
  });

  it('holds the bounce A/B the same way, moving only bounce', () => {
    const before = GI({ bounce: 1.6 });
    const start = beginCompare(GI_VOLUME, ENTITY, before, BOUNCE_AB, 'bounce')!;
    expect(written(start.patch)).toEqual(GI({ bounce: 0 }));
    expect(JSON.stringify(written(endCompare(start.session)))).toBe(JSON.stringify(before));
  });

  it('materializes and then removes when the scene authors no GiVolume', () => {
    const start = beginCompare(GI_VOLUME, ENTITY, null, GI_AB, 'gi')!;
    expect(written(start.patch)).toEqual({ sky_visibility: 0 });
    expect(params(endCompare(start.session))).toEqual({ entity: ENTITY, component: 'GiVolume' });
  });

  it('refuses an empty neutral, whatever the component', () => {
    expect(beginCompare(GI_VOLUME, ENTITY, GI(), {}, 'gi')).toBeNull();
  });
});
