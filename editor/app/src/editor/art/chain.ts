// The Art Director view's spine: the post-process chain as a model of what is *actually running*.
//
// WHY THIS EXISTS. `Inspector.tsx` already edits `PostProcess` generically — schema in, widgets out.
// What it structurally cannot show is the thing every look verdict in the post-process arc turned on:
// a field's value does not tell you whether the effect is contributing. `PostChain::for_scene`
// (crates/ludion-renderer/src/graph.rs) derives three of the effects out of the frame graph entirely
// when their gate is neutral, and `shafts` needs two more things on top of its own field: a
// `DirectionalLight` for the march to read, and a medium (`fog_density`) for it to scatter in. A scene
// authoring `shafts: 0.02` with either missing renders nothing and reports no error, because "absence
// is meaningful" is this component's whole posture. This module is the answer to "why did that do
// nothing", derived from values plus preconditions and cross-checkable against the real graph.
//
// PIPELINE ORDER IS THE COMPONENT'S CONTRACT, NOT SCHEMA ORDER. The order below is the one
// `composite.wgsl`'s `fs_main` executes, which is the tiebreaker whenever prose and code could
// disagree: the aberration is in the scene *fetch* (it is how the image was formed, not something
// applied to it), the defocus layer composites over the sharp scene before the bloom mix, halation
// joins that same mix, and the shafts term is added between the fog and the exposure. Ambient
// occlusion is not an imaging step at all — it modulates the ambient + IBL term inside the scene
// pass — so it heads the list rather than sitting in it.
//
// THE FIELD TABLE HAS NO BUILD-TIME GATE. `editor/protocol/src/protocol.d.ts` carries the command
// union only, never component schemas, so nothing can compare this table against the Rust struct at
// compile time (the plan's decision 11). The runtime safety net is `bucketSchemaFields`: a field the
// engine grows and this table does not know lands in a visible `other` bucket instead of vanishing,
// and the same call reports a field this table knows that the engine no longer has.
//
// Pure: no React, no wire, no DOM — every rule here is exercised against object literals in
// chain.test.ts.

/** One row of the chain — an effect a human directs, not a field. */
export type EffectId =
  | 'ssao'
  | 'chromatic_aberration'
  | 'dof'
  | 'bloom'
  | 'halation'
  | 'fog'
  | 'shafts'
  | 'exposure'
  | 'tonemap'
  | 'grade'
  | 'vignette'
  | 'grain'
  | 'dither';

/** Where in the frame an effect happens — what makes the chain's order legible rather than arbitrary.
 *  `scene` is inside the scene pass (lighting, not imaging); `fetch` is the composite's sampling of
 *  the scene, before any colour has been formed; `composite` is a step of the composite's pinned
 *  order. */
export type EffectStage = 'scene' | 'fetch' | 'composite';

/** A precondition an effect needs from the *scene* rather than from any `PostProcess` field. `sun` is
 *  a `DirectionalLight`: the shaft march reads the shadow map a sun is what builds. */
export type Precondition = 'sun';

/** One effect row: its place in the pipeline, the passes its gate adds to or removes from the frame
 *  graph, and the two kinds of precondition it needs on top of its own fields. */
export interface EffectRow {
  readonly id: EffectId;
  readonly label: string;
  readonly stage: EffectStage;
  /** The passes `PostChain::for_scene` builds only when this effect is live, by their
   *  `render.dump_graph` names. Empty for every effect that lives inside an unconditional pass — the
   *  composite, or the bloom pyramid, which is built whatever `bloom_strength` says. */
  readonly gatedPasses: readonly string[];
  /** The scene precondition, or `null`. */
  readonly requires: Precondition | null;
  /**
   * Another **effect** this one is a term of, or `null` — a precondition that is neither this effect's
   * own field nor the scene's.
   *
   * Only `shafts`, and it names `fog`: the march estimates the *directional* in-scatter of the
   * participating medium whose *ambient* in-scatter the fog row is, and its coefficient is
   * σ_s = `fog_albedo` · `fog_density`. With no medium the estimate is exactly zero, so
   * `PostChain::for_scene` builds neither shaft pass.
   *
   * It names the effect rather than the field on purpose: "is there a medium" is already spelled once,
   * as the fog row's own gate (`fog_density`), and duplicating that threshold here would be a second
   * place for it to drift from the engine's.
   */
  readonly requiresEffect: EffectId | null;
  /** An effect with no "off" — it is a choice among always-applied alternatives, never a contribution
   *  that can be zero. Only the tonemap: every operator is a curve, and `none` is a diagnostic
   *  passthrough rather than a bypass. */
  readonly alwaysLive: boolean;
}

