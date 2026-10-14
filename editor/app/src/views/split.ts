// The centre column's vertical-split geometry (`views/VerticalSplit.tsx`) — the one rule the drag
// handle follows, kept pure so it is testable without a DOM.

/** Clamps for the top pane's share of the split. The bottom pane takes the rest, so both bounds are
 *  really "neither pane collapses to nothing". */
export const MIN_SHARE = 0.25;
export const MAX_SHARE = 0.8;

/** The default split: the viewport gets a little over half. */
export const DEFAULT_SHARE = 0.55;

/**
 * The top pane's share for a pointer at `clientY`, given the split container's client rect.
 *
 * A zero-height container (an unmounted or collapsed column) has no meaningful answer, so it keeps
 * `current` rather than snapping the panes to a clamp bound.
 */
export function shareAt(clientY: number, rectTop: number, rectHeight: number, current: number): number {
  if (rectHeight <= 0) {
    return current;
  }
  const raw = (clientY - rectTop) / rectHeight;
  return Math.min(MAX_SHARE, Math.max(MIN_SHARE, raw));
}
