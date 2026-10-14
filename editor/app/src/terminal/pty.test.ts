import { describe, expect, it } from 'vitest';
import { decodeOutput } from './pty';

describe('decodeOutput', () => {
  it('reconstructs the exact bytes from a base64 chunk', () => {
    // "hi" plus a CSI escape and a high byte (0xE2) — a lossy string decode would corrupt the last.
    const bytes = new Uint8Array([0x68, 0x69, 0x1b, 0x5b, 0x36, 0x6e, 0xe2]);
    const b64 = btoa(String.fromCharCode(...bytes));
    expect(Array.from(decodeOutput(b64))).toEqual(Array.from(bytes));
  });

  it('decodes an empty chunk to an empty array', () => {
    expect(decodeOutput('')).toEqual(new Uint8Array([]));
  });
});
