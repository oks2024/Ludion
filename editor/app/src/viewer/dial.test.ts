import { describe, expect, it, vi } from 'vitest';
import { createDialGate, dial } from './dial';

/** A connector whose handshake the test settles by hand. */
function fakeNext() {
  let resolve!: (hello: string) => void;
  let reject!: (err: unknown) => void;
  const handshake = new Promise<string>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  const close = vi.fn();
  return { next: { connect: () => handshake, close }, close, resolve, reject };
}

describe('dial', () => {
  it('connects and keeps the client open when nothing superseded it', async () => {
    const gate = createDialGate();
    const f = fakeNext();
    const outcome = dial(f.next, 'ws://x', gate);
    f.resolve('hello');
    expect(await outcome).toEqual({ kind: 'connected', hello: 'hello' });
    expect(f.close).not.toHaveBeenCalled();
  });

  it('reports a failure and closes the client', async () => {
    const f = fakeNext();
    const outcome = dial(f.next, 'ws://x', createDialGate());
    const error = new Error('refused');
    f.reject(error);
    expect(await outcome).toEqual({ kind: 'failed', error });
    expect(f.close).toHaveBeenCalledTimes(1);
  });

  it('discards a dial invalidated mid-handshake and closes its client', async () => {
    const gate = createDialGate();
    const f = fakeNext();
    const outcome = dial(f.next, 'ws://x', gate);
    gate.invalidate();
    f.resolve('hello');
    expect(await outcome).toEqual({ kind: 'superseded' });
    expect(f.close).toHaveBeenCalledTimes(1);
  });

  it('a newer dial supersedes the first', async () => {
    const gate = createDialGate();
    const first = fakeNext();
    const second = fakeNext();
    const a = dial(first.next, 'ws://x', gate);
    const b = dial(second.next, 'ws://x', gate);
    first.resolve('one');
    second.resolve('two');
    expect(await a).toEqual({ kind: 'superseded' });
    expect(await b).toEqual({ kind: 'connected', hello: 'two' });
    expect(first.close).toHaveBeenCalledTimes(1);
    expect(second.close).not.toHaveBeenCalled();
  });

  it('a failure after invalidate is superseded, not failed', async () => {
    const gate = createDialGate();
    const f = fakeNext();
    const outcome = dial(f.next, 'ws://x', gate);
    gate.invalidate();
    f.reject(new Error('late'));
    expect(await outcome).toEqual({ kind: 'superseded' });
    expect(f.close).toHaveBeenCalledTimes(1);
  });
});
