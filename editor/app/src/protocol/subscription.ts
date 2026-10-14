import type { Channel, ProtocolClient, SubscribeFormat, SubscribeMode } from './client';

export interface ScopedSubscription {
  /** True for an event/frame of this subscription — and for any before the reply names its id. */
  accepts(sub: number): boolean;
  /** Idempotent. Unsubscribes now if the id is known, or as soon as the late reply arrives. */
  close(): void;
}

export interface OpenSubscriptionOptions {
  mode?: SubscribeMode;
  format?: SubscribeFormat;
  onSubscribed?: (sub: number) => void;
  onError?: (err: unknown) => void;
}

export function openSubscription(
  client: Pick<ProtocolClient, 'subscribe' | 'unsubscribe'>,
  channel: Channel,
  options: OpenSubscriptionOptions = {},
): ScopedSubscription {
  let subId: number | null = null;
  let closed = false;
  client
    .subscribe(channel, options.mode, options.format)
    .then((sub) => {
      if (closed) {
        void client.unsubscribe(sub).catch(() => {});
        return;
      }
      subId = sub;
      options.onSubscribed?.(sub);
    })
    .catch((err: unknown) => {
      if (!closed) options.onError?.(err);
    });
  return {
    accepts: (sub) => subId === null || sub === subId,
    close: () => {
      if (closed) return;
      closed = true;
      if (subId !== null) void client.unsubscribe(subId).catch(() => {});
    },
  };
}
