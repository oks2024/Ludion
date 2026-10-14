// The lift/gamma/gain triples presented the way a colourist holds them: one colour and one intensity
// per band, instead of three raw channel numbers.
//
// WHY IT LIVES UNDER `editor/art/` AND NOT IN `viewer/`. Nothing here is a general fact about numeric
// vectors. It is the knowledge that these three particular `PostProcess` fields are an ASC CDL
// slope-offset-power triple, that `gamma`/`gain` are channel *scales* while `lift` is a channel
// *offset*, and that the engine's own `temperature`/`tint` axes are the right basis to express their
// chroma in. That is field-specific knowledge about one component — exactly what `chain.ts`'s
// per-field table is — so it sits beside that table rather than in the shared inspector layer, which
// must keep working for a component it has never heard of.
//
// THE STORED TRIPLE STAYS THE SOURCE OF TRUTH. This is a reversible view, not a new representation:
// `decompose` reads a triple the scene already holds and `recompose` produces a triple the engine
// already understands. No component, schema, or wire payload changes because of it.
//
// WHAT THE ENGINE ACTUALLY COMPUTES (crates/ludion-renderer/src/shaders/composite.wgsl):
//
//     out  = pow(max(color * gain + lift, 0), 1 / gamma)          // `lift_gamma_gain`
//     gain = (1+temperature, 1, 1-temperature) * (1-tint, 1+tint, 1-tint)
//     gain /= dot(gain, LUMA_709)                                 // `white_balance`
//
// The second pair of lines is the decomposition this module generalizes: the shader already splits a
// global channel scale into a luma-normalized chroma (pure colour, zero brightness change) driven by
// two bipolar axes. Applying the same two axes per band is what makes a band's `temp` mean the *same*
// direction of warmth as the global `temperature` rather than a third unrelated control.
//
// TWO ALGEBRAS, AND THEY ARE NOT INTERCHANGEABLE. `gamma` and `gain` are multiplied into the channel,
// so their neutral is `[1,1,1]` and their chroma is a ratio — the shader's own form. `lift` is added,
// so its neutral is `[0,0,0]` and a multiplicative form would be wrong in a way that matters: at
// `master = 0` the chroma would be scaled by zero and the shadows could not be tinted at all, which is
// a move colourists make constantly. `lift` therefore decomposes as an offset in the luma-null plane.
// The two forms are tied together at neutral: the additive basis vectors are the derivatives of the
// multiplicative chroma at `temp = tint = 0` (see `AMBER_AXIS`), so a small `temp` pushes all three
// bands the same direction by the same amount, and only the far ends of the travel diverge.
//
// Pure: no React, no DOM, no wire — every rule here is exercised against plain numbers in
// grade.test.ts.

import { quantize } from '../../viewer/slider';
import type { FieldRange } from '../../viewer/inspect-fields';

/** A per-channel RGB value — what all three graded fields are on the wire (`[f32; 3]`). Mutable
 *  because it is what `recompose` hands back for a `component.set` payload; the module's own constants
 *  are the readonly form below. */
export type Triple = [number, number, number];

/** A triple nobody may write into. */
export type ReadonlyTriple = readonly [number, number, number];

/** Rec.709 relative luminance, the same weights `composite.wgsl` uses. The components sum to exactly
 *  `1` in IEEE double, which is load-bearing twice over: it makes `dot(ones, LUMA_709)` exactly `1`,
 *  so a grey triple's master is its own channel value, and it makes the additive band's neutral
 *  round-trip exactly rather than drifting by a scale factor. */
export const LUMA_709: ReadonlyTriple = [0.2126, 0.7152, 0.0722];

function dotLuma(a: number, b: number, c: number): number {
  return a * LUMA_709[0] + b * LUMA_709[1] + c * LUMA_709[2];
}

function dot3(u: ReadonlyTriple, v: ReadonlyTriple): number {
  return u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
}

/** `v` with its luma projected out, i.e. the component of `v` that carries colour and no brightness.
 *  `dot(lumaFree(v), LUMA_709)` is zero because `dot(ones, LUMA_709)` is one. */
function lumaFree(v: ReadonlyTriple): ReadonlyTriple {
  const k = dotLuma(v[0], v[1], v[2]);
  return [v[0] - k, v[1] - k, v[2] - k];
}

