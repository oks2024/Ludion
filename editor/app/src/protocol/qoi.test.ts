import { describe, expect, it } from 'vitest';
import { decodeQoi, QoiDecodeError } from './qoi';

const QOI_END_MARKER = [0, 0, 0, 0, 0, 0, 0, 1];

/** Assembles a full QOI byte stream from a pixel-chunk byte sequence (test-only helper). */
function buildQoi(width: number, height: number, chunks: number[]): Uint8Array {
  const header = [
    0x71, 0x6f, 0x69, 0x66, // "qoif"
    (width >>> 24) & 0xff, (width >>> 16) & 0xff, (width >>> 8) & 0xff, width & 0xff,
    (height >>> 24) & 0xff, (height >>> 16) & 0xff, (height >>> 8) & 0xff, height & 0xff,
    4, // channels
    0, // colorspace
  ];
  return new Uint8Array([...header, ...chunks, ...QOI_END_MARKER]);
}

type Pixel = readonly [number, number, number, number];

function expectPixels(bytes: Uint8Array, width: number, height: number, pixels: readonly Pixel[]): void {
  const image = decodeQoi(bytes);
  expect(image.width).toBe(width);
  expect(image.height).toBe(height);
  expect(image.rgba.length).toBe(width * height * 4);
  for (let i = 0; i < pixels.length; i++) {
    const [r, g, b, a] = pixels[i] as Pixel;
    const o = i * 4;
    expect([image.rgba[o], image.rgba[o + 1], image.rgba[o + 2], image.rgba[o + 3]]).toEqual([r, g, b, a]);
  }
}

describe('decodeQoi', () => {
  it('decodes QOI_OP_RGB chunks', () => {
    const bytes = buildQoi(2, 1, [0xfe, 200, 100, 50, 0xfe, 10, 20, 30]);
    expectPixels(bytes, 2, 1, [
      [200, 100, 50, 255],
      [10, 20, 30, 255],
    ]);
  });

  it('decodes QOI_OP_RGBA chunks, carrying alpha', () => {
    const bytes = buildQoi(2, 1, [0xff, 10, 20, 30, 128, 0xff, 1, 2, 3, 4]);
    expectPixels(bytes, 2, 1, [
      [10, 20, 30, 128],
      [1, 2, 3, 4],
    ]);
  });

  it('decodes QOI_OP_INDEX against the default zeroed running array', () => {
    // Index 0 of the running array starts at (0,0,0,0) before any pixel is decoded.
    const bytes = buildQoi(1, 1, [0x00]);
    expectPixels(bytes, 1, 1, [[0, 0, 0, 0]]);
  });

  it('decodes QOI_OP_INDEX recalling a previously-seen pixel', () => {
    // hashIndex(50,100,150,255) = (150 + 500 + 1050 + 2805) % 64 = 4505 % 64 = 25 = 0b011001.
    const bytes = buildQoi(3, 1, [0xfe, 50, 100, 150, 0x19, 0xfe, 5, 6, 7]);
    expectPixels(bytes, 3, 1, [
      [50, 100, 150, 255],
      [50, 100, 150, 255],
      [5, 6, 7, 255],
    ]);
  });

  it('decodes QOI_OP_DIFF relative to the previous pixel', () => {
    // dr=+1, dg=0, db=-1, each biased by +2: tag = 0b01_11_10_01 = 0x79.
    const bytes = buildQoi(3, 1, [0xfe, 100, 100, 100, 0x79, 0xfe, 9, 9, 9]);
    expectPixels(bytes, 3, 1, [
      [100, 100, 100, 255],
      [101, 100, 99, 255],
      [9, 9, 9, 255],
    ]);
  });

  it('decodes QOI_OP_LUMA relative to the previous pixel', () => {
    // dg=+10 (bias +32 -> 42 -> 0x2A), dr-dg=-3 (bias +8 -> 5), db-dg=+2 (bias +8 -> 10).
    // byte1 = 0x80 | 0x2A = 0xAA; byte2 = (5 << 4) | 10 = 0x5A.
    const bytes = buildQoi(3, 1, [0xfe, 100, 100, 100, 0xaa, 0x5a, 0xfe, 1, 2, 3]);
    expectPixels(bytes, 3, 1, [
      [100, 100, 100, 255],
      [107, 110, 112, 255],
      [1, 2, 3, 255],
    ]);
  });

  it('decodes QOI_OP_RUN, repeating the previous pixel', () => {
    // Stored run byte = (total run-chunk pixels - 1) = 4, tag = 0xC0 | 4 = 0xC4.
    // Total same-colored pixels = 1 (explicit RGB) + 5 (the run chunk's own output) = 6.
    const bytes = buildQoi(7, 1, [0xfe, 77, 88, 99, 0xc4, 0xfe, 1, 1, 1]);
    expectPixels(bytes, 7, 1, [
      [77, 88, 99, 255],
      [77, 88, 99, 255],
      [77, 88, 99, 255],
      [77, 88, 99, 255],
      [77, 88, 99, 255],
      [77, 88, 99, 255],
      [1, 1, 1, 255],
    ]);
  });

  it('rejects a buffer with a bad magic number', () => {
    const bytes = buildQoi(1, 1, [0x00]);
    bytes[0] = 0x00;
    expect(() => decodeQoi(bytes)).toThrow(QoiDecodeError);
  });

  it('rejects a buffer shorter than the header + padding', () => {
    expect(() => decodeQoi(new Uint8Array(10))).toThrow(QoiDecodeError);
  });

  it('decodes into a caller-provided output buffer instead of allocating one', () => {
    const bytes = buildQoi(2, 1, [0xfe, 200, 100, 50, 0xfe, 10, 20, 30]);
    const out = new Uint8ClampedArray(2 * 1 * 4);
    const image = decodeQoi(bytes, out);
    expect(image.rgba).toBe(out); // no fresh allocation — the same buffer comes back out
    expect(Array.from(out)).toEqual([200, 100, 50, 255, 10, 20, 30, 255]);
  });

  it('rejects a caller-provided output buffer of the wrong size', () => {
    const bytes = buildQoi(2, 1, [0xfe, 200, 100, 50, 0xfe, 10, 20, 30]);
    const out = new Uint8ClampedArray(4 * 4); // sized for 4 pixels, not 2
    expect(() => decodeQoi(bytes, out)).toThrow(QoiDecodeError);
    expect(() => decodeQoi(bytes, out)).toThrow(/output buffer size mismatch/);
  });
});
