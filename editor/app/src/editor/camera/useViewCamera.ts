// The one camera every editor view mounts: the UE5 gestures of `gestures.ts` driving the pure math
// of `fly.ts` and `voxel/orbit.ts`, over the `render.view` host override
// (`Plan/editor-stage-camera.md` decisions 2, 4, 5, 7).
//
// ONE POSE, TWO RIGS, NO CONVERSION. `CameraPose` is the single piece of camera state this hook
// owns; the fly rig and the orbit rig both consume and produce one, so a fly that ends and an
// Alt-orbit that starts hand the same numbers across untouched. An orbit gesture builds its
// `OrbitState` from that pose and a pivot AT THE MOMENT THE BUTTON GOES DOWN and throws it away on
// release — keeping a long-lived orbit state beside the pose is what produces the jump, because the
// two then disagree about where the camera is.
//
// THE OVERRIDE IS HOST STATE, NOT WORLD STATE. `render.view` moves the stream and nothing else: no
// `component.set` on a scene camera, so no dirtied session, no `scene.save` capture, no `WorldHash`
// move, and nothing for a game's own camera system to fight over. `{"pose": null}` hands the stream
// back to the scene's `Camera`, which is exactly what `Game` mode is.
//
// WRITES RIDE THE DISPLAY, ONE CALL IN FLIGHT. A `requestAnimationFrame` loop integrates held keys
// with a real `dt` and then offers the latest pose; a pose offered while the previous `render.view`
// is unanswered simply replaces the one waiting (`viewer/framePump.ts`'s shape — here the parked
// value is the desired pose itself, so at most one is ever queued and it is always the newest).
//
// THE VIEW SUPPLIES THE WORLD. Wire access, what "the selection" is, and where `Home` points all
// arrive as props: three views mount this and it knows which one it is only by its `viewId`, which
// is half of the `localStorage` key its pose is kept under.

import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
  WheelEvent as ReactWheelEvent,
} from 'react';
import type { ViewPose } from '@ludion/protocol';
import type { ProtocolClient } from '../../protocol/client';
import { toDiagnostic, type DiagnosticView } from '../../viewer/diagnostic';
import { eulerXyzDegToQuat } from '../gizmo/camera';
import {
  mat4FromRotationTranslation,
  vAdd,
  vNormalize,
  vScale,
  vSub,
  type Vec3,
} from '../gizmo/vecmath';
import {
  DEFAULT_FLY_SPEED,
  adjustFlySpeed,
  adjustFov,
  advance,
  currentFovYDeg,
  lookDrag,
  resetFov,
  type FlyState,
} from './fly';
import {
  STAGE_FOV_Y_DEG,
  dollyBy,
  fitDistance,
  lookAtPose,
  orbitDrag,
  orbitFromDirection,
  orbitPose,
  panDrag,
  type CameraPose,
  type OrbitState,
  type Sphere,
  type Viewport,
} from '../voxel/orbit';
import {
  NO_FLY_KEYS,
  flyAction,
  flyKeysFrom,
  fovDirection,
  isCameraDrag,
  resolveDrag,
  resolveWheel,
  viewCommand,
  type DragGesture,
} from './gestures';

/** Which camera the stream is showing. `game` is the scene's own `Camera` — the override cleared. */
export type ViewCameraMode = 'view' | 'game';

/** Dolly notches per pixel of an Alt+RMB drag. Dragging down pulls back, which is the sense of a
 *  wheel notch rolled toward the hand. */
const DOLLY_STEPS_PER_PIXEL = 0.01;

/** How far ahead the pivot sits when nothing is selected — "the point ahead of the camera", in world
 *  units. Replaced by the real distance the moment a gesture frames or orbits something. */
const DEFAULT_PIVOT_DISTANCE = 5;

/** A frame longer than this contributed no held-key movement: it is a tab that was backgrounded, a
 *  stalled main thread, or a breakpoint, and integrating it would teleport the camera. */
const MAX_FRAME_SECONDS = 0.1;

