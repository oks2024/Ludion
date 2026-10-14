// Subscribes to the engine's `frames` channel and paints each frame onto a canvas.
//
// The subscription is `latest_wins` (the viewer default from engine-architecture.md §6): under load
// the engine drops unsent frames rather than build latency. That promise only survives to the screen
// if the client sheds too, and it does — decode runs in a worker (`./frameWorker`) and a frame that
// arrives while the worker is busy REPLACES the one waiting instead of queueing behind it
// (`./framePump`). At most one frame is ever parked, so the viewport shows the newest image the
// decoder could reach rather than replaying a growing backlog.
//
// Frames arrive as `LFR1` binary messages carrying encoded pixels in one of three codecs, named by
// the header and chosen at subscribe time from where the server is (`./frameFormat`). Whichever
// arrives, the decode is the same shape of work and runs in the same place. `protocol/client.ts` calls its frame listeners inline on the WebSocket message event, so
// everything this module does on the main thread is time a keydown waits before it can reach `ws.send`
// (see `./lagProbe`): the handler here is a header check, a payload copy and a `postMessage`. The
// canvas keeps its intrinsic size equal to the frame's pixel dimensions (crisp, 1:1) and CSS scales it
// to fit its container while preserving aspect.
//
// Everything here is a side effect keyed on the live client and the fps cap: a new subscription is
// opened on connect (or when the cap changes) and cleanly torn down on disconnect/unmount, so a
// dropped connection never leaves a dangling subscription or a stale frame handler.

import { useEffect, useRef, useState, type RefObject } from 'react';
import type { ProtocolClient } from '../protocol/client';
import { openSubscription } from '../protocol/subscription';
import { canDecodeFormat, decodeFramePayload } from '../protocol/decode';
import type { DecodedFrame, FrameFormat, LfrFrameHeader } from '../protocol/frames';
import { chooseFrameFormat, describeFormatChoice, FORMAT_KEY } from './frameFormat';
import { rollingFps } from './fps';
import { createFramePump, chooseRenderPath, type FrameJob, type FramePathCaps, type RenderPath } from './framePump';
import type { WorkerReply, WorkerRequest } from './frameWorkerProtocol';
import { NO_FRAMES, sameFrameStatus, type FrameStatus } from './frameStatus';
import { NO_LAG, type LagProbe } from './lagProbe';

// The readout's shape and its empty value live in the store module, which is also where an app-wide
// consumer subscribes; this hook's own return is the LOCAL copy each frame surface draws its overlay
// from.
export type { FrameStatus };

/** How often the status overlay recomputes (4 Hz) — decoupled from the frame rate so a fast stream
 *  doesn't thrash React state. */
const STATUS_INTERVAL_MS = 250;

/** What a sink reports back. Two calls rather than one because the numbers answer different
 *  questions: `onFrame` is about the image (what it depicts, what it cost to decode), `onMainWork`
 *  is about the input leg (how much of this thread the frame path took). */
interface SinkListener {
  onMainWork(atMs: number, costMs: number): void;
  onFrame(frameIndex: number, width: number, height: number, atMs: number, decodeMs: number): void;
}

/** One canvas's frame pipeline, whichever path it runs on. */
interface FrameSink {
  readonly canvas: HTMLCanvasElement;
  /** Offers an arriving frame. May shed it — see `./framePump`. */
  push(frame: DecodedFrame, listener: SinkListener): void;
  /** Stops reporting and drops anything parked. The sink stays usable: an fps-cap change re-attaches
   *  to the same canvas, and on the offscreen path there is no way to build a second one. */
  detach(): void;
  dispose(): void;
}

// --- Path selection --------------------------------------------------------------------------

let resolvedPath: RenderPath | null = null;

function detectCaps(): FramePathCaps {
  const offscreenCanvas = typeof OffscreenCanvas === 'function';
  let offscreen2d = false;
  if (offscreenCanvas) {
    try {
      offscreen2d = new OffscreenCanvas(1, 1).getContext('2d') !== null;
    } catch {
      offscreen2d = false;
    }
  }
  return {
    worker: typeof Worker === 'function',
    offscreenCanvas,
    transferControl:
      typeof HTMLCanvasElement === 'function' &&
      typeof HTMLCanvasElement.prototype.transferControlToOffscreen === 'function',
    offscreen2d,
  };
}

