import { describe, expect, it } from 'vitest';
import {
  CONNECTED,
  DEFAULT_BACKOFF,
  reconnectStatus,
  reduce,
  type BackoffConfig,
  type ReconnectEffect,
  type ReconnectEvent,
  type ReconnectPhase,
} from './reconnect';

// A tiny in-memory driver: it applies the machine's effects the way `App` would, but against an
// injected clock and an injected "what does a connect attempt do" oracle, so a whole reconnect
// episode plays out deterministically. `schedule` advances the clock by the delay and re-dispatches
// `timer`; `connect` asks the oracle for the next outcome and dispatches it. Every scheduled delay is
// recorded so tests can assert the backoff ladder. (Same transport-injected shape as undo.test.ts.)
type ConnectOutcome = ReconnectEvent & { type: 'succeeded' | 'failed' };

function drive(
  start: ReconnectPhase,
  first: ReconnectEvent,
  outcomes: ConnectOutcome[],
  config: BackoffConfig = DEFAULT_BACKOFF,
  startNow = 0,
): { phase: ReconnectPhase; delays: number[]; connects: number } {
  let phase = start;
  let now = startNow;
  const delays: number[] = [];
  let connects = 0;
  const queue: ReconnectEvent[] = [first];

  while (queue.length > 0) {
    const event = queue.shift() as ReconnectEvent;
    const { phase: next, effects } = reduce(phase, event, config, now);
    phase = next;
    for (const effect of effects as ReconnectEffect[]) {
      if (effect.type === 'schedule') {
        delays.push(effect.delayMs);
        now += effect.delayMs; // the timer fires after the delay
        queue.push({ type: 'timer' });
      } else {
        connects += 1;
        const outcome = outcomes.shift();
        if (outcome !== undefined) {
          queue.push(outcome);
        }
      }
    }
  }
  return { phase, delays, connects };
}

const OK: ConnectOutcome = { type: 'succeeded' };
const FAIL: ConnectOutcome = { type: 'failed' };

describe('reduce — the happy blink', () => {
  it('lost → scheduled quick retry → connect → succeeded → connected', () => {
    const { phase, delays, connects } = drive(CONNECTED, { type: 'lost' }, [OK]);
    expect(phase).toEqual(CONNECTED);
    expect(delays).toEqual([250]); // one quick first retry
    expect(connects).toBe(1);
  });

  it('recovers on the third attempt of a longer blink', () => {
    const { phase, delays, connects } = drive(CONNECTED, { type: 'lost' }, [FAIL, FAIL, OK]);
    expect(phase).toEqual(CONNECTED);
    expect(delays).toEqual([250, 500, 1000]);
    expect(connects).toBe(3);
  });

  it('the individual lost transition schedules the first delay and enters reconnecting', () => {
    const t = reduce(CONNECTED, { type: 'lost' }, DEFAULT_BACKOFF, 5_000);
    expect(t.phase).toEqual({ kind: 'reconnecting', attempt: 0, firstLostAt: 5_000 });
    expect(t.effects).toEqual([{ type: 'schedule', delayMs: 250 }]);
  });

  it('a timer starts one connect attempt and counts it', () => {
    const t = reduce({ kind: 'reconnecting', attempt: 0, firstLostAt: 0 }, { type: 'timer' }, DEFAULT_BACKOFF, 250);
    expect(t.phase).toEqual({ kind: 'reconnecting', attempt: 1, firstLostAt: 0 });
    expect(t.effects).toEqual([{ type: 'connect' }]);
  });
});

describe('reduce — backoff growth and terminal give-up', () => {
  it('grows the delay each failure and gives up once no attempt fits the deadline', () => {
    // 30 s budget; the ladder caps at 4 s. Every attempt fails.
    const { phase, delays } = drive(CONNECTED, { type: 'lost' }, Array(50).fill(FAIL) as ConnectOutcome[]);
    expect(phase).toEqual({ kind: 'failed', reason: 'exhausted' });
    // Quick early retries backing off to a steady 4 s cadence.
    expect(delays.slice(0, 5)).toEqual([250, 500, 1000, 2000, 4000]);
    expect(delays.slice(5)).toEqual([4000, 4000, 4000, 4000, 4000]); // steady tail
    // Give-up is bounded: total scheduled time never exceeds the 30 s budget.
    const total = delays.reduce((a, b) => a + b, 0);
    expect(total).toBeLessThanOrEqual(DEFAULT_BACKOFF.deadlineMs);
  });

  it('does not schedule an attempt that would fire past the deadline', () => {
    const config: BackoffConfig = { delaysMs: [1000], deadlineMs: 2500 };
    // firstLostAt 0; attempts fire at 1000, 2000; a third at 3000 > 2500 → give up.
    const { phase, delays, connects } = drive(CONNECTED, { type: 'lost' }, Array(10).fill(FAIL) as ConnectOutcome[], config);
    expect(phase).toEqual({ kind: 'failed', reason: 'exhausted' });
    expect(delays).toEqual([1000, 1000]);
    expect(connects).toBe(2);
  });
});

describe('reduce — manual retry from the terminal state', () => {
  it('retry from exhausted starts a fresh episode and connects immediately (no first delay)', () => {
    const t = reduce({ kind: 'failed', reason: 'exhausted' }, { type: 'retry' }, DEFAULT_BACKOFF, 9_000);
    expect(t.phase).toEqual({ kind: 'reconnecting', attempt: 1, firstLostAt: 9_000 });
    expect(t.effects).toEqual([{ type: 'connect' }]);
  });

  it('a manual retry can then succeed', () => {
    const { phase } = drive({ kind: 'failed', reason: 'exhausted' }, { type: 'retry' }, [OK]);
    expect(phase).toEqual(CONNECTED);
  });

  it('a manual retry can fail back into a fresh backoff and eventually give up', () => {
    const { phase, delays } = drive({ kind: 'failed', reason: 'exhausted' }, { type: 'retry' }, Array(50).fill(FAIL) as ConnectOutcome[]);
    expect(phase).toEqual({ kind: 'failed', reason: 'exhausted' });
    // The immediate connect (no leading delay) fails, then the ladder resumes from the 500 ms rung.
    expect(delays[0]).toBe(500);
  });
});

describe('reduce — stray events are ignored (idempotent against the wrong phase)', () => {
  const noop = (phase: ReconnectPhase, event: ReconnectEvent): void => {
    const t = reduce(phase, event, DEFAULT_BACKOFF, 100);
    expect(t.phase).toBe(phase);
    expect(t.effects).toEqual([]);
  };

  it('a loss reported while already reconnecting does not restart the schedule', () => {
    noop({ kind: 'reconnecting', attempt: 2, firstLostAt: 0 }, { type: 'lost' });
  });
  it('a timer while connected is ignored', () => noop(CONNECTED, { type: 'timer' }));
  it('a succeeded while connected is ignored', () => noop(CONNECTED, { type: 'succeeded' }));
  it('a failed while connected is ignored', () => noop(CONNECTED, { type: 'failed' }));
  it('a retry while connected is ignored', () => noop(CONNECTED, { type: 'retry' }));
  it('a timer while failed is ignored', () => noop({ kind: 'failed', reason: 'exhausted' }, { type: 'timer' }));
});

describe('reconnectStatus', () => {
  it('maps each phase to the coarse UI status', () => {
    expect(reconnectStatus(CONNECTED)).toBe('live');
    expect(reconnectStatus({ kind: 'reconnecting', attempt: 1, firstLostAt: 0 })).toBe('reconnecting');
    expect(reconnectStatus({ kind: 'failed', reason: 'exhausted' })).toBe('failed');
  });
});