/** Where a view opens when nothing is stored and nothing is selectable — a pose that can see a
 *  subject standing at the origin, so the first `G` is never into the void. */
export const DEFAULT_VIEW_POSE: CameraPose = lookAtPose([0, 2, 6], [0, 1, 0], [0, 1, 0]);

const INITIAL_FLY: FlyState = {
  keys: NO_FLY_KEYS,
  shift: false,
  speedMetersPerSecond: DEFAULT_FLY_SPEED,
  fovDeltaDeg: 0,
};

// --- Persistence (pure) ------------------------------------------------------------------------ //

/** What is kept per view and scene: where the human left the camera, and which camera was showing. */
export interface StoredView {
  readonly pose: CameraPose;
  readonly mode: ViewCameraMode;
}

/** The `localStorage` namespace. A per-machine convenience — never project data, never committed. */
export const VIEW_CAMERA_KEY_PREFIX = 'ludion.editor.viewCamera';

/** The key a view's pose is kept under. Separators in the scene path are folded so a Windows path
 *  and a POSIX one name the same entry. */
export function viewCameraStorageKey(viewId: string, scenePath: string): string {
  return `${VIEW_CAMERA_KEY_PREFIX}.${viewId}.${scenePath.replace(/\\/g, '/')}`;
}

export function serializeStoredView(view: StoredView): string {
  return JSON.stringify({ pose: { translation: view.pose.translation, rotation: view.pose.rotation }, mode: view.mode });
}

function readVec3(value: unknown): Vec3 | null {
  if (!Array.isArray(value) || value.length !== 3) {
    return null;
  }
  const numbers = value.map((entry) => (typeof entry === 'number' && Number.isFinite(entry) ? entry : NaN));
  return numbers.some(Number.isNaN) ? null : [numbers[0] as number, numbers[1] as number, numbers[2] as number];
}

/**
 * The stored view, or `null` for anything this hook did not write — an entry from an older shape, a
 * hand-edited one, a truncated one. A bad entry opens the view on `Game` rather than wedging it at a
 * NaN pose, which no gesture can recover from.
 */
export function parseStoredView(raw: string | null): StoredView | null {
  if (raw === null) {
    return null;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof parsed !== 'object' || parsed === null) {
    return null;
  }
  const { pose, mode } = parsed as { pose?: unknown; mode?: unknown };
  if (typeof pose !== 'object' || pose === null || (mode !== 'view' && mode !== 'game')) {
    return null;
  }
  const { translation, rotation } = pose as { translation?: unknown; rotation?: unknown };
  const t = readVec3(translation);
  const r = readVec3(rotation);
  return t === null || r === null ? null : { pose: { translation: t, rotation: r }, mode };
}

// --- The wire payload (pure) ------------------------------------------------------------------- //

/**
 * The `render.view` pose for a camera state.
 *
 * `fov_y_deg` is sent only when the view named a base FOV or the human is holding Z/C: with neither,
 * the override keeps the scene camera's own field of view, and a view that never passes a base never
 * silently re-frames the game.
 */
export function viewPoseOf(pose: CameraPose, fly: FlyState, baseFovYDeg?: number): ViewPose {
  const translation: [number, number, number] = [pose.translation[0], pose.translation[1], pose.translation[2]];
  const rotation: [number, number, number] = [pose.rotation[0], pose.rotation[1], pose.rotation[2]];
  if (baseFovYDeg === undefined && fly.fovDeltaDeg === 0) {
    return { translation, rotation };
  }
  return { translation, rotation, fov_y_deg: currentFovYDeg(fly, baseFovYDeg ?? STAGE_FOV_Y_DEG) };
}

// --- The hook ---------------------------------------------------------------------------------- //