/** Opt-in switch for the canvas-in-worker path (see {@link renderPath}). */
const OFFSCREEN_KEY = 'ludion.editor.frameOffscreen';

function offscreenOptIn(): boolean {
  try {
    return window.localStorage.getItem(OFFSCREEN_KEY) === '1';
  } catch {
    return false;
  }
}

/** The URL whose codec choice was last announced, so a reconnect to the SAME server stays quiet and
 *  a connect to a different one (loopback vs remote — a different answer) says so. */
let announcedFor: string | null | undefined = undefined;

/** The codec to ask `client`'s server for. The policy and its measurements live in `./frameFormat`;
 *  this is the part that has to touch a real `localStorage` and a real console, kept out of there so
 *  the policy stays testable. Announced rather than silent: the three formats differ by more than a
 *  decode, and a reading taken without knowing which one ran is not a reading. */
function frameFormat(client: ProtocolClient): FrameFormat {
  const url = client.url;
  let override: string | null = null;
  try {
    override = window.localStorage.getItem(FORMAT_KEY);
  } catch {
    // A document with storage denied gets the URL's answer, like everyone else.
  }
  const choice = chooseFrameFormat(url, override);
  if (announcedFor !== url) {
    announcedFor = url;
    console.info(`[ludion] frame format: ${describeFormatChoice(choice, url)}`);
  }
  return choice.format;
}

/** The path every canvas in this document uses, decided once and announced once. Silent degradation
 *  is the failure mode worth avoiding here: the three paths differ by milliseconds per frame, and a
 *  reading taken without knowing which one ran is not a reading.
 *
 *  `offscreen` is capable but not the default. Both worker paths take the decode — 16 ms a frame for
 *  QOI, and a multi-megabyte copy even for RAW — off this thread; `offscreen` additionally
 *  takes the blit, worth a further ~1-3 ms per frame. It buys that by transferring the canvas, which
 *  leaves the element a placeholder whose bitmap the worker owns, while `Viewport` reads `canvas.width`
 *  for gizmo geometry and pointer picking. That trade cannot be judged outside a real browser, so the
 *  large and safe part of the win ships on by default and the last millisecond waits behind this
 *  switch. */
function renderPath(): RenderPath {
  if (resolvedPath === null) {
    const best = chooseRenderPath(detectCaps());
    resolvedPath = best === 'offscreen' && !offscreenOptIn() ? 'worker-blit' : best;
    console.info(`[ludion] frame decode path: ${resolvedPath}`);
  }
  return resolvedPath;
}

// --- The sinks -------------------------------------------------------------------------------

/** A detached copy of the frame's payload, owned outright so it can be transferred. The socket
 *  message's own buffer is off limits: `protocol/client.ts` hands the same `DecodedFrame` to every
 *  listener, and transferring out from under a sibling surface would neuter its payload. */
function ownPayload(frame: DecodedFrame): ArrayBuffer {
  const buffer = new ArrayBuffer(frame.payload.byteLength);
  new Uint8Array(buffer).set(frame.payload);
  return buffer;
}

/**
 * The main-thread canvas element is a placeholder once `transferControlToOffscreen` has run: its
 * `width`/`height` IDL setters throw, and the element does not follow the worker's canvas on its own.
 * Everything on this side that needs the frame's pixel size reads those attributes — the picker's
 * client→engine mapping, the gizmo overlay's `viewBox`, the 1:1 CSS fit — so they are mirrored
 * through `setAttribute`, which is not the throwing path. The first frame's size is set before the
 * transfer, so this only ever runs on a mid-stream resolution change.
 */
function mirrorPlaceholderSize(canvas: HTMLCanvasElement, width: number, height: number): void {
  try {
    canvas.setAttribute('width', String(width));
    canvas.setAttribute('height', String(height));
  } catch {
    // A browser that refuses leaves the previous size standing; the frame itself still renders.
  }
}