/**
 * The pinned pipeline order — the sequence `composite.wgsl` executes, with the scene-referred
 * ambient occlusion ahead of it.
 *
 * This is the order the chain panel renders, and it is deliberately not the schema's field order:
 * schema order is a struct's declaration, this is the component's documented contract.
 */
export const EFFECT_ORDER: readonly EffectRow[] = [
  {
    id: 'ssao',
    label: 'Ambient occlusion',
    stage: 'scene',
    gatedPasses: ['prepass', 'ssao', 'ssao_blur'],
    requires: null,
    requiresEffect: null,
    alwaysLive: false,
  },
  {
    id: 'chromatic_aberration',
    label: 'Chromatic aberration',
    stage: 'fetch',
    gatedPasses: [],
    requires: null,
    requiresEffect: null,
    alwaysLive: false,
  },
  { id: 'dof', label: 'Depth of field', stage: 'composite', gatedPasses: ['dof_coc', 'dof'], requires: null, requiresEffect: null, alwaysLive: false },
  { id: 'bloom', label: 'Bloom', stage: 'composite', gatedPasses: [], requires: null, requiresEffect: null, alwaysLive: false },
  { id: 'halation', label: 'Halation', stage: 'composite', gatedPasses: [], requires: null, requiresEffect: null, alwaysLive: false },
  { id: 'fog', label: 'Fog', stage: 'composite', gatedPasses: [], requires: null, requiresEffect: null, alwaysLive: false },
  {
    id: 'shafts',
    label: 'Light shafts',
    stage: 'composite',
    gatedPasses: ['shafts', 'shafts_blur'],
    requires: 'sun',
    requiresEffect: 'fog',
    alwaysLive: false,
  },
  { id: 'exposure', label: 'Exposure', stage: 'composite', gatedPasses: [], requires: null, requiresEffect: null, alwaysLive: false },
  { id: 'tonemap', label: 'Tonemap', stage: 'composite', gatedPasses: [], requires: null, requiresEffect: null, alwaysLive: true },
  { id: 'grade', label: 'Grade', stage: 'composite', gatedPasses: [], requires: null, requiresEffect: null, alwaysLive: false },
  { id: 'vignette', label: 'Vignette', stage: 'composite', gatedPasses: [], requires: null, requiresEffect: null, alwaysLive: false },
  { id: 'grain', label: 'Grain', stage: 'composite', gatedPasses: [], requires: null, requiresEffect: null, alwaysLive: false },
  { id: 'dither', label: 'Dither', stage: 'composite', gatedPasses: [], requires: null, requiresEffect: null, alwaysLive: false },
];

/** A `PostProcess` value as it travels the wire: a JSON scalar, a boolean, or a fixed triple. */
export type FieldValue = number | boolean | string | readonly number[];

