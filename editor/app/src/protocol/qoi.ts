// Hand-rolled QOI (Quite OK Image) decoder — see https://qoiformat.org/qoi-specification.pdf.
//
// No npm dependency: the frame stream sends QOI-encoded pixels over the wire
// (engine-architecture.md §6, "Frames are binary WS messages"), and decoding a handful of KB per
// frame is cheap enough in plain TS that pulling in a WASM/native dependency isn't worth it.
//
// The decoder always produces 4-channel RGBA output regardless of the header's declared channel
// count, ready to hand straight to `new ImageData(rgba, width, height)`.

export interface QoiImage {
  readonly width: number;
  readonly height: number;
  readonly rgba: Uint8ClampedArray;
}

export class QoiDecodeError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'QoiDecodeError';
  }
}

const QOI_MAGIC = 0x71_6f_69_66; // "qoif"
const QOI_HEADER_SIZE = 14;
const QOI_PADDING = 8; // 7 zero bytes + a trailing 0x01 end marker

const QOI_OP_INDEX = 0x00; // 00xxxxxx
const QOI_OP_DIFF = 0x40; // 01xxxxxx
const QOI_OP_LUMA = 0x80; // 10xxxxxx
const QOI_OP_RUN = 0xc0; // 11xxxxxx (excluding the two reserved full-byte tags below)
const QOI_OP_RGB = 0xfe;
const QOI_OP_RGBA = 0xff;
const QOI_MASK_2 = 0xc0;

function hashIndex(r: number, g: number, b: number, a: number): number {
  return (r * 3 + g * 5 + b * 7 + a * 11) % 64;
}

/**
 * Decodes a full QOI byte stream (header through the end marker) into RGBA pixels.
 *
 * `out`, if given, is decoded into directly instead of allocating a fresh buffer — the caller's
 * escape hatch for the per-frame hot path (e.g. a reused `ImageData.data`). It must be exactly
 * `width*height*4` bytes once the header's dimensions are known, or decoding throws.
 */
export function decodeQoi(bytes: Uint8Array, out?: Uint8ClampedArray): QoiImage {
  if (bytes.length < QOI_HEADER_SIZE + QOI_PADDING) {
    throw new QoiDecodeError(
      `QOI buffer too short: ${bytes.length} bytes, need at least ${QOI_HEADER_SIZE + QOI_PADDING}`,
    );
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);

  const magic = view.getUint32(0, false);
  if (magic !== QOI_MAGIC) {
    throw new QoiDecodeError(`bad QOI magic: expected 0x${QOI_MAGIC.toString(16)}, got 0x${magic.toString(16)}`);
  }
  const width = view.getUint32(4, false);
  const height = view.getUint32(8, false);
  const channels = view.getUint8(12);
  // byte 13 is the colorspace tag; the decoder does not interpret it.
  if (channels !== 3 && channels !== 4) {
    throw new QoiDecodeError(`invalid QOI channel count: ${channels}`);
  }

  const numPixels = width * height;
  let rgba: Uint8ClampedArray;
  if (out !== undefined) {
    if (out.length !== numPixels * 4) {
      throw new QoiDecodeError(
        `output buffer size mismatch: ${width}x${height} needs ${numPixels * 4} bytes, got ${out.length}`,
      );
    }
    rgba = out;
  } else {
    rgba = new Uint8ClampedArray(numPixels * 4);
  }
  // The running array of 64 previously-seen pixels; starts at (0,0,0,0) in every slot.
  const seen = new Uint8ClampedArray(64 * 4);

  let r = 0;
  let g = 0;
  let b = 0;
  let a = 255;
  let pos = QOI_HEADER_SIZE;
  const chunksEnd = bytes.length - QOI_PADDING;
  let run = 0;

  for (let pixelIndex = 0; pixelIndex < numPixels; pixelIndex++) {
    if (run > 0) {
      run--;
    } else if (pos < chunksEnd) {
      const tag = bytes[pos++] as number;
      if (tag === QOI_OP_RGB) {
        r = bytes[pos++] as number;
        g = bytes[pos++] as number;
        b = bytes[pos++] as number;
      } else if (tag === QOI_OP_RGBA) {
        r = bytes[pos++] as number;
        g = bytes[pos++] as number;
        b = bytes[pos++] as number;
        a = bytes[pos++] as number;
      } else if ((tag & QOI_MASK_2) === QOI_OP_INDEX) {
        const idx = tag & 0x3f;
        r = seen[idx * 4] as number;
        g = seen[idx * 4 + 1] as number;
        b = seen[idx * 4 + 2] as number;
        a = seen[idx * 4 + 3] as number;
      } else if ((tag & QOI_MASK_2) === QOI_OP_DIFF) {
        const dr = ((tag >> 4) & 0x03) - 2;
        const dg = ((tag >> 2) & 0x03) - 2;
        const db = (tag & 0x03) - 2;
        r = (r + dr) & 0xff;
        g = (g + dg) & 0xff;
        b = (b + db) & 0xff;
      } else if ((tag & QOI_MASK_2) === QOI_OP_LUMA) {
        const secondByte = bytes[pos++] as number;
        const dg = (tag & 0x3f) - 32;
        const drdg = ((secondByte >> 4) & 0x0f) - 8;
        const dbdg = (secondByte & 0x0f) - 8;
        r = (r + dg + drdg) & 0xff;
        g = (g + dg) & 0xff;
        b = (b + dg + dbdg) & 0xff;
      } else if ((tag & QOI_MASK_2) === QOI_OP_RUN) {
        run = tag & 0x3f;
      } else {
        // Unreachable: the four mask branches above (00/01/10/11) plus the two full-byte
        // equality checks (0xFE/0xFF) partition every possible byte value.
        throw new QoiDecodeError(`unreachable QOI tag byte: 0x${tag.toString(16)}`);
      }
      const h = hashIndex(r, g, b, a) * 4;
      seen[h] = r;
      seen[h + 1] = g;
      seen[h + 2] = b;
      seen[h + 3] = a;
    } else {
      throw new QoiDecodeError(
        `unexpected end of QOI data stream at pixel ${pixelIndex} of ${numPixels}`,
      );
    }
    const o = pixelIndex * 4;
    rgba[o] = r;
    rgba[o + 1] = g;
    rgba[o + 2] = b;
    rgba[o + 3] = a;
  }

  return { width, height, rgba };
}
