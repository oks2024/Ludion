// The frame decoder, off the main thread.
//
// Decoding walks the whole image — ~921,600 pixels at 1280x720 — and it used to run inside the
// WebSocket message event, where every millisecond it spent was a millisecond a keydown waited before
// it could reach `ws.send`. Here it costs the input leg nothing. That holds for the cheap formats
// too: `RAW_RGBA`'s "decode" is one copy, but it is a copy of megabytes.
//
// TWO MODES, chosen by whether the host ever sent an `attach`: with a transferred canvas the decode
// and the blit both happen here and only a timing comes back; without one the pixels are posted back
// for the host to blit. The second still removes the expensive half. See `./framePump`'s `RenderPath`.
//
// EVERY REPLY IS OWED. The host's pump treats a reply as "the decoder is free again", so a frame that
// throws must still answer — otherwise the busy flag latches and the stream stops for good.

import { decodeFramePayload } from '../protocol/decode';
import type { FrameJob, WorkerReply, WorkerRequest } from './frameWorkerProtocol';

// `lib.dom` is the only lib this package compiles against (`tsconfig.app.json`), so the worker global
// has no type here. Naming the two members used is more honest than pulling in a whole conflicting lib.
const scope = self as unknown as {
  postMessage(message: WorkerReply, transfer?: Transferable[]): void;
  addEventListener(type: 'message', listener: (event: MessageEvent<WorkerRequest>) => void): void;
};

/** The canvas the host transferred, or `null` in buffer mode. */
let canvas: OffscreenCanvas | null = null;
let ctx: OffscreenCanvasRenderingContext2D | null = null;
/** Reused across frames at steady state so decode writes straight into the blit's backing buffer;
 *  recreated when the frame's pixel dimensions change. Canvas mode only — in buffer mode the host
 *  owns the pixels and hands them back for reuse. */
let image: ImageData | null = null;

/** Decodes one job's payload into `out`, which is already sized `width*height*4`. The codec switch
 *  itself lives in `protocol/decode.ts`, shared with the main-thread sink. */
function decodeInto(job: FrameJob, out: Uint8ClampedArray): void {
  decodeFramePayload(job.format, new Uint8Array(job.payload), out);
}

function drawOnCanvas(job: Extract<WorkerRequest, { kind: 'frame' }>): void {
  const target = canvas as OffscreenCanvas;
  const context = ctx as OffscreenCanvasRenderingContext2D;
  if (image === null || image.width !== job.width || image.height !== job.height) {
    image = new ImageData(job.width, job.height);
  }
  decodeInto(job, image.data);
  // Resizing clears the surface, so it must precede the blit; the context survives it.
  if (target.width !== job.width || target.height !== job.height) {
    target.width = job.width;
    target.height = job.height;
  }
  context.putImageData(image, 0, 0);
}

function decodeToBuffer(job: Extract<WorkerRequest, { kind: 'frame' }>): ArrayBuffer {
  const bytes = job.width * job.height * 4;
  const out =
    job.recycle !== null && job.recycle.byteLength === bytes
      ? new Uint8ClampedArray(job.recycle)
      : new Uint8ClampedArray(bytes);
  decodeInto(job, out);
  return out.buffer;
}

scope.addEventListener('message', (event) => {
  const message = event.data;
  if (message.kind === 'attach') {
    canvas = message.canvas;
    // `desynchronized` lets the compositor skip a sync for a surface nothing reads back; `alpha:
    // false` matches the stream, which is opaque.
    ctx = canvas.getContext('2d', { alpha: false, desynchronized: true });
    scope.postMessage({ kind: 'attached', ok: ctx !== null });
    return;
  }

  const started = performance.now();
  try {
    if (ctx !== null) {
      drawOnCanvas(message);
      scope.postMessage({
        kind: 'decoded',
        frameIndex: message.frameIndex,
        width: message.width,
        height: message.height,
        decodeMs: performance.now() - started,
        pixels: null,
      });
      return;
    }
    const pixels = decodeToBuffer(message);
    scope.postMessage(
      {
        kind: 'decoded',
        frameIndex: message.frameIndex,
        width: message.width,
        height: message.height,
        decodeMs: performance.now() - started,
        pixels,
      },
      [pixels],
    );
  } catch (error) {
    scope.postMessage({ kind: 'failed', frameIndex: message.frameIndex, message: String(error) });
  }
});