/**
 * The warm↔cool axis of the additive band, and the **derivative of the multiplicative chroma with
 * respect to `temp` at neutral**. That identity is the whole reason the constant is written this way
 * rather than as three literals.
 *
 * The multiplicative chroma is `p(t,u) / dot(p(t,u), LUMA)` with `p = (1+t, 1, 1-t) * (1-u, 1+u, 1-u)`.
 * At `t = u = 0`, `p = ones` and `dot(p, LUMA) = 1`, so the quotient rule leaves
 * `∂chroma/∂t = ∂p/∂t - ones * dot(∂p/∂t, LUMA)` — the luma-free part of `∂p/∂t = (1, 0, -1)`.
 *
 * Replacing this with any other spanning pair of the luma-null plane would still invert `lift`
 * exactly, and would silently stop a given `temp` from meaning the same warmth in `lift` as it does
 * in `gain`. grade.test.ts differentiates the multiplicative form numerically to pin it.
 */
export const AMBER_AXIS: ReadonlyTriple = lumaFree([1, 0, -1]);

/** The magenta↔green axis of the additive band: likewise `∂chroma/∂u` at neutral, the luma-free part
 *  of `∂p/∂u = (-1, 1, -1)`. */
export const GREEN_AXIS: ReadonlyTriple = lumaFree([-1, 1, -1]);

// The Gram matrix of the (non-orthogonal) basis, and its determinant. `AMBER_AXIS` and `GREEN_AXIS`
// are independent and span the 2-D luma-null plane, so an offset that lies in that plane — which every
// `v - master * ones` does by construction — is reproduced *exactly* by this system. It is a change of
// basis, not a least-squares fit. The determinant is a compile-time-constant ~8.99, far from zero, so
// the solve needs no conditioning guard.
const GRAM_AA = dot3(AMBER_AXIS, AMBER_AXIS);
const GRAM_AG = dot3(AMBER_AXIS, GREEN_AXIS);
const GRAM_GG = dot3(GREEN_AXIS, GREEN_AXIS);
const GRAM_DET = GRAM_AA * GRAM_GG - GRAM_AG * GRAM_AG;

/** Which algebra a band's channels combine by — the one fact that decides its neutral, its chroma
 *  form, and whether `master = 0` is a usable working point. */
export type GradeAlgebra = 'multiplicative' | 'additive';

/** The `PostProcess` fields this module models. */
export type GradeBandId = 'lift' | 'gamma' | 'gain';

/** One graded band: which field it is and which algebra it obeys. */
export interface GradeBand {
  readonly id: GradeBandId;
  readonly label: string;
  readonly algebra: GradeAlgebra;
  /** The triple that means "this band does nothing" — `[0,0,0]` additive, `[1,1,1]` multiplicative. */
  readonly neutral: ReadonlyTriple;
}

/**
 * The three bands, in the shadows→midtones→highlights order a colourist works them, which is also the
 * order `lift_gamma_gain`'s signature lists them. Deliberately not schema order (`lift`, `gamma`,
 * `gain` happen to agree, but the ordering here is the contract, not a coincidence to rely on).
 */
export const GRADE_BANDS: readonly GradeBand[] = [
  { id: 'lift', label: 'Lift', algebra: 'additive', neutral: [0, 0, 0] },
  { id: 'gamma', label: 'Gamma', algebra: 'multiplicative', neutral: [1, 1, 1] },
  { id: 'gain', label: 'Gain', algebra: 'multiplicative', neutral: [1, 1, 1] },
];

/** The band a `PostProcess` field name names, or `null` for every other field. */
export function gradeBand(field: string): GradeBand | null {
  return GRADE_BANDS.find((band) => band.id === field) ?? null;
}

/** A band split into the two things a human directs: how much of it, and what colour it is. */
export interface GradeSplit {
  /** The band's intensity — its Rec.709 luma. `1` is neutral multiplicative, `0` neutral additive. */
  readonly master: number;
  /** Warm↔cool, the same axis and the same numbers as `PostProcess.temperature`. */
  readonly temp: number;
  /** Green↔magenta, the same axis and the same numbers as `PostProcess.tint`. */
  readonly tint: number;
}

/** The split that means "this band does nothing". */
export function neutralSplit(band: GradeBand): GradeSplit {
  return { master: band.algebra === 'additive' ? 0 : 1, temp: 0, tint: 0 };
}

