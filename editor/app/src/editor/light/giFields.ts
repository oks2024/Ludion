// The Lighter view's spine: `GiVolume`'s five fields modelled by what an edit COSTS.
//
// WHY THIS EXISTS. `Inspector.tsx` already edits `GiVolume` generically — schema in, widgets out —
// and that is precisely the presentation this component must not get. The five fields look alike in
// the schema (five floats, two of them with a declared travel) and are nothing alike in the engine:
// `normal_bias`, `sky_visibility` and a `bounce` that is already non-zero are sample-time uniforms
// that cost nothing to move, while `cell_size` re-voxelizes the scene (a CPU pass measured in
// hundreds of milliseconds) and re-bakes, `probe_spacing` re-bakes, and `bounce` re-bakes on the one
// crossing 0↔on. A generic slider hands all five the same 66 ms preview cadence, which turns a drag
// across the expensive two into one stall per preview frame.
//
// THE COST MODEL IS THE "BAKE" UI. There is no bake verb to expose: voxelization runs in `extract`
// and the GPU bake in `prepare_frame`, each behind a single-entry cache, so an edit's work happens on
// the next frame whether or not anything asked for it. What a human can be told is what a given edit
// will re-run, which is what {@link COST_BADGE} says per field — and it is why the panel orders the
// fields cheap-first rather than in the struct's declaration order.
//
// THE EXPENSIVE TWO ARE STEPPED, NOT DRAGGED. Their meaningful values are coarse (a doubling ladder —
// cell count grows as the cube of the reciprocal) and every distinct value redoes real work through a
// single-entry cache, so the panel offers discrete stops and one write per press. An authored value
// off the ladder is shown as an extra stop rather than snapped away: a control must never silently
// rewrite the number it was opened on, the rule `slider.ts`'s `widen` encodes for continuous travel.
//
// ABSENCE IS MEANINGFUL, TWICE. A scene with no `GiVolume` voxelizes nothing, and an authored one
// with `sky_visibility: 0` and no bounce is documented byte-identical to that — which is what makes
// {@link GI_AB} a free A/B axis and what {@link giState} has to fold together, so a card cannot claim
// GI is on while contributing nothing.
//
// Pure: no React, no wire — every rule here is exercised against object literals in giFields.test.ts.

/** The registry name of the component this view edits. */
export const GI_VOLUME = 'GiVolume';

/** One field of `GiVolume`. The struct has exactly these five. */
export type GiFieldName = 'cell_size' | 'probe_spacing' | 'normal_bias' | 'sky_visibility' | 'bounce';

/**
 * What an edit to a field makes the engine re-run.
 *
 * `revoxelize` is the CPU voxelization plus the bake that follows it; `rebake` is the GPU bake alone;
 * `rebake-on-toggle` is `bounce`'s, which re-bakes only on the 0↔non-zero crossing and rides a
 * uniform for every other move; `free` is a sample-time uniform.
 */
export type CostClass = 'revoxelize' | 'rebake' | 'rebake-on-toggle' | 'free';

/** The badge each cost class wears. Short enough to sit beside a field label. */
export const COST_BADGE: Readonly<Record<CostClass, string>> = {
  revoxelize: 're-voxelize + re-bake',
  rebake: 're-bake',
  'rebake-on-toggle': 're-bake on 0↔on',
  free: 'free',
};

/** One field row: what it is called, what it costs, and what the engine renders when it is absent. */
export interface GiFieldRow {
  readonly name: GiFieldName;
  readonly label: string;
  readonly cost: CostClass;
  /**
   * The serde default, from the field's doc comment in `crates/ludion-engine/src/components.rs`.
   *
   * Not decoration: `component.set` never carries the fields a scene left unauthored, so these are
   * the values the engine actually bakes with — what the panel dims and shows for an absent
   * component, and what {@link effectiveGi} applies before any state is judged.
   */
  readonly shipped: number;
}