/** One field of `PostProcess`, as the chain models it. */
export interface FieldEntry {
  /** The effect this field belongs to. Exactly one — see the exhaustiveness test. */
  readonly effect: EffectId;
  /**
   * The value at which this field contributes nothing, or `null` when it has none.
   *
   * `null` is two different honest answers. A *shaping* field (`vignette_radius`, `focus_distance`,
   * `fog_start`) has no value that means "nothing" — it says where the effect happens, and its own
   * gate is what switches the effect off. `tonemap` is the other: every operator is a curve, so
   * there is no neutral to write and it is modelled as not-comparable rather than given a fake one.
   */
  readonly neutral: FieldValue | null;
  /**
   * Whether this field's neutral switches the whole effect off.
   *
   * A gate is what `deriveLiveEffects` reads. `fog_height_falloff` is the case that makes the
   * distinction load-bearing: `0.0` is genuinely its neutral (the height integral collapses to plain
   * distance fog) but it is not fog's gate — `fog_density` is, and a scene with a height falloff and
   * no density has no fog at all.
   */
  readonly gate: boolean;
  /**
   * The engine's shipped default, from the field's doc comment in
   * `crates/ludion-engine/src/components.rs`.
   *
   * It is not decoration: a scene with no `PostProcess` renders through exactly these values
   * (the component's byte-identity guarantee), so they are what the chain reads for an unauthored
   * field and what the UI dims and labels as shipped. Three of them are *not* their field's neutral
   * — `bloom_strength`, `exposure` and `dither` ship live — which is why an unauthored scene is
   * styled rather than raw.
   */
  readonly shipped: FieldValue;
}

/**
 * Every `PostProcess` field, in the struct's declaration order.
 *
 * Mirrors `crates/ludion-engine/src/components.rs`; there is no compile-time tie between the two
 * (decision 11), so `bucketSchemaFields` against the runtime schema is what catches drift.
 */
export const FIELDS: Readonly<Record<string, FieldEntry>> = {
  bloom_threshold: { effect: 'bloom', neutral: null, gate: false, shipped: 1.0 },
  bloom_strength: { effect: 'bloom', neutral: 0, gate: true, shipped: 0.1 },
  exposure: { effect: 'exposure', neutral: 1.0, gate: true, shipped: 1.3 },
  tonemap: { effect: 'tonemap', neutral: null, gate: false, shipped: 'agx' },
  dither: { effect: 'dither', neutral: false, gate: true, shipped: true },
  contrast: { effect: 'grade', neutral: 1.0, gate: true, shipped: 1.0 },
  saturation: { effect: 'grade', neutral: 1.0, gate: true, shipped: 1.0 },
  temperature: { effect: 'grade', neutral: 0, gate: true, shipped: 0 },
  tint: { effect: 'grade', neutral: 0, gate: true, shipped: 0 },
  lift: { effect: 'grade', neutral: [0, 0, 0], gate: true, shipped: [0, 0, 0] },
  gamma: { effect: 'grade', neutral: [1, 1, 1], gate: true, shipped: [1, 1, 1] },
  gain: { effect: 'grade', neutral: [1, 1, 1], gate: true, shipped: [1, 1, 1] },
  fog_density: { effect: 'fog', neutral: 0, gate: true, shipped: 0 },
  fog_color: { effect: 'fog', neutral: null, gate: false, shipped: [0.5, 0.5, 0.5] },
  fog_start: { effect: 'fog', neutral: null, gate: false, shipped: 0 },
  fog_height: { effect: 'fog', neutral: null, gate: false, shipped: 0 },
  fog_height_falloff: { effect: 'fog', neutral: 0, gate: false, shipped: 0 },
  fog_albedo: { effect: 'fog', neutral: 1.0, gate: false, shipped: 1.0 },
  fog_sky: { effect: 'fog', neutral: 0, gate: false, shipped: 0 },
  fog_anisotropy: { effect: 'fog', neutral: null, gate: false, shipped: 0.7 },
  vignette: { effect: 'vignette', neutral: 0, gate: true, shipped: 0 },
  vignette_radius: { effect: 'vignette', neutral: null, gate: false, shipped: 0.5 },
  ssao: { effect: 'ssao', neutral: 0, gate: true, shipped: 0 },
  ssao_radius: { effect: 'ssao', neutral: null, gate: false, shipped: 0.5 },
  ssao_bias: { effect: 'ssao', neutral: null, gate: false, shipped: 0.02 },
  ssao_direct: { effect: 'ssao', neutral: 0, gate: false, shipped: 0 },
  shafts: { effect: 'shafts', neutral: 0, gate: true, shipped: 0 },
  dof: { effect: 'dof', neutral: 0, gate: true, shipped: 0 },
  focus_distance: { effect: 'dof', neutral: null, gate: false, shipped: 10.0 },
  chromatic_aberration: { effect: 'chromatic_aberration', neutral: 0, gate: true, shipped: 0 },
  halation: { effect: 'halation', neutral: 0, gate: true, shipped: 0 },
  grain: { effect: 'grain', neutral: 0, gate: true, shipped: 0 },
  grain_size: { effect: 'grain', neutral: null, gate: false, shipped: 0.002 },
};

