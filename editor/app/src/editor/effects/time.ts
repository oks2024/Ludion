// The Effects view's time strip, as pure data: how long one loop period is, when a loop is due,
// and the run/pause/speed state `sim.pause`/`sim.step`/`sim.set_speed` drive.
//
// FORWARD-ONLY BY DESIGN. There is no rewind, no scrub-back: an emitter's particles are a pure
// function of `(emitter, sim time)`, so the only way to see an earlier moment is to restart and
// step forward again. A restart re-stamps `started` at the session's current frame (read off a
// reply envelope, never the wall clock — the stage's sim time is the only clock that matters) and
// everything else composes on top of the engine's own stepping/pausing/speed methods.
//
// Pure: no React, no protocol client.

/** The engine's fixed tick rate (`TickRate::DEFAULT`, `crates/ludion-core/src/time.rs`). */
export const TICK_HZ = 60;

/** The speed strip's offered multipliers, via `sim.set_speed`. */
export const SPEEDS: readonly number[] = [0.25, 0.5, 1, 2];

/** The floor under a computed loop period, so a near-instant burst still reads as a loop rather
 *  than a flicker. */
export const MIN_LOOP_PERIOD_S = 0.5;

/** The engine's own `ParticleEmitter.life[1]` default (`crates/ludion-engine/src/components.rs`),
 *  used when a read component value doesn't carry a life pair. */
const DEFAULT_LIFE_MAX_S = 1.4;

function finiteNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

/**
 * The loop period in frames: `duration + life[1]` seconds read off the emitter value, floored at
 * `MIN_LOOP_PERIOD_S` and converted to frames at `tickHz`.
 *
 * `duration` counts only when it is a finite, non-negative number (else 0 — an endless emitter's
 * own `duration: 0`). `life[1]` counts only when `life` is an array whose second element is a
 * finite number (else the engine's own default, so a still-loading emitter previews a plausible
 * period rather than zero).
 */
export function periodFrames(value: unknown, tickHz = TICK_HZ): number {
  const record = value !== null && typeof value === 'object' ? (value as Record<string, unknown>) : {};

  const rawDuration = finiteNumber(record.duration);
  const duration = rawDuration !== null && rawDuration >= 0 ? rawDuration : 0;

  const life = record.life;
  const lifeMax = Array.isArray(life) ? (finiteNumber(life[1]) ?? DEFAULT_LIFE_MAX_S) : DEFAULT_LIFE_MAX_S;

  const seconds = Math.max(duration + lifeMax, MIN_LOOP_PERIOD_S);
  return Math.ceil(seconds * tickHz);
}

/** Whether the loop toggle should fire a restart: `frame` is a number, `started` a finite number,
 *  and the strip has advanced a full `period` past it. */
export function loopDue(frame: number | null, started: unknown, period: number): boolean {
  if (typeof frame !== 'number') {
    return false;
  }
  const startedAt = finiteNumber(started);
  if (startedAt === null) {
    return false;
  }
  return frame >= startedAt + period;
}

/** The strip's run state: whether the stage session is paused and its `sim.set_speed` multiplier. */
export interface SimRun {
  readonly paused: boolean;
  readonly speed: number;
}

/** A `ludion serve` session starts running unless launched with `--paused`, and the host spawns
 *  the effects stage without it. */
export const INITIAL_RUN: SimRun = { paused: false, speed: 1 };

/** `sim.step` runs its requested ticks and then pauses — Edit mode's frozen sim time otherwise
 *  keeps advancing every step call forever. */
export function afterStep(run: SimRun): SimRun {
  return { ...run, paused: true };
}

/** The pause toggle's own transition. */
export function afterPauseToggle(run: SimRun): SimRun {
  return { ...run, paused: !run.paused };
}

/** A validated `sim.set_speed` write: non-finite or non-positive speeds are rejected by returning
 *  `run` unchanged, so a bad control value can never reach the wire. */
export function withSpeed(run: SimRun, speed: number): SimRun {
  return Number.isFinite(speed) && speed > 0 ? { ...run, speed } : run;
}
