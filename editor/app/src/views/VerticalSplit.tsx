// The centre column's vertical split: a resizable top pane over a bottom pane, with a maximize state
// that collapses the top to a slim restore strip. Introduced by the Game Designer view
// (viewport over code) and shared with the Voxel Artist view (viewport over slice painter) rather
// than written twice — the two differ only in what they put in the panes and what the strip says.
//
// `maximized` is CONTROLLED by the parent because the toggle button lives in the bottom pane's own
// header (the code panel's, the painter's), not here; this component only owns the drag share.

import { useCallback, useRef, useState, type PointerEvent, type ReactNode } from 'react';
import { DEFAULT_SHARE, shareAt } from './split';

interface VerticalSplitProps {
  readonly top: ReactNode;
  readonly bottom: ReactNode;
  /** True while the bottom pane is maximized: the top pane is replaced by the restore strip. */
  readonly maximized: boolean;
  readonly onRestore: () => void;
  /** What the restore strip says the hidden pane is. */
  readonly hiddenLabel: string;
  readonly restoreLabel: string;
  /** Extra classes for the container (the column's own layout class). */
  readonly className?: string;
}

export function VerticalSplit({
  top,
  bottom,
  maximized,
  onRestore,
  hiddenLabel,
  restoreLabel,
  className,
}: VerticalSplitProps) {
  const [share, setShare] = useState(DEFAULT_SHARE);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const dragging = useRef(false);

  const onHandleDown = useCallback((event: PointerEvent<HTMLDivElement>) => {
    dragging.current = true;
    event.currentTarget.setPointerCapture(event.pointerId);
  }, []);
  const onHandleMove = useCallback((event: PointerEvent<HTMLDivElement>) => {
    if (!dragging.current || containerRef.current === null) {
      return;
    }
    const rect = containerRef.current.getBoundingClientRect();
    setShare((current) => shareAt(event.clientY, rect.top, rect.height, current));
  }, []);
  const onHandleUp = useCallback((event: PointerEvent<HTMLDivElement>) => {
    dragging.current = false;
    try {
      event.currentTarget.releasePointerCapture(event.pointerId);
    } catch {
      // capture may already be released (pointercancel); nothing to do.
    }
  }, []);

  return (
    <div className={className === undefined ? 'gd-center' : `${className} gd-center`} ref={containerRef}>
      {maximized ? (
        <div className="gd-vp-strip">
          <span className="gd-strip-label">{hiddenLabel}</span>
          <button type="button" className="icon-btn" onClick={onRestore}>
            {restoreLabel}
          </button>
        </div>
      ) : (
        <>
          <div className="gd-vp-pane" style={{ flexGrow: share }}>
            {top}
          </div>
          <div
            className="gd-split-handle"
            role="separator"
            aria-orientation="horizontal"
            onPointerDown={onHandleDown}
            onPointerMove={onHandleMove}
            onPointerUp={onHandleUp}
            onPointerCancel={onHandleUp}
          />
        </>
      )}
      <div className="gd-code-pane" style={{ flexGrow: maximized ? 1 : 1 - share }}>
        {bottom}
      </div>
    </div>
  );
}
