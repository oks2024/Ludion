// Gizmo layout + hit test: turns the selected entity's `Transform` and the camera kit into plain
// screen-space geometry the UI draws as SVG, and answers "which handle is under this pixel". Pure and
// transport-free like picking.ts, so every layout invariant and hit-priority tie is unit-testable
// without a browser (see model.test.ts).
//
// The two spaces: handles are defined in WORLD space (axis segments, plane quads, rotation rings, all
// axis-aligned to X/Y/Z), then projected to SCREEN space (engine pixels) through the shared camera
// math. The gizmo is WORLD-AXIS-ALIGNED, not object-aligned — the simplest correct choice given
// rotations are stored as world-frame Euler degrees, and it keeps the rotate solver's axis a fixed
// world axis. Handles keep a CONSTANT APPARENT SIZE: their world extent is scaled so the axis arms
// span ~80 px regardless of camera distance or projection (`worldLengthForPixels`), which is
// well-defined for orthographic too. The whole gizmo is hidden when the origin is behind the camera.

import type { Point } from '../../viewer/picking';
import {
  axisVector,
  worldLengthForPixels,
  worldToScreen,
  type CameraKit,
  type Transform,
  type Viewport,
} from './camera';
import { vAdd, vScale, type Vec3 } from './vecmath';

/** Which transform the gizmo manipulates. */
export type GizmoMode = 'translate' | 'rotate' | 'scale';

/** A world axis. */
export type Axis = 'x' | 'y' | 'z';

/**
 * A handle identity. `axis` means: for `axis`/`ring`/`scale` handles the axis they act on; for a
 * `plane` handle the axis NORMAL to the plane (so plane XY → `z`, YZ → `x`, XZ → `y`). `center` (the
 * uniform-scale handle) carries no axis.
 */
export interface HandleId {
  readonly kind: 'axis' | 'plane' | 'ring' | 'center';
  readonly axis?: Axis;
}

/** A projected line segment (axis arm). */
export interface Segment {
  readonly a: Point;
  readonly b: Point;
}

/** A projected quad (plane handle), corners in order. */
export interface Quad {
  readonly points: readonly [Point, Point, Point, Point];
}

/** A projected polyline (rotation ring), behind-camera points dropped. */
export interface Polyline {
  readonly points: readonly Point[];
}

/** A screen disc (uniform-scale center handle): a pixel radius around a point. */
export interface CenterDisc {
  readonly center: Point;
  readonly radius: number;
}

/** The drawable geometry of one handle, discriminated by `type`. */
export type HandleGeometry =
  | { readonly type: 'segment'; readonly segment: Segment }
  | { readonly type: 'quad'; readonly quad: Quad }
  | { readonly type: 'polyline'; readonly polyline: Polyline }
  | { readonly type: 'center'; readonly disc: CenterDisc };

/** One drawable, hit-testable handle. */
export interface GizmoHandle {
  readonly id: HandleId;
  readonly geometry: HandleGeometry;
}

/** The full projected gizmo for one mode. `origin` is the entity origin in screen pixels. */
export interface GizmoLayout {
  readonly mode: GizmoMode;
  readonly origin: Point;
  readonly handles: readonly GizmoHandle[];
}

/** Target on-screen length of an axis arm, in engine pixels. */
export const HANDLE_PIXELS = 80;
/** Rotation ring sampling (segments; the polyline holds up to this many + 1 closing points). */
export const RING_SEGMENTS = 48;
/** Plane quads sit between these fractions of an arm from the origin (a small offset square). */
const PLANE_INNER_FRAC = 0.18;
const PLANE_OUTER_FRAC = 0.5;
/** Pixel radius of the uniform-scale center handle. */
const CENTER_PIXEL_RADIUS = 8;
/** Hit tolerance to a segment/polyline, in pixels. */
export const HIT_TOLERANCE = 6;

const AXES: readonly Axis[] = ['x', 'y', 'z'];

/** The two in-plane axes for a plane whose normal is `axis`. */
function planeAxes(axis: Axis): readonly [Axis, Axis] {
  return axis === 'z' ? ['x', 'y'] : axis === 'x' ? ['y', 'z'] : ['x', 'z'];
}

