import { describe, expect, it } from 'vitest';
import { pollHandback, shouldRollBack, STALE_POLL_LIMIT } from './pendingBase';

describe('pollHandback', () => {
  it('decides nothing while the poll already owns the value', () => {
    expect(pollHandback({ polled: { x: 1 }, base: null, inflight: 0, stalePolls: 1 })).toEqual({
      handback: false,
      stalePolls: 1,
    });
  });

  it('hands back once the polled value deep-equals the base', () => {
    const polled = { translation: [1, 2, 3], scale: [1, 1, 1] };
    const base = { translation: [1, 2, 3], scale: [1, 1, 1] };
    expect(pollHandback({ polled, base, inflight: 0, stalePolls: 0 })).toEqual({
      handback: true,
      stalePolls: 0,
    });
  });

  it('keeps trusting the base while a send races the poll', () => {
    const refresh = { polled: { x: 1 }, base: { x: 2 }, inflight: 1, stalePolls: 1 };
    expect(pollHandback(refresh)).toEqual({ handback: false, stalePolls: 0 });
  });

  it('yields after two consecutive stale polls with nothing in flight', () => {
    // The engine normalised the value in transit (f64 -> f32), so it will never echo the sent bytes:
    // without this fallback the base would wedge stale forever.
    const polled = { exposure: 0.30000001192092896 };
    const base = { exposure: 0.3 };
    const first = pollHandback({ polled, base, inflight: 0, stalePolls: 0 });
    expect(first).toEqual({ handback: false, stalePolls: 1 });
    expect(pollHandback({ polled, base, inflight: 0, stalePolls: first.stalePolls })).toEqual({
      handback: true,
      stalePolls: 0,
    });
  });

  it('counts only consecutive stale polls', () => {
    const stale = pollHandback({ polled: { x: 1 }, base: { x: 2 }, inflight: 0, stalePolls: 0 });
    expect(stale.stalePolls).toBe(1);
    // A send arriving between two stale refreshes resets the count, so the next stale one starts over.
    const racing = pollHandback({
      polled: { x: 1 },
      base: { x: 2 },
      inflight: 1,
      stalePolls: stale.stalePolls,
    });
    expect(racing).toEqual({ handback: false, stalePolls: 0 });
    expect(
      pollHandback({ polled: { x: 1 }, base: { x: 2 }, inflight: 0, stalePolls: racing.stalePolls }),
    ).toEqual({ handback: false, stalePolls: 1 });
  });

  it('never yields on the first stale poll', () => {
    expect(STALE_POLL_LIMIT).toBe(2);
    expect(
      pollHandback({ polled: { x: 1 }, base: { x: 2 }, inflight: 0, stalePolls: 0 }).handback,
    ).toBe(false);
  });
});

describe('shouldRollBack', () => {
  it('rolls back the assembly that is still standing', () => {
    const assembled = { fog_density: 0.02 };
    expect(shouldRollBack(assembled, assembled)).toBe(true);
  });

  it('leaves a newer commit that composed on top alone', () => {
    const assembled = { fog_density: 0.02 };
    const newer = { fog_density: 0.02, fog_height: 4 };
    expect(shouldRollBack(newer, assembled)).toBe(false);
  });

  it('compares identity, not contents: a deep-equal newer commit still wins', () => {
    const assembled = { fog_density: 0.02 };
    expect(shouldRollBack({ ...assembled }, assembled)).toBe(false);
  });

  it('leaves a base already handed back to the poll alone', () => {
    expect(shouldRollBack(null, { fog_density: 0.02 })).toBe(false);
  });
});
