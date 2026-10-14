// The bottom status bar: live session readouts. Values that exist are wired (mode, dirty, fps + the
// frame interval derived from it, entity count from the hierarchy poll); the world conventions (Y-up,
// 1m grid) are static labels — the engine doesn't expose them as queryable data, so inventing plumbing
// for a constant would be dishonest.
//
// The frame pair is the one readout that does not arrive as a prop: it moves four times a second, so
// it is subscribed from `viewer/frameStatus` by the segment that shows it and nothing above re-renders
// for it.

import { useSyncExternalStore } from 'react';
import type { EditorMode } from '../App';
import { getFrameStatus, subscribeFrameStatus } from '../viewer/frameStatus';

interface StatusBarProps {
  readonly mode: EditorMode;
  readonly dirty: boolean;
  readonly entityCount: number;
  /** The human's placed 3D pointer, world coordinates. Absent when nothing is pointed. */
  readonly pointer?: readonly [number, number, number] | null;
  /** A view-owned line appended to the bar — the Voxel Artist's model path plus the engine's own
   *  readout verbatim (`statue.voxel.yaml · 9x9x9 · 289 filled · 1 component`). */
  readonly note?: string | null;
}

/** The frame pair, subscribed straight to the frame-status store rather than taken as a prop: it
 *  moves four times a second, and routing it through `App` state re-rendered every mounted view for
 *  it. The snapshot is the fps number alone, so a steady stream re-renders nothing at all. */
function FrameReadout() {
  const fps = useSyncExternalStore(subscribeFrameStatus, () => getFrameStatus().fps);
  // The delivered-frame interval, which is what this bar can honestly say: per-frame sim and GPU cost
  // does reach the wire now (the `profile` channel), but only while something is subscribed to it —
  // the Performance view is where those numbers live.
  const frameMs = fps > 0 ? (1000 / fps).toFixed(1) : '—';

  return (
    <>
      <span>
        frame <span className="num">{frameMs} ms</span>
      </span>
      <span className="sep">·</span>
      <span className="num">{fps} fps</span>
    </>
  );
}

export function StatusBar({ mode, dirty, entityCount, pointer, note }: StatusBarProps) {
  return (
    <div className="statusbar">
      <span>{mode} mode</span>
      <span className="sep">·</span>
      <span>{dirty ? 'unsaved changes' : 'saved'}</span>
      <span className="sep">·</span>
      <FrameReadout />
      <span className="sep">·</span>
      <span>
        <span className="num">{entityCount}</span> entities
      </span>
      <span className="sep">·</span>
      <span>Y-up</span>
      <span className="sep">·</span>
      <span>
        grid <span className="num">1m</span>
      </span>
      {note != null && note !== '' && (
        <>
          <span className="sep">·</span>
          <span className="status-note">{note}</span>
        </>
      )}
      {pointer != null && (
        <>
          <span className="sep">·</span>
          <span>
            pointer{' '}
            <span className="num">
              {pointer[0].toFixed(2)}, {pointer[1].toFixed(2)}, {pointer[2].toFixed(2)}
            </span>
          </span>
        </>
      )}
    </div>
  );
}
