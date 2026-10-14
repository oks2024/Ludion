import { describe, expect, it } from 'vitest';
import { writeAction, WRITE_THROTTLE_MS } from './save';

describe('writeAction — the throttle', () => {
  it('writes when nothing has been written yet', () => {
    expect(writeAction(null, 0, false)).toEqual({ kind: 'write' });
  });

  it('holds inside the interval, for the remainder', () => {
    expect(writeAction(1000, 1000 + 40, false, 150)).toEqual({ kind: 'hold', afterMs: 110 });
  });

  it('writes once the interval has elapsed', () => {
    expect(writeAction(1000, 1150, false, 150)).toEqual({ kind: 'write' });
    expect(writeAction(1000, 5000, false, 150)).toEqual({ kind: 'write' });
  });

  it('a commit always writes, however recent the last one', () => {
    expect(writeAction(1000, 1001, true, 150)).toEqual({ kind: 'write' });
    expect(writeAction(null, 0, true)).toEqual({ kind: 'write' });
  });

  it('never holds for longer than the interval, whatever the clock says', () => {
    // A backwards clock (a suspended tab, a re-based `performance.now`) must not stall a write.
    const held = writeAction(9000, 1000, false, 150);
    expect(held).toEqual({ kind: 'hold', afterMs: 150 });
  });

  it('holds are within [0, interval] across a sweep', () => {
    for (let elapsed = -500; elapsed < WRITE_THROTTLE_MS; elapsed += 7) {
      const action = writeAction(0, elapsed, false);
      expect(action.kind).toBe('hold');
      if (action.kind === 'hold') {
        expect(action.afterMs).toBeGreaterThanOrEqual(0);
        expect(action.afterMs).toBeLessThanOrEqual(WRITE_THROTTLE_MS);
      }
    }
  });

  it('the default interval matches the host watcher debounce', () => {
    expect(WRITE_THROTTLE_MS).toBe(150);
  });
});
