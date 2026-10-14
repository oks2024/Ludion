// The streaming viewport: a canvas painted by `useFrameStream`, a status overlay (resolution, sim
// frame, received fps), an fps-cap control, and keyboard input injection.
//
// Input capture is scoped to focus: the viewport is a focusable surface, and only while it holds
// focus do keyboard edges become `input.inject` commands (so typing into the inspector or the URL bar
// never leaks into the game). A small indicator shows when capture is live. Presses/releases are
// mirrored honestly (keydown -> key_down, keyup -> key_up) and OS auto-repeat is suppressed, matching
// the engine's own no-repeat edge semantics.
//
// `mode` gates that same input path: edit mode pauses the sim (App's mode toggle), and a paused sim
// shouldn't also be receiving keystrokes meant for gameplay, so `input.inject` is play-mode only.
//
// Mouse state is forwarded on the same play-mode gate (mousemap.ts), so twin-stick/hybrid aim modes
// work in the editor exactly as under a windowed host: pointer motion over the viewport becomes a
// `mouse_move` (mapped to render-target pixels by the SAME `clientToEnginePoint` the picker uses, then
// offered to `injectCoalescer.ts`'s latest-wins gate, which self-paces sends to the server's reply rate
// rather than the animation-frame rate), and button edges become `mouse_down`/`mouse_up`. Edges flush
// any parked move first, so an aim update is never overtaken on the wire by the click it preceded. Held
// buttons are released (an up edge) when the pointer leaves the viewport or play mode ends mid-hold so
// the engine's held-state can't stick. All of this is inert in edit mode, where the mouse drives
// selection/gizmos below and never reaches the sim.
//
// Click-picking (edit mode) and the selection overlay are the second half of the story (Plan/phase-6-
// editing.md decisions #4 and #5). A click maps to engine pixels and asks `render.bounds` "what did
// I just draw, where"; the pure hit test in `./picking` turns that answer into a selection (nearest
// to the camera among overlapping rects, smallest-area if the camera can't be read). The selection
// itself is drawn as a client-side overlay box over the streamed frame — polled at ~3 Hz so it tracks
// the entity as the sim moves it — never rendered into the canvas, so the streamed pixels stay free of
// editor chrome (decision #5).
//
// Transform gizmos sit on top (decision #7): while an entity is selected in edit mode the
// overlay poll also derives a `GizmoLayout` (camera + the selected entity's WORLD pose), drawn as an SVG.
// A parented entity's `Transform` is local to its parent, so the gizmo is drawn at the world pose and a
// drag writes the local value back (`GizmoFrame`); a root's local value is its world pose.
// Dragging a handle previews via throttled `component.set`s and collapses to one undo entry; all the
// math is the pure `editor/gizmo` core and the drag policy is `GizmoController`, so this component
// only translates DOM pointer/keyboard events into those calls and their instructions into protocol
// calls.
//
// The VIEW CAMERA (`Plan/editor-stage-camera.md`) is the one camera `useViewCamera` mounts here,
// over the `render.view` host override. Three things follow from that, and each is load-bearing:
//
//  - PLAIN LEFT IS STILL THE VIEW'S (`plainLeftOrbits: false`, decision 5). The hook's pointerdown is
//    offered the gesture first and answers whether it took it; a `false` falls straight through to
//    picking, the marker and the gizmo, exactly as before the camera existed.
//  - EVERY PROJECTION FOLLOWS THE CAMERA THAT IS SHOWING. While `View` is on, the `CameraKit` the
//    gizmo, the marker and the 3D pointer's ray project through is `viewOverrideKit` — the client-side
//    twin of `render_view.rs` — and `render.bounds` is asked for `camera: "view"`. Project through the
//    scene camera while the stream shows a flown one and every handle lands somewhere the cursor is
//    not. The kit is rebuilt LOCALLY on every pose change (the scene camera's clip planes are already
//    in hand), so a fly costs no extra wire traffic and the handles never lag the frame by a poll.
//  - LOOKING NEVER DIRTIES THE SESSION. The override is host render state: no `component.set` on the
//    scene camera, no editor camera entity, nothing `scene.save` can capture and nothing in a
//    `WorldHash`. Play mode forces `Game` (mouse coordinates are the game camera's pixels) and the
//    hook keeps the pose for the return to Edit.

import { useCallback, useEffect, useRef, useState, type KeyboardEvent, type MouseEvent, type MutableRefObject, type PointerEvent } from 'react';
import type { InputInjectParams } from '@ludion/protocol';
import type { ProtocolClient } from '../protocol/client';
import { componentSet } from '../protocol/commands';
import type { EditorMode, EntityId } from '../App';
import type { UndoEntry } from '../editor/undo';
import { toDiagnostic, type DiagnosticView } from './diagnostic';
import { DiagnosticBanner } from './DiagnosticBanner';
import { entityKey } from './Inspector';
import { createInjectCoalescer, type InjectCoalescer } from './injectCoalescer';
import { keyEdgeToInject, isTextEditingTarget } from './keymap';
import { createLagProbe, formatLag, type LagProbe } from './lagProbe';
import { mouseButtonInject, mouseMoveInject, pointerButtonName, releaseInjects, type EngineMouseButton } from './mousemap';
import {
  STREAM_SIZES,
  STREAM_SIZE_KEY,
  describeStreamSize,
  parseStreamSize,
  serializeStreamSize,
  type StreamSize,
} from './streamSize';
import { usePolling } from './usePolling';
import { useFrameStream, type FrameStatus } from './useFrameStream';
import { GizmoOverlay, type CanvasBox } from './GizmoOverlay';
import { buildGizmo, hitTest, type GizmoLayout, type GizmoMode } from '../editor/gizmo/model';
import { GizmoController } from '../editor/gizmo/controller';
import {
  OVERRIDE_FALLBACK_FOV_Y_DEG,
  readSceneCamera,
  gizmoParentQuery,
  gizmoPoseQuery,
  mergeQueryBodies,
  readGizmoPose,
  viewOverrideKit,
  type SceneCamera,
} from '../editor/gizmo/scene';
import { buildViewProj, screenToRay, type CameraKit, type Transform } from '../editor/gizmo/camera';
import type { Vec3 } from '../editor/gizmo/vecmath';
import { useViewCamera } from '../editor/camera/useViewCamera';
import type { Sphere, Viewport as CameraViewport } from '../editor/voxel/orbit';
import type { ContextCamera } from '../terminal/context';
import type { GizmoFrame, SnapSettings } from '../editor/gizmo/drag';
import { resolvePlacement } from '../editor/pointer/placement';
import { buildMarkerGizmo, MarkerDragController } from '../editor/pointer/markerDrag';
import {
  cameraTranslation,
  canvasClientRect,
  chooseHit,
  clientToEnginePoint,
  engineRectToCssRect,
  parseEntityBounds,
  selectedScreenRect,
  type EntityScreenBounds,
  type Rect,
} from './picking';

interface ViewportProps {
  readonly client: ProtocolClient;
  readonly mode: EditorMode;
  readonly selection: EntityId | null;
  readonly onSelect: (entity: EntityId | null) => void;
  /** Records a completed gizmo drag as one undo entry (push + dirty). */
  readonly onEditEntry: (entry: UndoEntry) => void;
  /** Marks the scene dirty the moment a drag's first preview set goes out. */
  readonly onDirty: () => void;
  /** Live connection status: while not `live` the canvas holds its last frame and an
   *  overlay says the socket dropped, so a `ludion dev` blink reads as a reconnect, not a dead viewport. */
  readonly connection?: 'live' | 'reconnecting' | 'failed';
  /** Scene root label for the top-left overlay chip (scene file basename), or null before known. */
  readonly scenePath?: string | null;
  /** Reports the live frame status up so the shell's status bar can read fps/frame timing. */
  readonly onFrameStatus?: (status: FrameStatus) => void;
  /** The human's placed 3D pointer (world coords), or `null` when none — an editor concept App owns,
   *  not a scene entity. Seeds the marker gizmo and, cleared, tears the marker down. */
  readonly pointer: readonly [number, number, number] | null;
  /** Commits a placed / dragged pointer position (or `null` to clear) back to App, which persists it,
   *  feeds the status bar, and writes it to the context bridge. Fired once per gesture, never per move. */
  readonly onPointerChange: (position: readonly [number, number, number] | null) => void;
  /** Reports which camera this viewport is showing, and where the view camera stands while it is the
   *  one showing, for the context bridge — so a persona knows whether there is a "here" to look at
   *  (`Plan/editor-stage-camera.md` decision 6). `null` on unmount. */
  readonly onCamera?: (camera: ContextCamera | null) => void;
  /** Filled with the world-point resolver (client position → AABB-or-ground hit, `null` off the canvas
   *  or on the sky), the same ray chain as the armed 3D pointer; cleared on unmount. Place-by-drag
   *  reads it at drop time. */
  readonly placeResolverRef?: MutableRefObject<PlaceResolver | null>;
}