export interface ViewCameraOptions {
  /** The view's session, or `null` while it is not dialed — gestures still move the pose, and the
   *  first pose after a client arrives is sent. */
  readonly client: ProtocolClient | null;
  /** The mounting view's id — half the storage key, and the reason this hook needs no other way of
   *  telling which view it is in. */
  readonly viewId: string;
  /** The scene the pose belongs to: the other half of the key. `null` disables persistence. */
  readonly scenePath: string | null;
  /** The canvas's CSS size — what pan and framing project through. */
  readonly viewport: Viewport;
  /** Whether a plain left drag orbits (a stage) or belongs to the view (pick, marquee, gizmo). */
  readonly plainLeftOrbits: boolean;
  /** The selection's world bounds: the orbit pivot and what `F` frames. `null` means nothing is
   *  selected, so the pivot falls to the point ahead of the camera and `F` does nothing. */
  readonly selectionBounds?: () => Sphere | null;
  /** Where `Home` goes — a stage's committed home pose. With none, `Home` re-frames the selection. */
  readonly homePose?: () => CameraPose | null;
  /** The scene camera's vertical FOV, when the view knows it. Passing it makes Z/C continuous from
   *  what is on screen; leaving it out keeps the scene camera's FOV until Z or C is held. */
  readonly baseFovYDeg?: number;
  /** Forces `Game` while true and restores the view camera when it clears — Play mode. */
  readonly forceGame?: boolean;
  /** `false` detaches the keys and the send loop: a view that is mounted but not on screen. */
  readonly enabled?: boolean;
}

/** The pointer props a view spreads onto its viewport element. `onPointerDown` answers whether the
 *  camera took the gesture, so a viewport with its own left-drag meaning can call it first and
 *  proceed when it returns `false`. */
export interface ViewCameraHandlers {
  readonly onPointerDown: (event: ReactPointerEvent<HTMLElement>) => boolean;
  readonly onPointerMove: (event: ReactPointerEvent<HTMLElement>) => void;
  readonly onPointerUp: (event: ReactPointerEvent<HTMLElement>) => void;
  readonly onPointerCancel: (event: ReactPointerEvent<HTMLElement>) => void;
  readonly onWheel: (event: ReactWheelEvent<HTMLElement>) => void;
  readonly onContextMenu: (event: ReactMouseEvent<HTMLElement>) => void;
}

export interface ViewCamera {
  /** Which camera the stream is showing, Play mode's force included. */
  readonly mode: ViewCameraMode;
  /** The view camera's pose, whichever mode is showing — `Game` keeps it for the flip back. */
  readonly pose: CameraPose;
  /** The FOV the override is asking for, for a readout. */
  readonly fovYDeg: number;
  /** Whether the look is held (RMB down): the state the movement keys are live in. */
  readonly flying: boolean;
  /** The wheel-adjusted fly speed, in world units per second. */
  readonly flySpeed: number;
  /** A failed `render.view`, for the view's banner. */
  readonly error: DiagnosticView | null;
  readonly handlers: ViewCameraHandlers;
  readonly setMode: (mode: ViewCameraMode) => void;
  /** The `G` key and the viewport chip. */
  readonly toggleMode: () => void;
  /** `F` — frame the selection from where the camera stands. A no-op with nothing selected. */
  readonly frameSelection: () => void;
  /** `Home` — the view's home pose, or the framed selection when it has none. */
  readonly goHome: () => void;
  /** Put the camera somewhere outright: the stage preset buttons. */
  readonly goToPose: (pose: CameraPose) => void;
  readonly dismissError: () => void;
}

function forwardOf(pose: CameraPose): Vec3 {
  // `fly.ts` keeps its own basis private and this needs only the view ray, so the one matrix column
  // is read here rather than widening that module's surface.
  const model = mat4FromRotationTranslation(eulerXyzDegToQuat(pose.rotation), [0, 0, 0]);
  return vNormalize([-model[8], -model[9], -model[10]]);
}

function posesEqual(a: CameraPose, b: CameraPose): boolean {
  return (
    a.translation[0] === b.translation[0] &&
    a.translation[1] === b.translation[1] &&
    a.translation[2] === b.translation[2] &&
    a.rotation[0] === b.rotation[0] &&
    a.rotation[1] === b.rotation[1] &&
    a.rotation[2] === b.rotation[2]
  );
}

