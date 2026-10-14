// The messages the frame-decoder worker and its host exchange.
//
// WHY IT IS ITS OWN MODULE. `./frameWorker` installs a `message` listener at import time, so the host
// cannot import the worker's module to reach a shared type without running the worker's body on the
// main thread. Type-only declarations therefore live here, where both sides can name them and neither
// side's code is pulled into the other's bundle.
//
// EVERY PAYLOAD IS TRANSFERRED, never cloned: a per-frame structured clone of an encoded payload
// (and, on the buffer path, of ~3.6 MB of RGBA on the way back) would just relocate the cost this
// whole arrangement exists to remove. A transferred `ArrayBuffer` is neutered on the sender's side,
// so nothing here may be read again after it is posted.

import type { FrameFormat } from '../protocol/frames';

/** One frame handed to the worker: the header fields the decode needs, plus the bytes it owns. */
export interface FrameJob {
  readonly frameIndex: number;
  readonly width: number;
  readonly height: number;
  /** Which codec `payload` is in. The worker has no access to the socket, so the LFR1 header's
   *  format has to travel with the bytes. */
  readonly format: FrameFormat;
  /** Encoded payload bytes (`format` names the codec) — a copy of the socket message's payload view,
   *  owned outright by the job. */
  readonly payload: ArrayBuffer;
}

export type WorkerRequest =
  /** Hands the worker the canvas it will draw into. Sent once per worker; a worker that never
   *  receives one decodes into a buffer and posts the pixels back instead. */
  | { readonly kind: 'attach'; readonly canvas: OffscreenCanvas }
  | ({
      readonly kind: 'frame';
      /** A previously-returned pixel buffer the worker may decode into again, or `null`. Only the
       *  buffer path ever recycles: on the canvas path the worker keeps its own `ImageData`. */
      readonly recycle: ArrayBuffer | null;
    } & FrameJob);

export type WorkerReply =
  /** Whether the transferred canvas yielded a 2D context. A `false` here is unrecoverable — the
   *  element can never be reclaimed by the main thread — so the host reports it rather than hiding it. */
  | { readonly kind: 'attached'; readonly ok: boolean }
  | {
      readonly kind: 'decoded';
      readonly frameIndex: number;
      readonly width: number;
      readonly height: number;
      /** What this frame cost where the decode ran. */
      readonly decodeMs: number;
      /** RGBA bytes for the host to blit, or `null` when the worker owns the canvas and blitted. */
      readonly pixels: ArrayBuffer | null;
    }
  /** A frame that could not be decoded. The host must still treat it as the end of the in-flight
   *  decode, or the pump's busy flag would latch and the stream would stop forever. */
  | { readonly kind: 'failed'; readonly frameIndex: number; readonly message: string };
