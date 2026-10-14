// A drag controller for the 3D pointer — a virtual marker the user places and then refines with the
// existing translate gizmo, but which is NOT a scene entity. It reuses `drag.ts`'s pure translate-axis
// / translate-plane solver (`beginDrag`/`updateDrag`) directly, unlike `GizmoController` (controller.ts)
// which additionally owns protocol sends, preview throttling, and undo entries for an ENTITY's
// `Transform` — none of which apply here (the marker has no entity, no undo history, and nothing to
// throttle over the wire). This module is the marker's whole policy: begin/advance/commit/cancel a
// translate-only drag over a plain `Vec3` position.

import { beginDrag, updateDrag, type DragSession, type SnapSettings } from '../gizmo/drag';
import { buildGizmo, type GizmoLayout } from '../gizmo/model';
import type { CameraKit, Transform, Viewport } from '../gizmo/camera';
import type { HandleId } from '../gizmo/model';
import type { Point } from '../../viewer/picking';
import type { Vec3 } from '../gizmo/vecmath';

/** The marker's `Transform` for gizmo purposes: identity rotation, unit scale, position as translation. */
export function markerTransform(position: Vec3): Transform {
  return { translation: position, rotation: [0, 0, 0], scale: [1, 1, 1] };
}

/** The projected translate gizmo for the marker at `position`, or `null` per `buildGizmo`'s own rules
 *  (origin behind the camera, or too degenerate to size handles). Always mode `'translate'` — the
 *  marker has no rotation or scale to manipulate. */
export function buildMarkerGizmo(position: Vec3, kit: CameraKit, size: Viewport): GizmoLayout | null {
  return buildGizmo(markerTransform(position), kit, size, 'translate');
}

/**
 * A marker drag in progress, driven by pointer events. Thin wrapper over a `drag.ts` `DragSession`
 * restricted to translate handles (the only mode `buildMarkerGizmo` ever produces) — no protocol
 * sends, no throttling, no undo entry; the caller decides what to do with the returned position.
 */
export class MarkerDragController {
  private session: DragSession | null = null;

  get dragging(): boolean {
    return this.session !== null;
  }

  /**
   * Begins a drag on `handleId` at the marker's current `basePos`. Returns `false` when the drag
   * cannot start (`beginDrag`'s degenerate pick) — the session stays untouched — and `true` once a
   * drag is active.
   */
  pointerDown(handleId: HandleId, basePos: Vec3, point: Point, kit: CameraKit, size: Viewport): boolean {
    const session = beginDrag(handleId, 'translate', markerTransform(basePos), point, kit, size);
    if (session === null) {
      return false;
    }
    this.session = session;
    return true;
  }

  /** Advances the drag, returning the previewed marker position, or `null` when no drag is active. */
  pointerMove(point: Point, snap: SnapSettings): Vec3 | null {
    if (this.session === null) {
      return null;
    }
    return updateDrag(this.session, point, snap).translation;
  }

  /** Commits the drag to its last previewed position, or `null` when no drag is active. Clears the session. */
  pointerUp(): Vec3 | null {
    const session = this.session;
    if (session === null) {
      return null;
    }
    this.session = null;
    return session.lastValue.translation;
  }

  /** Cancels the drag, returning the base position to restore, or `null` when no drag is active. Clears the session. */
  escape(): Vec3 | null {
    const session = this.session;
    if (session === null) {
      return null;
    }
    this.session = null;
    return session.base.translation;
  }

  /** Abandons any in-flight drag without returning anything. */
  reset(): void {
    this.session = null;
  }
}