/** Every field name this table knows, in the struct's declaration order. */
export const FIELD_NAMES: readonly string[] = Object.keys(FIELDS);

/** The field names belonging to `effect`, in declaration order. */
export function effectFields(effect: EffectId): readonly string[] {
  return FIELD_NAMES.filter((name) => FIELDS[name].effect === effect);
}

/** The row for `effect`. Total over `EffectId`, so callers never branch on `undefined`. */
export function effectRow(effect: EffectId): EffectRow {
  const row = EFFECT_ORDER.find((candidate) => candidate.id === effect);
  if (row === undefined) {
    throw new Error(`no chain row for effect '${effect}'`);
  }
  return row;
}

/** The effects whose gate adds or removes frame-graph passes — ambient occlusion, light shafts and
 *  depth of field, and nothing else (`PostChain::for_scene`). Every other effect is a branch inside
 *  a pass that is always built, so switching it off saves ALU and never a pass. */
export const PASS_GATING_EFFECTS: readonly EffectId[] = EFFECT_ORDER.filter(
  (row) => row.gatedPasses.length > 0,
).map((row) => row.id);

/** A `PostProcess` value keyed by field name, as `component.get` returns it. Absent keys read as
 *  their shipped default — which is exactly what the engine renders for them. */
export type PostProcessValues = Readonly<Record<string, unknown>>;

/** What the chain needs from the *scene* to answer "is this effect contributing" — the preconditions
 *  no `PostProcess` field can express. `hasSun` is whether the scene carries a `DirectionalLight`. */
export interface ChainContext {
  readonly hasSun: boolean;
}

/** Why a row reads the way it does. `neutral`: every gate is at its neutral, so the effect contributes
 *  nothing. `missing-sun` and `missing-medium` are the two silent-nothing cases this whole module
 *  exists to surface — the effect is engaged, and something outside its own fields withholds what it
 *  needs: a `DirectionalLight` for the march to read, or a medium for the march to scatter in. */
export type EffectReason = 'live' | 'neutral' | 'missing-sun' | 'missing-medium';

/** One row's derived state. */
export interface EffectState {
  readonly id: EffectId;
  /** Whether the effect actually contributes to the image. */
  readonly live: boolean;
  readonly reason: EffectReason;
  /** Whether the effect's own fields ask for it — true with `live: false` is precisely the
   *  authored-but-dead case. */
  readonly engaged: boolean;
  /** Whether the scene *authors* any of the effect's fields, as opposed to inheriting the shipped
   *  defaults. The UI dims an unauthored row's values. */
  readonly authored: boolean;
  /** The passes the graph should carry iff `live` (empty for a composite-resident effect). */
  readonly gatedPasses: readonly string[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Whether `value` equals `neutral`. Element-wise for the triples; a wrong-shaped value is never
 *  neutral (it cannot have come from a valid component, and reading it as "off" would hide it). */
function equalsNeutral(value: unknown, neutral: FieldValue): boolean {
  if (Array.isArray(neutral)) {
    return (
      Array.isArray(value) &&
      value.length === neutral.length &&
      neutral.every((element, index) => value[index] === element)
    );
  }
  return value === neutral;
}

/** The value the engine renders for `field`: what the scene authors, or the shipped default. */
export function effectiveValue(values: PostProcessValues, field: string): unknown {
  const entry = FIELDS[field];
  const authored = values[field];
  if (authored !== undefined) {
    return authored;
  }
  return entry === undefined ? undefined : entry.shipped;
}

/** Every field's effective value — the authored ones over the shipped defaults. What the chain panel
 *  hands `deriveFields` so an unauthored scene still shows the look it actually renders. */
export function effectiveValues(values: PostProcessValues): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const name of FIELD_NAMES) {
    out[name] = effectiveValue(values, name);
  }
  return out;
}

