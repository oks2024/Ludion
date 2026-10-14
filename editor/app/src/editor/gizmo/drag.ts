// Drag sessions, solvers, and snapping for the transform gizmos — a pure state machine with no
// timers and no DOM. A drag captures everything it needs at mousedown (`beginDrag`), maps each
// subsequent pointer position to a full preview `Transform` (`updateDrag`), and yields the single
// undo entry at release (`endDrag`). `component.set` takes WHOLE component values, so `updateDrag`
// returns the entire `Transform`, not a patch; per plan decision #7 one drag is ONE undo entry
// (value-at-mousedown → value-at-release) while every intermediate preview is still sent live.
//
// Two spaces again: the pointer arrives in SCREEN pixels and is unprojected to a WORLD picking ray
// (`screenToRay`); the solve happens in WORLD space against the handle's world axis/plane. The camera
// is captured in the session because edit mode pauses the sim (decision #6), so the view is stable
// for the drag's lifetime.
//
// Snapping quantizes AT THE GIZMO (decision #7 and the settled pillar): a snapped value is what gets
// sent, but the canonical scene writer stays lossless — quantization happens here, never in
// serialization. A step of `≤ 0` means "no snap".
//
// Degeneracy is contained in one place: any solve that would produce a non-finite number (ray
// parallel to the axis or plane, a zero-length base, a near-zero divisor) HOLDS the last good value
// instead. `updateDrag` never returns `NaN`.

import {
  applyWorldAxisRotation,
  axisVector,
  eulerXyzDegToQuat,
  quatToEulerXyzDeg,
  screenToRay,
  worldToScreen,
  type CameraKit,
  type Ray,
  type Transform,
  type Viewport,
} from './camera';
import type { Axis, GizmoMode, HandleId } from './model';
import type { Point } from '../../viewer/picking';
import { quatConjugate, quatMul, quatRotate, vAdd, vCross, vDot, vFinite, vLen, vNormalize, vScale, vSub, type Quat, type Vec3 } from './vecmath';

/** Snap steps applied at the gizmo. Each `≤ 0` disables snapping for that channel. */
export interface SnapSettings {
  /** World-unit grid for translation deltas. */
  readonly translateStep: number;
  /** Degree grid for rotation angle. */
  readonly rotateStepDeg: number;
  /** Multiplicative-result grid for scale components. */
  readonly scaleStep: number;
}

/** The one undo entry a drag produces: the value at mousedown and the value at release. */
export interface DragUndo {
  readonly before: Transform;
  readonly after: Transform;
}

/** A parent's world rotation and scale — the frame a child's `Parent`-local `Transform` lives in. */
export interface ParentFrame {
  readonly rotation: Quat;
  readonly scale: Vec3;
}

/** Where a parented entity stands at mousedown: its world pose and the parent's frame. */
export interface GizmoFrame {
  readonly world: Transform;
  readonly parent: ParentFrame;
}

/** Fields every session shares: the captured camera/viewport, the base value, and the held value. */
interface SessionBase {
  readonly kit: CameraKit;
  readonly size: Viewport;
  /** The `Transform` at mousedown (`Parent`-local) — the undo `before` and the degenerate-frame fallback. */
  readonly base: Transform;
  /** The world pose and parent frame at mousedown; `null` for a root, whose local IS its world pose. */
  readonly frame: GizmoFrame | null;
  /** The last finite value emitted; updated by `updateDrag`, held through degenerate frames. */
  lastValue: Transform;
}

type TranslateAxisSession = SessionBase & {
  readonly kind: 'translate-axis';
  readonly axis: Axis;
  readonly axisDir: Vec3;
  readonly origin: Vec3;
  readonly startParam: number;
};

type TranslatePlaneSession = SessionBase & {
  readonly kind: 'translate-plane';
  readonly normal: Vec3;
  readonly inPlane: readonly [Axis, Axis];
  readonly origin: Vec3;
  readonly startHit: Vec3;
};

type RotateSession = SessionBase & {
  readonly kind: 'rotate';
  readonly axisDir: Vec3;
  readonly origin: Vec3;
  readonly startDir: Vec3;
};

type ScaleAxisSession = SessionBase & {
  readonly kind: 'scale-axis';
  readonly axis: Axis;
  readonly axisDir: Vec3;
  readonly origin: Vec3;
  readonly startParam: number;
  readonly baseComponent: number;
};

