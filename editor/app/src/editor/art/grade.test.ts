import { describe, expect, it } from 'vitest';

import {
  AMBER_AXIS,
  CHROMA_TRAVEL,
  decompose,
  GRADE_BANDS,
  gradeBand,
  gradeTravel,
  GREEN_AXIS,
  LUMA_709,
  neutralSplit,
  recompose,
  recomposeForWrite,
  type GradeBand,
  type GradeSplit,
  type Triple,
} from './grade';
import type { FieldRange } from '../../viewer/inspect-fields';

// --- an independent reference, deliberately not built from the module under test ------------------
//
// `composite.wgsl`'s `white_balance`, transcribed. Every constant and every operation is restated
// here rather than imported: a reference that shares code with its subject asserts that the code
// equals itself.

const LUMA: readonly [number, number, number] = [0.2126, 0.7152, 0.0722];

function refDot(v: readonly number[]): number {
  return v[0] * LUMA[0] + v[1] * LUMA[1] + v[2] * LUMA[2];
}

/** `white_balance(color, temperature, tint)`, verbatim. */
function refWhiteBalance(color: readonly number[], temperature: number, tint: number): Triple {
  const amber = [1 + temperature, 1, 1 - temperature];
  const green = [1 - tint, 1 + tint, 1 - tint];
  const gain = [amber[0] * green[0], amber[1] * green[1], amber[2] * green[2]];
  const norm = refDot(gain);
  return [
    (color[0] * gain[0]) / norm,
    (color[1] * gain[1]) / norm,
    (color[2] * gain[2]) / norm,
  ];
}

/** The normalized chroma the multiplicative decomposition parameterizes: `white_balance` applied to
 *  pure white. Its derivatives at neutral are what the additive basis claims to be. */
function refChroma(temperature: number, tint: number): Triple {
  return refWhiteBalance([1, 1, 1], temperature, tint);
}

// --- helpers -------------------------------------------------------------------------------------

const LIFT = GRADE_BANDS[0];
const GAMMA = GRADE_BANDS[1];
const GAIN = GRADE_BANDS[2];
const MULTIPLICATIVE: readonly GradeBand[] = [GAMMA, GAIN];

/**
 * The round-trip tolerance, relative with an absolute floor of the same size.
 *
 * `2e-15` is ~9 IEEE-double ulps. Both inversions are a handful of multiplies, divides and one 2x2
 * solve over well-conditioned constants, so the achievable error is a few ulps and nothing more: the
 * measured worst over every sweep below is 5.6e-16 (~2.5 ulps, on a multiplicative triple with a 40:1
 * channel ratio), and the additive band peaks at 4.4e-16. JavaScript arithmetic is exactly specified
 * with no fused multiply-add, so those numbers do not move between machines and the ~4x headroom is
 * margin against a legitimate refactor, not against platform drift.
 *
 * It is deliberately NOT a loose "close enough" epsilon. The whole claim of this module is that the
 * view is reversible; a 1e-6 tolerance would pass a sign error in the third decimal of a slider.
 */
const ROUND_TRIP_TOL = 2e-15;

/** Error of `a` against `b`, relative once `|b|` exceeds 1 so the sweeps can share one bound across
 *  values spanning `[0.05, 2]` and `[-0.6, 0.6]`. */
function relError(a: number, b: number): number {
  return Math.abs(a - b) / Math.max(1, Math.abs(b));
}

function tripleError(a: readonly number[], b: readonly number[]): number {
  return Math.max(relError(a[0], b[0]), relError(a[1], b[1]), relError(a[2], b[2]));
}

/** `count` values spanning `[lo, hi]` inclusive. */
function sweep(lo: number, hi: number, count: number): number[] {
  return Array.from({ length: count }, (_, i) => lo + ((hi - lo) * i) / (count - 1));
}

// --- 1. round trip -------------------------------------------------------------------------------

