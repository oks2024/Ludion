// The Performance view's game preview: what the frame WAS, beside the numbers saying what it cost.
//
// The instrument alone cannot answer "why did that spike happen" — a breakdown says `enemy_ai` grew,
// not that the horde just spawned. This is the missing half, and it is deliberately small: a 16:9 box
// in the side column, no picking, no gizmos, no selection. Deliberately NOT `viewer/Viewport.tsx` for
// exactly that reason — that component's props are the editing session (selection, edit entries, the
// 3D pointer), none of which exist here. `editor/voxel/StageViewport.tsx` is the precedent this
// follows: reuse the frame stream, and nothing else.
//
// IT ADDS NO GPU WORK, which is what makes it safe to watch while reading GPU timings:
//   1. No client can change the render resolution. `stream.subscribe` carries a channel and a
//      delivery mode and nothing else, and the whole 27-method surface has no resize verb — the
//      stream renders at the engine's own `render_size`, fixed at construction.
//   2. The frame is already rendered. An armed profiler counts as a watcher (ludion-engine's
//      `live.rs`), so while this view holds its `profile` subscription the engine renders every
//      frame and DROPS the image when nobody streams. Subscribing here stops the discard; it does
//      not add a render.
//   3. Only the display scales. `useFrameStream` sets the canvas's intrinsic size to the frame's
//      pixel size and CSS fits it into this box, so a 1280x720 frame stays a 1280x720 frame.
//
// It is NOT free of transport cost, and that is the point rather than a caveat: the QOI encode and
// the socket write are real, they are this viewer's, and they are what the view's Transport group
// measures — absent entirely until something streams. The top-left chip carries the frame's real
// pixel dimensions for the same reason: it is how a human reads "still rendering full resolution"
// off the screen instead of trusting this comment.

import {
  useCallback,
  useEffect,
  useRef,
  type KeyboardEvent,
  type MouseEvent,
  type PointerEvent,
} from 'react';
import type { InputInjectParams } from '@ludion/protocol';
import type { EditorMode } from '../App';
import type { ProtocolClient } from '../protocol/client';
import { keyEdgeToInject, ModifierFold } from '../viewer/keymap';
import { createLagProbe, formatLag, type LagProbe } from '../viewer/lagProbe';
import {
  mouseButtonInject,
  mouseMoveInject,
  pointerButtonName,
  releaseInjects,
  type EngineMouseButton,
} from '../viewer/mousemap';
import { canvasClientRect, clientToEnginePoint } from '../viewer/picking';
import { useFrameStream, type FrameStatus } from '../viewer/useFrameStream';

/** Delivery cap for the preview. It throttles the per-subscriber forwarder, NOT the encode: the
 *  server encodes each frame once before the fan-out (ludion-server's `route_frame`), so this buys
 *  websocket bytes and client-side QOI decode and never server CPU. */
const PREVIEW_MAX_FPS = 30;

interface PreviewPanelProps {
  /** The game session's client, or `null` when the preview is off — the one off switch: the stream
   *  effect returns early and its cleanup has already unsubscribed. */
  readonly client: ProtocolClient | null;
  readonly mode: EditorMode;
  readonly onFrameStatus?: (status: FrameStatus) => void;
}

