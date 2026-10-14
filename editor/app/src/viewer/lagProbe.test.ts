import { describe, expect, it } from 'vitest';
import {
  FRAME_WINDOW_MS,
  INJECT_FRAME_LEAD,
  INJECT_TIMEOUT_MS,
  INPUT_WINDOW_MS,
  MAX_QUEUED_MS,
  NO_LAG,
  createLagProbe,
  formatLag,
} from './lagProbe';

describe('the inject → frame correspondence', () => {
  it('closes on the first frame stamped past the applied frame', () => {
    const probe = createLagProbe();
    const token = probe.noteInject(1000, 1010);
    probe.resolveInject(token, 40);
    // Frame 40 is the world BEFORE tick 40 ran, so it cannot show an input applied at the start of
    // tick 40; frame 41 is the first that can.
    probe.noteFrame(40, 1020, 5);
    expect(probe.snapshot(1020).lagMs).toBeNull();
    probe.noteFrame(41, 1080, 5);
    expect(probe.snapshot(1080).lagMs).toBe(80); // 1080 - the event's own 1000
  });

  it('derives the threshold from INJECT_FRAME_LEAD', () => {
    const probe = createLagProbe();
    const token = probe.noteInject(0, 10);
    probe.resolveInject(token, 7);
    probe.noteFrame(7 + INJECT_FRAME_LEAD - 1, 20, 1);
    expect(probe.snapshot(20).lagMs).toBeNull();
    probe.noteFrame(7 + INJECT_FRAME_LEAD, 30, 1);
    expect(probe.snapshot(30).lagMs).not.toBeNull();
  });

  it('closes on a later frame when the exact one was dropped by latest-wins', () => {
    const probe = createLagProbe();
    const token = probe.noteInject(100, 100);
    probe.resolveInject(token, 10);
    probe.noteFrame(25, 250, 5); // 11 never arrived; 25 still depicts the input
    expect(probe.snapshot(250).lagMs).toBe(150);
  });

  it('measures from the key event, not from the reply', () => {
    const probe = createLagProbe();
    const token = probe.noteInject(1000, 1030); // 30 ms stuck in the queue
    probe.resolveInject(token, 5);
    probe.noteFrame(6, 1100, 5);
    expect(probe.snapshot(1100).lagMs).toBe(100);
    expect(probe.snapshot(1100).queuedMs).toBe(30);
  });

  it('closes against the last frame when the reply arrives after it', () => {
    const probe = createLagProbe();
    const token = probe.noteInject(1000, 1000);
    probe.noteFrame(9, 1050, 5);
    probe.resolveInject(token, 8); // frame 9 already satisfies 8 + 1
    expect(probe.snapshot(1060).lagMs).toBe(50); // against the blit, not against `now`
  });

  it('records one sample per press, not one per frame that follows', () => {
    const probe = createLagProbe();
    const token = probe.noteInject(0, 0);
    probe.resolveInject(token, 1);
    probe.noteFrame(2, 50, 5);
    probe.noteFrame(3, 60, 5);
    probe.noteFrame(4, 70, 5);
    expect(probe.snapshot(70).lagMs).toBe(50);
  });

  it('abandons a press whose command failed', () => {
    const probe = createLagProbe();
    const token = probe.noteInject(0, 0);
    probe.resolveInject(token, null);
    probe.noteFrame(500, 100, 5);
    expect(probe.snapshot(100).lagMs).toBeNull();
  });

  it('ignores a reply for a token it never issued', () => {
    const probe = createLagProbe();
    probe.resolveInject(4242, 1);
    probe.noteFrame(99, 100, 5);
    expect(probe.snapshot(100).lagMs).toBeNull();
  });

  it('gives up on a press whose frame never comes', () => {
    const probe = createLagProbe();
    const token = probe.noteInject(1, 1);
    probe.resolveInject(token, 1);
    // The stream stalls past the timeout, then resumes with a frame that would have closed it: a
    // two-second "latency" is a paused sim, not a measurement, and must not reach the median.
    probe.noteFrame(2, INJECT_TIMEOUT_MS + 2, 5);
    expect(probe.snapshot(INJECT_TIMEOUT_MS + 2).lagMs).toBeNull();
  });

  it('expires a press the reply never answered', () => {
    const probe = createLagProbe();
    probe.noteInject(1, 1);
    expect(probe.snapshot(INJECT_TIMEOUT_MS + 2).lagMs).toBeNull();
    // The late reply now has nothing to attach to, so no frame can close it.
    probe.resolveInject(1, 1);
    probe.noteFrame(9, INJECT_TIMEOUT_MS + 10, 5);
    expect(probe.snapshot(INJECT_TIMEOUT_MS + 10).lagMs).toBeNull();
  });

  it('keeps presses independent when several are in flight', () => {
    const probe = createLagProbe();
    const a = probe.noteInject(0, 0);
    const b = probe.noteInject(10, 10);
    probe.resolveInject(a, 1);
    probe.resolveInject(b, 3);
    probe.noteFrame(2, 100, 5); // closes a only
    expect(probe.snapshot(100).lagMs).toBe(100);
    probe.noteFrame(4, 210, 5); // closes b: 210 - 10
    expect(probe.snapshot(210).lagMs).toBe(150); // median of 100 and 200
  });
});