describe('round trip', () => {
  it('recompose(decompose(v)) reproduces v for a swept grid of multiplicative triples', () => {
    // Positive channels only: `gamma` is validated `> 0` and `gain` `>= 0` (`L1130`), and a
    // multiplicative chroma is only defined where the master is positive. The zero and negative
    // corners are the degenerate cases below, not round-trip cases.
    const values = sweep(0.05, 2.0, 13);
    let worst = 0;
    let worstAt: Triple = [0, 0, 0];
    for (const band of MULTIPLICATIVE) {
      for (const r of values) {
        for (const g of values) {
          for (const b of values) {
            const v: Triple = [r, g, b];
            const error = tripleError(recompose(band, decompose(band, v)), v);
            if (error > worst) {
              worst = error;
              worstAt = v;
            }
          }
        }
      }
    }
    expect(worst, `worst triple ${JSON.stringify(worstAt)}`).toBeLessThan(ROUND_TRIP_TOL);
  });

  it('recompose(decompose(v)) reproduces v for a swept grid of additive triples', () => {
    // `lift` is unbounded and routinely negative (crushing the blacks), so the grid straddles zero and
    // extends past the declared `[-0.5, 0.5]` travel on both sides.
    const values = sweep(-0.6, 0.6, 13);
    let worst = 0;
    for (const r of values) {
      for (const g of values) {
        for (const b of values) {
          const v: Triple = [r, g, b];
          worst = Math.max(worst, tripleError(recompose(LIFT, decompose(LIFT, v)), v));
        }
      }
    }
    expect(worst).toBeLessThan(ROUND_TRIP_TOL);
  });

  it('decompose(recompose(split)) reproduces the split over the whole travel box', () => {
    const masters = { multiplicative: sweep(0.1, 2.0, 7), additive: sweep(-0.5, 0.5, 7) };
    // The chroma grid stops just short of `tint = 1`: there the multiplicative chroma is pure green
    // and `temp` is genuinely unrecoverable (see the degenerate cases), which is information loss in
    // the model, not an inversion defect.
    const chroma = sweep(-0.99, 0.99, 11);
    let worst = 0;
    for (const band of GRADE_BANDS) {
      for (const master of masters[band.algebra]) {
        for (const temp of chroma) {
          for (const tint of chroma) {
            const split: GradeSplit = { master, temp, tint };
            const back = decompose(band, recompose(band, split));
            worst = Math.max(
              worst,
              relError(back.master, master),
              relError(back.temp, temp),
              relError(back.tint, tint),
            );
          }
        }
      }
    }
    expect(worst).toBeLessThan(ROUND_TRIP_TOL);
  });
});

// --- 2. neutral ----------------------------------------------------------------------------------

describe('neutral', () => {
  it('maps each band neutral to a neutral split', () => {
    for (const band of GRADE_BANDS) {
      expect(decompose(band, [...band.neutral])).toEqual(neutralSplit(band));
    }
    expect(neutralSplit(LIFT)).toEqual({ master: 0, temp: 0, tint: 0 });
    expect(neutralSplit(GAMMA)).toEqual({ master: 1, temp: 0, tint: 0 });
    expect(neutralSplit(GAIN)).toEqual({ master: 1, temp: 0, tint: 0 });
  });

  it('maps a neutral split back to the band neutral, exactly', () => {
    for (const band of GRADE_BANDS) {
      expect(recompose(band, neutralSplit(band))).toEqual([...band.neutral]);
    }
    expect(recompose(GAIN, { master: 1, temp: 0, tint: 0 })).toEqual([1, 1, 1]);
    expect(recompose(LIFT, { master: 0, temp: 0, tint: 0 })).toEqual([0, 0, 0]);
  });

  it('leaves a grey triple achromatic', () => {
    expect(decompose(GAIN, [0.5, 0.5, 0.5])).toEqual({ master: 0.5, temp: 0, tint: 0 });
    expect(decompose(LIFT, [-0.2, -0.2, -0.2])).toEqual({ master: -0.2, temp: 0, tint: 0 });
  });
});

// --- 3. the "colour + intensity" claim -----------------------------------------------------------

describe('chroma carries no brightness', () => {
  it('holds the master luma across the whole multiplicative chroma box', () => {
    let worst = 0;
    for (const band of MULTIPLICATIVE) {
      for (const master of sweep(0.1, 2.0, 5)) {
        for (const temp of sweep(-1, 1, 21)) {
          for (const tint of sweep(-1, 1, 21)) {
            const v = recompose(band, { master, temp, tint });
            worst = Math.max(worst, relError(refDot(v), master));
          }
        }
      }
    }
    expect(worst).toBeLessThan(ROUND_TRIP_TOL);
  });

  it('holds the master luma across the whole additive chroma box', () => {
    let worst = 0;
    for (const master of sweep(-0.5, 0.5, 5)) {
      for (const temp of sweep(-1, 1, 21)) {
        for (const tint of sweep(-1, 1, 21)) {
          const v = recompose(LIFT, { master, temp, tint });
          worst = Math.max(worst, relError(refDot(v), master));
        }
      }
    }
    expect(worst).toBeLessThan(ROUND_TRIP_TOL);
  });

  it('leaves the master untouched when only the chroma moves', () => {
    for (const band of GRADE_BANDS) {
      const master = band.algebra === 'additive' ? 0.25 : 1.3;
      const base = decompose(band, recompose(band, { master, temp: 0, tint: 0 }));
      const shifted = decompose(band, recompose(band, { master, temp: -0.4, tint: 0.6 }));
      expect(shifted.master).toBeCloseTo(base.master, 12);
    }
  });
});

