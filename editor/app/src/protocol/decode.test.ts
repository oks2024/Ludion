import { describe, expect, it } from 'vitest';
import { canDecodeFormat, decodeFramePayload, RawDecodeError } from './decode';
import { Lz4DecodeError } from './lz4';
import { QoiDecodeError } from './qoi';

/** The 2x1 RGBA image every arm below is asked to produce, in every codec. */
const PIXELS = [200, 100, 50, 255, 10, 20, 30, 255];

/** The same two pixels as a QOI stream (header + two RGB chunks + end marker). */
function qoiBytes(): Uint8Array {
  return new Uint8Array([
    0x71, 0x6f, 0x69, 0x66, // "qoif"
    0, 0, 0, 2, // width
    0, 0, 0, 1, // height
    4, 0, // channels, colorspace
    0xfe, 200, 100, 50,
    0xfe, 10, 20, 30,
    0, 0, 0, 0, 0, 0, 0, 1, // end marker
  ]);
}

/** The same two pixels as one LZ4 block: a single literal run, no match. */
function lz4Bytes(): Uint8Array {
  return new Uint8Array([PIXELS.length << 4, ...PIXELS]);
}

function decode(format: 'QOI' | 'LZ4_RGBA' | 'RAW_RGBA', payload: Uint8Array): number[] {
  const out = new Uint8ClampedArray(PIXELS.length);
  decodeFramePayload(format, payload, out);
  return Array.from(out);
}

describe('decodeFramePayload', () => {
  it('produces the same pixels from all three formats', () => {
    expect(decode('QOI', qoiBytes())).toEqual(PIXELS);
    expect(decode('LZ4_RGBA', lz4Bytes())).toEqual(PIXELS);
    expect(decode('RAW_RGBA', new Uint8Array(PIXELS))).toEqual(PIXELS);
  });

  it('copies a RAW_RGBA payload over whatever the recycled buffer held', () => {
    const out = new Uint8ClampedArray(PIXELS.length).fill(0xff);
    decodeFramePayload('RAW_RGBA', new Uint8Array(PIXELS), out);
    expect(Array.from(out)).toEqual(PIXELS);
  });

  it('rejects a RAW_RGBA payload shorter than the frame', () => {
    // The dangerous one: a short copy would leave the tail of a recycled buffer showing the previous
    // frame, which looks like an image rather than like a failure.
    const short = new Uint8Array(PIXELS.slice(0, 4));
    expect(() => decode('RAW_RGBA', short)).toThrow(RawDecodeError);
    expect(() => decode('RAW_RGBA', short)).toThrow(/4 bytes for a 8-byte frame/);
  });

  it('rejects a RAW_RGBA payload longer than the frame', () => {
    const long = new Uint8Array([...PIXELS, 1, 2, 3, 4]);
    expect(() => decode('RAW_RGBA', long)).toThrow(/12 bytes for a 8-byte frame/);
  });

  it('surfaces the codec-specific error type for a malformed payload', () => {
    expect(() => decode('QOI', new Uint8Array(4))).toThrow(QoiDecodeError);
    // A token promising more literals than the block holds.
    expect(() => decode('LZ4_RGBA', new Uint8Array([0xf0, 1, 2]))).toThrow(Lz4DecodeError);
  });
});

describe('canDecodeFormat', () => {
  it('accepts every format this build decodes', () => {
    expect(canDecodeFormat('QOI')).toBe(true);
    expect(canDecodeFormat('LZ4_RGBA')).toBe(true);
    expect(canDecodeFormat('RAW_RGBA')).toBe(true);
  });
});
