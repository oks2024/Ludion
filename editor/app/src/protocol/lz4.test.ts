import { describe, expect, it } from 'vitest';
import { decompressLz4Block, Lz4DecodeError } from './lz4';

/** One LZ4 sequence: a literal run, optionally followed by a match. The final sequence of a block
 *  carries no match — the input simply ends after its literals. */
interface Sequence {
  readonly literals: readonly number[];
  /** `length` is the real match length (>= 4); the encoding's -4 bias is applied here. */
  readonly match?: { readonly offset: number; readonly length: number };
}

/** Emits the 255-terminated extension bytes for a length whose nibble saturated at 15. */
function extensionBytes(value: number): number[] {
  const bytes: number[] = [];
  let remaining = value - 15;
  while (remaining >= 255) {
    bytes.push(255);
    remaining -= 255;
  }
  bytes.push(remaining);
  return bytes;
}

/** Assembles a raw LZ4 block from a sequence description (test-only helper). */
function buildBlock(sequences: readonly Sequence[]): Uint8Array {
  const bytes: number[] = [];
  for (const sequence of sequences) {
    const literalLen = sequence.literals.length;
    const matchToken = sequence.match === undefined ? 0 : sequence.match.length - 4;
    bytes.push((Math.min(literalLen, 15) << 4) | Math.min(matchToken, 15));
    if (literalLen >= 15) {
      bytes.push(...extensionBytes(literalLen));
    }
    bytes.push(...sequence.literals);
    if (sequence.match !== undefined) {
      bytes.push(sequence.match.offset & 0xff, (sequence.match.offset >>> 8) & 0xff);
      if (matchToken >= 15) {
        bytes.push(...extensionBytes(matchToken));
      }
    }
  }
  return new Uint8Array(bytes);
}

/** Decompresses into an exactly-sized buffer and returns it as a plain array. */
function expand(block: Uint8Array, outLen: number): number[] {
  const out = new Uint8ClampedArray(outLen);
  decompressLz4Block(block, out);
  return Array.from(out);
}

describe('decompressLz4Block', () => {
  it('copies a literals-only block verbatim', () => {
    const block = buildBlock([{ literals: [10, 20, 30, 40, 50] }]);
    expect(expand(block, 5)).toEqual([10, 20, 30, 40, 50]);
  });

  it('expands a short match, then trailing literals', () => {
    const block = buildBlock([
      { literals: [1, 2, 3, 4], match: { offset: 4, length: 4 } },
      { literals: [9] },
    ]);
    expect(expand(block, 9)).toEqual([1, 2, 3, 4, 1, 2, 3, 4, 9]);
  });

  it('expands an overlapping match (offset 1 — the RLE case)', () => {
    // The match reads bytes this very copy is writing, so it can only be done one byte at a time.
    const block = buildBlock([{ literals: [7], match: { offset: 1, length: 6 } }]);
    expect(expand(block, 7)).toEqual([7, 7, 7, 7, 7, 7, 7]);
  });

  it('expands a partially overlapping match (offset 2)', () => {
    const block = buildBlock([{ literals: [3, 4], match: { offset: 2, length: 5 } }]);
    expect(expand(block, 7)).toEqual([3, 4, 3, 4, 3, 4, 3]);
  });

  it('reads a literal length extension (nibble 15 + one byte)', () => {
    const literals = Array.from({ length: 20 }, (_, i) => i + 1);
    const block = buildBlock([{ literals }]);
    expect(expand(block, 20)).toEqual(literals);
  });

  it('reads a literal length extension spanning a 255 byte', () => {
    const literals = Array.from({ length: 300 }, (_, i) => i & 0xff);
    const block = buildBlock([{ literals }]);
    // 300 = 15 + 255 + 30, so the extension is two bytes and the first one must not terminate it.
    expect(block[1]).toBe(255);
    expect(block[2]).toBe(30);
    expect(expand(block, 300)).toEqual(literals);
  });

  it('reads a match length extension', () => {
    const block = buildBlock([{ literals: [1, 2, 3, 4], match: { offset: 4, length: 25 } }]);
    const expected = [1, 2, 3, 4];
    for (let i = 0; i < 25; i++) {
      expected.push(expected[i] as number);
    }
    expect(expand(block, 29)).toEqual(expected);
  });

  it('rejects a match offset of 0', () => {
    const block = new Uint8Array([0x10, 0xaa, 0x00, 0x00]); // 1 literal, then offset 0
    expect(() => expand(block, 5)).toThrow(Lz4DecodeError);
    expect(() => expand(block, 5)).toThrow(/match offset 0/);
  });

  it('rejects a match offset reaching before the start of the output', () => {
    const block = buildBlock([{ literals: [1, 2], match: { offset: 5, length: 4 } }]);
    expect(() => expand(block, 6)).toThrow(/offset 5 reaches before the start/);
  });

  it('rejects input truncated inside a literal run', () => {
    const block = new Uint8Array([0x40, 1, 2]); // 4 literals declared, 2 present
    expect(() => expand(block, 4)).toThrow(/truncated LZ4 block: 4 literal bytes declared/);
  });

  it('rejects input truncated inside a length extension', () => {
    const block = new Uint8Array([0xf0, 255]); // nibble 15, an extension byte that never terminates
    expect(() => expand(block, 300)).toThrow(/literal length extension runs past the input/);
  });

  it('rejects input truncated inside a match offset', () => {
    const block = new Uint8Array([0x10, 0xaa, 0x03]); // 1 literal, then half an offset
    expect(() => expand(block, 5)).toThrow(/ends inside a match offset/);
  });

  it('rejects a block whose literals overrun the output buffer', () => {
    const block = buildBlock([{ literals: [1, 2, 3, 4] }]);
    expect(() => expand(block, 2)).toThrow(/overruns the output buffer: 4 literal bytes/);
  });

  it('rejects a block whose match overruns the output buffer', () => {
    const block = buildBlock([{ literals: [1, 2, 3, 4], match: { offset: 4, length: 8 } }]);
    expect(() => expand(block, 6)).toThrow(/overruns the output buffer: 8 match bytes/);
  });

  it('rejects a block that underruns the output buffer', () => {
    const block = buildBlock([{ literals: [1, 2, 3, 4] }]);
    expect(() => expand(block, 8)).toThrow(/underruns the output buffer: produced 4 bytes, expected 8/);
  });
});
