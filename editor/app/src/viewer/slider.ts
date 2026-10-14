// The arithmetic behind a bounded numeric field: how far its slider travels, what it steps by, and
// what a drag is allowed to write.
//
// WHY IT IS ITS OWN MODULE. `vite.config.ts` collects `src/**/*.test.ts` under a `node` environment,
// so nothing living inside a `.tsx` widget is reachable by a unit test. Every rule below decides what
// number a gesture produces, so every rule below is testable here and the widget is left with
// rendering.
//
// TWO RANGES, NOT ONE. `FieldRange.min`/`max` is *travel* — how far the handle moves, chosen so the
// useful part of the field is the whole track. `hardMin`/`hardMax` are the schema's *legal* bounds,
// which the scene validator enforces (`L1014`). Travel may be narrower than legal (a `[0, 1]` field
// whose authored values all sit under `0.01`), wider than legal, or declared where no legal bound
// exists at all.
//
// TWO SEND CADENCES, ONE UNDO ENTRY. `SliderDrag` at the bottom is the gesture's policy, modelled on
// `editor/gizmo/controller.ts` and sharing its clock: throttled latest-wins previews move the frame
// while the handle travels, and the exact value at release is sent unthrottled so the last pointer
// position is never dropped. Only that last send is reported, so one gesture is one undo entry.
//
// Pure: no React, no DOM, no wire.

import { PREVIEW_INTERVAL_MS } from '../editor/gizmo/controller';
import { shouldSendUpdate } from '../editor/gizmo/drag';
import type { FieldRange } from './inspect-fields';

/** Significant digits kept when a control's value is written. See {@link quantize}. */
export const QUANTIZE_DIGITS = 3;

/**
 * A control value as a text format should carry it: shortest-round-trip, free of picker noise.
 *
 * SIGNIFICANT DIGITS, NOT DECIMAL PLACES. Measured over all 256 8-bit levels of the voxel palette's
 * color picker: rounding the linear value to 3 DECIMALS breaks the picker's round trip for 19 of them
 * (worst case 2 levels off, because the sRGB curve's slope near black is 12.92, and near-black is
 * exactly where that palette lives — stone 0.1–0.2, creature flesh 0.03–0.08). Rounding to 3
 * SIGNIFICANT digits is exact for all 256 and reads like the authored numbers (`0.216`, `0.0137`).
 * The same property is what a slider needs: a knob at `0.002` must not quantize to `0`.
 */
export function quantize(value: number, digits: number = QUANTIZE_DIGITS): number {
  return Number.isFinite(value) ? Number(value.toPrecision(digits)) : value;
}

/** The range widened to contain `value` — a slider must never silently rewrite the number it was
 *  opened on, so an authored value outside the declared travel extends the track instead of being
 *  clipped by it. The legal bounds are untouched: widening travel does not make a value legal. */
export function widen(range: FieldRange, value: number): FieldRange {
  if (!Number.isFinite(value)) {
    return range;
  }
  return { ...range, min: Math.min(range.min, value), max: Math.max(range.max, value) };
}

/** Fallback steps for a span that cannot be subdivided (degenerate or non-finite travel). */
const DEGENERATE_STEP = { integer: 1, continuous: 0.01 } as const;

/**
 * A 1/2/5×10ⁿ step spanning `min`..`max` in ~100 stops, so the handle lands on numbers that read like
 * authored ones (`0.05`, `0.001`) rather than on the float the pixel happened to fall at.
 *
 * `integer` fields never step below 1; a wide integer span still gets a nice step (`[0, 1000]` → 10).
 */
export function niceStep(min: number, max: number, integer: boolean): number {
  const span = max - min;
  if (!Number.isFinite(span) || span <= 0) {
    return integer ? DEGENERATE_STEP.integer : DEGENERATE_STEP.continuous;
  }
  const raw = span / 100;
  const magnitude = Math.pow(10, Math.floor(Math.log10(raw)));
  // Rounded before it is bucketed, so a span that only differs from another by float error (`2.01-2`
  // vs `0.01`) cannot fall into a different bucket and step differently.
  const fraction = Number((raw / magnitude).toPrecision(12));
  const nice = fraction <= 1 ? 1 : fraction <= 2 ? 2 : fraction <= 5 ? 5 : 10;
  // A nice step carries one significant digit, so quantizing it only removes the float noise of the
  // multiply (`5 * 1e-3`).
  const step = quantize(nice * magnitude);
  return integer ? Math.max(DEGENERATE_STEP.integer, Math.round(step)) : step;
}

/** `value` clamped to the schema's legal bounds where it declares them. This is what keeps a drag
 *  from ever authoring a value the scene validator would reject; travel bounds are deliberately not
 *  applied, because {@link widen} may have opened the track past them on purpose. */