/**
 * Which effects are actually contributing, and why not when they are not.
 *
 * `values` is the live `PostProcess` (or `{}` for a scene that authors none — every field then reads
 * its shipped default, which is what the engine renders). Returns one state per row, in pinned
 * pipeline order.
 *
 * The rule is the engine's, in two halves: an effect is *engaged* when any of its gate fields differs
 * from that field's neutral, and *live* when it is engaged and every precondition outside its own
 * fields holds — the scene's (`requires`) and another effect's (`requiresEffect`). Those halves are
 * exactly `PostChain::for_scene`'s three-part shafts gate generalized (`post.shafts > 0.0 &&
 * post.fog_density > 0.0 && scene.sun.is_some()`), which is why `reconcileWithGraph` can hold this
 * model against the real graph.
 *
 * Engagement is computed for every row **before** any precondition is resolved, so `requiresEffect`
 * reads a sibling's engagement rather than depending on where that sibling sits in `EFFECT_ORDER`. It
 * reads *engaged*, not *live*, deliberately: what the shaft march needs is an authored medium, and
 * whether the fog row is itself blocked is not a question the fog row can be asked (it has no
 * precondition). Chaining `live` would make the relation transitive for no case that exists.
 */
export function deriveLiveEffects(values: PostProcessValues, ctx: ChainContext): readonly EffectState[] {
  const source: PostProcessValues = isRecord(values) ? values : {};
  const isEngaged = (row: EffectRow): boolean =>
    row.alwaysLive ||
    effectFields(row.id).some((name) => {
      const entry = FIELDS[name];
      return entry.gate && entry.neutral !== null && !equalsNeutral(effectiveValue(source, name), entry.neutral);
    });
  const engagedById = new Map<EffectId, boolean>(EFFECT_ORDER.map((row) => [row.id, isEngaged(row)]));

  return EFFECT_ORDER.map((row) => {
    const fields = effectFields(row.id);
    const authored = fields.some((name) => source[name] !== undefined);
    const engaged = engagedById.get(row.id) === true;
    const sunless = engaged && row.requires === 'sun' && !ctx.hasSun;
    const mediumless =
      engaged && row.requiresEffect !== null && engagedById.get(row.requiresEffect) !== true;
    // A missing sun outranks a missing medium when both are absent: the medium is a field on this very
    // panel and the sun is not, so naming the blocker the author cannot fix here is the more useful
    // answer — and it keeps the reason stable while they raise the density.
    const live = engaged && !sunless && !mediumless;
    const reason: EffectReason = live
      ? 'live'
      : sunless
        ? 'missing-sun'
        : mediumless
          ? 'missing-medium'
          : 'neutral';
    return { id: row.id, live, reason, engaged, authored, gatedPasses: row.gatedPasses };
  });
}

/**
 * A key over the only state a `render.dump_graph` answer can go stale against.
 *
 * A dump describes the graph the world scheduled when it was taken, so reconciling a *later* world
 * against it reports the client's own staleness as the engine disagreeing. This key is what a caller
 * compares to know whether its dump still describes the world it is looking at — and it deliberately
 * covers only the pass-gating effects, because nothing else can move the pass list, and keying on
 * every effect would spend a round trip on every fog nudge.
 */
export function gateStateKey(effects: readonly EffectState[]): string {
  return effects
    .filter((state) => state.gatedPasses.length > 0)
    .map((state) => `${state.id}:${state.live}`)
    .join('|');
}

/** One place where the chain's model and the real frame graph disagree. Either direction is a
 *  defect: a pass the model says should be there and is not means a precondition this module does not
 *  know about; the reverse means the model is stale against the engine. */