/**
 * The floor a chroma denominator must clear before it is divided by.
 *
 * A decomposed chroma is normalized (`dot(chroma, LUMA_709) == 1`), so both denominators are O(1) for
 * any grade worth editing and this floor is nine orders below them. It is not a tuning knob: it exists
 * so the two genuinely singular inputs — a pure-green chroma, where `temp` has no meaning at all, and
 * a malformed triple that lands on either zero — produce `0` instead of `NaN`.
 */
const DENOM_FLOOR = 1e-9;

/** Whether `v` is a usable component value: exactly three finite numbers. Anything else (a short
 *  array, a string element, a `NaN` left by a bad parse) is a malformed value the widget layer must
 *  survive rather than throw on. */
function isTriple(v: readonly number[]): v is ReadonlyTriple {
  return v.length === 3 && Number.isFinite(v[0]) && Number.isFinite(v[1]) && Number.isFinite(v[2]);
}

function finiteOr(value: number, fallback: number): number {
  return Number.isFinite(value) ? value : fallback;
}

/**
 * Splits a triple into intensity plus colour.
 *
 * Fail-soft throughout, because a malformed component value reaches here as readily as a good one:
 * a `v` that is not three finite numbers decomposes to the band's neutral, and a chroma whose axis is
 * undefined decomposes to `temp = tint = 0` rather than to a non-finite number. The returned split is
 * always three finite numbers.
 *
 * A multiplicative band with `master <= 0` is one of those undefined cases. `master = 0` is the honest
 * one — `[0,0,0]` gain is black, and black has no hue to recover. A negative master is *algebraically*
 * invertible, but a negative `gamma`/`gain` channel is a value the engine's semantic validator rejects
 * (`L1130`), so reading a colour out of it would be inventing meaning for a value that cannot render.
 * Both keep the master they had, so the brightness still round-trips even where the colour cannot.
 */
export function decompose(band: GradeBand, v: readonly number[]): GradeSplit {
  if (!isTriple(v)) {
    return neutralSplit(band);
  }
  const master = dotLuma(v[0], v[1], v[2]);
  if (!Number.isFinite(master)) {
    return neutralSplit(band);
  }

  if (band.algebra === 'additive') {
    // `offset` lies in the luma-null plane by construction, so the Gram solve below is exact.
    const offset: Triple = [v[0] - master, v[1] - master, v[2] - master];
    const ra = dot3(AMBER_AXIS, offset);
    const rg = dot3(GREEN_AXIS, offset);
    return {
      master,
      temp: finiteOr((ra * GRAM_GG - rg * GRAM_AG) / GRAM_DET, 0),
      tint: finiteOr((rg * GRAM_AA - ra * GRAM_AG) / GRAM_DET, 0),
    };
  }

  if (master <= 0) {
    return { master, temp: 0, tint: 0 };
  }
  // Normalizing by the master is what makes the two ratios below independent of intensity: they are
  // the ratios of `p = amber * green` itself, which the shader's own normalizer preserves.
  const c0 = v[0] / master;
  const c1 = v[1] / master;
  const c2 = v[2] / master;
  const outer = c0 + c2;
  const spread = 2 * c1 + outer;
  return {
    master,
    temp: Math.abs(outer) > DENOM_FLOOR ? finiteOr((c0 - c2) / outer, 0) : 0,
    tint: Math.abs(spread) > DENOM_FLOOR ? finiteOr((2 * c1 - outer) / spread, 0) : 0,
  };
}

/**
 * Rebuilds the triple a split describes — the exact inverse of {@link decompose} wherever the colour
 * is defined at all.
 *
 * Never emits a non-finite number. A non-finite *input* falls back to the band's neutral for that
 * control. A finite input that overflows on the way out — a huge chroma on either algebra, or a
 * multiplicative normalizer that reaches zero, which it cannot inside the `[-1, 1]` travel box (every
 * channel of `amber * green` is non-negative there and the normalizer bottoms out near `0.29`) but can
 * outside it — falls back to a neutral grey at the requested master, keeping the intensity where the
 * colour is unrepresentable.
 */
