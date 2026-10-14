import { describe, expect, it } from 'vitest';
import {
  BOUNCE_AB,
  CELL_SIZE_STEPS,
  COST_BADGE,
  effectiveGi,
  GI_AB,
  GI_FIELD_NAMES,
  GI_FIELD_ORDER,
  GI_TERM_HINT,
  GI_VOLUME,
  giFieldRow,
  giLightTerm,
  giState,
  isCurrentStep,
  MARK_COST_BADGE,
  offersBounceAb,
  offersGiAb,
  PROBE_SPACING_STEPS,
  showsMovingLightNote,
  stepListFor,
} from './giFields';

/** The value `component.get` returns for a real `GiVolume`: serde writes every field. */
const AUTHORED = (over: Record<string, unknown> = {}): Record<string, unknown> => ({
  cell_size: 0.5,
  probe_spacing: 1.0,
  normal_bias: 0.25,
  sky_visibility: 1.0,
  bounce: 0.0,
  ...over,
});

const values = (stops: readonly { value: number }[]): number[] => stops.map((stop) => stop.value);

describe('the field table', () => {
  it('carries the five fields cheap-first, so the stalling ones are not at the top of the card', () => {
    expect(GI_FIELD_NAMES).toEqual(['sky_visibility', 'bounce', 'normal_bias', 'probe_spacing', 'cell_size']);
  });

  it('names the engine cost of every field, and a badge for every class', () => {
    expect(GI_FIELD_ORDER.map((row) => row.cost)).toEqual([
      'free',
      'rebake-on-toggle',
      'free',
      'rebake',
      'revoxelize',
    ]);
    for (const row of GI_FIELD_ORDER) {
      expect(COST_BADGE[row.cost], `${row.name} needs a badge`).toBeTruthy();
    }
  });

  it('mirrors the serde defaults, which are what an absent field bakes with', () => {
    expect(effectiveGi(null)).toEqual({
      cell_size: 0.5,
      probe_spacing: 1.0,
      normal_bias: 0.25,
      sky_visibility: 1.0,
      bounce: 0.0,
    });
  });

  it('resolves every field name to its row', () => {
    expect(giFieldRow('cell_size').cost).toBe('revoxelize');
    expect(giFieldRow('bounce').label).toBe('Bounce');
  });

  it('names the component the view edits', () => {
    expect(GI_VOLUME).toBe('GiVolume');
  });
});

describe('effectiveGi', () => {
  it('lets an authored field beat its default and leaves the rest at theirs', () => {
    expect(effectiveGi({ bounce: 1.5 })).toEqual({
      cell_size: 0.5,
      probe_spacing: 1.0,
      normal_bias: 0.25,
      sky_visibility: 1.0,
      bounce: 1.5,
    });
  });

  it('reads a garbage value as unauthored rather than trusting it', () => {
    expect(effectiveGi({ bounce: 'lots' as unknown as number }).bounce).toBe(0);
    expect(effectiveGi({ sky_visibility: Number.NaN }).sky_visibility).toBe(1);
    expect(effectiveGi('nope' as unknown as Record<string, unknown>).sky_visibility).toBe(1);
  });

  it('keeps an authored zero, which is a meaningful value on both switchable fields', () => {
    expect(effectiveGi({ sky_visibility: 0 }).sky_visibility).toBe(0);
  });
});

describe('stepListFor', () => {
  it('offers the plain ladder for an absent component', () => {
    expect(values(stepListFor(CELL_SIZE_STEPS, null))).toEqual([...CELL_SIZE_STEPS]);
    expect(values(stepListFor(PROBE_SPACING_STEPS, undefined))).toEqual([...PROBE_SPACING_STEPS]);
    expect(stepListFor(CELL_SIZE_STEPS, null).every((stop) => !stop.offList)).toBe(true);
  });

  it('injects an off-ladder authored value in sorted position and flags it', () => {
    const stops = stepListFor(CELL_SIZE_STEPS, 0.3);
    expect(values(stops)).toEqual([0.125, 0.25, 0.3, 0.5, 1.0, 2.0]);
    expect(stops.filter((stop) => stop.offList)).toEqual([{ value: 0.3, offList: true }]);
  });

  it('injects below the first stop and above the last', () => {
    expect(values(stepListFor(PROBE_SPACING_STEPS, 0.1))).toEqual([0.1, 0.25, 0.5, 1.0, 2.0, 4.0]);
    expect(values(stepListFor(PROBE_SPACING_STEPS, 9))).toEqual([0.25, 0.5, 1.0, 2.0, 4.0, 9]);
    expect(stepListFor(PROBE_SPACING_STEPS, 9).at(-1)?.offList).toBe(true);
  });

  it('adds nothing for a value already on the ladder — exact equality, no tolerance', () => {
    expect(values(stepListFor(CELL_SIZE_STEPS, 0.25))).toEqual([...CELL_SIZE_STEPS]);
    // A near-miss is a different authored number, and snapping it would rewrite the scene.
    expect(values(stepListFor(CELL_SIZE_STEPS, 0.2500001))).toEqual([0.125, 0.25, 0.2500001, 0.5, 1.0, 2.0]);
  });

  it('ignores a non-finite value rather than putting it on the track', () => {
    expect(values(stepListFor(CELL_SIZE_STEPS, Number.NaN))).toEqual([...CELL_SIZE_STEPS]);
    expect(values(stepListFor(CELL_SIZE_STEPS, Number.POSITIVE_INFINITY))).toEqual([...CELL_SIZE_STEPS]);
  });

  it('does not alias the ladder it was given', () => {
    stepListFor(CELL_SIZE_STEPS, 0.3);
    expect([...CELL_SIZE_STEPS]).toEqual([0.125, 0.25, 0.5, 1.0, 2.0]);
  });
});

