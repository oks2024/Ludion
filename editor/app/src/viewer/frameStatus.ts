// The live frame readout as an external store, so the status bar's wobble costs one segment rather
// than the whole app.
//
// WHY IT IS NOT REACT STATE. `useFrameStream` recomputes its readout four times a second, and the
// one app-wide consumer is the status bar's frame/fps pair. Held in `App`'s `useState`, that tick
// re-rendered every mounted view — the Voxel Artist's 14k-span slice painter included — for a number
// two spans wide. An external store inverts it: the surfaces that produce a readout write here, and
// whoever displays it subscribes through `useSyncExternalStore`. A subscriber whose snapshot is a
// primitive (the fps number) then re-renders only when that number actually moves.
//
// Pure: no React, no Tauri, no DOM.

import { NO_LAG, type LagSnapshot } from './lagProbe';

/** The live viewport readout the status overlay renders. */
export interface FrameStatus {
  /** Whether at least one frame has arrived since the subscription opened. */
  readonly receiving: boolean;
  /** The most recent frame's pixel dimensions, or `null` before the first frame. */
  readonly width: number | null;
  readonly height: number | null;
  /** The sim frame index of the most recent frame. */
  readonly frameIndex: number | null;
  /** Received frames in the last rolling second. */
  readonly fps: number;
  /** Whether the newest frame's header said its global illumination was held rather than rebuilt for
   *  the world it depicts (`render.gi_rebake` at `hold`). Read off the header rather than off a
   *  decoded image: it is a statement about the world, and it is true of a frame the pump shed. */
  readonly giStale: boolean;
  /** The latency instrument's reading, or {@link NO_LAG} on a surface that runs no probe. Nested
   *  rather than spread flat so the store's one app-wide consumer — the status bar, which subscribes
   *  to `fps` alone — keeps the same re-render surface it had. */
  readonly lag: LagSnapshot;
}

/** No stream, or a stream that has not delivered yet. Also what a disconnect resets to: leaving the
 *  last frame's dimensions and fps standing would report a stream that no longer exists. */
export const NO_FRAMES: FrameStatus = {
  receiving: false,
  width: null,
  height: null,
  frameIndex: null,
  fps: 0,
  giStale: false,
  lag: NO_LAG,
};

/**
 * Whether two readouts say the same thing.
 *
 * The 4 Hz tick that produces these builds a fresh object every time, and a fresh object re-renders
 * every consuming surface whether or not a number moved — including while nothing is streaming at
 * all, which is the case where the readout is stable for minutes. Every field is compared because
 * every field is displayed somewhere; `frameIndex` alone changes on each real frame, so this skips
 * renders precisely when there is nothing to show.
 */
export function sameFrameStatus(a: FrameStatus, b: FrameStatus): boolean {
  return (
    a.receiving === b.receiving &&
    a.width === b.width &&
    a.height === b.height &&
    a.frameIndex === b.frameIndex &&
    a.fps === b.fps &&
    a.giStale === b.giStale &&
    a.lag.lagMs === b.lag.lagMs &&
    a.lag.queuedMs === b.lag.queuedMs &&
    a.lag.decodeMs === b.lag.decodeMs &&
    a.lag.duty === b.lag.duty &&
    a.lag.rejectedTimeStamps === b.lag.rejectedTimeStamps
  );
}

let current: FrameStatus = NO_FRAMES;
const listeners = new Set<() => void>();

/** The current readout. Stable between writes, which is what `useSyncExternalStore` requires. */
export function getFrameStatus(): FrameStatus {
  return current;
}

export function setFrameStatus(next: FrameStatus): void {
  current = next;
  // Copied so a listener unsubscribing (or subscribing) during the notification cannot skip another.
  for (const listener of [...listeners]) {
    listener();
  }
}

export function subscribeFrameStatus(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
