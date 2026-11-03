import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Command } from '@ludion/protocol';
import { classifyMessage, CONNECT_TIMEOUT_MS, ProtocolClient, ProtocolClientError, ProtocolError } from './client';

describe('classifyMessage (protocol demux)', () => {
  it('classifies an unsafe u64 in a response as a bigint', () => {
    const msg = classifyMessage('{"id":1,"ok":{"kind":"component","value":{"seed":18446744073709551614}}}');
    if (msg.kind !== 'response') throw new Error('expected a response');
    expect((msg.response as unknown as { ok: { value: { seed: unknown } } }).ok.value.seed).toBe(18446744073709551614n);
  });

  it('classifies an ok Response by its id field', () => {
    const raw = JSON.stringify({ id: 42, ok: { kind: 'ack' }, frame: 1203 });
    const msg = classifyMessage(raw);
    expect(msg.kind).toBe('response');
    if (msg.kind !== 'response') throw new Error('unreachable');
    expect(msg.response.id).toBe(42);
    const body = msg.response as unknown as { ok?: unknown };
    expect(body.ok).toEqual({ kind: 'ack' });
  });

  it('classifies an error Response by its id field', () => {
    const raw = JSON.stringify({
      id: 7,
      error: { code: 'L1042', message: 'missing texture', related: [] },
    });
    const msg = classifyMessage(raw);
    expect(msg.kind).toBe('response');
    if (msg.kind !== 'response') throw new Error('unreachable');
    const body = msg.response as unknown as { error?: { code: string } };
    expect(body.error?.code).toBe('L1042');
  });

  it('classifies an EventMessage by its event/sub/seq fields', () => {
    const raw = JSON.stringify({
      event: 'log',
      sub: 7,
      seq: 981,
      frame: 1204,
      data: { level: 'info', message: 'loaded' },
    });
    const msg = classifyMessage(raw);
    expect(msg.kind).toBe('event');
    if (msg.kind !== 'event') throw new Error('unreachable');
    expect(msg.event.sub).toBe(7);
    expect(msg.event.seq).toBe(981);
  });

  it('distinguishes responses from events sharing no fields in common', () => {
    // A response never has 'event'/'sub'/'seq' together; an event never has a bare numeric 'id'.
    const responseMsg = classifyMessage(JSON.stringify({ id: 1, ok: { kind: 'ack' } }));
    const eventMsg = classifyMessage(JSON.stringify({ event: 'scene.changed', sub: 1, seq: 1, data: {} }));
    expect(responseMsg.kind).toBe('response');
    expect(eventMsg.kind).toBe('event');
  });

  it('rejects malformed JSON', () => {
    expect(() => classifyMessage('{not json')).toThrow(ProtocolClientError);
  });

  it('rejects a JSON value that is not an object', () => {
    expect(() => classifyMessage('42')).toThrow(ProtocolClientError);
    expect(() => classifyMessage('null')).toThrow(ProtocolClientError);
    expect(() => classifyMessage('"hello"')).toThrow(ProtocolClientError);
  });

  it('rejects an object matching neither shape', () => {
    expect(() => classifyMessage(JSON.stringify({ foo: 'bar' }))).toThrow(ProtocolClientError);
  });
});

/** A scripted stand-in for the browser `WebSocket`: the test fires `open` / `message` / `close`. */
class FakeWebSocket {
  static OPEN = 1;
  static last: FakeWebSocket;
  readyState = 0;
  binaryType = 'blob';
  readonly sent: string[] = [];
  private readonly listeners = new Map<string, Array<(ev: unknown) => void>>();

  readonly url: string;

  constructor(url: string) {
    this.url = url;
    FakeWebSocket.last = this;
  }

  addEventListener(type: string, fn: (ev: unknown) => void): void {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), fn]);
  }

  send(data: string): void {
    this.sent.push(data);
  }

  close(): void {
    this.fire('close', {});
  }

  fire(type: string, ev: unknown): void {
    if (type === 'open') this.readyState = FakeWebSocket.OPEN;
    if (type === 'close') this.readyState = 3;
    for (const fn of this.listeners.get(type) ?? []) fn(ev);
  }

  reply(body: unknown): void {
    this.fire('message', { data: JSON.stringify(body) });
  }
}