// --- 4. the additive basis is luma-free ----------------------------------------------------------

describe('additive basis', () => {
  it('spans the luma-null plane', () => {
    expect(refDot(AMBER_AXIS)).toBeCloseTo(0, 15);
    expect(refDot(GREEN_AXIS)).toBeCloseTo(0, 15);
  });

  it('is the luma-free part of the raw warm and green directions', () => {
    // Pinned as literals as well as by construction: this is what stops a "simplification" to
    // (1,0,-1)/(-1,1,-1) — which would make lift's temp a brightness control — from passing.
    expect(AMBER_AXIS[0]).toBeCloseTo(0.8596, 12);
    expect(AMBER_AXIS[1]).toBeCloseTo(-0.1404, 12);
    expect(AMBER_AXIS[2]).toBeCloseTo(-1.1404, 12);
    expect(GREEN_AXIS[0]).toBeCloseTo(-1.4304, 12);
    expect(GREEN_AXIS[1]).toBeCloseTo(0.5696, 12);
    expect(GREEN_AXIS[2]).toBeCloseTo(-1.4304, 12);
  });

  it('is linearly independent, so the change of basis is exact rather than a fit', () => {
    const aa = AMBER_AXIS[0] ** 2 + AMBER_AXIS[1] ** 2 + AMBER_AXIS[2] ** 2;
    const gg = GREEN_AXIS[0] ** 2 + GREEN_AXIS[1] ** 2 + GREEN_AXIS[2] ** 2;
    const ag =
      AMBER_AXIS[0] * GREEN_AXIS[0] +
      AMBER_AXIS[1] * GREEN_AXIS[1] +
      AMBER_AXIS[2] * GREEN_AXIS[2];
    expect(aa * gg - ag * ag).toBeGreaterThan(1);
  });

  it('sums the Rec.709 weights to exactly one, which is what makes the additive master exact', () => {
    expect(LUMA_709[0] + LUMA_709[1] + LUMA_709[2]).toBe(1);
  });
});

// --- 5. the basis IS the multiplicative tangent ---------------------------------------------------

describe('the two algebras agree at neutral', () => {
  // Central differences of the independent `refChroma`. h = 1e-5 keeps the O(h^2) truncation and the
  // O(eps/h) cancellation both near 1e-10, so 1e-8 is a bound neither can reach by accident.
  const H = 1e-5;

  function centralDiff(f: (x: number) => Triple): Triple {
    const plus = f(H);
    const minus = f(-H);
    return [
      (plus[0] - minus[0]) / (2 * H),
      (plus[1] - minus[1]) / (2 * H),
      (plus[2] - minus[2]) / (2 * H),
    ];
  }

  it('d(chroma)/d(temp) at neutral is the amber axis', () => {
    const derivative = centralDiff((t) => refChroma(t, 0));
    for (let i = 0; i < 3; i += 1) {
      expect(derivative[i]).toBeCloseTo(AMBER_AXIS[i], 8);
    }
  });

  it('d(chroma)/d(tint) at neutral is the green axis', () => {
    const derivative = centralDiff((u) => refChroma(0, u));
    for (let i = 0; i < 3; i += 1) {
      expect(derivative[i]).toBeCloseTo(GREEN_AXIS[i], 8);
    }
  });

  it('makes a small temp mean the same warmth in every band', () => {
    // The consequence the tangent identity buys: the same small chroma push moves lift's offset and
    // gain's ratio along the same direction by the same amount.
    const h = 1e-4;
    const lift = recompose(LIFT, { master: 0, temp: h, tint: 0 });
    const gain = recompose(GAIN, { master: 1, temp: h, tint: 0 });
    for (let i = 0; i < 3; i += 1) {
      expect(gain[i] - 1).toBeCloseTo(lift[i], 7);
    }
  });
});