/** Resolves a client-space drop position to the world point under it, or `null` for no hit. */
export type PlaceResolver = (clientX: number, clientY: number) => Promise<Vec3 | null>;

/** The fps-cap choices offered in the overlay control; `null` means uncapped (viewer default). */
const FPS_CAPS: readonly (number | null)[] = [null, 15, 30, 60];

// The stream-extent choices sit beside these, in `./streamSize` — the list, its storage form and its
// bounds are pure data with real edge cases, and `vite.config.ts` only collects `src/**/*.test.ts`
// under node, so nothing testable may live in this file.

/** The gizmo modes, in toolbar order. */
const GIZMO_MODES: readonly GizmoMode[] = ['translate', 'rotate', 'scale'];

/** How often the selection overlay re-queries `render.bounds` (~3 Hz) — the same low-Hz poll the
 *  inspector uses, enough to track an entity the sim keeps moving without thrashing the engine. */
const OVERLAY_POLL_MS = 300;

const NO_SNAP: SnapSettings = { translateStep: 0, rotateStepDeg: 0, scaleStep: 0 };

/** The `localStorage` identity the view camera's pose is kept under here. ONE id for all four views
 *  that frame this viewport (Level Designer, Art Director, Lighter, Game Designer): they share one
 *  session, one scene and one stream, so a per-view pose would teleport the camera on a switch that
 *  changed nothing on screen. */
const MAIN_VIEW_CAMERA_ID = 'main-session';

/** How long the camera must sit still before the context bridge hears about it. A flown pose moves
 *  every frame; reporting each one would re-render the shell at frame rate to feed a file that is
 *  written debounced anyway, and no persona can act on a pose that has already gone. */
const CAMERA_CONTEXT_SETTLE_MS = 300;

/** The smallest radius `F` will frame, so a point-sized entity's bounds don't put the camera inside it. */
const MIN_FRAME_RADIUS = 0.5;

/** The canvas's placement within its stage plus its engine size — the SVG overlay's box and viewBox. */
function canvasCssBox(canvas: HTMLCanvasElement, stage: HTMLDivElement): CanvasBox {
  const cr = canvas.getBoundingClientRect();
  const sr = stage.getBoundingClientRect();
  return { x: cr.left - sr.left, y: cr.top - sr.top, w: cr.width, h: cr.height, engineW: canvas.width, engineH: canvas.height };
}

/** A number field's value clamped to a non-negative snap step (blank/negative/NaN → 0 = off). */
function snapStep(text: string): number {
  const n = Number(text);
  return Number.isFinite(n) && n > 0 ? n : 0;
}