/** Decode and blit on the message event — the no-worker floor, and what the fix is measured against. */
function createInlineSink(canvas: HTMLCanvasElement): FrameSink {
  const ctx = canvas.getContext('2d', { alpha: false, desynchronized: true });
  // Reused across frames at steady state so decode writes straight into the blit's backing buffer
  // instead of allocating + copying megabytes every frame; recreated on resize.
  let image: ImageData | null = null;
  let listener: SinkListener | null = null;

  return {
    canvas,
    push(frame: DecodedFrame, next: SinkListener): void {
      listener = next;
      if (ctx === null) {
        return;
      }
      const { width, height, frameIndex } = frame.header;
      const started = performance.now();
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }
      if (image === null || image.width !== width || image.height !== height) {
        image = new ImageData(width, height);
      }
      decodeFramePayload(frame.header.format, frame.payload, image.data);
      ctx.putImageData(image, 0, 0);
      const done = performance.now();
      listener.onMainWork(done, done - started);
      listener.onFrame(frameIndex, width, height, done, done - started);
    },
    detach(): void {
      listener = null;
    },
    dispose(): void {
      listener = null;
    },
  };
}

/**
 * Decode in a worker. With `offscreen` the canvas goes to the worker and the blit goes with it; with
 * `worker-blit` the pixels come back and this thread blits them, which still takes the expensive half
 * off the message event.
 *
 * Returns `null` when the worker cannot be constructed, so the caller can fall back to inline.
 */
function createWorkerSink(canvas: HTMLCanvasElement, path: 'offscreen' | 'worker-blit', first: LfrFrameHeader): FrameSink | null {
  let worker: Worker;
  try {
    // `new URL(..., import.meta.url)` is the form Vite recognizes and bundles; anything else ships a
    // path that only resolves in dev.
    worker = new Worker(new URL('./frameWorker.ts', import.meta.url), { type: 'module' });
  } catch (error) {
    console.error('[ludion] frame decode worker failed to start; decoding on the main thread', error);
    return null;
  }

  const pump = createFramePump();
  let listener: SinkListener | null = null;
  let ctx: CanvasRenderingContext2D | null = null;
  // The pixel buffer the worker last returned, handed back with the next job so the buffer path
  // reuses one allocation instead of producing megabytes of garbage per frame. Buffer path only.
  let recycle: ArrayBuffer | null = null;
  let width = first.width;
  let height = first.height;

  if (path === 'offscreen') {
    // Sized BEFORE the transfer, while the IDL setters still work — after it they throw, and the
    // element's own attributes are what the picker and the overlay read.
    canvas.width = width;
    canvas.height = height;
    try {
      const offscreen = canvas.transferControlToOffscreen();
      worker.postMessage({ kind: 'attach', canvas: offscreen } satisfies WorkerRequest, [offscreen]);
    } catch (error) {
      // One transfer per element is an invariant of the registry below, so reaching here means the
      // browser refused outright. The element is unusable either way; say so rather than go black.
      console.error('[ludion] canvas could not be transferred to the frame worker', error);
      worker.terminate();
      return null;
    }
  } else {
    ctx = canvas.getContext('2d', { alpha: false, desynchronized: true });
  }

  const postJob = (job: FrameJob): void => {
    const transfer: Transferable[] = [job.payload];
    const reuse = recycle;
    recycle = null;
    if (reuse !== null) {
      transfer.push(reuse);
    }
    worker.postMessage({ kind: 'frame', recycle: reuse, ...job } satisfies WorkerRequest, transfer);
  };

  const blit = (reply: Extract<WorkerReply, { kind: 'decoded' }>): void => {
    const pixels = reply.pixels;
    if (pixels === null) {
      return;
    }
    // Kept for reuse whether or not it was drawn — the allocation is the point, not the image.
    recycle = pixels;
    if (listener === null || ctx === null || pixels.byteLength !== reply.width * reply.height * 4) {
      return;
    }
    if (canvas.width !== reply.width || canvas.height !== reply.height) {
      canvas.width = reply.width;
      canvas.height = reply.height;
    }
    // A fresh `ImageData` per frame, but over the RECYCLED buffer: constructing the view is free,
    // and the buffer is the part worth reusing. It cannot be held across frames because posting it
    // back to the worker neuters it.
    ctx.putImageData(new ImageData(new Uint8ClampedArray(pixels), reply.width, reply.height), 0, 0);
  };

  worker.addEventListener('message', (event: MessageEvent<WorkerReply>) => {
    const reply = event.data;
    if (reply.kind === 'attached') {
      if (!reply.ok) {
        console.error('[ludion] frame worker could not open a 2D context on the transferred canvas');
      }
      return;
    }
    if (reply.kind === 'failed') {
      console.error(`[ludion] frame ${reply.frameIndex} failed to decode: ${reply.message}`);
    } else {
      const started = performance.now();
      blit(reply);
      const done = performance.now();
      listener?.onMainWork(done, done - started);
      listener?.onFrame(reply.frameIndex, reply.width, reply.height, done, reply.decodeMs);
    }
    // Owed for every reply, success or failure: the pump's busy flag is what sheds, and a flag that
    // never clears stops the stream for good.
    const next = pump.complete();
    if (next !== null) {
      postJob(next);
    }
  });

  return {
    canvas,
    push(frame: DecodedFrame, next: SinkListener): void {
      listener = next;
      const started = performance.now();
      if (frame.header.width !== width || frame.header.height !== height) {
        width = frame.header.width;
        height = frame.header.height;
        if (path === 'offscreen') {
          mirrorPlaceholderSize(canvas, width, height);
        }
      }
      // Sized here rather than at the blit so the element reports the frame's pixel size from the
      // moment the frame arrives — the picker and the gizmo overlay read it, and they poll on their
      // own clock, not on the decoder's.
      if (path === 'worker-blit' && (canvas.width !== width || canvas.height !== height)) {
        canvas.width = width;
        canvas.height = height;
      }
      const job: FrameJob = {
        frameIndex: frame.header.frameIndex,
        width: frame.header.width,
        height: frame.header.height,
        // The header's format, not the subscribed one: a frame from a subscription that has already
        // been replaced could still be in flight in the other codec.
        format: frame.header.format,
        payload: ownPayload(frame),
      };
      const post = pump.offer(job);
      if (post !== null) {
        postJob(post);
      }
      const done = performance.now();
      // Reported even when the frame was shed: the copy and the handoff happened on this thread
      // either way, and `duty` is about occupancy, not about images.
      next.onMainWork(done, done - started);
    },
    detach(): void {
      listener = null;
      pump.discard();
    },
    dispose(): void {
      listener = null;
      pump.discard();
      worker.terminate();
    },
  };
}

