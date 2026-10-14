// End-to-end integration test against a *real* engine over the wire.
//
// This is opt-in: it runs only when `LUDION_WS_URL` names a live `ludion serve` endpoint, and is
// skipped otherwise (so `npm test` stays green with no engine around). Node 22+ ships a global
// `WebSocket`, so the same `ProtocolClient` the browser uses connects unmodified here. It exercises
// the full Phase 4 read-only surface the viewer depends on: the handshake, `query.entities`, run
// control (`sim.pause` / `sim.step` / `sim.resume`), and one `frames` subscription per codec, each
// decoded with the decoder the viewer really uses — the exact path `useFrameStream` drives in the UI.
//
// To run it:
//   cargo build -p ludion-cli
//   target/debug/ludion serve --scene examples/gallery/sprites.yaml --addr 127.0.0.1:0 --paused
//   # parse the "listening on ws://..." line, then:
//   LUDION_WS_URL=ws://127.0.0.1:<port> npm test

import { afterAll, describe, expect, it } from 'vitest';
import { ProtocolClient } from '../protocol/client';
import { decodeFramePayload } from '../protocol/decode';
import type { DecodedFrame } from '../protocol/frames';
import { decompressLz4Block } from '../protocol/lz4';
import { decodeQoi } from '../protocol/qoi';

// `process` is a Node global not in this project's DOM-oriented lib types; reach it structurally so
// the test typechecks under `tsc -b` without pulling Node globals into the app's type surface.
const env = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {};
const url = env.LUDION_WS_URL;

describe.skipIf(url === undefined || url === '')('ProtocolClient against a live engine', () => {
  const client = new ProtocolClient();

  afterAll(() => {
    client.close();
  });

  it('completes the handshake and reports a schema hash', async () => {
    const hello = await client.connect(url as string);
    expect(hello.protocol_version).toBe(1);
    expect(hello.schema_hash).toMatch(/^[0-9a-f]{16}$/);
    expect(hello.engine_version.length).toBeGreaterThan(0);
  });

  it('lists entities via query.entities', async () => {
    const body = await client.call({ method: 'query.entities', params: { filter: [], include: [] } });
    expect(body.kind).toBe('query_result');
    if (body.kind !== 'query_result') {
      throw new Error('unreachable');
    }
    expect(Array.isArray(body.entities)).toBe(true);
  });

  it('drives run control: pause, step (with hash), resume', async () => {
    const paused = await client.call({ method: 'sim.pause' });
    expect(paused.kind).toBe('ack');

    const stepped = await client.call({ method: 'sim.step', params: { ticks: 2, hash: true } });
    expect(stepped.kind).toBe('stepped');
    if (stepped.kind === 'stepped') {
      // With hash: true the resulting WorldHash comes back as 16 hex digits.
      expect(String(stepped.hash)).toMatch(/^[0-9a-f]{16}$/);
    }

    const resumed = await client.call({ method: 'sim.resume' });
    expect(resumed.kind).toBe('ack');
  });

  /** Opens a `frames` subscription, resolves with its first frame, and closes it again. The listener
   *  is attached after the subscribe resolves and filters on the sub id, so a frame still draining
   *  from an earlier subscription can never be mistaken for this one's. */
  async function firstFrame(format?: 'QOI' | 'LZ4_RGBA' | 'RAW_RGBA'): Promise<DecodedFrame> {
    const sub = await client.subscribe('frames', undefined, format);
    try {
      return await new Promise<DecodedFrame>((resolve, reject) => {
        const timer = setTimeout(() => {
          off();
          reject(new Error('no frame arrived within 10s'));
        }, 10_000);
        const off = client.onFrame((f) => {
          if (f.header.sub !== sub) {
            return;
          }
          off();
          clearTimeout(timer);
          resolve(f);
        });
      });
    } finally {
      await client.unsubscribe(sub);
    }
  }

  it('subscribes to frames without a format and gets the wire default, QOI', async () => {
    // The regression test for the wire DEFAULT, not for what the viewer asks: `stream.subscribe` with
    // no `format` must keep meaning QOI, or every subscriber written before the field existed —
    // recorded replays included — silently changes codec.
    const frame = await firstFrame();

    expect(frame.header.format).toBe('QOI');
    const image = decodeQoi(frame.payload);
    expect(image.width).toBeGreaterThan(0);
    expect(image.height).toBeGreaterThan(0);
    expect(image.rgba.length).toBe(image.width * image.height * 4);
    // Surface the observed dimensions in the test output (the report wants them).
    console.log(`decoded QOI frame ${frame.header.frameIndex}: ${image.width}x${image.height}`);
  });

  it('subscribes to frames as LZ4_RGBA and decompresses the first one', async () => {
    const frame = await firstFrame('LZ4_RGBA');

    expect(frame.header.format).toBe('LZ4_RGBA');
    const { width, height } = frame.header;
    expect(width).toBeGreaterThan(0);
    expect(height).toBeGreaterThan(0);
    // An LZ4 block is not self-describing: the header's dimensions size the buffer, and the decoder
    // throws unless the block fills it exactly — which is the real assertion here.
    const rgba = new Uint8ClampedArray(width * height * 4);
    decompressLz4Block(frame.payload, rgba);
    console.log(
      `decompressed LZ4 frame ${frame.header.frameIndex}: ${width}x${height} ` +
        `from ${frame.payload.length} bytes`,
    );
  });

  it('subscribes to frames as RAW_RGBA and gets exactly width*height*4 bytes', async () => {
    const frame = await firstFrame('RAW_RGBA');

    expect(frame.header.format).toBe('RAW_RGBA');
    const { width, height } = frame.header;
    expect(width).toBeGreaterThan(0);
    expect(height).toBeGreaterThan(0);
    // The whole of RAW's contract: tightly packed, no padding, no stride — so the length IS the
    // assertion, and the "decode" is the copy that follows it.
    expect(frame.payload.length).toBe(width * height * 4);
    const rgba = new Uint8ClampedArray(width * height * 4);
    decodeFramePayload('RAW_RGBA', frame.payload, rgba);
    console.log(`copied RAW frame ${frame.header.frameIndex}: ${width}x${height}`);
  });
});
