import { describe, expect, it, vi } from 'vitest';
import { openSubscription } from './subscription';

/** A client whose `subscribe` reply the test resolves or rejects by hand. */
function fakeClient() {
  let resolve!: (sub: number) => void;
  let reject!: (err: unknown) => void;
  const reply = new Promise<number>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  const unsubscribe = vi.fn(() => Promise.resolve());
  const client = { subscribe: vi.fn(() => reply), unsubscribe };
  return { client, unsubscribe, resolve, reject };
}

const settle = () => new Promise<void>((r) => setTimeout(r, 0));

describe('openSubscription', () => {
  it('unsubscribes the exact id of a reply that arrives after close', async () => {
    const { client, unsubscribe, resolve } = fakeClient();
    const onSubscribed = vi.fn();
    const sub = openSubscription(client, 'frames', { onSubscribed });
    sub.close();
    resolve(7);
    await settle();
    expect(unsubscribe).toHaveBeenCalledExactlyOnceWith(7);
    expect(onSubscribed).not.toHaveBeenCalled();
  });

  it('unsubscribes once when closed after the reply; a second close is a no-op', async () => {
    const { client, unsubscribe, resolve } = fakeClient();
    const onSubscribed = vi.fn();
    const sub = openSubscription(client, 'log', { onSubscribed });
    resolve(3);
    await settle();
    expect(onSubscribed).toHaveBeenCalledExactlyOnceWith(3);
    sub.close();
    sub.close();
    expect(unsubscribe).toHaveBeenCalledExactlyOnceWith(3);
  });

  it('reports a rejection only while open', async () => {
    const live = fakeClient();
    const onErrorLive = vi.fn();
    openSubscription(live.client, 'log', { onError: onErrorLive });
    live.reject(new Error('no'));
    await settle();
    expect(onErrorLive).toHaveBeenCalledTimes(1);

    const closed = fakeClient();
    const onErrorClosed = vi.fn();
    openSubscription(closed.client, 'log', { onError: onErrorClosed }).close();
    closed.reject(new Error('no'));
    await settle();
    expect(onErrorClosed).not.toHaveBeenCalled();
  });

  it('accepts everything before the reply, then only its own id', async () => {
    const { client, resolve } = fakeClient();
    const sub = openSubscription(client, 'frames');
    expect(sub.accepts(99)).toBe(true);
    resolve(5);
    await settle();
    expect(sub.accepts(5)).toBe(true);
    expect(sub.accepts(6)).toBe(false);
  });

  it('forwards the channel, mode and format to subscribe', () => {
    const { client } = fakeClient();
    openSubscription(client, 'frames', { mode: { kind: 'latest' } as never, format: 'qoi' as never });
    expect(client.subscribe).toHaveBeenCalledWith('frames', { kind: 'latest' }, 'qoi');
  });
});