type ScaleUniformSession = SessionBase & {
  readonly kind: 'scale-uniform';
  readonly originScreen: Point;
  readonly startDist: number;
  readonly baseScale: Vec3;
};

/** An in-progress gizmo drag. Opaque to callers except through `updateDrag`/`endDrag`. */
export type DragSession =
  | TranslateAxisSession
  | TranslatePlaneSession
  | RotateSession
  | ScaleAxisSession
  | ScaleUniformSession;

/** Normalized-dot magnitude above which two directions count as parallel (ray/axis, per decision). */
const PARALLEL_DOT = 0.999;

/** The index of an axis in a `[x, y, z]` triple. */
function axisIndex(axis: Axis): 0 | 1 | 2 {
  return axis === 'x' ? 0 : axis === 'y' ? 1 : 2;
}

/** The two in-plane axes for a plane whose normal is `axis`. */
function planeAxes(axis: Axis): readonly [Axis, Axis] {
  return axis === 'z' ? ['x', 'y'] : axis === 'x' ? ['y', 'z'] : ['x', 'z'];
}

/**
 * Begins a drag on `handleId`, capturing the base value and the mousedown geometry. `mode` is needed
 * because an `axis` handle is ambiguous on its own — a translate gizmo and a scale gizmo both draw
 * axis arms with the same `HandleId` — so the current gizmo mode selects translate-axis vs scale-axis
 * (the caller always knows its mode). `plane`/`ring`/`center` are mode-unique and ignore `mode`.
 *
 * `baseTransform` is the entity's local value; for a parented entity `frame` carries its world pose and
 * the parent's frame — the solve happens in world space around the world origin, and each channel is
 * converted back to a local value (see `localDelta`), so undo `before`/`after` stay local.
 *
 * Returns `null` when the drag cannot start — the pointer does not form a ray, the handle is unknown,
 * or the initial pick is degenerate (an axis viewed exactly edge-on, or the pointer at the origin).
 */
export function beginDrag(
  handleId: HandleId,
  mode: GizmoMode,
  baseTransform: Transform,
  pointerEnginePoint: Point,
  cameraKit: CameraKit,
  viewport: Viewport,
  frame: GizmoFrame | null = null,
): DragSession | null {
  const origin = frame?.world.translation ?? baseTransform.translation;
  const shared = { kit: cameraKit, size: viewport, base: baseTransform, frame, lastValue: baseTransform };

  if (handleId.kind === 'center') {
    const originScreen = worldToScreen(origin, cameraKit.viewProj, viewport);
    if (originScreen === null) {
      return null;
    }
    const startDist = Math.hypot(pointerEnginePoint.x - originScreen.x, pointerEnginePoint.y - originScreen.y);
    if (startDist < 1e-6) {
      return null;
    }
    return { ...shared, kind: 'scale-uniform', originScreen, startDist, baseScale: baseTransform.scale };
  }

  const ray = screenToRay(pointerEnginePoint, viewport, cameraKit.invViewProj);
  if (ray === null || handleId.axis === undefined) {
    return null;
  }
  const axis = handleId.axis;
  const axisDir = axisVector(axis);

  if (handleId.kind === 'axis') {
    const startParam = closestParamOnLine(ray, origin, axisDir);
    if (startParam === null) {
      return null;
    }
    if (mode === 'scale') {
      return { ...shared, kind: 'scale-axis', axis, axisDir, origin, startParam, baseComponent: baseTransform.scale[axisIndex(axis)] };
    }
    return { ...shared, kind: 'translate-axis', axis, axisDir, origin, startParam };
  }

  if (handleId.kind === 'plane') {
    const startHit = rayPlaneIntersect(ray, origin, axisDir);
    if (startHit === null) {
      return null;
    }
    return { ...shared, kind: 'translate-plane', normal: axisDir, inPlane: planeAxes(axis), origin, startHit };
  }

  // kind === 'ring' (rotate)
  const hit = rayPlaneIntersect(ray, origin, axisDir);
  if (hit === null) {
    return null;
  }
  const startDir = vSub(hit, origin);
  if (vLen(startDir) < 1e-6) {
    return null;
  }
  return { ...shared, kind: 'rotate', axisDir, origin, startDir: vNormalize(startDir) };
}

