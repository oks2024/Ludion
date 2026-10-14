// Rolling received-frames-per-second counter for the streaming viewport.
//
// The viewport records a timestamp every time a frame arrives; the status overlay wants an
// instantaneous "frames in the last rolling second" figure. Both halves are kept as pure functions
// so the counting logic is unit-testable without a clock, a socket, or a canvas: the caller owns the
// timestamp buffer (a plain array) and supplies `now`.

/** How wide the rolling window is, in milliseconds (one second). */
export const DEFAULT_WINDOW_MS = 1000;

/**
 * Drops timestamps older than `now - windowMs` from the front of `timestamps`, returning a new
 * array with only the ones still inside the window. Assumes `timestamps` is non-decreasing (frames
 * are recorded in arrival order), so the survivors are always a suffix.
 */
export function pruneWindow(
  timestamps: readonly number[],
  now: number,
  windowMs: number = DEFAULT_WINDOW_MS,
): number[] {
  const cutoff = now - windowMs;
  let start = 0;
  while (start < timestamps.length && (timestamps[start] as number) <= cutoff) {
    start++;
  }
  return timestamps.slice(start);
}

/**
 * The frames-per-second reading: how many of `timestamps` fall within the last `windowMs` ending at
 * `now`. Since the window is exactly one second by default, the in-window count *is* the fps; when a
 * shorter window is used the count is scaled to a per-second rate.
 */
export function rollingFps(
  timestamps: readonly number[],
  now: number,
  windowMs: number = DEFAULT_WINDOW_MS,
): number {
  const inWindow = pruneWindow(timestamps, now, windowMs).length;
  return (inWindow * DEFAULT_WINDOW_MS) / windowMs;
}