describe('ProtocolClient.connect', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  function start(): { client: ProtocolClient; result: Promise<unknown>; ws: FakeWebSocket } {
    vi.stubGlobal('WebSocket', FakeWebSocket);
    const client = new ProtocolClient();
    const result = client.connect('ws://127.0.0.1:1');
    const ws = FakeWebSocket.last;
    ws.fire('open', {});
    return { client, result, ws };
  }

  it('resolves with the ServerHello and sends the Hello', async () => {
    const { result, ws } = start();
    const hello = { protocol_version: 1, schema_hash: 'abc', engine: 'ludion' };
    ws.reply(hello);
    await expect(result).resolves.toEqual(hello);
    expect(JSON.parse(ws.sent[0]).client).toBe('ludion-viewer');
  });

  it('rejects when the server closes before replying', async () => {
    const { result, ws } = start();
    ws.close();
    await expect(result).rejects.toBeInstanceOf(ProtocolClientError);
  }, 500);

  it('rejects and closes the socket when no reply arrives within the timeout', async () => {
    vi.useFakeTimers();
    const { result, ws } = start();
    const settled = expect(result).rejects.toThrow(/no handshake reply/);
    await vi.advanceTimersByTimeAsync(CONNECT_TIMEOUT_MS + 1);
    await settled;
    expect(ws.readyState).toBe(3);
  }, 500);

  it.each([
    ['L3003', '/protocol_version'],
    ['L3001', null],
  ])('rejects with the engine error envelope %s', async (code, path) => {
    const { result, ws } = start();
    ws.reply({ id: 0, error: { code, message: 'this engine speaks 2', path, help: 'regenerate', related: [] } });
    const err = await result.catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ProtocolError);
    const pe = err as ProtocolError;
    expect([pe.code, pe.diagnosticMessage, pe.help]).toEqual([code, 'this engine speaks 2', 'regenerate']);
  });

  it('a close after a settled handshake neither throws nor re-settles', async () => {
    const { client, result, ws } = start();
    ws.reply({ protocol_version: 1, schema_hash: 'abc', engine: 'ludion' });
    await result;
    const closed = vi.fn();
    client.onClose(closed);
    expect(() => ws.close()).not.toThrow();
    expect(closed).toHaveBeenCalledTimes(1);
  });
});

describe('ProtocolClient.callFull', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  async function connected(): Promise<{ client: ProtocolClient; ws: FakeWebSocket }> {
    vi.stubGlobal('WebSocket', FakeWebSocket);
    const client = new ProtocolClient();
    const result = client.connect('ws://127.0.0.1:1');
    const ws = FakeWebSocket.last;
    ws.fire('open', {});
    ws.reply({ protocol_version: 1, schema_hash: 'abc', engine: 'ludion' });
    await result;
    return { client, ws };
  }

  it('carries the sim frame and the gameplay frame of the reply', async () => {
    const { client, ws } = await connected();
    const call = client.callFull({ method: 'sim.hash' } as Command);
    const { id } = JSON.parse(ws.sent[1]) as { id: number };
    ws.reply({ id, ok: { kind: 'ack' }, frame: 50, game_frame: 20 });
    await expect(call).resolves.toEqual({ body: { kind: 'ack' }, frame: 50, gameFrame: 20 });
  });

  it('reads a reply without game_frame as null', async () => {
    const { client, ws } = await connected();
    const call = client.callFull({ method: 'sim.hash' } as Command);
    const { id } = JSON.parse(ws.sent[1]) as { id: number };
    ws.reply({ id, ok: { kind: 'ack' }, frame: 50 });
    await expect(call).resolves.toMatchObject({ frame: 50, gameFrame: null });
  });

  it('reports the request id before the request is sent', async () => {
    const { client, ws } = await connected();
    const sentBefore = ws.sent.length;
    let seen: { id: number; sent: number } | null = null;
    const call = client.callFull({ method: 'sim.hash' } as Command, (id) => {
      seen = { id, sent: ws.sent.length };
    });
    const { id } = JSON.parse(ws.sent[sentBefore]) as { id: number };
    expect(seen).toEqual({ id, sent: sentBefore });
    ws.reply({ id, ok: { kind: 'ack' }, frame: 1 });
    await call;
  });
});

describe('ProtocolClient listeners', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  async function connected(): Promise<{ client: ProtocolClient; ws: FakeWebSocket }> {
    vi.stubGlobal('WebSocket', FakeWebSocket);
    const client = new ProtocolClient();
    const result = client.connect('ws://127.0.0.1:1');
    const ws = FakeWebSocket.last;
    ws.fire('open', {});
    ws.reply({ protocol_version: 1, schema_hash: 'abc', engine: 'ludion' });
    await result;
    return { client, ws };
  }

  it('a throwing event listener does not starve the ones after it', async () => {
    const { client, ws } = await connected();
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const second = vi.fn();
    client.onEvent(() => {
      throw new Error('boom');
    });
    client.onEvent(second);
    ws.reply({ event: 'log', sub: 1, seq: 0 });
    expect(second).toHaveBeenCalledTimes(1);
  });

  it('a throwing close listener does not starve the ones after it', async () => {
    const { client, ws } = await connected();
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const second = vi.fn();
    client.onClose(() => {
      throw new Error('boom');
    });
    client.onClose(second);
    ws.close();
    expect(second).toHaveBeenCalledTimes(1);
  });
});
