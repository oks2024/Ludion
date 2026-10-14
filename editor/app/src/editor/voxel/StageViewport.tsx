// The stage viewport: the model stage's frame stream, plus the UE5 view camera that flies and
// orbits it (`Plan/editor-stage-camera.md` W3, decision 4).
//
// Deliberately NOT `viewer/Viewport.tsx`. That component is the game session's surface — picking,
// gizmos, the 3D pointer, play-mode input injection — and every one of those is meaningless here:
// the stage holds one subject, has no sim, and is never edited. What the two share is the frame
// stream, so this reuses `useFrameStream` and nothing else.
//
// THE CAMERA IS THE VIEW'S, NOT THIS COMPONENT'S. `useViewCamera` owns the gesture math and the
// `render.view` writes; this component only spreads its pointer handlers onto the canvas and reports
// the canvas's CSS size back, so the view's framing math has the real aspect.

import { useEffect, useRef } from 'react';
import type { ProtocolClient } from '../../protocol/client';
import type { ViewCameraHandlers } from '../camera/useViewCamera';
import { useFrameStream, type FrameStatus } from '../../viewer/useFrameStream';
import type { Viewport as ViewportSize } from './orbit';
import type { StageSlot } from './stage';
import { StageSlotCard } from './StageSlotCard';

interface StageViewportProps {
  /** The stage's own client, or `null` when it is not dialed (then `slot` explains why). */
  readonly client: ProtocolClient | null;
  /** Non-null replaces the canvas with a state card (starting / failed / no project dir / …). */
  readonly slot: StageSlot | null;
  readonly onRestart: () => void;
  /** Called when the human clicks **Add stage scenes** on a missing-scene card. Rendered only when
   *  both this and `slot.canAddScenes` are set. */
  readonly onAddScenes?: () => void;
  /** The view camera's pointer handlers — `useViewCamera(...).handlers`, spread onto the canvas
   *  verbatim. */
  readonly handlers: ViewCameraHandlers;
  /** The canvas's CSS size, reported on mount and resize — the framing math's viewport. */
  readonly onViewportSize: (size: ViewportSize) => void;
  readonly onFrameStatus?: (status: FrameStatus) => void;
  /** What is on the stage, for the top-left chip. */
  readonly label: string;
}

export function StageViewport({
  client,
  slot,
  onRestart,
  onAddScenes,
  handlers,
  onViewportSize,
  onFrameStatus,
  label,
}: StageViewportProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const stageRef = useRef<HTMLDivElement | null>(null);
  const status = useFrameStream(client, canvasRef, null);

  useEffect(() => {
    onFrameStatus?.(status);
  }, [status, onFrameStatus]);

  // Report the canvas's CSS size so the view can frame against the real aspect. The canvas only
  // exists once the stage is live, hence the dependency on `slot`.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (canvas === null) {
      return;
    }
    const report = (): void => {
      const rect = canvas.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        onViewportSize({ width: rect.width, height: rect.height });
      }
    };
    report();
    const observer = new ResizeObserver(report);
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [onViewportSize, slot]);

  if (slot !== null) {
    return (
      <div className="viewport vx-stage" ref={stageRef}>
        <StageSlotCard slot={slot}>
          {slot.canAddScenes && onAddScenes && (
            <button type="button" className="icon-btn" onClick={onAddScenes}>
              Add stage scenes
            </button>
          )}
          {slot.canRestart && (
            <button type="button" className="icon-btn" onClick={onRestart}>
              Restart the stage
            </button>
          )}
        </StageSlotCard>
      </div>
    );
  }

  const frameMs = status.fps > 0 ? (1000 / status.fps).toFixed(1) : '—';

  return (
    <div className="viewport vx-stage" ref={stageRef} {...handlers}>
      <canvas ref={canvasRef} className="viewport-canvas" />
      {!status.receiving && <div className="viewport-waiting">waiting for frames…</div>}

      <div className="vp-overlay vp-tl">{label}</div>
      <div className={status.receiving ? 'vp-overlay vp-tr ok' : 'vp-overlay vp-tr'}>
        <span className="num">{frameMs} ms</span>
        <span className="sep">·</span>
        <span className="num">{status.fps} fps</span>
      </div>
    </div>
  );
}
