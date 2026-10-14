// Which extent this viewport asks the host to stream at, and how that choice survives a reload.
//
// WHY THE KNOB EXISTS. Every stage of the frame path costs in proportion to the pixel count — the GPU
// readback, the unpad copy, the server's per-format encode, the socket write, this document's payload
// copy, the decode, the blit. `--quality` lowers GPU work and touches none of that, so on a low-power
// machine the only lever that pays at every stage at once is fewer pixels. 960x540 is 44% of 720p's
// pixels; 854x480 is 35%.
//
// WHY A SHORT LIST AND NOT A SLIDER. Each distinct extent reallocates the host's readback ring (three
// staging buffers) and the frame graph's physical cache. A `<select>` of discrete values costs one
// reallocation per human decision; a slider would thrash GPU allocation on every drag frame.
//
// WHAT THE CHOICE ACTUALLY DOES. It sends `render.size` — a plain command, never a subscription
// parameter: the stream keeps running and simply starts arriving at the new size, which every resize
// path in `useFrameStream` already handles. The extent is SESSION state on the host, shared by every
// attached client (last writer wins), which is what keeps `render.bounds` — whose `render_size` is the
// pixel space picking maps through — true for everyone at once.
//
// Pure and storage-free: the stored string arrives as an argument, so the policy is testable without
// a browser, a socket or a `localStorage`.

/** localStorage key holding the chosen extent, as `WIDTHxHEIGHT` or {@link HOST_DEFAULT}. */
export const STREAM_SIZE_KEY = 'ludion.editor.streamSize';

/** An extent in engine pixels: what a stream frame is produced at, and the pixel space picking maps
 *  through (`render.bounds` reports the same pair). */
export type StreamSize = readonly [number, number];

/** The widest extent the engine accepts before answering `L3025` (8K's width). */
export const MAX_STREAM_WIDTH = 7680;

/** The tallest extent the engine accepts before answering `L3025` (8K's height). */
export const MAX_STREAM_HEIGHT = 4320;

/** The stored value naming "leave the host's own extent alone". Its whole point is that it sends
 *  NOTHING, so a `--render-size` / `LUDION_RENDER_SIZE` startup extent and this knob compose: the
 *  human only overrides the host when they pick a number. */
export const HOST_DEFAULT = 'default';

/**
 * The extents the viewport offers, in menu order. `null` is the host's own default (send nothing).
 *
 * 720p is listed explicitly even though it is usually also the host default: naming it is how a human
 * who has dropped to 480p gets back to the engine's shipped extent without knowing what the host was
 * started with.
 */
export const STREAM_SIZES: readonly (StreamSize | null)[] = [
  null,
  [1280, 720],
  [960, 540],
  [854, 480],
  [640, 360],
];

/** One axis clamped into the range the engine accepts, rounded to a whole pixel count. */
function clampAxis(pixels: number, max: number): number {
  return Math.min(Math.max(Math.round(pixels), 1), max);
}

/**
 * `size` clamped into the extent range the engine accepts (`1..=7680` x `1..=4320`, whole pixels).
 *
 * Clamped rather than rejected: a stored value that drifted out of range still names an intent, and
 * the nearest legal extent honours it — whereas letting it through would earn an `L3025` the human
 * cannot act on from a `<select>` they never typed into.
 */
export function clampStreamSize(size: StreamSize): StreamSize {
  return [clampAxis(size[0], MAX_STREAM_WIDTH), clampAxis(size[1], MAX_STREAM_HEIGHT)];
}

/** The string form persisted under {@link STREAM_SIZE_KEY}: `960x540`, or {@link HOST_DEFAULT}. */
export function serializeStreamSize(size: StreamSize | null): string {
  return size === null ? HOST_DEFAULT : `${size[0]}x${size[1]}`;
}

/**
 * The extent a stored string names, or `null` for the host's own default.
 *
 * Anything unparseable reads as the host default rather than being rejected: a typo in a developer
 * key must not stop the viewport from rendering. A well-formed but out-of-range extent is clamped
 * ({@link clampStreamSize}) instead, because it did name an intent.
 */
export function parseStreamSize(raw: string | null): StreamSize | null {
  if (raw === null) {
    return null;
  }
  const match = /^\s*(\d+)\s*x\s*(\d+)\s*$/.exec(raw);
  if (match === null) {
    return null;
  }
  const width = Number(match[1]);
  const height = Number(match[2]);
  if (width === 0 || height === 0) {
    return null;
  }
  return clampStreamSize([width, height]);
}

/** The menu label for one choice: the host default, or the extent as `960 x 540`. */
export function describeStreamSize(size: StreamSize | null): string {
  return size === null ? 'host default' : `${size[0]}×${size[1]}`;
}
