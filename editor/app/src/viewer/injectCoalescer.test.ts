import { describe, expect, it, vi } from 'vitest';
import { createInjectCoalescer } from './injectCoalescer';
import type { InputInjectParams } from '@ludion/protocol';

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function move(x: number): InputInjectParams {
  return { event: { kind: 'mouse_move', position_bits: [x, 0] } };
}

describe('the mouse-move injection coalescer', () => {
  it('sends a move immediately when idle', () => {
    const send = vi.fn().mockReturnValue(deferred().promise);
    const coalescer = createInjectCoalescer(send);
    coalescer.move(move(1));
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith(move(1));
  });

  it('keeps only one move in flight, parking further offers', () => {
    const first = deferred();
    const send = vi.fn().mockReturnValue(first.promise);
    const coalescer = createInjectCoalescer(send);
    coalescer.move(move(1));
    coalescer.move(move(2));
    coalescer.move(move(3));
    // Still just the one dispatched send — 2 and 3 were parked behind it, not sent.
    expect(send).toHaveBeenCalledTimes(1);
  });

  it('replaces a parked move with a newer one (latest-wins)', async () => {
    const first = deferred();
    const send = vi.fn().mockReturnValueOnce(first.promise).mockReturnValue(deferred().promise);
    const coalescer = createInjectCoalescer(send);
    coalescer.move(move(1));
    coalescer.move(move(2));
    coalescer.move(move(3));
    first.resolve(undefined);
    await Promise.resolve();
    await Promise.resolve();
    // Frame 2 was replaced by frame 3 while frame 1 was in flight — only 1 and 3 ever reach `send`.
    expect(send).toHaveBeenCalledTimes(2);
    expect(send).toHaveBeenNthCalledWith(2, move(3));
  });

  it('releases the gate on a rejected reply so a transient failure cannot wedge the mouse', async () => {
    const first = deferred();
    const send = vi
      .fn()
      .mockReturnValueOnce(first.promise)
      .mockReturnValue(deferred().promise);
    const coalescer = createInjectCoalescer(send);
    coalescer.move(move(1));
    first.reject(new Error('transient'));
    await Promise.resolve();
    await Promise.resolve();
    // Nothing was parked when the rejection landed, so the gate is open again — a fresh move sends now.
    coalescer.move(move(2));
    expect(send).toHaveBeenCalledTimes(2);
    expect(send).toHaveBeenNthCalledWith(2, move(2));
  });

  it('flushMove sends the parked move immediately, ahead of the in-flight one settling', () => {
    const first = deferred();
    const send = vi.fn().mockReturnValueOnce(first.promise).mockReturnValue(deferred().promise);
    const coalescer = createInjectCoalescer(send);
    coalescer.move(move(1));
    coalescer.move(move(2));
    expect(send).toHaveBeenCalledTimes(1);
    coalescer.flushMove();
    // The parked move reached `send` synchronously — no need to wait for the first request to settle.
    expect(send).toHaveBeenCalledTimes(2);
    expect(send).toHaveBeenNthCalledWith(2, move(2));
  });

  it('flushMove is a no-op when nothing is parked', () => {
    const send = vi.fn().mockReturnValue(deferred().promise);
    const coalescer = createInjectCoalescer(send);
    coalescer.move(move(1));
    expect(send).toHaveBeenCalledTimes(1);
    coalescer.flushMove();
    expect(send).toHaveBeenCalledTimes(1);
  });
});
