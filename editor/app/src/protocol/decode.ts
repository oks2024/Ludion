// One frame payload, whichever codec it arrived in — the only place the format tag becomes a decode.
//
// WHY IT IS ITS OWN MODULE. The viewer decodes in two places (the worker, `viewer/frameWorker.ts`,
// and the no-worker inline sink in `viewer/useFrameStream.ts`), and a switch written twice is a
// switch that can disagree with itself — the fourth format would be added to one arm and not the
// other. Both call this; the codec list has exactly one owner, and it is testable without a worker.
//
// Every arm writes into a caller-supplied `out` sized `width*height*4` from the LFR1 header
// (`./frames`), because that is the one dimension source all three formats can agree on: QOI repeats
// the dimensions in its own header, LZ4 and RAW carry none at all.

import type { FrameFormat } from './frames';
import { decompressLz4Block } from './lz4';
import { decodeQoi } from './qoi';

/** A `RAW_RGBA` payload that is not exactly the frame's pixel count. */
export class RawDecodeError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'RawDecodeError';
  }
}

/** Whether this build has a decoder for `format`. Total over the union today — it exists so the
 *  viewer's defensive guard and the switch below name the same set, and so a format added to the
 *  wire but not yet to this module is a still viewport rather than an exception per frame. */
export function canDecodeFormat(format: FrameFormat): boolean {
  return format === 'QOI' || format === 'LZ4_RGBA' || format === 'RAW_RGBA';
}

/**
 * Decodes one frame's payload into `out`, which must already be `width*height*4` bytes.
 *
 * Throws whatever the codec throws — `QoiDecodeError`, `Lz4DecodeError`, {@link RawDecodeError} —
 * on malformed input. Callers treat all three the same: a frame that cannot be decoded is a frame
 * that is not drawn, never a frame that is drawn wrong.
 */
export function decodeFramePayload(
  format: FrameFormat,
  payload: Uint8Array,
  out: Uint8ClampedArray,
): void {
  if (format === 'RAW_RGBA') {
    // No decode at all: the server sent the pixels. The length check is the whole of the format's
    // validation, and it is not optional — a short payload would leave the tail of `out` holding the
    // PREVIOUS frame (the buffers are recycled), which reads as a plausible image rather than as a
    // failure.
    if (payload.length !== out.length) {
      throw new RawDecodeError(
        `RAW_RGBA payload size mismatch: ${payload.length} bytes for a ${out.length}-byte frame`,
      );
    }
    out.set(payload);
    return;
  }
  if (format === 'LZ4_RGBA') {
    decompressLz4Block(payload, out);
    return;
  }
  decodeQoi(payload, out);
}
