import { describe, expect, it } from 'vitest';
import { MAX_SHARE, MIN_SHARE, shareAt } from './split';

describe('shareAt', () => {
  it('maps the pointer to the top pane`s fraction of the container', () => {
    expect(shareAt(300, 100, 400, 0.55)).toBeCloseTo(0.5);
    expect(shareAt(240, 100, 400, 0.55)).toBeCloseTo(0.35);
  });

  it('clamps so neither pane can collapse', () => {
    expect(shareAt(0, 100, 400, 0.55)).toBe(MIN_SHARE);
    expect(shareAt(9999, 100, 400, 0.55)).toBe(MAX_SHARE);
  });

  it('keeps the current share when the container has no height', () => {
    expect(shareAt(300, 0, 0, 0.42)).toBe(0.42);
  });
});