/**
 * Maps the current pointer to the full preview `Transform`, snapped per `snap`. On any degenerate
 * frame the last good value is returned unchanged — never a non-finite value. The session's held
 * value is advanced as a side effect so a later degenerate frame holds the most recent good pose.
 */
export function updateDrag(session: DragSession, pointerEnginePoint: Point, snap: SnapSettings): Transform {
  const candidate = solve(session, pointerEnginePoint, snap);
  if (candidate !== null && transformFinite(candidate)) {
    session.lastValue = candidate;
    return candidate;
  }
  return session.lastValue;
}

/** Solves one frame for `session`, or `null` to signal "hold" (degenerate / non-finite). */
function solve(session: DragSession, pointer: Point, snap: SnapSettings): Transform | null {
  if (session.kind === 'scale-uniform') {
    const distNow = Math.hypot(pointer.x - session.originScreen.x, pointer.y - session.originScreen.y);
    if (session.startDist < 1e-6) {
      return null;
    }
    const ratio = distNow / session.startDist;
    const scale: Vec3 = [
      snapValue(session.baseScale[0] * ratio, snap.scaleStep),
      snapValue(session.baseScale[1] * ratio, snap.scaleStep),
      snapValue(session.baseScale[2] * ratio, snap.scaleStep),
    ];
    return { ...session.base, scale };
  }

  const ray = screenToRay(pointer, session.size, session.kit.invViewProj);
  if (ray === null) {
    return null;
  }

  switch (session.kind) {
    case 'translate-axis': {
      const paramNow = closestParamOnLine(ray, session.origin, session.axisDir);
      if (paramNow === null) {
        return null;
      }
      const delta = snapValue(paramNow - session.startParam, snap.translateStep);
      return translated(session, vScale(session.axisDir, delta));
    }
    case 'translate-plane': {
      const hit = rayPlaneIntersect(ray, session.origin, session.normal);
      if (hit === null) {
        return null;
      }
      const delta = vSub(hit, session.origin);
      const snapped: [number, number, number] = [delta[0], delta[1], delta[2]];
      for (const a of session.inPlane) {
        const i = axisIndex(a);
        snapped[i] = snapValue(delta[i], snap.translateStep);
      }
      return translated(session, snapped);
    }
    case 'rotate': {
      const hit = rayPlaneIntersect(ray, session.origin, session.axisDir);
      if (hit === null) {
        return null;
      }
      const dirNow = vSub(hit, session.origin);
      if (vLen(dirNow) < 1e-6) {
        return null;
      }
      const now = vNormalize(dirNow);
      const s = vDot(vCross(session.startDir, now), session.axisDir);
      const c = vDot(session.startDir, now);
      const angleDeg = snapValue((Math.atan2(s, c) * 180) / Math.PI, snap.rotateStepDeg);
      const axis = session.frame === null ? session.axisDir : quatRotate(quatConjugate(session.frame.parent.rotation), session.axisDir);
      return { ...session.base, rotation: applyWorldAxisRotation(session.base.rotation, axis, angleDeg) };
    }
    case 'scale-axis': {
      const paramNow = closestParamOnLine(ray, session.origin, session.axisDir);
      if (paramNow === null || Math.abs(session.startParam) < 1e-6) {
        return null;
      }
      const ratio = paramNow / session.startParam;
      const i = axisIndex(session.axis);
      const scale: [number, number, number] = [session.base.scale[0], session.base.scale[1], session.base.scale[2]];
      scale[i] = snapValue(session.baseComponent * ratio, snap.scaleStep);
      return { ...session.base, scale };
    }
  }
}

/**
 * `session.base` moved by a snapped WORLD delta. A root adds it to its translation directly (kept
 * bit-identical to the pre-hierarchy gizmo); a parented entity pulls it back through the parent's frame —
 * the inverse of `ludion_core::math::mul_trs`'s `p.t + rotate(p.r, p.s ⊙ local.t)`, so only the
 * translation channel changes.
 */
function translated(session: DragSession, worldDelta: Vec3): Transform {
  if (session.frame === null) {
    return { ...session.base, translation: vAdd(session.base.translation, worldDelta) };
  }
  const p = session.frame.parent;
  const d = quatRotate(quatConjugate(p.rotation), worldDelta);
  const local: Vec3 = [d[0] / p.scale[0], d[1] / p.scale[1], d[2] / p.scale[2]];
  return { ...session.base, translation: vAdd(session.base.translation, local) };
}