describe('isCurrentStep', () => {
  it('marks exactly the stop the component holds, so the highlighted press is the refused one', () => {
    const stops = stepListFor(CELL_SIZE_STEPS, 0.5);
    expect(stops.filter((stop) => isCurrentStep(stop.value, 0.5))).toEqual([{ value: 0.5, offList: false }]);
  });

  it('marks the injected off-ladder stop, which is the value the component actually holds', () => {
    const stops = stepListFor(CELL_SIZE_STEPS, 0.3);
    expect(stops.filter((stop) => isCurrentStep(stop.value, 0.3))).toEqual([{ value: 0.3, offList: true }]);
  });

  it('marks nothing on a near miss — that press writes, because it is a different value', () => {
    expect(isCurrentStep(0.25, 0.2500001)).toBe(false);
    // An unauthored component reads through its serde default, which is on the ladder.
    expect(isCurrentStep(0.5, effectiveGi(null).cell_size)).toBe(true);
  });
});

describe('giState', () => {
  it('reads an absent component as off', () => {
    expect(giState(null)).toBe('off');
  });

  it('reads a defaults-only component as sky, because sky visibility defaults to full', () => {
    expect(giState({})).toBe('sky');
    expect(giState(AUTHORED())).toBe('sky');
  });

  it('reads a zeroed sky visibility with no bounce as off — it is the no-component look', () => {
    expect(giState(AUTHORED({ sky_visibility: 0 }))).toBe('off');
    expect(giState({ sky_visibility: 0 })).toBe('off');
  });

  it('reads any bounce as bounce, even with the sky half switched off', () => {
    expect(giState(AUTHORED({ bounce: 1.0 }))).toBe('bounce');
    expect(giState(AUTHORED({ sky_visibility: 0, bounce: 0.5 }))).toBe('bounce');
    expect(giState({ bounce: 0.001 })).toBe('bounce');
  });

  it('does not read a negative or zero bounce as a bounce', () => {
    expect(giState(AUTHORED({ bounce: 0 }))).toBe('sky');
    expect(giState(AUTHORED({ bounce: -1 }))).toBe('sky');
  });
});

describe('the A/B holds', () => {
  it('neutralize one field each, and only that field', () => {
    expect(GI_AB).toEqual({ sky_visibility: 0 });
    expect(BOUNCE_AB).toEqual({ bounce: 0 });
  });

  it('offers the GI hold wherever the component is authored', () => {
    expect(offersGiAb(AUTHORED())).toBe(true);
    expect(offersGiAb({})).toBe(true);
    expect(offersGiAb(null)).toBe(false);
  });

  it('offers the bounce hold only against a bounce that contributes', () => {
    expect(offersBounceAb(AUTHORED({ bounce: 0.4 }))).toBe(true);
    expect(offersBounceAb(AUTHORED())).toBe(false);
    expect(offersBounceAb({})).toBe(false);
    expect(offersBounceAb(null)).toBe(false);
    // Zeroed sky visibility does not withdraw it: the bounce is added, not attenuated.
    expect(offersBounceAb(AUTHORED({ sky_visibility: 0, bounce: 0.4 }))).toBe(true);
  });
});

describe('giLightTerm', () => {
  it('names the term a light contributes through, which the marker alone decides', () => {
    expect(giLightTerm(true)).toBe('injected');
    expect(giLightTerm(false)).toBe('baked');
  });

  it('says what each term costs to move, and badges the press that swaps them', () => {
    expect(GI_TERM_HINT.baked).toMatch(/re-bakes/);
    expect(GI_TERM_HINT.injected).toMatch(/invalidates nothing/);
    // Marking is not free in the direction that makes moving free — the light changes terms either
    // way, and a term change is a bake.
    expect(MARK_COST_BADGE).toMatch(/re-bake/);
  });
});

describe('showsMovingLightNote', () => {
  it('is shown for the scene in general exactly where a light move would re-bake every frame', () => {
    expect(showsMovingLightNote(AUTHORED({ bounce: 1.0 }), null)).toBe(true);
    expect(showsMovingLightNote(AUTHORED(), null)).toBe(false);
    expect(showsMovingLightNote({}, null)).toBe(false);
    expect(showsMovingLightNote(null, null)).toBe(false);
  });

  it('is shown for a baked light, which is the one the claim is actually true of', () => {
    expect(showsMovingLightNote(AUTHORED({ bounce: 1.0 }), 'baked')).toBe(true);
    expect(showsMovingLightNote(AUTHORED(), 'baked')).toBe(false);
  });

  it('is never shown for an injected light, however much bounce is on', () => {
    // The blanket "any light" wording this replaces went false the moment injection shipped: a
    // marked light's motion invalidates neither cache.
    expect(showsMovingLightNote(AUTHORED({ bounce: 1.0 }), 'injected')).toBe(false);
    expect(showsMovingLightNote(AUTHORED({ bounce: 9.0 }), 'injected')).toBe(false);
    expect(showsMovingLightNote(AUTHORED(), 'injected')).toBe(false);
  });
});