/** The two axes `(u, v)` spanning the ring plane about `axis`, with `u × v = axis` (right-handed). */
function ringAxes(axis: Axis): readonly [Axis, Axis] {
  return axis === 'x' ? ['y', 'z'] : axis === 'y' ? ['z', 'x'] : ['x', 'y'];
}

/**
 * Builds the projected gizmo for `mode`, or `null` when it must be hidden — the origin is behind the
 * camera, or the projection is too degenerate to size handles. Individual handles whose endpoints
 * fall behind the camera are omitted; the gizmo still shows the rest.
 */
export function buildGizmo(
  transform: Transform,
  kit: CameraKit,
  size: Viewport,
  mode: GizmoMode,
): GizmoLayout | null {
  const o = transform.translation;
  const originScreen = worldToScreen(o, kit.viewProj, size);
  if (originScreen === null) {
    return null;
  }
  const armLen = worldLengthForPixels(o, HANDLE_PIXELS, kit, size);
  if (armLen === null || !Number.isFinite(armLen) || armLen <= 0) {
    return null;
  }

  const handles: GizmoHandle[] = [];
  if (mode === 'translate') {
    pushAxisArms(handles, o, armLen, kit, size);
    for (const axis of AXES) {
      const quad = planeQuad(o, axis, armLen, kit, size);
      if (quad !== null) {
        handles.push({ id: { kind: 'plane', axis }, geometry: { type: 'quad', quad } });
      }
    }
  } else if (mode === 'rotate') {
    for (const axis of AXES) {
      const polyline = ringPolyline(o, axis, armLen, kit, size);
      if (polyline !== null) {
        handles.push({ id: { kind: 'ring', axis }, geometry: { type: 'polyline', polyline } });
      }
    }
  } else {
    pushAxisArms(handles, o, armLen, kit, size);
    handles.push({
      id: { kind: 'center' },
      geometry: { type: 'center', disc: { center: originScreen, radius: CENTER_PIXEL_RADIUS } },
    });
  }

  return { mode, origin: originScreen, handles };
}

/** Appends the three axis arms (X/Y/Z segments), skipping any that project behind the camera. */
function pushAxisArms(out: GizmoHandle[], o: Vec3, armLen: number, kit: CameraKit, size: Viewport): void {
  for (const axis of AXES) {
    const tip = vAdd(o, vScale(axisVector(axis), armLen));
    const a = worldToScreen(o, kit.viewProj, size);
    const b = worldToScreen(tip, kit.viewProj, size);
    if (a !== null && b !== null) {
      out.push({ id: { kind: 'axis', axis }, geometry: { type: 'segment', segment: { a, b } } });
    }
  }
}

/** Projects the offset plane-handle square, or `null` if any corner is behind the camera. */
function planeQuad(o: Vec3, normal: Axis, armLen: number, kit: CameraKit, size: Viewport): Quad | null {
  const [ax, ay] = planeAxes(normal);
  const ex = axisVector(ax);
  const ey = axisVector(ay);
  const inner = armLen * PLANE_INNER_FRAC;
  const outer = armLen * PLANE_OUTER_FRAC;
  const corners: Vec3[] = [
    vAdd(o, vAdd(vScale(ex, inner), vScale(ey, inner))),
    vAdd(o, vAdd(vScale(ex, outer), vScale(ey, inner))),
    vAdd(o, vAdd(vScale(ex, outer), vScale(ey, outer))),
    vAdd(o, vAdd(vScale(ex, inner), vScale(ey, outer))),
  ];
  const projected: Point[] = [];
  for (const c of corners) {
    const p = worldToScreen(c, kit.viewProj, size);
    if (p === null) {
      return null;
    }
    projected.push(p);
  }
  return { points: [projected[0], projected[1], projected[2], projected[3]] };
}

/** Projects a rotation ring as a polyline, dropping behind-camera samples; `null` if too few survive. */
function ringPolyline(o: Vec3, axis: Axis, radius: number, kit: CameraKit, size: Viewport): Polyline | null {
  const [ua, va] = ringAxes(axis);
  const u = axisVector(ua);
  const v = axisVector(va);
  const points: Point[] = [];
  for (let i = 0; i <= RING_SEGMENTS; i++) {
    const t = (i / RING_SEGMENTS) * Math.PI * 2;
    const world = vAdd(o, vAdd(vScale(u, Math.cos(t) * radius), vScale(v, Math.sin(t) * radius)));
    const p = worldToScreen(world, kit.viewProj, size);
    if (p !== null) {
      points.push(p);
    }
  }
  return points.length >= 2 ? { points } : null;
}