export function PreviewPanel({ client, mode, onFrameStatus }: PreviewPanelProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  // This panel's own latency instrument. Deliberately not shared with the Viewport's: the two run at
  // different delivery caps over the same engine, so their readings are the comparison.
  const probeRef = useRef<LagProbe | null>(null);
  if (probeRef.current === null) {
    probeRef.current = createLagProbe();
  }
  const probe = probeRef.current;
  const status = useFrameStream(client, canvasRef, PREVIEW_MAX_FPS, probe);

  useEffect(() => {
    onFrameStatus?.(status);
  }, [status, onFrameStatus]);

  // The buttons the engine still thinks are held, and the move coalescer: one `mouse_move` per
  // animation frame at most, so a high-Hz pointer can't flood the very connection being measured.
  const heldButtonsRef = useRef<Set<EngineMouseButton>>(new Set());
  // Folds left/right modifier keys into one engine key across key events.
  const modifierFoldRef = useRef(new ModifierFold());
  const pendingMoveRef = useRef<{ x: number; y: number } | null>(null);
  const moveRafRef = useRef<number | null>(null);

  // Fire-and-forget, the same posture as the viewport's: input is high-frequency, so a transient
  // failure is never worth a banner. With the preview off there is no client and nothing to send.
  const inject = useCallback(
    (params: InputInjectParams): void => {
      void client?.call({ method: 'input.inject', params }).catch(() => {});
    },
    [client],
  );

  const releaseHeld = useCallback((): void => {
    for (const params of releaseInjects(heldButtonsRef.current)) {
      inject(params);
    }
    heldButtonsRef.current.clear();
  }, [inject]);

  // Edit mode pauses the sim by convention; keystrokes are gameplay input and stay play-mode only.
  const handleKey = useCallback(
    (event: KeyboardEvent<HTMLDivElement>) => {
      // Before anything else: the gap to the event's own timestamp is the main thread's dispatch
      // delay, and work done first would be counted as queue time.
      const handledAt = performance.now();
      if (mode !== 'play') {
        return;
      }
      const params = keyEdgeToInject({ code: event.code, type: event.type as 'keydown' | 'keyup', repeat: event.repeat }, modifierFoldRef.current);
      if (params === null) {
        return;
      }
      // The key is one the engine consumes: stop the browser's default (arrow/space scrolling).
      event.preventDefault();
      // The press is measured (a release is the same trip, but "press it and see it" is the latency a
      // human reports), so this path keeps the reply the shared `inject` discards. `nativeEvent`
      // rather than React's synthetic timestamp, which substitutes `Date.now()` for a falsy native
      // value and would sit on a different time origin.
      const token = event.type === 'keydown' ? probe.noteInject(event.nativeEvent.timeStamp, handledAt) : null;
      void client
        ?.callFull({ method: 'input.inject', params })
        .then((response) => {
          if (token !== null) {
            probe.resolveInject(token, response.frame);
          }
        })
        .catch(() => {
          if (token !== null) {
            probe.resolveInject(token, null);
          }
        });
    },
    [mode, client, probe],
  );

  const handlePointerDown = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      const button = mode === 'play' ? pointerButtonName(event.button) : null;
      if (button === null) {
        return;
      }
      heldButtonsRef.current.add(button);
      inject(mouseButtonInject(button, 'down'));
    },
    [mode, inject],
  );

  const handlePointerUp = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      const button = pointerButtonName(event.button);
      if (button !== null && heldButtonsRef.current.delete(button)) {
        inject(mouseButtonInject(button, 'up'));
      }
    },
    [inject],
  );

  const handlePointerMove = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      const canvas = canvasRef.current;
      if (mode !== 'play' || canvas === null) {
        return;
      }
      // Render-target pixels via the same mapping the picker uses, so the game's cursor lands on the
      // pixel under the human's.
      const point = clientToEnginePoint({ x: event.clientX, y: event.clientY }, canvasClientRect(canvas));
      if (point === null) {
        return;
      }
      pendingMoveRef.current = point;
      if (moveRafRef.current === null) {
        moveRafRef.current = requestAnimationFrame(() => {
          moveRafRef.current = null;
          const latest = pendingMoveRef.current;
          pendingMoveRef.current = null;
          if (latest !== null) {
            inject(mouseMoveInject(latest));
          }
        });
      }
    },
    [mode, inject],
  );

  // A button carried off the surface would otherwise never see its up edge.
  const handlePointerLeave = useCallback((): void => {
    releaseHeld();
  }, [releaseHeld]);

  // Play mode forwards a right-click to the sim, so the browser menu must not eat it.
  const handleContextMenu = useCallback(
    (event: MouseEvent<HTMLDivElement>): void => {
      if (mode === 'play') {
        event.preventDefault();
      }
    },
    [mode],
  );

  // Leaving play mode, losing the client (preview off), and unmounting all end the same way: release
  // whatever is held — through the client that was live when it was pressed, since the cleanup runs
  // with the previous closure — and drop a queued move so no rAF fires into a dead panel.
  useEffect(() => {
    if (mode !== 'play') {
      releaseHeld();
    }
    return () => {
      releaseHeld();
      if (moveRafRef.current !== null) {
        cancelAnimationFrame(moveRafRef.current);
        moveRafRef.current = null;
      }
      pendingMoveRef.current = null;
    };
  }, [mode, releaseHeld]);

  if (client === null) {
    return (
      <div className="viewport pf-preview-stage">
        <div className="pf-preview-off">preview off</div>
      </div>
    );
  }

  const frameMs = status.fps > 0 ? (1000 / status.fps).toFixed(1) : '—';

  return (
    <div
      className="viewport pf-preview-stage"
      tabIndex={0}
      role="application"
      aria-label="Game preview (focus to send input in play mode)"
      onKeyDown={handleKey}
      onKeyUp={handleKey}
      onPointerDown={handlePointerDown}
      onPointerUp={handlePointerUp}
      onPointerCancel={handlePointerUp}
      onPointerMove={handlePointerMove}
      onPointerLeave={handlePointerLeave}
      onContextMenu={handleContextMenu}
    >
      <canvas ref={canvasRef} className="viewport-canvas" />
      {!status.receiving && <div className="viewport-waiting">waiting for frames…</div>}

      {/* The rendered size, not the CSS size: the honest readout that the shrunken box costs the GPU
          nothing. */}
      <div className="vp-overlay vp-tl">
        {status.width === null || status.height === null ? 'no frame' : `${status.width}×${status.height}`}
      </div>
      <div className={status.receiving ? 'vp-overlay vp-tr ok' : 'vp-overlay vp-tr'}>
        <span className="num">{frameMs} ms</span>
        <span className="sep">·</span>
        <span className="num">{status.fps} fps</span>
        <span className="sep">·</span>
        <span className="num">{formatLag(status.lag)}</span>
      </div>
    </div>
  );
}
