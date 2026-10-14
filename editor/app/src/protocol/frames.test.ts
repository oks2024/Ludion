import { describe, expect, it } from 'vitest';
import { decodeFrameHeader, FrameDecodeError, LFR1_HEADER_LEN } from './frames';

interface HeaderFields {
  readonly format: number; // raw wire byte
  readonly sub: bigint;
  readonly frameIndex: bigint;
  readonly width: number;
  readonly height: number;
  readonly payload: Uint8Array;
  /** Byte 5 verbatim, so a test can set bits this build does not know about. Default 0. */
  readonly flags?: number;
  /** Bytes 6 and 7 verbatim — the two still-reserved ones. Default 0. */
  readonly reserved?: readonly [number, number];
}

/** Hand-builds a valid `LFR1` message buffer (test-only helper). */
function buildFrame(fields: HeaderFields): ArrayBuffer {
  const buf = new ArrayBuffer(LFR1_HEADER_LEN + fields.payload.length);
  const view = new DataView(buf);
  const bytes = new Uint8Array(buf);
  bytes.set([0x4c, 0x46, 0x52, 0x31], 0); // "LFR1"
  view.setUint8(4, fields.format);
  view.setUint8(5, fields.flags ?? 0);
  const [six, seven] = fields.reserved ?? [0, 0];
  view.setUint8(6, six);
  view.setUint8(7, seven);
  view.setBigUint64(8, fields.sub, true);
  view.setBigUint64(16, fields.frameIndex, true);
  view.setUint32(24, fields.width, true);
  view.setUint32(28, fields.height, true);
  view.setUint32(32, fields.payload.length, true);
  bytes.set(fields.payload, LFR1_HEADER_LEN);
  return buf;
}

describe('decodeFrameHeader', () => {
  it('decodes a well-formed QOI-format frame', () => {
    const payload = new Uint8Array([1, 2, 3, 4, 5]);
    const buf = buildFrame({ format: 0, sub: 7n, frameIndex: 1203n, width: 1920, height: 1080, payload });
    const decoded = decodeFrameHeader(buf);
    expect(decoded.header).toEqual({
      format: 'QOI',
      sub: 7,
      frameIndex: 1203,
      width: 1920,
      height: 1080,
      payloadLen: 5,
      giStale: false,
    });
    expect(Array.from(decoded.payload)).toEqual([1, 2, 3, 4, 5]);
  });

  it('decodes a well-formed LZ4_RGBA-format frame', () => {
    const payload = new Uint8Array([9, 8, 7]);
    const buf = buildFrame({ format: 1, sub: 0n, frameIndex: 0n, width: 4, height: 4, payload });
    const decoded = decodeFrameHeader(buf);
    expect(decoded.header.format).toBe('LZ4_RGBA');
  });

  it('decodes a well-formed RAW_RGBA-format frame', () => {
    const payload = new Uint8Array(2 * 2 * 4).fill(200);
    const buf = buildFrame({ format: 2, sub: 0n, frameIndex: 0n, width: 2, height: 2, payload });
    const decoded = decodeFrameHeader(buf);
    expect(decoded.header.format).toBe('RAW_RGBA');
    expect(decoded.payload.length).toBe(2 * 2 * 4);
  });

  it('handles large sub_id / frame_index within the safe-integer range', () => {
    const buf = buildFrame({
      format: 0,
      sub: 9_007_199_254_740_000n,
      frameIndex: 42n,
      width: 1,
      height: 1,
      payload: new Uint8Array(0),
    });
    const decoded = decodeFrameHeader(buf);
    expect(decoded.header.sub).toBe(9_007_199_254_740_000);
  });

  it('reads the gi_stale flag out of byte 5 bit 0', () => {
    const held = buildFrame({
      format: 0,
      flags: 0b0000_0001,
      sub: 1n,
      frameIndex: 2n,
      width: 8,
      height: 8,
      payload: new Uint8Array(0),
    });
    expect(decodeFrameHeader(held).header.giStale).toBe(true);
  });

  it('reads a cleared flags byte as light that is not stale', () => {
    const fresh = buildFrame({
      format: 0,
      flags: 0,
      sub: 1n,
      frameIndex: 2n,
      width: 8,
      height: 8,
      payload: new Uint8Array(0),
    });
    expect(decodeFrameHeader(fresh).header.giStale).toBe(false);
  });

  it('ignores every flags bit but bit 0, and bytes 6..8, which stay reserved', () => {
    // A header from a build that means something by the bits this one does not know: the frame is
    // still readable and only the flag this build understands is reported.
    const future = buildFrame({
      format: 0,
      flags: 0b1111_1110,
      reserved: [0xff, 0xff],
      sub: 3n,
      frameIndex: 4n,
      width: 2,
      height: 2,
      payload: new Uint8Array(0),
    });
    const decoded = decodeFrameHeader(future);
    expect(decoded.header.giStale).toBe(false);
    expect(decoded.header).toEqual({
      format: 'QOI',
      sub: 3,
      frameIndex: 4,
      width: 2,
      height: 2,
      payloadLen: 0,
      giStale: false,
    });
    // The same header with bit 0 set as well still decodes to exactly one flag.
    const both = buildFrame({
      format: 0,
      flags: 0b1111_1111,
      reserved: [0xff, 0xff],
      sub: 3n,
      frameIndex: 4n,
      width: 2,
      height: 2,
      payload: new Uint8Array(0),
    });
    expect(decodeFrameHeader(both).header.giStale).toBe(true);
  });

  it('rejects a bad magic number', () => {
    const buf = buildFrame({ format: 0, sub: 0n, frameIndex: 0n, width: 1, height: 1, payload: new Uint8Array(0) });
    new Uint8Array(buf)[0] = 0x58; // corrupt "L" -> "X"
    expect(() => decodeFrameHeader(buf)).toThrow(FrameDecodeError);
    expect(() => decodeFrameHeader(buf)).toThrow(/magic/);
  });

  it('rejects an unknown format byte', () => {
    // 0, 1 and 2 are the three formats that exist; 3 is the first byte no build has ever meant.
    const buf = buildFrame({ format: 3, sub: 0n, frameIndex: 0n, width: 1, height: 1, payload: new Uint8Array(0) });
    expect(() => decodeFrameHeader(buf)).toThrow(/format/);
  });

  it('rejects a buffer shorter than the fixed header', () => {
    expect(() => decodeFrameHeader(new ArrayBuffer(35))).toThrow(FrameDecodeError);
    expect(() => decodeFrameHeader(new ArrayBuffer(0))).toThrow(/truncated/);
  });

  it('rejects a buffer whose declared payload_len exceeds the bytes actually present', () => {
    const buf = buildFrame({
      format: 0,
      sub: 0n,
      frameIndex: 0n,
      width: 1,
      height: 1,
      payload: new Uint8Array([1, 2, 3]),
    });
    // Truncate the message after the header, discarding the payload the header promises.
    const truncated = buf.slice(0, LFR1_HEADER_LEN + 1);
    expect(() => decodeFrameHeader(truncated)).toThrow(/truncated/);
  });
});