// --- The per-canvas registry -----------------------------------------------------------------

// ONE SINK PER CANVAS ELEMENT, and the reason is `transferControlToOffscreen`: it is one-way and
// permanent, so a second transfer of the same element throws and leaves a canvas nothing can ever
// draw on. The hook's effect re-runs whenever the client, the fps cap or the probe changes while the
// element persists across those runs, and StrictMode re-runs it once more in development — so the
// sink's lifetime is keyed on the ELEMENT, never on the effect. An effect teardown only detaches.
//
// One worker per canvas rather than a shared pool: the three frame surfaces are rarely mounted at
// once, a pool would need its own scheduling, and the offscreen path pins a canvas to a worker anyway.
const sinks = new Map<HTMLCanvasElement, FrameSink>();

/** Disposes sinks whose canvas has left the document — the only signal that a surface is really gone
 *  rather than mid-teardown. Deliberately not driven by effect cleanup, which fires on every
 *  re-subscription and twice per mount under StrictMode, both times with the element still in place. */
function sweepDetachedSinks(): void {
  for (const [canvas, sink] of sinks) {
    if (!canvas.isConnected) {
      sink.dispose();
      sinks.delete(canvas);
    }
  }
}

function acquireSink(canvas: HTMLCanvasElement, first: LfrFrameHeader): FrameSink {
  const existing = sinks.get(canvas);
  if (existing !== undefined) {
    return existing;
  }
  sweepDetachedSinks();
  const path = renderPath();
  const sink =
    (path === 'inline' ? null : createWorkerSink(canvas, path, first)) ?? createInlineSink(canvas);
  sinks.set(canvas, sink);
  return sink;
}

/**
 * Streams frames from `client` onto the canvas in `canvasRef`, returning a {@link FrameStatus} for
 * the overlay. `maxFps` caps delivery (`null` = uncapped); changing it reopens the subscription.
 *
 * `probe` is the optional latency instrument (`./lagProbe`): a surface that also injects input holds
 * one and passes it here so the blit side of the measurement lands on the same clock as the keydown
 * side. A surface with no input passes nothing and the reading stays {@link NO_LAG}. It must be
 * stable across renders (a `useRef`) — it is a subscription dependency.
 */
