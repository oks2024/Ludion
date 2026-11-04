// When an in-progress voxel edit reaches the disk.
//
// EVERY SAVE IS ALSO A RELOAD. The host watches `<content-root>/voxels/**` and turns each write into
// an `asset.reload` on the stage session — the voxel views send no reload of their own (contract 3),
// and there is no reload control in this UI. So the rate the disk sees is the rate the viewport
// re-meshes, and that rate is a decision this module owns rather than a side effect of how often a
// pointer moves.
//
// TWO POLICIES, because the two editors want different things:
//
//   - The SLICE PAINTER commits ON RELEASE and never mid-stroke. A cell edit replaces one character
//     with one character, so a whole drag applies against ONE doc (`edit.ts` says so explicitly) and
//     the stroke is a single write. Nothing about a half-painted stroke is worth watching in the
//     viewport, and the local grid already draws the stroke live from the working document.
//   - The PALETTE editor THROTTLES at `WRITE_THROTTLE_MS` during a drag and always commits on
//     release. Watching the model change *while* you drag the color is the entire point of this view
//     (palette calibration by re-rendering was the slow loop), so mid-drag writes are the feature,
//     not the cost.
//
// `WRITE_THROTTLE_MS` is the host watcher's own debounce: writing faster than the watcher coalesces
// buys no extra frames, only disk churn.
//
// Pure: no React, no Tauri, no DOM.

/** The floor between two disk writes of the same document, in milliseconds. */
export const WRITE_THROTTLE_MS = 150;

/** What to do with a write the UI just asked for. */
export type WriteAction =
  | { readonly kind: 'write' }
  /** Too soon: write again after this many milliseconds (a trailing write, latest content wins). */
  | { readonly kind: 'hold'; readonly afterMs: number };

const WRITE: WriteAction = { kind: 'write' };

/**
 * Whether a requested write may go now.
 *
 * `commit` (a pointer release, an undo, a single click) always writes: the file must end a gesture
 * holding exactly what the human left on screen, which is the same posture the camera throttle takes
 * (`stage.ts`'s `sendDue`, flushed unthrottled at the end of a gesture).
 *
 * `lastWriteMs` of `null` — nothing written yet — is always due. A `hold` is clamped to
 * `[0, intervalMs]` so no clock reading (a paused tab, a backwards `performance.now`) can stall a
 * write for longer than the interval itself.
 */
export function writeAction(
  lastWriteMs: number | null,
  now: number,
  commit: boolean,
  intervalMs: number = WRITE_THROTTLE_MS,
): WriteAction {
  if (commit || lastWriteMs === null) {
    return WRITE;
  }
  const elapsed = now - lastWriteMs;
  if (elapsed >= intervalMs) {
    return WRITE;
  }
  return { kind: 'hold', afterMs: Math.min(intervalMs, Math.max(0, intervalMs - elapsed)) };
}