/**
 * The five fields in the order the panel renders them: cheapest edit first.
 *
 * Deliberately not the struct's declaration order. Declaration order puts the two fields that stall
 * the session at the top of the card, which invites the drag this module exists to prevent.
 */
export const GI_FIELD_ORDER: readonly GiFieldRow[] = [
  { name: 'sky_visibility', label: 'Sky visibility', cost: 'free', shipped: 1.0 },
  { name: 'bounce', label: 'Bounce', cost: 'rebake-on-toggle', shipped: 0.0 },
  { name: 'normal_bias', label: 'Normal bias', cost: 'free', shipped: 0.25 },
  { name: 'probe_spacing', label: 'Probe spacing', cost: 'rebake', shipped: 1.0 },
  { name: 'cell_size', label: 'Cell size', cost: 'revoxelize', shipped: 0.5 },
];

/** The field names in panel order. */
export const GI_FIELD_NAMES: readonly GiFieldName[] = GI_FIELD_ORDER.map((row) => row.name);

/** The row for `name`. Total over `GiFieldName`, so callers never branch on `undefined`. */
export function giFieldRow(name: GiFieldName): GiFieldRow {
  const row = GI_FIELD_ORDER.find((candidate) => candidate.name === name);
  if (row === undefined) {
    throw new Error(`no GiVolume row for field '${name}'`);
  }
  return row;
}

/** The stops offered for `cell_size` — a doubling ladder from the gallery's calibrated interior to a
 *  district-coarse end, with the engine default in the middle. */
export const CELL_SIZE_STEPS: readonly number[] = [0.125, 0.25, 0.5, 1.0, 2.0];

/** The stops offered for `probe_spacing`, on the same ladder. */
export const PROBE_SPACING_STEPS: readonly number[] = [0.25, 0.5, 1.0, 2.0, 4.0];

/** One stop of a stepped control. `offList` marks the authored value that is not on the ladder — the
 *  panel highlights it, so a scene authoring `0.3` can see and keep its own number. */
export interface StepStop {
  readonly value: number;
  readonly offList: boolean;
}

/**
 * The ladder `steps`, with an authored `current` that is not on it injected in sorted position.
 *
 * `null`/`undefined` (the component is absent) and a non-finite value give the plain ladder — there
 * is no authored number to preserve. Membership is exact equality with no epsilon: these values
 * round-trip through canonical YAML and JSON unchanged, so a near-miss is a different authored value
 * and hiding it behind a tolerance would let a press silently rewrite it.
 */
export function stepListFor(steps: readonly number[], current: number | null | undefined): readonly StepStop[] {
  const ladder = steps.map((value) => ({ value, offList: false }));
  if (typeof current !== 'number' || !Number.isFinite(current) || steps.includes(current)) {
    return ladder;
  }
  const at = ladder.findIndex((stop) => stop.value > current);
  const stop: StepStop = { value: current, offList: true };
  return at < 0 ? [...ladder, stop] : [...ladder.slice(0, at), stop, ...ladder.slice(at)];
}

/**
 * Whether `stop` is the value a stepped control currently holds.
 *
 * One predicate answers both of the row's questions — which stop is highlighted, and which press is
 * refused — so a stop can never look current and still write. That matters more here than on a
 * slider: the press it would send costs a re-bake, or a re-voxelize, for a value the engine already
 * holds. Exact equality, for {@link stepListFor}'s reason.
 */
