import { describe, expect, it } from 'vitest';
import { shouldPoll } from './poll';

describe('shouldPoll', () => {
  it('polls only when connected, visible, and idle', () => {
    expect(shouldPoll({ connected: true, visible: true, inFlight: false })).toBe(true);
  });

  it('never polls while disconnected', () => {
    expect(shouldPoll({ connected: false, visible: true, inFlight: false })).toBe(false);
  });

  it('never polls while the surface is hidden', () => {
    expect(shouldPoll({ connected: true, visible: false, inFlight: false })).toBe(false);
  });

  it('never starts an overlapping poll while one is in flight', () => {
    expect(shouldPoll({ connected: true, visible: true, inFlight: true })).toBe(false);
  });
});