// --- 6. degenerate inputs -------------------------------------------------------------------------

describe('degenerate decomposition', () => {
  it('returns the band neutral for a value that is not three finite numbers', () => {
    const malformed: readonly number[][] = [
      [],
      [1, 1],
      [1, 1, 1, 1],
      [Number.NaN, 1, 1],
      [1, Number.POSITIVE_INFINITY, 1],
      [1, 1, Number.NEGATIVE_INFINITY],
    ];
    for (const band of GRADE_BANDS) {
      for (const v of malformed) {
        expect(decompose(band, v)).toEqual(neutralSplit(band));
      }
    }
  });

  it('drops the chroma of a multiplicative band whose master is zero, keeping the master', () => {
    for (const band of MULTIPLICATIVE) {
      expect(decompose(band, [0, 0, 0])).toEqual({ master: 0, temp: 0, tint: 0 });
      // Black has no hue: a zero master round-trips as black whatever chroma is asked for.
      expect(recompose(band, { master: 0, temp: 0.5, tint: -0.5 })).toEqual([0, 0, 0]);
    }
  });

  it('drops the chroma of a multiplicative band whose master is negative', () => {
    // Algebraically invertible, deliberately not inverted: a negative gamma/gain channel is a value
    // the engine rejects (L1130), so there is no colour there to report.
    const split = decompose(GAIN, [-1, -1, -1]);
    expect(split.temp).toBe(0);
    expect(split.tint).toBe(0);
    expect(split.master).toBeCloseTo(-1, 15);
    expect(decompose(GAIN, [-0.5, -1, -2]).temp).toBe(0);
  });

  it('still recovers tint from a pure-green chroma, where only temp is undefined', () => {
    // v = [0, 1, 0]: the `c0 + c2` denominator is exactly zero. That IS `tint = 1` — the far end of
    // the travel — and temp has no meaning there, so the guarded answer is also the correct one and
    // the value round-trips.
    const split = decompose(GAIN, [0, 1, 0]);
    expect(split.temp).toBe(0);
    expect(split.tint).toBeCloseTo(1, 12);
    const back = recompose(GAIN, split);
    expect(back[0]).toBeCloseTo(0, 15);
    expect(back[1]).toBeCloseTo(1, 12);
    expect(back[2]).toBeCloseTo(0, 15);
  });

  it('zeroes temp when the outer denominator vanishes on a value outside the model', () => {
    // c0 = -c2 with a non-zero mid channel is unreachable by any (temp, tint): it is a malformed
    // triple, and the answer is a defined split rather than a NaN.
    const split = decompose(GAIN, [1, 5, -1]);
    expect(split.temp).toBe(0);
    expect(Number.isFinite(split.tint)).toBe(true);
    expect(split.master).toBeCloseTo(3.7164, 12);
  });

  it('zeroes tint when the spread denominator vanishes', () => {
    // 2*c1 + c0 + c2 == 0 exactly, with a positive master: the green direction pushed to infinity.
    const split = decompose(GAIN, [-1, 1, -1]);
    expect(split.tint).toBe(0);
    expect(split.temp).toBeCloseTo(0, 15);
    expect(Number.isFinite(split.master)).toBe(true);
  });

  it('never produces a non-finite split', () => {
    const wild = [0, 1e-300, 1e300, -1e300, 1, -1, 1e-12];
    for (const band of GRADE_BANDS) {
      for (const r of wild) {
        for (const g of wild) {
          for (const b of wild) {
            const split = decompose(band, [r, g, b]);
            expect(Number.isFinite(split.master)).toBe(true);
            expect(Number.isFinite(split.temp)).toBe(true);
            expect(Number.isFinite(split.tint)).toBe(true);
          }
        }
      }
    }
  });
});