/** Whether a key event belongs to something the human is typing in. The camera's keys are single
 *  letters, so a viewport listening on `window` would otherwise eat every `w` of a filename. */
function isEditableTarget(target: EventTarget | null): boolean {
  if (target === null || typeof HTMLElement === 'undefined' || !(target instanceof HTMLElement)) {
    return false;
  }
  return (
    target.isContentEditable ||
    target.tagName === 'INPUT' ||
    target.tagName === 'TEXTAREA' ||
    target.tagName === 'SELECT'
  );
}

function readStorage(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStorage(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // A webview with storage disabled loses the convenience and nothing else.
  }
}

/** One drag in flight. `orbit` is rebuilt at every button change, which is what keeps the pivot
 *  gestures and the pose from drifting apart. */
interface DragSession {
  gesture: Exclude<DragGesture, 'pick' | 'none'>;
  readonly pointerId: number;
  readonly element: HTMLElement;
  x: number;
  y: number;
  orbit: OrbitState | null;
  locked: boolean;
}

export function useViewCamera(options: ViewCameraOptions): ViewCamera {
  const {
    client,
    viewId,
    scenePath,
    viewport,
    plainLeftOrbits,
    selectionBounds,
    homePose,
    baseFovYDeg,
    forceGame = false,
    enabled = true,
  } = options;

  const [pose, setPose] = useState<CameraPose>(DEFAULT_VIEW_POSE);
  const [mode, setModeState] = useState<ViewCameraMode>('game');
  const [fly, setFly] = useState<FlyState>(INITIAL_FLY);
  const [flying, setFlying] = useState(false);
  const [error, setError] = useState<DiagnosticView | null>(null);
  /** Bumped when the stored pose has been read, so the send effect below never fires against the
   *  pre-restore state and flash the game camera at a view that opens flown. */
  const [restored, setRestored] = useState(0);

  // Everything the event handlers and the rAF loop read lives in a ref as well: they run between
  // renders, where a captured value is a frame stale — and a stale pose is a visible jump.
  const poseRef = useRef(pose);
  const flyRef = useRef(fly);
  const flyingRef = useRef(false);
  const modeRef = useRef<ViewCameraMode>(forceGame ? 'game' : mode);
  const enabledRef = useRef(enabled);
  const clientRef = useRef(client);
  const viewportRef = useRef(viewport);
  const errorRef = useRef<DiagnosticView | null>(null);
  const baseFovRef = useRef(baseFovYDeg);
  const plainLeftOrbitsRef = useRef(plainLeftOrbits);
  const selectionBoundsRef = useRef(selectionBounds);
  const homePoseRef = useRef(homePose);
  const storageKeyRef = useRef<string | null>(null);
  /** Whether the pose on hand is one the human (or a restore) put there, rather than the default. */
  const posedRef = useRef(false);
  const pivotDistanceRef = useRef(DEFAULT_PIVOT_DISTANCE);
  const dragRef = useRef<DragSession | null>(null);
  const heldRef = useRef<Set<string>>(new Set());
  const shiftRef = useRef(false);
  /** The latest-wins park: `undefined` is nothing owed, `null` is "clear the override". */
  const desiredRef = useRef<ViewPose | null | undefined>(undefined);
  const inFlightRef = useRef(false);
  const loadedRef = useRef(false);

  const effectiveMode: ViewCameraMode = forceGame ? 'game' : mode;
  modeRef.current = effectiveMode;
  enabledRef.current = enabled;
  clientRef.current = client;
  viewportRef.current = viewport;
  baseFovRef.current = baseFovYDeg;
  plainLeftOrbitsRef.current = plainLeftOrbits;
  selectionBoundsRef.current = selectionBounds;
  homePoseRef.current = homePose;
  storageKeyRef.current = scenePath === null ? null : viewCameraStorageKey(viewId, scenePath);

  // --- the send loop --------------------------------------------------------------------------- //

  const pump = useCallback((): void => {
    const target = desiredRef.current;
    const session = clientRef.current;
    if (target === undefined || inFlightRef.current || session === null) {
      return;
    }
    desiredRef.current = undefined;
    inFlightRef.current = true;
    void session
      .call({ method: 'render.view', params: { pose: target } })
      .then(() => {
        if (errorRef.current !== null) {
          errorRef.current = null;
          setError(null);
        }
      })
      .catch((err: unknown) => {
        errorRef.current = toDiagnostic(err);
        setError(errorRef.current);
      })
      .finally(() => {
        inFlightRef.current = false;
        pump();
      });
  }, []);

  /** Offers the current camera state to the send loop. In `Game` mode the override is the human's
   *  absence, so nothing a gesture does is offered. */
  const markDirty = useCallback((): void => {
    if (modeRef.current === 'view') {
      desiredRef.current = viewPoseOf(poseRef.current, flyRef.current, baseFovRef.current);
    }
  }, []);

  // --- committing state ------------------------------------------------------------------------ //

  const commitPose = useCallback(
    (next: CameraPose): void => {
      posedRef.current = true;
      if (posesEqual(poseRef.current, next)) {
        return;
      }
      poseRef.current = next;
      setPose(next);
      markDirty();
    },
    [markDirty],
  );

  const commitFly = useCallback(
    (next: FlyState): void => {
      const previous = flyRef.current;
      flyRef.current = next;
      // Held keys change every frame and nothing renders them; only the two values a readout shows —
      // and that the override's FOV depends on — are worth a render.
      if (
        previous.speedMetersPerSecond !== next.speedMetersPerSecond ||
        previous.fovDeltaDeg !== next.fovDeltaDeg
      ) {
        setFly(next);
        markDirty();
      }
    },
    [markDirty],
  );

  /** Writes where the human left the camera — at the end of a gesture, a wheel notch or a mode
   *  change, never per frame: a `localStorage` write inside the fly loop is a synchronous disk touch
   *  60 times a second, and the pose mid-drag is not one anybody chose. */
  const persist = useCallback((at: ViewCameraMode): void => {
    const key = storageKeyRef.current;
    if (key !== null && posedRef.current) {
      writeStorage(key, serializeStoredView({ pose: poseRef.current, mode: at }));
    }
  }, []);

  // --- the pivot ------------------------------------------------------------------------------- //

  /** The orbit state a pivot gesture starts from: around the selection's centre when there is one,
   *  else around a point ahead of the camera at the last distance a gesture established. */
  const pivotOrbit = useCallback((): OrbitState => {
    const at = poseRef.current;
    const sphere = selectionBoundsRef.current?.() ?? null;
    if (sphere === null) {
      const centre = vAdd(at.translation, vScale(forwardOf(at), pivotDistanceRef.current));
      return orbitFromDirection(vSub(at.translation, centre), centre, pivotDistanceRef.current);
    }
    const away = vSub(at.translation, sphere.center);
    return orbitFromDirection(away, sphere.center, Math.hypot(away[0], away[1], away[2]));
  }, []);

  const applyOrbit = useCallback(
    (state: OrbitState): void => {
      pivotDistanceRef.current = state.distance;
      commitPose(orbitPose(state));
    },
    [commitPose],
  );

  // --- the commands ---------------------------------------------------------------------------- //

  const fovNow = useCallback(
    (): number => currentFovYDeg(flyRef.current, baseFovRef.current ?? STAGE_FOV_Y_DEG),
    [],
  );

  /** The pose that frames the selection from where the camera stands, or `null` with none. */
  const framedPose = useCallback((): CameraPose | null => {
    const sphere = selectionBoundsRef.current?.() ?? null;
    if (sphere === null) {
      return null;
    }
    const distance = fitDistance(sphere.radius, viewportRef.current, fovNow());
    const state = orbitFromDirection(vSub(poseRef.current.translation, sphere.center), sphere.center, distance);
    pivotDistanceRef.current = state.distance;
    return orbitPose(state);
  }, [fovNow]);

  const setMode = useCallback(
    (next: ViewCameraMode): void => {
      if (next === 'view' && !posedRef.current) {
        // Nothing was ever stored or framed here; arriving at the default pose would be arriving
        // nowhere, so the first flip lands on the same pose `Home` gives.
        commitPose(homePoseRef.current?.() ?? framedPose() ?? DEFAULT_VIEW_POSE);
      }
      setModeState(next);
      persist(next);
    },
    [commitPose, framedPose, persist],
  );

  const toggleMode = useCallback((): void => {
    setMode(modeRef.current === 'view' ? 'game' : 'view');
  }, [setMode]);

  const frameSelection = useCallback((): void => {
    const next = framedPose();
    if (next === null) {
      return;
    }
    commitPose(next);
    // Framing is an instruction to look at something, so it is also a request to be in the camera
    // that can.
    setMode('view');
  }, [commitPose, framedPose, setMode]);

  const goHome = useCallback((): void => {
    const next = homePoseRef.current?.() ?? framedPose();
    if (next === null) {
      return;
    }
    commitPose(next);
    setMode('view');
  }, [commitPose, framedPose, setMode]);

  const goToPose = useCallback(
    (next: CameraPose): void => {
      commitPose(next);
      persist(modeRef.current);
    },
    [commitPose, persist],
  );

  // --- pointer gestures ------------------------------------------------------------------------ //

  const endFly = useCallback(
    (session: DragSession): void => {
      flyingRef.current = false;
      setFlying(false);
      heldRef.current.clear();
      commitFly(resetFov({ ...flyRef.current, keys: NO_FLY_KEYS, shift: false }));
      if (session.locked) {
        session.locked = false;
        if (typeof document !== 'undefined' && document.pointerLockElement === session.element) {
          document.exitPointerLock();
        }
      }
    },
    [commitFly],
  );

  const startFly = useCallback((session: DragSession): void => {
    flyingRef.current = true;
    setFlying(true);
    // Pointer lock keeps a long look from running out of screen; it is a request, not a guarantee
    // (a webview can refuse one outside a user gesture), and the drag still works through pointer
    // capture and client deltas when it is refused.
    const requested: unknown = session.element.requestPointerLock?.();
    session.locked = true;
    if (requested instanceof Promise) {
      requested.catch(() => {
        session.locked = false;
      });
    }
  }, []);

  const switchGesture = useCallback(
    (session: DragSession, next: Exclude<DragGesture, 'pick' | 'none'>): void => {
      if (session.gesture === next) {
        return;
      }
      if (session.gesture === 'fly-look') {
        endFly(session);
      }
      session.gesture = next;
      session.orbit = next === 'fly-look' ? null : pivotOrbit();
      if (next === 'fly-look') {
        startFly(session);
      }
    },
    [endFly, pivotOrbit, startFly],
  );

  const endDrag = useCallback((): void => {
    const session = dragRef.current;
    if (session === null) {
      return;
    }
    dragRef.current = null;
    if (session.gesture === 'fly-look') {
      endFly(session);
    }
    if (session.element.hasPointerCapture(session.pointerId)) {
      session.element.releasePointerCapture(session.pointerId);
    }
    persist(modeRef.current);
  }, [endFly, persist]);

  const onPointerDown = useCallback(
    (event: ReactPointerEvent<HTMLElement>): boolean => {
      if (!enabledRef.current || modeRef.current !== 'view') {
        return false;
      }
      const gesture = resolveDrag({
        buttons: event.buttons,
        alt: event.altKey,
        plainLeftOrbits: plainLeftOrbitsRef.current,
      });
      if (!isCameraDrag(gesture)) {
        return false;
      }
      const taken = gesture as Exclude<DragGesture, 'pick' | 'none'>;
      const current = dragRef.current;
      if (current !== null && current.pointerId === event.pointerId) {
        // A second button pressed mid-drag re-reads the table: LMB then RMB becomes a pan.
        switchGesture(current, taken);
        current.x = event.clientX;
        current.y = event.clientY;
      } else {
        const element = event.currentTarget;
        const session: DragSession = {
          gesture: taken,
          pointerId: event.pointerId,
          element,
          x: event.clientX,
          y: event.clientY,
          orbit: taken === 'fly-look' ? null : pivotOrbit(),
          locked: false,
        };
        dragRef.current = session;
        element.setPointerCapture(event.pointerId);
        if (taken === 'fly-look') {
          startFly(session);
        }
      }
      event.preventDefault();
      return true;
    },
    [pivotOrbit, startFly, switchGesture],
  );

  const onPointerMove = useCallback(
    (event: ReactPointerEvent<HTMLElement>): void => {
      const session = dragRef.current;
      if (session === null) {
        return;
      }
      // Under pointer lock the cursor does not move, so `clientX` stops changing and only the raw
      // movement deltas carry the drag.
      const locked =
        session.locked && typeof document !== 'undefined' && document.pointerLockElement === session.element;
      const dx = locked ? event.movementX : event.clientX - session.x;
      const dy = locked ? event.movementY : event.clientY - session.y;
      session.x = event.clientX;
      session.y = event.clientY;
      if (dx === 0 && dy === 0) {
        return;
      }
      if (session.gesture === 'fly-look') {
        commitPose(lookDrag(poseRef.current, dx, dy));
        return;
      }
      const state = session.orbit;
      if (state === null) {
        return;
      }
      const next =
        session.gesture === 'orbit'
          ? orbitDrag(state, dx, dy)
          : session.gesture === 'pan'
            ? panDrag(state, dx, dy, viewportRef.current, fovNow())
            : dollyBy(state, dy * DOLLY_STEPS_PER_PIXEL);
      session.orbit = next;
      applyOrbit(next);
    },
    [applyOrbit, commitPose, fovNow],
  );

  const onPointerUp = useCallback(
    (event: ReactPointerEvent<HTMLElement>): void => {
      const session = dragRef.current;
      if (session === null) {
        return;
      }
      // Buttons still held after this release keep the drag alive under whatever they now mean —
      // releasing LMB out of an LMB+RMB pan drops straight into the look, as UE does.
      const gesture = resolveDrag({
        buttons: event.buttons,
        alt: event.altKey,
        plainLeftOrbits: plainLeftOrbitsRef.current,
      });
      if (isCameraDrag(gesture)) {
        switchGesture(session, gesture as Exclude<DragGesture, 'pick' | 'none'>);
        session.x = event.clientX;
        session.y = event.clientY;
        return;
      }
      endDrag();
    },
    [endDrag, switchGesture],
  );

  const onPointerCancel = useCallback((): void => endDrag(), [endDrag]);

  const onWheel = useCallback(
    (event: ReactWheelEvent<HTMLElement>): void => {
      if (!enabledRef.current || modeRef.current !== 'view') {
        return;
      }
      const resolution = resolveWheel(event.deltaY, flyingRef.current);
      if (resolution.steps === 0) {
        return;
      }
      if (resolution.kind === 'fly-speed') {
        commitFly(adjustFlySpeed(flyRef.current, resolution.steps));
        return;
      }
      const session = dragRef.current;
      const next = dollyBy(session?.orbit ?? pivotOrbit(), resolution.steps);
      if (session !== null) {
        session.orbit = next;
      }
      applyOrbit(next);
      persist(modeRef.current);
    },
    [applyOrbit, commitFly, persist, pivotOrbit],
  );

  const onContextMenu = useCallback((event: ReactMouseEvent<HTMLElement>): void => {
    // The look is a held right button; the menu it would otherwise raise ends the drag on the first
    // frame of every fly.
    if (enabledRef.current && modeRef.current === 'view') {
      event.preventDefault();
    }
  }, []);

  // --- keys ------------------------------------------------------------------------------------ //

  useEffect(() => {
    if (!enabled) {
      return;
    }
    const held = heldRef.current;
    const onKeyDown = (event: KeyboardEvent): void => {
      if (isEditableTarget(event.target)) {
        return;
      }
      shiftRef.current = event.shiftKey;
      const command = viewCommand(event.code);
      if (command !== null && !event.altKey && !event.ctrlKey && !event.metaKey) {
        event.preventDefault();
        if (command === 'frame') {
          frameSelection();
        } else if (command === 'home') {
          goHome();
        } else {
          toggleMode();
        }
        return;
      }
      // Movement is live only under a held look, which is what keeps W and G from being the same
      // kind of key: one drives a gesture in flight, the other is a standing command.
      if (flyingRef.current && flyAction(event.code) !== null) {
        held.add(event.code);
        event.preventDefault();
      }
    };
    const onKeyUp = (event: KeyboardEvent): void => {
      shiftRef.current = event.shiftKey;
      held.delete(event.code);
    };
    // A key released while another window has focus never arrives, and a key believed held forever
    // is a camera that flies on its own.
    const onBlur = (): void => {
      held.clear();
      shiftRef.current = false;
    };
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('keyup', onKeyUp);
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('keyup', onKeyUp);
      window.removeEventListener('blur', onBlur);
      held.clear();
    };
  }, [enabled, frameSelection, goHome, toggleMode]);

  // --- the frame loop -------------------------------------------------------------------------- //

  useEffect(() => {
    if (!enabled || effectiveMode !== 'view') {
      return;
    }
    let handle = 0;
    let last = performance.now();
    const tick = (now: number): void => {
      handle = requestAnimationFrame(tick);
      const dt = Math.min(Math.max(now - last, 0) / 1000, MAX_FRAME_SECONDS);
      last = now;
      if (flyingRef.current) {
        const held = heldRef.current;
        commitFly(
          adjustFov(
            { ...flyRef.current, keys: flyKeysFrom(held), shift: shiftRef.current },
            fovDirection(held),
            dt,
          ),
        );
        commitPose(advance(poseRef.current, flyRef.current, dt));
      }
      pump();
    };
    handle = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(handle);
  }, [enabled, effectiveMode, commitFly, commitPose, pump]);

  // --- restore, and the mode's own write ------------------------------------------------------- //

  useEffect(() => {
    const key = scenePath === null ? null : viewCameraStorageKey(viewId, scenePath);
    const stored = key === null ? null : parseStoredView(readStorage(key));
    if (stored === null) {
      posedRef.current = false;
      setModeState('game');
    } else {
      poseRef.current = stored.pose;
      posedRef.current = true;
      setPose(stored.pose);
      setModeState(stored.mode);
    }
    loadedRef.current = true;
    setRestored((n) => n + 1);
  }, [viewId, scenePath]);

  useEffect(() => {
    if (!enabled || !loadedRef.current) {
      return;
    }
    // A mode change, a re-dialed client and a restore all mean the same thing: say what this view's
    // camera is, now. `Game` is the override cleared, and clearing one that is not set is free.
    desiredRef.current =
      effectiveMode === 'game' ? null : viewPoseOf(poseRef.current, flyRef.current, baseFovRef.current);
    pump();
  }, [enabled, effectiveMode, client, restored, pump]);

  const dismissError = useCallback((): void => {
    errorRef.current = null;
    setError(null);
  }, []);

  return {
    mode: effectiveMode,
    pose,
    fovYDeg: currentFovYDeg(fly, baseFovYDeg ?? STAGE_FOV_Y_DEG),
    flying,
    flySpeed: fly.speedMetersPerSecond,
    error,
    handlers: { onPointerDown, onPointerMove, onPointerUp, onPointerCancel, onWheel, onContextMenu },
    setMode,
    toggleMode,
    frameSelection,
    goHome,
    goToPose,
    dismissError,
  };
}
