// The decisions behind a graded band's control: whether a field is a band at all, which rows it
// presents, and which split each row's write is rebuilt from.
//
// WHY IT IS NOT IN grade.ts. That module is the mathematics — an exactly invertible view of one
// triple, knowing nothing of fields, tracks or gestures. This is the other half: it reads a
// `FieldDescriptor` to decide whether the split view applies, and it holds the one piece of gesture
// state the mathematics makes necessary. `GradeBandControl.tsx` is then only rendering.
//
// WHY A GESTURE HOLDS ITS OWN SPLIT. The stored triple is the source of truth and the split is
// re-derived from it on every render — but `recomposeForWrite` quantizes, so re-deriving from the
// triple a drag just wrote gives back a split a few thousandths off the one it was recomposed from
// (grade.test.ts measures the worst at 3.15e-3). Rebuild the next write from that and the two handles
// the hand is NOT on drift a little further with every preview. So a live gesture keeps the split it
// is mutating and recomposes from that — `SliderDrag`'s captured base, one level up, and the same
// rule hold-to-compare follows: restore the captured object, never a recomputed one.
//
// WHERE THE BOUND IS. The master's track carries the field's own declared travel and is the only
// thing bounded; the triple leaves here exactly as `recompose` built it. Clamping a recomposed
// channel would silently shift the chroma the human set (grade.ts's report), and the value it would
// clamp against does not exist — the schema declares no bound on these arrays.
//
// Pure: no React, no DOM, no wire.

import type { FieldDescriptor, FieldRange } from '../../viewer/inspect-fields';
import type { CommitCadence } from '../../viewer/useComponentCommit';
import {
  gradeBand,
  gradeTravel,
  recomposeForWrite,
  type GradeBand,
  type GradeSplit,
  type Triple,
} from './grade';

/** The three things a band presents, in the order they stack: how much of it, then what colour. */
export type GradeAxis = 'master' | 'temp' | 'tint';

/** One row of a band's control: the axis it writes, and how far its track travels. */
export interface GradeRow {
  readonly axis: GradeAxis;
  readonly range: FieldRange;
}

/** A field rendered as a band: which band it is, and the rows it presents. */
export interface GradeControl {
  readonly band: GradeBand;
  readonly rows: readonly GradeRow[];
}

/**
 * The band control for `field`, or `null` for every field that is not one of the three graded
 * triples — `null` means render the shared `FieldWidget`, exactly as before.
 *
 * A band whose schema declares no travel falls back too. The master's throw IS the field's
 * `x-ui-range`, and there is no second place to get one: a track without it would have to invent its
 * own extent, which is the one thing `deriveField` refuses to do when it reports `range: null`.
 *
 * The shape is checked as well as the name, so a `lift` that stopped being a 3-wide vector — or one
 * nested inside another field — degrades to the generic widget instead of being decomposed as
 * something it is not.
 */
export function gradeControl(field: FieldDescriptor): GradeControl | null {
  if (field.kind !== 'vector' || field.length !== 3 || field.path.length !== 1) {
    return null;
  }
  const name = field.path[0];
  const band = typeof name === 'string' ? gradeBand(name) : null;
  if (band === null) {
    return null;
  }
  const travel = gradeTravel(field.range);
  if (travel.master === null) {
    return null;
  }
  return {
    band,
    rows: [
      { axis: 'master', range: travel.master },
      { axis: 'temp', range: travel.temp },
      { axis: 'tint', range: travel.tint },
    ],
  };
}

/** `split` with one axis replaced. Exhaustive rather than a computed key, so an axis added to
 *  {@link GradeAxis} is a compile error here and not a silently ignored row. */
function withAxis(split: GradeSplit, axis: GradeAxis, value: number): GradeSplit {
  switch (axis) {
    case 'master':
      return { ...split, master: value };
    case 'temp':
      return { ...split, temp: value };
    case 'tint':
      return { ...split, tint: value };
  }
}

/** The split a live gesture is mutating, and the row it belongs to. */
interface HeldSplit {
  readonly axis: GradeAxis;
  readonly split: GradeSplit;
}

/**
 * One band's gesture state — one instance per rendered control, and it sits directly on top of
 * `SliderDrag`: that decides what number a row's handle produces, this decides which triple that
 * number is written as.
 */
export class GradeGesture {
  private held: HeldSplit | null = null;

  /** Whether a gesture is live (the control's own state is otherwise entirely derived). */
  get holding(): boolean {
    return this.held !== null;
  }

  /**
   * The triple a write on `axis` produces, advancing the gesture.
   *
   * A `'preview'` write is mid-gesture, so the split it produced is what the next one builds on;
   * every other cadence ends the gesture and hands the rows back to the stored triple. Releasing on
   * the commit is complete because `SliderDrag` follows any run of previews with exactly one commit,
   * whether the gesture was released, cancelled or escaped.
   *
   * A row touched while another row's gesture is somehow still live rebases on `live` rather than on
   * that row's split: two handles cannot be under one pointer, so the second row is a new gesture.
   */
  write(
    band: GradeBand,
    axis: GradeAxis,
    value: number,
    live: GradeSplit,
    cadence?: CommitCadence,
  ): Triple {
    const base = this.held !== null && this.held.axis === axis ? this.held.split : live;
    const split = withAxis(base, axis, value);
    this.held = cadence === 'preview' ? { axis, split } : null;
    return recomposeForWrite(band, split);
  }
}
