// The gizmo's SVG chrome: draws a `GizmoLayout` (engine-pixel geometry from `model.ts`) over the
// streamed frame. Pointer-events are off on everything — hit-testing is the pure `hitTest`, driven by
// the stage's own pointer handlers, never the DOM — so this is display only.
//
// Geometry stays in ENGINE pixels: the `<svg>` is sized to exactly overlay the canvas's CSS box and
// carries a `viewBox` of the engine render size with `preserveAspectRatio="none"`, so it stretches
// engine coordinates onto the (possibly non-uniformly scaled) canvas box the same way the selection
// overlay's `engineRectToCssRect` does. Strokes use `non-scaling-stroke` so they stay crisp under that
// stretch.

import { memo } from 'react';
import type { Axis, GizmoLayout, GizmoHandle } from '../editor/gizmo/model';
import type { Point } from './picking';

/** The canvas's placement within the stage plus its engine size — the SVG box and viewBox. */
export interface CanvasBox {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
  readonly engineW: number;
  readonly engineH: number;
}

const AXIS_COLOR: Readonly<Record<Axis, string>> = { x: '#e5484d', y: '#30a46c', z: '#0091ff' };
const CENTER_COLOR = '#f0f0f5';

/** The color a handle draws in — its axis hue, or a neutral tone for the uniform-scale center. */
function handleColor(handle: GizmoHandle): string {
  return handle.id.axis !== undefined ? AXIS_COLOR[handle.id.axis] : CENTER_COLOR;
}

/**
 * Memoized because its props move far more slowly than its parent re-renders: `layout` and `box` are
 * rebuilt by the ~3 Hz overlay poll (or a drag), while the viewport's 4 Hz frame readout re-renders
 * the whole subtree around it. Without this, every one of those ticks reconciles the full handle SVG
 * for props that are referentially unchanged.
 */
export const GizmoOverlay = memo(function GizmoOverlay({
  layout,
  box,
  variant = 'entity',
}: {
  readonly layout: GizmoLayout;
  readonly box: CanvasBox;
  /** `'pointer'` additionally draws the 3D-pointer origin pin at the layout origin. */
  readonly variant?: 'entity' | 'pointer';
}) {
  if (box.engineW <= 0 || box.engineH <= 0) {
    return null;
  }
  return (
    <svg
      className="viewport-gizmo"
      style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
      viewBox={`0 0 ${box.engineW} ${box.engineH}`}
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      {layout.handles.map((handle, i) => (
        <Handle key={i} handle={handle} mode={layout.mode} />
      ))}
      {variant === 'pointer' && <OriginPin point={layout.origin} />}
    </svg>
  );
});

/** The 3D pointer's "here" marker: a ringed dot on the accent hue at the gizmo origin. Colors come
 *  from CSS (the `--accent` token) so the pin tracks the app's theme; strokes stay crisp under the
 *  non-uniform engine→canvas stretch like the handles do. */
function OriginPin({ point }: { readonly point: Point }) {
  return (
    <g className="gizmo-pin">
      <circle cx={point.x} cy={point.y} r={7} fill="none" strokeWidth={2} vectorEffect="non-scaling-stroke" />
      <circle className="pin-dot" cx={point.x} cy={point.y} r={2.5} />
    </g>
  );
}

function Handle({ handle, mode }: { readonly handle: GizmoHandle; readonly mode: GizmoLayout['mode'] }) {
  const color = handleColor(handle);
  const g = handle.geometry;
  switch (g.type) {
    case 'segment': {
      const { a, b } = g.segment;
      return (
        <g>
          <line x1={a.x} y1={a.y} x2={b.x} y2={b.y} stroke={color} strokeWidth={2} vectorEffect="non-scaling-stroke" />
          {mode === 'scale' ? (
            <ScaleTip point={b} color={color} />
          ) : (
            <Arrowhead from={a} to={b} color={color} />
          )}
        </g>
      );
    }
    case 'quad':
      return (
        <polygon
          points={g.quad.points.map((p) => `${p.x},${p.y}`).join(' ')}
          fill={color}
          fillOpacity={0.25}
          stroke={color}
          strokeWidth={1.5}
          vectorEffect="non-scaling-stroke"
        />
      );
    case 'polyline':
      return (
        <polyline
          points={g.polyline.points.map((p) => `${p.x},${p.y}`).join(' ')}
          fill="none"
          stroke={color}
          strokeWidth={2}
          vectorEffect="non-scaling-stroke"
        />
      );
    case 'center':
      return (
        <circle
          cx={g.disc.center.x}
          cy={g.disc.center.y}
          r={g.disc.radius}
          fill={color}
          fillOpacity={0.35}
          stroke={color}
          strokeWidth={1.5}
          vectorEffect="non-scaling-stroke"
        />
      );
  }
}

/** A filled triangle at `to`, pointing along `from → to` (the translate-arm tip). */
function Arrowhead({ from, to, color }: { readonly from: Point; readonly to: Point; readonly color: string }) {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const len = Math.hypot(dx, dy);
  if (len < 1e-6) {
    return null;
  }
  const ux = dx / len;
  const uy = dy / len;
  const size = Math.max(6, len * 0.16);
  const width = size * 0.5;
  const baseX = to.x - ux * size;
  const baseY = to.y - uy * size;
  const points = [
    `${to.x},${to.y}`,
    `${baseX - uy * width},${baseY + ux * width}`,
    `${baseX + uy * width},${baseY - ux * width}`,
  ].join(' ');
  return <polygon points={points} fill={color} />;
}

/** A small filled square centered on `point` (the scale-arm tip). */
function ScaleTip({ point, color }: { readonly point: Point; readonly color: string }) {
  const r = 5;
  return <rect x={point.x - r} y={point.y - r} width={r * 2} height={r * 2} fill={color} />;
}
