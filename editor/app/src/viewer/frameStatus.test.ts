import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  NO_FRAMES,
  getFrameStatus,
  sameFrameStatus,
  setFrameStatus,
  subscribeFrameStatus,
  type FrameStatus,
} from './frameStatus';
import { NO_LAG } from './lagProbe';

const STREAMING: FrameStatus = {
  receiving: true,
  width: 1920,
  height: 1080,
  frameIndex: 42,
  fps: 60,
  giStale: false,
  lag: NO_LAG,
};

afterEach(() => {
  setFrameStatus(NO_FRAMES);
});

describe('frameStatus store', () => {
  it('starts with no frames', () => {
    expect(getFrameStatus()).toEqual(NO_FRAMES);
  });

  it('reads back what was written', () => {
    setFrameStatus(STREAMING);
    expect(getFrameStatus()).toBe(STREAMING);
  });

  it('returns the same snapshot between writes', () => {
    setFrameStatus(STREAMING);
    expect(getFrameStatus()).toBe(getFrameStatus());
  });

  it('notifies every subscriber on a write', () => {
    const a = vi.fn();
    const b = vi.fn();
    subscribeFrameStatus(a);
    subscribeFrameStatus(b);
    setFrameStatus(STREAMING);
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(1);
  });

  it('stops notifying after unsubscribe', () => {
    const listener = vi.fn();
    const off = subscribeFrameStatus(listener);
    setFrameStatus(STREAMING);
    off();
    setFrameStatus(NO_FRAMES);
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it('survives a listener unsubscribing during the notification', () => {
    const other = vi.fn();
    const off = subscribeFrameStatus(() => off());
    subscribeFrameStatus(other);
    setFrameStatus(STREAMING);
    expect(other).toHaveBeenCalledTimes(1);
  });

  it('compares two readouts by value, not by identity', () => {
    // The 4 Hz tick rebuilds the object every time; a distinct object saying the same thing must not
    // count as news.
    expect(sameFrameStatus(STREAMING, { ...STREAMING })).toBe(true);
    expect(sameFrameStatus(NO_FRAMES, { ...NO_FRAMES, lag: { ...NO_LAG } })).toBe(true);
  });

  it('notices a change in any displayed field', () => {
    expect(sameFrameStatus(STREAMING, { ...STREAMING, fps: 59 })).toBe(false);
    expect(sameFrameStatus(STREAMING, { ...STREAMING, frameIndex: 43 })).toBe(false);
    expect(sameFrameStatus(STREAMING, { ...STREAMING, receiving: false })).toBe(false);
    expect(sameFrameStatus(STREAMING, { ...STREAMING, width: 1280 })).toBe(false);
    expect(sameFrameStatus(STREAMING, { ...STREAMING, height: 720 })).toBe(false);
    // The Lighter's STALE badge subscribes to this one; a flag that never counted as news would
    // leave the badge showing the state the stream had when some other field last moved.
    expect(sameFrameStatus(STREAMING, { ...STREAMING, giStale: true })).toBe(false);
  });

  it('notices a change inside the nested lag reading', () => {
    expect(sameFrameStatus(STREAMING, { ...STREAMING, lag: { ...NO_LAG, lagMs: 40 } })).toBe(false);
    expect(sameFrameStatus(STREAMING, { ...STREAMING, lag: { ...NO_LAG, queuedMs: 3 } })).toBe(false);
    expect(sameFrameStatus(STREAMING, { ...STREAMING, lag: { ...NO_LAG, decodeMs: 12 } })).toBe(false);
    expect(sameFrameStatus(STREAMING, { ...STREAMING, lag: { ...NO_LAG, duty: 0.5 } })).toBe(false);
    expect(sameFrameStatus(STREAMING, { ...STREAMING, lag: { ...NO_LAG, rejectedTimeStamps: 1 } })).toBe(false);
  });

  it('sees the value the listener was woken for', () => {
    let seen: FrameStatus | null = null;
    const off = subscribeFrameStatus(() => {
      seen = getFrameStatus();
    });
    setFrameStatus(STREAMING);
    off();
    expect(seen).toBe(STREAMING);
  });
});
