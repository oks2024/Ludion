// Decoder for the `LFR1` binary frame header (engine-architecture.md §6, "Frame streaming").
//
// Layout (36 bytes, little-endian, followed by the payload):
//   0..4   magic "LFR1"
//   4      format: 0 = QOI, 1 = LZ4_RGBA, 2 = RAW_RGBA
//   5      flags: bit 0 = gi_stale
//   6..8   reserved
//   8..16  sub_id (u64)
//   16..24 frame_index (u64)
//   24..28 width (u32)
//   28..32 height (u32)
//   32..36 payload_len (u32)
//   36..   payload

export type FrameFormat = 'QOI' | 'LZ4_RGBA' | 'RAW_RGBA';

export interface LfrFrameHeader {
  readonly format: FrameFormat;
  /** The subscription this frame belongs to. Widened from the wire's u64 (see {@link u64ToNumber}). */
  readonly sub: number;
  /** The sim frame index this image was rendered from. */
  readonly frameIndex: number;
  readonly width: number;
  readonly height: number;
  readonly payloadLen: number;
  /**
   * Whether this image's global illumination was *held* — served from a voxelization or probe bake
   * built under inputs the frame no longer had, because the session's rebake lever is at `hold`
   * (`render.gi_rebake`). Travels with the image so a viewer learns it is looking at stale light
   * without asking a second question.
   */
  readonly giStale: boolean;
}

export interface DecodedFrame {
  readonly header: LfrFrameHeader;
  readonly payload: Uint8Array;
}

export class FrameDecodeError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'FrameDecodeError';
  }
}

const MAGIC = [0x4c, 0x46, 0x52, 0x31] as const; // "LFR1"
export const LFR1_HEADER_LEN = 36;

/** Byte 5, bit 0: the frame's global illumination was served from a held baseline rather than
 *  rebuilt for the world it depicts. Every other bit of the flags byte — and bytes 6..8 — stays
 *  reserved and is ignored, so a header written by a build that means something by them still
 *  decodes here. */
const FLAG_GI_STALE = 0b0000_0001;

/** Converts a wire u64 to a JS `number`, rejecting values outside the safe-integer range. */
function u64ToNumber(value: bigint, field: string): number {
  if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
    throw new FrameDecodeError(`LFR1 ${field} exceeds Number.MAX_SAFE_INTEGER: ${value}`);
  }
  return Number(value);
}

/** Parses an `LFR1` binary WS message into its header and payload (a view, not a copy). */
export function decodeFrameHeader(buffer: ArrayBuffer): DecodedFrame {
  if (buffer.byteLength < LFR1_HEADER_LEN) {
    throw new FrameDecodeError(
      `LFR1 frame truncated: buffer is ${buffer.byteLength} bytes, header needs ${LFR1_HEADER_LEN}`,
    );
  }
  const view = new DataView(buffer);

  for (let i = 0; i < MAGIC.length; i++) {
    const byte = view.getUint8(i);
    if (byte !== MAGIC[i]) {
      throw new FrameDecodeError(
        `bad LFR1 magic at byte ${i}: expected 0x${(MAGIC[i] as number).toString(16)}, got 0x${byte.toString(16)}`,
      );
    }
  }

  const formatByte = view.getUint8(4);
  let format: FrameFormat;
  if (formatByte === 0) {
    format = 'QOI';
  } else if (formatByte === 1) {
    format = 'LZ4_RGBA';
  } else if (formatByte === 2) {
    format = 'RAW_RGBA';
  } else {
    throw new FrameDecodeError(`unknown LFR1 frame format byte: ${formatByte}`);
  }
  const giStale = (view.getUint8(5) & FLAG_GI_STALE) !== 0;
  // bytes 6..8 are reserved and intentionally ignored.

  const sub = u64ToNumber(view.getBigUint64(8, true), 'sub_id');
  const frameIndex = u64ToNumber(view.getBigUint64(16, true), 'frame_index');
  const width = view.getUint32(24, true);
  const height = view.getUint32(28, true);
  const payloadLen = view.getUint32(32, true);

  const totalLen = LFR1_HEADER_LEN + payloadLen;
  if (buffer.byteLength < totalLen) {
    throw new FrameDecodeError(
      `LFR1 frame truncated: header declares a ${payloadLen}-byte payload but only ` +
        `${buffer.byteLength - LFR1_HEADER_LEN} bytes follow the header`,
    );
  }

  const payload = new Uint8Array(buffer, LFR1_HEADER_LEN, payloadLen);
  return {
    header: { format, sub, frameIndex, width, height, payloadLen, giStale },
    payload,
  };
}
