// The drag-orchestration policy for the transform gizmos: a pure, DOM-free state machine wrapping a
// `drag.ts` `DragSession` with the wiring concerns the React layer would otherwise tangle up —
// preview throttling, always-send-the-final-value, one-undo-entry-per-drag, no-op suppression, and
// click suppression. The component translates DOM pointer events into these four calls and turns the
// returned instructions into protocol calls; everything policy-shaped is decided (and tested) here,
// so jsdom never enters the picture (see controller.test.ts).
//
// Two send cadences flow through `send`: throttled previews during the drag (latest-wins, ~15 Hz via
// `shouldSendUpdate`) and the exact final value at release (unthrottled, so the last pointer position
// is never dropped). `preview` is separate — it is the current pose on every move, used to re-derive
// the overlay geometry locally so the handles track the pointer without waiting for a poll.
//
// A parented entity's `Transform` is local to its parent, but the gizmo sits at the entity's WORLD
// pose: `send`/`entry` stay local, and every instruction carrying `preview` or `send` also carries
// `world`, the pose to draw the gizmo at (for a root, the same value).

import { endDrag, beginDrag, dragWorld, shouldSendUpdate, updateDrag, type DragSession, type GizmoFrame, type SnapSettings } from './drag';
import type { CameraKit, Transform, Viewport } from './camera';
import type { GizmoMode, HandleId } from './model';
import type { Point } from '../../viewer/picking';
import type { EntityId, UndoEntry } from '../undo';
import type { Vec3 } from './vecmath';

/** ~15 Hz: the minimum gap between preview `component.set`s during a drag (plan decision #7). */
export const PREVIEW_INTERVAL_MS = 66;

/**
 * What the React layer should do after one event. All fields optional:
 * - `preview` — the current pose (present on every move of a live drag); re-derive the overlay from it.
 * - `send` — a `component.set` to issue (throttled on move, the exact final value on release/cancel).
 * - `world` — the world pose to draw the gizmo at, alongside every `preview` or `send`.
 * - `entry` — the single undo entry, present only at a release that actually changed the transform.
 * - `suppressClick` — set on a pointerDown that captured a handle, so the pick click is swallowed.
 * - `done` — the gesture ended (release or cancel): release pointer capture and let the poll resume.
 */
export interface DragInstruction {
  readonly preview?: Transform;
  readonly send?: Transform;
  readonly world?: Transform;
  readonly entry?: UndoEntry;
  readonly suppressClick?: boolean;
  readonly done?: boolean;
}

interface DragState {
  readonly session: DragSession;
  readonly base: Transform;
  readonly entity: EntityId;
  readonly frame: GizmoFrame | null;
  /** `shouldSendUpdate` clock; `-Infinity` so the first changed move always sends. */
  lastSentMs: number;
  /** Whether any preview set has gone out — a cancel still leaves the scene dirty (decision #7). */
  dirtied: boolean;
}

function vec3Equals(a: Vec3, b: Vec3): boolean {
  return a[0] === b[0] && a[1] === b[1] && a[2] === b[2];
}

function transformEquals(a: Transform, b: Transform): boolean {
  return vec3Equals(a.translation, b.translation) && vec3Equals(a.rotation, b.rotation) && vec3Equals(a.scale, b.scale);
}

/** A gizmo drag in progress, driven by pointer events. One instance lives for the editor's lifetime. */
export class GizmoController {
  private active: DragState | null = null;
  private readonly intervalMs: number;

  constructor(intervalMs = PREVIEW_INTERVAL_MS) {
    this.intervalMs = intervalMs;
  }

  get dragging(): boolean {
    return this.active !== null;
  }

  /**
   * Begins a drag on the handle under the pointer, or returns `null` when no drag starts — either the
   * pick is degenerate (`beginDrag` fails) so the caller should fall through to click-picking. On a
   * successful start the pick click is suppressed.
   */
  pointerDown(
    handleId: HandleId,
    mode: GizmoMode,
    base: Transform,
    entity: EntityId,
    point: Point,
    kit: CameraKit,
    viewport: Viewport,
    frame: GizmoFrame | null = null,
  ): DragInstruction | null {
    const session = beginDrag(handleId, mode, base, point, kit, viewport, frame);
    if (session === null) {
      return null;
    }
    this.active = { session, base, entity, frame, lastSentMs: Number.NEGATIVE_INFINITY, dirtied: false };
    return { suppressClick: true };
  }

  /** Advances the drag: always yields a `preview`; yields a throttled `send` when the pose changed. */
  pointerMove(point: Point, nowMs: number, snap: SnapSettings): DragInstruction {
    const a = this.active;
    if (a === null) {
      return {};
    }
    const preview = updateDrag(a.session, point, snap);
    const world = dragWorld(a.session);
    // Skip sends only while the pose has never left base; once any preview went out, a move back
    // exactly onto base must still send, or the engine would be left at the last preview.
    if ((transformEquals(preview, a.base) && !a.dirtied) || !shouldSendUpdate(a.lastSentMs, nowMs, this.intervalMs)) {
      return { preview, world };
    }
    a.lastSentMs = nowMs;
    a.dirtied = true;
    return { preview, send: preview, world };
  }

  /**
   * Ends the drag. Sends the exact final value and records one undo entry — unless the pose never
   * left its mousedown value, in which case nothing is sent and nothing is recorded.
   */
  pointerUp(): DragInstruction {
    const a = this.active;
    if (a === null) {
      return {};
    }
    this.active = null;
    const { before, after } = endDrag(a.session, a.session.lastValue);
    if (transformEquals(after, before)) {
      // The pose ended where it started, but previews may still have gone out (drag away and
      // back): the engine must be restored to base. No undo entry either way.
      return a.dirtied ? { send: after, world: dragWorld(a.session), done: true } : { done: true };
    }
    return {
      send: after,
      world: dragWorld(a.session),
      entry: { kind: 'set', entity: a.entity, component: 'Transform', before, after },
      done: true,
    };
  }

  /**
   * Cancels the drag: restores the mousedown value (one final `component.set`, no undo entry) if any
   * preview went out. The scene stays dirty on purpose — the engine did record real sets (decision #7).
   */
  escape(): DragInstruction {
    const a = this.active;
    if (a === null) {
      return {};
    }
    this.active = null;
    return a.dirtied ? { send: a.base, world: a.frame?.world ?? a.base, done: true } : { done: true };
  }

  /** Abandons any in-flight drag without emitting anything (mode/selection changed out from under it). */
  reset(): void {
    this.active = null;
  }
}
