// Which arriving frame gets decoded, and which is thrown away — the client's half of latest-wins.
//
// WHY IT EXISTS. The engine drops unsent frames rather than build latency (the `latest_wins` viewer
// default, applied at three stages server-side), but a client that decodes every frame it is handed
// gives all of that back: the browser's message-event queue is FIFO and unbounded, so once decode is
// slower than arrival every queued frame is still decoded and blitted, and the delay between "what
// the sim did" and "what is on screen" grows without limit.
//
// THE SHEDDING POINT IS THE DECODER'S BUSY FLAG, and deliberately not a timer. A frame that arrives
// while the decoder is idle is posted immediately — no scheduling hop, so an un-backlogged stream pays
// nothing for the mechanism. A frame that arrives while the decoder is busy REPLACES whatever was
// waiting, because the newer image is strictly the better answer to "what does the world look like
// now" and the older one has no remaining value. At most one frame is ever parked, so the client's
// backlog is bounded at two (one decoding, one waiting) no matter how far behind it falls.
//
// Pure: no DOM, no worker, no clock. The caller decides what a "job" is and does the posting; this
// module only answers post-it / park-it / drop-it, which is the part worth a test.

import type { FrameJob } from './frameWorkerProtocol';

export type { FrameJob };

/** Where a frame's pixels are produced and drawn. */
export type RenderPath =
  /** Worker decodes and blits into a canvas transferred to it: nothing on the main thread. */
  | 'offscreen'
  /** Worker decodes, main thread blits the returned pixels: the decode still leaves the input leg. */
  | 'worker-blit'
  /** Decode and blit inline on the message event — what a browser without workers gets. */
  | 'inline';

/** What the host environment actually offers, read once (see `useFrameStream`'s detection). */
export interface FramePathCaps {
  readonly worker: boolean;
  readonly offscreenCanvas: boolean;
  /** `HTMLCanvasElement.prototype.transferControlToOffscreen` is present. */
  readonly transferControl: boolean;
  /** A throwaway `OffscreenCanvas` yielded a 2D context. Probed rather than assumed because the
   *  transfer is one-way and permanent: a canvas handed to a worker that then cannot draw on it is
   *  a canvas nothing can ever draw on again. */
  readonly offscreen2d: boolean;
}

/** The best path the given capabilities support. Every fallback still removes the decode from the
 *  message event except `inline`, which is the no-worker floor. */
export function chooseRenderPath(caps: FramePathCaps): RenderPath {
  if (!caps.worker) {
    return 'inline';
  }
  if (caps.offscreenCanvas && caps.transferControl && caps.offscreen2d) {
    return 'offscreen';
  }
  return 'worker-blit';
}

/**
 * The latest-wins gate in front of one decoder.
 *
 * The caller's contract: post exactly the job each method hands back, and call {@link complete} once
 * per posted job — including for a decode that failed, or the busy flag latches and the stream stops.
 */
export interface FramePump {
  /** Offers an arriving frame. Returns the job to post now, or `null` when it was parked (the
   *  decoder is busy) — in which case any previously parked job has been dropped. */
  offer(job: FrameJob): FrameJob | null;
  /** Reports the in-flight decode as finished. Returns the parked job to post now, or `null`. */
  complete(): FrameJob | null;
  /** Drops the parked job without posting it (a subscription ended). Leaves the in-flight decode
   *  alone — it is already in the worker and its completion is still owed. */
  discard(): void;
  /** Whether a decode is in flight. */
  readonly busy: boolean;
  /** Frames replaced while waiting — the shed count, the thing latency is being traded for. A
   *  {@link discard} is teardown, not shedding, and is not counted. */
  readonly dropped: number;
}

/** Creates a {@link FramePump}. One per decoder; they hold no shared state. */
export function createFramePump(): FramePump {
  let busy = false;
  let parked: FrameJob | null = null;
  let dropped = 0;

  return {
    offer(job: FrameJob): FrameJob | null {
      if (busy) {
        if (parked !== null) {
          dropped++;
        }
        parked = job;
        return null;
      }
      busy = true;
      return job;
    },

    complete(): FrameJob | null {
      const next = parked;
      parked = null;
      // A completion with nothing waiting leaves the pump idle; a completion that arrives when the
      // pump is already idle (a reply outliving a `discard`) is harmless and stays idle.
      busy = next !== null;
      return next;
    },

    discard(): void {
      parked = null;
    },

    get busy(): boolean {
      return busy;
    },

    get dropped(): number {
      return dropped;
    },
  };
}