export function isCurrentStep(stop: number, current: number): boolean {
  return stop === current;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** A field's authored number, or the serde default — a non-numeric value cannot have come from a
 *  valid component, and reading it as authored would let garbage decide a state chip. */
function numberOr(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

/** Every field's effective value: what the scene authors over the serde defaults, which together are
 *  what the engine bakes with. `null` (no component) yields the defaults, so the panel can dim and
 *  show them without a second code path. */
export function effectiveGi(value: Record<string, unknown> | null): Readonly<Record<GiFieldName, number>> {
  const source = isRecord(value) ? value : {};
  const out = {} as Record<GiFieldName, number>;
  for (const row of GI_FIELD_ORDER) {
    out[row.name] = numberOr(source[row.name], row.shipped);
  }
  return out;
}

/**
 * What the GI card's chip says.
 *
 * `off` covers two states the engine cannot tell apart: no component at all, and an authored one
 * whose `sky_visibility` is `0` with no bounce — documented byte-identical to having none. `bounce`
 * outranks a zeroed sky visibility, because a bounce term is added to the ambient rather than
 * attenuating it, so it still contributes with the sky half switched off.
 */
export function giState(value: Record<string, unknown> | null): 'off' | 'sky' | 'bounce' {
  if (!isRecord(value)) {
    return 'off';
  }
  const gi = effectiveGi(value);
  if (gi.bounce > 0) {
    return 'bounce';
  }
  return gi.sky_visibility === 0 ? 'off' : 'sky';
}

// --- which term a light's contribution lives in -------------------------------------------------

/**
 * Where a light's contribution reaches the volume.
 *
 * `baked` is the default and the expensive one: the light feeds the probe bake's key, so moving or
 * recolouring it rebuilds the bake — and a `Mesh` on the same entity feeds the voxelizer's instance
 * signature, so moving it re-voxelizes too. `injected` is what `GiDynamic` buys: the light is added
 * per frame instead, and its motion invalidates neither cache.
 */
export type GiLightTerm = 'baked' | 'injected';

/** Which term a light lives in, given whether it carries `GiDynamic` (the marker's registry name
 *  lives with the other light components, in `lights.ts`). The chip's text is the term itself —
 *  there is no shorter true word for either state. */
export function giLightTerm(dynamic: boolean): GiLightTerm {
  return dynamic ? 'injected' : 'baked';
}

/** What each term costs to move, for the chip's title. */
export const GI_TERM_HINT: Readonly<Record<GiLightTerm, string>> = {
  baked: 'Baked into the volume: moving or recolouring this light re-bakes it.',
  injected: 'Injected per frame: moving or recolouring this light invalidates nothing.',
};

/** The badge beside the toggle, in {@link COST_BADGE}'s idiom: pressing it moves the light between
 *  the two terms, which is one bake either way — including the press that switches injection ON. */
export const MARK_COST_BADGE = 're-bake either way';

/**
 * Whether a moving-light note applies.
 *
 * `term` is `null` for the card-level claim about the scene's lights in general, and the light's own
 * term for a claim about one light. A bake keys on the lights, so with `bounce > 0` moving a `baked`
 * light re-bakes the volume every frame — but an `injected` one is free, which is exactly what the
 * blanket "any light" wording stopped being able to say once injection shipped.
 */
export function showsMovingLightNote(
  value: Record<string, unknown> | null,
  term: GiLightTerm | null,
): boolean {
  return term !== 'injected' && isRecord(value) && effectiveGi(value).bounce > 0;
}

/** The neutral patch behind "GI A/B": the free comparison, since `sky_visibility` is a sample-time
 *  uniform in both directions and `0` is exactly the no-component look. */
export const GI_AB: Readonly<Record<string, number>> = { sky_visibility: 0 };

/** The neutral patch behind "Bounce A/B". One re-bake per end of the hold — offered only where there
 *  is a bounce to remove. */
export const BOUNCE_AB: Readonly<Record<string, number>> = { bounce: 0 };

/** Whether "GI A/B" is offered: the component is authored. With none, the frame already shows the
 *  neutral end and the press would materialize a component to compare against itself. */
export function offersGiAb(value: Record<string, unknown> | null): boolean {
  return isRecord(value);
}

/** Whether "Bounce A/B" is offered: the component is authored and its effective bounce contributes.
 *  At `0` the hold would pay two bakes to show the same image twice. */
export function offersBounceAb(value: Record<string, unknown> | null): boolean {
  return isRecord(value) && effectiveGi(value).bounce > 0;
}
