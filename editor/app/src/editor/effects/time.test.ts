import { describe, expect, it } from 'vitest';
import {
  INITIAL_RUN,
  MIN_LOOP_PERIOD_S,
  SPEEDS,
  TICK_HZ,
  afterPauseToggle,
  afterStep,
  loopDue,
  periodFrames,
  withSpeed,
} from './time';

describe('constants', () => {
  it('pins the tick rate, the offered speeds and the loop floor', () => {
    expect(TICK_HZ).toBe(60);
    expect(SPEEDS).toEqual([0.25, 0.5, 1, 2]);
    expect(MIN_LOOP_PERIOD_S).toBe(0.5);
  });
});

describe('periodFrames', () => {
  it('uses the engine defaults (duration 0, life[1] 1.4) when the value carries neither', () => {
    expect(periodFrames({})).toBe(Math.ceil(1.4 * 60));
  });

  it('sums duration and life[1] once that exceeds the floor', () => {
    // duration 0 + life[1] 0.6 = 0.6s, above the 0.5s floor: ceil(0.6 * 60) = 36.
    expect(periodFrames({ duration: 0, life: [0.3, 0.6] })).toBe(36);
  });

  it('floors a short burst at MIN_LOOP_PERIOD_S, which wins over the raw sum', () => {
    // duration 0 + life[1] 0.1 = 0.1s, below the floor: ceil(0.5 * 60) = 30, not ceil(0.1 * 60) = 6.
    expect(periodFrames({ duration: 0, life: [0.05, 0.1] })).toBe(30);
  });

  it('adds a finite non-negative duration on top of life[1]', () => {
    expect(periodFrames({ duration: 2, life: [0.8, 1.4] })).toBe(Math.ceil(3.4 * 60));
  });

  it('treats a negative or non-finite duration as 0', () => {
    expect(periodFrames({ duration: -5, life: [0.8, 1.4] })).toBe(periodFrames({ duration: 0, life: [0.8, 1.4] }));
    expect(periodFrames({ duration: Number.NaN, life: [0.8, 1.4] })).toBe(periodFrames({ duration: 0, life: [0.8, 1.4] }));
    expect(periodFrames({ duration: Infinity, life: [0.8, 1.4] })).toBe(periodFrames({ duration: 0, life: [0.8, 1.4] }));
  });

  it('falls back to the engine default life[1] when life is missing, short, or non-finite', () => {
    expect(periodFrames({ duration: 0 })).toBe(periodFrames({ duration: 0, life: [0.8, 1.4] }));
    expect(periodFrames({ duration: 0, life: [0.8] })).toBe(periodFrames({ duration: 0, life: [0.8, 1.4] }));
    expect(periodFrames({ duration: 0, life: [0.8, Number.NaN] })).toBe(periodFrames({ duration: 0, life: [0.8, 1.4] }));
    expect(periodFrames({ duration: 0, life: 'nope' })).toBe(periodFrames({ duration: 0, life: [0.8, 1.4] }));
  });

  it('tolerates a non-object value entirely', () => {
    expect(periodFrames(null)).toBe(periodFrames({}));
    expect(periodFrames(undefined)).toBe(periodFrames({}));
    expect(periodFrames('not an emitter')).toBe(periodFrames({}));
  });

  it('honors a custom tick rate', () => {
    expect(periodFrames({ duration: 1, life: [0, 1] }, 30)).toBe(Math.ceil(2 * 30));
  });
});

describe('loopDue', () => {
  it('is false when frame is null', () => {
    expect(loopDue(null, 0, 30)).toBe(false);
  });

  it('is false when started is not a finite number', () => {
    expect(loopDue(100, 'x', 30)).toBe(false);
    expect(loopDue(100, Number.NaN, 30)).toBe(false);
    expect(loopDue(100, undefined, 30)).toBe(false);
  });

  it('is false before the period elapses and true at the exact boundary and beyond', () => {
    expect(loopDue(59, 30, 30)).toBe(false);
    expect(loopDue(60, 30, 30)).toBe(true);
    expect(loopDue(61, 30, 30)).toBe(true);
  });
});

describe('SimRun', () => {
  it('starts running at 1x, matching a ludion serve launched without --paused', () => {
    expect(INITIAL_RUN).toEqual({ paused: false, speed: 1 });
  });

  it('afterStep always leaves the run paused', () => {
    expect(afterStep({ paused: false, speed: 1 })).toEqual({ paused: true, speed: 1 });
    expect(afterStep({ paused: true, speed: 2 })).toEqual({ paused: true, speed: 2 });
  });

  it('afterPauseToggle flips paused and preserves speed', () => {
    expect(afterPauseToggle({ paused: false, speed: 2 })).toEqual({ paused: true, speed: 2 });
    expect(afterPauseToggle({ paused: true, speed: 2 })).toEqual({ paused: false, speed: 2 });
  });

  it('withSpeed accepts a finite positive speed and rejects everything else unchanged', () => {
    const run = { paused: false, speed: 1 };
    expect(withSpeed(run, 2)).toEqual({ paused: false, speed: 2 });
    expect(withSpeed(run, 0)).toBe(run);
    expect(withSpeed(run, -1)).toBe(run);
    expect(withSpeed(run, Number.NaN)).toBe(run);
    expect(withSpeed(run, Infinity)).toBe(run);
  });
});