describe('the timeStamp sanity guard', () => {
  it('reports no queuedMs for a zero timestamp and counts the rejection', () => {
    const probe = createLagProbe();
    probe.noteInject(0, 500);
    const snap = probe.snapshot(500);
    expect(snap.queuedMs).toBeNull();
    expect(snap.rejectedTimeStamps).toBe(1);
  });

  it('rejects a timestamp from the future', () => {
    const probe = createLagProbe();
    probe.noteInject(900, 800);
    expect(probe.snapshot(800).rejectedTimeStamps).toBe(1);
  });

  it('rejects a timestamp on a foreign time origin', () => {
    const probe = createLagProbe();
    probe.noteInject(1, 1 + MAX_QUEUED_MS + 1);
    expect(probe.snapshot(99_999).rejectedTimeStamps).toBe(1);
  });

  it('rejects a non-finite timestamp', () => {
    const probe = createLagProbe();
    probe.noteInject(Number.NaN, 10);
    probe.noteInject(Number.POSITIVE_INFINITY, 10);
    expect(probe.snapshot(10).rejectedTimeStamps).toBe(2);
  });

  it('still measures lag from the handler when the timestamp is unusable', () => {
    const probe = createLagProbe();
    const token = probe.noteInject(0, 100); // rejected: origin falls back to the handler's clock
    probe.resolveInject(token, 1);
    probe.noteFrame(2, 180, 5);
    expect(probe.snapshot(180).lagMs).toBe(80);
  });

  it('accepts a plausible dispatch delay', () => {
    const probe = createLagProbe();
    probe.noteInject(1000, 1000); // a handler that ran immediately
    probe.noteInject(2000, 2014);
    const snap = probe.snapshot(2014);
    expect(snap.rejectedTimeStamps).toBe(0);
    expect(snap.queuedMs).toBe(7); // median of 0 and 14
  });
});

describe('the rolling windows', () => {
  it('reports the median decode, not the mean', () => {
    const probe = createLagProbe();
    for (const [i, ms] of [1, 1, 1, 1, 100].entries()) {
      probe.noteFrame(i, 10 + i, ms);
    }
    expect(probe.snapshot(20).decodeMs).toBe(1);
  });

  it('averages the two middle samples of an even count', () => {
    const probe = createLagProbe();
    probe.noteFrame(0, 10, 4);
    probe.noteFrame(1, 11, 6);
    expect(probe.snapshot(12).decodeMs).toBe(5);
  });

  it('ages frame samples out of the one-second window', () => {
    const probe = createLagProbe();
    probe.noteFrame(0, 100, 8);
    expect(probe.snapshot(100 + FRAME_WINDOW_MS - 1).decodeMs).toBe(8);
    expect(probe.snapshot(100 + FRAME_WINDOW_MS).decodeMs).toBeNull();
  });

  it('holds input samples for the wider input window', () => {
    const probe = createLagProbe();
    probe.noteInject(1000, 1010);
    // Past the frame window, well inside the input window: a keypress is far rarer than a frame.
    expect(probe.snapshot(1010 + FRAME_WINDOW_MS + 1).queuedMs).toBe(10);
    expect(probe.snapshot(1010 + INPUT_WINDOW_MS + 1).queuedMs).toBeNull();
  });

  it('starts empty', () => {
    expect(createLagProbe().snapshot(0)).toEqual(NO_LAG);
  });

  it('keeps only the most recent samples once the ring wraps', () => {
    const probe = createLagProbe();
    // Far more frames than the ring holds, all inside one window. An unbounded buffer would answer
    // 50 (700 of them against 300); the ring keeps only the recent tail, which is all 2.
    for (let i = 0; i < 1000; i++) {
      probe.noteFrame(i, i, i < 700 ? 50 : 2);
    }
    expect(probe.snapshot(999).decodeMs).toBe(2);
  });
});