export function clampToLegal(value: number, range: FieldRange): number {
  let clamped = value;
  if (range.hardMin !== null && clamped < range.hardMin) {
    clamped = range.hardMin;
  }
  if (range.hardMax !== null && clamped > range.hardMax) {
    clamped = range.hardMax;
  }
  return clamped;
}

/**
 * The number a slider gesture writes, given the raw position `raw` the track reports.
 *
 * Quantized BEFORE it is clamped, never after: rounding a value that already sits on a legal bound
 * can push it back off (`0.9999` → `1` past a `maximum` of `0.9999`), which is the one thing this
 * path exists to prevent. An integer field rounds instead — a track whose `min` is fractional can
 * otherwise land between stops however nice its step is.
 */
export function dragValue(raw: number, range: FieldRange, integer: boolean): number {
  return clampToLegal(integer ? Math.round(raw) : quantize(raw), range);
}

// --- the gesture ---------------------------------------------------------------------------------

/**
 * What the widget should do after one track event. Every field is optional:
 * - `handle` — where the handle sits now; present on every move of a live gesture, and what the row
 *   displays instead of the ~3 Hz polled value while the pointer owns it.
 * - `preview` — a `component.set` at the preview cadence: it moves the frame and NOT the undo stack.
 *   Throttled, latest-wins.
 * - `commit` — the gesture's one reported write. Never throttled, so the exact value the gesture
 *   ended on always reaches the engine.
 * - `done` — the gesture ended (release, cancel, or Escape): drop the local handle and let the poll
 *   drive the row again.
 */
export interface SliderInstruction {
  readonly handle?: number;
  readonly preview?: number;
  readonly commit?: number;
  readonly done?: boolean;
}

interface SliderDragState {
  /** The value the gesture opened on. Escape writes exactly this back. */
  readonly base: number;
  readonly range: FieldRange;
  readonly integer: boolean;
  /** `shouldSendUpdate` clock; `-Infinity` so the first changed move always previews. */
  lastSentMs: number;
  /** Whether any preview went out — a gesture that ends back on `base` must still write, and the
   *  scene stays dirty, because the engine really did record those sets. */
  dirtied: boolean;
}

/**
 * One slider gesture, driven by track events. One instance per rendered track.
 *
 * Unlike `GizmoController`, this layer has a single channel back to its caller — a reported write is
 * both the undo entry and the dirty mark — so a gesture that ends back on its base still commits: the
 * engine holds previews that must be undone, and the entry that carries that restore spans base→base.
 * Undoing it is correctly a no-op; leaving the scene silently clean would not be.
 */
export class SliderDrag {
  private active: SliderDragState | null = null;
  private readonly intervalMs: number;

  constructor(intervalMs = PREVIEW_INTERVAL_MS) {
    this.intervalMs = intervalMs;
  }

  get dragging(): boolean {
    return this.active !== null;
  }

  /**
   * Opens a gesture on `base` — the value the track was showing before the pointer touched it.
   *
   * Idempotent while one is live, and that is load-bearing: previews compose into the value the
   * widget is handed, so a second `begin` mid-gesture would move the target Escape restores to.
   */
  begin(base: number, range: FieldRange, integer: boolean): void {
    if (this.active === null) {
      this.active = { base, range, integer, lastSentMs: Number.NEGATIVE_INFINITY, dirtied: false };
    }
  }

  /** Advances the gesture: always yields a `handle`; yields a throttled `preview` when the value moved. */
  move(raw: number, nowMs: number): SliderInstruction {
    const a = this.active;
    if (a === null) {
      return {};
    }
    const handle = dragValue(raw, a.range, a.integer);
    // Skip previews only while the value has never left base; once one went out, a move back exactly
    // onto base must still write, or the engine would be left holding the last preview.
    if ((handle === a.base && !a.dirtied) || !shouldSendUpdate(a.lastSentMs, nowMs, this.intervalMs)) {
      return { handle };
    }
    a.lastSentMs = nowMs;
    a.dirtied = true;
    return { handle, preview: handle };
  }

  /**
   * Ends the gesture on the exact value the track reports, whatever the throttle last let through.
   * A gesture that neither moved nor previewed writes nothing — a redundant `component.set` would be
   * an undo entry for an edit that did not happen.
   */
  end(raw: number): SliderInstruction {
    const a = this.active;
    if (a === null) {
      return {};
    }
    this.active = null;
    const final = dragValue(raw, a.range, a.integer);
    return final === a.base && !a.dirtied ? { done: true } : { commit: final, done: true };
  }

  /** Cancels the gesture: writes the captured base back, so Escape restores the value the gesture
   *  opened on rather than a value recomputed from the track. Nothing to restore, nothing to write. */
  escape(): SliderInstruction {
    const a = this.active;
    if (a === null) {
      return {};
    }
    this.active = null;
    return a.dirtied ? { commit: a.base, done: true } : { done: true };
  }

  /** Abandons a live gesture without writing anything (the field or its target went away). */
  reset(): void {
    this.active = null;
  }
}
