// Reconnect state machine for the editor (Plan/phase-7-designer-flow.md decision 6).
//
// `ludion dev` restarts the game process on every hot reload: the `ws://` endpoint vanishes for a
// 1–3 s "blink" and comes back on the SAME port. The editor must ride that blink without the human
// touching anything — the caller policy `protocol/client.ts` documents but deliberately doesn't
// implement ("Reconnection policy belongs to the caller").
//
// This module is that policy as a *pure* machine — states, transitions, and the backoff schedule —
// with every side effect (open a socket, arm a timer) injected by the driver
// in `App`. Kept transport- and timer-free so the whole thing is unit-testable with an injected clock,
// the same shape as `editor/undo.ts` and `viewer/picking.ts`.
//
// The bounds are tuned to the dev blink: quick first retries (a blink is back within seconds), backing
// off to a slower steady cadence, and giving up after a bounded total so a genuinely dead engine isn't
// masked as an endless "reconnecting…" (the plan's risk list calls this out).

/** Why the machine stopped retrying: the time budget ran out. Terminal; only a manual retry leaves it. */
export type FailureReason = 'exhausted';

/**
 * The machine's phase.
 *
 * - `connected` — the live, resting state; the socket is up.
 * - `reconnecting` — the socket dropped and we're riding the blink. `attempt` counts connect attempts
 *   started this episode (0 right after the loss, before the first scheduled try); `firstLostAt` is the
 *   clock value at the loss, against which the give-up deadline is measured.
 * - `failed` — terminal; retrying stopped. A manual `retry` starts a fresh episode.
 */
export type ReconnectPhase =
  | { readonly kind: 'connected' }
  | { readonly kind: 'reconnecting'; readonly attempt: number; readonly firstLostAt: number }
  | { readonly kind: 'failed'; readonly reason: FailureReason };

/**
 * An event fed to the machine by the driver.
 *
 * - `lost` — the live socket closed/errored (from the client's `onClose`).
 * - `timer` — a scheduled backoff delay elapsed; time to try connecting.
 * - `succeeded` — a connect attempt completed the handshake.
 * - `failed` — a connect attempt failed.
 * - `retry` — the human asked to retry from the terminal state.
 */
export type ReconnectEvent =
  | { readonly type: 'lost' }
  | { readonly type: 'timer' }
  | { readonly type: 'succeeded' }
  | { readonly type: 'failed' }
  | { readonly type: 'retry' };

/**
 * A side effect the driver must run. `schedule` arms a one-shot timer that dispatches `timer` after
 * `delayMs`; `connect` opens a fresh socket + handshake now (its outcome dispatches `succeeded`/`failed`).
 */
export type ReconnectEffect =
  | { readonly type: 'schedule'; readonly delayMs: number }
  | { readonly type: 'connect' };

/** The per-attempt delay ladder and the total give-up budget. */
export interface BackoffConfig {
  /** Delay (ms) before attempt N (1-indexed): `delaysMs[min(N-1, len-1)]`; the last value repeats. */
  readonly delaysMs: readonly number[];
  /** Total wall-clock budget (ms) from the loss; once no further attempt fits under it, give up. */
  readonly deadlineMs: number;
}

/** The next phase plus the effects the driver runs to get there. */
export interface Transition {
  readonly phase: ReconnectPhase;
  readonly effects: readonly ReconnectEffect[];
}

/**
 * Tuned to the 1–3 s dev blink: 250 ms → 500 ms → 1 s → 2 s → 4 s (then steady at 4 s), giving up
 * after 30 s total (~10 attempts). A blink is back inside the first two or three tries; 30 s is long
 * enough that a slow rebuild+restart still recovers, short enough that a crashed engine surfaces.
 */
export const DEFAULT_BACKOFF: BackoffConfig = {
  delaysMs: [250, 500, 1000, 2000, 4000],
  deadlineMs: 30_000,
};

/** The resting phase — exported so the driver and tests share one literal. */
export const CONNECTED: ReconnectPhase = { kind: 'connected' };

const NO_EFFECTS: readonly ReconnectEffect[] = [];

/** The delay before the next attempt, given how many have already been started this episode. */
function delayFor(config: BackoffConfig, attemptsMade: number): number {
  const i = Math.min(Math.max(attemptsMade, 0), config.delaysMs.length - 1);
  return config.delaysMs[i] ?? 0;
}

/**
 * The pure transition. Given the current `phase`, an `event`, the backoff `config`, and the current
 * clock `now` (ms), returns the next phase and the effects to run. Total and side-effect-free; every
 * event is idempotent against a phase it doesn't apply to (returns the phase unchanged, no effects),
 * so a late/stray event from a torn-down attempt can't corrupt the machine.
 */
export function reduce(phase: ReconnectPhase, event: ReconnectEvent, config: BackoffConfig, now: number): Transition {
  switch (event.type) {
    case 'lost': {
      // Only a live connection can be lost; a loss reported while already recovering is a stale
      // attempt-socket closing — ignore it so it can't restart the schedule.
      if (phase.kind !== 'connected') {
        return { phase, effects: NO_EFFECTS };
      }
      return {
        phase: { kind: 'reconnecting', attempt: 0, firstLostAt: now },
        effects: [{ type: 'schedule', delayMs: delayFor(config, 0) }],
      };
    }
    case 'timer': {
      if (phase.kind !== 'reconnecting') {
        return { phase, effects: NO_EFFECTS };
      }
      return { phase: { ...phase, attempt: phase.attempt + 1 }, effects: [{ type: 'connect' }] };
    }
    case 'succeeded': {
      if (phase.kind !== 'reconnecting') {
        return { phase, effects: NO_EFFECTS };
      }
      return { phase: CONNECTED, effects: NO_EFFECTS };
    }
    case 'failed': {
      if (phase.kind !== 'reconnecting') {
        return { phase, effects: NO_EFFECTS };
      }
      const delayMs = delayFor(config, phase.attempt);
      // Give up if the next attempt would fire past the deadline — don't schedule a doomed try.
      if (now + delayMs > phase.firstLostAt + config.deadlineMs) {
        return { phase: { kind: 'failed', reason: 'exhausted' }, effects: NO_EFFECTS };
      }
      return { phase, effects: [{ type: 'schedule', delayMs }] };
    }
    case 'retry': {
      // A manual retry only means something from the terminal state; it starts a fresh episode and,
      // since the human asked, connects immediately rather than waiting out a first backoff delay.
      if (phase.kind !== 'failed') {
        return { phase, effects: NO_EFFECTS };
      }
      return { phase: { kind: 'reconnecting', attempt: 1, firstLostAt: now }, effects: [{ type: 'connect' }] };
    }
    default: {
      const never: never = event;
      return never;
    }
  }
}

/** Coarse status for the UI: whether the viewport is live, riding a blink, or terminally disconnected. */
export function reconnectStatus(phase: ReconnectPhase): 'live' | 'reconnecting' | 'failed' {
  return phase.kind === 'connected' ? 'live' : phase.kind;
}
