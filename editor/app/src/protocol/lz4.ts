// Hand-rolled LZ4 *block* decompressor — see https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md.
//
// No npm dependency: the frame stream can send LZ4-compressed RGBA over the wire (the server's
// `LZ4_RGBA` format tag, `crates/ludion-server/src/delivery.rs`), and decompressing a block is a
// mechanical byte loop — a few dozen lines. The alternatives all cost more than they save: a WASM
// build has to load inside a module worker AND under vitest's `node` environment, and a JS package
// would be a runtime dependency for something the spec fits on one page.
//
// Decompression only. Nothing in the editor ever compresses.
//
// LZ4 is nobody's default any more: measured on real engine content it is both bigger on the wire and
// slower to encode than QOI, so `viewer/frameFormat.ts` reaches it only through the override. The
// decoder stays because the format stays on the wire, and a subscriber may still ask for it.
//
// This is the LZ4 *block* format, not the framed one: there is no magic, no checksum and no stored
// content size. The uncompressed size comes from the LFR1 header's width/height (`./frames`), which
// is why the entry point takes the output buffer rather than returning one — a size prepended to the
// payload would be a second, contradictable source of truth.

export class Lz4DecodeError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'Lz4DecodeError';
  }
}

/** Adds the 255-terminated length-extension bytes at `src[pos]` onto `base`. */
function readLengthExtension(
  src: Uint8Array,
  pos: number,
  base: number,
  what: string,
): { length: number; next: number } {
  let length = base;
  let next = pos;
  for (;;) {
    if (next >= src.length) {
      throw new Lz4DecodeError(`truncated LZ4 block: ${what} length extension runs past the input`);
    }
    const byte = src[next++];
    length += byte;
    if (byte !== 255) {
      return { length, next };
    }
  }
}

/**
 * Decompresses one raw LZ4 block into `out`, which must be exactly the frame's `width*height*4`
 * bytes (the LFR1 header is the dimension source; an LZ4 block is not self-describing).
 *
 * Throws {@link Lz4DecodeError} on malformed input or if the block does not fill `out` exactly.
 */
export function decompressLz4Block(src: Uint8Array, out: Uint8ClampedArray): void {
  const srcLen = src.length;
  const outLen = out.length;
  let sp = 0;
  let op = 0;

  while (sp < srcLen) {
    const token = src[sp++];

    let literalLen = token >>> 4;
    if (literalLen === 15) {
      const extended = readLengthExtension(src, sp, literalLen, 'literal');
      literalLen = extended.length;
      sp = extended.next;
    }
    if (literalLen > 0) {
      if (sp + literalLen > srcLen) {
        throw new Lz4DecodeError(
          `truncated LZ4 block: ${literalLen} literal bytes declared, ${srcLen - sp} remain`,
        );
      }
      if (op + literalLen > outLen) {
        throw new Lz4DecodeError(
          `LZ4 block overruns the output buffer: ${literalLen} literal bytes at offset ${op} of ${outLen}`,
        );
      }
      out.set(src.subarray(sp, sp + literalLen), op);
      sp += literalLen;
      op += literalLen;
    }

    // The last sequence of a block is literals only — it carries no match, and the input ends here.
    if (sp === srcLen) {
      break;
    }
    if (sp + 2 > srcLen) {
      throw new Lz4DecodeError('truncated LZ4 block: input ends inside a match offset');
    }
    const offset = src[sp] | (src[sp + 1] << 8);
    sp += 2;
    if (offset === 0) {
      throw new Lz4DecodeError('malformed LZ4 block: match offset 0');
    }
    if (offset > op) {
      throw new Lz4DecodeError(
        `malformed LZ4 block: match offset ${offset} reaches before the start of the output (${op} bytes written)`,
      );
    }

    let matchLen = token & 0x0f;
    if (matchLen === 15) {
      const extended = readLengthExtension(src, sp, matchLen, 'match');
      matchLen = extended.length;
      sp = extended.next;
    }
    matchLen += 4; // the minimum match length is not stored in the token
    if (op + matchLen > outLen) {
      throw new Lz4DecodeError(
        `LZ4 block overruns the output buffer: ${matchLen} match bytes at offset ${op} of ${outLen}`,
      );
    }
    // Byte at a time, deliberately: a match may overlap its own output (offset < matchLen is how LZ4
    // spells a run), so each byte must be able to read one this loop just wrote.
    let from = op - offset;
    for (let i = 0; i < matchLen; i++) {
      out[op++] = out[from++];
    }
  }

  if (op !== outLen) {
    throw new Lz4DecodeError(
      `LZ4 block underruns the output buffer: produced ${op} bytes, expected ${outLen}`,
    );
  }
}