export function Viewport({
  client,
  mode,
  selection,
  onSelect,
  onEditEntry,
  onDirty,
  connection = 'live',
  scenePath = null,
  onFrameStatus,
  pointer,
  onPointerChange,
  onCamera,
  placeResolverRef,
}: ViewportProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const stageRef = useRef<HTMLDivElement | null>(null);
  const [maxFps, setMaxFps] = useState<number | null>(null);
  // The stream extent this viewport asks the host for, restored from the last session's choice.
  // `null` is "leave the host's own extent alone", which is what lets a `--render-size` startup
  // extent and this knob compose (see `./streamSize`).
  const [streamSize, setStreamSize] = useState<StreamSize | null>(() => {
    try {
      return parseStreamSize(window.localStorage.getItem(STREAM_SIZE_KEY));
    } catch {
      return null; // A document with storage denied gets the host's extent, like everyone else.
    }
  });
  const [focused, setFocused] = useState(false);
  // The latency instrument, owned here because this is the surface that both injects input and blits
  // frames — the two ends of the measurement. Lazily built into a ref so it is one object for the
  // component's life (the frame stream keys its subscription on it) and its rings survive re-renders.
  const probeRef = useRef<LagProbe | null>(null);
  if (probeRef.current === null) {
    probeRef.current = createLagProbe();
  }
  const probe = probeRef.current;
  const status = useFrameStream(client, canvasRef, maxFps, probe);

  // Lift the frame status so the shell's status bar reads the same fps/timing the overlay shows.
  useEffect(() => {
    onFrameStatus?.(status);
  }, [status, onFrameStatus]);

  // The selection overlay box in CSS pixels (positioned inside the stage), or `null` when hidden.
  const [overlayRect, setOverlayRect] = useState<Rect | null>(null);
  // A failed `render.bounds` from a user-initiated pick; cleared by the next successful pick or poll.
  const [pickError, setPickError] = useState<DiagnosticView | null>(null);
  // The latest parsed bounds and the current selection, held in refs so the resize handler and the
  // poll callback recompute the box without restarting on every render.
  const boundsRef = useRef<readonly EntityScreenBounds[] | null>(null);
  const selectionRef = useRef<EntityId | null>(selection);
  selectionRef.current = selection;

  // --- The view camera (Plan/editor-stage-camera.md) -----------------------------------------

  // The scene's own camera, as last read from the wire: the override borrows its clip planes and,
  // for a pose that names none, its field of view — so the client's projection is the renderer's.
  const sceneCameraRef = useRef<SceneCamera | null>(null);
  // The angle the override is asking for, or `null` before any camera has been read. Passing it as
  // the hook's base is what makes every pose it sends carry a field of view this component already
  // knows, which is the only way the two projections can be identical rather than merely close.
  const [sceneFovYDeg, setSceneFovYDeg] = useState<number | null>(null);
  // The canvas's CSS size — what the hook's pan and framing project through (its deltas are CSS
  // pixels, unlike everything the gizmo does, which is engine pixels).
  const [cssViewport, setCssViewport] = useState<CameraViewport>({ width: 16, height: 9 });

  // What `F` frames and what an Alt-orbit pivots around: the selected entity's world AABB, which the
  // bounds poll already has in hand. Read through refs, so the hook never re-subscribes for it.
  const selectionSphere = useCallback((): Sphere | null => {
    const sel = selectionRef.current;
    const bounds = boundsRef.current;
    if (sel === null || bounds === null) {
      return null;
    }
    const match = bounds.find((b) => b.entity.index === sel.index && b.entity.generation === sel.generation);
    if (match === undefined) {
      return null;
    }
    const { min, max } = match.worldAabb;
    const center: Vec3 = [(min[0] + max[0]) / 2, (min[1] + max[1]) / 2, (min[2] + max[2]) / 2];
    const radius = Math.hypot(max[0] - min[0], max[1] - min[1], max[2] - min[2]) / 2;
    return { center, radius: Math.max(radius, MIN_FRAME_RADIUS) };
  }, []);

  const camera = useViewCamera({
    client,
    viewId: MAIN_VIEW_CAMERA_ID,
    scenePath,
    viewport: cssViewport,
    // Decision 5's one binding not copied from Unreal: plain left here stays pick / marquee / gizmo.
    plainLeftOrbits: false,
    selectionBounds: selectionSphere,
    baseFovYDeg: sceneFovYDeg ?? undefined,
    // Mouse coordinates in play are the game camera's pixels, so play is always the game camera. The
    // hook keeps the pose while it is forced, and restores it the moment edit resumes.
    forceGame: mode === 'play',
  });

  const cameraModeRef = useRef(camera.mode);
  cameraModeRef.current = camera.mode;
  const cameraPoseRef = useRef(camera.pose);
  cameraPoseRef.current = camera.pose;
  // `null` until a scene camera has been read, which is exactly when no base was passed and the
  // pose the hook sends therefore names no field of view.
  const cameraFovRef = useRef<number | null>(null);
  cameraFovRef.current = sceneFovYDeg === null ? null : camera.fovYDeg;
  const cameraHandlersRef = useRef(camera.handlers);
  cameraHandlersRef.current = camera.handlers;
  /** Whether a camera gesture owns the pointer, so the view's own drags stay out of its way. */
  const cameraDragRef = useRef(false);
  const flyingRef = useRef(camera.flying);
  flyingRef.current = camera.flying;

  /** Which camera a `render.bounds` / `render.screenshot` must answer through to match the frame the
   *  human is looking at. `"scene"` is the wire default, and the value every agent call keeps. */
  const boundsCamera = useCallback(
    (): 'scene' | 'view' => (cameraModeRef.current === 'view' ? 'view' : 'scene'),
    [],
  );

  /** The kit every overlay projects through: the flown override while the view camera is showing,
   *  the scene's own `Camera` otherwise. `null` only when there is no camera to project through at
   *  all, which the override never is — it supplies its own viewpoint. */
  const projectionKit = useCallback((scene: SceneCamera | null, size: CameraViewport): CameraKit | null => {
    if (cameraModeRef.current === 'view') {
      return viewOverrideKit(cameraPoseRef.current, cameraFovRef.current, scene, size);
    }
    return scene === null ? null : buildViewProj(scene.transform, scene.camera, size);
  }, []);

  /** Records the scene camera a wire body just reported, and the angle the override inherits from it
   *  (an orthographic camera lends none, so it inherits the renderer's own fallback). */
  const noteSceneCamera = useCallback((scene: SceneCamera | null): void => {
    sceneCameraRef.current = scene;
    setSceneFovYDeg(
      scene === null
        ? null
        : scene.camera.projection.kind === 'perspective'
          ? scene.camera.projection.fovYDeg
          : OVERRIDE_FALLBACK_FOV_Y_DEG,
    );
  }, []);

  // Report the canvas's CSS size, which is what a pan's pixel deltas and `F`'s framing are measured
  // in. The canvas is letterboxed, so its own rect — not the stage's — carries the frame's aspect.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (canvas === null) {
      return;
    }
    const report = (): void => {
      const rect = canvas.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        setCssViewport((size) =>
          size.width === rect.width && size.height === rect.height ? size : { width: rect.width, height: rect.height },
        );
      }
    };
    report();
    const observer = new ResizeObserver(report);
    observer.observe(canvas);
    return () => observer.disconnect();
  }, []);

  // --- Gizmo state (viewport-local tools; App doesn't need mode/snap) ---------------------------
  const [gizmoMode, setGizmoMode] = useState<GizmoMode>('translate');
  const [snap, setSnap] = useState<SnapSettings>(NO_SNAP);
  const [gizmoLayout, setGizmoLayout] = useState<GizmoLayout | null>(null);
  const [canvasBox, setCanvasBox] = useState<CanvasBox | null>(null);

  // Refs mirror the pieces the pointer/keyboard handlers need without re-subscribing every render:
  // the projected kit + base transform (for `beginDrag`/`buildGizmo`), the live layout (for `hitTest`),
  // and the current mode/snap. `layoutRef` shadows `gizmoLayout` so a pointerdown hit-tests the latest.
  const kitRef = useRef<CameraKit | null>(null);
  const transformRef = useRef<Transform | null>(null); // the selected entity's LOCAL `Transform`
  const frameRef = useRef<GizmoFrame | null>(null); // its world pose + parent frame; null for a root
  const layoutRef = useRef<GizmoLayout | null>(null);
  const modeRef = useRef(mode);
  modeRef.current = mode;
  const gizmoModeRef = useRef(gizmoMode);
  gizmoModeRef.current = gizmoMode;
  const snapRef = useRef(snap);
  snapRef.current = snap;

  // --- Play-mode mouse forwarding (mousemap.ts) -------------------------------------------------
  // The buttons currently held down in the engine (so a leave / mode-exit can release them), and the
  // move coalescer: pointermove offers the latest render-target position to injectCoalescer.ts's
  // latest-wins gate, which self-paces sends to the server's reply rate instead of rAF rate (this
  // feature's whole point — see injectCoalescer.ts's header).
  const heldButtonsRef = useRef<Set<EngineMouseButton>>(new Set());

  // Mirrors `client` every render so the coalescer's `send` (bound once, below) never closes over a
  // stale client after a reconnect swaps the prop (App recreates `ProtocolClient` on reconnect).
  const clientRef = useRef(client);
  clientRef.current = client;

  const coalescerRef = useRef<InjectCoalescer | null>(null);
  if (coalescerRef.current === null) {
    coalescerRef.current = createInjectCoalescer((params) =>
      clientRef.current.call({ method: 'input.inject', params }).catch(() => {}),
    );
  }
  const coalescer = coalescerRef.current;

  // The drag session (pure policy) plus the DOM bookkeeping a capture needs.
  const controllerRef = useRef(new GizmoController());
  const draggingRef = useRef(false);
  const pointerIdRef = useRef<number | null>(null);
  const dragEntityRef = useRef<EntityId | null>(null);
  const suppressClickRef = useRef(false);

  // --- 3D pointer (the "here" marker) — a virtual, non-entity gizmo the human places ------------
  // `armed`: the next viewport click places/replaces the marker instead of picking. `markerPosRef`
  // is the live world position (seeded from the `pointer` prop, advanced locally during a drag so App
  // doesn't re-render per move). `markerLayout(+Ref)` is the projected translate gizmo drawn at it, and
  // `markerControllerRef` is its pure translate-only drag policy (no protocol/undo — see markerDrag.ts).
  const [armed, setArmed] = useState(false);
  const armedRef = useRef(armed);
  armedRef.current = armed;
  const markerPosRef = useRef<Vec3 | null>(pointer);
  const [markerLayout, setMarkerLayout] = useState<GizmoLayout | null>(null);
  const markerLayoutRef = useRef<GizmoLayout | null>(null);
  const markerControllerRef = useRef(new MarkerDragController());
  const draggingMarkerRef = useRef(false);
  // Held in refs so the window-level key handlers (keyed on `mode`) call the current closures without
  // re-subscribing on every pointer change.
  const onPointerChangeRef = useRef(onPointerChange);
  onPointerChangeRef.current = onPointerChange;

  // Recomputes the overlay box from the given bounds and selection against the live canvas/stage
  // geometry. Hides the box (null) when anything needed is missing: no frame, no selection, or the
  // selected entity is absent / behind the camera in the latest bounds.
  const applyOverlay = useCallback(
    (bounds: readonly EntityScreenBounds[] | null, sel: EntityId | null): void => {
      const canvas = canvasRef.current;
      const stage = stageRef.current;
      if (canvas === null || stage === null || bounds === null || sel === null) {
        setOverlayRect(null);
        return;
      }
      const engineRect = selectedScreenRect(bounds, sel);
      if (engineRect === null) {
        setOverlayRect(null);
        return;
      }
      const canvasRect = canvas.getBoundingClientRect();
      const stageRect = stage.getBoundingClientRect();
      setOverlayRect(
        engineRectToCssRect(engineRect, {
          offsetX: canvasRect.left - stageRect.left,
          offsetY: canvasRect.top - stageRect.top,
          cssWidth: canvasRect.width,
          cssHeight: canvasRect.height,
          engineWidth: canvas.width,
          engineHeight: canvas.height,
        }),
      );
    },
    [],
  );

  // Clears only the ENTITY gizmo's derived state (no transform, entity gone, or degenerate projection).
  // Deliberately leaves `kitRef` alone: the camera kit is shared with the marker gizmo, so a deselect
  // must not wipe the projection a still-live marker needs (the kit-lifetime hazard). The selection box
  // is independent — it stays whatever `applyOverlay` last set.
  const clearEntityGizmo = useCallback((): void => {
    transformRef.current = null;
    frameRef.current = null;
    layoutRef.current = null;
    setGizmoLayout(null);
  }, []);

  // Hides the marker gizmo (its projected layout) without forgetting the marker itself — the position
  // survives in `markerPosRef` / the `pointer` prop. Used when leaving edit or when the projection fails.
  const clearMarkerLayout = useCallback((): void => {
    markerLayoutRef.current = null;
    setMarkerLayout(null);
  }, []);

  // Derives the overlays from the wire in ONE camera round-trip: the shared `CameraKit`
  // (`query.entities`) then, from it, (a) the selected entity's gizmo (`query.entities` with `world` → `buildGizmo`)
  // and (b) the marker's translate gizmo (`buildMarkerGizmo`). Quiet degradation — any missing/malformed
  // piece just hides that overlay. `skipEntity`/`skipMarker` let the poll leave a side that's mid-drag
  // untouched (its preview is authoritative until the gesture ends), while still refreshing the other.
  const refreshOverlays = useCallback(
    async (sel: EntityId | null, markerPos: Vec3 | null, skip?: { entity?: boolean; marker?: boolean }): Promise<void> => {
      const skipEntity = skip?.entity ?? false;
      const skipMarker = skip?.marker ?? false;
      const canvas = canvasRef.current;
      const stage = stageRef.current;
      if (canvas === null || stage === null || modeRef.current !== 'edit' || canvas.width <= 0 || canvas.height <= 0) {
        if (!skipEntity) clearEntityGizmo();
        if (!skipMarker) clearMarkerLayout();
        kitRef.current = null;
        return;
      }
      if (sel === null && markerPos === null) {
        clearEntityGizmo();
        clearMarkerLayout();
        kitRef.current = null;
        return;
      }
      const size = { width: canvas.width, height: canvas.height };
      try {
        const camBody = await client.call({
          method: 'query.entities',
          params: { filter: ['Camera'], include: ['Transform', 'Camera'] },
        });
        const scene = readSceneCamera(camBody);
        noteSceneCamera(scene);
        const kit = projectionKit(scene, size);
        if (kit === null) {
          if (!skipEntity) clearEntityGizmo();
          if (!skipMarker) clearMarkerLayout();
          kitRef.current = null;
          return;
        }
        kitRef.current = kit;
        setCanvasBox(canvasCssBox(canvas, stage));
        if (!skipEntity) {
          if (sel === null) {
            clearEntityGizmo();
          } else {
            const selfBody = await client.call({
              method: 'query.entities',
              params: gizmoPoseQuery(sel),
            });
            const parentParams = gizmoParentQuery(selfBody, sel);
            const poseBody =
              parentParams === null
                ? selfBody
                : mergeQueryBodies(
                    selfBody,
                    await client.call({ method: 'query.entities', params: parentParams }),
                  );
            const pose = readGizmoPose(poseBody, sel);
            if (pose === null) {
              clearEntityGizmo();
            } else {
              transformRef.current = pose.local;
              frameRef.current = pose.frame;
              const layout = buildGizmo(pose.frame?.world ?? pose.local, kit, size, gizmoModeRef.current);
              layoutRef.current = layout;
              setGizmoLayout(layout);
            }
          }
        }
        if (!skipMarker) {
          if (markerPos === null) {
            clearMarkerLayout();
          } else {
            const layout = buildMarkerGizmo(markerPos, kit, size);
            markerLayoutRef.current = layout;
            setMarkerLayout(layout);
          }
        }
      } catch {
        if (!skipEntity) clearEntityGizmo();
        if (!skipMarker) clearMarkerLayout();
      }
    },
    [client, clearEntityGizmo, clearMarkerLayout, noteSceneCamera, projectionKit],
  );

  // Forgets the marker entirely — position, drag session, and layout — for a Shift+P clear or an App
  // pointer reset (foreign scene load). Distinct from `clearMarkerLayout`, which keeps the position.
  const clearMarker = useCallback((): void => {
    markerPosRef.current = null;
    markerControllerRef.current.reset();
    draggingMarkerRef.current = false;
    clearMarkerLayout();
  }, [clearMarkerLayout]);

  // Fire-and-forget a mouse `input.inject`, same best-effort posture as the keyboard path: input is
  // high-frequency, so a transient failure is never worth a banner.
  const injectMouse = useCallback(
    (params: InputInjectParams): void => {
      void client.call({ method: 'input.inject', params }).catch(() => {});
    },
    [client],
  );

  // Release every button the engine still thinks is held (leave / mode-exit), then forget them, so a
  // button held while the pointer leaves the viewport or play flips to edit can't stick down.
  const releaseHeldButtons = useCallback((): void => {
    coalescer.flushMove();
    for (const params of releaseInjects(heldButtonsRef.current)) {
      injectMouse(params);
    }
    heldButtonsRef.current.clear();
  }, [injectMouse, coalescer]);

  const handleKey = useCallback(
    (event: KeyboardEvent<HTMLDivElement>) => {
      // Read the clock before anything else: the gap to the event's own timestamp IS the main
      // thread's dispatch delay, and any work done first would be counted as queue time.
      const handledAt = performance.now();
      // Edit mode pauses the sim by convention; keystrokes are gameplay input and stay play-mode only.
      if (mode !== 'play') {
        return;
      }
      const inject = keyEdgeToInject({ code: event.code, type: event.type as 'keydown' | 'keyup', repeat: event.repeat });
      if (inject === null) {
        return;
      }
      // The key is one the engine consumes: stop the browser's default (arrow/space scrolling).
      event.preventDefault();
      // A parked aim update must reach the wire ahead of this edge, or the sim would see the edge
      // (e.g. an interact/reload key) against a stale cursor position.
      coalescer.flushMove();
      // Measured on the press alone: a release is the same wire trip, but "press it and see it" is
      // the latency a human reports. `nativeEvent.timeStamp` rather than React's synthetic copy,
      // which substitutes `Date.now()` for a falsy native value and would put the two clocks on
      // different origins — the probe's guard catches that, but there is no reason to feed it.
      const token = event.type === 'keydown' ? probe.noteInject(event.nativeEvent.timeStamp, handledAt) : null;
      void client
        .callFull({ method: 'input.inject', params: inject })
        .then((response) => {
          if (token !== null) {
            probe.resolveInject(token, response.frame);
          }
        })
        .catch(() => {
          // Input is best-effort and high-frequency; a transient failure isn't worth a banner.
          if (token !== null) {
            probe.resolveInject(token, null);
          }
        });
    },
    [client, mode, probe, coalescer],
  );

  // The ray → AABB-or-ground chain shared by the armed 3D pointer and place-by-drag: the world point
  // under a client position, plus the kit and size it was projected with. `null` is a miss (off the
  // canvas, no camera, degenerate ray, sky). A `render.bounds` failure propagates to the caller.
  const resolveWorldPoint = useCallback(
    async (clientX: number, clientY: number): Promise<{ position: Vec3; kit: CameraKit; size: { width: number; height: number } } | null> => {
      const canvas = canvasRef.current;
      if (canvas === null) {
        return null;
      }
      const point = clientToEnginePoint({ x: clientX, y: clientY }, canvasClientRect(canvas));
      if (point === null) {
        return null;
      }
      const body = await client.call({ method: 'render.bounds', params: { camera: boundsCamera() } });
      const bounds = parseEntityBounds(body) ?? [];
      const camBody = await client.call({
        method: 'query.entities',
        params: { filter: ['Camera'], include: ['Transform', 'Camera'] },
      });
      const liveCanvas = canvasRef.current;
      if (liveCanvas === null) {
        return null;
      }
      const size = { width: liveCanvas.width, height: liveCanvas.height };
      const scene = readSceneCamera(camBody);
      noteSceneCamera(scene);
      const kit = projectionKit(scene, size);
      if (kit === null) {
        return null;
      }
      const ray = screenToRay(point, size, kit.invViewProj);
      if (ray === null) {
        return null;
      }
      const placed = resolvePlacement(ray, bounds);
      return placed === null ? null : { position: placed.position, kit, size };
    },
    [client, boundsCamera, noteSceneCamera, projectionKit],
  );

  const handleStageClick = useCallback(
    (event: MouseEvent<HTMLDivElement>) => {
      // A click that closed a gizmo drag isn't a pick — swallow it once (the flag is set at pointerdown).
      if (suppressClickRef.current) {
        suppressClickRef.current = false;
        return;
      }
      if (mode !== 'edit') {
        return;
      }
      const canvas = canvasRef.current;
      if (canvas === null) {
        return;
      }
      const point = clientToEnginePoint({ x: event.clientX, y: event.clientY }, canvasClientRect(canvas));
      // No frame yet (zero intrinsic size) or a zero-size canvas: nothing to hit-test against, so
      // leave the current selection untouched rather than clearing it on an un-mappable click.
      if (point === null) {
        return;
      }

      // Armed: this click places (or replaces) the 3D pointer instead of picking — selection is left
      // untouched. Raycast the click against entity AABBs (nearest) or the y=0 ground; an unresolvable
      // ray (sky) stays armed and is a no-op. We hold the same kit we projected with, so the marker
      // gizmo can appear immediately without waiting for the poll.
      if (armedRef.current) {
        void (async () => {
          try {
            const resolved = await resolveWorldPoint(event.clientX, event.clientY);
            const stage = stageRef.current;
            const liveCanvas = canvasRef.current;
            if (resolved === null || liveCanvas === null || stage === null) {
              return; // sky, no camera or degenerate ray: stay armed, no-op
            }
            const { position, kit, size } = resolved;
            setPickError(null);
            onPointerChange(position);
            markerPosRef.current = position;
            setArmed(false);
            kitRef.current = kit;
            setCanvasBox(canvasCssBox(liveCanvas, stage));
            const layout = buildMarkerGizmo(position, kit, size);
            markerLayoutRef.current = layout;
            setMarkerLayout(layout);
          } catch (err) {
            // A user-initiated `render.bounds` failure (e.g. L2102 no camera) is the human's to see.
            setPickError(toDiagnostic(err));
          }
        })();
        return;
      }

      void (async () => {
        try {
          const body = await client.call({ method: 'render.bounds', params: { camera: boundsCamera() } });
          const bounds = parseEntityBounds(body);
          if (bounds === null) {
            // A well-formed but unexpected body: treat as an empty pick (clears), same as a miss.
            onSelect(null);
            return;
          }
          boundsRef.current = bounds;
          // The camera translation is the depth tie-break for overlapping rects, and must be the
          // camera the rects were projected through — while the view camera is showing, that is the
          // flown pose, which needs no round-trip. A failure to read it (no camera, error) is
          // non-fatal: `chooseHit` falls back to the smallest-area rect.
          let cameraAt: readonly [number, number, number] | null = null;
          if (cameraModeRef.current === 'view') {
            cameraAt = cameraPoseRef.current.translation;
          } else {
            try {
              const cameraBody = await client.call({
                method: 'query.entities',
                params: { filter: ['Camera'], include: ['Transform'] },
              });
              cameraAt = cameraTranslation(cameraBody);
            } catch {
              cameraAt = null;
            }
          }
          const hit = chooseHit(point, bounds, cameraAt);
          setPickError(null);
          onSelect(hit);
          // We already hold the bounds this pick used — refresh the box immediately rather than
          // waiting for the ~3 Hz poll to catch up, and derive the gizmo for the newly-picked entity.
          applyOverlay(bounds, hit);
          void refreshOverlays(hit, markerPosRef.current);
        } catch (err) {
          // A failed `render.bounds` (e.g. L2102 no camera) is the human's to see, not swallowed.
          setPickError(toDiagnostic(err));
        }
      })();
    },
    [mode, client, onSelect, applyOverlay, refreshOverlays, onPointerChange, boundsCamera, resolveWorldPoint],
  );

  // Hands the resolver to App for place-by-drag; the client rect is checked here because
  // `clientToEnginePoint` extrapolates past the canvas edge rather than clamping.
  useEffect(() => {
    if (placeResolverRef === undefined) {
      return;
    }
    placeResolverRef.current = async (clientX, clientY) => {
      const canvas = canvasRef.current;
      if (canvas === null) {
        return null;
      }
      const r = canvas.getBoundingClientRect();
      if (clientX < r.left || clientX > r.right || clientY < r.top || clientY > r.bottom) {
        return null;
      }
      return (await resolveWorldPoint(clientX, clientY))?.position ?? null;
    };
    return () => {
      placeResolverRef.current = null;
    };
  }, [placeResolverRef, resolveWorldPoint]);

  // The selection overlay poll: while an entity is selected in edit mode, re-query `render.bounds` at
  // ~3 Hz and re-place the box so it tracks the entity as the sim moves it. Errors here stay quiet
  // (the user-initiated pick above is what surfaces failures) to avoid a banner flickering at 3 Hz.
  // Also refreshes the gizmo on the same round-trip — but never while a drag is live, so the poll
  // can't overwrite the pose the drag is previewing (decision #7).
  const pollOverlay = useCallback(async (): Promise<void> => {
    try {
      const body = await client.call({ method: 'render.bounds', params: { camera: boundsCamera() } });
      const bounds = parseEntityBounds(body);
      if (bounds !== null) {
        boundsRef.current = bounds;
        setPickError(null);
        applyOverlay(bounds, selectionRef.current);
      }
    } catch {
      // Quiet: a persistent failure is already visible from the pick that set the selection.
    }
    // Refresh both gizmos on the same round-trip — but never the side a drag is previewing, so the poll
    // can't overwrite the pose the gesture owns (decision #7, extended to the marker).
    await refreshOverlays(selectionRef.current, markerPosRef.current, {
      entity: draggingRef.current,
      marker: draggingMarkerRef.current,
    });
  }, [client, applyOverlay, refreshOverlays, boundsCamera]);

  usePolling(mode === 'edit' && (selection !== null || pointer !== null), OVERLAY_POLL_MS, pollOverlay);

  // Ask the host for the chosen stream extent — on mount, and whenever the human picks another.
  //
  // A PLAIN COMMAND, deliberately not a subscription parameter: it must never join `useFrameStream`'s
  // effect deps, because re-subscribing is exactly what a resize must not do. Frames simply start
  // arriving at the new size and the hook's existing resize paths take them. The `null` choice sends
  // nothing at all, so the host's own `--render-size` survives an editor that never asked.
  useEffect(() => {
    if (streamSize === null) {
      return;
    }
    const [width, height] = streamSize;
    void client
      .call({ method: 'render.size', params: { width, height } })
      // `render.bounds` reports the same extent, and the overlay would otherwise carry a box scaled
      // by the old one until the ~3 Hz poll caught up — so re-place it on this round-trip.
      .then(() => pollOverlay())
      .catch(() => {
        // Quiet: a host that refuses the extent keeps streaming at the one it has, which is visible
        // in the overlay's own resolution readout.
      });
  }, [client, streamSize, pollOverlay]);

  // Persist the choice separately from sending it, so a stored value is written exactly once per
  // human decision rather than once per reconnect.
  useEffect(() => {
    try {
      window.localStorage.setItem(STREAM_SIZE_KEY, serializeStreamSize(streamSize));
    } catch {
      // Storage denied: the knob still works for this session.
    }
  }, [streamSize]);

  // Seed the live marker position from the `pointer` prop. When App clears it (Shift+P, a foreign
  // scene load), forget the marker's drag/layout too — the position is the source of truth here.
  useEffect(() => {
    markerPosRef.current = pointer;
    if (pointer === null) {
      markerControllerRef.current.reset();
      draggingMarkerRef.current = false;
      clearMarkerLayout();
    }
  }, [pointer, clearMarkerLayout]);

  // Refresh the overlays immediately when the selection, marker, or edit mode changes, rather than
  // waiting for the next poll tick (the box does the same via `applyOverlay` on pick).
  // `camera.mode` joins the list: flipping to `Game` must put the overlays back on the scene camera's
  // projection at once rather than a poll tick later, and the round-trip also re-reads that camera.
  useEffect(() => {
    if (mode === 'edit' && (selection !== null || pointer !== null)) {
      void refreshOverlays(selection, pointer);
    }
  }, [mode, selection, pointer, refreshOverlays, camera.mode]);

  // A flown pose moves every frame while the scene camera stands still, and the overlays must move
  // with it or a handle drifts off its entity the moment the human looks around. Everything the
  // override's projection needs is already in hand (the pose, and the scene camera's clip planes
  // from the last poll), so this is pure local math — no wire call per frame. A side that is mid-drag
  // is left alone, exactly as the poll leaves it: its own preview is authoritative.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (mode !== 'edit' || camera.mode !== 'view' || canvas === null || canvas.width <= 0 || canvas.height <= 0) {
      return;
    }
    const size = { width: canvas.width, height: canvas.height };
    const kit = viewOverrideKit(camera.pose, cameraFovRef.current, sceneCameraRef.current, size);
    kitRef.current = kit;
    if (!draggingRef.current && selectionRef.current !== null && transformRef.current !== null) {
      const layout = buildGizmo(frameRef.current?.world ?? transformRef.current, kit, size, gizmoModeRef.current);
      layoutRef.current = layout;
      setGizmoLayout(layout);
    }
    if (!draggingMarkerRef.current && markerPosRef.current !== null) {
      const layout = buildMarkerGizmo(markerPosRef.current, kit, size);
      markerLayoutRef.current = layout;
      setMarkerLayout(layout);
    }
  }, [mode, camera.mode, camera.pose, camera.fovYDeg]);

  // The context bridge's camera block, once the camera has settled (see CAMERA_CONTEXT_SETTLE_MS).
  // The pose is reported only while the view camera is the one showing: in `Game` the override is
  // cleared, so there is no "here" for a persona's `camera: "view"` to resolve against.
  const onCameraRef = useRef(onCamera);
  onCameraRef.current = onCamera;
  useEffect(() => {
    const report = onCameraRef.current;
    if (report === undefined) {
      return;
    }
    const pose = camera.pose;
    const timer = window.setTimeout(() => {
      report(
        camera.mode === 'view'
          ? {
              mode: 'view',
              pose: {
                translation: [pose.translation[0], pose.translation[1], pose.translation[2]],
                rotation: [pose.rotation[0], pose.rotation[1], pose.rotation[2]],
              },
            }
          : { mode: 'game', pose: null },
      );
    }, CAMERA_CONTEXT_SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [camera.mode, camera.pose]);

  // A viewport that is gone is reporting nothing, not still showing what it last showed.
  useEffect(() => () => onCameraRef.current?.(null), []);

  // Switching gizmo mode rebuilds the layout locally from the held kit/transform — no refetch needed.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (canvas === null || kitRef.current === null || transformRef.current === null || draggingRef.current) {
      return;
    }
    const layout = buildGizmo(frameRef.current?.world ?? transformRef.current, kitRef.current, { width: canvas.width, height: canvas.height }, gizmoMode);
    layoutRef.current = layout;
    setGizmoLayout(layout);
  }, [gizmoMode]);

  // Hide the box and entity gizmo the instant the selection clears (the poll also stops if no marker
  // remains), abandoning any in-flight entity drag. The camera kit and marker deliberately survive a
  // deselect — the marker is independent. Leaving edit mode is the harder cut: overlays are edit-gated,
  // so it also cancels the armed state, abandons any marker drag, hides the marker gizmo (its POSITION
  // survives in App state, unrelated to sim state, and returns when edit resumes), and drops the kit.
  useEffect(() => {
    if (selection === null) {
      setOverlayRect(null);
      clearEntityGizmo();
      draggingRef.current = false;
      controllerRef.current.reset();
    }
    if (mode !== 'edit') {
      setPickError(null);
      setArmed(false);
      draggingMarkerRef.current = false;
      markerControllerRef.current.reset();
      clearMarkerLayout();
      kitRef.current = null;
    }
  }, [mode, selection, clearEntityGizmo, clearMarkerLayout]);

  // Keep the box and SVG aligned when the window (and thus the CSS-scaled canvas) resizes. The engine
  // render size is fixed by the stream, so the layout geometry is unchanged — only the SVG placement.
  useEffect(() => {
    const onResize = (): void => {
      applyOverlay(boundsRef.current, selectionRef.current);
      const canvas = canvasRef.current;
      const stage = stageRef.current;
      if (canvas !== null && stage !== null && (layoutRef.current !== null || markerLayoutRef.current !== null)) {
        setCanvasBox(canvasCssBox(canvas, stage));
      }
    };
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, [applyOverlay]);

  // --- Gizmo pointer interaction ---------------------------------------------------------------

  const handlePointerDown = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      // The camera is offered the gesture first and answers whether it took it. It declines every
      // press that is not one of its own bindings — and all of them in `Game` and in play — so a
      // `false` leaves the rest of this handler exactly as it was before the camera existed.
      if (cameraHandlersRef.current.onPointerDown(event)) {
        cameraDragRef.current = true;
        // Only the primary button raises a `click`, and the one that ends an Alt+LMB orbit must not
        // also pick. Setting this for any other button would swallow the NEXT genuine pick.
        if (event.button === 0) {
          suppressClickRef.current = true;
        }
        return;
      }
      // Play mode: forward the button press to the sim and remember it as held. Editor-side click
      // behavior (picking, gizmos) is edit-only, so nothing below runs and nothing is disturbed.
      if (mode === 'play') {
        const button = pointerButtonName(event.button);
        if (button !== null) {
          // A parked aim update must precede the click it aims — flush before the down edge.
          coalescer.flushMove();
          heldButtonsRef.current.add(button);
          injectMouse(mouseButtonInject(button, 'down'));
        }
        return;
      }
      if (mode !== 'edit') {
        return;
      }
      // Armed: this press belongs to the click that places the marker, not to a drag.
      if (armedRef.current) {
        return;
      }
      const canvas = canvasRef.current;
      const kit = kitRef.current;
      if (canvas === null || kit === null) {
        return; // no projection yet — let the click-pick handler run
      }
      const point = clientToEnginePoint({ x: event.clientX, y: event.clientY }, canvasClientRect(canvas));
      if (point === null) {
        return;
      }
      const size = { width: canvas.width, height: canvas.height };

      // Hit-test the MARKER gizmo first — it draws alongside the entity gizmo and takes priority so a
      // grab on the marker's handles never falls through to the entity beneath it.
      const markerLayoutNow = markerLayoutRef.current;
      const markerPos = markerPosRef.current;
      if (markerLayoutNow !== null && markerPos !== null) {
        const markerHit = hitTest(point, markerLayoutNow);
        if (markerHit !== null && markerControllerRef.current.pointerDown(markerHit, markerPos, point, kit, size)) {
          event.preventDefault();
          draggingMarkerRef.current = true;
          pointerIdRef.current = event.pointerId;
          event.currentTarget.setPointerCapture(event.pointerId);
          suppressClickRef.current = true;
          return;
        }
      }

      // Entity gizmo path.
      const layout = layoutRef.current;
      const base = transformRef.current;
      if (layout === null || base === null || selection === null) {
        return; // no entity gizmo present — let the click-pick handler run
      }
      const hit = hitTest(point, layout);
      if (hit === null) {
        return; // clicked past the handles — fall through to click-picking
      }
      const instr = controllerRef.current.pointerDown(hit, gizmoModeRef.current, base, selection, point, kit, size, frameRef.current);
      if (instr === null) {
        return; // degenerate pick — let the click-pick handler have it
      }
      event.preventDefault();
      draggingRef.current = true;
      dragEntityRef.current = selection;
      pointerIdRef.current = event.pointerId;
      event.currentTarget.setPointerCapture(event.pointerId);
      if (instr.suppressClick) {
        suppressClickRef.current = true;
      }
    },
    [mode, selection, injectMouse, coalescer],
  );

  // Fire-and-forget a preview set (throttled, latest-wins); transient failures are ignored like input.
  const sendPreview = useCallback(
    (value: Transform): void => {
      const entity = dragEntityRef.current;
      if (entity === null) {
        return;
      }
      void client.call(componentSet(entity, 'Transform', value)).catch(() => {});
    },
    [client],
  );

  const handlePointerMove = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      // A camera gesture in flight owns the pointer: the hook ignores a move with no drag of its own,
      // and while it has one nothing below may also claim the motion.
      cameraHandlersRef.current.onPointerMove(event);
      if (cameraDragRef.current) {
        return;
      }
      const canvas = canvasRef.current;
      if (canvas === null) {
        return;
      }
      // Play mode: map to render-target pixels (the SAME transform click-picking uses) and offer to the
      // move coalescer, which self-paces sends to the server's reply rate so a high-Hz pointermove
      // stream never floods the wire.
      if (mode === 'play') {
        const point = clientToEnginePoint({ x: event.clientX, y: event.clientY }, canvasClientRect(canvas));
        if (point === null) {
          return;
        }
        coalescer.move(mouseMoveInject(point));
        return;
      }
      // Marker drag: advance the pure controller and rebuild the marker layout locally. Deliberately
      // NOT `onPointerChange` per move — App re-render + the context-bridge write stay once-per-gesture.
      if (draggingMarkerRef.current) {
        const point = clientToEnginePoint({ x: event.clientX, y: event.clientY }, canvasClientRect(canvas));
        if (point === null) {
          return;
        }
        const next = markerControllerRef.current.pointerMove(point, snapRef.current);
        if (next !== null) {
          markerPosRef.current = next;
          const kit = kitRef.current;
          if (kit !== null) {
            const layout = buildMarkerGizmo(next, kit, { width: canvas.width, height: canvas.height });
            markerLayoutRef.current = layout;
            setMarkerLayout(layout);
          }
        }
        return;
      }
      if (!draggingRef.current) {
        return;
      }
      const point = clientToEnginePoint({ x: event.clientX, y: event.clientY }, canvasClientRect(canvas));
      if (point === null) {
        return;
      }
      const instr = controllerRef.current.pointerMove(point, performance.now(), snapRef.current);
      // Re-derive the overlay from the local preview so the handles track the pointer between polls.
      if (instr.preview !== undefined && kitRef.current !== null) {
        const layout = buildGizmo(instr.world ?? instr.preview, kitRef.current, { width: canvas.width, height: canvas.height }, gizmoModeRef.current);
        layoutRef.current = layout;
        setGizmoLayout(layout);
      }
      if (instr.send !== undefined) {
        onDirty();
        sendPreview(instr.send);
      }
    },
    [onDirty, sendPreview, mode, coalescer],
  );

  const finishDrag = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      // A camera gesture keeps the pointer across button changes (releasing LMB out of an LMB+RMB pan
      // drops into the look), so it is only over once nothing is held.
      if (cameraDragRef.current) {
        if (event.type === 'pointercancel') {
          cameraHandlersRef.current.onPointerCancel(event);
          cameraDragRef.current = false;
        } else {
          cameraHandlersRef.current.onPointerUp(event);
          cameraDragRef.current = event.buttons !== 0;
        }
        return;
      }
      // Play mode: mirror the press with a button release and drop it from the held set. `pointercancel`
      // routes here too, so a cancelled press still releases rather than sticking down.
      if (mode === 'play') {
        const button = pointerButtonName(event.button);
        if (button !== null && heldButtonsRef.current.delete(button)) {
          // A parked aim update must precede the click it aims — flush before the up edge too.
          coalescer.flushMove();
          injectMouse(mouseButtonInject(button, 'up'));
        }
        return;
      }
      // Marker drag: commit the final position to App (the one write per gesture). The layout is
      // already current from the last move.
      if (draggingMarkerRef.current) {
        draggingMarkerRef.current = false;
        releaseCapture(event.currentTarget);
        const final = markerControllerRef.current.pointerUp();
        if (final !== null) {
          markerPosRef.current = final;
          onPointerChange(final);
        }
        return;
      }
      if (!draggingRef.current) {
        return;
      }
      draggingRef.current = false;
      releaseCapture(event.currentTarget);
      const instr = controllerRef.current.pointerUp();
      if (instr.send !== undefined) {
        // The exact final value, always sent (regardless of throttle) so the last position isn't lost.
        sendPreview(instr.send);
        // The final value is also the next drag's base — an immediate re-grab must not wait for the
        // ~3 Hz poll to catch up, or it would solve against the pre-drag origin.
        transformRef.current = instr.send;
        if (frameRef.current !== null && instr.world !== undefined) {
          frameRef.current = { ...frameRef.current, world: instr.world };
        }
      }
      if (instr.entry !== undefined) {
        onEditEntry(instr.entry);
      }
    },
    [onEditEntry, sendPreview, onPointerChange, mode, injectMouse, coalescer],
  );

  const releaseCapture = (el: HTMLElement): void => {
    const id = pointerIdRef.current;
    pointerIdRef.current = null;
    if (id !== null && el.hasPointerCapture(id)) {
      el.releasePointerCapture(id);
    }
  };

  // Play mode: the pointer left the viewport — release anything held so a button carried off the
  // surface can't stick down (its up edge would otherwise never arrive). Inert in edit mode.
  const handlePointerLeave = useCallback((): void => {
    if (mode === 'play') {
      releaseHeldButtons();
    }
  }, [mode, releaseHeldButtons]);

  // Play mode: swallow the context menu so a right-click can drive gameplay (its down/up are forwarded)
  // instead of popping the browser menu. Edit mode keeps the default menu untouched.
  const handleContextMenu = useCallback(
    (event: MouseEvent<HTMLDivElement>): void => {
      // The fly look is a held right button; the menu it would otherwise raise ends the drag on its
      // first frame, so the camera gets its say before play mode's.
      cameraHandlersRef.current.onContextMenu(event);
      if (mode === 'play') {
        event.preventDefault();
      }
    },
    [mode],
  );

  // Leaving play mode mid-hold: release any still-held buttons, so a paused sim doesn't keep a button
  // stuck down. (`releaseHeldButtons` flushes any parked move first, so it isn't silently dropped.) The
  // keyboard path has no equivalent blur/mode-exit release — a held key on mode-exit is a pre-existing
  // gap this doesn't touch.
  useEffect(() => {
    if (mode !== 'play') {
      releaseHeldButtons();
    }
  }, [mode, releaseHeldButtons]);

  // Escape cancels a live drag: restore the mousedown value and snap the overlay back to it. Handled
  // window-level (the capture can carry the pointer anywhere) and only while a drag is in flight.
  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== 'Escape') {
        return;
      }
      // Marker drag: cancel and restore the base (mousedown) position, rebuilding the layout there.
      if (draggingMarkerRef.current) {
        event.preventDefault();
        draggingMarkerRef.current = false;
        const stage = stageRef.current;
        if (stage !== null) {
          releaseCapture(stage);
        }
        const base = markerControllerRef.current.escape();
        if (base !== null) {
          markerPosRef.current = base;
          const canvas = canvasRef.current;
          if (canvas !== null && kitRef.current !== null) {
            const layout = buildMarkerGizmo(base, kitRef.current, { width: canvas.width, height: canvas.height });
            markerLayoutRef.current = layout;
            setMarkerLayout(layout);
          }
        }
        return;
      }
      // Entity drag: restore the base value and snap the overlay back.
      if (draggingRef.current) {
        event.preventDefault();
        draggingRef.current = false;
        const stage = stageRef.current;
        if (stage !== null) {
          releaseCapture(stage);
        }
        const instr = controllerRef.current.escape();
        if (instr.send !== undefined) {
          sendPreview(instr.send);
        }
        // Snap the overlay back to the base pose (unchanged in the refs — the poll was frozen mid-drag).
        const canvas = canvasRef.current;
        if (canvas !== null && kitRef.current !== null && transformRef.current !== null) {
          const layout = buildGizmo(frameRef.current?.world ?? transformRef.current, kitRef.current, { width: canvas.width, height: canvas.height }, gizmoModeRef.current);
          layoutRef.current = layout;
          setGizmoLayout(layout);
        }
        return;
      }
      // Not dragging: cancel an armed placement (the click is disarmed, the marker unchanged).
      if (armedRef.current) {
        event.preventDefault();
        setArmed(false);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [sendPreview]);

  // Edit-mode gizmo/pointer keys. Window-level, never while a text field is focused, and always
  // rejecting ctrl/meta/alt — play mode keeps these keys for `input.inject`, so this is edit-only.
  // W/E/R select translate/rotate/scale (plain, no shift). P arms the 3D-pointer placement; Shift+P
  // clears the marker — both handled before the shift guard that gates W/E/R.
  useEffect(() => {
    if (mode !== 'edit') {
      return;
    }
    const onKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (event.ctrlKey || event.metaKey || event.altKey || isTextEditingTarget(event.target)) {
        return;
      }
      // Not mid-drag (either gizmo): a session solves for the mode/marker it began with, and arming
      // mid-drag would be nonsensical. Nor while the fly look is held, where W and E are the camera's
      // own movement keys and would otherwise also switch the gizmo underneath it.
      if (draggingRef.current || draggingMarkerRef.current || flyingRef.current) {
        return;
      }
      const key = event.key.toLowerCase();
      if (key === 'p') {
        event.preventDefault();
        if (event.shiftKey) {
          clearMarker();
          onPointerChangeRef.current(null);
          setArmed(false);
        } else {
          setArmed(true);
        }
        return;
      }
      if (event.shiftKey) {
        return;
      }
      switch (key) {
        case 'w':
          setGizmoMode('translate');
          break;
        case 'e':
          setGizmoMode('rotate');
          break;
        case 'r':
          setGizmoMode('scale');
          break;
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [mode, clearMarker]);

  const frameMs = status.fps > 0 ? (1000 / status.fps).toFixed(1) : '—';
  const hint =
    mode === 'edit'
      ? armed
        ? 'click to place the 3D pointer · Esc to cancel'
        : 'click to select · drag a handle to transform · W/E/R · P to point · G for the game camera'
      : 'click the viewport, then use the keyboard';

  return (
    <>
      <div
        className="viewport"
        ref={stageRef}
        tabIndex={0}
        role="application"
        aria-label="Game viewport (focus to send keyboard input)"
        data-mode={mode}
        data-armed={armed ? 'true' : undefined}
        data-selection={selection === null ? undefined : entityKey(selection)}
        onKeyDown={handleKey}
        onKeyUp={handleKey}
        onClick={handleStageClick}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={finishDrag}
        onPointerCancel={finishDrag}
        onPointerLeave={handlePointerLeave}
        onWheel={camera.handlers.onWheel}
        onContextMenu={handleContextMenu}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
      >
        <canvas ref={canvasRef} className="viewport-canvas" />
        {connection === 'live' && !status.receiving && <div className="viewport-waiting">waiting for frames…</div>}
        {connection !== 'live' && (
          <div className={connection === 'failed' ? 'viewport-reconnecting viewport-disconnected' : 'viewport-reconnecting'}>
            {connection === 'failed' ? 'connection lost' : 'reconnecting…'}
          </div>
        )}

        {overlayRect !== null && (
          <div
            className="viewport-selection"
            style={{ left: overlayRect.x, top: overlayRect.y, width: overlayRect.w, height: overlayRect.h }}
          />
        )}

        {gizmoLayout !== null && canvasBox !== null && <GizmoOverlay layout={gizmoLayout} box={canvasBox} />}
        {markerLayout !== null && canvasBox !== null && (
          <GizmoOverlay layout={markerLayout} box={canvasBox} variant="pointer" />
        )}

        {armed && <div className="vp-overlay vp-arm">placing pointer</div>}

        <div className="vp-overlay vp-tl">
          {scenePath ?? 'scene'} <span className="sep">·</span> {mode}
          {/* The camera chip. Its own pointer events stop here: the stage beneath it picks on a click
              and hit-tests gizmo handles on a press, and neither is what pressing a button means. */}
          {mode === 'edit' && (
            <>
              <span className="sep">·</span>
              <button
                type="button"
                className={camera.mode === 'view' ? 'vp-cam active' : 'vp-cam'}
                title="G — flip between the free view camera and the scene's own camera"
                onPointerDown={(event) => event.stopPropagation()}
                onClick={(event) => {
                  event.stopPropagation();
                  camera.toggleMode();
                }}
              >
                {camera.mode === 'view' ? 'View' : 'Game'}
              </button>
              {camera.flying && <span className="num">{camera.flySpeed.toFixed(1)} u/s</span>}
            </>
          )}
        </div>
        <div className={status.receiving ? 'vp-overlay vp-tr ok' : 'vp-overlay vp-tr'}>
          <span className="num">{frameMs} ms</span>
          <span className="sep">·</span>
          <span className="num">{status.fps} fps</span>
          <span className="sep">·</span>
          <span className="num">{formatLag(status.lag)}</span>
        </div>

        {focused && mode === 'play' && <div className="vp-capture">input capture</div>}

        {(pickError !== null || camera.error !== null) && (
          <div className="vp-pickerror">
            {pickError !== null && <DiagnosticBanner diagnostic={pickError} onDismiss={() => setPickError(null)} />}
            {camera.error !== null && <DiagnosticBanner diagnostic={camera.error} onDismiss={camera.dismissError} />}
          </div>
        )}
      </div>

      <div className="editorbar">
        <label>
          max fps
          <select
            value={maxFps === null ? 'uncapped' : String(maxFps)}
            onChange={(event) => setMaxFps(event.target.value === 'uncapped' ? null : Number(event.target.value))}
          >
            {FPS_CAPS.map((cap) => (
              <option key={cap === null ? 'uncapped' : cap} value={cap === null ? 'uncapped' : String(cap)}>
                {cap === null ? 'uncapped' : `${cap} fps`}
              </option>
            ))}
          </select>
        </label>

        <label>
          stream size
          <select
            value={serializeStreamSize(streamSize)}
            onChange={(event) => setStreamSize(parseStreamSize(event.target.value))}
          >
            {STREAM_SIZES.map((size) => (
              <option key={serializeStreamSize(size)} value={serializeStreamSize(size)}>
                {describeStreamSize(size)}
              </option>
            ))}
          </select>
        </label>

        {mode === 'edit' && (
          <div className="gizmo-toolbar">
            <div className="gizmo-modes">
              {GIZMO_MODES.map((m) => (
                <button
                  key={m}
                  type="button"
                  className={m === gizmoMode ? 'mode-button active' : 'mode-button'}
                  onClick={() => setGizmoMode(m)}
                >
                  {m}
                </button>
              ))}
            </div>
            <div className="gizmo-snaps">
              <label>
                grid
                <input
                  type="number"
                  min={0}
                  step="any"
                  value={snap.translateStep}
                  onChange={(e) => setSnap((s) => ({ ...s, translateStep: snapStep(e.target.value) }))}
                />
              </label>
              <label>
                deg
                <input
                  type="number"
                  min={0}
                  step="any"
                  value={snap.rotateStepDeg}
                  onChange={(e) => setSnap((s) => ({ ...s, rotateStepDeg: snapStep(e.target.value) }))}
                />
              </label>
              <label>
                scale
                <input
                  type="number"
                  min={0}
                  step="any"
                  value={snap.scaleStep}
                  onChange={(e) => setSnap((s) => ({ ...s, scaleStep: snapStep(e.target.value) }))}
                />
              </label>
            </div>
          </div>
        )}

        <span className="editorbar-hint">{hint}</span>
      </div>
    </>
  );
}