export function recompose(band: GradeBand, split: GradeSplit): Triple {
  const neutral = neutralSplit(band);
  const master = finiteOr(split.master, neutral.master);
  const temp = finiteOr(split.temp, 0);
  const tint = finiteOr(split.tint, 0);
  const grey: Triple = [master, master, master];

  if (band.algebra === 'additive') {
    // The grey level plus the luma-free colour, which is the whole value — not the offset alone, as
    // it is on the way in.
    const shifted: Triple = [
      master + temp * AMBER_AXIS[0] + tint * GREEN_AXIS[0],
      master + temp * AMBER_AXIS[1] + tint * GREEN_AXIS[1],
      master + temp * AMBER_AXIS[2] + tint * GREEN_AXIS[2],
    ];
    return isTriple(shifted) ? shifted : grey;
  }

  // `white_balance`'s product, verbatim: two commuting diagonal scales, normalized so the result
  // carries exactly the master's luma and none of its own.
  const p0 = (1 + temp) * (1 - tint);
  const p1 = 1 + tint;
  const p2 = (1 - temp) * (1 - tint);
  const norm = dotLuma(p0, p1, p2);
  if (!Number.isFinite(norm) || norm === 0) {
    return grey;
  }
  // Associated as `color * gain / dot(gain, LUMA)`, which is the shader's own expression order, so a
  // band recomposes to the bits `white_balance` would produce for a grey of this luma rather than to
  // a value that merely rounds to them.
  const scaled: Triple = [(master * p0) / norm, (master * p1) / norm, (master * p2) / norm];
  return isTriple(scaled) ? scaled : grey;
}

/** {@link recompose} rounded the way a control's write is rounded elsewhere in the editor — 3
 *  significant digits, so the triple that lands in the scene file reads like an authored number and
 *  produces a minimal diff instead of carrying the picker's float noise. The rounding costs the split
 *  about 0.1% relative, which is why it is a separate function: the arithmetic above stays exactly
 *  invertible and only the write path gives that up. */
export function recomposeForWrite(band: GradeBand, split: GradeSplit): Triple {
  const v = recompose(band, split);
  return [quantize(v[0]), quantize(v[1]), quantize(v[2])];
}

/**
 * The travel of the two chroma controls, shared by all three bands.
 *
 * WHY `[-1, 1]`. It is what `PostProcess.temperature`/`tint` already declare (`range(min=-1, max=1)`
 * in components.rs), and reusing it is the point: a band's `temp` of `0.3` should read as the same
 * amount of warm as the global `temperature` of `0.3`, which it does exactly at neutral and closely
 * across the box. The bound is also derivable rather than taste — `amber * green` has a negative
 * channel outside `|temp| <= 1, |tint| <= 1` and nowhere inside it, so the box is precisely the region
 * where a multiplicative band recomposes to a triple the semantic validator accepts (`L1130`).
 *
 * WHY NO HARD BOUND HERE. `hardMin`/`hardMax` are the *schema's* legal bounds, and `temp`/`tint` are
 * not schema fields — the stored triple is. The legality that matters is enforced where it exists, on
 * the components the recomposed triple is written as; declaring a bound here would let a drag clamp a
 * chroma back inside the box while `widen` had deliberately opened the track past it for an authored
 * value that already sits outside.
 */
export const CHROMA_TRAVEL: FieldRange = { min: -1, max: 1, hardMin: null, hardMax: null };

/** Travel for one band's three controls. `master` is `null` when the field declares none — the same
 *  signal `NumberField.range` carries, and the same answer: render a plain number box, not a track. */
export interface GradeTravel {
  readonly master: FieldRange | null;
  readonly temp: FieldRange;
  readonly tint: FieldRange;
}

/**
 * The travel for a band's controls, given the range the field's own schema declares.
 *
 * The master's travel is the field's `x-ui-range` unchanged, taken as a parameter rather than restated
 * here: `lift`'s `[-0.5, 0.5]`, `gamma`'s `[0.5, 2.0]` and `gain`'s `[0.0, 2.0]` are already the right
 * throws for an intensity, because per-channel travel and luma travel are the same span — the master
 * of a uniform triple *is* its channel value. Restating them would be a second copy of the schema, and
 * a copy that goes stale the first time a range is retuned in Rust.
 */
export function gradeTravel(masterRange: FieldRange | null): GradeTravel {
  return { master: masterRange, temp: CHROMA_TRAVEL, tint: CHROMA_TRAVEL };
}