export interface GraphDisagreement {
  readonly effect: EffectId;
  readonly pass: string;
  /** What the model says: the pass should be in the graph. */
  readonly expected: boolean;
  /** What `render.dump_graph` reports. */
  readonly present: boolean;
}

/**
 * Cross-checks the derived states against the pass names `render.dump_graph` returns.
 *
 * Only the three pass-gating effects can disagree — every other effect is a branch inside a pass that
 * is always built, so the graph has nothing to say about it. An empty result is agreement; anything
 * else the panel shows, because a model that quietly diverges from the engine is worse than no model.
 */
export function reconcileWithGraph(
  effects: readonly EffectState[],
  passNames: readonly string[],
): readonly GraphDisagreement[] {
  const present = new Set(passNames);
  const out: GraphDisagreement[] = [];
  for (const state of effects) {
    for (const pass of state.gatedPasses) {
      const inGraph = present.has(pass);
      if (inGraph !== state.live) {
        out.push({ effect: state.id, pass, expected: state.live, present: inGraph });
      }
    }
  }
  return out;
}

/** How the runtime schema's fields split across the chain. */
export interface FieldBuckets {
  /** The recognised field names per effect, in the order the schema listed them. Every `EffectId` is
   *  a key, possibly with an empty list. */
  readonly byEffect: Readonly<Record<EffectId, readonly string[]>>;
  /** Fields the engine has and this table does not — a field a later engine change grew. Shown as its
   *  own row rather than dropped, which is the only reason a missing build-time gate is survivable. */
  readonly other: readonly string[];
  /** The other direction: fields this table claims and the engine's schema no longer reports. */
  readonly missing: readonly string[];
}

/**
 * Buckets the field names of `PostProcess`'s runtime schema (from `engine.describe`) into the chain's
 * effects, plus the two drift buckets.
 *
 * This is the safety net the plan's decision 11 chooses in place of a build-time check: nothing can
 * compare a TypeScript table against a Rust struct at compile time, so the tab asks the engine what
 * fields exist and refuses to lose one.
 */
export function bucketSchemaFields(names: readonly string[]): FieldBuckets {
  const byEffect = {} as Record<EffectId, string[]>;
  for (const row of EFFECT_ORDER) {
    byEffect[row.id] = [];
  }
  const other: string[] = [];
  for (const name of names) {
    const entry = FIELDS[name];
    if (entry === undefined) {
      other.push(name);
    } else {
      byEffect[entry.effect].push(name);
    }
  }
  const seen = new Set(names);
  return { byEffect, other, missing: FIELD_NAMES.filter((name) => !seen.has(name)) };
}

/** The property names of a component schema as `engine.describe` reports it (a struct schema's
 *  `properties` keys), or `[]` for any other shape — the input `bucketSchemaFields` wants, so the
 *  view stays wiring. */
export function schemaFieldNames(schema: unknown): readonly string[] {
  if (!isRecord(schema) || !isRecord(schema.properties)) {
    return [];
  }
  return Object.keys(schema.properties);
}

/**
 * The partial `PostProcess` value that switches `effect` off — every one of its fields that has a
 * neutral, at that neutral.
 *
 * Empty for the tonemap, which has no off. Non-gate neutrals ride along (`fog_height_falloff` with
 * `fog_density`): they are already inert once the gate is neutral, and writing the whole effect's
 * neutral state is one rule instead of two.
 */
export function neutralPatch(effect: EffectId): Record<string, FieldValue> {
  const out: Record<string, FieldValue> = {};
  for (const name of effectFields(effect)) {
    const { neutral } = FIELDS[name];
    if (neutral !== null) {
      out[name] = neutral;
    }
  }
  return out;
}

/** Whether an effect can be held-to-compare at all: it has something to neutralize. False only for
 *  the tonemap, and the UI disables its toggle rather than offering one that does nothing. */
export function isComparable(effect: EffectId): boolean {
  return effectFields(effect).some((name) => FIELDS[name].neutral !== null);
}
