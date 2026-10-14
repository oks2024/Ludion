import { describe, expect, it } from 'vitest';
import { IDLE_AFTER_MS, terminalStatus } from './status';

describe('terminalStatus', () => {
  it('is exited once the child has exited, regardless of output recency', () => {
    expect(terminalStatus({ lastOutputAt: 1000, exited: true, now: 1000 })).toBe('exited');
  });

  it('is working just after recent output', () => {
    expect(terminalStatus({ lastOutputAt: 1000, exited: false, now: 1000 + IDLE_AFTER_MS - 1 })).toBe('working');
  });

  it('is idle once output has been quiet past the threshold', () => {
    expect(terminalStatus({ lastOutputAt: 1000, exited: false, now: 1000 + IDLE_AFTER_MS })).toBe('idle');
  });

  it('is idle before any output', () => {
    expect(terminalStatus({ lastOutputAt: null, exited: false, now: 5000 })).toBe('idle');
  });
});