describe('degenerate recomposition', () => {
  it('substitutes the band neutral for a non-finite control', () => {
    expect(recompose(GAIN, { master: Number.NaN, temp: 0, tint: 0 })).toEqual([1, 1, 1]);
    expect(recompose(LIFT, { master: Number.NaN, temp: 0, tint: 0 })).toEqual([0, 0, 0]);
    expect(recompose(GAIN, { master: 1, temp: Number.NaN, tint: Number.NaN })).toEqual([1, 1, 1]);
    expect(recompose(LIFT, { master: 0, temp: Number.POSITIVE_INFINITY, tint: 0 })).toEqual([
      0, 0, 0,
    ]);
  });

  it('falls back to a neutral grey when the multiplicative normalizer is unusable', () => {
    // The product overflows before the normalizer can be formed.
    expect(recompose(GAIN, { master: 1, temp: 1e200, tint: 1e200 })).toEqual([1, 1, 1]);
  });

  it('never produces a non-finite triple, including around the normalizer root', () => {
    // The multiplicative normalizer `dot(amber * green, LUMA)` passes through zero near temp = -7.12
    // (with tint = 0) and near tint = -2.32 (with temp = 0) — both far outside the travel box, both
    // reachable if a track is widened onto an authored value.
    const wild = [
      0,
      1,
      -1,
      2,
      -2.3234,
      -2.32342007434,
      -7.1225,
      -7.12250712250712,
      1e-300,
      1e300,
      -1e300,
      Number.MAX_VALUE,
    ];
    for (const band of GRADE_BANDS) {
      for (const master of [0, 1, -1, 1e300, -1e300]) {
        for (const temp of wild) {
          for (const tint of wild) {
            const v = recompose(band, { master, temp, tint });
            expect(v).toHaveLength(3);
            expect(Number.isFinite(v[0])).toBe(true);
            expect(Number.isFinite(v[1])).toBe(true);
            expect(Number.isFinite(v[2])).toBe(true);
          }
        }
      }
    }
  });
});

// --- 7. agreement with the shader -----------------------------------------------------------------

describe('agreement with composite.wgsl', () => {
  it('recomposes a multiplicative band exactly as white_balance scales a grey of that luma', () => {
    // The claim under test: a band's (master, temp, tint) is the shader's own white balance applied
    // to a neutral of luminance `master`. Same axes, same normalizer, same numbers — bit-identical,
    // not merely close, because `recompose` keeps the shader's `color * gain / dot(gain, LUMA)`
    // association rather than pre-dividing.
    let worst = 0;
    for (const band of MULTIPLICATIVE) {
      for (const master of sweep(0.05, 2.0, 9)) {
        for (const temp of sweep(-1, 1, 17)) {
          for (const tint of sweep(-1, 1, 17)) {
            const mine = recompose(band, { master, temp, tint });
            const reference = refWhiteBalance([master, master, master], temp, tint);
            worst = Math.max(worst, tripleError(mine, reference));
          }
        }
      }
    }
    expect(worst).toBe(0);
  });

  it('recovers the temperature and tint the shader was given', () => {
    let worst = 0;
    for (const temperature of sweep(-0.95, 0.95, 21)) {
      for (const tint of sweep(-0.95, 0.95, 21)) {
        const balanced = refWhiteBalance([1.4, 1.4, 1.4], temperature, tint);
        const split = decompose(GAIN, balanced);
        worst = Math.max(
          worst,
          relError(split.master, 1.4),
          relError(split.temp, temperature),
          relError(split.tint, tint),
        );
      }
    }
    expect(worst).toBeLessThan(ROUND_TRIP_TOL);
  });

  it('matches the shader on a hand-computed warm gain', () => {
    // A worked example, so a wholesale rewrite of both sides cannot agree with itself: gain
    // [1.2, 1.0, 0.8] is a mild warm push with no green/magenta content.
    const split = decompose(GAIN, [1.2, 1.0, 0.8]);
    expect(split.master).toBeCloseTo(1.02808, 12);
    expect(split.temp).toBeCloseTo(0.2, 12);
    expect(split.tint).toBeCloseTo(0, 12);
  });
});

// --- the band table and the travel ----------------------------------------------------------------

describe('bands', () => {
  it('names the three graded fields and nothing else', () => {
    expect(GRADE_BANDS.map((b) => b.id)).toEqual(['lift', 'gamma', 'gain']);
    expect(gradeBand('lift')).toBe(LIFT);
    expect(gradeBand('gamma')).toBe(GAMMA);
    expect(gradeBand('gain')).toBe(GAIN);
    for (const field of ['temperature', 'tint', 'saturation', 'contrast', 'bloom_strength', '']) {
      expect(gradeBand(field)).toBeNull();
    }
  });

  it('pairs each band with the algebra and neutral its field actually uses', () => {
    expect(LIFT.algebra).toBe('additive');
    expect(LIFT.neutral).toEqual([0, 0, 0]);
    expect(GAMMA.algebra).toBe('multiplicative');
    expect(GAMMA.neutral).toEqual([1, 1, 1]);
    expect(GAIN.algebra).toBe('multiplicative');
    expect(GAIN.neutral).toEqual([1, 1, 1]);
  });
});

