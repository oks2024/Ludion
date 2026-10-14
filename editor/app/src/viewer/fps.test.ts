import { describe, expect, it } from 'vitest';
import { pruneWindow, rollingFps } from './fps';

describe('pruneWindow', () => {
  it('keeps only timestamps within the window ending at now', () => {
    // now = 1000, window = 1000 -> cutoff = 0; strictly-older-than-cutoff (<= 0) drops.
    const kept = pruneWindow([0, 1, 500, 999, 1000], 1000, 1000);
    expect(kept).toEqual([1, 500, 999, 1000]);
  });

  it('drops everything once the window has moved past all samples', () => {
    expect(pruneWindow([10, 20, 30], 5000, 1000)).toEqual([]);
  });

  it('keeps everything when all samples are fresh', () => {
    expect(pruneWindow([4900, 4950, 5000], 5000, 1000)).toEqual([4900, 4950, 5000]);
  });

  it('returns an empty array for no samples', () => {
    expect(pruneWindow([], 1234)).toEqual([]);
  });
});

describe('rollingFps', () => {
  it('counts frames in the last second as the fps for a 1000ms window', () => {
    const stamps = [100, 200, 300, 400, 500, 600, 700, 800, 900, 1000];
    expect(rollingFps(stamps, 1000)).toBe(10);
  });

  it('excludes frames that have aged out of the window', () => {
    // Only 1500 and 1400 survive a window ending at 1500 (cutoff 500, <=500 drops 0/500).
    expect(rollingFps([0, 500, 1400, 1500], 1500)).toBe(2);
  });

  it('scales a sub-second window to a per-second rate', () => {
    // 3 frames in a 500ms window -> 6 fps.
    expect(rollingFps([600, 700, 1000], 1000, 500)).toBe(6);
  });

  it('is zero with no recent frames', () => {
    expect(rollingFps([], 1000)).toBe(0);
  });
});