// --- Hit testing ------------------------------------------------------------------------------- //

/** Higher wins when handles overlap: planes/center sit on top, then axes, then rings underneath. */
function priority(kind: HandleId['kind']): number {
  return kind === 'plane' || kind === 'center' ? 3 : kind === 'axis' ? 2 : 1;
}

/**
 * The handle under `point` (engine pixels), or `null` for a miss. Among handles within tolerance the
 * winner is the highest-priority one (plane/center over axis over ring); ties break to the smaller
 * distance, then to layout order. Segment/polyline handles hit within `HIT_TOLERANCE`; plane quads
 * hit on interior; the center disc hits within its radius.
 */
export function hitTest(point: Point, layout: GizmoLayout): HandleId | null {
  let best: { id: HandleId; pr: number; dist: number } | null = null;
  for (const handle of layout.handles) {
    const dist = handleDistance(point, handle.geometry);
    if (dist === null) {
      continue;
    }
    const pr = priority(handle.id.kind);
    if (best === null || pr > best.pr || (pr === best.pr && dist < best.dist)) {
      best = { id: handle.id, pr, dist };
    }
  }
  return best?.id ?? null;
}

/** A hit "distance" (smaller = tighter) if `point` hits `geometry`, else `null` for a miss. */
function handleDistance(point: Point, geometry: HandleGeometry): number | null {
  switch (geometry.type) {
    case 'segment': {
      const d = pointSegmentDistance(point, geometry.segment.a, geometry.segment.b);
      return d <= HIT_TOLERANCE ? d : null;
    }
    case 'polyline': {
      const d = pointPolylineDistance(point, geometry.polyline.points);
      return d !== null && d <= HIT_TOLERANCE ? d : null;
    }
    case 'quad': {
      if (!pointInPolygon(point, geometry.quad.points)) {
        return null;
      }
      return pointToCentroid(point, geometry.quad.points);
    }
    case 'center': {
      const d = Math.hypot(point.x - geometry.disc.center.x, point.y - geometry.disc.center.y);
      return d <= geometry.disc.radius ? d : null;
    }
  }
}

/** Distance from `p` to segment `a→b` (the classic clamped-projection formula). */
export function pointSegmentDistance(p: Point, a: Point, b: Point): number {
  const abx = b.x - a.x;
  const aby = b.y - a.y;
  const lenSq = abx * abx + aby * aby;
  if (lenSq < 1e-12) {
    return Math.hypot(p.x - a.x, p.y - a.y);
  }
  let t = ((p.x - a.x) * abx + (p.y - a.y) * aby) / lenSq;
  t = Math.max(0, Math.min(1, t));
  return Math.hypot(p.x - (a.x + t * abx), p.y - (a.y + t * aby));
}

/** Minimum distance from `p` to any segment of the polyline, or `null` for fewer than 2 points. */
function pointPolylineDistance(p: Point, points: readonly Point[]): number | null {
  if (points.length < 2) {
    return null;
  }
  let min = Infinity;
  for (let i = 0; i + 1 < points.length; i++) {
    min = Math.min(min, pointSegmentDistance(p, points[i], points[i + 1]));
  }
  return min;
}

/** Ray-casting point-in-polygon (works for the possibly non-convex projected quad). */
function pointInPolygon(p: Point, poly: readonly Point[]): boolean {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const yi = poly[i].y;
    const yj = poly[j].y;
    const xi = poly[i].x;
    const xj = poly[j].x;
    const intersects = yi > p.y !== yj > p.y && p.x < ((xj - xi) * (p.y - yi)) / (yj - yi) + xi;
    if (intersects) {
      inside = !inside;
    }
  }
  return inside;
}

/** Distance from `p` to a polygon's vertex centroid — the plane-handle tie-break key. */
function pointToCentroid(p: Point, poly: readonly Point[]): number {
  let cx = 0;
  let cy = 0;
  for (const v of poly) {
    cx += v.x;
    cy += v.y;
  }
  cx /= poly.length;
  cy /= poly.length;
  return Math.hypot(p.x - cx, p.y - cy);
}