describe('travel', () => {
  // The ranges `deriveFields` produces for the three fields, from their `x-ui-range` in
  // crates/ludion-engine/src/components.rs. Restated here as *inputs* to the function under test:
  // the module must not carry its own copy.
  const LIFT_RANGE: FieldRange = { min: -0.5, max: 0.5, hardMin: null, hardMax: null };
  const GAMMA_RANGE: FieldRange = { min: 0.5, max: 2.0, hardMin: null, hardMax: null };
  const GAIN_RANGE: FieldRange = { min: 0.0, max: 2.0, hardMin: null, hardMax: null };

  it('passes the field range through as the master travel', () => {
    expect(gradeTravel(LIFT_RANGE).master).toBe(LIFT_RANGE);
    expect(gradeTravel(GAMMA_RANGE).master).toBe(GAMMA_RANGE);
    expect(gradeTravel(GAIN_RANGE).master).toBe(GAIN_RANGE);
  });

  it('reports no master travel when the field declares none', () => {
    expect(gradeTravel(null).master).toBeNull();
  });

  it('gives every band the same bipolar chroma travel, with no schema bound of its own', () => {
    for (const range of [LIFT_RANGE, GAMMA_RANGE, GAIN_RANGE, null]) {
      const travel = gradeTravel(range);
      expect(travel.temp).toEqual({ min: -1, max: 1, hardMin: null, hardMax: null });
      expect(travel.tint).toEqual(travel.temp);
    }
    expect(CHROMA_TRAVEL.min).toBe(-1);
    expect(CHROMA_TRAVEL.max).toBe(1);
  });

  it('is exactly the box where a multiplicative band stays non-negative', () => {
    // The justification for [-1, 1], asserted rather than asserted-in-prose: inside the box every
    // channel of a positive-master recompose is >= 0 (what L1130 requires of gamma and gain), and
    // just outside it on any of the four sides, one is not.
    for (const temp of sweep(-1, 1, 21)) {
      for (const tint of sweep(-1, 1, 21)) {
        const v = recompose(GAIN, { master: 1, temp, tint });
        expect(Math.min(v[0], v[1], v[2])).toBeGreaterThanOrEqual(-1e-15);
      }
    }
    for (const outside of [
      { temp: 1.1, tint: 0 },
      { temp: -1.1, tint: 0 },
      { temp: 0, tint: 1.1 },
      { temp: 0, tint: -1.1 },
    ]) {
      const v = recompose(GAIN, { master: 1, ...outside });
      expect(Math.min(v[0], v[1], v[2])).toBeLessThan(0);
    }
  });
});

// --- the write path -------------------------------------------------------------------------------

describe('recomposeForWrite', () => {
  it('rounds to the editor-wide 3 significant digits', () => {
    expect(recomposeForWrite(GAIN, { master: 1, temp: 0.2, tint: 0 })).toEqual([1.17, 0.973, 0.778]);
    expect(recomposeForWrite(LIFT, { master: 0, temp: 0, tint: 0 })).toEqual([0, 0, 0]);
  });

  it('costs the split less than one stop of the chroma track', () => {
    // What the rounding buys is a minimal git diff in the scene file; what it costs is that a handle
    // re-read from the stored triple lands slightly off where it was released. The worst measured
    // drift is 3.15e-3 (on a multiplicative band around temp = -0.15, where a master of 1.4 puts the
    // channels near 1.2 and 3 significant digits are only 2 decimals), against a `niceStep` of 0.02
    // for the [-1, 1] chroma track: the error stays under a fifth of one stop, so the handle never
    // visibly moves. Pinned so a change to QUANTIZE_DIGITS shows up here rather than as slider jitter
    // nobody can explain.
    let worst = 0;
    for (const band of GRADE_BANDS) {
      const master = band.algebra === 'additive' ? 0.3 : 1.4;
      for (const temp of sweep(-0.9, 0.9, 13)) {
        for (const tint of sweep(-0.9, 0.9, 13)) {
          const back = decompose(band, recomposeForWrite(band, { master, temp, tint }));
          worst = Math.max(
            worst,
            relError(back.master, master),
            relError(back.temp, temp),
            relError(back.tint, tint),
          );
        }
      }
    }
    expect(worst).toBeLessThan(4e-3);
  });
});