export function useFrameStream(
  client: ProtocolClient | null,
  canvasRef: RefObject<HTMLCanvasElement | null>,
  maxFps: number | null,
  probe?: LagProbe | null,
): FrameStatus {
  const [status, setStatus] = useState<FrameStatus>(NO_FRAMES);
  // Frame arrival timestamps (ms), pruned to the rolling window by the status tick.
  const timestampsRef = useRef<number[]>([]);
  const latestRef = useRef<{ width: number; height: number; frameIndex: number } | null>(null);
  const receivingRef = useRef(false);
  // Taken from the HEADER, not from a decoded image: the flag is a statement about the world's
  // global illumination, so it is true of a frame the pump sheds as much as of one it paints, and
  // the worker's reply does not carry it.
  const giStaleRef = useRef(false);

  useEffect(() => {
    // Reset before the guard, not after it: losing the client is a real transition (a surface toggled
    // off, a session dropped), and leaving the last frame's dimensions and fps standing would report a
    // stream that no longer exists to whoever renders the status.
    timestampsRef.current = [];
    latestRef.current = null;
    receivingRef.current = false;
    giStaleRef.current = false;
    setStatus(NO_FRAMES);
    if (client === null) {
      return;
    }

    let sink: FrameSink | null = null;

    const listener: SinkListener = {
      onMainWork(atMs: number, costMs: number): void {
        probe?.noteMainWork(atMs, costMs);
      },
      onFrame(frameIndex: number, width: number, height: number, atMs: number, decodeMs: number): void {
        probe?.noteFrame(frameIndex, atMs, decodeMs);
        timestampsRef.current.push(atMs);
        latestRef.current = { width, height, frameIndex };
        receivingRef.current = true;
      },
    };

    const drawFrame = (frame: DecodedFrame): void => {
      // Ignore frames belonging to a different subscription (a stale one still draining).
      if (!sub.accepts(frame.header.sub)) {
        return;
      }
      giStaleRef.current = frame.header.giStale;
      if (!canDecodeFormat(frame.header.format)) {
        // The defensive guard against a format this viewer has no decoder for: ignore it rather than
        // throw, so a server speaking a codec added after this build degrades to a still viewport
        // instead of an exception per frame.
        return;
      }
      const canvas = canvasRef.current;
      if (canvas === null) {
        return;
      }
      // Acquired here rather than in the effect body because the canvas may not exist yet when the
      // effect runs (a surface that renders a status card in its place) and the first frame's
      // dimensions are what the offscreen transfer needs to size the element with.
      sink = acquireSink(canvas, frame.header);
      sink.push(frame, listener);
    };

    const offFrame = client.onFrame(drawFrame);

    const mode =
      maxFps === null
        ? ({ mode: 'latest_wins' } as const)
        : ({ mode: 'latest_wins', max_fps: maxFps } as const);
    // A failed subscribe (e.g. the socket dropped mid-handshake) surfaces via the connection state;
    // nothing actionable to draw here.
    const sub = openSubscription(client, 'frames', { mode, format: frameFormat(client) });

    const statusTimer = window.setInterval(() => {
      const now = performance.now();
      timestampsRef.current = timestampsRef.current.filter((t) => t > now - 1000);
      const latest = latestRef.current;
      const next: FrameStatus = {
        receiving: receivingRef.current,
        width: latest?.width ?? null,
        height: latest?.height ?? null,
        frameIndex: latest?.frameIndex ?? null,
        fps: rollingFps(timestampsRef.current, now),
        giStale: giStaleRef.current,
        lag: probe?.snapshot(now) ?? NO_LAG,
      };
      // A fresh object every tick re-renders every consuming surface unconditionally, including while
      // nothing streams at all. Only a reading that actually moved is worth a render.
      setStatus((prev) => (sameFrameStatus(prev, next) ? prev : next));
    }, STATUS_INTERVAL_MS);

    return () => {
      offFrame();
      window.clearInterval(statusTimer);
      // Detach, never dispose: the sink belongs to the canvas element, which outlives this effect.
      sink?.detach();
      // The canvas has not left the document yet at teardown time, so the sweep waits for the task
      // after this one — by then a real unmount has removed it and a re-subscription has not.
      window.setTimeout(sweepDetachedSinks, 0);
      sub.close();
    };
  }, [client, canvasRef, maxFps, probe]);

  return status;
}