/**
 * The pose the overlay should draw the gizmo at: `lastValue` for a root, otherwise the mousedown world
 * pose with its translation moved by the local delta pushed forward through the parent's frame and its
 * rotation recomposed as parent ∘ the drag's current local rotation.
 */
export function dragWorld(session: DragSession): Transform {
  if (session.frame === null) {
    return session.lastValue;
  }
  const { world, parent } = session.frame;
  const dLocal = vSub(session.lastValue.translation, session.base.translation);
  const scaled: Vec3 = [dLocal[0] * parent.scale[0], dLocal[1] * parent.scale[1], dLocal[2] * parent.scale[2]];
  const rotation = quatToEulerXyzDeg(quatMul(parent.rotation, eulerXyzDegToQuat(session.lastValue.rotation)));
  return { ...world, translation: vAdd(world.translation, quatRotate(parent.rotation, scaled)), rotation };
}

/**
 * Ends the drag, returning the one undo entry: the base value and the final (last good) value. The
 * caller passes the last `updateDrag` result as `lastValue`; the recorded intermediate sets are
 * independent of this (decision #7 — a replay shows every set, undo collapses to this pair).
 */
export function endDrag(session: DragSession, lastValue: Transform): DragUndo {
  return { before: session.base, after: lastValue };
}

/** Quantizes `v` to the nearest multiple of `step`; `step ≤ 0` (or non-finite) returns `v` unchanged.
 *  Snapping lives here so the canonical writer stays lossless (settled pillar). */
export function snapValue(v: number, step: number): number {
  if (!(step > 0)) {
    return v;
  }
  return Math.round(v / step) * step;
}

/**
 * A latest-wins throttle decision for the ~15 Hz preview cadence: `true` when at least `intervalMs`
 * has elapsed since the last send. Pure — the wiring layer owns the clock and `lastSentMs`, and is
 * responsible for always sending the final value at `endDrag` regardless of this (so the last pointer
 * position is never dropped). Kept here rather than in the wiring layer so the cadence rule is tested
 * alongside the drag it paces.
 */
export function shouldSendUpdate(lastSentMs: number, nowMs: number, intervalMs: number): boolean {
  return nowMs - lastSentMs >= intervalMs;
}

// --- Geometry primitives ----------------------------------------------------------------------- //

/** Whether all three of a Transform's vectors are finite (the single finiteness gate). */
function transformFinite(t: Transform): boolean {
  return vFinite(t.translation) && vFinite(t.rotation) && vFinite(t.scale);
}

/**
 * The parameter along the axis line `lineOrigin + s·lineDir` (world units, `lineDir` unit) of the
 * point closest to the pick `ray`. `null` when the ray and axis are (near-)parallel — the closest
 * point is then ill-conditioned, so the caller holds.
 */
function closestParamOnLine(ray: Ray, lineOrigin: Vec3, lineDir: Vec3): number | null {
  const d1 = ray.dir;
  const d2 = lineDir;
  const a = vDot(d1, d1);
  const e = vDot(d2, d2);
  const b = vDot(d1, d2);
  const denom = a * e - b * b;
  if (denom <= 0 || Math.abs(b) / Math.sqrt(a * e) > PARALLEL_DOT) {
    return null;
  }
  const r = vSub(ray.origin, lineOrigin);
  const f = vDot(d2, r);
  const c = vDot(d1, r);
  const s1 = (b * f - c * e) / denom;
  return (b * s1 + f) / e;
}

/**
 * The point where `ray` meets the plane through `planePoint` with unit `normal`, or `null` when the
 * ray is (near-)parallel to the plane (its direction nearly perpendicular to the normal).
 */
function rayPlaneIntersect(ray: Ray, planePoint: Vec3, normal: Vec3): Vec3 | null {
  const denom = vDot(ray.dir, normal);
  if (Math.abs(denom) < 1e-6) {
    return null;
  }
  const t = vDot(vSub(planePoint, ray.origin), normal) / denom;
  if (!Number.isFinite(t)) {
    return null;
  }
  return vAdd(ray.origin, vScale(ray.dir, t));
}