describe('the duty accumulator', () => {
  it('is the share of the last second the main thread spent on frames', () => {
    const probe = createLagProbe();
    for (let i = 0; i < 10; i++) {
      probe.noteMainWork(i * 100 + 1, 50); // 10 frames, 50 ms each, over one second
    }
    expect(probe.snapshot(1000).duty).toBeCloseTo(0.5, 6);
  });

  it('approaches 1 when the frame path fills the second', () => {
    const probe = createLagProbe();
    for (let i = 0; i < 10; i++) {
      probe.noteMainWork(i * 100 + 1, 98);
    }
    expect(probe.snapshot(1000).duty).toBeCloseTo(0.98, 6);
  });

  it('never exceeds 1', () => {
    const probe = createLagProbe();
    probe.noteMainWork(1, 5000);
    expect(probe.snapshot(2).duty).toBe(1);
  });

  it('falls back to zero as the window empties', () => {
    const probe = createLagProbe();
    probe.noteMainWork(10, 500);
    expect(probe.snapshot(10 + FRAME_WINDOW_MS + 1).duty).toBe(0);
  });

  it('is zero with no frames', () => {
    expect(createLagProbe().snapshot(5000).duty).toBe(0);
  });

  it('ignores the decode cost, which is what makes an off-thread decode visible', () => {
    const probe = createLagProbe();
    // A saturating decode that costs this thread almost nothing: `decodeMs` stays large and `duty`
    // collapses. Counting the decode here would report a main thread that is not busy as saturated.
    for (let i = 0; i < 10; i++) {
      probe.noteFrame(i, i * 100 + 1, 95);
      probe.noteMainWork(i * 100 + 1, 0.4);
    }
    const snap = probe.snapshot(1000);
    expect(snap.decodeMs).toBe(95);
    expect(snap.duty).toBeCloseTo(0.004, 6);
  });

  it('counts main-thread work for a frame that was shed before display', () => {
    const probe = createLagProbe();
    // The copy and the handoff happen for every arrival, including the ones latest-wins drops, so
    // occupancy is reported without a frame ever reaching `noteFrame`.
    probe.noteMainWork(10, 200);
    expect(probe.snapshot(20).duty).toBeCloseTo(0.2, 6);
    expect(probe.snapshot(20).decodeMs).toBeNull();
  });
});

describe('formatLag', () => {
  it('renders the four figures', () => {
    expect(
      formatLag({ lagMs: 62, queuedMs: 14, decodeMs: 18, duty: 0.92, rejectedTimeStamps: 0 }),
    ).toBe('lag 62 ms · q 14 ms · dec 18 ms · duty 0.92');
  });

  it('keeps a decimal below 10 ms so a sub-millisecond decode is not zero', () => {
    expect(formatLag({ ...NO_LAG, decodeMs: 0.4 })).toContain('dec 0.4 ms');
  });

  it('shows an em dash for an unmeasured figure', () => {
    expect(formatLag(NO_LAG)).toBe('lag — · q — · dec — · duty 0.00');
  });

  it('distinguishes a rejected timestamp from an unpressed key', () => {
    expect(formatLag({ ...NO_LAG, rejectedTimeStamps: 3 })).toContain('q ?');
  });
});
